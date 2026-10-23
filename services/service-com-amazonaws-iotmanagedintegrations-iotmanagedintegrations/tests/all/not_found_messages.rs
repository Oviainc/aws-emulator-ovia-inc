// SPDX-License-Identifier: CC0-1.0

//! How this service states that the managed thing a request named does not exist.
//!
//! RULE: an operation that resolves a managed thing by the `Identifier` its request
//! carries, and finds none, is answered with the modelled `ResourceNotFoundException`
//! (HTTP 404, wire code `ResourceNotFoundException`,
//! `smithy_query(query="shape",shape="ResourceNotFoundException")` -> members `Message`,
//! `ResourceId`, `ResourceType`, `httpError` 404), whose `ResourceId` is the identifier
//! the request carried and whose `Message` is the reason the resolving path states:
//!
//! * a MUTATION builds it from the request — `<ResourceType> for identifier '<the
//!   Identifier sent>' not found`, with the identifier reproduced exactly as the request
//!   spelled it, no re-casing and no truncation;
//! * a READ states which KIND of resource was not found — `<Resource name> resource not
//!   found.` — and names the identifier only on `ResourceId`.
//!
//! Either sentence is the same whatever made the record absent: never created, deleted,
//! or created in another region.
//!
//! The recorded 404s this rule was derived from are one row of the table below
//! (`nonexistentmanagedthing0001` on `GetManagedThing`, `UpdateManagedThing` and
//! `DeleteManagedThing`); the other rows span what the rule generalises over: the
//! operation that resolves the identifier, and the form of the identifier itself — its
//! length at both ends of `ManagedThingId`'s `@length` (1..64), the punctuation its
//! `@pattern` (`^[a-zA-Z0-9:_-]*$`) admits, a digits-only id, and mixed case.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the statuses, headers and bodies asserted here are the ones a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The wire error code the model's `@httpError` 404 shape is rendered under.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn string(&self, name: &str) -> Option<&str> {
        self.body.get(name).and_then(Value::as_str)
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path_and_query}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// [`call_in`] in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    call_in(router, REGION, method, path_and_query, body).await
}

/// Whether the operation that resolved the identifier only REPORTS the resource or
/// MUTATES it, which is what decides the reason it states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolver {
    Read,
    Mutation,
}

/// One operation that resolves a managed thing by the `Identifier` its request carries:
/// its name, its HTTP method, the body it sends (`None` for the methods that carry none)
/// and which kind of resolver it is. Every row of the identifier table is driven through
/// every one of these.
///
/// `smithy_query(query="digest")`: `GetManagedThing` GET, `UpdateManagedThing` PUT and
/// `DeleteManagedThing` DELETE, all on `/managed-things/{Identifier}`.
struct Resolving {
    operation: &'static str,
    method: &'static str,
    body: Option<Value>,
    kind: Resolver,
}

fn resolving_operations() -> Vec<Resolving> {
    vec![
        Resolving {
            operation: "GetManagedThing",
            method: "GET",
            body: None,
            kind: Resolver::Read,
        },
        // An update that asks for one member to change: the 404 comes from resolving the
        // identifier, not from what the body holds.
        Resolving {
            operation: "UpdateManagedThing",
            method: "PUT",
            body: Some(json!({"Name": "lamp one"})),
            kind: Resolver::Mutation,
        },
        Resolving {
            operation: "DeleteManagedThing",
            method: "DELETE",
            body: None,
            kind: Resolver::Mutation,
        },
    ]
}

/// The sentence the rule requires, written out here rather than imported so the test pins
/// the text a client reads and not the implementation's own formatter.
fn expected_message(kind: Resolver, identifier: &str) -> String {
    match kind {
        Resolver::Read => "Managed Thing resource not found.".to_string(),
        Resolver::Mutation => format!("ManagedThing for identifier '{identifier}' not found"),
    }
}

/// Assert one answer against the rule.
fn assert_not_found(case: &str, reply: &Reply, kind: Resolver, identifier: &str) {
    let expected = expected_message(kind, identifier);
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(RESOURCE_NOT_FOUND),
        "case {case:?}: expected the modelled {RESOURCE_NOT_FOUND} code, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.string("Message"),
        Some(expected.as_str()),
        "case {case:?}: expected the reason {expected:?}, got {:?}",
        reply.string("Message")
    );
    assert_eq!(
        reply.string("ResourceId"),
        Some(identifier),
        "case {case:?}: the 404 names the identifier asked for, got {:?}",
        reply.string("ResourceId")
    );
    assert_eq!(
        reply.string("ResourceType"),
        Some("ManagedThing"),
        "case {case:?}: the 404 names the resource type, got {:?}",
        reply.string("ResourceType")
    );
}

