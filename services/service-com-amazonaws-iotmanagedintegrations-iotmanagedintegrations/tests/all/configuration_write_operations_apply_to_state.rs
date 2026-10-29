// SPDX-License-Identifier: CC0-1.0

//! What a DELETE and an UPDATE of a configuration do to the stored record, end to end over
//! the wire.
//!
//! RULE: a write operation on a configuration this service identifies by the id its request
//! carries is applied to the stored record before it answers success — a delete REMOVES the
//! record, so every later operation that resolves that id answers the modelled
//! `ResourceNotFoundException` (HTTP 404) and the resource it configured is free again, and
//! an update APPLIES the member it carries, so every later read reports the new value and
//! every member the request does not address is left as it was. An id no record is held
//! under is that same 404, whatever made it absent — never created, already deleted, or
//! created in another region — and a request a modelled constraint refuses changes nothing.
//!
//! Each resource states its 404 in its own words, which is what a client reads:
//! an event log configuration's reason is fixed text naming nothing from the request
//! ([`EVENT_LOG_ABSENT_REASON`]), an OTA task configuration's quotes the identifier
//! ([`ota_task_absent_reason`]), and neither carries the optional `ResourceId` or
//! `ResourceType` member of the error shape.
//!
//! The recorded units this was derived from (a scoped and a wildcard event log
//! configuration deleted then read, an OTA task configuration deleted then read, and a
//! wildcard configuration whose level was updated then read) are single rows of the tables
//! below. The rows differ along what the rule generalises over: the log target (every
//! `ResourceType` the pattern admits, with and without a `ResourceId`), the `LogLevel`
//! value on both sides of an update (the model's whole value set), which members an OTA
//! create carried, the operation that resolves the absent id, the cause of the absence, and
//! the region the request is signed for.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, headers and bodies asserted here are the ones a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::LOG_LEVEL_VALUES;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The collection route the event-log-configuration operations are bound to
/// (`smithy_query(query="operation",operation="CreateEventLogConfiguration")` -> POST
/// `/event-log-configurations`; the member routes add `/{Id}`).
const EVENT_LOG_COLLECTION: &str = "/event-log-configurations";

/// The collection route the OTA-task-configuration operations are bound to
/// (`smithy_query(query="operation",operation="CreateOtaTaskConfiguration")` -> POST
/// `/ota-task-configurations`; the member routes add `/{Identifier}`).
const OTA_TASK_COLLECTION: &str = "/ota-task-configurations";

/// The wire error codes the model's `@httpError` shapes are rendered under.
const NOT_FOUND: &str = "ResourceNotFoundException";
const VALIDATION: &str = "ValidationException";

/// The modelled success status of each write under test
/// (`smithy_query(query="operation",operation="DeleteEventLogConfiguration,DeleteOtaTaskConfiguration,UpdateEventLogConfiguration")`
/// -> 200, 204 and 200).
const DELETE_EVENT_LOG_STATUS: u16 = 200;
const DELETE_OTA_TASK_STATUS: u16 = 204;
const UPDATE_EVENT_LOG_STATUS: u16 = 200;

/// Every `ResourceType` an event log configuration can address, read off the
/// `SmartHomeResourceType` `@pattern`
/// (`smithy_query(query="shape",shape="SmartHomeResourceType")` ->
/// `^[*]$|^(managed-thing|credential-locker|provisioning-profile|ota-task|account-association)$`).
const RESOURCE_TYPES: &[&str] = &[
    "*",
    "managed-thing",
    "credential-locker",
    "provisioning-profile",
    "ota-task",
    "account-association",
];

/// The sentence this service states for an absent event log configuration, written out here
/// rather than imported so the test pins the text a client reads and not the
/// implementation's own constant. It names nothing the request carried.
const EVENT_LOG_ABSENT_REASON: &str = "Event log configuration doesn't exist";

/// What a read reports on `ResourceId` for a configuration whose target names no resource,
/// written out here for the same reason.
const UNSCOPED_RESOURCE_ID: &str = "unknown";

/// The sentence this service states for an absent OTA task configuration: fixed text plus
/// the identifier the request carried.
fn ota_task_absent_reason(identifier: &str) -> String {
    format!("OTA task configuration not found for id: {identifier}")
}

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

    fn message(&self) -> &str {
        self.string("Message").unwrap_or_default()
    }
}

/// A router holding one `iotmanagedintegrations` service over `engine`: the same assembly
/// `core/router` performs, with one service mounted. The caller keeps the engine handle, so
/// a case can check the store itself and not only what the wire reports.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
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

/// [`call_in`] in [`REGION`].
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    call_in(router, REGION, method, path, body).await
}

