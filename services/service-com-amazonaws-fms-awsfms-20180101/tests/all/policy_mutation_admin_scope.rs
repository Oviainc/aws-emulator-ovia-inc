// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the `CustomerPolicyStatus` of a stored policy lets
//! a caller DO with it, as opposed to what it reports.
//!
//! `Policy$PolicyStatus` states the rule for both of its values: `ACTIVE` is
//! "the administrator can manage and delete the policy", and
//! `OUT_OF_ADMIN_SCOPE` is "the administrator can view the policy, but they
//! can't edit or delete the policy. Existing policy protections stay in place".
//! So a stored policy the administrative scope governing it does not cover is
//! READ-ONLY: `PutPolicy` naming its `PolicyId` and `DeletePolicy` naming it are
//! both refused with the modelled `InvalidOperationException`, the stored
//! document, its `PolicyUpdateToken` and its tags are left as they were, and the
//! policy keeps being readable. A policy that scope DOES cover is editable and
//! deletable as before.
//!
//! The rules are asserted over a TABLE, not over one request: the administrative
//! scopes ([`crate::policy_status_derivation::scope_cases`] — all-enabled and
//! listed forms of the policy-type and Region scopes, and each of them absent),
//! every policy type of
//! [`crate::policy_status_derivation::POLICY_TYPES`] and both Regions the
//! suite uses, so each row's expected status is derived from the rule rather
//! than written down. Creating a policy out of scope is NOT part of the rule and
//! stays a 200: the model's sentence is about an EXISTING policy, and the
//! derivation suite pins the create.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router, reusing the request harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::policy_lifecycle::{call, call_in, emulator_with_administrator, error_code, REGION};
use crate::policy_status_derivation::{
    admin_scope, document, emulator_with_another_default_administrator, expected_status,
    get_policy_status, put_admin_scope, put_new, scope_cases, OTHER_REGION, POLICY_TYPES,
};

/// The `CustomerPolicyStatus` of a policy a governing administrative scope does
/// not cover.
const OUT_OF_ADMIN_SCOPE: &str = "OUT_OF_ADMIN_SCOPE";

/// The modelled error a refused edit or delete of a view-only policy answers.
const REFUSAL: &str = "InvalidOperationException";

/// One stored policy, as the test needs to address it: its id, the current
/// update token and the name the stored document carries.
struct Stored {
    policy_id: String,
    update_token: String,
    policy_name: String,
}

/// Create one policy of `security_service_type` in `region` and answer what the
/// test needs to address it afterwards.
async fn create(
    router: &MultiServiceRouter,
    case: &str,
    security_service_type: &str,
    resource_type: &str,
    region: &str,
) -> Stored {
    let document = document(security_service_type, resource_type, None);
    let stored = put_new(router, case, &document, region).await;
    Stored {
        policy_id: stored["PolicyId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no minted PolicyId in {stored}"))
            .to_string(),
        update_token: stored["PolicyUpdateToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no minted PolicyUpdateToken in {stored}"))
            .to_string(),
        policy_name: stored["PolicyName"].as_str().unwrap_or_default().to_string(),
    }
}

/// An edit of `stored`: the same document, renamed, carrying the policy's id and
/// the token it was last reported with.
fn edit(
    stored: &Stored,
    security_service_type: &str,
    resource_type: &str,
    new_name: &str,
) -> Value {
    let mut policy = document(security_service_type, resource_type, None);
    policy["PolicyId"] = json!(stored.policy_id);
    policy["PolicyUpdateToken"] = json!(stored.update_token);
    policy["PolicyName"] = json!(new_name);
    json!({ "Policy": policy })
}

/// The whole stored `Policy` `GetPolicy` reports, which a refused mutation must
/// leave untouched.
async fn read(router: &MultiServiceRouter, case: &str, policy_id: &str) -> Value {
    let (status, body) = call(router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetPolicy to answer 200, got {status} with {body}"
    );
    body["Policy"].clone()
}

