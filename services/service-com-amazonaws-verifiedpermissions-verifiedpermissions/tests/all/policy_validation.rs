// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the two halves of a policy-validation refusal, and for
//! the policy that is NOT refused.
//!
//! 1. Every request this service refuses on its own input checks answers with
//!    the model's own `ValidationException` — HTTP 400, the fixed `message`, and
//!    a `fieldList` whose entry names the offending member by its JSON-pointer
//!    `path`. The table below spans the operations and the members that rule
//!    covers, and the kinds of failure (absent, unparseable, over-long,
//!    not-ours, out of this emulator's reach, past a bound). A refusal by the
//!    store's SCHEMA is its own rule, pinned in
//!    [`a_schema_refusal_reports_every_reason_at_the_validated_member`].
//! 2. A schema-conformant static policy submitted to a `STRICT` store is STORED,
//!    and every member of the create response comes back out of state through
//!    `GetPolicy` and `ListPolicies`.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the harness in [`crate::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::INVALID_INPUT_MESSAGE;
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::state::cedar::{
    MAX_BATCH_REQUESTS, MAX_ENTITIES_PER_REQUEST, MAX_STATEMENT_BYTES,
};

use crate::harness::{call, error_code, ok, router, store_with_mode};
use crate::schema::photo_app_schema;

/// `{entityType, entityId}` of a scope member, or `None` when the response omits
/// it — which is what the model says happens for a scope that names no entity.
fn entity(value: &Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    Some((
        object.get("entityType")?.as_str()?.to_string(),
        object.get("entityId")?.as_str()?.to_string(),
    ))
}

/// `{actionType, actionId}` pairs of an `actions` list, sorted, so a comparison
/// does not depend on the order the parser walked the scope in.
fn actions(value: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| {
            Some((
                item.get("actionType")?.as_str()?.to_string(),
                item.get("actionId")?.as_str()?.to_string(),
            ))
        })
        .collect();
    out.sort();
    out
}

/// A statement whose text is longer than [`MAX_STATEMENT_BYTES`], reached by
/// padding a string literal inside a condition, so the only thing wrong with it
/// is its size.
fn over_long_statement() -> String {
    format!(
        "permit(principal, action, resource) when {{ context.tag == \"{}\" }};",
        "x".repeat(MAX_STATEMENT_BYTES + 1)
    )
}

/// An `entities.entityList` carrying one more entity than
/// [`MAX_ENTITIES_PER_REQUEST`] allows.
fn too_many_entities() -> Value {
    let items: Vec<Value> = (0..=MAX_ENTITIES_PER_REQUEST)
        .map(|n| {
            json!({ "identifier": { "entityType": "PhotoApp::User", "entityId": n.to_string() } })
        })
        .collect();
    json!({ "entityList": items })
}

/// One request of a batch: a complete authorization triple, or one missing its
/// `principal` when `principal` is `None`.
fn batch_request(principal: Option<&str>) -> Value {
    let mut request = json!({
        "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
        "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
    });
    if let Some(principal) = principal {
        request["principal"] = json!({ "entityType": "PhotoApp::User", "entityId": principal });
    }
    request
}

/// A batch carrying one more request than [`MAX_BATCH_REQUESTS`] allows, every
/// one of them well formed and sharing its principal, so the only thing wrong
/// with the batch is its size.
fn oversized_batch() -> Value {
    Value::Array(
        (0..=MAX_BATCH_REQUESTS)
            .map(|_| batch_request(Some("alice")))
            .collect(),
    )
}

/// One request of a batch whose principal carries `entity_type`, so a row can
/// vary the principal's TYPE while keeping the batch's shared resource.
fn batch_request_typed(entity_type: &str, entity_id: &str) -> Value {
    let mut request = batch_request(Some(entity_id));
    request["principal"]["entityType"] = json!(entity_type);
    request
}

// ---------------------------------------------------------------------------
// The refusal
// ---------------------------------------------------------------------------

