// SPDX-License-Identifier: CC0-1.0

//! Event log configurations, end to end over the wire: `CreateEventLogConfiguration` ->
//! `GetEventLogConfiguration`.
//!
//! RULE: an event log configuration sets the logging level of ONE log target — the
//! `ResourceType` a create names (`*` for the whole account, or one of the resource types
//! the `SmartHomeResourceType` `@pattern` admits) together with the `ResourceId` inside that
//! type when it names one. `CreateEventLogConfiguration` (POST `/event-log-configurations`,
//! success 201) mints a fresh `LogConfigurationId` for the target, stores the submitted
//! level against it and answers with that id, and `GetEventLogConfiguration` (GET
//! `/event-log-configurations/{Id}`, `@readonly`) reports back exactly the target and level
//! that were stored. A target holds at most one configuration, so a SECOND create for the
//! same target is refused with the modelled `ConflictException` (HTTP 409) and leaves the
//! stored configuration untouched, while a retry carrying the `ClientToken`
//! (`@idempotencyToken`) of a create that already succeeded answers with that create's id
//! and performs no further action. An `Id` the caller's region holds no configuration under
//! is the modelled `ResourceNotFoundException` (HTTP 404), and a request outside a modelled
//! constraint — a level outside `LogLevel`, a resource type the pattern does not admit, a
//! resource id past its `@length` maximum — is the modelled `ValidationException` (HTTP 400)
//! that stores nothing.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.event-log-configuration-lifecycle-and-duplicate-conflict`) is
//! ONE row of the tables below. The rows differ along what the rule generalises over: the
//! resource type (the whole set the `@pattern` admits), whether the target names a resource
//! or a whole type, the form and length of the `ResourceId`, the `LogLevel` value (the
//! model's whole value set), the region the request is signed for, and whether a create
//! repeats a target or a client token.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    EVENT_LOG_CONFIGURATION_RESOURCE_TYPE, LOG_LEVEL_VALUES,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The collection route `CreateEventLogConfiguration` is bound to
/// (`smithy_query(query="operation",operation="CreateEventLogConfiguration")` -> POST
/// `/event-log-configurations`).
const COLLECTION: &str = "/event-log-configurations";

/// The wire error codes the model's `@httpError` shapes are rendered under.
const VALIDATION: &str = "ValidationException";
const CONFLICT: &str = "ConflictException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// Every `ResourceType` an event log configuration can address, read off the
/// `SmartHomeResourceType` `@pattern`
/// (`smithy_query(query="shape",shape="SmartHomeResourceType")` ->
/// `^[*]$|^(managed-thing|credential-locker|provisioning-profile|ota-task|account-association)$`):
/// the account wildcard, then the five resource types it names.
const RESOURCE_TYPES: &[&str] = &[
    "*",
    "managed-thing",
    "credential-locker",
    "provisioning-profile",
    "ota-task",
    "account-association",
];

/// The `@length` maximum of `SmartHomeResourceId`, in characters
/// (`smithy_query(query="shape",shape="SmartHomeResourceId")` -> length 1..200).
const RESOURCE_ID_MAX_LENGTH: usize = 200;

/// The `@length` bounds of `LogConfigurationId`, in characters
/// (`smithy_query(query="shape",shape="LogConfigurationId")` -> length 1..200, pattern
/// `^[A-Za-z0-9]+$`).
const LOG_CONFIGURATION_ID_BOUNDS: (usize, usize) = (1, 200);

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn string(&self, name: &str) -> Option<&str> {
        self.body
            .get(name)
            .filter(|value| !value.is_null())
            .and_then(Value::as_str)
    }

    /// The `Id` a create answered with.
    fn id(&self) -> Option<&str> {
        self.string("Id")
    }

    /// The reason an error body carries, on the member the model declares for it.
    fn message(&self) -> &str {
        self.string("Message")
            .or_else(|| self.string("message"))
            .unwrap_or_default()
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
    path: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path}"
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

/// `CreateEventLogConfiguration` with `body`, signed for `region`.
async fn create_in(router: &MultiServiceRouter, region: &str, body: Value) -> Reply {
    call_in(router, region, "POST", COLLECTION, Some(body)).await
}

/// [`create_in`] in [`REGION`].
async fn create(router: &MultiServiceRouter, body: Value) -> Reply {
    create_in(router, REGION, body).await
}