/// A view-only policy refuses the edit and the delete, keeps its document, its
/// token and its readability — and a policy the scope covers accepts both.
#[tokio::test]
async fn an_out_of_scope_policy_refuses_the_edit_and_the_delete() {
    for case in scope_cases() {
        let router = emulator_with_another_default_administrator().await;
        put_admin_scope(&router, case.name, &case.scope).await;

        for (security_service_type, resource_type) in POLICY_TYPES {
            for region in [REGION, OTHER_REGION] {
                let row = format!("{}/{security_service_type}/{region}", case.name);
                let want = expected_status(&case, security_service_type, region);
                let stored =
                    create(&router, &row, security_service_type, resource_type, region).await;
                let reported = get_policy_status(&router, &row, &stored.policy_id, REGION).await;
                assert_eq!(
                    reported,
                    json!(want),
                    "case {row:?}: expected the stored policy to be reported {want}, got {reported}"
                );

                let request = edit(&stored, security_service_type, resource_type, "renamed");
                let (status, body) = call_in(&router, "PutPolicy", &request, region).await;
                if want == OUT_OF_ADMIN_SCOPE {
                    assert_eq!(
                        status, 400,
                        "case {row:?}: expected the edit of a view-only policy to be refused \
                         with 400, got {status} with {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        REFUSAL,
                        "case {row:?}: expected the refusal {REFUSAL}, got {body}"
                    );
                    // Nothing was written: the document, the tags and the token
                    // the caller already held are all still current, so the
                    // policy's protections stay exactly as they were.
                    let after = read(&router, &row, &stored.policy_id).await;
                    assert_eq!(
                        after["PolicyName"],
                        json!(stored.policy_name),
                        "case {row:?}: a refused edit must leave the stored document alone, got \
                         {after}"
                    );
                    assert_eq!(
                        after["PolicyUpdateToken"],
                        json!(stored.update_token),
                        "case {row:?}: a refused edit must not rotate the PolicyUpdateToken, got \
                         {after}"
                    );

                    // The cleanup selector is a dimension of the refusal: a
                    // delete that would ALSO take the policy's protections with
                    // it is refused exactly as the bare one is, because
                    // "existing policy protections stay in place".
                    for cleanup in [false, true] {
                        let (status, body) = call_in(
                            &router,
                            "DeletePolicy",
                            &json!({
                                "PolicyId": stored.policy_id,
                                "DeleteAllPolicyResources": cleanup
                            }),
                            region,
                        )
                        .await;
                        assert_eq!(
                            status, 400,
                            "case {row:?}: expected the delete of a view-only policy with \
                             DeleteAllPolicyResources {cleanup} to be refused with 400, got \
                             {status} with {body}"
                        );
                        assert_eq!(
                            error_code(&body),
                            REFUSAL,
                            "case {row:?}: expected the refusal {REFUSAL} with \
                             DeleteAllPolicyResources {cleanup}, got {body}"
                        );
                    }
                    let after = read(&router, &row, &stored.policy_id).await;
                    assert_eq!(
                        after["PolicyId"],
                        json!(stored.policy_id),
                        "case {row:?}: a refused delete must leave the policy readable, got \
                         {after}"
                    );
                } else {
                    // "The administrator can manage and delete the policy":
                    // both mutations go through, exactly as before.
                    assert_eq!(
                        status, 200,
                        "case {row:?}: expected the edit of an in-scope policy to answer 200, \
                         got {status} with {body}"
                    );
                    assert_eq!(
                        body["Policy"]["PolicyName"],
                        json!("renamed"),
                        "case {row:?}: expected the edit to be reported, got {}",
                        body["Policy"]
                    );
                    assert_ne!(
                        body["Policy"]["PolicyUpdateToken"],
                        json!(stored.update_token),
                        "case {row:?}: expected a successful edit to rotate the \
                         PolicyUpdateToken, got {}",
                        body["Policy"]
                    );
                    let after = read(&router, &row, &stored.policy_id).await;
                    assert_eq!(
                        after["PolicyName"],
                        json!("renamed"),
                        "case {row:?}: expected the edit to be persisted, got {after}"
                    );

                    let (status, body) = call_in(
                        &router,
                        "DeletePolicy",
                        &json!({ "PolicyId": stored.policy_id }),
                        region,
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {row:?}: expected the delete of an in-scope policy to answer 200, \
                         got {status} with {body}"
                    );
                    let (status, body) = call(
                        &router,
                        "GetPolicy",
                        &json!({ "PolicyId": stored.policy_id }),
                    )
                    .await;
                    assert_eq!(
                        status, 400,
                        "case {row:?}: expected the deleted policy to be gone, got {status} with \
                         {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        "ResourceNotFoundException",
                        "case {row:?}: expected the deleted policy to be not found, got {body}"
                    );
                }
            }
        }
    }
}