/// A request refused by this service's own input checks answers with the
/// modelled `ValidationException`: status 400, the fixed message, and a
/// `fieldList` entry whose `path` is the JSON pointer of the member at fault and
/// whose `message` says why.
///
/// The rows span the operations that validate caller input, the members each one
/// validates, and the reason each is refused — the recorded CreatePolicy case
/// (`/definition/static/statement`) is one row of many.
#[tokio::test]
async fn a_refused_request_names_the_offending_member_in_field_list() {
    let router = router();
    let off_store = store_with_mode(&router, "OFF").await;
    let strict_store = store_with_mode(&router, "STRICT").await;

    let conformant = "permit(principal == PhotoApp::User::\"alice\", \
                      action == PhotoApp::Action::\"viewPhoto\", \
                      resource == PhotoApp::Photo::\"vacation.jpg\");";

    // (case, operation, request, the path the refusal must name)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            // The member at fault is the statement's own CONTENT, which no
            // schema was consulted about: the store's validation mode is OFF.
            "a statement that is not Cedar",
            "CreatePolicy",
            json!({
                "policyStoreId": off_store,
                "definition": { "static": { "statement": "this is not cedar" } },
            }),
            "/definition/static/statement",
        ),
        (
            "a statement longer than one policy may carry",
            "CreatePolicy",
            json!({
                "policyStoreId": off_store,
                "definition": { "static": { "statement": over_long_statement() } },
            }),
            "/definition/static/statement",
        ),
        (
            "a STRICT store with no schema to validate against",
            "CreatePolicy",
            json!({
                "policyStoreId": strict_store,
                "definition": { "static": { "statement": conformant } },
            }),
            // Nothing is wrong with the statement here: the store named by
            // `policyStoreId` is the one holding no schema.
            "/policyStoreId",
        ),
        (
            "a schema that is not JSON",
            "PutSchema",
            json!({
                "policyStoreId": off_store,
                "definition": { "cedarJson": "definitely not json" },
            }),
            "/definition/cedarJson",
        ),
        (
            "a schema that is JSON but not a Cedar schema",
            "PutSchema",
            json!({
                "policyStoreId": off_store,
                "definition": { "cedarJson": "{\"PhotoApp\":{\"entityTypes\":{\"User\":{\"memberOfTypes\":[\"Group\"]}},\"actions\":{}}}" },
            }),
            "/definition/cedarJson",
        ),
        (
            "a cursor this service did not issue",
            "ListPolicies",
            json!({ "policyStoreId": off_store, "nextToken": "notacursor" }),
            "/nextToken",
        ),
        (
            "an authorization request with no principal",
            "IsAuthorized",
            json!({
                "policyStoreId": off_store,
                "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
            }),
            "/principal",
        ),
        (
            "an authorization request with no action",
            "IsAuthorized",
            json!({
                "policyStoreId": off_store,
                "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
            }),
            "/action",
        ),
        (
            "an authorization request with no resource",
            "IsAuthorized",
            json!({
                "policyStoreId": off_store,
                "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
            }),
            "/resource",
        ),
        (
            "a principal whose type is not a Cedar entity type",
            "IsAuthorized",
            json!({
                "policyStoreId": off_store,
                "principal": { "entityType": "1 not a type", "entityId": "alice" },
                "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
            }),
            "/principal",
        ),
        (
            "a context whose cedarJson is not JSON",
            "IsAuthorized",
            json!({
                "policyStoreId": off_store,
                "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
                "context": { "cedarJson": "{not json" },
            }),
            "/context/cedarJson",
        ),
        (
            // Valid JSON, but not a Cedar context: the refusal comes from the
            // evaluation and must still name the union ARM the client sent.
            "a context whose cedarJson is JSON but not a Cedar context",
            "IsAuthorized",
            json!({
                "policyStoreId": off_store,
                "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
                "context": { "cedarJson": "[1, 2]" },
            }),
            "/context/cedarJson",
        ),
        (
            "entities whose cedarJson is not JSON",
            "IsAuthorized",
            json!({
                "policyStoreId": off_store,
                "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
                "entities": { "cedarJson": "{not json" },
            }),
            "/entities/cedarJson",
        ),
        (
            "more entities than one request may carry",
            "IsAuthorized",
            json!({
                "policyStoreId": off_store,
                "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
                "action": { "actionType": "PhotoApp::Action", "actionId": "viewPhoto" },
                "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation.jpg" },
                "entities": too_many_entities(),
            }),
            "/entities",
        ),
        (
            "an alias name without the documented prefix",
            "CreatePolicyStoreAlias",
            json!({ "policyStoreId": off_store, "aliasName": "no-prefix" }),
            "/aliasName",
        ),
        (
            // Every resource this emulator holds is in its own account, so an
            // ARN naming another one names a pool that does not exist here.
            "a Cognito user pool this emulator cannot resolve",
            "CreateIdentitySource",
            json!({
                "policyStoreId": off_store,
                "configuration": {
                    "cognitoUserPoolConfiguration": {
                        "userPoolArn":
                            "arn:aws:cognito-idp:us-east-1:123456789012:userpool/us-east-1_1a2b3c4d5",
                    },
                },
            }),
            "/configuration/cognitoUserPoolConfiguration/userPoolArn",
        ),
        (
            // An OIDC source's principals carry an entity type only the
            // caller's own schema names, so it cannot be defaulted.
            "an OIDC identity source with no principalEntityType",
            "CreateIdentitySource",
            json!({
                "policyStoreId": off_store,
                "configuration": {
                    "openIdConnectConfiguration": {
                        "issuer": "https://auth.example.com",
                        "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
                    },
                },
            }),
            "/principalEntityType",
        ),
        (
            // The published batch size: the model bounds `requests` below but
            // not above, so the list's own size is refused here.
            "more authorization requests than one batch may carry",
            "BatchIsAuthorized",
            json!({
                "policyStoreId": off_store,
                "requests": oversized_batch(),
            }),
            "/requests",
        ),
        (
            // One request of a batch is a complete Cedar triple too, so the
            // refusal names WHICH request, by its position in the list.
            "a batch whose second request has no principal",
            "BatchIsAuthorized",
            json!({
                "policyStoreId": off_store,
                "requests": [
                    batch_request(Some("alice")),
                    batch_request(None),
                ],
            }),
            "/requests/1/principal",
        ),
        (
            // The evaluation of ONE request of a batch names the member within
            // that request, so the refusal carries the request's index too.
            "a batch whose second request has a principal type that is not Cedar",
            "BatchIsAuthorized",
            json!({
                "policyStoreId": off_store,
                "requests": [
                    batch_request(Some("alice")),
                    batch_request_typed("1 not a type", "bob"),
                ],
            }),
            "/requests/1/principal",
        ),
    ];

    for (case, operation, request, path) in &cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(INVALID_INPUT_MESSAGE),
            "case {case:?}: expected the fixed message {INVALID_INPUT_MESSAGE:?}, got {body}"
        );
        let fields = body["fieldList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            fields.len(),
            1,
            "case {case:?}: expected exactly one fieldList entry, got {body}"
        );
        assert_eq!(
            fields[0]["path"].as_str(),
            Some(*path),
            "case {case:?}: expected fieldList path {path:?}, got {body}"
        );
        assert!(
            fields[0]["message"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "case {case:?}: expected a non-empty fieldList message, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The schema refusal
// ---------------------------------------------------------------------------

/// Policy content the store's SCHEMA refuses answers with one `fieldList` entry
/// per reason the Cedar validator raised, every entry naming the request member
/// whose CONTENT was validated — `/definition` where the policy content arrives
/// inside a definition, `/statement` where the operation carries it at the top
/// level — and never one entry concatenating several reasons, nor an entry at a
/// deeper member the schema said nothing about.
///
/// The rows differ along the operation that validates (create, update, template
/// create), the arm the content arrives in, and HOW MANY distinct reasons the
/// policy raises: a policy naming two undeclared entity types raises one reason
/// per type, which is what pins "one entry per reason" rather than "one entry".
#[tokio::test]
async fn a_schema_refusal_reports_every_reason_at_the_validated_member() {
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

    // A policy the schema does sanction, so the UpdatePolicy row has something
    // to replace and only its ACTION changes — the one scope element an update
    // may change — leaving the schema as the only thing that can refuse it.
    let created = ok(
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
    let policy_id = created["policyId"].as_str().unwrap_or_default().to_string();

    // (case, operation, request, the member whose content was validated, how
    //  many reasons the policy raises at least)
    let cases: Vec<(&str, &str, Value, &str, usize)> = vec![
        (
            // The recorded case: an action id the schema never declares.
            "CreatePolicy naming an action the schema does not declare",
            "CreatePolicy",
            json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement":
                    "permit(principal == PhotoApp::User::\"alice\", \
                     action == PhotoApp::Action::\"deletePhoto\", \
                     resource == PhotoApp::Photo::\"vacation.jpg\");" } },
            }),
            "/definition",
            1,
        ),
        (
            // TWO undeclared entity types: two independent reasons, so two
            // entries. One concatenated entry would fail here.
            "CreatePolicy naming two entity types the schema does not declare",
            "CreatePolicy",
            json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement":
                    "permit(principal == PhotoApp::Ghost::\"g\", \
                     action == PhotoApp::Action::\"viewPhoto\", \
                     resource == PhotoApp::Album::\"a\");" } },
            }),
            "/definition",
            2,
        ),
        (
            // An attribute the schema's entity shape does not declare.
            "CreatePolicy reading an attribute the schema does not declare",
            "CreatePolicy",
            json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement":
                    "permit(principal == PhotoApp::User::\"alice\", \
                     action == PhotoApp::Action::\"viewPhoto\", \
                     resource == PhotoApp::Photo::\"vacation.jpg\") \
                     when { principal.nickname == \"al\" };" } },
            }),
            "/definition",
            1,
        ),
        (
            // The same rule on the UPDATE path: the replacement changes only
            // the action, so the schema is what refuses it.
            "UpdatePolicy replacing the action with one the schema does not declare",
            "UpdatePolicy",
            json!({
                "policyStoreId": store_id,
                "policyId": policy_id,
                "definition": { "static": { "statement":
                    "permit(principal == PhotoApp::User::\"alice\", \
                     action == PhotoApp::Action::\"deletePhoto\", \
                     resource == PhotoApp::Photo::\"vacation.jpg\");" } },
            }),
            "/definition",
            1,
        ),
        (
            // A policy TEMPLATE carries its Cedar text at the top level, so the
            // validated member is `/statement`.
            "CreatePolicyTemplate naming an action the schema does not declare",
            "CreatePolicyTemplate",
            json!({
                "policyStoreId": store_id,
                "statement": "permit(principal == ?principal, \
                              action == PhotoApp::Action::\"deletePhoto\", \
                              resource == PhotoApp::Photo::\"vacation.jpg\");",
            }),
            "/statement",
            1,
        ),
    ];

    for (case, operation, request, path, least) in &cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(INVALID_INPUT_MESSAGE),
            "case {case:?}: expected the fixed message {INVALID_INPUT_MESSAGE:?}, got {body}"
        );
        let fields = body["fieldList"].as_array().cloned().unwrap_or_default();
        assert!(
            fields.len() >= *least,
            "case {case:?}: expected at least {least} fieldList entries, one per validation \
             reason, got {body}"
        );
        for field in &fields {
            assert_eq!(
                field["path"].as_str(),
                Some(*path),
                "case {case:?}: every schema-validation entry names the validated member \
                 {path:?}, got {body}"
            );
            let reason = field["message"].as_str().unwrap_or_default();
            assert!(
                !reason.is_empty(),
                "case {case:?}: expected a non-empty fieldList message, got {body}"
            );
            // One entry per reason, so no entry carries several of them joined
            // together — which is exactly what a concatenated message looks like.
            assert!(
                !reason.contains("; "),
                "case {case:?}: each reason gets its own entry rather than being \
                 concatenated, got {reason:?}"
            );
        }
        let distinct: std::collections::BTreeSet<&str> = fields
            .iter()
            .map(|field| field["message"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            distinct.len(),
            fields.len(),
            "case {case:?}: each entry reports a DIFFERENT reason, got {body}"
        );
    }

    // Nothing above was stored: the refused create and the refused update left
    // the store holding exactly the one policy that validated, with its original
    // statement.
    let listed = ok(&router, "ListPolicies", &json!({ "policyStoreId": store_id })).await;
    assert_eq!(
        listed["policies"].as_array().map(Vec::len),
        Some(1),
        "a refused policy must not be stored, got {listed}"
    );
    let fetched = ok(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": policy_id }),
    )
    .await;
    assert!(
        fetched["definition"]["static"]["statement"]
            .as_str()
            .is_some_and(|statement| statement.contains("viewPhoto")),
        "a refused update must leave the stored statement alone, got {fetched}"
    );
}

