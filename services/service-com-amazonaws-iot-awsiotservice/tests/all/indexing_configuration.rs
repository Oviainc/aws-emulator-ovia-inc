// SPDX-License-Identifier: CC0-1.0

//! The account's FLEET INDEXING configuration, end to end over HTTP.
//!
//! Every test drives real requests through the `MultiServiceRouter` this crate's
//! service is mounted in, so routing, the restJson1 `GET /indexing/config` and
//! `POST /indexing/config` bindings, the handlers and the response serialization are
//! all exercised.
//!
//! The rules under test, none of them specific to one configuration:
//!
//! * `GetIndexingConfiguration` reports the configuration this service HOLDS: what
//!   the last `UpdateIndexingConfiguration` persisted, or — for an account that has
//!   never updated it — the disabled configuration, every mode at the documented
//!   `OFF` value;
//! * the read is a read: repeating it, with no update between, reports the same
//!   configuration and leaves the store alone;
//! * a configuration section the update SUPPLIES replaces the stored one in full (an
//!   optional mode the request omits goes back to `OFF`), and one it OMITS is left
//!   exactly as it was;
//! * `managedFields` a caller supplies are not recorded — "You can't modify managed
//!   fields by updating fleet indexing configuration"
//!   (`smithy_query(query="doc",shape="ThingIndexingConfiguration",member="managedFields")`);
//! * a value outside the enum the model declares for a member, and the one documented
//!   cross-member rule ("To enable thing connectivity indexing, *thingIndexMode* must
//!   not be set to OFF"), are refused with the modelled `InvalidRequestException`
//!   (400) and change nothing.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::ops::types::{
    Field, ThingGroupIndexingConfiguration, ThingIndexingConfiguration,
    DEVICE_DEFENDER_INDEXING_MODE_VALUES, FIELD_TYPE_VALUES, FLEET_INDEXING_API_VALUES,
    NAMED_SHADOW_INDEXING_MODE_VALUES, TARGET_FIELD_ORDER_VALUES,
    THING_CONNECTIVITY_INDEXING_MODE_VALUES, THING_GROUP_INDEXING_MODE_VALUES,
    THING_INDEXING_MODE_VALUES,
};
use service_com_amazonaws_iot_awsiotservice::state::{
    ComAmazonawsIotAwsiotserviceState, IndexingConfiguration, INDEXING_MODE_OFF, MAX_CUSTOM_FIELDS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. The configuration is
/// account-wide, so no assertion depends on it.
const REGION: &str = "eu-west-2";

/// The optional mode members of `thingIndexingConfiguration`, each with the catalog
/// the model declares for it. Read with
/// `smithy_query(query="closure",operation="UpdateIndexingConfiguration")`.
const OPTIONAL_THING_MODES: &[(&str, &[&str])] = &[
    (
        "thingConnectivityIndexingMode",
        THING_CONNECTIVITY_INDEXING_MODE_VALUES,
    ),
    (
        "deviceDefenderIndexingMode",
        DEVICE_DEFENDER_INDEXING_MODE_VALUES,
    ),
    (
        "namedShadowIndexingMode",
        NAMED_SHADOW_INDEXING_MODE_VALUES,
    ),
];

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// This service's store, as the handlers reach it.
    fn state(&self) -> engine::ServiceState<ComAmazonawsIotAwsiotserviceState> {
        self.engine.state::<ComAmazonawsIotAwsiotserviceState>()
    }

    /// Overwrite the stored configuration, so a read can be tested against a record
    /// no request in the test wrote.
    fn seed(&self, configuration: IndexingConfiguration) {
        self.state().lock().indexing_configuration = configuration;
    }

    async fn dispatch(&self, method: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}/indexing/config"))
            .header("host", host)
            .header("content-type", "application/json")
            // The credential scope names the `iot` signing name the model's
            // `aws.auth#sigv4` trait declares, which is what routes the request.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the indexing-configuration request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    async fn get_indexing_configuration(&self) -> Reply {
        self.dispatch("GET", None).await
    }

    async fn update_indexing_configuration(&self, body: &Value) -> Reply {
        self.dispatch("POST", Some(body)).await
    }

    /// The configuration a successful read reports, as JSON.
    async fn reported(&self, case: &str) -> Value {
        let reply = self.get_indexing_configuration().await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected GetIndexingConfiguration to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
        reply.json()
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    /// The error code a restJson1 error response carries.
    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }
}

