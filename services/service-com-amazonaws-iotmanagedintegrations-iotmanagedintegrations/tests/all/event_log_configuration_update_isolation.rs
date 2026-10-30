// SPDX-License-Identifier: CC0-1.0

//! `UpdateEventLogConfiguration`, end to end over the wire: what ONE update changes, and what
//! it must leave alone.
//!
//! RULE: an update of a resource this service identifies by the id its path carries applies
//! the member the request addresses to exactly that stored record, so a later read of it
//! reports the new value while its identifier and every member the request does not address
//! stay as they were — every OTHER record keeps what it was created with, including one
//! another region's endpoint holds, and the update mints and removes none. A request a
//! modelled constraint refuses — a `@required` member absent from the body — is the modelled
//! `ValidationException` (HTTP 400) and changes nothing, whether or not the id it names
//! resolves.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.event-log-configuration-lifecycle-and-duplicate-conflict`,
//! which updates the level of the single configuration it created) is ONE row of the table
//! below. The rows differ along what the rule generalises over: which record of a store
//! holding many is addressed, the log target that record configures (every `ResourceType`
//! the pattern admits, with and without a `ResourceId`), the region its endpoint holds it
//! in, the `LogLevel` it was created at, and the distance the update moves that level —
//! including the zero distance of re-applying the level it already has.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::LOG_LEVEL_VALUES;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The collection route the event-log-configuration operations are bound to
/// (`smithy_query(query="operation",operation="CreateEventLogConfiguration,UpdateEventLogConfiguration")`
/// -> POST `/event-log-configurations` and PATCH `/event-log-configurations/{Id}`).
const COLLECTION: &str = "/event-log-configurations";

/// The modelled success status of `UpdateEventLogConfiguration`
/// (`smithy_query(query="operation",operation="UpdateEventLogConfiguration")` -> 200).
const UPDATE_STATUS: u16 = 200;

/// The wire error codes the model's `@httpError` shapes are rendered under.
const VALIDATION: &str = "ValidationException";

/// The two regions whose endpoints this table holds records in. Every resource of this
/// service is regional, so a record is addressable only through the region that created it,
/// and a write in one region is a write no other region's records see.
const REGIONS: &[&str] = &["us-east-1", "eu-central-1"];

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

/// What a read reports on `ResourceId` for a configuration whose target names no resource,
/// written out here rather than imported so the test pins the value a client reads and not
/// the implementation's own constant.
const UNSCOPED_RESOURCE_ID: &str = "unknown";

/// The member path a refusal names for an absent `EventLogLevel`, in lowerCamel as the wire
/// names it, and the requirement the framework words that constraint class with.
const LEVEL_MEMBER_PATH: &str = "eventLogLevel";
const ABSENT_MEMBER_REQUIREMENT: &str = "Member must not be null";

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
        self.string("Message")
            .or_else(|| self.string("message"))
            .unwrap_or_default()
    }
}

