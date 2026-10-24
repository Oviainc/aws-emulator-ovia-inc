// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the administrator gate in front of Firewall Manager's
//! POLICY operations — `PutPolicy`, `GetPolicy`, `DeletePolicy` and
//! `ListPolicies`.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`], with the
//! `X-Amz-Target` header and a SigV4 credential scope, so the whole
//! deserialize → handle → serialize walk runs and the assertions are about the
//! WIRE response.
//!
//! The rule under test is one sentence: policy management is organisation-scoped,
//! so each of these operations decides authorization FIRST — with no active
//! Firewall Manager default-administrator association in the account/Region it
//! answers HTTP 400 `AccessDeniedException`, naming the calling account and the
//! Region the request was signed for, before any identifier is looked up, any
//! payload validated or any state read, written or removed; with an association
//! it judges the request on its own merits.
//!
//! The table in [`policy_calls`] is the class that rule ranges over: the rows
//! differ in operation, in payload form (a create with no identifier, an update
//! of a known and of an unknown id, an id-keyed read and delete, a list with no
//! members, with a page bound and with a foreign pagination token) and in whether
//! the identifier they name exists — and every row is asserted in three Regions.

use serde_json::{json, Value};

// The wire harness, the store seam and the administrator helpers live in one
// place for this test target, so no two suites can drift in how a request is
// signed, how the gate is opened or how the store is read.
use crate::admin_account_disassociation::wait_out_the_disassociation;
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// Regions every row is asserted in: the suite's own plus two others, so an
/// answer composed from the request can be told from a constant.
const REGIONS: &[&str] = &[REGION, "eu-central-1", "ap-southeast-4"];

/// A `PolicyId` (the model's `com.amazonaws.fms#PolicyId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names no policy.
const UNKNOWN_POLICY_ID: &str = "4d8f6c21-7b53-4e0a-9c3d-61ab2f5e7049";

/// A `NextToken` this service never minted, so a served page could only be an
/// invented one.
const FOREIGN_NEXT_TOKEN: &str = "not.a.token.this.service.minted";

/// The message member of an awsJson1_1 error envelope, under either casing: the
/// modelled error shapes of this service spell it `Message`, the synthetic
/// authorization refusal `message`.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A policy document carrying exactly the members the model requires of `Policy`
/// (`PolicyName`, `SecurityServicePolicyData`, `ResourceType`,
/// `ExcludeResourceTags`, `RemediationEnabled`), with a resource type the model
/// lists for a `WAFV2` policy — so nothing here is what a refusal is about.
fn policy_document(name: &str) -> Value {
    json!({
        "PolicyName": name,
        "SecurityServicePolicyData": { "Type": "WAFV2" },
        "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
        "ExcludeResourceTags": false,
        "RemediationEnabled": false
    })
}

/// One row: a name for the assertion messages, the operation, and the request.
struct PolicyCall {
    case: String,
    operation: &'static str,
    body: Value,
}

/// Every policy operation, in the payload forms the rule ranges over.
///
/// `known_policy_id` is a policy the store actually holds, so a row that names it
/// differs from the otherwise identical row naming [`UNKNOWN_POLICY_ID`] ONLY in
/// whether the lookup would have found something — which is what shows the gate
/// runs before the lookup.
fn policy_calls(known_policy_id: &str) -> Vec<PolicyCall> {
    let mut update_known = policy_document("gated-update-of-a-stored-policy");
    update_known["PolicyId"] = json!(known_policy_id);
    update_known["PolicyUpdateToken"] = json!("not-the-current-token");
    let mut update_unknown = policy_document("gated-update-of-nothing");
    update_unknown["PolicyId"] = json!(UNKNOWN_POLICY_ID);
    update_unknown["PolicyUpdateToken"] = json!("not-the-current-token");

    vec![
        PolicyCall {
            case: "PutPolicy creating a policy".to_string(),
            operation: "PutPolicy",
            body: json!({ "Policy": policy_document("gated-create") }),
        },
        PolicyCall {
            case: "PutPolicy creating a policy with tags".to_string(),
            operation: "PutPolicy",
            body: json!({
                "Policy": policy_document("gated-create-with-tags"),
                "TagList": [{ "Key": "stage", "Value": "test" }],
            }),
        },
        PolicyCall {
            case: format!("PutPolicy updating the stored {known_policy_id}"),
            operation: "PutPolicy",
            body: json!({ "Policy": update_known }),
        },
        PolicyCall {
            case: "PutPolicy updating an id that names nothing".to_string(),
            operation: "PutPolicy",
            body: json!({ "Policy": update_unknown }),
        },
        PolicyCall {
            case: format!("GetPolicy reading the stored {known_policy_id}"),
            operation: "GetPolicy",
            body: json!({ "PolicyId": known_policy_id }),
        },
        PolicyCall {
            case: "GetPolicy reading an id that names nothing".to_string(),
            operation: "GetPolicy",
            body: json!({ "PolicyId": UNKNOWN_POLICY_ID }),
        },
        PolicyCall {
            case: format!("DeletePolicy deleting the stored {known_policy_id}"),
            operation: "DeletePolicy",
            body: json!({ "PolicyId": known_policy_id, "DeleteAllPolicyResources": true }),
        },
        PolicyCall {
            case: "DeletePolicy deleting an id that names nothing".to_string(),
            operation: "DeletePolicy",
            body: json!({ "PolicyId": UNKNOWN_POLICY_ID }),
        },
        PolicyCall {
            case: "ListPolicies with no members".to_string(),
            operation: "ListPolicies",
            body: json!({}),
        },
        PolicyCall {
            case: "ListPolicies with a page bound".to_string(),
            operation: "ListPolicies",
            body: json!({ "MaxResults": 1 }),
        },
        PolicyCall {
            case: "ListPolicies with a foreign pagination token".to_string(),
            operation: "ListPolicies",
            body: json!({ "NextToken": FOREIGN_NEXT_TOKEN }),
        },
    ]
}

