// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the referential rule between a Firewall Manager policy
//! and the resource sets it uses.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`] with the `X-Amz-Target` header and a
//! SigV4 credential scope (the harness in [`crate::policy_lifecycle`]), so the
//! whole deserialize → handle → serialize walk runs and the assertions are about
//! the WIRE response.
//!
//! The rule under test is one sentence: a resource set a stored policy document
//! names in `ResourceSetIds` is in USE by that policy, so removing it is refused
//! with HTTP 400 `InvalidOperationException` and nothing is removed, and the set
//! becomes removable only once no stored policy names it — which is the same
//! rule `PutPolicy` applies from the other side when it refuses a document whose
//! `ResourceSetIds` names an id no stored resource set carries. Together the two
//! halves keep ONE invariant: every id a stored policy names resolves to a
//! stored resource set, so the ordinary read-modify-write of a stored policy is
//! always accepted.
//!
//! The table in [`cases`] is the class the rule ranges over: the rows differ in
//! how many resource sets exist, in how many policies use the one under test, in
//! where its id sits in a policy's `ResourceSetIds` (alone, first, middle,
//! last), in which policy types hold the reference, and in how the reference is
//! cleared before the set is finally deleted (editing the policy document, or
//! deleting the policy).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::administrator_gated_operations::{associate_admin, error_message, with_store};
use crate::policy_document_validation::policy_document;
use crate::policy_lifecycle::{call, emulator_with_engine, error_code};

/// One policy of a row: the policy type it is written for, the resource type
/// that type accepts, and the resource sets (by index into the row's sets) its
/// `ResourceSetIds` names.
///
/// The `(security_service_type, resource_type)` pairs are ones the model lists
/// as valid for each other, from the `Policy$ResourceType` documentation
/// ("The following are valid resource types for each Firewall Manager policy
/// type"), read with
///
/// ```text
/// smithy_query(query="doc",shape="com.amazonaws.fms#Policy",member="ResourceType",model_path="services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json")
/// ```
struct PolicyRow {
    security_service_type: &'static str,
    resource_type: &'static str,
    sets: &'static [usize],
}

/// How a row clears the references to the set under test before deleting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clear {
    /// Edit each using policy's document so it no longer names the set — the
    /// ordinary read-modify-write of the policy.
    EditTheDocument,
    /// Delete each using policy outright.
    DeleteThePolicy,
}

/// One row of the table: the resource sets to create, the policies to create
/// over them, the set whose deletion is finally driven through, and how the
/// references to it are cleared.
struct Case {
    name: &'static str,
    sets: usize,
    policies: &'static [PolicyRow],
    target: usize,
    clear: Clear,
}

/// The class the rule is asserted over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one-policy-naming-the-only-set",
            sets: 1,
            policies: &[PolicyRow {
                security_service_type: "NETWORK_FIREWALL",
                resource_type: "AWS::EC2::VPC",
                sets: &[0],
            }],
            target: 0,
            clear: Clear::EditTheDocument,
        },
        Case {
            name: "one-policy-three-sets-target-first",
            sets: 3,
            policies: &[PolicyRow {
                security_service_type: "WAFV2",
                resource_type: "AWS::CloudFront::Distribution",
                sets: &[0, 1, 2],
            }],
            target: 0,
            clear: Clear::EditTheDocument,
        },
        Case {
            name: "one-policy-three-sets-target-middle",
            sets: 3,
            policies: &[PolicyRow {
                security_service_type: "SHIELD_ADVANCED",
                resource_type: "AWS::EC2::EIP",
                sets: &[0, 1, 2],
            }],
            target: 1,
            clear: Clear::DeleteThePolicy,
        },
        Case {
            name: "one-policy-three-sets-target-last-one-set-unused",
            sets: 4,
            policies: &[PolicyRow {
                security_service_type: "DNS_FIREWALL",
                resource_type: "AWS::EC2::VPC",
                sets: &[0, 1, 2],
            }],
            target: 2,
            clear: Clear::EditTheDocument,
        },
        Case {
            name: "two-policies-share-the-target",
            sets: 2,
            policies: &[
                PolicyRow {
                    security_service_type: "NETWORK_FIREWALL",
                    resource_type: "AWS::EC2::VPC",
                    sets: &[0, 1],
                },
                PolicyRow {
                    security_service_type: "WAFV2",
                    resource_type: "AWS::ApiGateway::Stage",
                    sets: &[0],
                },
            ],
            target: 0,
            clear: Clear::DeleteThePolicy,
        },
        Case {
            name: "three-policies-one-each-plus-an-unused-set",
            sets: 4,
            policies: &[
                PolicyRow {
                    security_service_type: "NETWORK_FIREWALL",
                    resource_type: "AWS::EC2::VPC",
                    sets: &[0],
                },
                PolicyRow {
                    security_service_type: "SHIELD_ADVANCED",
                    resource_type: "AWS::ElasticLoadBalancingV2::LoadBalancer",
                    sets: &[1, 2],
                },
                PolicyRow {
                    security_service_type: "WAFV2",
                    resource_type: "AWS::CloudFront::Distribution",
                    sets: &[2],
                },
            ],
            target: 2,
            clear: Clear::EditTheDocument,
        },
    ]
}