/// The disabled configuration, as a read reports it: every mode `OFF`, nothing else.
fn disabled_report() -> Value {
    json!({
        "thingIndexingConfiguration": {
            "thingIndexingMode": INDEXING_MODE_OFF,
            "thingConnectivityIndexingMode": INDEXING_MODE_OFF,
            "deviceDefenderIndexingMode": INDEXING_MODE_OFF,
            "namedShadowIndexingMode": INDEXING_MODE_OFF,
        },
        "thingGroupIndexingConfiguration": {
            "thingGroupIndexingMode": INDEXING_MODE_OFF,
        },
    })
}

/// A `thingIndexingConfiguration` body carrying `thingIndexingMode` and one more
/// member, built member by member because the member NAME is what varies.
fn thing_section_with(thing_mode: &str, member: &str, value: &str) -> Value {
    let mut section = Map::new();
    section.insert(
        "thingIndexingMode".to_string(),
        Value::String(thing_mode.to_string()),
    );
    section.insert(member.to_string(), Value::String(value.to_string()));
    let mut request = Map::new();
    request.insert(
        "thingIndexingConfiguration".to_string(),
        Value::Object(section),
    );
    Value::Object(request)
}

/// One `Field`, in the JSON form the model binds it to.
fn field(name: &str, field_type: &str) -> Value {
    json!({ "name": name, "type": field_type })
}

/// A custom field of EVERY type `FieldType` declares — the model's own catalog, so
/// the row covers types the issue never cited.
fn fields_of_every_modelled_type() -> Value {
    Value::Array(
        FIELD_TYPE_VALUES
            .iter()
            .enumerate()
            .map(|(index, field_type)| field(&format!("attributes.custom{index}"), field_type))
            .collect(),
    )
}

/// The thing section as the service records it: the supplied members, each optional
/// mode the request left out filled in with `OFF`, and `managedFields` dropped.
fn expected_thing(supplied: &Value) -> Value {
    let mut expected = Map::new();
    expected.insert(
        "thingIndexingMode".to_string(),
        supplied["thingIndexingMode"].clone(),
    );
    for (member, _) in OPTIONAL_THING_MODES {
        let value = supplied
            .get(*member)
            .cloned()
            .unwrap_or_else(|| json!(INDEXING_MODE_OFF));
        expected.insert((*member).to_string(), value);
    }
    for member in ["customFields", "filter"] {
        if let Some(value) = supplied.get(member) {
            expected.insert(member.to_string(), value.clone());
        }
    }
    Value::Object(expected)
}

/// The thing group section as the service records it, under the same rules.
fn expected_thing_group(supplied: &Value) -> Value {
    let mut expected = Map::new();
    expected.insert(
        "thingGroupIndexingMode".to_string(),
        supplied["thingGroupIndexingMode"].clone(),
    );
    if let Some(value) = supplied.get("customFields") {
        expected.insert("customFields".to_string(), value.clone());
    }
    Value::Object(expected)
}

/// What a read must report after `request` was persisted, given what it supplied.
fn expected_report(request: &Value) -> Value {
    let mut expected = Map::new();
    if let Some(section) = request.get("thingIndexingConfiguration") {
        expected.insert(
            "thingIndexingConfiguration".to_string(),
            expected_thing(section),
        );
    }
    if let Some(section) = request.get("thingGroupIndexingConfiguration") {
        expected.insert(
            "thingGroupIndexingConfiguration".to_string(),
            expected_thing_group(section),
        );
    }
    Value::Object(expected)
}

/// One update that must round-trip through the reader.
struct RoundTripCase {
    name: &'static str,
    request: fn() -> Value,
}

