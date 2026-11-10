// SPDX-License-Identifier: CC0-1.0

//! The ONE test harness for this crate's suites: how a request reaches the service, and
//! the readers every suite uses to look at what came back.
//!
//! Both suites in this test binary drive the service the way a client does — a `POST /`
//! with `X-Amz-Target: AWSMigrationHubMultiAccountService.<Operation>` through the
//! `MultiServiceRouter`, so routing, deserialization, constraint validation, the handler
//! and serialization all run — so the request building, the router mounting and the
//! response readers live here rather than once per suite, where a change to the
//! credential scope or the target prefix would reach only one of them.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_migrationhubconfig_awsmigrationhubmultiaccountservice::state::{
    ComAmazonawsMigrationhubconfigAwsmigrationhubmultiaccountserviceState as MhcState,
    HomeRegionControlRecord,
};
use service_com_amazonaws_migrationhubconfig_awsmigrationhubmultiaccountservice::EmulatedService;

/// The `X-Amz-Target` prefix for this service (OPERATIONS.md, "Wire protocol").
pub const TARGET_PREFIX: &str = "AWSMigrationHubMultiAccountService";

/// The model's `HomeRegion` pattern, from
/// `smithy_query(query="operation", operation="CreateHomeRegionControl")`.
pub const HOME_REGION_PATTERN: &str = "^([a-z]+)-([a-z]+)-([0-9]+)$";

/// Regions spanning that pattern, none of which the issue cited: two-letter and
/// three-letter area codes, several middle words, and a two-digit ordinal.
pub const HOME_REGIONS: &[&str] = &[
    "eu-central-1",
    "ap-southeast-3",
    "sa-east-1",
    "me-south-1",
    "cn-northwest-1",
    "us-west-12",
];

/// One emulator: the shared engine plus a router carrying only this service.
pub struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: smithy_server_runtime::MultiServiceRouter,
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // These tests exercise the service, not SigV4: the Authorization header
        // below is a routing signal only.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Call `operation` with `body`, returning the HTTP status and the parsed
    /// response body (`Value::Null` for an empty body).
    pub async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://migrationhub-config.us-west-2.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            // The SigV4 credential scope names the signing name (`mgh`), which is
            // how the router picks the service that claims the request.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEMULATOR/20260102/us-west-2/mgh/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000",
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("a test body always serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// The controls currently in the store, in store order.
    pub fn stored_control_ids(&self) -> Vec<String> {
        self.engine
            .state::<MhcState>()
            .lock()
            .home_region_controls
            .keys()
            .cloned()
            .collect()
    }

    /// Put `records` in the store directly, so a filter/pagination case can span
    /// more controls than one account's lifecycle ever holds. Only the control map
    /// is seeded: `DescribeHomeRegionControls` reports what the store holds and
    /// does not read the per-account index.
    pub fn seed(&self, records: Vec<HomeRegionControlRecord>) {
        let state = self.engine.state::<MhcState>();
        let mut store = state.lock();
        for record in records {
            store
                .home_region_controls
                .insert(record.control_id.clone(), record);
        }
    }
}

/// The account every emulated caller is: the canonical account id the vendored
/// `engine` crate vends (`engine::emulator_account_id`).
pub fn account() -> &'static str {
    engine::emulator_account_id()
}

/// A well-formed `CreateHomeRegionControl` request for `home_region`.
pub fn create_body(home_region: &str, dry_run: bool) -> Value {
    json!({
        "HomeRegion": home_region,
        "Target": { "Type": "ACCOUNT", "Id": account() },
        "DryRun": dry_run,
    })
}

/// Whether `control_id` satisfies the model's `ControlId` pattern
/// `^hrc-[a-z0-9]{12}$`.
pub fn is_well_formed_control_id(control_id: &str) -> bool {
    match control_id.strip_prefix("hrc-") {
        Some(suffix) => {
            suffix.len() == 12
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        }
        None => false,
    }
}

/// The awsJson error code a response carries, or a stand-in naming its absence.
pub fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("<no __type>")
}

/// The error message a response carries, or a stand-in naming its absence.
pub fn message(body: &Value) -> &str {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or("<no Message>")
}

/// The `ControlId`s of a `DescribeHomeRegionControls` page, in the order it listed them.
pub fn control_ids(body: &Value) -> Vec<String> {
    body["HomeRegionControls"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|control| {
            control["ControlId"]
                .as_str()
                .unwrap_or("<no ControlId>")
                .to_string()
        })
        .collect()
}
