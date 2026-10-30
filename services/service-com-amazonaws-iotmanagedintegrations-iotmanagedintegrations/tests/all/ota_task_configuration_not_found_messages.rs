// SPDX-License-Identifier: CC0-1.0

//! How this service states that the over-the-air (OTA) task configuration a request named
//! does not exist.
//!
//! RULE: the sentence a 404 carries belongs to the PATH that resolved the resource, not to
//! the resource alone, so a DELETE that finds nothing to delete states its own reason
//! rather than the reason a READ of that same identifier states. For an OTA task
//! configuration both answers are the modelled `ResourceNotFoundException` (HTTP 404, wire
//! code `ResourceNotFoundException`,
//! `smithy_query(query="shape",shape="ResourceNotFoundException")` -> members `Message`,
//! `ResourceId`, `ResourceType`, `httpError` 404) with neither optional member filled, and
//! they differ in `Message`:
//!
//! * a READ (`GET /ota-task-configurations/{Identifier}`) quotes the identifier the
//!   request carried — `OTA task configuration not found for id: <the Identifier sent>`;
//! * a DELETE (`DELETE /ota-task-configurations/{Identifier}`) states that the delete
//!   found nothing to remove, in one sentence with no variable part at all — `Failed to
//!   delete Ota task configuration, resource does not exist.`
//!
//! Either sentence is the same whatever made the record absent: never created, already
//! deleted, or held only by another region.
//!
//! The recorded refusal this suite was written for is ONE row of the table below (a delete
//! of an id nothing ever minted); the other rows span what the rule generalises over: the
//! operation that resolved the identifier, the form of the identifier itself — both ends of
//! `OtaTaskConfigurationId`'s `@length` (1..64) and the character classes its `@pattern`
//! (`^[a-zA-Z0-9]*$`) admits — and the cause of the absence.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the statuses, headers and bodies asserted here are the ones a client sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The route the OTA task-configuration collection is bound to
/// (`smithy_query(query="operation",operation="CreateOtaTaskConfiguration")` ->
/// `POST /ota-task-configurations`); a single configuration hangs off it under its id.
const COLLECTION: &str = "/ota-task-configurations";

/// The wire error code the model's `@httpError` 404 shape is rendered under.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The constant sentence the DELETE path states when it finds nothing to delete, written
/// out here rather than imported so the suite pins the text a client reads and not the
/// implementation's own formatter. It has no variable part, so it is the same for every
/// request.
const DELETE_REASON: &str = "Failed to delete Ota task configuration, resource does not exist.";

/// The fixed half of the sentence the READ path states; the identifier the request carried
/// follows it.
const READ_REASON_PREFIX: &str = "OTA task configuration not found for id: ";

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
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
/// credential scope is what routes the request to this service and names the region its
/// records belong to.
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

/// Which path resolved the identifier, which is what decides the reason it states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolver {
    Read,
    Delete,
}

impl Resolver {
    /// The operation this path is served by, and its HTTP method
    /// (`smithy_query(query="digest")`: `GetOtaTaskConfiguration` GET and
    /// `DeleteOtaTaskConfiguration` DELETE, both on
    /// `/ota-task-configurations/{Identifier}`).
    fn operation(self) -> &'static str {
        match self {
            Self::Read => "GetOtaTaskConfiguration",
            Self::Delete => "DeleteOtaTaskConfiguration",
        }
    }

    fn method(self) -> &'static str {
        match self {
            Self::Read => "GET",
            Self::Delete => "DELETE",
        }
    }

    /// The sentence the rule requires of this path.
    fn expected_message(self, identifier: &str) -> String {
        match self {
            Self::Read => format!("{READ_REASON_PREFIX}{identifier}"),
            Self::Delete => DELETE_REASON.to_string(),
        }
    }
}

/// Every path that resolves an OTA task configuration by the `Identifier` its request
/// carries. Each row of the identifier table is driven through every one of these.
const RESOLVERS: &[Resolver] = &[Resolver::Read, Resolver::Delete];

