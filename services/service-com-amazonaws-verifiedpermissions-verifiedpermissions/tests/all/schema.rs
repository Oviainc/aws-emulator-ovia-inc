// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for a policy store's schema: `PutSchema`, `GetSchema`, and
//! the two things a stored schema governs — `STRICT` validation of a submitted
//! static policy, and the typing of an authorization request's context and
//! entities.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the harness in
//! [`crate::policy_store_authorization`], so a value is only visible to a later
//! request if it really reached state.

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::state::cedar::MAX_SCHEMA_BYTES;

use crate::harness::{call, error_code, ok, router, store_with_mode};

/// The namespaces a response reports, sorted, so the comparison does not depend
/// on the order the parse walked them in. An absent list reads as empty.
fn namespaces(value: &Value) -> Vec<String> {
    let mut out: Vec<String> = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item.as_str().unwrap_or_default().to_string())
        .collect();
    out.sort();
    out
}

/// The schema used wherever a test needs a real one: one namespace, two entity
/// types, and two actions whose context declares a single boolean attribute.
///
/// The shape of this document is the Cedar schema JSON format that
/// `SchemaDefinition$cedarJson` documents ("The schema definition must be written
/// in Cedar schema JSON"): an object keyed by namespace, each namespace holding
/// `entityTypes` and `actions`, and an action's `appliesTo` naming its
/// `principalTypes`, `resourceTypes` and `context` record type.
///
/// `pub(crate)` so a sibling suite in this test target reaches the same schema
/// rather than declaring a second one. The two actions differ only in name, so a
/// suite can vary how MANY actions a policy's scope names without varying
/// anything else.
pub(crate) fn photo_app_schema() -> String {
    let applies_to = json!({
        "principalTypes": ["User"],
        "resourceTypes": ["Photo"],
        "context": {
            "type": "Record",
            "attributes": { "authenticated": { "type": "Boolean" } },
        },
    });
    serde_json::to_string(&json!({
        "PhotoApp": {
            "entityTypes": {
                "User": { "shape": { "type": "Record", "attributes": {} } },
                "Photo": { "shape": { "type": "Record", "attributes": {} } },
            },
            "actions": {
                "viewPhoto": { "appliesTo": applies_to },
                "sharePhoto": { "appliesTo": applies_to },
            },
        },
    }))
    .expect("the schema document serializes")
}

/// A namespace block declaring one entity type and one action over it, so a test
/// can vary the NAMESPACE without varying anything else.
fn namespace_block() -> Value {
    json!({
        "entityTypes": { "User": { "shape": { "type": "Record", "attributes": {} } } },
        "actions": {
            "read": {
                "appliesTo": { "principalTypes": ["User"], "resourceTypes": ["User"] },
            },
        },
    })
}

/// A valid schema whose text is exactly `bytes` long, reached by padding the one
/// entity type's NAME — a Cedar identifier has no length limit of its own, so the
/// document stays valid at any size.
fn schema_of(bytes: usize) -> String {
    let prefix = "{\"PhotoApp\":{\"actions\":{},\"entityTypes\":{\"";
    let suffix = "\":{}}}}";
    let name_len = bytes.saturating_sub(prefix.len() + suffix.len());
    format!("{prefix}{}{suffix}", "A".repeat(name_len))
}

// ---------------------------------------------------------------------------
// PutSchema / GetSchema
// ---------------------------------------------------------------------------