// ---------------------------------------------------------------------------
// The policy that is not refused
// ---------------------------------------------------------------------------

/// A static policy the store's schema sanctions is created in a `STRICT` store
/// and persists: the create response carries every modelled member read off the
/// Cedar text, and `GetPolicy` and `ListPolicies` report the same policy back.
///
/// The rows differ along the FORM of the statement's scope — which of the three
/// scope slots names an entity, how many actions it names, and the effect — which
/// is what decides whether `principal`, `resource` and `actions` are present.
#[tokio::test]
async fn a_schema_conformant_static_policy_is_stored_and_read_back() {
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

    type Scope = (
        Option<(&'static str, &'static str)>,
        Option<(&'static str, &'static str)>,
        Vec<(&'static str, &'static str)>,
    );
    // (case, statement, effect, (principal, resource, actions))
    let cases: Vec<(&str, String, &str, Scope)> = vec![
        (
            "every scope slot names an entity",
            "permit(principal == PhotoApp::User::\"alice\", \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"vacation.jpg\");"
                .to_string(),
            "Permit",
            (
                Some(("PhotoApp::User", "alice")),
                Some(("PhotoApp::Photo", "vacation.jpg")),
                vec![("PhotoApp::Action", "viewPhoto")],
            ),
        ),
        (
            "a forbid with an unconstrained scope",
            "forbid(principal, action, resource) when { !context.authenticated };".to_string(),
            "Forbid",
            (None, None, Vec::new()),
        ),
        (
            "a scope naming two actions",
            "permit(principal == PhotoApp::User::\"bob\", \
             action in [PhotoApp::Action::\"viewPhoto\", PhotoApp::Action::\"sharePhoto\"], \
             resource == PhotoApp::Photo::\"beach.jpg\");"
                .to_string(),
            "Permit",
            (
                Some(("PhotoApp::User", "bob")),
                Some(("PhotoApp::Photo", "beach.jpg")),
                vec![
                    ("PhotoApp::Action", "sharePhoto"),
                    ("PhotoApp::Action", "viewPhoto"),
                ],
            ),
        ),
        (
            "a principal named by type only",
            "permit(principal is PhotoApp::User, \
             action == PhotoApp::Action::\"viewPhoto\", \
             resource == PhotoApp::Photo::\"beach.jpg\");"
                .to_string(),
            "Permit",
            (
                None,
                Some(("PhotoApp::Photo", "beach.jpg")),
                vec![("PhotoApp::Action", "viewPhoto")],
            ),
        ),
    ];

    let mut created: Vec<(String, String)> = Vec::new();
    for (case, statement, effect, (principal, resource, expected_actions)) in &cases {
        let body = call(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
        assert_eq!(
            body.0, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            body.0, body.1
        );
        let body = body.1;

        let policy_id = body["policyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !policy_id.is_empty(),
            "case {case:?}: policyId is required on the response, got {body}"
        );
        assert_eq!(
            body["policyStoreId"].as_str(),
            Some(store_id.as_str()),
            "case {case:?}: expected policyStoreId {store_id}, got {body}"
        );
        assert_eq!(
            body["policyType"].as_str(),
            Some("STATIC"),
            "case {case:?}: expected policyType STATIC, got {body}"
        );
        assert_eq!(
            body["effect"].as_str(),
            Some(*effect),
            "case {case:?}: expected effect {effect}, got {body}"
        );
        assert_eq!(
            entity(&body["principal"]),
            principal.map(|(t, i)| (t.to_string(), i.to_string())),
            "case {case:?}: unexpected principal in {body}"
        );
        assert_eq!(
            entity(&body["resource"]),
            resource.map(|(t, i)| (t.to_string(), i.to_string())),
            "case {case:?}: unexpected resource in {body}"
        );
        assert_eq!(
            actions(&body["actions"]),
            expected_actions
                .iter()
                .map(|(t, i)| (t.to_string(), i.to_string()))
                .collect::<Vec<_>>(),
            "case {case:?}: unexpected actions in {body}"
        );
        assert!(
            !body["createdDate"].is_null() && !body["lastUpdatedDate"].is_null(),
            "case {case:?}: createdDate and lastUpdatedDate are required, got {body}"
        );

        // The policy exists only if a later READ finds it: `GetPolicy` reports
        // the same scope and the Cedar text byte for byte.
        let got = ok(
            &router,
            "GetPolicy",
            &json!({ "policyStoreId": store_id, "policyId": policy_id }),
        )
        .await;
        assert_eq!(
            got["definition"]["static"]["statement"].as_str(),
            Some(statement.as_str()),
            "case {case:?}: GetPolicy must report the statement as it was given, got {got}"
        );
        assert_eq!(
            got["effect"].as_str(),
            Some(*effect),
            "case {case:?}: GetPolicy reported a different effect: {got}"
        );
        assert_eq!(
            entity(&got["principal"]),
            entity(&body["principal"]),
            "case {case:?}: GetPolicy reported a different principal: {got}"
        );
        assert_eq!(
            actions(&got["actions"]),
            actions(&body["actions"]),
            "case {case:?}: GetPolicy reported different actions: {got}"
        );
        created.push((policy_id, effect.to_string()));
    }

    // Every created policy lists, each item carrying the same identity and
    // effect the create reported.
    let listed = ok(&router, "ListPolicies", &json!({ "policyStoreId": store_id })).await;
    let items = listed["policies"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        created.len(),
        "expected {} listed policies, got {listed}",
        created.len()
    );
    for (policy_id, effect) in &created {
        let item = items
            .iter()
            .find(|item| item["policyId"].as_str() == Some(policy_id.as_str()));
        let item = item.unwrap_or_else(|| {
            panic!("case {policy_id:?}: the created policy must list, got {listed}")
        });
        assert_eq!(
            item["policyType"].as_str(),
            Some("STATIC"),
            "case {policy_id:?}: expected policyType STATIC, got {item}"
        );
        assert_eq!(
            item["effect"].as_str(),
            Some(effect.as_str()),
            "case {policy_id:?}: expected effect {effect}, got {item}"
        );
        assert_eq!(
            item["policyStoreId"].as_str(),
            Some(store_id.as_str()),
            "case {policy_id:?}: expected policyStoreId {store_id}, got {item}"
        );
        assert!(
            item["definition"]["static"].is_object(),
            "case {policy_id:?}: a listed static policy carries definition.static, got {item}"
        );
    }
}