/// Create `count` resource sets and return the minted ids, in creation order.
async fn create_sets(router: &MultiServiceRouter, case: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let (status, body) = call(
            router,
            "PutResourceSet",
            &json!({
                "ResourceSet": {
                    "Name": format!("{case}-set-{index}"),
                    "Description": format!("set {index} of case {case}"),
                    "ResourceTypeList": ["AWS::EC2::VPC"],
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: PutResourceSet must answer 200, got {status} with {body}"
        );
        ids.push(
            body["ResourceSet"]["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: PutResourceSet minted no Id: {body}"))
                .to_string(),
        );
    }
    ids
}

/// Create one row's policy over the already-created `sets` and return the minted
/// `PolicyId`.
async fn create_policy(
    router: &MultiServiceRouter,
    case: &str,
    index: usize,
    row: &PolicyRow,
    sets: &[String],
) -> String {
    let mut document = policy_document(
        &format!("{case}-policy-{index}"),
        row.security_service_type,
        row.resource_type,
        None,
    );
    let ids: Vec<&str> = row
        .sets
        .iter()
        .map(|set| sets[*set].as_str())
        .collect();
    document["ResourceSetIds"] = json!(ids);
    let (status, body) = call(router, "PutPolicy", &json!({ "Policy": document })).await;
    assert_eq!(
        status, 200,
        "case {case}: PutPolicy over {ids:?} must answer 200, got {status} with {body}"
    );
    body["Policy"]["PolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: PutPolicy minted no PolicyId: {body}"))
        .to_string()
}