/// The create body that configures `level` for the log target `resource_type`/`resource_id`.
fn event_log_request(resource_type: &str, resource_id: Option<&str>, level: &str) -> Value {
    let mut body = json!({"ResourceType": resource_type, "EventLogLevel": level});
    if let Some(resource_id) = resource_id {
        body["ResourceId"] = json!(resource_id);
    }
    body
}

/// `CreateEventLogConfiguration`, asserting the modelled 201 and answering the minted id.
async fn create_event_log(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    resource_id: Option<&str>,
    level: &str,
) -> String {
    let reply = call(
        router,
        "POST",
        EVENT_LOG_COLLECTION,
        Some(event_log_request(resource_type, resource_id, level)),
    )
    .await;
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201 from CreateEventLogConfiguration, got {} with body {:?}",
        reply.status, reply.body
    );
    reply
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: expected an Id, got body {:?}", reply.body))
        .to_string()
}

/// `GetEventLogConfiguration` for `id`.
async fn get_event_log(router: &MultiServiceRouter, id: &str) -> Reply {
    call(router, "GET", &format!("{EVENT_LOG_COLLECTION}/{id}"), None).await
}

/// `UpdateEventLogConfiguration` for `id`, asking for `level`.
async fn update_event_log(router: &MultiServiceRouter, id: &str, level: &str) -> Reply {
    call(
        router,
        "PATCH",
        &format!("{EVENT_LOG_COLLECTION}/{id}"),
        Some(json!({"EventLogLevel": level})),
    )
    .await
}

/// `DeleteEventLogConfiguration` for `id`.
async fn delete_event_log(router: &MultiServiceRouter, id: &str) -> Reply {
    call(
        router,
        "DELETE",
        &format!("{EVENT_LOG_COLLECTION}/{id}"),
        None,
    )
    .await
}

/// `CreateOtaTaskConfiguration` with `body`, asserting the modelled 201 and answering the
/// minted id.
async fn create_ota_task(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let reply = call(router, "POST", OTA_TASK_COLLECTION, Some(body)).await;
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201 from CreateOtaTaskConfiguration, got {} with body {:?}",
        reply.status, reply.body
    );
    reply
        .string("TaskConfigurationId")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a TaskConfigurationId, got body {:?}",
                reply.body
            )
        })
        .to_string()
}

/// Assert one answer is the modelled refusal `status`/`code`.
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
}

/// Assert one answer is the 404 an absent configuration is stated by: the modelled status
/// and code, `reason`, and neither optional member of the error shape.
fn assert_absent(case: &str, reply: &Reply, reason: &str) {
    assert_refused(case, reply, 404, NOT_FOUND);
    assert_eq!(
        reply.message(),
        reason,
        "case {case:?}: expected the reason {reason:?}, got {:?}",
        reply.message()
    );
    for member in ["ResourceId", "ResourceType"] {
        assert_eq!(
            reply.member(member),
            None,
            "case {case:?}: expected the 404 to carry no {member}, got {:?} in body {:?}",
            reply.member(member),
            reply.body
        );
    }
}

/// Assert a get reports exactly the target and level the store should hold.
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
        "case {case:?}: expected HTTP 200 from GetEventLogConfiguration, got {} with body {:?}",
        reply.status, reply.body
    );
    let expected_resource_id = resource_id.unwrap_or(UNSCOPED_RESOURCE_ID);
    for (member, expected) in [
        ("Id", id),
        ("ResourceType", resource_type),
        ("ResourceId", expected_resource_id),
        ("EventLogLevel", level),
    ] {
        assert_eq!(
            reply.string(member),
            Some(expected),
            "case {case:?}: expected {member} {expected:?}, got {:?} in body {:?}",
            reply.string(member),
            reply.body
        );
    }
}

