// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `BatchIsAuthorizedWithToken`.
//!
//! The rule under test: a token-based batch decides EACH of its requests against the
//! policy store's persisted Cedar policies for the ONE principal the presented token's
//! verified claims name, and answers one result per request in the order they were
//! requested — each echoing its request and carrying that request's own decision,
//! determining policies and evaluation errors — plus the token's principal once for the
//! whole batch. Every result is the answer the single-request `IsAuthorizedWithToken`
//! gives for that request, and a token that does not verify is refused rather than
//! answered with a series of denials.
//!
//! Every assertion goes over the wire through the shared harness, so a decision is only
//! right if it really came out of state. Every token is minted by the Amazon Cognito
//! user-pool service mounted on the same engine and signed with that pool's own key, so
//! the signature check is the real one.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, error_code, fault_message};
// The `IsAuthorizedWithToken` suite owns the one token-minting harness this test crate
// has — a pool, an app client, a signed-in user and the identity source that trusts it —
// so a batch here presents the same really-signed tokens the single-request suite does.
use crate::is_authorized_with_token::{
    app_client, identity_source, no_identity_source_message, policy, pool_issuer, sign_in,
    with_tampered_signature, SignedIn, DOCUMENT, GROUP_TYPE, PERMITTED_GROUP, PRINCIPAL_TYPE,
};

/// The entity type every resource in this suite is named with.
const DOCUMENT_TYPE: &str = "App::Doc";

/// The action a read request asks about, as `(actionType, actionId)`.
const READ: (&str, &str) = ("App::Action", "read");

/// The action a write request asks about.
const WRITE: (&str, &str) = ("App::Action", "write");

/// A document the forbid policy below names.
const SECRET: &str = "secret";

/// How many requests one batch may carry, from `BatchIsAuthorizedWithToken`'s own model
/// documentation: "The `requests` of a `BatchIsAuthorizedWithToken` API request can
/// contain up to 30 requests."
const MAX_BATCH_REQUESTS: usize = 30;

/// The policies this suite's store holds, in the order their ids are collected.
///
/// They are written so that the three things a result can turn on are all exercised: the
/// principal's GROUP (which only the token can supply), a `forbid` overriding a `permit`,
/// and one request's own `context`.
fn statements(pool: &str) -> Vec<String> {
    vec![
        // 0: every member of the permitted group may read anything.
        format!(
            "permit(principal in {GROUP_TYPE}::\"{pool}|{PERMITTED_GROUP}\", \
             action == {}::\"{}\", resource);",
            READ.0, READ.1
        ),
        // 1: nobody may read the secret document — a forbid that overrides policy 0.
        format!(
            "forbid(principal, action == {}::\"{}\", resource == {DOCUMENT_TYPE}::\"{SECRET}\");",
            READ.0, READ.1
        ),
        // 2: a write of the one document, gated on the request's own context.
        format!(
            "permit(principal, action == {}::\"{}\", \
             resource == {DOCUMENT_TYPE}::\"{DOCUMENT}\") when {{ context.mfa == true }};",
            WRITE.0, WRITE.1
        ),
    ]
}

/// The seeded world every case below decides against: a router, the user pool whose
/// tokens the store's identity source trusts, two signed-in identities (one in the
/// permitted group, one in none) and the stored policies' ids in [`statements`] order.
struct Fixture {
    router: MultiServiceRouter,
    pool: String,
    store: String,
    member: SignedIn,
    outsider: SignedIn,
    policies: Vec<String>,
}