/// Configurations spanning what the rule generalises over: every thing indexing mode
/// the model declares, every optional mode's values, the number and order of custom
/// fields, and each part of the `IndexingFilter`.
const ROUND_TRIP_CASES: &[RoundTripCase] = &[
    RoundTripCase {
        name: "registry indexing, nothing else",
        request: || {
            json!({ "thingIndexingConfiguration": { "thingIndexingMode": "REGISTRY" } })
        },
    },
    RoundTripCase {
        name: "registry and shadow indexing with every optional mode on",
        request: || {
            json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": "REGISTRY_AND_SHADOW",
                    "thingConnectivityIndexingMode": "STATUS",
                    "deviceDefenderIndexingMode": "VIOLATIONS",
                    "namedShadowIndexingMode": "ON",
                },
            })
        },
    },
    RoundTripCase {
        name: "one custom field",
        request: || {
            json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": "REGISTRY",
                    "customFields": [field("attributes.serial", "String")],
                },
            })
        },
    },
    RoundTripCase {
        name: "a custom field of every modelled type",
        request: || {
            json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": "REGISTRY",
                    "customFields": fields_of_every_modelled_type(),
                },
            })
        },
    },
    RoundTripCase {
        name: "two custom fields in descending name order",
        request: || {
            json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": "REGISTRY",
                    "customFields": [
                        field("attributes.zone", "Number"),
                        field("attributes.alpha", "Boolean"),
                    ],
                },
            })
        },
    },
    RoundTripCase {
        name: "named shadow and geolocation filters",
        request: || {
            json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": "REGISTRY_AND_SHADOW",
                    "namedShadowIndexingMode": "ON",
                    "filter": {
                        "namedShadowNames": ["$package", "telemetry-1"],
                        "geoLocations": [
                            { "name": "shadow.name.coords.desired.position", "order": "LatLon" },
                            { "name": "attributes.where", "order": "LonLat" },
                        ],
                    },
                },
            })
        },
    },
    RoundTripCase {
        name: "a connectivity filter naming every modelled fleet indexing api",
        request: || {
            json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": "REGISTRY",
                    "thingConnectivityIndexingMode": "STATUS",
                    "filter": {
                        "connectivity": { "includeSocketInformation": FLEET_INDEXING_API_VALUES },
                    },
                },
            })
        },
    },
    RoundTripCase {
        name: "thing group indexing on, with custom fields",
        request: || {
            json!({
                "thingGroupIndexingConfiguration": {
                    "thingGroupIndexingMode": "ON",
                    "customFields": [field("attributes.owner", "String")],
                },
            })
        },
    },
    RoundTripCase {
        name: "both sections at once",
        request: || {
            json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": "REGISTRY_AND_SHADOW",
                    "deviceDefenderIndexingMode": "VIOLATIONS",
                    "customFields": [field("shadow.desired.temperature", "Number")],
                },
                "thingGroupIndexingConfiguration": { "thingGroupIndexingMode": "ON" },
            })
        },
    },
    RoundTripCase {
        name: "indexing turned back off in both sections",
        request: || {
            json!({
                "thingIndexingConfiguration": { "thingIndexingMode": INDEXING_MODE_OFF },
                "thingGroupIndexingConfiguration": { "thingGroupIndexingMode": INDEXING_MODE_OFF },
            })
        },
    },
];

/// An account that has never updated its configuration has fleet indexing disabled,
/// and reading it is a read: repeating the call reports the same thing and the store
/// is untouched.
#[tokio::test]
async fn fleet_indexing_is_disabled_until_an_update_and_reading_it_changes_nothing() {
    let emulator = Emulator::new();

    for round in 0..3 {
        let reported = emulator.reported(&format!("round {round}")).await;
        assert_eq!(
            reported,
            disabled_report(),
            "round {round}: expected the disabled configuration, got {reported}"
        );
        let stored = emulator.state().lock().indexing_configuration.clone();
        assert_eq!(
            stored.thing.thing_indexing_mode, INDEXING_MODE_OFF,
            "round {round}: a read changed the stored thingIndexingMode to {:?}",
            stored.thing.thing_indexing_mode
        );
        assert_eq!(
            stored.thing_group.thing_group_indexing_mode, INDEXING_MODE_OFF,
            "round {round}: a read changed the stored thingGroupIndexingMode to {:?}",
            stored.thing_group.thing_group_indexing_mode
        );
    }
}

/// Every configuration a caller writes is reported back by the reader, member for
/// member, with the omitted optional modes resolved to `OFF`.
#[tokio::test]
async fn every_configuration_a_caller_writes_is_reported_back() {
    for case in ROUND_TRIP_CASES {
        // A fresh emulator per row, so each row is read against its own write and no
        // row can pass on another row's state.
        let emulator = Emulator::new();
        let request = (case.request)();

        let update = emulator.update_indexing_configuration(&request).await;
        assert_eq!(
            update.status,
            200,
            "case {}: expected UpdateIndexingConfiguration to answer 200, got {} body {}",
            case.name,
            update.status,
            update.text()
        );

        let reported = emulator.reported(case.name).await;
        let expected = expected_report(&request);
        for (section, value) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                &reported[section], value,
                "case {}: expected {section} {value}, got {}",
                case.name, reported[section]
            );
        }
        // Reading twice reports the same configuration.
        let again = emulator.reported(case.name).await;
        assert_eq!(
            again, reported,
            "case {}: a second read reported {again} instead of {reported}",
            case.name
        );
    }
}