/// The log targets the delete table spans: every `ResourceType` the pattern admits as a
/// whole, and resource-specific targets whose `ResourceId` differs in form and length.
fn event_log_targets() -> Vec<(&'static str, Option<String>)> {
    let mut targets: Vec<(&'static str, Option<String>)> = RESOURCE_TYPES
        .iter()
        .map(|resource_type| (*resource_type, None))
        .collect();
    for resource_id in ["thing1", "ThingTWO", "thing+three", "*", "t"] {
        targets.push(("managed-thing", Some(resource_id.to_string())));
    }
    // The `@length` maximum of `SmartHomeResourceId`
    // (`smithy_query(query="shape",shape="SmartHomeResourceId")` -> length 1..200).
    targets.push(("credential-locker", Some("a".repeat(200))));
    targets
}

/// RULE (see the module docs): a delete removes the event log configuration, so every later
/// operation that resolves its `Id` states it absent and the log target it held is free for
/// another create.
#[tokio::test]
async fn deleting_an_event_log_configuration_removes_it_from_the_store() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    let targets = event_log_targets();
    for (row, (resource_type, resource_id)) in targets.iter().enumerate() {
        // Every `LogLevel` the model declares is exercised, cycling over the rows.
        let level = LOG_LEVEL_VALUES[row % LOG_LEVEL_VALUES.len()];
        let case = format!("{resource_type}/{resource_id:?}/{level}");
        let id = create_event_log(
            &router,
            &case,
            resource_type,
            resource_id.as_deref(),
            level,
        )
        .await;

        // It is there before the delete.
        let read = get_event_log(&router, &id).await;
        assert_reports(
            &case,
            &read,
            &id,
            resource_type,
            resource_id.as_deref(),
            level,
        );

        let removed = delete_event_log(&router, &id).await;
        assert_eq!(
            removed.status, DELETE_EVENT_LOG_STATUS,
            "case {case:?}: expected HTTP {DELETE_EVENT_LOG_STATUS} from \
             DeleteEventLogConfiguration, got {} with body {:?}",
            removed.status, removed.body
        );

        // The record is gone from the store itself, not only from what the read reports.
        assert!(
            !engine
                .state::<ServiceState>()
                .lock()
                .event_log_configurations
                .contains_key(&id),
            "case {case:?}: the delete must have removed the record under {id:?}"
        );

        // Every operation that resolves the id now states it absent.
        assert_absent(
            &format!("{case}/get after the delete"),
            &get_event_log(&router, &id).await,
            EVENT_LOG_ABSENT_REASON,
        );
        assert_absent(
            &format!("{case}/second delete"),
            &delete_event_log(&router, &id).await,
            EVENT_LOG_ABSENT_REASON,
        );
        assert_absent(
            &format!("{case}/update after the delete"),
            &update_event_log(&router, &id, "ERROR").await,
            EVENT_LOG_ABSENT_REASON,
        );

        // And the target it configured is free: a create naming it again is accepted
        // rather than refused as a duplicate, under a fresh identifier.
        let recreated = create_event_log(
            &router,
            &format!("{case}/recreated"),
            resource_type,
            resource_id.as_deref(),
            level,
        )
        .await;
        assert_ne!(
            recreated, id,
            "case {case:?}: a create after the delete mints its own identifier"
        );
        // Leave the store as this row found it, so the row that follows starts clean.
        let removed = delete_event_log(&router, &recreated).await;
        assert_eq!(
            removed.status, DELETE_EVENT_LOG_STATUS,
            "case {case:?}: expected the recreated configuration to delete as well, got {} \
             with body {:?}",
            removed.status, removed.body
        );
    }

    assert!(
        engine
            .state::<ServiceState>()
            .lock()
            .event_log_configurations
            .is_empty(),
        "every configuration this test created was deleted, so the store must hold none"
    );
}

/// RULE: a delete removes the OTA task configuration, so every later operation that
/// resolves its `Identifier` states it absent — quoting that identifier.
#[tokio::test]
async fn deleting_an_ota_task_configuration_removes_it_from_the_store() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // The rows differ along which members the create carried, since a delete must not
    // depend on the record's contents.
    let cases: Vec<(&str, Value)> = vec![
        ("every member absent", json!({})),
        ("name only", json!({"Name": "ota config one"})),
        (
            "name, description and a push config",
            json!({
                "Name": "fleet lamps config",
                "Description": "rollout for the lamp fleet",
                "PushConfig": {"TimeoutConfig": {"InProgressTimeoutInMinutes": 60}},
            }),
        ),
        (
            "a client token, so the delete must forget it too",
            json!({"Name": "tokened config", "ClientToken": "token1"}),
        ),
    ];

    for (case, body) in &cases {
        let id = create_ota_task(&router, case, body.clone()).await;
        let path = format!("{OTA_TASK_COLLECTION}/{id}");

        let read = call(&router, "GET", &path, None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 before the delete, got {} with body {:?}",
            read.status, read.body
        );

        let removed = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            removed.status, DELETE_OTA_TASK_STATUS,
            "case {case:?}: expected HTTP {DELETE_OTA_TASK_STATUS} from \
             DeleteOtaTaskConfiguration, got {} with body {:?}",
            removed.status, removed.body
        );
        assert!(
            !engine
                .state::<ServiceState>()
                .lock()
                .ota_task_configurations
                .contains_key(&id),
            "case {case:?}: the delete must have removed the record under {id:?}"
        );

        let reason = ota_task_absent_reason(&id);
        assert_absent(
            &format!("{case}/get after the delete"),
            &call(&router, "GET", &path, None).await,
            &reason,
        );
        assert_absent(
            &format!("{case}/second delete"),
            &call(&router, "DELETE", &path, None).await,
            &reason,
        );

        // A create repeating the row's `ClientToken` must mint a new record rather than
        // replay into the deleted id.
        if body.get("ClientToken").is_some() {
            let after = create_ota_task(&router, &format!("{case}/token replayed"), body.clone())
                .await;
            assert_ne!(
                after, id,
                "case {case:?}: a token whose record was deleted must not replay into it"
            );
            let removed = call(
                &router,
                "DELETE",
                &format!("{OTA_TASK_COLLECTION}/{after}"),
                None,
            )
            .await;
            assert_eq!(
                removed.status, DELETE_OTA_TASK_STATUS,
                "case {case:?}: expected the second record to delete as well, got {} with \
                 body {:?}",
                removed.status, removed.body
            );
        }
    }

    assert!(
        engine
            .state::<ServiceState>()
            .lock()
            .ota_task_configurations
            .is_empty(),
        "every configuration this test created was deleted, so the store must hold none"
    );
}

