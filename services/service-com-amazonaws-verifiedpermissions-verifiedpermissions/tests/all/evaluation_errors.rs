// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the fail-closed-with-errors contract of the
//! authorization operations.
//!
//! `IsAuthorizedOutput` REQUIRES an `errors` member ("Errors that occurred while
//! making an authorization decision, for example, a policy references an Entity
//! or entity Attribute that does not exist in the slice"), which is the model's
//! statement that a failure raised while EVALUATING is reported in-band on the
//! 200 rather than as a request-level refusal. So a request whose `context` or
//! `entities` the store's schema does not sanction is still evaluated: the
//! erroring policy determines nothing (the decision fails closed to `DENY`
//! unless another policy independently decides it), and the error's description
//! is reported in `errors`. `ValidationException` stays for a request that
//! breaks the operation's own input contract — an absent member of the triple,
//! or a `context`/`entities` payload that is no Cedar value at all or is past a
//! bound.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the shared harness in [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{call, error_code, ok, router, store_with_mode};

/// A schema whose one action declares a context record with a REQUIRED `Long`
/// attribute, so a policy can apply a long-only operator to it and a request can
/// supply it at another type.
///
/// Shape per `SchemaDefinition$cedarJson` ("The schema definition must be written
/// in Cedar schema JSON"): an object keyed by namespace, each holding
/// `entityTypes` and `actions`, an action's `appliesTo` naming its
/// `principalTypes`, `resourceTypes` and `context` record type.
fn counting_schema() -> String {
    serde_json::to_string(&json!({
        "PhotoApp": {
            "entityTypes": {
                "User": { "shape": { "type": "Record", "attributes": {} } },
                "Photo": { "shape": { "type": "Record", "attributes": {} } },
            },
            "actions": {
                "viewPhoto": {
                    "appliesTo": {
                        "principalTypes": ["User"],
                        "resourceTypes": ["Photo"],
                        "context": {
                            "type": "Record",
                            "attributes": { "count": { "type": "Long", "required": true } },
                        },
                    },
                },
            },
        },
    }))
    .expect("the schema document serializes")
}

/// The statement that reads the schema's `Long` context attribute through a
/// long-only operator, so a value of any other type is an evaluation-time type
/// error.
const COUNTING_POLICY: &str = "permit(principal == PhotoApp::User::\"alice\", \
     action == PhotoApp::Action::\"viewPhoto\", \
     resource == PhotoApp::Photo::\"vacation.jpg\") \
     when { context.count > 0 };";

/// A store in `mode` holding [`counting_schema`] and `statements`, with the
/// `policyId` each statement was stored under.
async fn store_with_policies(
    router: &smithy_server_runtime::MultiServiceRouter,
    mode: &str,
    statements: &[&str],
) -> (String, Vec<String>) {
    let store_id = store_with_mode(router, mode).await;
    ok(
        router,
        "PutSchema",
        &json!({
            "policyStoreId": store_id,
            "definition": { "cedarJson": counting_schema() },
        }),
    )
    .await;
    let mut policy_ids = Vec::new();
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
        policy_ids.push(
            created["policyId"]
                .as_str()
                .expect("policyId is required on the CreatePolicy response")
                .to_string(),
        );
    }
    (store_id, policy_ids)
}

