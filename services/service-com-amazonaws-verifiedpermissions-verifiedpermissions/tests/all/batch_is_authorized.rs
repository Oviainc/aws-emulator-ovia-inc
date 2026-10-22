// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `BatchIsAuthorized`.
//!
//! The rule under test: a batch evaluates EACH of its requests against the policy
//! store's persisted Cedar policies and answers one result per request, in the
//! order they were requested, each carrying that request's own decision,
//! determining policies and evaluation errors — the same answer the single-request
//! `IsAuthorized` gives for that request.
//!
//! Every assertion goes over the wire through the shared harness, so a decision is
//! only right if it really came out of state.

use serde_json::{json, Value};

use super::harness::{call, error_code, ok, router, store_with_mode, store_with_validation_off};
// The schema suite owns the one real Cedar schema this test crate has.
use super::schema::photo_app_schema;

/// One request inside a batch, and what the service must answer for it.
struct Req {
    /// `(entityType, entityId)`.
    principal: (&'static str, &'static str),
    /// `(actionType, actionId)`.
    action: (&'static str, &'static str),
    /// `(entityType, entityId)`.
    resource: (&'static str, &'static str),
    /// `ALLOW` or `DENY`.
    decision: &'static str,
    /// Indexes into the case's `statements` whose policy ids must be exactly the
    /// request's `determiningPolicies`.
    determining: &'static [usize],
}

/// One batch: the Cedar text to store, then the requests to send together.
struct BatchCase {
    case: &'static str,
    statements: &'static [&'static str],
    requests: &'static [Req],
}

/// The `requests` member for a batch of [`Req`]s.
fn requests_json(requests: &[Req]) -> Value {
    Value::Array(
        requests
            .iter()
            .map(|r| {
                json!({
                    "principal": { "entityType": r.principal.0, "entityId": r.principal.1 },
                    "action": { "actionType": r.action.0, "actionId": r.action.1 },
                    "resource": { "entityType": r.resource.0, "entityId": r.resource.1 },
                })
            })
            .collect::<Vec<Value>>(),
    )
}

