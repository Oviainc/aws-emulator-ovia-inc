// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MigrationHub Config home-region-control lifecycle.
//!
//! Every case drives the service the way a client does: a `POST /` with
//! `X-Amz-Target: AWSMigrationHubMultiAccountService.<Operation>` through the
//! `MultiServiceRouter`, so routing, deserialization, constraint validation,
//! the handler and serialization all run.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_migrationhubconfig_awsmigrationhubmultiaccountservice::state::{
    ComAmazonawsMigrationhubconfigAwsmigrationhubmultiaccountserviceState as MhcState,
    HomeRegionControlRecord,
};
use service_com_amazonaws_migrationhubconfig_awsmigrationhubmultiaccountservice::EmulatedService;

/// The `X-Amz-Target` prefix for this service (OPERATIONS.md, "Wire protocol").
const TARGET_PREFIX: &str = "AWSMigrationHubMultiAccountService";

/// Regions spanning the model's `HomeRegion` pattern
/// (`^([a-z]+)-([a-z]+)-([0-9]+)$`, from
/// `smithy_query(query="operation", operation="CreateHomeRegionControl")`), none of
/// which the issue cited: two-letter and three-letter area codes, several middle
/// words, and a two-digit ordinal.
const HOME_REGIONS: &[&str] = &[
    "eu-central-1",
    "ap-southeast-3",
    "sa-east-1",
    "me-south-1",
    "cn-northwest-1",
    "us-west-12",
];

