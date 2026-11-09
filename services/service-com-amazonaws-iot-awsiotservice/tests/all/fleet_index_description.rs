// SPDX-License-Identifier: CC0-1.0

//! The account's FLEET INDEXES, as `DescribeIndex` describes them, end to end over
//! HTTP.
//!
//! Every test drives real requests through the `MultiServiceRouter` this crate's
//! service is mounted in, so routing, the restJson1 `GET /indices/{indexName}` and
//! `POST /indexing/config` bindings, the handlers and the response serialization are
//! all exercised.
//!
//! The rules under test, none of them specific to one index or one configuration:
//!
//! * an index EXISTS for the account exactly while the indexing-configuration
//!   section that drives it is not `OFF`, so a describe of one of the indexes this
//!   service recognises that the account has not enabled is refused with the modelled
//!   `ResourceNotFoundException` (404), naming the requested index, while a name that is
//!   no fleet index at all is refused `InvalidRequestException` (400) instead;
//! * `schema` is what the thing index CONTAINS, added up from the configuration's
//!   data sources: the five values `DescribeIndexResponse$schema` documents
//!   (`smithy_query(query="doc",shape="DescribeIndexResponse",member="schema")`);
//! * `indexStatus` is the build the last configuration change started — `BUILDING`
//!   for an index that did not exist before it, `REBUILDING` for one that did — until
//!   the clock makes that build due, and `ACTIVE` from then on;
//! * describing an index writes nothing: a later caller sees what the earlier one
//!   saw, and the stored configuration is untouched.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::ops::types::{
    ThingGroupIndexingConfiguration, ThingIndexingConfiguration, INDEX_STATUS_VALUES,
};
use service_com_amazonaws_iot_awsiotservice::state::{
    ComAmazonawsIotAwsiotserviceState, FleetIndexBuild, IndexingConfiguration, FLEET_INDEX_NAMES,
    INDEXING_MODE_OFF, INDEX_BUILD_SPAN, INDEX_STATUS_ACTIVE, INDEX_STATUS_BUILDING,
    INDEX_STATUS_REBUILDING, THING_GROUP_INDEX_NAME, THING_INDEX_NAME,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for. An index is account-wide, so
/// no assertion depends on it.
const REGION: &str = "ap-south-1";

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

    async fn dispatch(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
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
            .expect("the fleet-index request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }

    async fn describe_index(&self, index_name: &str) -> Reply {
        self.dispatch("GET", &format!("/indices/{index_name}"), None)
            .await
    }

    async fn update_indexing_configuration(&self, body: &Value) -> Reply {
        self.dispatch("POST", "/indexing/config", Some(body)).await
    }

    async fn get_indexing_configuration(&self) -> Reply {
        self.dispatch("GET", "/indexing/config", None).await
    }

    /// Record `request` as the account's indexing configuration, asserting the update
    /// itself was accepted — the write whose effect the describes then read back.
    async fn enable(&self, case: &str, request: &Value) {
        let reply = self.update_indexing_configuration(request).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected the update to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
    }

    /// The description a successful describe reports, as JSON.
    async fn described(&self, case: &str, index_name: &str) -> Value {
        let reply = self.describe_index(index_name).await;
        assert_eq!(
            reply.status,
            200,
            "case {case}: expected DescribeIndex {index_name} to answer 200, got {} body {}",
            reply.status,
            reply.text()
        );
        reply.json()
    }

    /// Finish the build the clock owes index `index_name`, by moving the recorded due
    /// instant into the past — the state a caller that waited out
    /// [`INDEX_BUILD_SPAN`] would find, without the test sleeping for it.
    fn finish_build(&self, index_name: &str) {
        let state = self.state();
        let mut store = state.lock();
        let build = store
            .index_builds
            .get_mut(index_name)
            .unwrap_or_else(|| panic!("no build is recorded for {index_name}"));
        build.active_at = SystemTime::now() - INDEX_BUILD_SPAN;
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

    /// The message a restJson1 error response carries.
    fn error_message(&self) -> String {
        self.json()["message"]
            .as_str()
            .unwrap_or_else(|| panic!("no message in {}", self.text()))
            .to_string()
    }
}

/// One thing indexing configuration and the schema the thing index reports under it.
struct SchemaCase {
    name: &'static str,
    /// The `thingIndexingConfiguration` the update supplies.
    section: fn() -> Value,
    /// The value `DescribeIndexResponse$schema` carries for it.
    schema: &'static str,
}

/// Configurations spanning what the rule generalises over: each thing indexing mode
/// the model declares, connectivity indexing on and off beside each of them, and each
/// further data source on its own and together — so every documented `schema` value
/// is reached, by more than one configuration where more than one produces it.
const SCHEMA_CASES: &[SchemaCase] = &[
    SchemaCase {
        name: "registry data only",
        section: || json!({ "thingIndexingMode": "REGISTRY" }),
        schema: "REGISTRY",
    },
    SchemaCase {
        name: "registry data, connectivity explicitly off",
        section: || {
            json!({
                "thingIndexingMode": "REGISTRY",
                "thingConnectivityIndexingMode": "OFF",
            })
        },
        schema: "REGISTRY",
    },
    SchemaCase {
        name: "registry and shadow data",
        section: || json!({ "thingIndexingMode": "REGISTRY_AND_SHADOW" }),
        schema: "REGISTRY_AND_SHADOW",
    },
    SchemaCase {
        name: "registry data and connectivity status",
        section: || {
            json!({
                "thingIndexingMode": "REGISTRY",
                "thingConnectivityIndexingMode": "STATUS",
            })
        },
        schema: "REGISTRY_AND_CONNECTIVITY_STATUS",
    },
    SchemaCase {
        name: "registry, shadow and connectivity status data",
        section: || {
            json!({
                "thingIndexingMode": "REGISTRY_AND_SHADOW",
                "thingConnectivityIndexingMode": "STATUS",
            })
        },
        schema: "REGISTRY_AND_SHADOW_AND_CONNECTIVITY_STATUS",
    },
    SchemaCase {
        name: "registry data and named shadows",
        section: || {
            json!({
                "thingIndexingMode": "REGISTRY",
                "namedShadowIndexingMode": "ON",
            })
        },
        schema: "MULTI_INDEXING_MODE",
    },
    SchemaCase {
        name: "registry and shadow data and Device Defender violations",
        section: || {
            json!({
                "thingIndexingMode": "REGISTRY_AND_SHADOW",
                "deviceDefenderIndexingMode": "VIOLATIONS",
            })
        },
        schema: "MULTI_INDEXING_MODE",
    },
    SchemaCase {
        name: "every data source at once",
        section: || {
            json!({
                "thingIndexingMode": "REGISTRY_AND_SHADOW",
                "thingConnectivityIndexingMode": "STATUS",
                "deviceDefenderIndexingMode": "VIOLATIONS",
                "namedShadowIndexingMode": "ON",
            })
        },
        schema: "MULTI_INDEXING_MODE",
    },
];

/// One fleet index, with the update body that enables the section driving it — the
/// pair every rule that holds for BOTH indexes is tested over.
type Enablement = (&'static str, fn() -> Value);

/// Both fleet indexes, each with the update that enables it.
const ENABLEMENTS: &[Enablement] = &[
    (THING_INDEX_NAME, thing_index_on),
    (THING_GROUP_INDEX_NAME, thing_group_index_on),
];

/// The configuration that enables the thing index, as a request body.
fn thing_index_on() -> Value {
    json!({ "thingIndexingConfiguration": { "thingIndexingMode": "REGISTRY" } })
}

/// The configuration that enables the thing group index, as a request body.
fn thing_group_index_on() -> Value {
    json!({ "thingGroupIndexingConfiguration": { "thingGroupIndexingMode": "ON" } })
}

/// A configuration with both indexes enabled and settled, seeded straight into the
/// store so a describe can be tested against a record no request in the test wrote.
fn both_indexes_enabled() -> IndexingConfiguration {
    IndexingConfiguration {
        thing: ThingIndexingConfiguration {
            thing_indexing_mode: "REGISTRY_AND_SHADOW".to_string(),
            thing_connectivity_indexing_mode: Some("STATUS".to_string()),
            device_defender_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
            named_shadow_indexing_mode: Some(INDEXING_MODE_OFF.to_string()),
            managed_fields: None,
            custom_fields: None,
            filter: None,
        },
        thing_group: ThingGroupIndexingConfiguration {
            thing_group_indexing_mode: "ON".to_string(),
            managed_fields: None,
            custom_fields: None,
        },
    }
}

/// The thing index's `schema` is the configuration's data sources added up: every
/// value the model documents, reached through the write that configured it.
#[tokio::test]
async fn the_thing_index_schema_is_what_the_configuration_indexes() {
    for case in SCHEMA_CASES {
        let emulator = Emulator::new();
        let request = json!({ "thingIndexingConfiguration": (case.section)() });
        emulator.enable(case.name, &request).await;
        emulator.finish_build(THING_INDEX_NAME);

        let described = emulator.described(case.name, THING_INDEX_NAME).await;
        assert_eq!(
            described,
            json!({
                "indexName": THING_INDEX_NAME,
                "indexStatus": INDEX_STATUS_ACTIVE,
                "schema": case.schema,
            }),
            "case {}: expected schema {} for {request}, got {described}",
            case.name,
            case.schema
        );
    }
}

/// An index the account has not enabled does not exist: every fleet index is refused
/// with the modelled `ResourceNotFoundException` (404) while its section is `OFF`, and
/// served once it is on — and turning the section off again makes it gone.
#[tokio::test]
async fn an_index_exists_exactly_while_its_configuration_section_is_on() {
    for (index_name, enable) in ENABLEMENTS {
        let emulator = Emulator::new();
        // Before any update: an account that has never configured fleet indexing has
        // no indexes at all.
        let reply = emulator.describe_index(index_name).await;
        assert_eq!(
            reply.status,
            404,
            "case {index_name} before enabling: expected 404, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {index_name} before enabling: expected ResourceNotFoundException, got {}",
            reply.text()
        );

        emulator.enable(index_name, &enable()).await;
        let described = emulator.described(index_name, index_name).await;
        assert_eq!(
            described["indexName"],
            json!(index_name),
            "case {index_name}: expected the described index to name itself, got {described}"
        );

        // The same section back at OFF: the index is gone again, so the describe that
        // just succeeded now fails.
        let off = match *index_name {
            THING_GROUP_INDEX_NAME => json!({
                "thingGroupIndexingConfiguration": { "thingGroupIndexingMode": INDEXING_MODE_OFF },
            }),
            _ => json!({
                "thingIndexingConfiguration": { "thingIndexingMode": INDEXING_MODE_OFF },
            }),
        };
        emulator.enable(index_name, &off).await;
        let reply = emulator.describe_index(index_name).await;
        assert_eq!(
            reply.status,
            404,
            "case {index_name} after disabling: expected 404, got {} body {}",
            reply.status,
            reply.text()
        );
    }
}

/// A name that is no fleet index at all is not a missing resource: whatever the account
/// has enabled, it is refused as a request this plane cannot act on — `400`
/// `InvalidRequestException` naming the member and the name that was asked for.
#[tokio::test]
async fn a_name_that_is_no_fleet_index_is_refused() {
    // Names the modelled `IndexName` pattern accepts (`^[a-zA-Z0-9:_-]+$`) that name
    // no index: a near miss of each real one, the wrong case of one, and a name a
    // caller invented.
    let unknown = [
        "AWS_Thing",
        "AWS_ThingGroup",
        "aws_things",
        "my-fleet-index",
        "AWS_Things:1",
    ];
    let emulator = Emulator::new();
    // Both real indexes are enabled, so a refusal here is about the NAME and not
    // about an account with no indexes.
    emulator.state().lock().indexing_configuration = both_indexes_enabled();

    for index_name in unknown {
        assert!(
            !FLEET_INDEX_NAMES.contains(&index_name),
            "case {index_name}: this row must name no fleet index, and {FLEET_INDEX_NAMES:?} holds it"
        );
        let reply = emulator.describe_index(index_name).await;
        assert_eq!(
            reply.status,
            400,
            "case {index_name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "InvalidRequestException",
            "case {index_name}: expected InvalidRequestException, got {}",
            reply.text()
        );
        assert_eq!(
            reply.error_message(),
            format!("Unrecognized indexName {index_name}"),
            "case {index_name}: the refusal must name the requested index, got {}",
            reply.text()
        );
    }

    // The enabled indexes are still served, so the refusals above are about those
    // names alone.
    for index_name in FLEET_INDEX_NAMES {
        let described = emulator.described("both indexes enabled", index_name).await;
        assert_eq!(
            described["indexName"],
            json!(index_name),
            "case {index_name}: expected the enabled index to be described, got {described}"
        );
    }
}

/// The status is the build the last configuration change started: `BUILDING` for an
/// index that did not exist before it, `REBUILDING` for one that did, and `ACTIVE`
/// once the clock has made that build due.
#[tokio::test]
async fn the_status_is_the_build_the_last_configuration_change_started() {
    for (index_name, enable) in ENABLEMENTS {
        let emulator = Emulator::new();

        // The change that brought the index into existence: it is built from nothing.
        emulator.enable(index_name, &enable()).await;
        let building = emulator.described(index_name, index_name).await;
        assert_eq!(
            building["indexStatus"],
            json!(INDEX_STATUS_BUILDING),
            "case {index_name} just enabled: expected {INDEX_STATUS_BUILDING}, got {building}"
        );

        // The build the clock owed is due: the index is active, with no further call.
        emulator.finish_build(index_name);
        let active = emulator.described(index_name, index_name).await;
        assert_eq!(
            active["indexStatus"],
            json!(INDEX_STATUS_ACTIVE),
            "case {index_name} after its build was due: expected {INDEX_STATUS_ACTIVE}, got {active}"
        );

        // A change to an index that already existed re-indexes what it held.
        emulator.enable(index_name, &enable()).await;
        let rebuilding = emulator.described(index_name, index_name).await;
        assert_eq!(
            rebuilding["indexStatus"],
            json!(INDEX_STATUS_REBUILDING),
            "case {index_name} reconfigured: expected {INDEX_STATUS_REBUILDING}, got {rebuilding}"
        );
        emulator.finish_build(index_name);
        let active_again = emulator.described(index_name, index_name).await;
        assert_eq!(
            active_again["indexStatus"],
            json!(INDEX_STATUS_ACTIVE),
            "case {index_name} after its rebuild was due: expected {INDEX_STATUS_ACTIVE}, \
             got {active_again}"
        );
    }
}

/// Every status reported is one the model declares, and the status a describe reports
/// is the one the store HOLDS — seeded straight in, so no response can be a fixed
/// value.
#[tokio::test]
async fn the_reported_status_is_the_one_the_store_holds() {
    let seeds: &[(&str, &str)] = &[
        ("a build still running", INDEX_STATUS_BUILDING),
        ("a rebuild still running", INDEX_STATUS_REBUILDING),
    ];

    for (case, pending_status) in seeds {
        assert!(
            INDEX_STATUS_VALUES.contains(pending_status),
            "case {case}: {pending_status} must be one of the modelled {INDEX_STATUS_VALUES:?}"
        );
        assert!(
            INDEX_STATUS_VALUES.contains(&INDEX_STATUS_ACTIVE),
            "case {case}: {INDEX_STATUS_ACTIVE} must be one of the modelled {INDEX_STATUS_VALUES:?}"
        );
        let emulator = Emulator::new();
        emulator.state().lock().indexing_configuration = both_indexes_enabled();
        // One index is mid-build and the other has no build outstanding, so the two
        // describes cannot both be satisfied by one answer.
        emulator.state().lock().index_builds.insert(
            THING_INDEX_NAME.to_string(),
            FleetIndexBuild::new(pending_status, SystemTime::now()),
        );

        let thing_index = emulator.described(case, THING_INDEX_NAME).await;
        assert_eq!(
            thing_index["indexStatus"],
            json!(*pending_status),
            "case {case}: expected {pending_status}, got {thing_index}"
        );
        let group_index = emulator.described(case, THING_GROUP_INDEX_NAME).await;
        assert_eq!(
            group_index["indexStatus"],
            json!(INDEX_STATUS_ACTIVE),
            "case {case}: an index with no build outstanding is active, got {group_index}"
        );
    }
}

/// The thing group index reports its name and status but no `schema`: the value list
/// `DescribeIndexResponse$schema` documents describes what a THING index contains.
#[tokio::test]
async fn the_thing_group_index_reports_no_schema() {
    let emulator = Emulator::new();
    emulator
        .enable("thing group indexing on", &thing_group_index_on())
        .await;
    emulator.finish_build(THING_GROUP_INDEX_NAME);

    let described = emulator
        .described("thing group indexing on", THING_GROUP_INDEX_NAME)
        .await;
    assert_eq!(
        described,
        json!({
            "indexName": THING_GROUP_INDEX_NAME,
            "indexStatus": INDEX_STATUS_ACTIVE,
        }),
        "case thing group indexing on: expected the group index without a schema, got {described}"
    );
}

/// Describing an index changes nothing a later caller can see: repeated describes
/// report the same description, the build they observed is not advanced by the read,
/// and the stored configuration is untouched.
#[tokio::test]
async fn describing_an_index_changes_nothing() {
    let emulator = Emulator::new();
    emulator
        .enable("both sections enabled", &thing_index_on())
        .await;
    emulator
        .enable("both sections enabled", &thing_group_index_on())
        .await;
    let configuration_before = emulator.get_indexing_configuration().await.json();

    for index_name in FLEET_INDEX_NAMES {
        let first = emulator.described("repeated describe", index_name).await;
        let second = emulator.described("repeated describe", index_name).await;
        assert_eq!(
            first, second,
            "case {index_name}: a describe changed what the next one reports: {first} then {second}"
        );
        assert_eq!(
            first["indexStatus"],
            json!(INDEX_STATUS_BUILDING),
            "case {index_name}: the build must not be advanced by reading it, got {first}"
        );
    }

    let configuration_after = emulator.get_indexing_configuration().await.json();
    assert_eq!(
        configuration_after, configuration_before,
        "case both sections enabled: describing an index changed the stored configuration to \
         {configuration_after}"
    );
}