/// A stored schema round-trips: `PutSchema` reports the namespaces the schema
/// declares, and `GetSchema` reads back the same text, the same namespaces and
/// the same timestamps.
#[tokio::test]
async fn schema_round_trips_and_reports_the_namespaces_it_declares() {
    // Rows differ along how many namespaces the schema declares and the FORM of
    // each name: none at all, the unnamed namespace, one name, a `::`-qualified
    // name, and several at once.
    let cases: &[(&str, Value, &[&str])] = &[
        ("no definitions", json!({}), &[]),
        ("the unnamed namespace", json!({ "": namespace_block() }), &[""]),
        (
            "one namespace",
            json!({ "PhotoApp": namespace_block() }),
            &["PhotoApp"],
        ),
        (
            "a qualified namespace name",
            json!({ "Org::PhotoApp": namespace_block() }),
            &["Org::PhotoApp"],
        ),
        (
            "three namespaces",
            json!({
                "A": namespace_block(),
                "B": namespace_block(),
                "Org::C": namespace_block(),
            }),
            &["A", "B", "Org::C"],
        ),
    ];

    let router = router();
    for (case, schema, expected) in cases {
        let store_id = store_with_mode(&router, "OFF").await;
        let text = serde_json::to_string(schema).expect("the schema document serializes");

        let put = ok(
            &router,
            "PutSchema",
            &json!({ "policyStoreId": store_id, "definition": { "cedarJson": text } }),
        )
        .await;
        let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        assert_eq!(
            namespaces(&put["namespaces"]),
            expected,
            "case {case:?}: PutSchema reported the wrong namespaces, got {put}"
        );
        assert_eq!(
            put["policyStoreId"].as_str(),
            Some(store_id.as_str()),
            "case {case:?}: PutSchema reported the wrong policyStoreId, got {put}"
        );
        assert!(
            !put["createdDate"].is_null() && !put["lastUpdatedDate"].is_null(),
            "case {case:?}: createdDate/lastUpdatedDate are required, got {put}"
        );

        // The schema is only really stored if the reader reports it back.
        let fetched = ok(
            &router,
            "GetSchema",
            &json!({ "policyStoreId": store_id }),
        )
        .await;
        assert_eq!(
            fetched["schema"].as_str(),
            Some(text.as_str()),
            "case {case:?}: GetSchema reported a different schema, got {fetched}"
        );
        assert_eq!(
            namespaces(&fetched["namespaces"]),
            expected,
            "case {case:?}: GetSchema reported different namespaces, got {fetched}"
        );
        assert_eq!(
            fetched["createdDate"], put["createdDate"],
            "case {case:?}: createdDate changed between put and get, got {fetched}"
        );
        assert_eq!(
            fetched["lastUpdatedDate"], put["lastUpdatedDate"],
            "case {case:?}: lastUpdatedDate changed between put and get, got {fetched}"
        );
    }
}

/// Putting a schema over an existing one UPDATES it: the new definition replaces
/// the old one and the creation stamp stays the first put's.
#[tokio::test]
async fn putting_a_schema_again_updates_it_and_keeps_its_created_date() {
    let router = router();
    let store_id = store_with_mode(&router, "OFF").await;

    // Rows differ along the namespace the successive definitions declare, so each
    // put is observably a different schema from the one before it.
    let cases: &[(&str, Value, &[&str])] = &[
        ("first put", json!({ "A": namespace_block() }), &["A"]),
        (
            "replaced by two namespaces",
            json!({ "A": namespace_block(), "B": namespace_block() }),
            &["A", "B"],
        ),
        ("replaced by one again", json!({ "B": namespace_block() }), &["B"]),
        ("emptied", json!({}), &[]),
    ];

    let mut first_created: Option<Value> = None;
    for (case, schema, expected) in cases {
        let text = serde_json::to_string(schema).expect("the schema document serializes");
        let put = ok(
            &router,
            "PutSchema",
            &json!({ "policyStoreId": store_id, "definition": { "cedarJson": text } }),
        )
        .await;
        let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        assert_eq!(
            namespaces(&put["namespaces"]),
            expected,
            "case {case:?}: PutSchema reported the wrong namespaces, got {put}"
        );

        let created = first_created.get_or_insert_with(|| put["createdDate"].clone());
        assert_eq!(
            &put["createdDate"], created,
            "case {case:?}: an update must keep the schema's original createdDate, got {put}"
        );

        let fetched = ok(&router, "GetSchema", &json!({ "policyStoreId": store_id })).await;
        assert_eq!(
            fetched["schema"].as_str(),
            Some(text.as_str()),
            "case {case:?}: GetSchema did not report the schema just put, got {fetched}"
        );
        assert_eq!(
            namespaces(&fetched["namespaces"]),
            expected,
            "case {case:?}: GetSchema reported different namespaces, got {fetched}"
        );
    }
}