/// Every pair of modes the model declares round-trips: the reader reports the value
/// the writer stored for each optional mode, not a fixed one.
#[tokio::test]
async fn each_optional_mode_reports_the_value_that_was_written() {
    // `OFF` is not a valid thing index for connectivity STATUS (the documented
    // cross-member rule, pinned by its own test), so the enabling mode here is one
    // the rule allows.
    for thing_mode in THING_INDEXING_MODE_VALUES
        .iter()
        .filter(|mode| **mode != INDEXING_MODE_OFF)
    {
        for (member, catalog) in OPTIONAL_THING_MODES {
            for value in *catalog {
                let emulator = Emulator::new();
                let request = thing_section_with(thing_mode, member, value);
                let case = format!("thingIndexingMode {thing_mode} with {member} {value}");
                let update = emulator.update_indexing_configuration(&request).await;
                assert_eq!(
                    update.status,
                    200,
                    "case {case}: expected 200, got {} body {}",
                    update.status,
                    update.text()
                );
                let reported = emulator.reported(&case).await;
                assert_eq!(
                    reported["thingIndexingConfiguration"][*member],
                    json!(value),
                    "case {case}: expected {member} {value}, got {}",
                    reported["thingIndexingConfiguration"][*member]
                );
            }
        }
    }

    for group_mode in THING_GROUP_INDEXING_MODE_VALUES {
        let emulator = Emulator::new();
        let request =
            json!({ "thingGroupIndexingConfiguration": { "thingGroupIndexingMode": group_mode } });
        let case = format!("thingGroupIndexingMode {group_mode}");
        let update = emulator.update_indexing_configuration(&request).await;
        assert_eq!(
            update.status,
            200,
            "case {case}: expected 200, got {} body {}",
            update.status,
            update.text()
        );
        let reported = emulator.reported(&case).await;
        assert_eq!(
            reported["thingGroupIndexingConfiguration"]["thingGroupIndexingMode"],
            json!(group_mode),
            "case {case}: expected thingGroupIndexingMode {group_mode}, got {}",
            reported["thingGroupIndexingConfiguration"]["thingGroupIndexingMode"]
        );
    }
}

/// A section the request supplies replaces the stored one in FULL; a section it omits
/// is left exactly as it was.
#[tokio::test]
async fn a_supplied_section_is_replaced_in_full_and_an_omitted_one_is_untouched() {
    let emulator = Emulator::new();

    let full = json!({
        "thingIndexingConfiguration": {
            "thingIndexingMode": "REGISTRY_AND_SHADOW",
            "thingConnectivityIndexingMode": "STATUS",
            "deviceDefenderIndexingMode": "VIOLATIONS",
            "namedShadowIndexingMode": "ON",
            "customFields": [field("attributes.serial", "String")],
            "filter": { "namedShadowNames": ["telemetry-1"] },
        },
        "thingGroupIndexingConfiguration": {
            "thingGroupIndexingMode": "ON",
            "customFields": [field("attributes.owner", "String")],
        },
    });
    let reply = emulator.update_indexing_configuration(&full).await;
    assert_eq!(
        reply.status,
        200,
        "case the full configuration: expected 200, got {} body {}",
        reply.status,
        reply.text()
    );
    let after_full = emulator.reported("the full configuration").await;
    assert_eq!(
        after_full,
        expected_report(&full),
        "case the full configuration: got {after_full}"
    );

    // Only the thing GROUP section is supplied: the thing section is untouched.
    let group_only = json!({
        "thingGroupIndexingConfiguration": { "thingGroupIndexingMode": INDEXING_MODE_OFF },
    });
    let reply = emulator.update_indexing_configuration(&group_only).await;
    assert_eq!(
        reply.status,
        200,
        "case the thing group section alone: expected 200, got {} body {}",
        reply.status,
        reply.text()
    );
    let after_group = emulator.reported("the thing group section alone").await;
    assert_eq!(
        after_group["thingIndexingConfiguration"], after_full["thingIndexingConfiguration"],
        "case the thing group section alone: the omitted thing section changed to {}",
        after_group["thingIndexingConfiguration"]
    );
    assert_eq!(
        after_group["thingGroupIndexingConfiguration"],
        expected_thing_group(&group_only["thingGroupIndexingConfiguration"]),
        "case the thing group section alone: got {}",
        after_group["thingGroupIndexingConfiguration"]
    );

    // The thing section again, with only its required member: the whole section is
    // replaced, so the optional modes go back to OFF and the fields and filter go.
    let bare = json!({ "thingIndexingConfiguration": { "thingIndexingMode": "REGISTRY" } });
    let reply = emulator.update_indexing_configuration(&bare).await;
    assert_eq!(
        reply.status,
        200,
        "case the bare thing section: expected 200, got {} body {}",
        reply.status,
        reply.text()
    );
    let after_bare = emulator.reported("the bare thing section").await;
    assert_eq!(
        after_bare["thingIndexingConfiguration"],
        expected_thing(&bare["thingIndexingConfiguration"]),
        "case the bare thing section: got {}",
        after_bare["thingIndexingConfiguration"]
    );
    assert_eq!(
        after_bare["thingGroupIndexingConfiguration"],
        after_group["thingGroupIndexingConfiguration"],
        "case the bare thing section: the omitted thing group section changed to {}",
        after_bare["thingGroupIndexingConfiguration"]
    );
}

