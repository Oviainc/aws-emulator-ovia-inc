// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the COVERAGE of Firewall Manager's administrator gate
//! over the resource-level tagging operations: they are outside it.
//!
//! The rule asserted here, in one sentence: a Firewall Manager resource-level
//! tagging operation names its target by the model's generic `ResourceArn` and
//! nothing else, so it is answered from that ARN alone — the ARN's FORM first (a
//! string that is not an ARN is HTTP 400 `ValidationException`, whose message is
//! built from the request and names the offending value), then the ARN's
//! RESOLUTION against the resources this service stores (an ARN naming none of
//! them is the modelled HTTP 400 `ResourceNotFoundException`), otherwise the tag
//! set is reported or written — and that answer does not depend on whether the
//! organisation holds a Firewall Manager default-administrator association.
//!
//! Which operations: the three whose only resource identifier is that generic
//! `ResourceArn` — `ListTagsForResource`, `TagResource` and `UntagResource`.
//! None of them declares an authorization error (their modelled unions are
//! `InternalError` / `InvalidInput` / `InvalidOperation` / `ResourceNotFound`,
//! plus `LimitExceeded` on the writer), so no tagging answer may carry the
//! `AccessDeniedException` code the organisation-scoped operations are refused
//! with — the suites `administrator_gated_operations` and
//! `policy_administrator_gate` are what assert that gate where it does apply.
//!
//! The table in [`arn_cases`] is the class the rule ranges over, crossed with
//! every one of the three operations: six strings that are not ARNs (no `arn:`
//! prefix, too few colon-separated fields, an empty partition, service or
//! resource), five well-formed ARNs that name nothing (one per taggable resource
//! kind, plus one leaving Region and account empty the way a global ARN does)
//! and two ARNs of resources the store does hold. Every row is driven in more
//! than one Region, with no association and with one, so a Region-dependent or
//! association-dependent answer could not pass.
//!
//! The wire harness is [`crate::policy_lifecycle`]'s, so these assertions are
//! about the same deserialize → handle → serialize walk a client drives.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{
    associate_admin, error_message, seed_apps_list, seed_protocols_list, with_store,
};
use crate::policy_lifecycle::{call_in, emulator_with_engine, error_code, ACCOUNT, REGION};
use crate::tagging_and_managing_admins::{list_tags_in, tag_list};

/// Regions every row is asserted in: the suite's own Region plus two others, so
/// an answer that moved with the Region could not pass.
const REGIONS: &[&str] = &[REGION, "eu-central-1", "ap-southeast-4"];

/// A `ListId` (`com.amazonaws.fms#ListId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) for the seeded protocols list.
const PROTOCOLS_LIST_ID: &str = "33333333-4444-5555-6666-777777777777";

/// A second well-formed `ListId`, for the seeded applications list.
const APPS_LIST_ID: &str = "88888888-9999-aaaa-bbbb-cccccccccccc";

/// A `ListId` that names no list at all.
const UNKNOWN_LIST_ID: &str = "cccccccc-dddd-eeee-ffff-000000000000";

/// A `PolicyId` (`com.amazonaws.fms#PolicyId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A `Base62Id` (`com.amazonaws.fms#Base62Id`: 22 characters matching
/// `^[a-z0-9A-Z]{22}$`) that names no resource set.
const UNKNOWN_RESOURCE_SET_ID: &str = "0123456789AbCdEfGhIjKl";

/// The tag key the write rows add and the removal rows name, so one row's
/// request can be built for any of the three operations.
const ROW_KEY: &str = "gate.key";

/// The operations whose only resource identifier is the model's generic
/// `ResourceArn`, and which therefore follow the rule under test.
const TAGGING_OPERATIONS: [&str; 3] = ["ListTagsForResource", "TagResource", "UntagResource"];

/// What a tagging call answers for one `ResourceArn` once that ARN is the only
/// thing deciding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The ARN names a stored resource: the call is answered.
    Answered,
    /// The string is not an ARN at all: HTTP 400 `ValidationException`, whose
    /// message names the offending value.
    Malformed,
    /// The ARN is well formed and names no stored resource: the modelled HTTP
    /// 400 `ResourceNotFoundException`.
    NotFound,
}

/// One row of the table: a `ResourceArn` and the answer every tagging operation
/// must give for it.
struct ArnCase {
    case: &'static str,
    arn: String,
    expect: Expect,
}

