// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the administrator gate in front of Firewall Manager's
//! TAGGING operations (`TagResource`, `ListTagsForResource`).
//!
//! The rule asserted here, in one sentence: an organisation-scoped Firewall
//! Manager operation is answered only while the organisation has a Firewall
//! Manager default-administrator association in the called account/Region, and
//! that decision is made BEFORE any identifier is resolved and before any
//! payload member is validated — so with no association every such call answers
//! the one HTTP 400 `AccessDeniedException` whose `Message` names the calling
//! account and the Region the request was signed for, and writes nothing.
//!
//! The table in [`gate_rows`] is the class the rule ranges over: both tagging
//! operations crossed with the payload forms whose own answers differ from each
//! other once the gate is open — a string that is not an ARN at all
//! (`ValidationException`), a well-formed ARN naming nothing
//! (`ResourceNotFoundException`), the ARN of a resource that DOES exist (200), a
//! `TagList` repeating a key (`InvalidInputException`) and an empty `TagList`
//! (200). Each row is driven twice: with no association, where every row must
//! answer the SAME refusal, and with an administrator associated, where each row
//! answers its own code — which is what shows the authorization decision comes
//! first rather than after the lookup or the validation.
//!
//! Every row is asserted in more than one Region, so the refusal's message can be
//! seen to be composed from the request.
//!
//! The wire harness is [`crate::policy_lifecycle`]'s, so these assertions are
//! about the same deserialize → handle → serialize walk a client drives.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::admin_account_disassociation::wait_out_the_disassociation;
use crate::administrator_gated_operations::{
    associate_admin, seed_apps_list, seed_protocols_list, with_store,
};
use crate::error_message_members::message_member;
use crate::policy_lifecycle::{call_in, emulator_with_engine, error_code, ACCOUNT, REGION};
use crate::tagging_and_managing_admins::{list_tags, minimal_policy, tag_list, tag_pairs};

/// Regions every row is asserted in: the suite's own Region plus two others, so
/// a message composed from the request can be told from a constant.
const REGIONS: &[&str] = &[REGION, "eu-central-1", "ap-southeast-4"];

/// A `ListId` (`com.amazonaws.fms#ListId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) for the seeded protocols list.
const PROTOCOLS_LIST_ID: &str = "33333333-4444-5555-6666-777777777777";

/// A second well-formed `ListId`, for the seeded applications list.
const APPS_LIST_ID: &str = "88888888-9999-aaaa-bbbb-cccccccccccc";

/// A `PolicyId` (`com.amazonaws.fms#PolicyId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A `Base62Id` (`com.amazonaws.fms#Base62Id`: 22 characters matching
/// `^[a-z0-9A-Z]{22}$`) that names no resource set.
const UNKNOWN_RESOURCE_SET_ID: &str = "0123456789AbCdEfGhIjKl";

/// What a row answers once the gate is OPEN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The call succeeds.
    Ok,
    /// The call is refused with this modelled error code.
    Code(&'static str),
}

/// One row of the gate table: the operation, the request, and the answer an
/// AUTHORISED caller gets for it.
struct Row {
    case: String,
    operation: &'static str,
    body: Value,
    with_admin: Expect,
}

/// The tagging requests the rule is asserted over, built against the ARNs of the
/// resources the emulator holds.
///
/// The rows differ in operation, in the FORM of the `ResourceArn` (not an ARN at
/// all, well formed but naming nothing, naming a stored resource of two
/// different kinds) and in the `TagList` (absent, empty, one tag, a repeated
/// key) — so no two rows share the answer an authorised caller gets.
fn gate_rows(protocols_list_arn: &str, apps_list_arn: &str) -> Vec<Row> {
    let unknown_policy_arn = state::policy_arn(REGION, UNKNOWN_POLICY_ID);
    let unknown_resource_set_arn = state::resource_set_arn(REGION, UNKNOWN_RESOURCE_SET_ID);
    let not_an_arn = "this-is-not-an-arn-at-all";
    vec![
        Row {
            case: "ListTagsForResource with a string that is not an ARN".to_string(),
            operation: "ListTagsForResource",
            body: json!({ "ResourceArn": not_an_arn }),
            with_admin: Expect::Code("ValidationException"),
        },
        Row {
            case: "TagResource with a string that is not an ARN".to_string(),
            operation: "TagResource",
            body: json!({ "ResourceArn": not_an_arn, "TagList": tag_list(&[("k", "v")]) }),
            with_admin: Expect::Code("ValidationException"),
        },
        Row {
            case: "ListTagsForResource with a policy ARN naming no policy".to_string(),
            operation: "ListTagsForResource",
            body: json!({ "ResourceArn": unknown_policy_arn }),
            with_admin: Expect::Code("ResourceNotFoundException"),
        },
        Row {
            case: "TagResource with a resource-set ARN naming nothing".to_string(),
            operation: "TagResource",
            body: json!({
                "ResourceArn": unknown_resource_set_arn,
                "TagList": tag_list(&[("stage", "test")]),
            }),
            with_admin: Expect::Code("ResourceNotFoundException"),
        },
        Row {
            case: "ListTagsForResource with the ARN of a stored protocols list".to_string(),
            operation: "ListTagsForResource",
            body: json!({ "ResourceArn": protocols_list_arn }),
            with_admin: Expect::Ok,
        },
        Row {
            case: "TagResource with the ARN of a stored protocols list".to_string(),
            operation: "TagResource",
            body: json!({
                "ResourceArn": protocols_list_arn,
                "TagList": tag_list(&[("owner", "ops")]),
            }),
            with_admin: Expect::Ok,
        },
        Row {
            case: "ListTagsForResource with the ARN of a stored applications list".to_string(),
            operation: "ListTagsForResource",
            body: json!({ "ResourceArn": apps_list_arn }),
            with_admin: Expect::Ok,
        },
        Row {
            case: "TagResource with an empty TagList on a stored resource".to_string(),
            operation: "TagResource",
            body: json!({ "ResourceArn": apps_list_arn, "TagList": tag_list(&[]) }),
            with_admin: Expect::Ok,
        },
        Row {
            case: "TagResource with a TagList repeating a key".to_string(),
            operation: "TagResource",
            body: json!({
                "ResourceArn": apps_list_arn,
                "TagList": tag_list(&[("dup", "1"), ("dup", "2")]),
            }),
            with_admin: Expect::Code("InvalidInputException"),
        },
    ]
}