/// A request that supplies neither section changes nothing.
#[tokio::test]
async fn an_update_that_supplies_no_section_changes_nothing() {
    let emulator = Emulator::new();
    let stored = json!({
        "thingIndexingConfiguration": {
            "thingIndexingMode": "REGISTRY",
            "namedShadowIndexingMode": "ON",
        },
    });
    let reply = emulator.update_indexing_configuration(&stored).await;
    assert_eq!(
        reply.status,
        200,
        "the seeding update must succeed, got {} body {}",
        reply.status,
        reply.text()
    );
    let before = emulator.reported("before the empty update").await;

    let reply = emulator.update_indexing_configuration(&json!({})).await;
    assert_eq!(
        reply.status,
        200,
        "case an update with no sections: expected 200, got {} body {}",
        reply.status,
        reply.text()
    );
    let after = emulator.reported("after the empty update").await;
    assert_eq!(
        after, before,
        "case an update with no sections: the configuration changed to {after}"
    );
}

/// `managedFields` are the service's, not the caller's: a request that supplies them
/// records none.
#[tokio::test]
async fn managed_fields_a_caller_supplies_are_not_recorded() {
    let cases: &[(&str, Value)] = &[
        (
            "managed fields on the thing section",
            json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": "REGISTRY",
                    "managedFields": [field("thingName", "String"), field("registry.version", "Number")],
                },
            }),
        ),
        (
            "managed fields on the thing group section",
            json!({
                "thingGroupIndexingConfiguration": {
                    "thingGroupIndexingMode": "ON",
                    "managedFields": [field("thingGroupName", "String")],
                },
            }),
        ),
    ];

    for (name, request) in cases {
        let emulator = Emulator::new();
        let reply = emulator.update_indexing_configuration(request).await;
        assert_eq!(
            reply.status,
            200,
            "case {name}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
        let reported = emulator.reported(name).await;
        for section in [
            "thingIndexingConfiguration",
            "thingGroupIndexingConfiguration",
        ] {
            assert!(
                reported[section].get("managedFields").is_none(),
                "case {name}: {section} reports managedFields {}",
                reported[section]
            );
        }
    }
}