/// The `ResourceArn` class the rule is asserted over.
///
/// The rows differ in the FORM of the value (not an ARN, an ARN missing fields,
/// an ARN with an empty field, a complete ARN) and in what a complete ARN names
/// (nothing, of each taggable kind this service stores; a stored protocols list;
/// a stored applications list).
fn arn_cases(protocols_list_arn: &str, apps_list_arn: &str) -> Vec<ArnCase> {
    vec![
        ArnCase {
            case: "a string with no arn: prefix",
            arn: "this-is-not-an-arn-at-all".to_string(),
            expect: Expect::Malformed,
        },
        ArnCase {
            case: "the bare word arn",
            arn: "arn".to_string(),
            expect: Expect::Malformed,
        },
        ArnCase {
            case: "an ARN truncated after the Region",
            arn: format!("arn:aws:fms:{REGION}"),
            expect: Expect::Malformed,
        },
        ArnCase {
            case: "an ARN naming no partition",
            arn: format!("arn::fms:{REGION}:{ACCOUNT}:policy/{UNKNOWN_POLICY_ID}"),
            expect: Expect::Malformed,
        },
        ArnCase {
            case: "an ARN naming no service",
            arn: format!("arn:aws::{REGION}:{ACCOUNT}:policy/{UNKNOWN_POLICY_ID}"),
            expect: Expect::Malformed,
        },
        ArnCase {
            case: "an ARN naming no resource",
            arn: format!("arn:aws:fms:{REGION}:{ACCOUNT}:"),
            expect: Expect::Malformed,
        },
        ArnCase {
            case: "a policy ARN naming no policy",
            arn: state::policy_arn(REGION, UNKNOWN_POLICY_ID),
            expect: Expect::NotFound,
        },
        ArnCase {
            case: "a resource-set ARN naming no resource set",
            arn: state::resource_set_arn(REGION, UNKNOWN_RESOURCE_SET_ID),
            expect: Expect::NotFound,
        },
        ArnCase {
            case: "a protocols-list ARN naming no list",
            arn: state::protocols_list_arn(REGION, UNKNOWN_LIST_ID),
            expect: Expect::NotFound,
        },
        ArnCase {
            case: "an applications-list ARN naming no list",
            arn: state::apps_list_arn(REGION, UNKNOWN_LIST_ID),
            expect: Expect::NotFound,
        },
        ArnCase {
            // Region and account id are empty, as a global resource's ARN leaves
            // them: well formed, so this is a lookup that finds nothing rather
            // than a malformed value.
            case: "a well-formed ARN leaving Region and account empty",
            arn: format!("arn:aws:fms:::policy/{UNKNOWN_POLICY_ID}"),
            expect: Expect::NotFound,
        },
        ArnCase {
            case: "the ARN of a stored protocols list",
            arn: protocols_list_arn.to_string(),
            expect: Expect::Answered,
        },
        ArnCase {
            case: "the ARN of a stored applications list",
            arn: apps_list_arn.to_string(),
            expect: Expect::Answered,
        },
    ]
}

/// The request body for `operation` naming `arn`: each of the three operations
/// carries the same `ResourceArn` and its own tag member.
fn request_for(operation: &str, arn: &str) -> Value {
    match operation {
        "ListTagsForResource" => json!({ "ResourceArn": arn }),
        "TagResource" => json!({
            "ResourceArn": arn,
            "TagList": tag_list(&[(ROW_KEY, "gate.value")]),
        }),
        "UntagResource" => json!({ "ResourceArn": arn, "TagKeys": [ROW_KEY] }),
        other => panic!("{other} is not one of the resource-level tagging operations"),
    }
}

/// A fresh emulator holding one taggable resource of two kinds, seeded through
/// the store the way their own writers write them — both `PutProtocolsList` and
/// `PutAppsList` ARE administrator-gated, so seeding is what gives the table an
/// existing resource to name without associating an administrator.
fn emulator_holding_taggable_resources() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    with_store(&engine, |store| {
        seed_protocols_list(store, PROTOCOLS_LIST_ID, "gate.protocols", &["TCP"], false);
        seed_apps_list(store, REGION, APPS_LIST_ID, "gate.apps", &[], false);
    });
    (router, engine)
}

/// Drive one (operation, ARN) row in `region` and assert the ARN alone decided
/// the answer.
async fn assert_arn_decides(
    router: &MultiServiceRouter,
    operation: &str,
    row: &ArnCase,
    region: &str,
) {
    let case = format!("{operation} with {} in {region}", row.case);
    let (status, body) = call_in(router, operation, &request_for(operation, &row.arn), region).await;
    match row.expect {
        Expect::Answered => assert_eq!(
            status, 200,
            "case {case:?}: the ARN names a stored resource, so the call is answered, got \
             {status} with {body}"
        ),
        Expect::Malformed => {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: a value that is not an ARN is refused on its form, got {status} \
                 with {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(row.arn.as_str()),
                "case {case:?}: the form refusal is built from the request, so it names the \
                 offending value {:?}, got {message:?}",
                row.arn
            );
        }
        Expect::NotFound => assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a well-formed ARN naming nothing is the modelled not-found, got \
             {status} with {body}"
        ),
    }
}