/// RULE: an update applies the `EventLogLevel` it carries to the stored record, so every
/// later read reports it, and leaves every member the request does not address — the
/// identifier and the log target — as it was. Re-applying a level changes nothing
/// (`@idempotent`).
#[tokio::test]
async fn updating_an_event_log_configuration_applies_the_level() {
    // The rows span every ORDERED PAIR of levels the model declares, each over a fresh
    // store so a row's target cannot collide with another's: the level a configuration was
    // created at, and the one it is moved to — including the pair that asks for the level it
    // already has.
    for (row, from) in LOG_LEVEL_VALUES.iter().enumerate() {
        for (column, to) in LOG_LEVEL_VALUES.iter().enumerate() {
            let router = emulator_over(Arc::new(engine::InMemoryEngine::new()));
            let resource_type = RESOURCE_TYPES[(row + column) % RESOURCE_TYPES.len()];
            // The wildcard resource type addresses the account as a whole, so it carries no
            // resource of its own; every other row names one.
            let resource_id = if resource_type == "*" {
                None
            } else {
                Some(format!("target{row}{column}"))
            };
            let case = format!("{resource_type}/{resource_id:?}/{from}->{to}");
            let id = create_event_log(&router, &case, resource_type, resource_id.as_deref(), from)
                .await;

            let updated = update_event_log(&router, &id, to).await;
            assert_eq!(
                updated.status, UPDATE_EVENT_LOG_STATUS,
                "case {case:?}: expected HTTP {UPDATE_EVENT_LOG_STATUS} from \
                 UpdateEventLogConfiguration, got {} with body {:?}",
                updated.status, updated.body
            );

            // The read reports the level the update asked for, and the target is untouched.
            assert_reports(
                &case,
                &get_event_log(&router, &id).await,
                &id,
                resource_type,
                resource_id.as_deref(),
                to,
            );

            // Repeating the same update is the same record: an idempotent call.
            let again = update_event_log(&router, &id, to).await;
            assert_eq!(
                again.status, UPDATE_EVENT_LOG_STATUS,
                "case {case:?}: a repeated update must be accepted too, got {} with body {:?}",
                again.status, again.body
            );
            assert_reports(
                &format!("{case}/repeated"),
                &get_event_log(&router, &id).await,
                &id,
                resource_type,
                resource_id.as_deref(),
                to,
            );

            // The target still holds exactly ONE configuration: a create naming it again is
            // the duplicate the conflict rule refuses, so the update minted nothing.
            let duplicate = call(
                &router,
                "POST",
                EVENT_LOG_COLLECTION,
                Some(event_log_request(
                    resource_type,
                    resource_id.as_deref(),
                    to,
                )),
            )
            .await;
            assert_refused(
                &format!("{case}/the target still holds one configuration"),
                &duplicate,
                409,
                "ConflictException",
            );
        }
    }
}

