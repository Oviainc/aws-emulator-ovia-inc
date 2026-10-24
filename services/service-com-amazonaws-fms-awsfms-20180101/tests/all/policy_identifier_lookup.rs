// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `PolicyId` lookup that decides `GetPolicy` and
//! `DeletePolicy`.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`], with the
//! `X-Amz-Target` header and a SigV4 credential scope, so the whole
//! deserialize → handle → serialize walk runs and the assertions are about the
//! WIRE response.
//!
//! The rule under test is one sentence: an operation whose input is an opaque
//! resource identifier answers from the store that holds that resource — the
//! resource when the identifier names one, and the modelled
//! `ResourceNotFoundException` (HTTP 400) when it names none — so its answer
//! depends on the identifier and the stored state alone, never on an
//! organisation-level authorization condition this service's model declares no
//! error for.
//!
//! The table in [`well_formed_policy_ids`] is the class the rule ranges over:
//! every row satisfies the model's constraint on `com.amazonaws.fms#PolicyId`
//! (`@length` min 36 max 36, pattern `^[a-z0-9A-Z-]{36}$`) and the rows differ in
//! FORM — a UUID in both casings, no hyphens at all, only hyphens, only digits,
//! and letter/digit/hyphen mixes — because the constraint is all the form a
//! lookup key has to have. Each is asserted against an empty store, against a
//! store that holds other policies, and in three Regions.

use serde_json::{json, Value};

// The wire harness, the store seam and the administrator helper live in one place
// for this test target, so no two suites can drift in how a request is signed,
// how the gate is opened or how the store is read.
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, REGION};

/// Regions every row is asserted in: the suite's own plus two others, so an
/// answer composed from the request can be told from a constant.
const REGIONS: &[&str] = &[REGION, "eu-central-1", "ap-southeast-4"];

/// The `Message` every `ResourceNotFoundException` of this service carries, as
/// the `error_message_members` suite pins it.
const NOT_FOUND_MESSAGE: &str = "The referenced item does not exist.";

/// Well-formed `PolicyId` values no policy in a fresh store carries.
///
/// Source of the catalog: the model's `com.amazonaws.fms#PolicyId` — `@length`
/// min 36 max 36, pattern `^[a-z0-9A-Z-]{36}$` (`smithy_query(query="shape",
/// shape="PolicyId")`). Nothing narrows that to a UUID, so each row below is a
/// legal lookup key of a different form; [`every_row_satisfies_the_model_constraint`]
/// re-checks that against the constraint rather than trusting the literals.
fn well_formed_policy_ids() -> Vec<(&'static str, String)> {
    vec![
        ("a canonical lowercase UUID", "4d8f6c21-7b53-4e0a-9c3d-61ab2f5e7049".to_string()),
        ("the same UUID upper-cased", "4D8F6C21-7B53-4E0A-9C3D-61AB2F5E7049".to_string()),
        ("36 identical lowercase letters", "x".repeat(36)),
        ("36 identical digits", "0".repeat(36)),
        ("36 hyphens", "-".repeat(36)),
        ("a mixed-case letter/digit run", "aB3".repeat(12)),
        ("a letter/digit/hyphen run", "Zz9-".repeat(9)),
    ]
}

/// `PolicyId` values the model's constraint REFUSES: two lengths either side of
/// 36, and a 36-character value carrying a character the pattern excludes.
fn malformed_policy_ids() -> Vec<(&'static str, String)> {
    vec![
        ("one character short of the fixed length", "x".repeat(35)),
        ("one character over the fixed length", "x".repeat(37)),
        (
            "the right length with a character the pattern excludes",
            format!("{}_", "x".repeat(35)),
        ),
    ]
}