/// The reader reports what STATE holds, not a value compiled into it: a record seeded
/// straight into the store is what the next read answers with.
#[tokio::test]
async fn the_reader_reports_what_the_store_holds() {
    // Two seeded records, so no single hardcoded response can satisfy both rows.
    let seeds: Vec<(&str, IndexingConfiguration)> = vec![
        (
            "registry indexing with one custom field",
            IndexingConfiguration {
                thing: ThingIndexingConfiguration {
                    thing_indexing_mode: "REGISTRY".to_string(),
                    thing_connectivity_indexing_mode: Some("STATUS".to_string()),
                    device_defender_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
                    named_shadow_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
                    managed_fields: None,
                    custom_fields: Some(vec![Field {
                        name: Some("attributes.seeded".to_string()),
                        r#type: Some("String".to_string()),
                    }]),
                    filter: None,
                },
                thing_group: ThingGroupIndexingConfiguration {
                    thing_group_indexing_mode: "ON".to_string(),
                    managed_fields: None,
                    custom_fields: None,
                },
            },
        ),
        (
            "registry and shadow indexing, groups off",
            IndexingConfiguration {
                thing: ThingIndexingConfiguration {
                    thing_indexing_mode: "REGISTRY_AND_SHADOW".to_string(),
                    thing_connectivity_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
                    device_defender_indexing_mode: Some("VIOLATIONS".to_string()),
                    named_shadow_indexing_mode: Some("ON".to_string()),
                    managed_fields: None,
                    custom_fields: None,
                    filter: None,
                },
                thing_group: ThingGroupIndexingConfiguration {
                    thing_group_indexing_mode: INDEXING_MODE_OFF.to_string(),
                    managed_fields: None,
                    custom_fields: None,
                },
            },
        ),
    ];

    for (name, seed) in seeds {
        let emulator = Emulator::new();
        let expected_thing_mode = seed.thing.thing_indexing_mode.clone();
        let expected_defender = seed.thing.device_defender_indexing_mode.clone();
        let expected_group_mode = seed.thing_group.thing_group_indexing_mode.clone();
        let expected_custom = seed.thing.custom_fields.as_ref().map(|fields| {
            Value::Array(
                fields
                    .iter()
                    .map(|one| {
                        field(
                            one.name.as_deref().unwrap_or_default(),
                            one.r#type.as_deref().unwrap_or_default(),
                        )
                    })
                    .collect(),
            )
        });
        emulator.seed(seed);

        let reported = emulator.reported(name).await;
        assert_eq!(
            reported["thingIndexingConfiguration"]["thingIndexingMode"],
            json!(expected_thing_mode),
            "case {name}: expected thingIndexingMode {expected_thing_mode}, got {}",
            reported["thingIndexingConfiguration"]["thingIndexingMode"]
        );
        assert_eq!(
            reported["thingIndexingConfiguration"]["deviceDefenderIndexingMode"],
            json!(expected_defender),
            "case {name}: expected deviceDefenderIndexingMode {expected_defender:?}, got {}",
            reported["thingIndexingConfiguration"]["deviceDefenderIndexingMode"]
        );
        assert_eq!(
            reported["thingGroupIndexingConfiguration"]["thingGroupIndexingMode"],
            json!(expected_group_mode),
            "case {name}: expected thingGroupIndexingMode {expected_group_mode}, got {}",
            reported["thingGroupIndexingConfiguration"]["thingGroupIndexingMode"]
        );
        match expected_custom {
            Some(expected) => assert_eq!(
                reported["thingIndexingConfiguration"]["customFields"], expected,
                "case {name}: expected customFields {expected}, got {}",
                reported["thingIndexingConfiguration"]["customFields"]
            ),
            None => assert!(
                reported["thingIndexingConfiguration"]
                    .get("customFields")
                    .is_none(),
                "case {name}: expected no customFields, got {}",
                reported["thingIndexingConfiguration"]
            ),
        }
    }
}