/// Text that is not a Cedar schema is refused, and nothing is persisted.
#[tokio::test]
async fn put_schema_refuses_a_definition_that_is_not_a_cedar_schema() {
    // Rows differ along WHY the document is not a schema: not JSON at all, JSON
    // of the wrong kind, an unknown type name, an action over an entity type that
    // is not declared, and a `memberOfTypes` entry that is not declared.
    let cases: &[(&str, Value)] = &[
        ("not JSON", json!("definitely not json")),
        ("a JSON array", json!("[]")),
        ("a JSON string", json!("\"PhotoApp\"")),
        (
            "an unknown attribute type",
            json!(serde_json::to_string(&json!({
                "PhotoApp": {
                    "entityTypes": {
                        "User": {
                            "shape": {
                                "type": "Record",
                                "attributes": { "age": { "type": "Bogus" } },
                            },
                        },
                    },
                    "actions": {},
                },
            }))
            .expect("serializes")),
        ),
        (
            "an action over an undeclared resource type",
            json!(serde_json::to_string(&json!({
                "PhotoApp": {
                    "entityTypes": { "User": {} },
                    "actions": {
                        "view": {
                            "appliesTo": {
                                "principalTypes": ["User"],
                                "resourceTypes": ["Photo"],
                            },
                        },
                    },
                },
            }))
            .expect("serializes")),
        ),
        (
            "a memberOfTypes entry that is not declared",
            json!(serde_json::to_string(&json!({
                "PhotoApp": {
                    "entityTypes": { "User": { "memberOfTypes": ["Group"] } },
                    "actions": {},
                },
            }))
            .expect("serializes")),
        ),
    ];

    let router = router();
    let store_id = store_with_mode(&router, "OFF").await;
    for (case, text) in cases {
        let (status, body) = call(
            &router,
            "PutSchema",
            &json!({ "policyStoreId": store_id, "definition": { "cedarJson": text } }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
    }

    // A refused put stores nothing, so the store still reports the EMPTY schema
    // document it has held since it was created.
    let fetched = ok(&router, "GetSchema", &json!({ "policyStoreId": store_id })).await;
    assert_eq!(
        fetched["schema"].as_str(),
        Some("{}"),
        "case \"nothing persisted\": a refused put must leave the empty document, got {fetched}"
    );
    assert_eq!(
        namespaces(&fetched["namespaces"]),
        Vec::<String>::new(),
        "case \"nothing persisted\": the empty document declares no namespace, got {fetched}"
    );
}

/// The schema text one store may hold is bounded, and the bound is reported with
/// the modelled quota error.
#[tokio::test]
async fn put_schema_bounds_the_schema_text_it_stores() {
    // The bound itself, and one byte past it.
    let cases: &[(&str, usize, u16, &str)] = &[
        ("at the byte bound", MAX_SCHEMA_BYTES, 200, ""),
        (
            "one byte past the bound",
            MAX_SCHEMA_BYTES + 1,
            402,
            "ServiceQuotaExceededException",
        ),
    ];

    let router = router();
    for (case, bytes, expected, expected_code) in cases {
        let store_id = store_with_mode(&router, "OFF").await;
        let text = schema_of(*bytes);
        assert_eq!(
            text.len(),
            *bytes,
            "case {case:?}: the generated schema is {} bytes, expected {bytes}",
            text.len()
        );
        let (status, body) = call(
            &router,
            "PutSchema",
            &json!({ "policyStoreId": store_id, "definition": { "cedarJson": text } }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&body),
                *expected_code,
                "case {case:?}: expected a {expected_code}, got {body}"
            );
        }
    }
}

/// A schema belongs to a policy store, so both operations refuse a reference that
/// resolves to no store.
#[tokio::test]
async fn schema_operations_report_what_is_missing() {
    let router = router();
    let schema = photo_app_schema();

    // Rows differ along WHICH operation is called and HOW the reference fails to
    // resolve — an unknown id, and an alias that names no store.
    let cases: &[(&str, &str, Value, &str)] = &[
        (
            "PutSchema on an unknown store id",
            "PutSchema",
            json!({ "policyStoreId": "PSNOSUCHSTORE", "definition": { "cedarJson": schema } }),
            "POLICY_STORE",
        ),
        (
            "PutSchema through an unknown alias",
            "PutSchema",
            json!({
                "policyStoreId": "policy-store-alias/nope",
                "definition": { "cedarJson": schema },
            }),
            // The reference named an ALIAS, so that is the resource reported
            // missing — `ResourceNotFoundException$resourceType` is "the resource
            // type of the resource referenced in the failed request".
            "POLICY_STORE_ALIAS",
        ),
        (
            "GetSchema on an unknown store id",
            "GetSchema",
            json!({ "policyStoreId": "PSNOSUCHSTORE" }),
            "POLICY_STORE",
        ),
        (
            "GetSchema through an unknown alias",
            "GetSchema",
            json!({ "policyStoreId": "policy-store-alias/nope" }),
            // The reference named an ALIAS, so that is the resource reported
            // missing — `ResourceNotFoundException$resourceType` is "the resource
            // type of the resource referenced in the failed request".
            "POLICY_STORE_ALIAS",
        ),
    ];

    for (case, operation, request, resource_type) in cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(*resource_type),
            "case {case:?}: expected resourceType {resource_type}, got {body}"
        );
    }
}