/// A fresh emulator holding one taggable resource of two kinds, seeded through
/// the store the way their own writers write them — both `PutProtocolsList` and
/// `PutAppsList` are administrator-gated themselves, so seeding is what gives
/// the table an EXISTING resource to name without first opening the gate under
/// test.
fn emulator_holding_taggable_resources() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    with_store(&engine, |store| {
        seed_protocols_list(store, PROTOCOLS_LIST_ID, "gate.protocols", &["TCP"], false);
        seed_apps_list(store, REGION, APPS_LIST_ID, "gate.apps", &[], false);
    });
    (router, engine)
}

/// With no administrator association, EVERY tagging call answers HTTP 400
/// `AccessDeniedException` — whatever its `ResourceArn` and `TagList` carry, and
/// whichever Region it is signed for — with a `Message` built from the request.
#[tokio::test]
async fn no_administrator_association_refuses_every_tagging_call_with_access_denied() {
    let protocols_arn = state::protocols_list_arn(REGION, PROTOCOLS_LIST_ID);
    let apps_arn = state::apps_list_arn(REGION, APPS_LIST_ID);

    for region in REGIONS {
        let (router, _engine) = emulator_holding_taggable_resources();

        // The precondition, read through an implemented operation rather than
        // assumed: nothing is associated, which GetAdminAccount reports as
        // ResourceNotFoundException.
        let (status, body) = call_in(&router, "GetAdminAccount", &json!({}), region).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "region {region}: expected no administrator association, got {status} with {body}"
        );

        for row in gate_rows(&protocols_arn, &apps_arn) {
            let case = &row.case;
            let (status, body) = call_in(&router, row.operation, &row.body, region).await;
            assert_eq!(
                status, 400,
                "case {case:?} in {region}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?} in {region}: the authorization decision comes first, so the \
                 answer must be the administrator refusal, got {body}"
            );
            let Some(message) = message_member(&body) else {
                panic!(
                    "case {case:?} in {region}: the refusal must carry its message under the \
                     member name this service's error shapes declare (`Message`), got {body}"
                );
            };
            assert!(
                message.contains(ACCOUNT) && message.contains(region),
                "case {case:?} in {region}: the refusal's message is composed from the request, \
                 so it must name the calling account and the Region the administrator was looked \
                 for in, got {message:?}"
            );
        }
    }
}

/// With an administrator associated, each row answers its OWN code — the
/// malformed ARN a `ValidationException`, the ARN naming nothing a
/// `ResourceNotFoundException`, the repeated key an `InvalidInputException` and
/// the stored resource a 200. Taken with the test above, this is what shows the
/// gate is decided BEFORE the lookup and before the payload is validated: the
/// same requests that differ here are indistinguishable while the gate is shut.
#[tokio::test]
async fn an_associated_administrator_gets_each_tagging_row_answered_on_its_own_merits() {
    let protocols_arn = state::protocols_list_arn(REGION, PROTOCOLS_LIST_ID);
    let apps_arn = state::apps_list_arn(REGION, APPS_LIST_ID);

    for region in REGIONS {
        let (router, _engine) = emulator_holding_taggable_resources();
        associate_admin(&router).await;

        for row in gate_rows(&protocols_arn, &apps_arn) {
            let case = &row.case;
            let (status, body) = call_in(&router, row.operation, &row.body, region).await;
            match row.with_admin {
                Expect::Ok => assert_eq!(
                    status, 200,
                    "case {case:?} in {region}: an authorised caller must be answered, got \
                     {status} with {body}"
                ),
                Expect::Code(expected) => {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (400, expected),
                        "case {case:?} in {region}: expected 400 {expected}, got {status} with \
                         {body}"
                    );
                }
            }
        }
    }
}