/// The `errorDescription` of every reported evaluation error.
fn error_descriptions(body: &Value) -> Vec<String> {
    body["errors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["errorDescription"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `policyId` of every determining policy.
fn determining(body: &Value) -> Vec<String> {
    body["determiningPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["policyId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the context table: the `context` to send, the decision it must be
/// evaluated into, and whether the response must report an evaluation error.
struct ContextCase {
    case: &'static str,
    context: Value,
    decision: &'static str,
    /// Whether `errors` must be non-empty — i.e. whether evaluating the stored
    /// policy against this context raises anything.
    errors: bool,
    /// Whether the stored policy must appear in `determiningPolicies`.
    determining: bool,
}

/// The table, spanning the TYPE and the presence of the one context attribute
/// the schema declares, over both `ContextDefinition` arms the model offers.
fn context_cases() -> Vec<ContextCase> {
    vec![
        ContextCase {
            case: "the declared long, condition true",
            context: json!({ "contextMap": { "count": { "long": 5 } } }),
            decision: "ALLOW",
            errors: false,
            determining: true,
        },
        ContextCase {
            case: "the declared long, condition false",
            context: json!({ "contextMap": { "count": { "long": 0 } } }),
            decision: "DENY",
            errors: false,
            determining: false,
        },
        ContextCase {
            case: "the declared long, negative",
            context: json!({ "contextMap": { "count": { "long": -3 } } }),
            decision: "DENY",
            errors: false,
            determining: false,
        },
        ContextCase {
            case: "a boolean where the schema declares a long",
            context: json!({ "contextMap": { "count": { "boolean": true } } }),
            decision: "DENY",
            errors: true,
            determining: false,
        },
        ContextCase {
            case: "a string where the schema declares a long",
            context: json!({ "contextMap": { "count": { "string": "5" } } }),
            decision: "DENY",
            errors: true,
            determining: false,
        },
        ContextCase {
            case: "a set where the schema declares a long",
            context: json!({ "contextMap": { "count": { "set": [{ "long": 5 }] } } }),
            decision: "DENY",
            errors: true,
            determining: false,
        },
        ContextCase {
            case: "a record where the schema declares a long",
            context: json!({ "contextMap": { "count": { "record": { "k": { "long": 5 } } } } }),
            decision: "DENY",
            errors: true,
            determining: false,
        },
        ContextCase {
            case: "an entity reference where the schema declares a long",
            context: json!({ "contextMap": { "count": { "entityIdentifier": {
                "entityType": "PhotoApp::User",
                "entityId": "alice",
            } } } }),
            decision: "DENY",
            errors: true,
            determining: false,
        },
        ContextCase {
            case: "the declared attribute absent",
            context: json!({ "contextMap": {} }),
            decision: "DENY",
            errors: true,
            determining: false,
        },
        ContextCase {
            case: "no context at all",
            context: Value::Null,
            decision: "DENY",
            errors: true,
            determining: false,
        },
        ContextCase {
            case: "an attribute the schema does not declare, beside the declared one",
            context: json!({ "contextMap": {
                "count": { "long": 5 },
                "mfa": { "boolean": true },
            } }),
            decision: "ALLOW",
            errors: false,
            determining: true,
        },
        ContextCase {
            case: "the declared long through cedarJson",
            context: json!({ "cedarJson": "{\"count\": 7}" }),
            decision: "ALLOW",
            errors: false,
            determining: true,
        },
        ContextCase {
            case: "the wrong type through cedarJson",
            context: json!({ "cedarJson": "{\"count\": \"7\"}" }),
            decision: "DENY",
            errors: true,
            determining: false,
        },
    ]
}

/// A context value the store's schema does not sanction is EVALUATED, not
/// refused: the response is 200, the decision fails closed, the erroring policy
/// determines nothing, and the type error is reported in `errors`. The store's
/// validation mode does not change it — the schema governs which POLICIES a
/// store accepts, not the values a request carries.
#[tokio::test]
async fn a_context_the_schema_cannot_type_is_evaluated_into_a_failed_closed_decision() {
    for mode in ["OFF", "STRICT"] {
        let router = router();
        let (store_id, policy_ids) = store_with_policies(&router, mode, &[COUNTING_POLICY]).await;
        let policy_id = policy_ids[0].clone();

        for row in context_cases() {
            let case = format!("{mode} store, {}", row.case);
            let mut request = json!({
                "policyStoreId": store_id,
                "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
            });
            if !row.context.is_null() {
                request["context"] = row.context.clone();
            }

            let (status, body) = call(&router, "IsAuthorized", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: an evaluation failure is reported in-band, so the call must \
                 answer 200, got {status} {body}"
            );
            assert_eq!(
                body["decision"].as_str(),
                Some(row.decision),
                "case {case:?}: expected decision {}, got {body}",
                row.decision
            );
            let errors = error_descriptions(&body);
            assert_eq!(
                !errors.is_empty(),
                row.errors,
                "case {case:?}: expected errors non-empty = {}, got {errors:?}",
                row.errors
            );
            assert!(
                errors.iter().all(|description| !description.is_empty()),
                "case {case:?}: every reported error must describe itself, got {errors:?}"
            );
            let expected_determining: Vec<String> = if row.determining {
                vec![policy_id.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(
                determining(&body),
                expected_determining,
                "case {case:?}: expected determiningPolicies {expected_determining:?}, got {body}"
            );
        }
    }
}

/// A request that breaks the operation's own input contract is still a 400
/// `ValidationException`: the in-band channel is for what the EVALUATION raised,
/// not for a payload the service cannot read as a Cedar value at all.
#[tokio::test]
async fn a_request_that_breaks_its_input_contract_is_still_refused() {
    let router = router();
    let (store_id, _) = store_with_policies(&router, "OFF", &[COUNTING_POLICY]).await;
    let base = json!({
        "policyStoreId": store_id,
        "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
        "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
        "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
        "context": { "contextMap": { "count": { "long": 5 } } },
    });

    // Rows differ along WHICH part of the input contract is broken: an absent
    // member of the triple, a context that is not JSON, a context that is JSON
    // but no Cedar record, and an entities payload that is no entity store.
    let mut no_principal = base.clone();
    no_principal["principal"] = Value::Null;
    let mut no_action = base.clone();
    no_action["action"] = Value::Null;
    let mut unparseable_context = base.clone();
    unparseable_context["context"] = json!({ "cedarJson": "{not json" });
    let mut non_record_context = base.clone();
    non_record_context["context"] = json!({ "cedarJson": "[1, 2, 3]" });
    let mut bad_entities = base.clone();
    bad_entities["entities"] = json!({ "cedarJson": "{\"not\": \"an entity store\"}" });

    let cases: &[(&str, Value)] = &[
        ("an absent principal", no_principal),
        ("an absent action", no_action),
        ("a context that is not JSON", unparseable_context),
        ("a context that is not a Cedar record", non_record_context),
        ("entities that are not a Cedar entity store", bad_entities),
    ];

    for (case, request) in cases {
        let (status, body) = call(&router, "IsAuthorized", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a broken input contract must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
    }
}

/// An erroring policy determines NOTHING: a clean policy evaluated beside it
/// still decides the request, and the error is reported alongside the decision.
#[tokio::test]
async fn an_erroring_policy_does_not_determine_the_decision() {
    // The second statement reads a context attribute the schema does not
    // declare, so it errors on every request while the first one evaluates
    // cleanly. A STRICT store would refuse to store it, so the store is `OFF`.
    let clean = COUNTING_POLICY;
    let erroring = "permit(principal == PhotoApp::User::\"alice\", \
         action == PhotoApp::Action::\"viewPhoto\", \
         resource == PhotoApp::Photo::\"vacation.jpg\") \
         when { context.undeclared > 0 };";
    let router = router();
    let (store_id, policy_ids) = store_with_policies(&router, "OFF", &[clean, erroring]).await;

    let answer = ok(
        &router,
        "IsAuthorized",
        &json!({
            "policyStoreId": store_id,
            "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
            "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
            "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
            "context": { "contextMap": { "count": { "long": 5 } } },
        }),
    )
    .await;

    assert_eq!(
        answer["decision"].as_str(),
        Some("ALLOW"),
        "case \"one clean permit beside one erroring policy\": the clean policy decides, \
         got {answer}"
    );
    assert_eq!(
        determining(&answer),
        vec![policy_ids[0].clone()],
        "case \"one clean permit beside one erroring policy\": only the policy that \
         evaluated may determine the decision, got {answer}"
    );
    let errors = error_descriptions(&answer);
    assert_eq!(
        errors.len(),
        1,
        "case \"one clean permit beside one erroring policy\": the erroring policy must be \
         reported exactly once, got {errors:?}"
    );
    assert!(
        errors[0].contains(&policy_ids[1]),
        "case \"one clean permit beside one erroring policy\": the reported error must name \
         the policy that raised it ({}), got {errors:?}",
        policy_ids[1]
    );
}

/// `BatchIsAuthorized` reports an evaluation failure on the RESULT of the
/// request that raised it, leaving the other results decided, rather than
/// refusing the batch.
#[tokio::test]
async fn a_batch_reports_an_evaluation_failure_per_request() {
    let router = router();
    let (store_id, policy_ids) = store_with_policies(&router, "STRICT", &[COUNTING_POLICY]).await;
    let request_for = |context: Value| {
        json!({
            "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
            "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
            "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
            "context": context,
        })
    };

    // The rows of the batch differ along the type of the one declared context
    // attribute, so one result is decided cleanly and the others fail closed.
    let answer = ok(
        &router,
        "BatchIsAuthorized",
        &json!({
            "policyStoreId": store_id,
            "requests": [
                request_for(json!({ "contextMap": { "count": { "long": 5 } } })),
                request_for(json!({ "contextMap": { "count": { "boolean": true } } })),
                request_for(json!({ "contextMap": { "count": { "string": "5" } } })),
                request_for(json!({ "contextMap": {} })),
            ],
        }),
    )
    .await;

    let results = answer["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        results.len(),
        4,
        "case \"mixed batch\": one result per request, in order, got {answer}"
    );
    // (case, decision, errors non-empty, determining policies)
    let expected: &[(&str, &str, bool, Vec<String>)] = &[
        ("the declared long", "ALLOW", false, vec![policy_ids[0].clone()]),
        ("a boolean where a long is declared", "DENY", true, Vec::new()),
        ("a string where a long is declared", "DENY", true, Vec::new()),
        ("the declared attribute absent", "DENY", true, Vec::new()),
    ];
    for (result, (case, decision, errors, determining_policies)) in results.iter().zip(expected) {
        assert_eq!(
            result["decision"].as_str(),
            Some(*decision),
            "case {case:?}: expected decision {decision}, got {result}"
        );
        let reported = error_descriptions(result);
        assert_eq!(
            !reported.is_empty(),
            *errors,
            "case {case:?}: expected errors non-empty = {errors}, got {reported:?}"
        );
        assert_eq!(
            determining(result),
            *determining_policies,
            "case {case:?}: expected determiningPolicies {determining_policies:?}, got {result}"
        );
    }
}

/// An `entities` slice the schema does not type is evaluated too: the request is
/// answered with a decision, and anything the evaluation raises about the slice
/// is reported in `errors`.
#[tokio::test]
async fn an_entity_slice_the_schema_cannot_type_is_evaluated() {
    let router = router();
    let (store_id, policy_ids) = store_with_policies(&router, "OFF", &[COUNTING_POLICY]).await;
    let base = json!({
        "policyStoreId": store_id,
        "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
        "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
        "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
        "context": { "contextMap": { "count": { "long": 5 } } },
    });

    // Rows differ along what the schema does not sanction about the slice: a
    // declared type (the control), an undeclared entity type, and an attribute
    // the declared type does not have.
    let cases: &[(&str, Value)] = &[
        (
            "an entity of a declared type",
            json!({ "entityList": [{
                "identifier": { "entityType": "PhotoApp::User", "entityId": "alice" },
            }] }),
        ),
        (
            "an entity of a type the schema does not declare",
            json!({ "entityList": [{
                "identifier": { "entityType": "PhotoApp::Admin", "entityId": "root" },
            }] }),
        ),
        (
            "an entity attribute the schema does not declare",
            json!({ "entityList": [{
                "identifier": { "entityType": "PhotoApp::User", "entityId": "alice" },
                "attributes": { "age": { "long": 31 } },
            }] }),
        ),
    ];

    for (case, entities) in cases {
        let mut request = base.clone();
        request["entities"] = entities.clone();
        let (status, body) = call(&router, "IsAuthorized", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a slice the schema cannot type is evaluated, not refused, \
             got {status} {body}"
        );
        assert_eq!(
            body["decision"].as_str(),
            Some("ALLOW"),
            "case {case:?}: the stored policy does not read the slice, so it still \
             decides, got {body}"
        );
        assert_eq!(
            determining(&body),
            vec![policy_ids[0].clone()],
            "case {case:?}: expected the stored policy to determine the decision, got {body}"
        );
        assert_eq!(
            error_descriptions(&body),
            Vec::<String>::new(),
            "case {case:?}: nothing was raised while evaluating, so errors must be empty, \
             got {body}"
        );
    }
}