/// A policy store that has never been given a schema still HAS a schema document
/// to report: the empty one, `{}`, dated at the store's own creation. "No schema"
/// is a state of that one document — the state `PutSchema` returns a store to
/// with `{}` — not a missing resource, so `GetSchema` answers 200 and not a
/// `ResourceNotFoundException` naming `SCHEMA`.
#[tokio::test]
async fn get_schema_reports_the_empty_document_until_a_schema_is_stored() {
    let router = router();

    // Rows differ along the validation mode the store was created with: the
    // document a store starts with does not depend on it.
    for mode in ["OFF", "STRICT"] {
        let store_id = store_with_mode(&router, mode).await;
        let store = ok(&router, "GetPolicyStore", &json!({ "policyStoreId": store_id })).await;

        let empty = ok(&router, "GetSchema", &json!({ "policyStoreId": store_id })).await;
        assert_eq!(
            empty["schema"].as_str(),
            Some("{}"),
            "case {mode:?}: a store without a schema reports the empty document, got {empty}"
        );
        assert_eq!(
            namespaces(&empty["namespaces"]),
            Vec::<String>::new(),
            "case {mode:?}: the empty document declares no namespace, got {empty}"
        );
        assert_eq!(
            empty["policyStoreId"].as_str(),
            Some(store_id.as_str()),
            "case {mode:?}: GetSchema reported the wrong policyStoreId, got {empty}"
        );
        assert_eq!(
            empty["createdDate"], store["createdDate"],
            "case {mode:?}: the document dates from the store's creation, got {empty}"
        );
        assert_eq!(
            empty["lastUpdatedDate"], store["createdDate"],
            "case {mode:?}: nothing has updated the document yet, got {empty}"
        );

        // Reading it changed nothing: the second read is the first.
        let again = ok(&router, "GetSchema", &json!({ "policyStoreId": store_id })).await;
        assert_eq!(
            again, empty,
            "case {mode:?}: GetSchema must not change what a later read sees, got {again}"
        );

        // And a put replaces that document, which is what makes the empty one
        // the document's INITIAL state rather than a fixed answer.
        let text = photo_app_schema();
        ok(
            &router,
            "PutSchema",
            &json!({ "policyStoreId": store_id, "definition": { "cedarJson": text } }),
        )
        .await;
        let stored = ok(&router, "GetSchema", &json!({ "policyStoreId": store_id })).await;
        assert_eq!(
            stored["schema"].as_str(),
            Some(text.as_str()),
            "case {mode:?}: GetSchema must report the schema that was put, got {stored}"
        );
        assert_eq!(
            namespaces(&stored["namespaces"]),
            vec!["PhotoApp".to_string()],
            "case {mode:?}: GetSchema must report the stored namespaces, got {stored}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a stored schema governs
// ---------------------------------------------------------------------------

/// A `STRICT` store validates every static policy submitted to it against its
/// schema: one whose entity types, action and context attributes the schema
/// declares at the types it uses them at is stored, and any other is refused.
#[tokio::test]
async fn a_strict_store_validates_static_policies_against_its_schema() {
    let router = router();
    let store_id = store_with_mode(&router, "STRICT").await;

    // Before a schema exists there is nothing to validate against, so a STRICT
    // store refuses every static policy.
    let (status, body) = call(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement":
                "permit(principal == PhotoApp::User::\"alice\", \
                 action == PhotoApp::Action::\"viewPhoto\", \
                 resource == PhotoApp::Photo::\"vacation.jpg\");" } },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"strict without a schema\": expected HTTP 400, got {status} {body}"
    );

    ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": store_id,
            "definition": { "cedarJson": photo_app_schema() },
        }),
    )
    .await;

    // Rows differ along WHICH part of the policy the schema does or does not
    // sanction: the action, the entity types, a context attribute's existence,
    // and a context attribute's type.
    let cases: &[(&str, &str, u16)] = &[
        (
            "every part declared",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            200,
        ),
        (
            "a condition on a declared context attribute",
            "permit(principal == PhotoApp::User::\"bob\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"beach.jpg\") when { context.authenticated };",
            200,
        ),
        (
            "an unconstrained scope",
            "forbid(principal, action, resource) when { !context.authenticated };",
            200,
        ),
        (
            "an action the schema does not declare",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"deletePhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            400,
        ),
        (
            "an entity type the schema does not declare",
            "permit(principal == PhotoApp::Admin::\"root\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");",
            400,
        ),
        (
            "a context attribute the schema does not declare",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\") when { context.mfa };",
            400,
        ),
        (
            "a context attribute used at the wrong type",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\") \
             when { context.authenticated > 3 };",
            400,
        ),
    ];

    let mut stored = 0usize;
    for (case, statement, expected) in cases {
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected == 200 {
            stored += 1;
        } else {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
        }
    }

    // A refused policy is not in the store: only the accepted rows are.
    let listed = ok(&router, "ListPolicies", &json!({ "policyStoreId": store_id })).await;
    assert_eq!(
        listed["policies"].as_array().map_or(0, Vec::len),
        stored,
        "case \"only validated policies are stored\": expected {stored} policies, got {listed}"
    );

    // The SAME schema in an `OFF` store validates nothing: the mode is what
    // decides whether a validation failure blocks the write.
    let off_store = store_with_mode(&router, "OFF").await;
    ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": off_store,
            "definition": { "cedarJson": photo_app_schema() },
        }),
    )
    .await;
    let (status, body) = call(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": off_store,
            "definition": { "static": { "statement":
                "permit(principal == PhotoApp::User::\"alice\", \
                 action == PhotoApp::Action::\"deletePhoto\", \
                 resource == PhotoApp::Photo::\"vacation.jpg\");" } },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"validation OFF with a schema\": a store with validation off must accept a \
         policy the schema does not sanction, got {status} {body}"
    );
}