/// The `policyId`s a result's `determiningPolicies` names, sorted.
fn determining_ids(result: &Value) -> Vec<String> {
    let mut ids: Vec<String> = result["determiningPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["policyId"].as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// Store every statement of a case in a fresh store and return
/// `(policyStoreId, policy ids in statement order)`.
async fn store_with(
    router: &smithy_server_runtime::MultiServiceRouter,
    statements: &[&str],
) -> (String, Vec<String>) {
    let store_id = store_with_validation_off(router).await;
    let mut ids = Vec::new();
    for statement in statements {
        let created = ok(
            router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
        ids.push(created["policyId"].as_str().unwrap_or_default().to_string());
    }
    (store_id, ids)
}

/// A batch answers one result per request, in request order, each echoing its
/// request and carrying the decision Cedar computed for it.
///
/// The rows differ along how many requests the batch carries, which side of the
/// triple is the shared one (principal or resource), what varies across the
/// requests (resource, principal, action) and how the decisions mix (allow-only,
/// deny-only, and both in one batch, including a `Forbid` overriding a `Permit`).
#[tokio::test]
async fn a_batch_answers_one_cedar_decision_per_request_in_order() {
    let cases: &[BatchCase] = &[
        BatchCase {
            case: "single request",
            statements: &[
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p1\");",
            ],
            requests: &[Req {
                principal: ("User", "alice"),
                action: ("Action", "view"),
                resource: ("Photo", "p1"),
                decision: "ALLOW",
                determining: &[0],
            }],
        },
        BatchCase {
            case: "one principal, many resources, mixed decisions",
            statements: &[
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p1\");",
                "forbid(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p3\");",
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p3\");",
            ],
            requests: &[
                Req {
                    principal: ("User", "alice"),
                    action: ("Action", "view"),
                    resource: ("Photo", "p1"),
                    decision: "ALLOW",
                    determining: &[0],
                },
                Req {
                    // No policy matches: the default deny, with no determining policy.
                    principal: ("User", "alice"),
                    action: ("Action", "view"),
                    resource: ("Photo", "p2"),
                    decision: "DENY",
                    determining: &[],
                },
                Req {
                    // The forbid beats the permit on the same resource.
                    principal: ("User", "alice"),
                    action: ("Action", "view"),
                    resource: ("Photo", "p3"),
                    decision: "DENY",
                    determining: &[1],
                },
            ],
        },
        BatchCase {
            case: "one resource, many principals",
            statements: &[
                "permit(principal == User::\"bob\", action == Action::\"view\", resource == Photo::\"shared\");",
                "permit(principal == User::\"carol\", action == Action::\"view\", resource == Photo::\"shared\");",
            ],
            requests: &[
                Req {
                    principal: ("User", "bob"),
                    action: ("Action", "view"),
                    resource: ("Photo", "shared"),
                    decision: "ALLOW",
                    determining: &[0],
                },
                Req {
                    principal: ("User", "dana"),
                    action: ("Action", "view"),
                    resource: ("Photo", "shared"),
                    decision: "DENY",
                    determining: &[],
                },
                Req {
                    principal: ("User", "carol"),
                    action: ("Action", "view"),
                    resource: ("Photo", "shared"),
                    decision: "ALLOW",
                    determining: &[1],
                },
            ],
        },
        BatchCase {
            case: "same principal and resource, actions differ",
            statements: &[
                "permit(principal == User::\"erin\", action in [Action::\"view\", Action::\"comment\"], resource == Photo::\"p\");",
            ],
            requests: &[
                Req {
                    principal: ("User", "erin"),
                    action: ("Action", "view"),
                    resource: ("Photo", "p"),
                    decision: "ALLOW",
                    determining: &[0],
                },
                Req {
                    principal: ("User", "erin"),
                    action: ("Action", "comment"),
                    resource: ("Photo", "p"),
                    decision: "ALLOW",
                    determining: &[0],
                },
                Req {
                    principal: ("User", "erin"),
                    action: ("Action", "delete"),
                    resource: ("Photo", "p"),
                    decision: "DENY",
                    determining: &[],
                },
            ],
        },
        BatchCase {
            case: "two matching permits on one request",
            statements: &[
                "permit(principal == User::\"frank\", action == Action::\"view\", resource == Photo::\"p\");",
                "permit(principal, action == Action::\"view\", resource == Photo::\"p\");",
            ],
            requests: &[Req {
                principal: ("User", "frank"),
                action: ("Action", "view"),
                resource: ("Photo", "p"),
                decision: "ALLOW",
                determining: &[0, 1],
            }],
        },
    ];

    for BatchCase {
        case,
        statements,
        requests,
    } in cases
    {
        let router = router();
        let (store_id, policy_ids) = store_with(&router, statements).await;
        let sent = requests_json(requests);

        let answer = ok(
            &router,
            "BatchIsAuthorized",
            &json!({ "policyStoreId": store_id, "requests": sent }),
        )
        .await;

        let results = answer["results"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            requests.len(),
            "case {case:?}: expected one result per request, got {answer}"
        );

        for (index, expected) in requests.iter().enumerate() {
            let result = &results[index];
            assert_eq!(
                result["request"],
                sent[index],
                "case {case:?} request {index}: the result must echo the request it \
                 decided, got {result}"
            );
            assert_eq!(
                result["decision"].as_str().unwrap_or_default(),
                expected.decision,
                "case {case:?} request {index}: unexpected decision, got {result}"
            );
            let mut wanted: Vec<String> = expected
                .determining
                .iter()
                .map(|i| policy_ids[*i].clone())
                .collect();
            wanted.sort();
            assert_eq!(
                determining_ids(result),
                wanted,
                "case {case:?} request {index}: unexpected determiningPolicies, got {result}"
            );
            assert_eq!(
                result["errors"].as_array().map(Vec::len),
                Some(0),
                "case {case:?} request {index}: a clean evaluation reports no errors, \
                 got {result}"
            );

            // The same request on its own must get the same answer: a batch is a
            // batch of `IsAuthorized` evaluations, not a second decision procedure.
            let mut single = sent[index].clone();
            single["policyStoreId"] = json!(store_id);
            let alone = ok(&router, "IsAuthorized", &single).await;
            assert_eq!(
                alone["decision"], result["decision"],
                "case {case:?} request {index}: the batch and IsAuthorized must agree, \
                 got {alone} and {result}"
            );
            assert_eq!(
                determining_ids(&alone),
                determining_ids(result),
                "case {case:?} request {index}: the batch and IsAuthorized must name the \
                 same determining policies, got {alone} and {result}"
            );
        }
    }
}

/// The per-request `context` and the batch-wide `entities` both reach the Cedar
/// evaluator: two requests that differ only in their context get different
/// decisions, and the batch's entity hierarchy decides a group membership test.
#[tokio::test]
async fn per_request_context_and_batch_entities_reach_the_evaluator() {
    let router = router();
    let (store_id, _) = store_with(
        &router,
        &[
            "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p\")\n\
             when { context.mfa == true };",
            "permit(principal in Group::\"admins\", action == Action::\"edit\", resource == Photo::\"p\");",
        ],
    )
    .await;

    // Rows differ along the context carried by each request of ONE batch.
    let context_cases: &[(&str, Value, &str)] = &[
        ("mfa satisfied", json!({ "contextMap": { "mfa": { "boolean": true } } }), "ALLOW"),
        ("mfa refused", json!({ "contextMap": { "mfa": { "boolean": false } } }), "DENY"),
        ("context as cedarJson", json!({ "cedarJson": "{\"mfa\": true}" }), "ALLOW"),
    ];
    let requests: Vec<Value> = context_cases
        .iter()
        .map(|(_, context, _)| {
            json!({
                "principal": { "entityType": "User", "entityId": "alice" },
                "action": { "actionType": "Action", "actionId": "view" },
                "resource": { "entityType": "Photo", "entityId": "p" },
                "context": context,
            })
        })
        .collect();
    let answer = ok(
        &router,
        "BatchIsAuthorized",
        &json!({ "policyStoreId": store_id, "requests": requests }),
    )
    .await;
    let results = answer["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        results.len(),
        context_cases.len(),
        "expected one result per request, got {answer}"
    );
    for (index, (case, _, expected)) in context_cases.iter().enumerate() {
        assert_eq!(
            results[index]["decision"].as_str().unwrap_or_default(),
            *expected,
            "case {case:?}: unexpected decision, got {}",
            results[index]
        );
    }

    // Rows differ along whether the batch's entity slice puts the principal in the
    // group the policy names. Both requests share the resource, so the batch is a
    // valid one either way.
    let entity_cases: &[(&str, Value, &[&str])] = &[
        ("no hierarchy", Value::Null, &["DENY", "DENY"]),
        (
            "mallory in the group",
            json!({ "entityList": [{
                "identifier": { "entityType": "User", "entityId": "mallory" },
                "parents": [{ "entityType": "Group", "entityId": "admins" }],
            }] }),
            &["ALLOW", "DENY"],
        ),
        (
            "hierarchy as cedarJson",
            json!({ "cedarJson": "[{\"uid\": {\"type\": \"User\", \"id\": \"mallory\"}, \"attrs\": {}, \"parents\": [{\"type\": \"Group\", \"id\": \"admins\"}]}]" }),
            &["ALLOW", "DENY"],
        ),
    ];
    for (case, entities, expected) in entity_cases {
        let mut request = json!({
            "policyStoreId": store_id,
            "requests": [
                {
                    "principal": { "entityType": "User", "entityId": "mallory" },
                    "action": { "actionType": "Action", "actionId": "edit" },
                    "resource": { "entityType": "Photo", "entityId": "p" },
                },
                {
                    "principal": { "entityType": "User", "entityId": "nobody" },
                    "action": { "actionType": "Action", "actionId": "edit" },
                    "resource": { "entityType": "Photo", "entityId": "p" },
                },
            ],
        });
        if !entities.is_null() {
            request["entities"] = entities.clone();
        }
        let answer = ok(&router, "BatchIsAuthorized", &request).await;
        let decisions: Vec<String> = answer["results"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|r| r["decision"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            decisions, *expected,
            "case {case:?}: unexpected decisions, got {answer}"
        );
    }
}

/// The store's schema types every request of a batch where it types it, and a
/// request whose `context` it does not type is still EVALUATED: its result
/// carries the decision it failed closed to and the error the evaluation raised,
/// and the other results are decided as usual. The batch is not refused.
#[tokio::test]
async fn a_batch_is_typed_against_the_stores_schema() {
    let router = router();
    let store_id = store_with_mode(&router, "STRICT").await;
    ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": store_id,
            "definition": { "cedarJson": photo_app_schema() },
        }),
    )
    .await;
    ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement":
                "permit(principal == PhotoApp::User::\"alice\", \
                 action == PhotoApp::Action::\"viewPhoto\", \
                 resource == PhotoApp::Photo::\"vacation.jpg\") \
                 when { context.authenticated };" } },
        }),
    )
    .await;

    // Every request shares the principal, so the batch is a valid one; the rows
    // differ along the resource and the declared boolean's value.
    let request_for = |photo: &str, context: Value| {
        json!({
            "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
            "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
            "resource": { "entityType": "PhotoApp::Photo", "entityId": photo },
            "context": context,
        })
    };
    let authenticated = |value: bool| json!({ "contextMap": { "authenticated": { "boolean": value } } });

    let answer = ok(
        &router,
        "BatchIsAuthorized",
        &json!({
            "policyStoreId": store_id,
            "requests": [
                request_for("vacation.jpg", authenticated(true)),
                request_for("vacation.jpg", authenticated(false)),
                request_for("beach.jpg", authenticated(true)),
            ],
        }),
    )
    .await;
    let decisions: Vec<String> = answer["results"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|r| r["decision"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        decisions,
        vec!["ALLOW", "DENY", "DENY"],
        "case \"typed batch\": unexpected decisions, got {answer}"
    );

    // Rows differ along WHICH request of the batch carries the context the
    // schema does not type, and in what way it fails to type-check. The second
    // column is the decision that request must fail closed to, and the third
    // whether the evaluation raises an error the result reports.
    let bad_cases: &[(&str, Value, &str, bool)] = &[
        (
            "a string where the schema declares a boolean",
            json!({ "contextMap": { "authenticated": { "string": "true" } } }),
            "DENY",
            true,
        ),
        (
            "an attribute the schema does not declare",
            json!({ "contextMap": {
                "authenticated": { "boolean": true },
                "mfa": { "boolean": true },
            } }),
            "ALLOW",
            false,
        ),
        (
            "the declared attribute absent",
            json!({ "contextMap": {} }),
            "DENY",
            true,
        ),
    ];
    for (case, context, decision, errors) in bad_cases {
        let (status, body) = call(
            &router,
            "BatchIsAuthorized",
            &json!({
                "policyStoreId": store_id,
                "requests": [
                    request_for("vacation.jpg", authenticated(true)),
                    request_for("vacation.jpg", context.clone()),
                ],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an evaluation failure is reported in-band, so the batch must \
             answer 200, got {status} {body}"
        );
        let results = body["results"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            2,
            "case {case:?}: one result per request, got {body}"
        );
        assert_eq!(
            results[0]["decision"].as_str(),
            Some("ALLOW"),
            "case {case:?}: the well-typed request is still decided, got {body}"
        );
        assert_eq!(
            results[1]["decision"].as_str(),
            Some(*decision),
            "case {case:?}: expected decision {decision}, got {body}"
        );
        let reported = results[1]["errors"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            !reported.is_empty(),
            *errors,
            "case {case:?}: expected errors non-empty = {errors}, got {body}"
        );
    }
}

/// A policy that cannot be loaded is reported on EVERY request of the batch
/// rather than making the store read as a clean deny.
#[tokio::test]
async fn an_evaluation_error_is_reported_per_request() {
    let router = router();
    let (store_id, _) = store_with(
        &router,
        &[
            // Reading an attribute the principal does not carry is a Cedar
            // evaluation error, not a silent `false`.
            "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p\")\n\
             when { principal.department == \"eng\" };",
        ],
    )
    .await;

    let answer = ok(
        &router,
        "BatchIsAuthorized",
        &json!({
            "policyStoreId": store_id,
            "requests": [
                {
                    "principal": { "entityType": "User", "entityId": "alice" },
                    "action": { "actionType": "Action", "actionId": "view" },
                    "resource": { "entityType": "Photo", "entityId": "p" },
                },
                {
                    "principal": { "entityType": "User", "entityId": "alice" },
                    "action": { "actionType": "Action", "actionId": "view" },
                    "resource": { "entityType": "Photo", "entityId": "other" },
                },
            ],
        }),
    )
    .await;
    let results = answer["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        results[0]["errors"].as_array().map(Vec::len),
        Some(1),
        "case \"attribute missing\": the failing policy must be reported, got {answer}"
    );
    assert_eq!(
        results[0]["decision"].as_str().unwrap_or_default(),
        "DENY",
        "case \"attribute missing\": a policy that errors cannot allow, got {answer}"
    );
    // The second request matches no policy at all, so nothing errored for it.
    assert_eq!(
        results[1]["errors"].as_array().map(Vec::len),
        Some(0),
        "case \"no matching policy\": nothing evaluated, so nothing errored, got {answer}"
    );
}

/// The batch is a READ: calling it leaves the store exactly as it was, and the
/// same batch asked twice answers the same thing.
#[tokio::test]
async fn a_batch_changes_nothing_and_follows_the_stored_policies() {
    let router = router();
    let (store_id, policy_ids) = store_with(
        &router,
        &["permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p\");"],
    )
    .await;
    let request = json!({
        "policyStoreId": store_id,
        "requests": [{
            "principal": { "entityType": "User", "entityId": "alice" },
            "action": { "actionType": "Action", "actionId": "view" },
            "resource": { "entityType": "Photo", "entityId": "p" },
        }],
    });

    let first = ok(&router, "BatchIsAuthorized", &request).await;
    let second = ok(&router, "BatchIsAuthorized", &request).await;
    assert_eq!(
        first, second,
        "case \"repeated batch\": a read must answer the same twice, got {first} and {second}"
    );
    assert_eq!(
        first["results"][0]["decision"].as_str().unwrap_or_default(),
        "ALLOW",
        "case \"repeated batch\": the stored permit must allow, got {first}"
    );

    let listed = ok(
        &router,
        "ListPolicies",
        &json!({ "policyStoreId": store_id }),
    )
    .await;
    assert_eq!(
        listed["policies"].as_array().map(Vec::len),
        Some(1),
        "case \"repeated batch\": the batch must not have touched the store, got {listed}"
    );

    // Delete the policy and the same batch must flip: the decision comes from
    // state, not from the request.
    ok(
        &router,
        "DeletePolicy",
        &json!({ "policyStoreId": store_id, "policyId": policy_ids[0] }),
    )
    .await;
    let after_delete = ok(&router, "BatchIsAuthorized", &request).await;
    assert_eq!(
        after_delete["results"][0]["decision"]
            .as_str()
            .unwrap_or_default(),
        "DENY",
        "case \"after delete\": the deleted permit must no longer allow, got {after_delete}"
    );
}

/// The batch size bound: a batch at the published limit is evaluated, one request
/// past it is refused with a `ValidationException` instead of evaluated.
#[tokio::test]
async fn the_batch_size_bound_refuses_instead_of_evaluating() {
    /// `MAX_BATCH_REQUESTS` in `src/state/cedar.rs`, from `BatchIsAuthorized`'s
    /// own model documentation: "The requests of a BatchIsAuthorized API request
    /// can contain up to 30 requests."
    const MAX_BATCH_REQUESTS: usize = 30;

    let router = router();
    let (store_id, _) = store_with(
        &router,
        &["permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p0\");"],
    )
    .await;

    let batch_of = |count: usize| {
        (0..count)
            .map(|n| {
                json!({
                    "principal": { "entityType": "User", "entityId": "alice" },
                    "action": { "actionType": "Action", "actionId": "view" },
                    "resource": { "entityType": "Photo", "entityId": format!("p{n}") },
                })
            })
            .collect::<Vec<Value>>()
    };

    // One row per published bound and one past it.
    let cases: &[(&str, usize, u16)] = &[
        ("one request", 1, 200),
        ("at the batch bound", MAX_BATCH_REQUESTS, 200),
        ("one past the batch bound", MAX_BATCH_REQUESTS + 1, 400),
    ];
    for (case, count, expected) in cases {
        let (status, body) = call(
            &router,
            "BatchIsAuthorized",
            &json!({ "policyStoreId": store_id, "requests": batch_of(*count) }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected == 200 {
            assert_eq!(
                body["results"].as_array().map(Vec::len),
                Some(*count),
                "case {case:?}: expected {count} results, got {body}"
            );
            assert_eq!(
                body["results"][0]["decision"].as_str().unwrap_or_default(),
                "ALLOW",
                "case {case:?}: the first request matches the stored permit, got {body}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: a batch past the bound is a ValidationException, got {body}"
            );
        }
    }
}

/// Every refusal the operation owes: an unknown store, a request missing a member
/// of its triple, and a batch that shares neither principal nor resource.
#[tokio::test]
async fn a_batch_refuses_what_it_cannot_evaluate() {
    let router = router();
    let (store_id, _) = store_with(
        &router,
        &["permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"p\");"],
    )
    .await;
    let good = json!({
        "principal": { "entityType": "User", "entityId": "alice" },
        "action": { "actionType": "Action", "actionId": "view" },
        "resource": { "entityType": "Photo", "entityId": "p" },
    });

    // Rows differ along WHICH part of the request cannot be evaluated.
    let cases: &[(&str, Value, u16, &str)] = &[
        (
            "unknown policy store",
            json!({ "policyStoreId": "PSnope", "requests": [good] }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "store alias naming nothing",
            json!({ "policyStoreId": "policy-store-alias/nope", "requests": [good] }),
            404,
            "ResourceNotFoundException",
        ),
        (
            // The store is resolved before any payload is converted, so a
            // reference to a store that does not exist wins over a refusal about
            // the request's own members.
            "unknown policy store with an incomplete request",
            json!({
                "policyStoreId": "PSnope",
                "requests": [{
                    "action": { "actionType": "Action", "actionId": "view" },
                    "resource": { "entityType": "Photo", "entityId": "p" },
                }],
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "request missing its principal",
            json!({
                "policyStoreId": store_id,
                "requests": [{
                    "action": { "actionType": "Action", "actionId": "view" },
                    "resource": { "entityType": "Photo", "entityId": "p" },
                }],
            }),
            400,
            "ValidationException",
        ),
        (
            "request missing its action",
            json!({
                "policyStoreId": store_id,
                "requests": [{
                    "principal": { "entityType": "User", "entityId": "alice" },
                    "resource": { "entityType": "Photo", "entityId": "p" },
                }],
            }),
            400,
            "ValidationException",
        ),
        (
            "request missing its resource",
            json!({
                "policyStoreId": store_id,
                "requests": [{
                    "principal": { "entityType": "User", "entityId": "alice" },
                    "action": { "actionType": "Action", "actionId": "view" },
                }],
            }),
            400,
            "ValidationException",
        ),
        (
            // "Either the principal or the resource parameter must be identical
            // across all requests."
            "neither principal nor resource shared",
            json!({
                "policyStoreId": store_id,
                "requests": [
                    good,
                    {
                        "principal": { "entityType": "User", "entityId": "bob" },
                        "action": { "actionType": "Action", "actionId": "view" },
                        "resource": { "entityType": "Photo", "entityId": "other" },
                    },
                ],
            }),
            400,
            "ValidationException",
        ),
        (
            "entityType that is not valid Cedar",
            json!({
                "policyStoreId": store_id,
                "requests": [{
                    "principal": { "entityType": "not a type", "entityId": "alice" },
                    "action": { "actionType": "Action", "actionId": "view" },
                    "resource": { "entityType": "Photo", "entityId": "p" },
                }],
            }),
            400,
            "ValidationException",
        ),
    ];
    for (case, request, status, code) in cases {
        let (actual, body) = call(&router, "BatchIsAuthorized", request).await;
        assert_eq!(
            actual, *status,
            "case {case:?}: expected HTTP {status}, got {actual} {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected {code}, got {body}"
        );
    }

    // An empty `requests` list is refused by the model's own `@length min 1`.
    let (status, body) = call(
        &router,
        "BatchIsAuthorized",
        &json!({ "policyStoreId": store_id, "requests": [] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"empty requests\": expected HTTP 400, got {status} {body}"
    );
}