/// RULE: a write that resolves no record changes nothing and is stated absent, and a write
/// a modelled constraint refuses leaves the stored record as it was.
#[tokio::test]
async fn a_write_that_resolves_nothing_or_is_refused_changes_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // One configuration to read back after each refused write, and one in another region to
    // address through the wrong endpoint.
    let id = create_event_log(&router, "stored", "managed-thing", Some("thing1"), "INFO").await;
    let elsewhere = call_in(
        &router,
        "eu-central-1",
        "POST",
        EVENT_LOG_COLLECTION,
        Some(event_log_request("*", None, "WARN")),
    )
    .await;
    let elsewhere_id = elsewhere
        .string("Id")
        .expect("the eu-central-1 create answers with an Id")
        .to_string();
    let ota_id = create_ota_task(&router, "stored ota", json!({"Name": "stored config"})).await;
    let ota_elsewhere = call_in(
        &router,
        "eu-central-1",
        "POST",
        OTA_TASK_COLLECTION,
        Some(json!({"Name": "elsewhere config"})),
    )
    .await;
    let ota_elsewhere_id = ota_elsewhere
        .string("TaskConfigurationId")
        .expect("the eu-central-1 create answers with a TaskConfigurationId")
        .to_string();

    // The rows differ along the operation that resolves the id and why this region holds no
    // record under it: nothing ever minted it (in several forms the id patterns admit), or
    // another region's endpoint holds it.
    let unknown_event_log_ids = vec![
        ("never minted, one character", "a".to_string()),
        ("never minted, mixed case", "NoSuchConfiguration1".to_string()),
        ("never minted, digits", "0123456789".to_string()),
        // `LogConfigurationId`: length 1..200.
        ("never minted, at the length maximum", "b".repeat(200)),
        ("another region's configuration", elsewhere_id.clone()),
    ];
    for (why, unknown) in &unknown_event_log_ids {
        assert_absent(
            &format!("DeleteEventLogConfiguration/{why}"),
            &delete_event_log(&router, unknown).await,
            EVENT_LOG_ABSENT_REASON,
        );
        assert_absent(
            &format!("UpdateEventLogConfiguration/{why}"),
            &update_event_log(&router, unknown, "DEBUG").await,
            EVENT_LOG_ABSENT_REASON,
        );
    }

    let unknown_ota_ids = vec![
        ("never minted, one character", "a".to_string()),
        // `OtaTaskConfigurationId`: length 1..64.
        ("never minted, at the length maximum", "b".repeat(64)),
        ("another region's configuration", ota_elsewhere_id.clone()),
    ];
    for (why, unknown) in &unknown_ota_ids {
        assert_absent(
            &format!("DeleteOtaTaskConfiguration/{why}"),
            &call(
                &router,
                "DELETE",
                &format!("{OTA_TASK_COLLECTION}/{unknown}"),
                None,
            )
            .await,
            &ota_task_absent_reason(unknown),
        );
    }

    // A level outside the modelled `LogLevel` value set is the modelled 400.
    for rejected in ["TRACE", "info", "ERROR "] {
        let refused = update_event_log(&router, &id, rejected).await;
        assert_refused(
            &format!("UpdateEventLogConfiguration/{rejected}"),
            &refused,
            400,
            VALIDATION,
        );
    }

    // None of the refusals touched anything: both stored records are still there, with the
    // level and target their creates gave them, and the out-of-region ones survived the
    // writes aimed at them.
    assert_reports(
        "the stored configuration survived",
        &get_event_log(&router, &id).await,
        &id,
        "managed-thing",
        Some("thing1"),
        "INFO",
    );
    let read_elsewhere = call_in(
        &router,
        "eu-central-1",
        "GET",
        &format!("{EVENT_LOG_COLLECTION}/{elsewhere_id}"),
        None,
    )
    .await;
    assert_reports(
        "the eu-central-1 configuration survived",
        &read_elsewhere,
        &elsewhere_id,
        "*",
        None,
        "WARN",
    );
    let read_ota = call(&router, "GET", &format!("{OTA_TASK_COLLECTION}/{ota_id}"), None).await;
    assert_eq!(
        read_ota.status, 200,
        "the stored OTA task configuration must still be readable, got {} with body {:?}",
        read_ota.status, read_ota.body
    );
    let read_ota_elsewhere = call_in(
        &router,
        "eu-central-1",
        "GET",
        &format!("{OTA_TASK_COLLECTION}/{ota_elsewhere_id}"),
        None,
    )
    .await;
    assert_eq!(
        read_ota_elsewhere.status, 200,
        "the eu-central-1 OTA task configuration must still be readable, got {} with body {:?}",
        read_ota_elsewhere.status, read_ota_elsewhere.body
    );

    let store = engine.state::<ServiceState>();
    let store = store.lock();
    assert_eq!(
        store.event_log_configurations.len(),
        2,
        "the refused writes must not have added or removed an event log configuration, got {:?}",
        store.event_log_configurations
    );
    assert_eq!(
        store.ota_task_configurations.len(),
        2,
        "the refused writes must not have added or removed an OTA task configuration, got {:?}",
        store.ota_task_configurations
    );
}