/// When a store holds a schema, that schema types an authorization request where
/// it types it — and a `context` or an entity value it does NOT type is still
/// evaluated, because the schema constrains the policies a store accepts rather
/// than the values a request carries. The type error a policy reading such a
/// value raises is reported in the 200 response's `errors` with the decision
/// failing closed, never as a refusal; the whole contract is tabulated in
/// [`crate::evaluation_errors`].
#[tokio::test]
async fn an_authorization_request_is_typed_against_the_stores_schema() {
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

    let base = json!({
        "policyStoreId": store_id,
        "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
        "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
        "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
    });

    // Rows differ along the TYPE and the presence of the context attribute the
    // schema declares as a boolean, over both context forms the model offers.
    // The third column is the decision the request must be evaluated into; the
    // fourth, whether evaluating the stored policy against it raises an error
    // that the response reports in `errors`. Every row is answered with a
    // decision: a value the schema cannot type is evaluated, not refused.
    let cases: &[(&str, Value, &str, bool)] = &[
        (
            "the declared boolean, true",
            json!({ "contextMap": { "authenticated": { "boolean": true } } }),
            "ALLOW",
            false,
        ),
        (
            "the declared boolean, false",
            json!({ "contextMap": { "authenticated": { "boolean": false } } }),
            "DENY",
            false,
        ),
        (
            "the declared boolean as cedarJson",
            json!({ "cedarJson": "{\"authenticated\": true}" }),
            "ALLOW",
            false,
        ),
        (
            "a string where the schema declares a boolean",
            json!({ "contextMap": { "authenticated": { "string": "true" } } }),
            "DENY",
            true,
        ),
        (
            "a long where the schema declares a boolean",
            json!({ "contextMap": { "authenticated": { "long": 1 } } }),
            "DENY",
            true,
        ),
        (
            "a set where the schema declares a boolean",
            json!({ "contextMap": { "authenticated": { "set": [{ "boolean": true }] } } }),
            "DENY",
            true,
        ),
        (
            "a record where the schema declares a boolean",
            json!({ "contextMap": { "authenticated": { "record": { "k": { "boolean": true } } } } }),
            "DENY",
            true,
        ),
        (
            "the wrong type through cedarJson",
            json!({ "cedarJson": "{\"authenticated\": \"true\"}" }),
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

    for (case, context, decision, errors) in cases {
        let mut request = base.clone();
        request["context"] = context.clone();
        let (status, body) = call(&router, "IsAuthorized", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an evaluation failure is reported in-band, so the call must \
             answer 200, got {status} {body}"
        );
        assert_eq!(
            body["decision"].as_str(),
            Some(*decision),
            "case {case:?}: expected decision {decision}, got {body}"
        );
        let reported = body["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            !reported.is_empty(),
            *errors,
            "case {case:?}: expected errors non-empty = {errors}, got {body}"
        );
    }

    // The same schema types the request's ENTITIES where it types them, and a
    // slice it does not type is evaluated too: rows differ along whether the
    // entity's type, and then its attributes, are what the schema declares.
    let entity_cases: &[(&str, Value)] = &[
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

    for (case, entities) in entity_cases {
        let mut request = base.clone();
        request["context"] = json!({ "contextMap": { "authenticated": { "boolean": true } } });
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
            "case {case:?}: the stored policy still decides the request, got {body}"
        );
    }
}