/// Create one policy through the API and return its `PolicyId`. The caller must
/// have opened the gate first.
async fn create_policy(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> String {
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
    body["Policy"]["PolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: PutPolicy minted no PolicyId: {body}"))
        .to_string()
}

/// With policies in the store but NO administrator association, every policy
/// operation answers the authorization refusal — whatever its payload, whichever
/// Region it is signed for, and whether or not the id it names exists — and the
/// refusal's message is composed from the request.
#[tokio::test]
async fn no_administrator_association_refuses_every_policy_operation() {
    let (router, engine) = emulator_with_engine();
    // A stored policy, so the gate is shown to run BEFORE the lookup: the known
    // and the unknown id are refused identically, and the list has something to
    // report and still reports nothing. Writing it is organisation-scoped too, so
    // it is created with an administrator associated and the association is then
    // removed through the store seam — which is exactly the precondition the rule
    // is about: a store that holds policies but no administrator.
    associate_admin(&router).await;
    let known = create_policy(&router, "stored-before-the-gate-closed").await;
    with_store(&engine, |store| store.admin_account = None);

    let mut per_region: Vec<String> = Vec::new();
    for region in REGIONS {
        let mut messages: Vec<String> = Vec::new();
        for row in policy_calls(&known) {
            let (status, body) = call_in(&router, row.operation, &row.body, region).await;
            assert_eq!(
                status, 400,
                "case {:?} in {region}: expected HTTP 400, got {status} with {body}",
                row.case
            );
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {:?} in {region}: expected the authorization refusal, got {body}",
                row.case
            );
            let message = error_message(&body);
            assert!(
                message.contains(ACCOUNT) && message.contains(region),
                "case {:?} in {region}: the refusal's message is composed from the request, so \
                 it must name the calling account and the Region the administrator was looked \
                 for in, got {message:?}",
                row.case
            );
            messages.push(message);
        }
        messages.dedup();
        assert_eq!(
            messages.len(),
            1,
            "region {region}: every policy operation must answer the SAME refusal, got {messages:?}"
        );
        per_region.push(messages.remove(0));
    }
    let mut distinct = per_region.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        REGIONS.len(),
        "the refusal is composed from the request, so each Region gets its own message, \
         got {per_region:?}"
    );

    // Nothing was written and nothing was removed: the store holds exactly the
    // one policy that was there before the table ran.
    let (stored, arns) = with_store(&engine, |store| {
        (
            store.policies.keys().cloned().collect::<Vec<_>>(),
            store.tags_by_arn.len(),
        )
    });
    assert_eq!(
        stored,
        vec![known.clone()],
        "a refused policy call must neither create nor delete a policy, store holds {stored:?}"
    );
    assert_eq!(
        arns, 1,
        "a refused policy call must not touch the tag index, it holds {arns} entries"
    );

    // And the surviving policy is readable again once the gate is open, which is
    // what says the refused calls changed nothing a client can see.
    associate_admin(&router).await;
    let (status, body) = call(&router, "GetPolicy", &json!({ "PolicyId": known })).await;
    assert_eq!(
        status, 200,
        "the refused DeletePolicy must have deleted nothing, got {status} with {body}"
    );
}

/// On a FRESH store with no association, a refused `PutPolicy` mints nothing:
/// opening the gate afterwards reveals an empty policy list, so no state AWS
/// would never have created is visible to a later reader.
#[tokio::test]
async fn a_refused_policy_call_persists_nothing() {
    let (router, engine) = emulator_with_engine();

    for row in policy_calls(UNKNOWN_POLICY_ID) {
        let (status, body) = call(&router, row.operation, &row.body).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {:?}: expected the authorization refusal, got {status} with {body}",
            row.case
        );
    }

    with_store(&engine, |store| {
        assert!(
            store.policies.is_empty() && store.tags_by_arn.is_empty(),
            "a refused policy call must write nothing, store holds {:?} and {:?}",
            store.policies,
            store.tags_by_arn
        );
    });

    // Read through the API too, so the assertion is not only about the store.
    associate_admin(&router).await;
    let (status, body) = call(&router, "ListPolicies", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListPolicies must answer an associated administrator, got {status} with {body}"
    );
    assert_eq!(
        body["PolicyList"].as_array().map(Vec::len),
        Some(0),
        "every PutPolicy above was refused, so no policy may exist, got {}",
        body["PolicyList"]
    );
    assert!(
        body["NextToken"].is_null(),
        "an empty list has no next page, got {}",
        body["NextToken"]
    );
}