/// One emulator: the shared engine plus a router carrying only this service.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: smithy_server_runtime::MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
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
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
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
    fn stored_control_ids(&self) -> Vec<String> {
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
    fn seed(&self, records: Vec<HomeRegionControlRecord>) {
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
fn account() -> &'static str {
    engine::emulator_account_id()
}

fn create_body(home_region: &str, dry_run: bool) -> Value {
    json!({
        "HomeRegion": home_region,
        "Target": { "Type": "ACCOUNT", "Id": account() },
        "DryRun": dry_run,
    })
}

/// Whether `control_id` satisfies the model's `ControlId` pattern
/// `^hrc-[a-z0-9]{12}$`.
fn is_well_formed_control_id(control_id: &str) -> bool {
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

fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("<no __type>")
}

fn control_ids(body: &Value) -> Vec<String> {
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

#[tokio::test]
async fn the_home_region_control_lifecycle_round_trips_through_the_store() {
    let emulator = Emulator::new();

    // Nothing is configured yet: GetHomeRegion must not answer a hollow 200.
    let (status, body) = emulator.call("GetHomeRegion", &json!({})).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInputException"),
        "GetHomeRegion before any control: expected 400 InvalidInputException, got {status} {body}"
    );

    let (status, created) = emulator
        .call("CreateHomeRegionControl", &create_body("eu-central-1", false))
        .await;
    assert_eq!(status, 200, "CreateHomeRegionControl: got {status} {created}");
    let control = &created["HomeRegionControl"];
    let control_id = control["ControlId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        is_well_formed_control_id(&control_id),
        "created control id must match ^hrc-[a-z0-9]{{12}}$, got {control_id:?}"
    );
    assert_eq!(
        control["HomeRegion"], "eu-central-1",
        "created control: got {control}"
    );
    assert_eq!(
        control["Target"],
        json!({ "Type": "ACCOUNT", "Id": account() }),
        "created control target: got {control}"
    );
    assert!(
        control["RequestedTime"].as_f64().is_some(),
        "created control must carry a RequestedTime timestamp, got {control}"
    );
    assert_eq!(
        emulator.stored_control_ids(),
        vec![control_id.clone()],
        "the created control must be the one in the store"
    );

    // The created region is readable through both readers.
    let (status, body) = emulator.call("GetHomeRegion", &json!({})).await;
    assert_eq!(
        (status, &body["HomeRegion"]),
        (200, &json!("eu-central-1")),
        "GetHomeRegion after create: got {status} {body}"
    );

    // Each filter the model declares, applied to the one stored control.
    let cases: [(&str, Value, Vec<String>); 6] = [
        ("no filter", json!({}), vec![control_id.clone()]),
        (
            "matching ControlId",
            json!({ "ControlId": control_id.clone() }),
            vec![control_id.clone()],
        ),
        (
            "other ControlId",
            json!({ "ControlId": "hrc-000000000099" }),
            vec![],
        ),
        (
            "matching HomeRegion",
            json!({ "HomeRegion": "eu-central-1" }),
            vec![control_id.clone()],
        ),
        (
            "other HomeRegion",
            json!({ "HomeRegion": "ap-southeast-3" }),
            vec![],
        ),
        (
            "matching Target",
            json!({ "Target": { "Type": "ACCOUNT", "Id": account() } }),
            vec![control_id.clone()],
        ),
    ];
    for (case, filter, expected) in cases {
        let (status, body) = emulator.call("DescribeHomeRegionControls", &filter).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            control_ids(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case {case:?}: a single page must carry no NextToken, got {body}"
        );
    }

    // Delete releases the account's home region.
    let (status, body) = emulator
        .call(
            "DeleteHomeRegionControl",
            &json!({ "ControlId": control_id.clone() }),
        )
        .await;
    assert_eq!(status, 200, "DeleteHomeRegionControl: got {status} {body}");
    assert!(
        emulator.stored_control_ids().is_empty(),
        "delete must remove the control, store holds {:?}",
        emulator.stored_control_ids()
    );

    let (status, body) = emulator.call("DescribeHomeRegionControls", &json!({})).await;
    assert_eq!(
        (status, control_ids(&body)),
        (200, vec![]),
        "describe after delete: got {status} {body}"
    );

    let (status, body) = emulator.call("GetHomeRegion", &json!({})).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInputException"),
        "GetHomeRegion after delete: expected the region to be unset, got {status} {body}"
    );

    // Deleting the same id twice is refused: the second call removes nothing.
    let (status, body) = emulator
        .call(
            "DeleteHomeRegionControl",
            &json!({ "ControlId": control_id.clone() }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInputException"),
        "deleting {control_id} twice: got {status} {body}"
    );
}

#[tokio::test]
async fn any_region_the_model_accepts_round_trips_and_mints_a_fresh_control_id() {
    let emulator = Emulator::new();
    let mut seen: Vec<String> = Vec::new();

    for region in HOME_REGIONS {
        let (status, created) = emulator
            .call("CreateHomeRegionControl", &create_body(region, false))
            .await;
        assert_eq!(status, 200, "case {region:?}: got {status} {created}");
        let control_id = created["HomeRegionControl"]["ControlId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_well_formed_control_id(&control_id),
            "case {region:?}: control id must match ^hrc-[a-z0-9]{{12}}$, got {control_id:?}"
        );
        assert!(
            !seen.contains(&control_id),
            "case {region:?}: control id {control_id} was already minted ({seen:?})"
        );
        seen.push(control_id.clone());

        let (status, body) = emulator.call("GetHomeRegion", &json!({})).await;
        assert_eq!(
            (status, body["HomeRegion"].as_str()),
            (200, Some(*region)),
            "case {region:?}: GetHomeRegion must report the region just created, got {body}"
        );

        let (status, body) = emulator
            .call(
                "DeleteHomeRegionControl",
                &json!({ "ControlId": control_id.clone() }),
            )
            .await;
        assert_eq!(status, 200, "case {region:?}: delete got {status} {body}");
    }

    assert_eq!(
        seen.len(),
        HOME_REGIONS.len(),
        "every region must have minted its own control: {seen:?}"
    );
}

#[tokio::test]
async fn an_account_keeps_the_one_home_region_it_already_has() {
    let emulator = Emulator::new();

    let (_, created) = emulator
        .call("CreateHomeRegionControl", &create_body("sa-east-1", false))
        .await;
    let control_id = created["HomeRegionControl"]["ControlId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A different region is refused, and the stored one is untouched.
    for region in ["me-south-1", "ap-southeast-3"] {
        let (status, body) = emulator
            .call("CreateHomeRegionControl", &create_body(region, false))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInputException"),
            "case {region:?}: a second home region must be refused, got {status} {body}"
        );
    }

    // The same region again names the control that already holds it; no second
    // control is minted.
    let (status, body) = emulator
        .call("CreateHomeRegionControl", &create_body("sa-east-1", false))
        .await;
    assert_eq!(
        (status, body["HomeRegionControl"]["ControlId"].as_str()),
        (200, Some(control_id.as_str())),
        "re-creating the same home region must answer the existing control, got {status} {body}"
    );
    assert_eq!(
        emulator.stored_control_ids(),
        vec![control_id.clone()],
        "no second control may be minted for one account"
    );

    let (status, body) = emulator.call("GetHomeRegion", &json!({})).await;
    assert_eq!(
        (status, body["HomeRegion"].as_str()),
        (200, Some("sa-east-1")),
        "the home region must still be the one first set, got {status} {body}"
    );

    // A target that is not the current account is not a target this account may
    // set (`Target$Id`: "This must be the current account").
    for other in ["123456789012", "999999999999"] {
        let body = json!({
            "HomeRegion": "sa-east-1",
            "Target": { "Type": "ACCOUNT", "Id": other },
            "DryRun": false,
        });
        let (status, body) = emulator.call("CreateHomeRegionControl", &body).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInputException"),
            "case target {other:?}: expected 400 InvalidInputException, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_dry_run_create_reports_dry_run_operation_and_persists_nothing() {
    let emulator = Emulator::new();

    // Before anything exists, and again with a control in the store: a dry run
    // never changes what a later caller sees.
    let (status, body) = emulator
        .call("CreateHomeRegionControl", &create_body("eu-central-1", true))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "DryRunOperation"),
        "dry run on an unconfigured account: got {status} {body}"
    );
    assert!(
        emulator.stored_control_ids().is_empty(),
        "a dry run must persist nothing, store holds {:?}",
        emulator.stored_control_ids()
    );
    let (status, body) = emulator.call("GetHomeRegion", &json!({})).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInputException"),
        "a dry run must leave the home region unset, got {status} {body}"
    );

    let (_, created) = emulator
        .call("CreateHomeRegionControl", &create_body("eu-central-1", false))
        .await;
    let control_id = created["HomeRegionControl"]["ControlId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, body) = emulator
        .call("CreateHomeRegionControl", &create_body("eu-central-1", true))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "DryRunOperation"),
        "dry run on a configured account: got {status} {body}"
    );
    assert_eq!(
        emulator.stored_control_ids(),
        vec![control_id],
        "a dry run must not add a control"
    );

    // A dry run of a request that would be refused answers the refusal, not
    // `DryRunOperation`: the request is validated first.
    let (status, body) = emulator
        .call("CreateHomeRegionControl", &create_body("me-south-1", true))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "InvalidInputException"),
        "dry run of a second home region: got {status} {body}"
    );
}