/// The message member of an awsJson1_1 error envelope, under either casing: the
/// modelled error shapes of this service spell it `Message`, the synthetic
/// authorization refusal `message`.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A policy document carrying exactly the members the model requires of `Policy`,
/// with a resource type the model lists for a `WAFV2` policy.
fn policy_document(name: &str) -> Value {
    json!({
        "PolicyName": name,
        "SecurityServicePolicyData": { "Type": "WAFV2" },
        "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
        "ExcludeResourceTags": false,
        "RemediationEnabled": false
    })
}

/// Create one policy through the API and return `(PolicyId, the Policy document
/// PutPolicy answered, the ARN it minted)`. The caller must have associated an
/// administrator first, because writing a policy IS organisation-scoped.
async fn create_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> (String, Value, String) {
    let (status, body) = call(
        router,
        "PutPolicy",
        &json!({ "Policy": policy_document(name) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected PutPolicy to answer 200 for an associated administrator, \
         got {status} with {body}"
    );
    let policy_id = body["Policy"]["PolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: PutPolicy minted no PolicyId: {body}"))
        .to_string();
    let arn = body["PolicyArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: PutPolicy minted no PolicyArn: {body}"))
        .to_string();
    (policy_id, body["Policy"].clone(), arn)
}

/// The table's rows really are what the model admits: 36 characters, each of them
/// a lowercase letter, an uppercase letter, a digit or a hyphen.
#[tokio::test]
async fn every_row_satisfies_the_model_constraint() {
    for (case, id) in well_formed_policy_ids() {
        assert_eq!(
            id.chars().count(),
            36,
            "case {case:?}: PolicyId is a fixed 36 characters, {id:?} has {}",
            id.chars().count()
        );
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {case:?}: PolicyId admits only [a-z0-9A-Z-], got {id:?}"
        );
    }
    for (case, id) in malformed_policy_ids() {
        let legal = id.chars().count() == 36
            && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        assert!(
            !legal,
            "case {case:?}: this row must VIOLATE the constraint, got the legal {id:?}"
        );
    }
}

/// With NO administrator association, a `PolicyId` no policy carries is the
/// modelled not-found on both id-keyed policy operations — for every form the
/// constraint admits and in every Region — and never an authorization code,
/// which this operation's modelled error set does not contain.
#[tokio::test]
async fn an_unknown_policy_id_is_not_found_without_an_administrator() {
    let (router, _engine) = emulator_with_engine();

    // The precondition, read through an implemented operation rather than
    // assumed: no administrator is associated.
    let (status, body) = call(&router, "GetAdminAccount", &json!({})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "expected no administrator association, got {status} with {body}"
    );

    for region in REGIONS {
        for (case, id) in well_formed_policy_ids() {
            for (operation, request) in [
                ("GetPolicy", json!({ "PolicyId": id })),
                (
                    "DeletePolicy",
                    json!({ "PolicyId": id, "DeleteAllPolicyResources": true }),
                ),
            ] {
                let (status, body) = call_in(&router, operation, &request, region).await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {case:?} ({id:?}) on {operation} in {region}: expected the modelled \
                     not-found, got {status} with {body}"
                );
                assert_eq!(
                    error_message(&body),
                    NOT_FOUND_MESSAGE,
                    "case {case:?} ({id:?}) on {operation} in {region}: expected the not-found \
                     message, got {body}"
                );
            }
        }
    }
}