/// With NO administrator association, every tagging call is answered from its
/// `ResourceArn`: the malformed values on their form, the well-formed ARNs on
/// what they resolve to — never with the organisation-scoped authorization
/// refusal, which none of these operations models.
#[tokio::test]
async fn tagging_is_answered_from_the_arn_with_no_administrator_association() {
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

        for row in arn_cases(&protocols_arn, &apps_arn) {
            for operation in TAGGING_OPERATIONS {
                assert_arn_decides(&router, operation, &row, region).await;
            }
        }
    }
}

/// Associating an administrator changes NOTHING about the answers: the same
/// table, driven with the association in place, answers exactly as it does
/// without one — which is what shows the administrator gate does not cover these
/// operations rather than merely being open.
#[tokio::test]
async fn an_administrator_association_changes_no_tagging_answer() {
    let protocols_arn = state::protocols_list_arn(REGION, PROTOCOLS_LIST_ID);
    let apps_arn = state::apps_list_arn(REGION, APPS_LIST_ID);

    for region in REGIONS {
        let (router, _engine) = emulator_holding_taggable_resources();
        associate_admin(&router).await;

        for row in arn_cases(&protocols_arn, &apps_arn) {
            for operation in TAGGING_OPERATIONS {
                assert_arn_decides(&router, operation, &row, region).await;
            }
        }
    }
}

/// The tag set a caller writes with no administrator association is the tag set
/// it reads back, and the keys it removes are gone — the whole round trip
/// served without any association, on every Region the table is asserted in.
#[tokio::test]
async fn tags_written_with_no_association_are_read_back_and_removed() {
    let arn = state::protocols_list_arn(REGION, PROTOCOLS_LIST_ID);
    let written = [("stage", "test"), ("team", "net")];

    for region in REGIONS {
        let (router, _engine) = emulator_holding_taggable_resources();

        let (status, body) = call_in(
            &router,
            "TagResource",
            &json!({ "ResourceArn": &arn, "TagList": tag_list(&written) }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: a tag write needs no administrator association, got {status} with \
             {body}"
        );

        let (status, code, held) = list_tags_in(&router, &arn, region).await;
        assert_eq!(
            (status, code.as_str(), held.as_slice()),
            (
                200,
                "",
                [
                    ("stage".to_string(), "test".to_string()),
                    ("team".to_string(), "net".to_string()),
                ]
                .as_slice()
            ),
            "region {region}: the written tag set must be read back, got {status} / {code:?} / \
             {held:?}"
        );

        let (status, body) = call_in(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": &arn, "TagKeys": ["stage"] }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: a tag removal needs no administrator association, got {status} with \
             {body}"
        );

        let (status, code, left) = list_tags_in(&router, &arn, region).await;
        assert_eq!(
            (status, code.as_str(), left.as_slice()),
            (
                200,
                "",
                [("team".to_string(), "net".to_string())].as_slice()
            ),
            "region {region}: exactly the named key must be gone, got {status} / {code:?} / \
             {left:?}"
        );
    }
}

/// A refused tagging call writes NOTHING: after every malformed and every
/// naming-nothing row has been refused, the stored resources carry no tags, the
/// ARN-keyed tag index holds no entry, and the ARNs that named nothing still
/// name nothing.
#[tokio::test]
async fn a_refused_tagging_call_leaves_no_tag_state_behind() {
    let protocols_arn = state::protocols_list_arn(REGION, PROTOCOLS_LIST_ID);
    let apps_arn = state::apps_list_arn(REGION, APPS_LIST_ID);
    let (router, engine) = emulator_holding_taggable_resources();

    for row in arn_cases(&protocols_arn, &apps_arn) {
        if row.expect == Expect::Answered {
            continue;
        }
        for operation in TAGGING_OPERATIONS {
            assert_arn_decides(&router, operation, &row, REGION).await;
        }
    }

    with_store(&engine, |store| {
        for arn in [&protocols_arn, &apps_arn] {
            let held = store.tags_for_arn(arn);
            assert!(
                held.as_ref().is_some_and(|tags| tags.is_empty()),
                "a refused tagging call must leave {arn:?} carrying no tags, got {held:?}"
            );
        }
        assert!(
            store.tags_by_arn.is_empty(),
            "a refused tagging call must write no tag index entry, got {:?}",
            store.tags_by_arn
        );
    });

    for arn in [&protocols_arn, &apps_arn] {
        let (status, code, tags) = list_tags_in(&router, arn, REGION).await;
        assert_eq!(
            (status, code.as_str(), tags.as_slice()),
            (200, "", [].as_slice()),
            "the resource {arn:?} must still carry no tags, got {status} / {code:?} / {tags:?}"
        );
    }
    let unknown = state::policy_arn(REGION, UNKNOWN_POLICY_ID);
    let (status, code, _) = list_tags_in(&router, &unknown, REGION).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourceNotFoundException"),
        "a refused tagging call must have created no resource, got {status} / {code:?}"
    );
}