/// RULE (see the module docs): every operation that resolves a managed thing states a
/// missing one with its resolving path's reason, for every form of identifier the model
/// admits.
#[tokio::test]
async fn a_missing_managed_thing_is_stated_by_the_resolving_path() {
    let router = emulator();

    // `ManagedThingId`: `@length` 1..64, `@pattern` ^[a-zA-Z0-9:_-]*$
    // (`smithy_query(query="operation",operation="GetManagedThing")`). The rows differ
    // along the identifier's form: the recorded id, both ends of the length bound, the
    // punctuation the pattern admits, digits only, and mixed case.
    let longest = "f".repeat(64);
    let identifiers = vec![
        ("the recorded id", "nonexistentmanagedthing0001".to_string()),
        ("one character, the length minimum", "a".to_string()),
        ("64 characters, the length maximum", longest.clone()),
        (
            "the punctuation the pattern admits",
            "thing:with_punctuation-0".to_string(),
        ),
        ("digits only", "0123456789".to_string()),
        ("mixed case", "MiXeDcAsEiD".to_string()),
    ];

    for (form, identifier) in &identifiers {
        for resolving in resolving_operations() {
            let case = format!("{}/{form}", resolving.operation);
            let reply = call(
                &router,
                resolving.method,
                &format!("/managed-things/{identifier}"),
                resolving.body,
            )
            .await;
            assert_not_found(&case, &reply, resolving.kind, identifier);
        }
    }
}

/// RULE: the same reasons state a managed thing that is absent for any other cause — one
/// that was deleted, and one that exists only in another region — so nothing about either
/// sentence depends on the record never having existed.
#[tokio::test]
async fn a_deleted_or_out_of_region_managed_thing_is_stated_the_same_way() {
    let router = emulator();

    let create = json!({
        "Role": "DEVICE",
        "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
        "AuthenticationMaterial": "qr:0000000000",
    });

    // A record created and then deleted: every resolving operation states the
    // service-generated id it was holding.
    let created = call(&router, "POST", "/managed-things", Some(create.clone())).await;
    assert_eq!(
        created.status, 201,
        "case \"deleted\": expected HTTP 201 from CreateManagedThing, got {} with body {:?}",
        created.status, created.body
    );
    let deleted_id = created
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case \"deleted\": expected an Id, got body {:?}",
                created.body
            )
        })
        .to_string();
    let removed = call(
        &router,
        "DELETE",
        &format!("/managed-things/{deleted_id}"),
        None,
    )
    .await;
    assert_eq!(
        removed.status, 204,
        "case \"deleted\": expected HTTP 204 from DeleteManagedThing, got {} with body {:?}",
        removed.status, removed.body
    );
    for resolving in resolving_operations() {
        let case = format!("{}/deleted", resolving.operation);
        let reply = call(
            &router,
            resolving.method,
            &format!("/managed-things/{deleted_id}"),
            resolving.body,
        )
        .await;
        assert_not_found(&case, &reply, resolving.kind, &deleted_id);
    }

    // A record that exists, addressed through another region's endpoint: a managed thing
    // is regional, so that region holds none under this id.
    let elsewhere = call(&router, "POST", "/managed-things", Some(create)).await;
    assert_eq!(
        elsewhere.status, 201,
        "case \"other region\": expected HTTP 201 from CreateManagedThing, got {} with body {:?}",
        elsewhere.status, elsewhere.body
    );
    let live_id = elsewhere
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case \"other region\": expected an Id, got body {:?}",
                elsewhere.body
            )
        })
        .to_string();
    for resolving in resolving_operations() {
        let case = format!("{}/other region", resolving.operation);
        let reply = call_in(
            &router,
            "us-west-2",
            resolving.method,
            &format!("/managed-things/{live_id}"),
            resolving.body,
        )
        .await;
        assert_not_found(&case, &reply, resolving.kind, &live_id);
    }
}

/// RULE: a mutation's reason is built from THIS request's identifier — two requests that
/// name different missing things are stated differently, and neither sentence names the
/// other's identifier. A read's reason names no identifier at all, so it is the same
/// sentence for both while its `ResourceId` still differs.
#[tokio::test]
async fn two_requests_are_stated_from_their_own_identifiers() {
    let router = emulator();

    let first = "firstmissingthing";
    let second = "secondmissingthing";
    for resolving in resolving_operations() {
        let one = call(
            &router,
            resolving.method,
            &format!("/managed-things/{first}"),
            resolving.body.clone(),
        )
        .await;
        let other = call(
            &router,
            resolving.method,
            &format!("/managed-things/{second}"),
            resolving.body,
        )
        .await;

        let case = format!("{}/two identifiers", resolving.operation);
        assert_not_found(&case, &one, resolving.kind, first);
        assert_not_found(&case, &other, resolving.kind, second);
        assert!(
            !one.string("Message").unwrap_or_default().contains(second),
            "case {case:?}: the refusal must not name the other identifier, got {:?}",
            one.string("Message")
        );
        if resolving.kind == Resolver::Mutation {
            assert_ne!(
                one.string("Message"),
                other.string("Message"),
                "case {case:?}: a mutation's reason is built from its own identifier, got {:?} \
                 for both",
                one.string("Message")
            );
        }
    }
}