/// Whether a policy may be edited is decided per CALL from the scope that
/// governs it, so narrowing the administrator's scope makes a policy view-only
/// and widening it makes the very same policy editable and deletable again —
/// with the token the refused edit was not allowed to rotate.
#[tokio::test]
async fn widening_the_administrative_scope_makes_the_policy_editable_again() {
    let router = emulator_with_another_default_administrator().await;
    let (security_service_type, resource_type) = POLICY_TYPES[0];
    let stored = create(
        &router,
        "initially-in-scope",
        security_service_type,
        resource_type,
        REGION,
    )
    .await;

    // Narrowed to another Region, so the policy is out of the caller's scope.
    put_admin_scope(
        &router,
        "narrowed",
        &admin_scope(
            json!({ "AllPolicyTypesEnabled": true }),
            json!({ "Regions": [OTHER_REGION] }),
        ),
    )
    .await;
    for (operation, request) in [
        (
            "PutPolicy",
            edit(&stored, security_service_type, resource_type, "renamed"),
        ),
        (
            "DeletePolicy",
            json!({ "PolicyId": stored.policy_id }),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case narrowed/{operation}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            REFUSAL,
            "case narrowed/{operation}: expected the refusal {REFUSAL}, got {body}"
        );
    }

    // Widened again: the same policy, never touched in between, takes the edit
    // the narrowed scope refused — with the token it still carries.
    put_admin_scope(
        &router,
        "widened",
        &admin_scope(
            json!({ "AllPolicyTypesEnabled": true }),
            json!({ "AllRegionsEnabled": true }),
        ),
    )
    .await;
    let request = edit(&stored, security_service_type, resource_type, "renamed");
    let (status, body) = call(&router, "PutPolicy", &request).await;
    assert_eq!(
        status, 200,
        "case widened: expected the edit to answer 200, got {status} with {body}"
    );
    assert_eq!(
        body["Policy"]["PolicyName"],
        json!("renamed"),
        "case widened: expected the edit to be reported, got {}",
        body["Policy"]
    );
    let (status, body) = call(
        &router,
        "DeletePolicy",
        &json!({ "PolicyId": stored.policy_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case widened: expected the delete to answer 200, got {status} with {body}"
    );
}

/// The ordinary single-administrator organisation is untouched: with no
/// delegated administrator configured for the caller, every policy is in the
/// default administrator's full scope, so every policy type is editable and
/// deletable.
#[tokio::test]
async fn the_default_administrator_edits_and_deletes_every_policy() {
    let router = emulator_with_administrator().await;
    for (security_service_type, resource_type) in POLICY_TYPES {
        for region in [REGION, OTHER_REGION] {
            let row = format!("{security_service_type}/{region}");
            let stored =
                create(&router, &row, security_service_type, resource_type, region).await;
            let reported = get_policy_status(&router, &row, &stored.policy_id, region).await;
            assert_eq!(
                reported,
                json!("ACTIVE"),
                "case {row:?}: the default administrator's policies are ACTIVE, got {reported}"
            );

            let request = edit(&stored, security_service_type, resource_type, "renamed");
            let (status, body) = call_in(&router, "PutPolicy", &request, region).await;
            assert_eq!(
                status, 200,
                "case {row:?}: expected the edit to answer 200, got {status} with {body}"
            );
            let (status, body) = call_in(
                &router,
                "DeletePolicy",
                &json!({ "PolicyId": stored.policy_id }),
                region,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {row:?}: expected the delete to answer 200, got {status} with {body}"
            );
        }
    }
}