/// A value outside the enum the model declares for its member is refused with HTTP
/// 400 and changes nothing. The refusal comes from the boundary validator — an enum's
/// value set is a MODELLED constraint, so no request carrying a value outside one
/// ever reaches the handler — which is why the status, and not the error code, is
/// what this asserts.
#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused_and_changes_nothing() {
    let mut cases: Vec<(String, Value)> = Vec::new();

    // Every mode member, with a value its own catalog does not list: the lowercase
    // form, a value borrowed from ANOTHER mode's enum, and the empty string.
    for bad in ["registry", "ON", ""] {
        cases.push((
            format!("thingIndexingMode {bad:?}"),
            json!({ "thingIndexingConfiguration": { "thingIndexingMode": bad } }),
        ));
    }
    for (member, _) in OPTIONAL_THING_MODES {
        for bad in ["on", "REGISTRY", "OFF "] {
            cases.push((
                format!("{member} {bad:?}"),
                thing_section_with("REGISTRY", member, bad),
            ));
        }
    }
    for bad in ["on", "STATUS", "Off"] {
        cases.push((
            format!("thingGroupIndexingMode {bad:?}"),
            json!({ "thingGroupIndexingConfiguration": { "thingGroupIndexingMode": bad } }),
        ));
    }
    // The enums reached through a list or a nested structure.
    for bad in ["string", "Int"] {
        cases.push((
            format!("a custom field of type {bad:?}"),
            json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": "REGISTRY",
                    "customFields": [field("attributes.serial", bad)],
                },
            }),
        ));
        cases.push((
            format!("a thing group custom field of type {bad:?}"),
            json!({
                "thingGroupIndexingConfiguration": {
                    "thingGroupIndexingMode": "ON",
                    "customFields": [field("attributes.owner", bad)],
                },
            }),
        ));
    }
    cases.push((
        "a geolocation target ordered \"LatLong\"".to_string(),
        json!({
            "thingIndexingConfiguration": {
                "thingIndexingMode": "REGISTRY_AND_SHADOW",
                "filter": { "geoLocations": [{ "name": "attributes.where", "order": "LatLong" }] },
            },
        }),
    ));
    cases.push((
        "a connectivity filter naming an unmodelled api".to_string(),
        json!({
            "thingIndexingConfiguration": {
                "thingIndexingMode": "REGISTRY",
                "filter": {
                    "connectivity": { "includeSocketInformation": ["GET_THING_CONNECTIVITY"] },
                },
            },
        }),
    ));
    // Each refusal is checked against a store that already holds a configuration, so
    // "changed nothing" is a real claim and not a claim about the default.
    let seeded = json!({
        "thingIndexingConfiguration": {
            "thingIndexingMode": "REGISTRY",
            "namedShadowIndexingMode": "ON",
        },
        "thingGroupIndexingConfiguration": { "thingGroupIndexingMode": "ON" },
    });

    for (name, request) in &cases {
        let emulator = Emulator::new();
        let reply = emulator.update_indexing_configuration(&seeded).await;
        assert_eq!(
            reply.status,
            200,
            "case {name}: the seeding update must succeed, got {} body {}",
            reply.status,
            reply.text()
        );
        let before = emulator.reported(name).await;

        let reply = emulator.update_indexing_configuration(request).await;
        assert_eq!(
            reply.status,
            400,
            "case {name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        let after = emulator.reported(name).await;
        assert_eq!(
            after, before,
            "case {name}: a refused update changed the configuration to {after}"
        );
    }
}

/// A custom-field list one field past the emulator's own ceiling is refused with the
/// operation's modelled `InvalidRequestException` (400) — the handler's own refusal,
/// since `Fields` carries no modelled length — and changes nothing.
#[tokio::test]
async fn a_custom_field_list_past_the_bound_is_refused_and_changes_nothing() {
    let over_the_bound: Vec<Value> = (0..=MAX_CUSTOM_FIELDS)
        .map(|index| field(&format!("attributes.f{index}"), "String"))
        .collect();
    let cases: &[(String, Value)] = &[
        (
            format!("{} thing custom fields", MAX_CUSTOM_FIELDS + 1),
            json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": "REGISTRY",
                    "customFields": Value::Array(over_the_bound.clone()),
                },
            }),
        ),
        (
            format!("{} thing group custom fields", MAX_CUSTOM_FIELDS + 1),
            json!({
                "thingGroupIndexingConfiguration": {
                    "thingGroupIndexingMode": "ON",
                    "customFields": Value::Array(over_the_bound.clone()),
                },
            }),
        ),
    ];

    for (name, request) in cases {
        let emulator = Emulator::new();
        let reply = emulator.update_indexing_configuration(request).await;
        assert_eq!(
            reply.status,
            400,
            "case {name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "InvalidRequestException",
            "case {name}: expected InvalidRequestException, got {}",
            reply.text()
        );
        let reported = emulator.reported(name).await;
        assert_eq!(
            reported,
            disabled_report(),
            "case {name}: a refused update changed the configuration to {reported}"
        );
    }
}