/// Resolve `identifier` through `resolver` and assert the answer against the rule,
/// answering the `Message` it carried.
async fn assert_not_found(
    router: &MultiServiceRouter,
    region: &str,
    resolver: Resolver,
    case: &str,
    identifier: &str,
) -> String {
    let reply = call_in(
        router,
        region,
        resolver.method(),
        &format!("{COLLECTION}/{identifier}"),
        None,
    )
    .await;
    let case = format!("{}/{case}", resolver.operation());
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
    let expected = resolver.expected_message(identifier);
    assert_eq!(
        reply.string("Message"),
        Some(expected.as_str()),
        "case {case:?}: expected the reason {expected:?}, got {:?}",
        reply.member("Message")
    );
    // The reason is the whole body: this resource's 404 fills neither optional member of
    // the error shape, on either path.
    for member in ["ResourceId", "ResourceType"] {
        assert_eq!(
            reply.member(member),
            None,
            "case {case:?}: expected the 404 to carry no {member}, got {:?} in body {:?}",
            reply.member(member),
            reply.body
        );
    }
    reply
        .string("Message")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got body {:?}", reply.body))
        .to_string()
}

/// Create one configuration in `region` and answer the service-generated id it is held
/// under.
async fn create_in(router: &MultiServiceRouter, region: &str, case: &str) -> String {
    let created = call_in(
        router,
        region,
        "POST",
        COLLECTION,
        Some(json!({"Name": "not found suite config"})),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateOtaTaskConfiguration, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("TaskConfigurationId")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a TaskConfigurationId, got body {:?}",
                created.body
            )
        })
        .to_string()
}

/// Create one configuration and delete it again, answering the id it was held under: an
/// identifier the suite did not choose, absent because it was removed.
async fn create_then_delete(router: &MultiServiceRouter, case: &str) -> String {
    let id = create_in(router, REGION, case).await;
    let removed = call(router, "DELETE", &format!("{COLLECTION}/{id}"), None).await;
    assert_eq!(
        removed.status, 204,
        "case {case:?}: expected HTTP 204 from DeleteOtaTaskConfiguration, got {} with body {:?}",
        removed.status, removed.body
    );
    id
}

/// The identifier forms the table spans: `OtaTaskConfigurationId` is `@length` 1..64,
/// `@pattern` `^[a-zA-Z0-9]*$`
/// (`smithy_query(query="operation",operation="DeleteOtaTaskConfiguration")`), so the rows
/// are both ends of the bound and each character class the pattern admits. The first row is
/// the recorded one.
fn identifier_forms() -> Vec<(String, String)> {
    vec![
        ("the recorded id", "nonexistentotaconfig00001".to_string()),
        ("one character, the length minimum", "z".to_string()),
        ("64 characters, the length maximum", "a".repeat(64)),
        ("digits only", "0123456789".to_string()),
        ("mixed case", "MiXeDcAsEiD".to_string()),
    ]
    .into_iter()
    .map(|(form, identifier)| (form.to_string(), identifier))
    .collect()
}

/// RULE (see the module docs): each path states a missing OTA task configuration with ITS
/// OWN reason — the read quotes the identifier, the delete states the constant
/// delete sentence — for every form of identifier the model admits.
#[tokio::test]
async fn a_missing_ota_task_configuration_is_stated_by_the_resolving_path() {
    let router = emulator();

    for (form, identifier) in identifier_forms() {
        for &resolver in RESOLVERS {
            assert_not_found(&router, REGION, resolver, &form, &identifier).await;
        }
    }
}

/// RULE: the sentence does not depend on WHY the record is absent — one that was deleted,
/// and one that exists only in another region, are stated exactly as one that was never
/// created. A delete of an absent configuration is also the second call of an idempotent
/// delete, so repeating it answers the same way every time.
#[tokio::test]
async fn a_deleted_or_out_of_region_configuration_is_stated_the_same_way() {
    let router = emulator();

    // Absent because a delete removed it, under an id the service minted.
    let deleted = create_then_delete(&router, "deleted").await;
    for &resolver in RESOLVERS {
        let reason = assert_not_found(&router, REGION, resolver, "deleted", &deleted).await;
        assert_eq!(
            reason,
            resolver.expected_message(&deleted),
            "case \"deleted\": the cause of the absence must not change the reason, got \
             {reason:?}"
        );
    }

    // Repeating the delete of an id already gone: the same refusal, call after call.
    let mut repeated = Vec::new();
    for attempt in 0..3 {
        let case = format!("repeated delete {attempt}");
        repeated.push(assert_not_found(&router, REGION, Resolver::Delete, &case, &deleted).await);
    }
    let distinct: BTreeSet<&str> = repeated.iter().map(String::as_str).collect();
    assert_eq!(
        distinct.len(),
        1,
        "every repeat of a delete that finds nothing must state the same sentence, got \
         {distinct:?}"
    );

    // Absent in this region because another region holds it: an OTA task configuration is
    // regional.
    let elsewhere = create_in(&router, "us-west-2", "other region").await;
    for &resolver in RESOLVERS {
        assert_not_found(&router, REGION, resolver, "other region", &elsewhere).await;
    }
    // The region that does hold it still does: the refusals above removed nothing.
    let still_there = call_in(
        &router,
        "us-west-2",
        "GET",
        &format!("{COLLECTION}/{elsewhere}"),
        None,
    )
    .await;
    assert_eq!(
        still_there.status, 200,
        "case \"other region\": the owning region must still hold the record, got {} with body \
         {:?}",
        still_there.status, still_there.body
    );
}