/// `GetEventLogConfiguration` for `id`, signed for `region`.
async fn get_in(router: &MultiServiceRouter, region: &str, id: &str) -> Reply {
    call_in(router, region, "GET", &format!("{COLLECTION}/{id}"), None).await
}

/// [`get_in`] in [`REGION`].
async fn get(router: &MultiServiceRouter, id: &str) -> Reply {
    get_in(router, REGION, id).await
}

/// The create request body that configures `level` for the target `resource_type`/
/// `resource_id`.
fn request(resource_type: &str, resource_id: Option<&str>, level: &str) -> Value {
    let mut body = json!({"ResourceType": resource_type, "EventLogLevel": level});
    if let Some(resource_id) = resource_id {
        body["ResourceId"] = json!(resource_id);
    }
    body
}

/// Assert one create was accepted, and answer the `LogConfigurationId` it minted — checking
/// that id against its own modelled shape.
fn accepted_id(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201, got {} with body {:?}",
        reply.status, reply.body
    );
    let id = reply.id().unwrap_or_default().to_string();
    let (min, max) = LOG_CONFIGURATION_ID_BOUNDS;
    assert!(
        (min..=max).contains(&id.chars().count()) && id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: expected a LogConfigurationId of {min}..={max} alphanumerics, \
         got {id:?} in body {:?}",
        reply.body
    );
    id
}

/// Assert one get reports exactly the target and level a create stored.
fn assert_reports(
    case: &str,
    reply: &Reply,
    id: &str,
    resource_type: &str,
    resource_id: Option<&str>,
    level: &str,
) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("Id"),
        Some(id),
        "case {case:?}: expected Id {id:?}, got {:?} in body {:?}",
        reply.string("Id"),
        reply.body
    );
    assert_eq!(
        reply.string("ResourceType"),
        Some(resource_type),
        "case {case:?}: expected ResourceType {resource_type:?}, got {:?} in body {:?}",
        reply.string("ResourceType"),
        reply.body
    );
    assert_eq!(
        reply.string("ResourceId"),
        resource_id,
        "case {case:?}: expected ResourceId {resource_id:?}, got {:?} in body {:?}",
        reply.string("ResourceId"),
        reply.body
    );
    assert_eq!(
        reply.string("EventLogLevel"),
        Some(level),
        "case {case:?}: expected EventLogLevel {level:?}, got {:?} in body {:?}",
        reply.string("EventLogLevel"),
        reply.body
    );
}

/// Assert one answer is the modelled refusal `status`/`code` carrying a reason.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected the modelled {code} code, got {:?}",
        reply.error_type
    );
    assert!(
        !reply.message().is_empty(),
        "case {case:?}: expected the modelled Message member to carry a reason, got body {:?}",
        reply.body
    );
}

/// A `ResourceId` of exactly `length` characters the `SmartHomeResourceId` `@pattern`
/// (`^[a-zA-Z0-9+*]*$`) admits, so a case can sit on the modelled `@length` bound or one
/// character past it.
fn resource_id_of_length(length: usize) -> String {
    "a".repeat(length)
}

/// RULE: a create mints a configuration for the log target it names and a get reads back
/// exactly the target and level that were stored — for every resource type the pattern
/// admits, for a target that names a whole type and one that names a resource inside it, and
/// for every `LogLevel` value. Each create mints its own identifier.
#[tokio::test]
async fn a_create_stores_the_target_and_level_and_a_get_reads_them_back() {
    let router = emulator();

    // The rows differ along what selects a configuration — the resource type and the
    // resource inside it — and along the level stored for it. Every row names a DISTINCT
    // target, so none of them is the duplicate the conflict rule covers below: the account
    // wildcard, each resource type as a whole, and several resource-specific targets whose
    // ids differ in form and length (alphanumeric, mixed case, the `+` and `*` characters
    // the pattern admits, one character, and exactly the modelled maximum).
    let mut cases: Vec<(&str, Option<String>)> = Vec::new();
    for resource_type in RESOURCE_TYPES {
        cases.push((resource_type, None));
    }
    for resource_id in [
        "thing1".to_string(),
        "ThingTWO".to_string(),
        "thing+three".to_string(),
        "*".to_string(),
        "t".to_string(),
        resource_id_of_length(RESOURCE_ID_MAX_LENGTH),
    ] {
        cases.push(("managed-thing", Some(resource_id)));
    }

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (row, (resource_type, resource_id)) in cases.iter().enumerate() {
        // Every `LogLevel` the model declares is exercised, cycling over the rows.
        let level = LOG_LEVEL_VALUES[row % LOG_LEVEL_VALUES.len()];
        let case = format!("{resource_type}/{resource_id:?}/{level}");
        let created = create(
            &router,
            request(resource_type, resource_id.as_deref(), level),
        )
        .await;
        let id = accepted_id(&case, &created);
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: every create mints its own identifier, but {id:?} was minted twice"
        );

        let read = get(&router, &id).await;
        assert_reports(
            &case,
            &read,
            &id,
            resource_type,
            resource_id.as_deref(),
            level,
        );

        // A get is a pure read: repeating it reports the same thing.
        let reread = get(&router, &id).await;
        assert_reports(
            &format!("{case}/reread"),
            &reread,
            &id,
            resource_type,
            resource_id.as_deref(),
            level,
        );
    }
}