#[tokio::test]
async fn describe_filters_and_paginates_the_controls_in_the_store() {
    let emulator = Emulator::new();
    // Seven controls, so a page size smaller than the store exercises the token
    // on several boundaries. Ids ascend in creation order, which is the order
    // `DescribeHomeRegionControls` walks.
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let seeded: Vec<HomeRegionControlRecord> = (1..=7u64)
        .map(|n| HomeRegionControlRecord {
            control_id: format!("hrc-{n:012x}"),
            home_region: HOME_REGIONS[(n as usize - 1) % HOME_REGIONS.len()].to_string(),
            target_type: "ACCOUNT".to_string(),
            target_id: Some(account().to_string()),
            requested_time: base + Duration::from_secs(n),
        })
        .collect();
    let all_ids: Vec<String> = seeded
        .iter()
        .map(|record| record.control_id.clone())
        .collect();
    emulator.seed(seeded);

    // Filters, over a store big enough for a filter to select a subset.
    let filter_cases: [(&str, Value, Vec<String>); 5] = [
        ("no filter", json!({}), all_ids.clone()),
        (
            "ControlId selects one",
            json!({ "ControlId": all_ids[4].clone() }),
            vec![all_ids[4].clone()],
        ),
        (
            "unknown ControlId selects none",
            json!({ "ControlId": "hrc-0000000000ff" }),
            vec![],
        ),
        (
            "HomeRegion selects every control in it",
            json!({ "HomeRegion": HOME_REGIONS[0] }),
            vec![all_ids[0].clone(), all_ids[6].clone()],
        ),
        (
            "a Target of another account selects none",
            json!({ "Target": { "Type": "ACCOUNT", "Id": "123456789012" } }),
            vec![],
        ),
    ];
    for (case, filter, expected) in filter_cases {
        let (status, body) = emulator.call("DescribeHomeRegionControls", &filter).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            control_ids(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // Page sizes spanning the model's `MaxResults` range (1..100): below, at and
    // above the number of stored controls, plus the published bounds.
    for max_results in [1_i64, 2, 3, 6, 7, 8, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": max_results });
            if let Some(token) = token.clone() {
                request["NextToken"] = Value::String(token);
            }
            let (status, body) = emulator.call("DescribeHomeRegionControls", &request).await;
            assert_eq!(
                status, 200,
                "case MaxResults={max_results}: got {status} {body}"
            );
            let page = control_ids(&body);
            assert!(
                page.len() <= usize::try_from(max_results).expect("a positive page size"),
                "case MaxResults={max_results}: a page carried {} controls, got {body}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= all_ids.len() + 1,
                "case MaxResults={max_results}: pagination did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, all_ids,
            "case MaxResults={max_results}: walking every page must yield each control once, in store order"
        );
    }

    // A token this service never issued is refused rather than silently ignored.
    for bad_token in ["notatoken", "6e6f70652d313233", "aGVsbG8="] {
        let (status, body) = emulator
            .call(
                "DescribeHomeRegionControls",
                &json!({ "NextToken": bad_token }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInputException"),
            "case token {bad_token:?}: expected 400 InvalidInputException, got {status} {body}"
        );
    }

    // An empty token is no token at all (the model allows length 0).
    let (status, body) = emulator
        .call("DescribeHomeRegionControls", &json!({ "NextToken": "" }))
        .await;
    assert_eq!(
        (status, control_ids(&body)),
        (200, all_ids.clone()),
        "an empty NextToken must start at the first control, got {status} {body}"
    );
}

#[tokio::test]
async fn a_request_outside_the_models_constraints_is_refused_as_invalid_input() {
    let emulator = Emulator::new();

    // Each case violates one rule the model declares, so every one must answer
    // the error this API declares for bad input rather than be accepted.
    let cases: [(&str, &str, Value); 8] = [
        (
            "HomeRegion outside the pattern",
            "CreateHomeRegionControl",
            create_body("EU-CENTRAL-1", false),
        ),
        (
            "HomeRegion shorter than the minimum length",
            "CreateHomeRegionControl",
            create_body("", false),
        ),
        (
            "HomeRegion absent",
            "CreateHomeRegionControl",
            json!({ "Target": { "Type": "ACCOUNT", "Id": account() }, "DryRun": false }),
        ),
        (
            "Target absent",
            "CreateHomeRegionControl",
            json!({ "HomeRegion": "eu-central-1", "DryRun": false }),
        ),
        (
            "Target type outside the enum",
            "CreateHomeRegionControl",
            json!({
                "HomeRegion": "eu-central-1",
                "Target": { "Type": "ORGANIZATION", "Id": account() },
                "DryRun": false,
            }),
        ),
        (
            "Target id outside the pattern",
            "CreateHomeRegionControl",
            json!({
                "HomeRegion": "eu-central-1",
                "Target": { "Type": "ACCOUNT", "Id": "12345" },
                "DryRun": false,
            }),
        ),
        (
            "ControlId outside the pattern",
            "DeleteHomeRegionControl",
            json!({ "ControlId": "hrc-NOTVALID" }),
        ),
        (
            "MaxResults above the range",
            "DescribeHomeRegionControls",
            json!({ "MaxResults": 101 }),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInputException"),
            "case {case:?}: expected 400 InvalidInputException, got {status} {body}"
        );
    }

    assert!(
        emulator.stored_control_ids().is_empty(),
        "a refused request must persist nothing, store holds {:?}",
        emulator.stored_control_ids()
    );
}