/// The emulator's ceiling on a caller-grown list is a ceiling, not a refusal of every
/// large list: exactly `MAX_CUSTOM_FIELDS` fields are recorded and reported.
#[tokio::test]
async fn custom_fields_are_recorded_up_to_the_bound() {
    let emulator = Emulator::new();
    let fields: Vec<Value> = (0..MAX_CUSTOM_FIELDS)
        .map(|index| field(&format!("attributes.f{index}"), "String"))
        .collect();
    let request = json!({
        "thingIndexingConfiguration": {
            "thingIndexingMode": "REGISTRY",
            "customFields": Value::Array(fields.clone()),
        },
    });
    let case = format!("{MAX_CUSTOM_FIELDS} custom fields");

    let reply = emulator.update_indexing_configuration(&request).await;
    assert_eq!(
        reply.status,
        200,
        "case {case}: expected 200, got {} body {}",
        reply.status,
        reply.text()
    );
    let reported = emulator.reported(&case).await;
    assert_eq!(
        reported["thingIndexingConfiguration"]["customFields"],
        Value::Array(fields),
        "case {case}: got {}",
        reported["thingIndexingConfiguration"]["customFields"]
    );
}

/// The documented cross-member rule: connectivity status indexing needs a thing index
/// that is not `OFF`, and is accepted under every thing indexing mode that is.
#[tokio::test]
async fn connectivity_status_indexing_requires_an_enabled_thing_index() {
    for thing_mode in THING_INDEXING_MODE_VALUES {
        for connectivity in THING_CONNECTIVITY_INDEXING_MODE_VALUES {
            let emulator = Emulator::new();
            let request = json!({
                "thingIndexingConfiguration": {
                    "thingIndexingMode": thing_mode,
                    "thingConnectivityIndexingMode": connectivity,
                },
            });
            let case =
                format!("thingIndexingMode {thing_mode} with thingConnectivityIndexingMode {connectivity}");
            let refused = *thing_mode == INDEXING_MODE_OFF && *connectivity != INDEXING_MODE_OFF;

            let reply = emulator.update_indexing_configuration(&request).await;
            if refused {
                assert_eq!(
                    reply.status,
                    400,
                    "case {case}: expected 400, got {} body {}",
                    reply.status,
                    reply.text()
                );
                assert_eq!(
                    reply.error_code(),
                    "InvalidRequestException",
                    "case {case}: expected InvalidRequestException, got {}",
                    reply.text()
                );
                let reported = emulator.reported(&case).await;
                assert_eq!(
                    reported,
                    disabled_report(),
                    "case {case}: a refused update changed the configuration to {reported}"
                );
            } else {
                assert_eq!(
                    reply.status,
                    200,
                    "case {case}: expected 200, got {} body {}",
                    reply.status,
                    reply.text()
                );
                let reported = emulator.reported(&case).await;
                assert_eq!(
                    reported["thingIndexingConfiguration"]["thingConnectivityIndexingMode"],
                    json!(connectivity),
                    "case {case}: expected thingConnectivityIndexingMode {connectivity}, got {}",
                    reported["thingIndexingConfiguration"]["thingConnectivityIndexingMode"]
                );
            }
        }
    }
}

/// Every geolocation order and connectivity api the model declares is accepted, so the
/// refusals above are about the value and not about the member.
#[tokio::test]
async fn every_modelled_filter_value_is_accepted() {
    for order in TARGET_FIELD_ORDER_VALUES {
        let emulator = Emulator::new();
        let request = json!({
            "thingIndexingConfiguration": {
                "thingIndexingMode": "REGISTRY_AND_SHADOW",
                "filter": { "geoLocations": [{ "name": "attributes.where", "order": order }] },
            },
        });
        let case = format!("a geolocation target ordered {order}");
        let reply = emulator.update_indexing_configuration(&request).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
        let reported = emulator.reported(&case).await;
        assert_eq!(
            reported["thingIndexingConfiguration"]["filter"]["geoLocations"][0]["order"],
            json!(order),
            "case {case}: got {}",
            reported["thingIndexingConfiguration"]["filter"]
        );
    }

    for api in FLEET_INDEXING_API_VALUES {
        let emulator = Emulator::new();
        let request = json!({
            "thingIndexingConfiguration": {
                "thingIndexingMode": "REGISTRY",
                "thingConnectivityIndexingMode": "STATUS",
                "filter": { "connectivity": { "includeSocketInformation": [api] } },
            },
        });
        let case = format!("a connectivity filter naming {api}");
        let reply = emulator.update_indexing_configuration(&request).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
        let reported = emulator.reported(&case).await;
        assert_eq!(
            reported["thingIndexingConfiguration"]["filter"]["connectivity"]
                ["includeSocketInformation"][0],
            json!(api),
            "case {case}: got {}",
            reported["thingIndexingConfiguration"]["filter"]
        );
    }
}