/// One stored configuration, as the table expects a read of it to report: the record is
/// keyed by the id its create minted, and `level` is the only thing an update changes.
struct Stored {
    region: &'static str,
    id: String,
    resource_type: &'static str,
    resource_id: Option<String>,
    level: &'static str,
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

/// The create body that configures `level` for the log target `resource_type`/`resource_id`.
fn create_request(resource_type: &str, resource_id: Option<&str>, level: &str) -> Value {
    let mut body = json!({"ResourceType": resource_type, "EventLogLevel": level});
    if let Some(resource_id) = resource_id {
        body["ResourceId"] = json!(resource_id);
    }
    body
}

/// `CreateEventLogConfiguration` in `region`, asserting the modelled 201 and answering the
/// minted id.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    resource_type: &str,
    resource_id: Option<&str>,
    level: &str,
) -> String {
    let reply = call_in(
        router,
        region,
        "POST",
        COLLECTION,
        Some(create_request(resource_type, resource_id, level)),
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

/// `UpdateEventLogConfiguration` in `region` for `id`, with `body`.
async fn update_in(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    body: Value,
) -> Reply {
    call_in(
        router,
        region,
        "PATCH",
        &format!("{COLLECTION}/{id}"),
        Some(body),
    )
    .await
}

/// `GetEventLogConfiguration` in `region` for `id`.
async fn get_in(router: &MultiServiceRouter, region: &str, id: &str) -> Reply {
    call_in(router, region, "GET", &format!("{COLLECTION}/{id}"), None).await
}

/// Assert a read of `record` reports exactly the target and level the store should hold.
async fn assert_reports(router: &MultiServiceRouter, case: &str, record: &Stored) {
    let reply = get_in(router, record.region, &record.id).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200 from GetEventLogConfiguration, got {} with body {:?}",
        reply.status, reply.body
    );
    // A target that names no resource is reported through the placeholder.
    let expected_resource_id = record.resource_id.as_deref().unwrap_or(UNSCOPED_RESOURCE_ID);
    for (member, expected) in [
        ("Id", record.id.as_str()),
        ("ResourceType", record.resource_type),
        ("ResourceId", expected_resource_id),
        ("EventLogLevel", record.level),
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

/// Every `LogConfigurationId` the store holds, so a case can state that a write minted and
/// removed none.
fn stored_ids(engine: &Arc<engine::InMemoryEngine>) -> BTreeSet<String> {
    engine
        .state::<ServiceState>()
        .lock()
        .event_log_configurations
        .keys()
        .cloned()
        .collect()
}

/// The table's records: every resource type as a whole and a resource-specific target inside
/// each type the wildcard does not cover, in each region, created at a level that cycles over
/// the model's whole value set.
fn table() -> Vec<(&'static str, &'static str, Option<String>)> {
    let mut rows: Vec<(&'static str, &'static str, Option<String>)> = Vec::new();
    for region in REGIONS {
        for resource_type in RESOURCE_TYPES {
            rows.push((region, resource_type, None));
            // The account wildcard addresses the account as a whole, so it carries no
            // resource of its own; every other type also gets a resource-specific target,
            // whose ids differ in form and length within the `SmartHomeResourceId`
            // `@pattern` (`^[a-zA-Z0-9+*]*$`).
            if *resource_type != "*" {
                let resource_id = match *resource_type {
                    "managed-thing" => "thing1".to_string(),
                    "credential-locker" => "LockerTWO".to_string(),
                    "provisioning-profile" => "profile+three".to_string(),
                    "ota-task" => "t".to_string(),
                    _ => "*".to_string(),
                };
                rows.push((region, resource_type, Some(resource_id)));
            }
        }
    }
    rows
}

/// RULE (see the module docs): one update applies the level it carries to exactly the record
/// its path names, leaving that record's identifier and target and every other record in the
/// store — in this region and in another — as they were, and minting nothing.
#[tokio::test]
async fn an_update_changes_only_the_configuration_its_path_names() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    let rows = table();
    let mut records: Vec<Stored> = Vec::new();
    for (row, (region, resource_type, resource_id)) in rows.iter().enumerate() {
        let level = LOG_LEVEL_VALUES[row % LOG_LEVEL_VALUES.len()];
        let case = format!("{region}/{resource_type}/{resource_id:?}/{level}");
        let id = create_in(
            &router,
            region,
            &case,
            resource_type,
            resource_id.as_deref(),
            level,
        )
        .await;
        records.push(Stored {
            region,
            id,
            resource_type,
            resource_id: resource_id.clone(),
            level,
        });
    }

    let ids_before = stored_ids(&engine);
    assert_eq!(
        ids_before.len(),
        records.len(),
        "the store must hold one record per table row, got {} for {} rows",
        ids_before.len(),
        records.len()
    );

    for row in 0..records.len() {
        // The row's BLOCK index is how far the update moves the level, so the table spans
        // re-applying the level a record already has (block 0) and moving it by each
        // distance the value set allows.
        let shift = row / LOG_LEVEL_VALUES.len();
        let to = LOG_LEVEL_VALUES[(row + shift) % LOG_LEVEL_VALUES.len()];
        let case = format!(
            "{}/{}/{:?}/{}->{to}",
            records[row].region,
            records[row].resource_type,
            records[row].resource_id,
            records[row].level
        );

        let updated = update_in(
            &router,
            records[row].region,
            &records[row].id,
            json!({"EventLogLevel": to}),
        )
        .await;
        assert_eq!(
            updated.status, UPDATE_STATUS,
            "case {case:?}: expected HTTP {UPDATE_STATUS} from UpdateEventLogConfiguration, \
             got {} with body {:?}",
            updated.status, updated.body
        );
        // The operation's modelled output declares no member, so its answer carries none.
        for member in ["Id", "ResourceType", "ResourceId", "EventLogLevel"] {
            assert_eq!(
                updated.member(member),
                None,
                "case {case:?}: expected the update to answer no {member}, got {:?} in body {:?}",
                updated.member(member),
                updated.body
            );
        }

        records[row].level = to;

        // Every record now reads back as the table expects: the updated one at its new
        // level, under the same identifier and target, and every other one — including the
        // ones another region's endpoint holds — exactly as it was created.
        for (other, record) in records.iter().enumerate() {
            let label = if other == row {
                format!("{case}/the updated record")
            } else {
                format!("{case}/record {other} must be untouched")
            };
            assert_reports(&router, &label, record).await;
        }

        // And the write minted and removed nothing: the store holds the same identifiers.
        assert_eq!(
            stored_ids(&engine),
            ids_before,
            "case {case:?}: an update neither mints nor removes a record"
        );
    }
}

/// RULE: a request a modelled constraint refuses changes nothing — an absent `@required`
/// member is the modelled 400 naming that member and the requirement, whether or not the id
/// the path names resolves, and the stored level is the one its last accepted write left.
#[tokio::test]
async fn an_update_missing_the_required_level_is_refused_and_changes_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));
    let region = REGIONS[0];

    let mut record = Stored {
        region,
        id: create_in(
            &router,
            region,
            "stored",
            "managed-thing",
            Some("thing1"),
            "INFO",
        )
        .await,
        resource_type: "managed-thing",
        resource_id: Some("thing1".to_string()),
        level: "INFO",
    };

    // One accepted write first, so the level the refusals must leave alone is one an update
    // put there rather than the one the create did.
    let accepted = update_in(&router, region, &record.id, json!({"EventLogLevel": "ERROR"})).await;
    assert_eq!(
        accepted.status, UPDATE_STATUS,
        "the accepted update must answer HTTP {UPDATE_STATUS}, got {} with body {:?}",
        accepted.status, accepted.body
    );
    record.level = "ERROR";
    assert_reports(&router, "the accepted update applied", &record).await;

    let ids_before = stored_ids(&engine);

    // The rows differ along the body the required member is missing from, and along whether
    // the path names a record this region holds: a constraint is a property of the input
    // shape, so it is refused before any record is resolved.
    let cases: Vec<(String, String, Value)> = vec![
        (
            "an empty body".to_string(),
            record.id.clone(),
            json!({}),
        ),
        (
            "a body carrying only members the operation does not model".to_string(),
            record.id.clone(),
            json!({"ResourceType": "managed-thing", "ResourceId": "thing1"}),
        ),
        (
            "an empty body on an id nothing minted".to_string(),
            "NoSuchConfiguration1".to_string(),
            json!({}),
        ),
    ];

    for (case, id, body) in cases {
        let refused = update_in(&router, region, &id, body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some(VALIDATION),
            "case {case:?}: expected the modelled {VALIDATION} code, got {:?}",
            refused.error_type
        );
        let message = refused.message();
        assert!(
            message.contains(LEVEL_MEMBER_PATH) && message.contains(ABSENT_MEMBER_REQUIREMENT),
            "case {case:?}: expected the reason to name {LEVEL_MEMBER_PATH:?} and restate \
             {ABSENT_MEMBER_REQUIREMENT:?}, got {message:?}"
        );

        // Nothing moved: the stored record still reports the level its last accepted write
        // left, and the store holds the same identifiers.
        assert_reports(&router, &format!("{case}/the record survived"), &record).await;
        assert_eq!(
            stored_ids(&engine),
            ids_before,
            "case {case:?}: a refused update neither mints nor removes a record"
        );
    }
}