/// The stored policy document, read back through `GetPolicy`.
async fn get_policy(router: &MultiServiceRouter, case: &str, policy_id: &str) -> Value {
    let (status, body) = call(router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
    assert_eq!(
        status, 200,
        "case {case}: GetPolicy for {policy_id} must answer 200, got {status} with {body}"
    );
    body["Policy"].clone()
}

/// The invariant both halves of the rule keep, read through the store the
/// handlers share: every id a stored policy's `ResourceSetIds` names resolves to
/// a stored resource set.
fn assert_every_reference_resolves(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stage: &str,
) {
    with_store(engine, |store| {
        for (policy_id, record) in &store.policies {
            for id in record.policy.resource_set_ids.iter().flatten() {
                assert!(
                    store.resource_sets.contains_key(id),
                    "case {case} ({stage}): policy {policy_id} names resource set {id}, which no \
                     stored resource set carries; stored sets are {:?}",
                    store.resource_sets.keys().collect::<Vec<_>>()
                );
            }
        }
    });
}

/// The read-modify-write every client does with a stored policy: `GetPolicy`
/// then `PutPolicy` with exactly the document it answered, changing nothing.
/// Answers the `(status, body)` of the `PutPolicy`.
async fn round_trip(router: &MultiServiceRouter, case: &str, policy_id: &str) -> (u16, Value) {
    let document = get_policy(router, case, policy_id).await;
    call(router, "PutPolicy", &json!({ "Policy": document })).await
}

/// For every row: a resource set a stored policy uses cannot be deleted — the
/// delete is HTTP 400 `InvalidOperationException` naming the set and every
/// policy using it, and it removes nothing — while a set no policy uses deletes
/// normally; once the references to the set under test are cleared, by editing
/// the using policies or by deleting them, the same delete answers 200.
#[tokio::test]
async fn a_resource_set_a_policy_uses_cannot_be_deleted_until_the_reference_is_cleared() {
    for case in cases() {
        let name = case.name;
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;

        let sets = create_sets(&router, name, case.sets).await;
        let mut policies = Vec::new();
        for (index, row) in case.policies.iter().enumerate() {
            let policy_id = create_policy(&router, name, index, row, &sets).await;
            policies.push((policy_id, row));
        }
        assert_every_reference_resolves(&engine, name, "after the writes");

        // Every set a policy uses refuses the delete, and the refusal names the
        // set and each policy using it.
        for (index, set_id) in sets.iter().enumerate() {
            let users: Vec<&str> = policies
                .iter()
                .filter(|(_, row)| row.sets.contains(&index))
                .map(|(policy_id, _)| policy_id.as_str())
                .collect();
            if users.is_empty() {
                continue;
            }
            let (status, body) = call(
                &router,
                "DeleteResourceSet",
                &json!({ "Identifier": set_id }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidOperationException"),
                "case {name}: deleting set {set_id}, used by {users:?}, must be refused, got \
                 {status} with {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(set_id),
                "case {name}: the refusal must name the resource set {set_id}, got {message:?}"
            );
            for policy_id in &users {
                assert!(
                    message.contains(policy_id),
                    "case {name}: the refusal must name the using policy {policy_id}, got \
                     {message:?}"
                );
            }
            // Nothing was removed: the set still reads back.
            let (status, body) = call(
                &router,
                "GetResourceSet",
                &json!({ "Identifier": set_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name}: a refused delete must leave set {set_id} readable, got {status} \
                 with {body}"
            );
        }
        with_store(&engine, |store| {
            let stored: Vec<String> = store.resource_sets.keys().cloned().collect();
            let mut wanted = sets.clone();
            wanted.sort();
            assert_eq!(
                stored, wanted,
                "case {name}: the refused deletes must leave every set stored, got {stored:?}"
            );
        });

        // A set no policy uses is not in use, so it deletes normally: the
        // refusal is about the reference, not about deletes.
        for (index, set_id) in sets.iter().enumerate() {
            if case.policies.iter().any(|row| row.sets.contains(&index)) {
                continue;
            }
            let (status, body) = call(
                &router,
                "DeleteResourceSet",
                &json!({ "Identifier": set_id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name}: set {set_id} is used by no policy, so the delete must answer 200, \
                 got {status} with {body}"
            );
            let (status, body) = call(
                &router,
                "GetResourceSet",
                &json!({ "Identifier": set_id }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {name}: the deleted set {set_id} must not be readable, got {status} with \
                 {body}"
            );
        }

        // Clear the references to the set under test, the way a client would.
        let target = sets[case.target].clone();
        for (policy_id, row) in &policies {
            if !row.sets.contains(&case.target) {
                continue;
            }
            match case.clear {
                Clear::EditTheDocument => {
                    let mut document = get_policy(&router, name, policy_id).await;
                    let remaining: Vec<String> = document["ResourceSetIds"]
                        .as_array()
                        .map(|ids| {
                            ids.iter()
                                .filter_map(Value::as_str)
                                .filter(|id| *id != target.as_str())
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    document["ResourceSetIds"] = json!(remaining);
                    let (status, body) =
                        call(&router, "PutPolicy", &json!({ "Policy": document })).await;
                    assert_eq!(
                        status, 200,
                        "case {name}: dropping {target} from policy {policy_id} must answer 200, \
                         got {status} with {body}"
                    );
                }
                Clear::DeleteThePolicy => {
                    let (status, body) =
                        call(&router, "DeletePolicy", &json!({ "PolicyId": policy_id })).await;
                    assert_eq!(
                        status, 200,
                        "case {name}: deleting policy {policy_id} must answer 200, got {status} \
                         with {body}"
                    );
                }
            }
        }

        // With no policy using it, the same delete goes through.
        let (status, body) = call(
            &router,
            "DeleteResourceSet",
            &json!({ "Identifier": target }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: with the reference cleared, deleting {target} must answer 200, got \
             {status} with {body}"
        );
        let (status, body) = call(
            &router,
            "GetResourceSet",
            &json!({ "Identifier": target }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {name}: the deleted set {target} must not be readable, got {status} with {body}"
        );
        assert_every_reference_resolves(&engine, name, "after the delete");

        // Every policy still stored reads back and writes back unchanged: the
        // invariant above is exactly what makes that possible.
        let remaining: Vec<String> = with_store(&engine, |store| {
            store.policies.keys().cloned().collect()
        });
        for policy_id in &remaining {
            let (status, body) = round_trip(&router, name, policy_id).await;
            assert_eq!(
                status, 200,
                "case {name}: policy {policy_id} must still be editable, got {status} with {body}"
            );
        }
    }
}

/// The symptom the rule rules out, end to end: a policy that nobody edited stays
/// editable. With the set it uses still in place (the delete having been
/// refused), `GetPolicy` followed by `PutPolicy` with exactly the document that
/// was answered — changing nothing — succeeds, and the stored document still
/// names the set.
#[tokio::test]
async fn an_untouched_policy_stays_editable_after_a_refused_delete_of_the_set_it_uses() {
    let case = "untouched-policy";
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let sets = create_sets(&router, case, 1).await;
    let row = PolicyRow {
        security_service_type: "NETWORK_FIREWALL",
        resource_type: "AWS::EC2::VPC",
        sets: &[0],
    };
    let policy_id = create_policy(&router, case, 0, &row, &sets).await;

    let (status, body) = call(
        &router,
        "DeleteResourceSet",
        &json!({ "Identifier": sets[0] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidOperationException"),
        "case {case}: deleting the set the policy uses must be refused, got {status} with {body}"
    );

    // The read-modify-write of the untouched policy, twice over: a successful
    // update rotates the token, so the second round trip proves the first one
    // left an editable policy behind as well.
    for attempt in 1..=2 {
        let (status, body) = round_trip(&router, case, &policy_id).await;
        assert_eq!(
            status, 200,
            "case {case}: round trip {attempt} of the untouched policy must answer 200, got \
             {status} with {body}"
        );
        assert_eq!(
            body["Policy"]["ResourceSetIds"],
            json!([sets[0]]),
            "case {case}: round trip {attempt} must keep the ResourceSetIds, got {}",
            body["Policy"]
        );
    }
    assert_every_reference_resolves(&engine, case, "after the round trips");

    // Deleting the policy frees the set, and then the set deletes too.
    let (status, body) = call(&router, "DeletePolicy", &json!({ "PolicyId": policy_id })).await;
    assert_eq!(
        status, 200,
        "case {case}: DeletePolicy must answer 200, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "DeleteResourceSet",
        &json!({ "Identifier": sets[0] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: with the policy gone the set must delete, got {status} with {body}"
    );
    with_store(&engine, |store| {
        assert!(
            store.resource_sets.is_empty() && store.policies.is_empty(),
            "case {case}: the store must hold neither the policy nor the set, got {:?} and {:?}",
            store.policies.keys().collect::<Vec<_>>(),
            store.resource_sets.keys().collect::<Vec<_>>()
        );
    });
}