/// A refused tagging call writes NOTHING: after the whole table has been refused
/// with no association, associating an administrator shows the stored resources
/// still carrying no tags and the ARNs that named nothing still naming nothing.
#[tokio::test]
async fn a_refused_tagging_call_leaves_no_tag_state_behind() {
    let protocols_arn = state::protocols_list_arn(REGION, PROTOCOLS_LIST_ID);
    let apps_arn = state::apps_list_arn(REGION, APPS_LIST_ID);
    let (router, engine) = emulator_holding_taggable_resources();

    for row in gate_rows(&protocols_arn, &apps_arn) {
        let (status, body) = call_in(&router, row.operation, &row.body, REGION).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {:?}: expected the tagging call to be refused, got {status} with {body}",
            row.case
        );
    }

    // Nothing reached the store: neither the resources the table named nor the
    // ARN-keyed tag index knows about any of those writes.
    with_store(&engine, |store| {
        for arn in [&protocols_arn, &apps_arn] {
            let held = store.tags_for_arn(arn);
            assert!(
                held.as_ref().is_some_and(|tags| tags.is_empty()),
                "a refused TagResource must leave {arn:?} carrying no tags, got {held:?}"
            );
        }
        assert!(
            store.tags_by_arn.is_empty(),
            "a refused TagResource must write no tag index entry, got {:?}",
            store.tags_by_arn
        );
    });

    associate_admin(&router).await;
    for arn in [&protocols_arn, &apps_arn] {
        let (status, code, tags) = list_tags(&router, arn).await;
        assert_eq!(
            (status, code.as_str(), tags.as_slice()),
            (200, "", [].as_slice()),
            "the resource {arn:?} must still carry no tags, got {status} / {code:?} / {tags:?}"
        );
    }
    let (status, code, _) = list_tags(&router, &state::policy_arn(REGION, UNKNOWN_POLICY_ID)).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourceNotFoundException"),
        "a refused TagResource must have created no resource, got {status} / {code:?}"
    );
}

/// The gate CLOSES again when the association is torn down: tagging works while
/// an administrator holds the slot, is refused once the tear-down the clock owed
/// has completed, and the tag set the resource held is exactly what a
/// re-associated administrator reads back — so the refused writes changed
/// nothing.
#[tokio::test]
async fn tearing_the_association_down_closes_the_gate_again() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    // A policy created by an authorised caller, carrying the tags the rest of
    // this test compares against.
    let held = tag_list(&[("stage", "test"), ("team", "net")]);
    let (status, created) = call_in(
        &router,
        "PutPolicy",
        &json!({ "Policy": minimal_policy("gated.policy"), "TagList": held }),
        REGION,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected PutPolicy to answer 200, got {status} with {created}"
    );
    let arn = created["PolicyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("PutPolicy reported no PolicyArn: {created}"))
        .to_string();

    // While the administrator holds the slot, both tagging operations are
    // answered.
    let (status, code, tags) = list_tags(&router, &arn).await;
    assert_eq!(
        (status, code.as_str(), tags.as_slice()),
        (200, "", tag_pairs(&held).as_slice()),
        "an authorised caller must read the created tag set, got {status} / {code:?} / {tags:?}"
    );

    // Tear the association down and wait out the span the clock owes it.
    let (status, body) = call_in(&router, "DisassociateAdminAccount", &json!({}), REGION).await;
    assert_eq!(
        status, 200,
        "the holder's disassociation must be accepted, got {status} with {body}"
    );
    wait_out_the_disassociation();

    // The slot is empty again, so the gate answers the refusal — and the write
    // it refuses does not reach the policy.
    for (case, operation, request) in [
        (
            "read after tear-down",
            "ListTagsForResource",
            json!({ "ResourceArn": arn }),
        ),
        (
            "write after tear-down",
            "TagResource",
            json!({ "ResourceArn": arn, "TagList": tag_list(&[("stage", "rewritten")]) }),
        ),
    ] {
        let (status, body) = call_in(&router, operation, &request, REGION).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {case:?}: a torn-down association is no administrator, got {status} with {body}"
        );
    }

    // Re-associating shows the tag set untouched by the refused write.
    associate_admin(&router).await;
    let (status, code, tags) = list_tags(&router, &arn).await;
    assert_eq!(
        (status, code.as_str(), tags.as_slice()),
        (200, "", tag_pairs(&held).as_slice()),
        "the refused write must have changed nothing, got {status} / {code:?} / {tags:?}"
    );
}