/// Seed [`Fixture`].
async fn fixture() -> Fixture {
    let (router, _engine) = harness::router_with_engine();
    let pool = harness::user_pool(&router, "avp-batch-token-pool").await;
    let client = app_client(&router, &pool, "avp-batch-token-client").await;
    let member = sign_in(
        &router,
        &pool,
        &client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;
    let outsider = sign_in(&router, &pool, &client, "outsider", &[], &json!([])).await;

    let store = harness::store_with_validation_off(&router).await;
    identity_source(&router, &store, &pool, &[&client], Some(GROUP_TYPE)).await;
    let mut policies = Vec::new();
    for statement in statements(&pool) {
        policies.push(policy(&router, &store, statement).await);
    }

    Fixture {
        router,
        pool,
        store,
        member,
        outsider,
        policies,
    }
}

/// One request inside a batch: the action and the document it asks about, plus a `context`
/// when the case carries one.
fn request(action: (&str, &str), document: &str, context: Option<Value>) -> Value {
    let mut item = json!({
        "action": { "actionType": action.0, "actionId": action.1 },
        "resource": { "entityType": DOCUMENT_TYPE, "entityId": document },
    });
    if let Some(context) = context {
        item["context"] = context;
    }
    item
}

/// A `contextMap` carrying the one attribute policy 2 reads.
fn mfa(satisfied: bool) -> Value {
    json!({ "contextMap": { "mfa": { "boolean": satisfied } } })
}

/// A `BatchIsAuthorizedWithToken` body against `store`, carrying whatever token members
/// `tokens` names and the given requests.
fn batch(store: &str, tokens: &Value, requests: &[Value]) -> Value {
    let mut body = json!({ "policyStoreId": store, "requests": requests });
    for (member, value) in tokens.as_object().expect("the token members are an object") {
        body[member.clone()] = value.clone();
    }
    body
}

/// A single-request `IsAuthorizedWithToken` body for ONE request of a batch: the same
/// store, the same token members and that request's own members.
fn single(store: &str, tokens: &Value, request: &Value) -> Value {
    let mut body = batch(store, tokens, &[]);
    body.as_object_mut()
        .expect("a request body is an object")
        .remove("requests");
    for (member, value) in request.as_object().expect("a request is an object") {
        body[member.clone()] = value.clone();
    }
    body
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

/// What one request of a batch must be answered with: a decision, and the indexes into
/// [`statements`] whose policy ids must be exactly its `determiningPolicies`.
struct Answer {
    decision: &'static str,
    determining: Vec<usize>,
}

/// An `Answer` for brevity at the call sites below.
fn answer(decision: &'static str, determining: &[usize]) -> Answer {
    Answer {
        decision,
        determining: determining.to_vec(),
    }
}

/// One case of the decision table below: the batch to send and the answers it must earn.
struct Row {
    case: &'static str,
    /// The token members the batch presents.
    tokens: Value,
    /// The batch's `requests`, in the order they are sent.
    requests: Vec<Value>,
    /// One [`Answer`] per request, in that same order.
    expected: Vec<Answer>,
    /// The entity id of the principal the presented token names.
    entity_id: String,
}

/// The one-request slice several cases below send, without cloning the request to make it.
fn only(request: &Value) -> &[Value] {
    std::slice::from_ref(request)
}

/// A batch answers one Cedar decision per request, in request order, for the principal the
/// presented token names — and reports that principal once for the whole batch.
///
/// The rows differ along what the rule generalises over: how many requests the batch
/// carries (one, a few, and the most one may carry), which member the token arrived in
/// (`identityToken`, `accessToken`, both), which identity it names (a group member and a
/// principal in no group), what varies across the requests (the resource, the action, the
/// context alone) and how the decisions mix (allow-only, deny-only, and both in one batch,
/// including a `forbid` overriding a `permit`).
#[tokio::test]
async fn a_batch_answers_one_decision_per_request_in_order_for_the_tokens_principal() {
    let world = fixture().await;

    let identity_token = json!({ "identityToken": world.member.id_token });
    let access_token = json!({ "accessToken": world.member.access_token });
    let both_tokens = json!({
        "accessToken": world.member.access_token,
        "identityToken": world.member.id_token,
    });
    let outsider_token = json!({ "identityToken": world.outsider.id_token });

    let largest: Vec<Value> = (0..MAX_BATCH_REQUESTS)
        .map(|index| request(READ, &format!("doc-{index}"), None))
        .collect();

    let rows: Vec<Row> = vec![
        Row {
            case: "one read by a group member, on an identity token",
            tokens: identity_token.clone(),
            requests: vec![request(READ, DOCUMENT, None)],
            expected: vec![answer("ALLOW", &[0])],
            entity_id: world.member.entity_id(),
        },
        Row {
            case: "a read, a forbidden read and a context-gated write, on an access token",
            tokens: access_token,
            requests: vec![
                request(READ, DOCUMENT, None),
                request(READ, SECRET, None),
                request(WRITE, DOCUMENT, Some(mfa(true))),
            ],
            expected: vec![
                answer("ALLOW", &[0]),
                answer("DENY", &[1]),
                answer("ALLOW", &[2]),
            ],
            entity_id: world.member.entity_id(),
        },
        Row {
            case: "two requests differing only in their context, on both tokens",
            tokens: both_tokens,
            requests: vec![
                request(WRITE, DOCUMENT, Some(mfa(true))),
                request(WRITE, DOCUMENT, Some(mfa(false))),
            ],
            expected: vec![answer("ALLOW", &[2]), answer("DENY", &[])],
            entity_id: world.member.entity_id(),
        },
        Row {
            case: "the most requests one batch may carry",
            tokens: identity_token,
            requests: largest.clone(),
            expected: largest.iter().map(|_| answer("ALLOW", &[0])).collect(),
            entity_id: world.member.entity_id(),
        },
        Row {
            case: "the same reads for a principal in no group",
            tokens: outsider_token,
            requests: vec![
                request(READ, DOCUMENT, None),
                request(READ, "doc-2", None),
                request(READ, SECRET, None),
            ],
            expected: vec![
                answer("DENY", &[]),
                answer("DENY", &[]),
                answer("DENY", &[1]),
            ],
            entity_id: world.outsider.entity_id(),
        },
    ];

    for row in rows {
        let case = row.case;
        let answered = harness::ok(
            &world.router,
            "BatchIsAuthorizedWithToken",
            &batch(&world.store, &row.tokens, &row.requests),
        )
        .await;

        assert_eq!(
            answered["principal"],
            json!({ "entityType": PRINCIPAL_TYPE, "entityId": row.entity_id }),
            "case {case:?}: the batch must report the token's principal, got {answered}"
        );
        let results = answered["results"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            row.requests.len(),
            "case {case:?}: expected one result per request, got {answered}"
        );

        for (index, wanted) in row.expected.iter().enumerate() {
            let result = &results[index];
            assert_eq!(
                result["request"], row.requests[index],
                "case {case:?} request {index}: the result must echo the request it decided, \
                 got {result}"
            );
            assert_eq!(
                result["decision"].as_str(),
                Some(wanted.decision),
                "case {case:?} request {index}: expected {}, got {result}",
                wanted.decision
            );
            let mut ids: Vec<String> = wanted
                .determining
                .iter()
                .map(|policy| world.policies[*policy].clone())
                .collect();
            ids.sort();
            assert_eq!(
                determining_ids(result),
                ids,
                "case {case:?} request {index}: expected determining policies {ids:?}, got \
                 {result}"
            );
            assert_eq!(
                result["errors"],
                json!([]),
                "case {case:?} request {index}: a clean evaluation reports no errors, got \
                 {result}"
            );
        }
    }
}

/// Each result of a batch is the answer the single-request `IsAuthorizedWithToken` gives
/// for that request: the batch is a series of the same decisions, not a cheaper
/// approximation of them.
///
/// The requests differ along the action, the resource and the context, so the equivalence
/// is asserted over an allow, a forbid-determined deny, a context-satisfied allow and a
/// deny with no determining policy at all.
#[tokio::test]
async fn every_result_matches_the_single_request_answer_for_that_request() {
    let world = fixture().await;
    let tokens = json!({ "identityToken": world.member.id_token });
    let requests = vec![
        request(READ, DOCUMENT, None),
        request(READ, SECRET, None),
        request(WRITE, DOCUMENT, Some(mfa(true))),
        request(WRITE, DOCUMENT, Some(mfa(false))),
    ];

    let batched = harness::ok(
        &world.router,
        "BatchIsAuthorizedWithToken",
        &batch(&world.store, &tokens, &requests),
    )
    .await;
    let results = batched["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        results.len(),
        requests.len(),
        "case \"the whole batch\": expected one result per request, got {batched}"
    );

    for (index, sent) in requests.iter().enumerate() {
        let alone = harness::ok(
            &world.router,
            "IsAuthorizedWithToken",
            &single(&world.store, &tokens, sent),
        )
        .await;
        let case = format!("request {index}: {sent}");
        assert_eq!(
            results[index]["decision"], alone["decision"],
            "case {case:?}: expected the single-request decision {}, got {}",
            alone["decision"], results[index]
        );
        assert_eq!(
            determining_ids(&results[index]),
            determining_ids(&alone),
            "case {case:?}: expected the single-request determining policies {:?}, got {}",
            determining_ids(&alone),
            results[index]
        );
        assert_eq!(
            results[index]["errors"], alone["errors"],
            "case {case:?}: expected the single-request errors {}, got {}",
            alone["errors"], results[index]
        );
        assert_eq!(
            batched["principal"], alone["principal"],
            "case {case:?}: both calls decide for the same token principal, got {batched} and \
             {alone}"
        );
    }
}

/// One `entities` slice is a property of the BATCH: it is supplied once and every request
/// is evaluated against it, so an attribute declared there decides the requests that read
/// it and only those.
#[tokio::test]
async fn one_entity_slice_serves_every_request_of_the_batch() {
    let world = fixture().await;
    // A second store, so the only policy in play is the one reading a resource attribute.
    let store = harness::store_with_validation_off(&world.router).await;
    identity_source(
        &world.router,
        &store,
        &world.pool,
        &[&world.member.client_id],
        Some(GROUP_TYPE),
    )
    .await;
    let public_only = policy(
        &world.router,
        &store,
        format!(
            "permit(principal, action == {}::\"{}\", resource) \
             when {{ resource.classification == \"public\" }};",
            READ.0, READ.1
        ),
    )
    .await;

    let classified = |document: &str, classification: &str| {
        json!({
            "identifier": { "entityType": DOCUMENT_TYPE, "entityId": document },
            "attributes": { "classification": { "string": classification } },
        })
    };
    let mut body = batch(
        &store,
        &json!({ "identityToken": world.member.id_token }),
        &[
            request(READ, DOCUMENT, None),
            request(READ, SECRET, None),
            request(READ, "doc-3", None),
        ],
    );
    body["entities"] = json!({
        "entityList": [
            classified(DOCUMENT, "public"),
            classified(SECRET, "restricted"),
            classified("doc-3", "public"),
        ],
    });

    let answered = harness::ok(&world.router, "BatchIsAuthorizedWithToken", &body).await;
    let results = answered["results"].as_array().cloned().unwrap_or_default();
    // (the request's index, the decision the slice's attribute makes for it)
    let expected: [(usize, &str); 3] = [(0, "ALLOW"), (1, "DENY"), (2, "ALLOW")];
    assert_eq!(
        results.len(),
        expected.len(),
        "case \"a slice shared by three requests\": expected one result per request, got \
         {answered}"
    );
    for (index, decision) in expected {
        assert_eq!(
            results[index]["decision"].as_str(),
            Some(decision),
            "case \"request {index} against the batch's entity slice\": expected {decision}, \
             got {}",
            results[index]
        );
        if decision == "ALLOW" {
            assert_eq!(
                determining_ids(&results[index]),
                vec![public_only.clone()],
                "case \"request {index} against the batch's entity slice\": the attribute \
                 policy must determine it, got {}",
                results[index]
            );
        }
    }
}

/// What a refusal row expects of the call.
enum Expected {
    /// A 400 `ValidationException` whose `fieldList` names these request members.
    InvalidInput(Vec<&'static str>),
    /// A 400 `ValidationException` whose message carries this clause.
    Refused(String),
    /// A 404 `ResourceNotFoundException` for this resource, with this message.
    NotFound {
        resource_type: &'static str,
        resource_id: String,
        message: String,
    },
}

/// A batch this operation's input contract refuses is refused WHOLE — one modelled error
/// naming what is wrong, and no `results` at all — rather than being answered with a
/// series of denials that would read as "evaluated and denied".
///
/// The rows differ along what is wrong: the tokens the batch presented, a member of one
/// request's half-triple, one request's payload, the batch's own size, the batch's
/// `entities`, and the resources the call references.
#[tokio::test]
async fn a_batch_the_input_contract_refuses_is_refused_whole() {
    let world = fixture().await;
    let tokens = json!({ "identityToken": world.member.id_token });
    let read = request(READ, DOCUMENT, None);

    // A pool the store holds no identity source for: its tokens are perfectly good and
    // this store cannot decide with them.
    let foreign_pool = harness::user_pool(&world.router, "avp-foreign-pool").await;
    let foreign_client = app_client(&world.router, &foreign_pool, "avp-foreign-client").await;
    let foreign = sign_in(
        &world.router,
        &foreign_pool,
        &foreign_client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;

    // A store id no store in this engine carries.
    let unknown_store = "PSdoesnotexist11111111";

    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            "no token at all",
            batch(&world.store, &json!({}), only(&read)),
            Expected::InvalidInput(vec!["/accessToken", "/identityToken"]),
        ),
        (
            "two tokens naming two different principals",
            batch(
                &world.store,
                &json!({
                    "accessToken": world.outsider.access_token,
                    "identityToken": world.member.id_token,
                }),
                only(&read),
            ),
            Expected::InvalidInput(vec!["/accessToken", "/identityToken"]),
        ),
        (
            "the only request carries no action",
            batch(
                &world.store,
                &tokens,
                &[json!({
                    "resource": { "entityType": DOCUMENT_TYPE, "entityId": DOCUMENT },
                })],
            ),
            Expected::InvalidInput(vec!["/requests/0/action"]),
        ),
        (
            "the second request carries no resource",
            batch(
                &world.store,
                &tokens,
                &[
                    read.clone(),
                    json!({ "action": { "actionType": READ.0, "actionId": READ.1 } }),
                ],
            ),
            Expected::InvalidInput(vec!["/requests/1/resource"]),
        ),
        (
            "the second request's context is no Cedar JSON",
            batch(
                &world.store,
                &tokens,
                &[
                    read.clone(),
                    request(READ, DOCUMENT, Some(json!({ "cedarJson": "not-json" }))),
                ],
            ),
            Expected::InvalidInput(vec!["/requests/1/context/cedarJson"]),
        ),
        (
            "one request more than a batch may carry",
            batch(
                &world.store,
                &tokens,
                &(0..=MAX_BATCH_REQUESTS)
                    .map(|index| request(READ, &format!("doc-{index}"), None))
                    .collect::<Vec<Value>>(),
            ),
            Expected::InvalidInput(vec!["/requests"]),
        ),
        (
            "the batch's entities carry an entity of the type the identity source generates",
            {
                let mut body = batch(&world.store, &tokens, only(&read));
                body["entities"] = json!({
                    "entityList": [{
                        "identifier": {
                            "entityType": PRINCIPAL_TYPE,
                            "entityId": world.member.entity_id(),
                        },
                    }],
                });
                body
            },
            Expected::InvalidInput(vec!["/entities"]),
        ),
        (
            "the batch's entities are no Cedar entity store",
            {
                let mut body = batch(&world.store, &tokens, only(&read));
                body["entities"] = json!({ "cedarJson": "{\"not\":\"a list\"}" });
                body
            },
            Expected::InvalidInput(vec!["/entities/cedarJson"]),
        ),
        (
            "a token whose signature was tampered with",
            batch(
                &world.store,
                &json!({ "identityToken": with_tampered_signature(&world.member.id_token) }),
                only(&read),
            ),
            Expected::Refused("Failed to verify IdentityToken: signature is invalid".to_string()),
        ),
        (
            "a token from an issuer this store holds no identity source for",
            batch(
                &world.store,
                &json!({ "identityToken": foreign.id_token }),
                only(&read),
            ),
            Expected::NotFound {
                resource_type: "IDENTITY_SOURCE",
                resource_id: pool_issuer(&foreign_pool),
                message: no_identity_source_message(&pool_issuer(&foreign_pool), &world.store),
            },
        ),
        (
            "a policy store that does not exist, beside a request with no action",
            batch(
                unknown_store,
                &tokens,
                &[json!({
                    "resource": { "entityType": DOCUMENT_TYPE, "entityId": DOCUMENT },
                })],
            ),
            Expected::NotFound {
                resource_type: "POLICY_STORE",
                resource_id: unknown_store.to_string(),
                message: format!("Policy store with id '{unknown_store}' not found"),
            },
        ),
    ];

    for (case, body, expected) in rows {
        let (status, answered) =
            harness::call(&world.router, "BatchIsAuthorizedWithToken", &body).await;
        assert_eq!(
            answered["results"],
            Value::Null,
            "case {case:?}: a refused batch decides nothing, got {answered}"
        );
        match expected {
            Expected::InvalidInput(paths) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {answered}"
                );
                assert_eq!(
                    error_code(&answered),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException, got {answered}"
                );
                let named: Vec<&str> = answered["fieldList"]
                    .as_array()
                    .map(|fields| {
                        fields
                            .iter()
                            .filter_map(|field| field["path"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    named, paths,
                    "case {case:?}: expected fieldList to name {paths:?}, got {answered}"
                );
            }
            Expected::Refused(clause) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {answered}"
                );
                assert_eq!(
                    error_code(&answered),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException, got {answered}"
                );
                assert_eq!(
                    fault_message(&answered),
                    clause,
                    "case {case:?}: expected the message {clause:?}, got {answered}"
                );
            }
            Expected::NotFound {
                resource_type,
                resource_id,
                message,
            } => {
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected HTTP 404, got {status} with {answered}"
                );
                assert_eq!(
                    error_code(&answered),
                    "ResourceNotFoundException",
                    "case {case:?}: expected a ResourceNotFoundException, got {answered}"
                );
                assert_eq!(
                    answered["resourceType"].as_str(),
                    Some(resource_type),
                    "case {case:?}: expected resourceType {resource_type}, got {answered}"
                );
                assert_eq!(
                    answered["resourceId"].as_str(),
                    Some(resource_id.as_str()),
                    "case {case:?}: expected resourceId {resource_id}, got {answered}"
                );
                assert_eq!(
                    fault_message(&answered),
                    message,
                    "case {case:?}: expected the message {message:?}, got {answered}"
                );
            }
        }
    }

    // The store is untouched by every refusal above: the batch that does verify still
    // decides as it did.
    let answered = harness::ok(
        &world.router,
        "BatchIsAuthorizedWithToken",
        &batch(&world.store, &tokens, only(&read)),
    )
    .await;
    assert_eq!(
        answered["results"][0]["decision"].as_str(),
        Some("ALLOW"),
        "case \"a verified batch after every refusal\": expected ALLOW, got {answered}"
    );
}