/// With an administrator associated, the same requests are judged on their OWN
/// merits: the gate is authorization only, not a second validator — a create
/// succeeds and round-trips, a stale update token is the modelled
/// `InvalidOperationException`, an id that names nothing is
/// `ResourceNotFoundException` and a foreign pagination token is refused as a bad
/// request.
#[tokio::test]
async fn an_associated_administrator_is_answered_on_the_merits() {
    for region in REGIONS {
        let (router, _engine) = emulator_with_engine();
        let (status, body) = call_in(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": ACCOUNT }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: AssociateAdminAccount must answer 200, got {status} with {body}"
        );

        // A create is answered, and the created policy round-trips through the
        // reader and the list.
        let (status, created) = call_in(
            &router,
            "PutPolicy",
            &json!({ "Policy": policy_document("answered-on-the-merits") }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: expected PutPolicy to answer 200, got {status} with {created}"
        );
        let policy_id = created["Policy"]["PolicyId"]
            .as_str()
            .unwrap_or_else(|| panic!("region {region}: PutPolicy minted no PolicyId: {created}"))
            .to_string();
        let (status, got) = call_in(
            &router,
            "GetPolicy",
            &json!({ "PolicyId": policy_id }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: expected GetPolicy to answer 200, got {status} with {got}"
        );
        assert_eq!(
            got["Policy"], created["Policy"],
            "region {region}: GetPolicy must report the policy PutPolicy stored, got {}",
            got["Policy"]
        );
        let (status, listed) = call_in(&router, "ListPolicies", &json!({}), region).await;
        assert_eq!(
            status, 200,
            "region {region}: expected ListPolicies to answer 200, got {status} with {listed}"
        );
        assert!(
            listed["PolicyList"]
                .as_array()
                .is_some_and(|page| page
                    .iter()
                    .any(|s| s["PolicyId"].as_str() == Some(policy_id.as_str()))),
            "region {region}: ListPolicies must report the created policy, got {}",
            listed["PolicyList"]
        );

        // Each refusal that is NOT about authorization is still the refusal the
        // request earns.
        let mut stale = policy_document("stale-token-update");
        stale["PolicyId"] = json!(policy_id);
        stale["PolicyUpdateToken"] = json!("not-the-current-token");
        for (case, operation, body, expected) in [
            (
                "a stale update token",
                "PutPolicy",
                json!({ "Policy": stale }),
                "InvalidOperationException",
            ),
            (
                "a read of an id that names nothing",
                "GetPolicy",
                json!({ "PolicyId": UNKNOWN_POLICY_ID }),
                "ResourceNotFoundException",
            ),
            (
                "a delete of an id that names nothing",
                "DeletePolicy",
                json!({ "PolicyId": UNKNOWN_POLICY_ID }),
                "ResourceNotFoundException",
            ),
            (
                "a foreign pagination token",
                "ListPolicies",
                json!({ "NextToken": FOREIGN_NEXT_TOKEN }),
                "ValidationException",
            ),
        ] {
            let (status, body) = call_in(&router, operation, &body, region).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, expected),
                "case {case:?} in {region}: expected {expected}, got {status} with {body}"
            );
        }

        // And the delete of a policy that DOES exist is performed.
        let (status, body) = call_in(
            &router,
            "DeletePolicy",
            &json!({ "PolicyId": policy_id }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: expected DeletePolicy to answer 200, got {status} with {body}"
        );
        let (status, body) = call_in(
            &router,
            "GetPolicy",
            &json!({ "PolicyId": policy_id }),
            region,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "region {region}: the deleted policy must be gone, got {status} with {body}"
        );
    }
}

/// The gate CLOSES again when the association is torn down: once the
/// disassociation the clock owed has completed, every policy operation answers
/// the refusal once more — and the policies the administrator created are still
/// there, because a refused call removes nothing.
#[tokio::test]
async fn tearing_the_association_down_closes_the_gate_again() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;
    let known = create_policy(&router, "created-before-the-tear-down").await;

    let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
    assert_eq!(
        status, 200,
        "DisassociateAdminAccount must answer 200 for the holder, got {status} with {body}"
    );
    wait_out_the_disassociation();

    for row in policy_calls(&known) {
        let (status, body) = call(&router, row.operation, &row.body).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {:?}: a torn-down association is no administrator, got {status} with {body}",
            row.case
        );
    }

    let stored = with_store(&engine, |store| {
        store.policies.keys().cloned().collect::<Vec<_>>()
    });
    assert_eq!(
        stored,
        vec![known],
        "the refused calls must have changed nothing, store holds {stored:?}"
    );
}