/// RULE: a log target holds at most ONE configuration, so a second create for the same
/// target is the modelled 409 and leaves the stored configuration untouched — while a create
/// for any other target is accepted.
#[tokio::test]
async fn a_second_create_for_the_same_target_conflicts() {
    // The rows differ along the target that is repeated: the account wildcard, a whole
    // resource type, and a resource inside a type.
    let targets: Vec<(&str, Option<&str>)> = vec![
        ("*", None),
        ("ota-task", None),
        ("managed-thing", Some("thing1")),
        ("account-association", Some("assoc9")),
    ];

    for (resource_type, resource_id) in targets {
        // A fresh store per row, so the only configuration it holds is the row's own.
        let router = emulator();
        let case = format!("{resource_type}/{resource_id:?}");

        let first = create(&router, request(resource_type, resource_id, "INFO")).await;
        let id = accepted_id(&case, &first);

        // Repeating the target — at another level, so a conflict cannot be mistaken for an
        // accepted restatement — is refused, whatever level it asks for.
        for level in LOG_LEVEL_VALUES {
            let case = format!("{case}/again at {level}");
            let again = create(&router, request(resource_type, resource_id, level)).await;
            assert_refused(&case, &again, 409, CONFLICT);
            assert!(
                again.message().contains(&id),
                "case {case:?}: expected the reason to name the existing configuration {id:?}, \
                 got {:?}",
                again.message()
            );
            assert!(
                again.id().is_none(),
                "case {case:?}: a refused create mints no identifier, got {:?}",
                again.body
            );
        }

        // The first configuration is untouched by the refusals.
        let read = get(&router, &id).await;
        assert_reports(&case, &read, &id, resource_type, resource_id, "INFO");

        // Another target in the same store is still free.
        let other = create(&router, request("credential-locker", Some("locker1"), "WARN")).await;
        let other_id = accepted_id(&format!("{case}/another target"), &other);
        assert_ne!(
            other_id, id,
            "case {case:?}: a different target gets its own configuration"
        );
    }
}

/// RULE: a retry carrying the `ClientToken` of a create that already succeeded answers with
/// that create's identifier and performs no further action — so it neither conflicts nor
/// mints a second configuration — while a create for the same target under a DIFFERENT token
/// is the duplicate the conflict rule refuses.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_first_configuration() {
    // The rows differ along the token's form (the `ClientToken` `@pattern` is
    // `^[a-zA-Z0-9=_-]+$`, length 1..64) and along the target it created.
    let cases: Vec<(&str, &str, Option<&str>, &str)> = vec![
        ("t", "*", None, "DEBUG"),
        ("token-1", "managed-thing", Some("thing1"), "INFO"),
        ("TOKEN_2=", "ota-task", None, "WARN"),
        (
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "credential-locker",
            Some("locker1"),
            "ERROR",
        ),
    ];

    for (token, resource_type, resource_id, level) in cases {
        let router = emulator();
        let case = format!("{token}/{resource_type}/{resource_id:?}");

        let mut body = request(resource_type, resource_id, level);
        body["ClientToken"] = json!(token);

        let first = create(&router, body.clone()).await;
        let id = accepted_id(&case, &first);

        // Every retry under the same token answers with the first call's identifier.
        for attempt in 2..=3 {
            let case = format!("{case}/attempt {attempt}");
            let retry = create(&router, body.clone()).await;
            let replayed = accepted_id(&case, &retry);
            assert_eq!(
                replayed, id,
                "case {case:?}: a replayed ClientToken answers with the first identifier {id:?}, \
                 got {replayed:?}"
            );
        }

        // And it performed no further action: the stored configuration is the first one.
        let read = get(&router, &id).await;
        assert_reports(&case, &read, &id, resource_type, resource_id, level);

        // The same target under another token is an ordinary duplicate. The other token is
        // this one with its first character replaced, so it stays inside the modelled
        // `ClientToken` length however long the row's token is.
        let mut other = request(resource_type, resource_id, level);
        let other_token = format!("z{}", &token[1..]);
        other["ClientToken"] = json!(other_token);
        let duplicate = create(&router, other).await;
        assert_refused(&format!("{case}/another token"), &duplicate, 409, CONFLICT);
    }
}