/// RULE, the half the two paths do NOT share: the DELETE path's reason is
/// REQUEST-INDEPENDENT — one constant sentence ([`DELETE_REASON`]) for every absent
/// configuration, byte-identical across every form of identifier the model admits and
/// never echoing the identifier the request carried — while the READ path's reason is
/// built from the request, so it differs per identifier and quotes it.
#[tokio::test]
async fn the_delete_paths_reason_is_constant_while_the_reads_is_built_from_the_request() {
    let router = emulator();

    let mut forms = identifier_forms();
    // A last row whose absence has a different cause, under an id the service minted.
    forms.push((
        "deleted, under the id the service minted".to_string(),
        create_then_delete(&router, "deleted").await,
    ));

    let mut delete_reasons: Vec<(String, String)> = Vec::new();
    let mut read_reasons: BTreeSet<String> = BTreeSet::new();
    for (form, identifier) in &forms {
        let deleted = assert_not_found(&router, REGION, Resolver::Delete, form, identifier).await;
        assert!(
            !deleted.contains(identifier.as_str()),
            "case {form:?}: the delete path's reason must not echo the identifier, got \
             {deleted:?}"
        );
        delete_reasons.push((form.clone(), deleted));

        let read = assert_not_found(&router, REGION, Resolver::Read, form, identifier).await;
        assert!(
            read.contains(identifier.as_str()),
            "case {form:?}: the read path's reason must quote the identifier, got {read:?}"
        );
        read_reasons.insert(read);
    }

    for (form, reason) in &delete_reasons {
        assert_eq!(
            reason, DELETE_REASON,
            "case {form:?}: expected the delete path's constant reason {DELETE_REASON:?}, got \
             {reason:?}"
        );
    }
    let distinct: BTreeSet<&str> = delete_reasons
        .iter()
        .map(|(_, reason)| reason.as_str())
        .collect();
    assert_eq!(
        distinct.len(),
        1,
        "every delete that finds nothing must state the same sentence, got {distinct:?}"
    );
    assert_eq!(
        read_reasons.len(),
        forms.len(),
        "each read states its own identifier, so the reasons must all differ, got \
         {read_reasons:?}"
    );
}

/// RULE: a delete that DOES find the configuration removes it and answers the modelled
/// 204 — the refusal above is reserved for an identifier this region holds nothing under,
/// and a read of the removed id is the read path's reason, so the two paths' wordings are
/// pinned against the same round trip.
#[tokio::test]
async fn a_delete_that_finds_the_configuration_answers_the_modelled_success() {
    let router = emulator();

    let id = create_in(&router, REGION, "round trip").await;
    let read = call(&router, "GET", &format!("{COLLECTION}/{id}"), None).await;
    assert_eq!(
        read.status, 200,
        "case \"round trip\": expected HTTP 200 from GetOtaTaskConfiguration before the delete, \
         got {} with body {:?}",
        read.status, read.body
    );

    let removed = call(&router, "DELETE", &format!("{COLLECTION}/{id}"), None).await;
    assert_eq!(
        removed.status, 204,
        "case \"round trip\": expected HTTP 204 from DeleteOtaTaskConfiguration, got {} with \
         body {:?}",
        removed.status, removed.body
    );

    assert_not_found(&router, REGION, Resolver::Read, "round trip", &id).await;
    assert_not_found(&router, REGION, Resolver::Delete, "round trip", &id).await;
}