/// The not-found is a LOOKUP, not a blanket refusal: on a store that holds
/// policies but has no administrator association, each unknown id is still
/// not-found while the stored ids are served — and the refused deletes removed
/// nothing.
#[tokio::test]
async fn the_lookup_tells_a_stored_policy_from_an_unknown_id() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let mut stored = Vec::new();
    for name in ["first-stored-policy", "second-stored-policy"] {
        stored.push(create_policy(&router, name).await);
    }
    // The precondition the rule is about: a store that holds policies and no
    // administrator association at all.
    with_store(&engine, |store| store.admin_account = None);

    for (case, id) in well_formed_policy_ids() {
        let (status, body) = call(&router, "GetPolicy", &json!({ "PolicyId": id })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?} ({id:?}): an id no stored policy carries is not-found even with \
             policies in the store, got {status} with {body}"
        );
    }

    for (name, (policy_id, created, arn)) in ["first-stored-policy", "second-stored-policy"]
        .iter()
        .zip(stored.iter())
    {
        let (status, got) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a stored policy is reported with no administrator associated, \
             got {status} with {got}"
        );
        assert_eq!(
            &got["Policy"], created,
            "case {name:?}: GetPolicy must report the policy PutPolicy stored, got {}",
            got["Policy"]
        );
        assert_eq!(
            got["PolicyArn"].as_str(),
            Some(arn.as_str()),
            "case {name:?}: GetPolicy must report the ARN PutPolicy minted, got {}",
            got["PolicyArn"]
        );
    }

    let held = with_store(&engine, |store| {
        store.policies.keys().cloned().collect::<Vec<_>>()
    });
    let mut expected: Vec<String> = stored.iter().map(|(id, _, _)| id.clone()).collect();
    expected.sort();
    assert_eq!(
        held, expected,
        "a not-found delete must remove nothing, store holds {held:?}"
    );
}

/// A delete decided by the lookup is PERFORMED: with no administrator
/// association, `DeletePolicy` of a stored id answers 200 and the policy is gone
/// from every reader, while the other stored policy is untouched; deleting the
/// same id twice finds nothing the second time.
#[tokio::test]
async fn deleting_a_stored_policy_id_removes_exactly_that_policy() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let (doomed, _, doomed_arn) = create_policy(&router, "policy-to-delete").await;
    let (survivor, _, _) = create_policy(&router, "policy-to-keep").await;
    with_store(&engine, |store| store.admin_account = None);

    let (status, body) = call(&router, "DeletePolicy", &json!({ "PolicyId": doomed })).await;
    assert_eq!(
        status, 200,
        "case {doomed:?}: a delete of a stored id is performed with no administrator \
         associated, got {status} with {body}"
    );

    let (status, body) = call(&router, "GetPolicy", &json!({ "PolicyId": doomed })).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case {doomed:?}: the deleted policy must be gone, got {status} with {body}"
    );
    let (status, body) = call(&router, "DeletePolicy", &json!({ "PolicyId": doomed })).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case {doomed:?}: a second delete of the same id finds nothing, got {status} with {body}"
    );

    let (status, body) = call(&router, "GetPolicy", &json!({ "PolicyId": survivor })).await;
    assert_eq!(
        status, 200,
        "case {survivor:?}: only the named policy is deleted, got {status} with {body}"
    );

    let (held, tagged) = with_store(&engine, |store| {
        (
            store.policies.keys().cloned().collect::<Vec<_>>(),
            store.tags_by_arn.contains_key(&doomed_arn),
        )
    });
    assert_eq!(
        held,
        vec![survivor.clone()],
        "the delete must remove exactly one policy, store holds {held:?}"
    );
    assert!(
        !tagged,
        "the deleted policy's tag index entry must go with it, {doomed_arn:?} is still indexed"
    );
}

/// A `PolicyId` the model's constraint refuses never reaches the lookup: it is
/// refused as a bad request — the same answer with and without an administrator
/// association, so the refusal is about the request and not about authorization.
#[tokio::test]
async fn an_id_outside_the_constraint_is_refused_as_a_bad_request() {
    for associate in [false, true] {
        let (router, _engine) = emulator_with_engine();
        if associate {
            associate_admin(&router).await;
        }
        for (case, id) in malformed_policy_ids() {
            for (operation, request) in [
                ("GetPolicy", json!({ "PolicyId": id })),
                ("DeletePolicy", json!({ "PolicyId": id })),
            ] {
                let (status, body) = call(&router, operation, &request).await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?} ({id:?}) on {operation} with administrator={associate}: \
                     expected the boundary validator's refusal, got {status} with {body}"
                );
            }
        }
    }
}