/// RULE: a get answers the modelled 404 for an `Id` the caller's region holds no
/// configuration under — an identifier nothing minted, and one minted by another region's
/// endpoint — reporting the resource type and the identifier the request carried.
#[tokio::test]
async fn a_get_of_an_unknown_id_is_the_modelled_not_found() {
    let router = emulator();

    // A configuration minted in one region, to read from another.
    let elsewhere = create_in(
        &router,
        "eu-central-1",
        request("managed-thing", Some("thing1"), "INFO"),
    )
    .await;
    let elsewhere_id = accepted_id("eu-central-1 create", &elsewhere);

    // The rows differ along why the region holds nothing under the id: it was never minted
    // (in several forms the `LogConfigurationId` pattern admits), or it belongs to another
    // region's endpoint.
    let cases: Vec<(&str, String)> = vec![
        ("never minted", "a".to_string()),
        ("never minted, mixed case", "NoSuchConfiguration1".to_string()),
        ("never minted, digits", "0123456789".to_string()),
        (
            "never minted, at the modelled maximum",
            "b".repeat(LOG_CONFIGURATION_ID_BOUNDS.1),
        ),
        ("another region's configuration", elsewhere_id.clone()),
    ];

    for (why, id) in cases {
        let read = get(&router, &id).await;
        assert_refused(why, &read, 404, NOT_FOUND);
        assert_eq!(
            read.string("ResourceType"),
            Some(EVENT_LOG_CONFIGURATION_RESOURCE_TYPE),
            "case {why:?}: expected the 404 to name the resource type \
             {EVENT_LOG_CONFIGURATION_RESOURCE_TYPE:?}, got {:?}",
            read.string("ResourceType")
        );
        assert_eq!(
            read.string("ResourceId"),
            Some(id.as_str()),
            "case {why:?}: expected the 404 to report the identifier the request carried, \
             got {:?}",
            read.string("ResourceId")
        );
    }

    // The configuration is still there for the region that minted it: a refused read in
    // another region changed nothing.
    let read = get_in(&router, "eu-central-1", &elsewhere_id).await;
    assert_reports(
        "eu-central-1 read back",
        &read,
        &elsewhere_id,
        "managed-thing",
        Some("thing1"),
        "INFO",
    );
}

/// RULE: a create outside a modelled constraint is the modelled 400 and stores nothing — so
/// the target it named is still free afterwards.
#[tokio::test]
async fn a_create_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    // The rows differ along the constraint the request violates: the `LogLevel` value set,
    // the `SmartHomeResourceType` pattern, and the `SmartHomeResourceId` length — the last
    // one character past the modelled maximum, whose in-bounds neighbour is accepted in the
    // table above.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a level outside LogLevel",
            request("managed-thing", Some("thing1"), "TRACE"),
        ),
        (
            "a level in the wrong case",
            request("managed-thing", Some("thing1"), "info"),
        ),
        (
            "a resource type the pattern does not admit",
            request("device", Some("thing1"), "INFO"),
        ),
        (
            "a resource type with a trailing wildcard",
            request("managed-thing*", Some("thing1"), "INFO"),
        ),
        (
            "a resource id past the modelled maximum",
            request(
                "managed-thing",
                Some(&resource_id_of_length(RESOURCE_ID_MAX_LENGTH + 1)),
                "INFO",
            ),
        ),
    ];

    for (case, body) in cases {
        let router = emulator();
        let refused = create(&router, body).await;
        assert_refused(case, &refused, 400, VALIDATION);
        assert!(
            refused.id().is_none(),
            "case {case:?}: a refused create mints no identifier, got {:?}",
            refused.body
        );

        // Nothing was stored, so the target the refused request named is still free: the
        // create that names it now is accepted rather than refused as a duplicate.
        let accepted = create(&router, request("managed-thing", Some("thing1"), "INFO")).await;
        accepted_id(&format!("{case}/the target is still free"), &accepted);
    }
}
