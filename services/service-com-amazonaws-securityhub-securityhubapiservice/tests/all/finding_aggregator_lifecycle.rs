// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Security Hub CSPM finding aggregator (cross-Region
//! aggregation) lifecycle: CreateFindingAggregator → ListFindingAggregators →
//! GetFindingAggregator → DeleteFindingAggregator.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// The Region most cases sign for and therefore create the aggregator in — its home
/// Region.
const HOME_REGION: &str = "us-west-2";

/// One emulator with the Security Hub service mounted, driven over HTTP. Each call
/// names the Region it is signed for, which is how a handler learns the Region (the
/// SigV4 credential scope), exactly as a signed SDK request carries it.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request and return `(status, error type, parsed body)`. An empty
    /// body parses as `Value::Null`, which is what an operation with no output
    /// members returns; the error type is the `x-amzn-errortype` header a refusal
    /// carries, which is the error CODE an SDK reports.
    async fn dispatch(
        &self,
        method: &str,
        path: &str,
        region: &str,
        body: Option<Value>,
    ) -> (u16, String, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}/securityhub/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(serialized.unwrap_or_default()))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, error_type, parsed)
    }

    /// Dispatch one request and return `(status, parsed body)`.
    async fn call(&self, method: &str, path: &str, region: &str, body: Option<Value>) -> (u16, Value) {
        let (status, _, payload) = self.dispatch(method, path, region, body).await;
        (status, payload)
    }

    async fn enable(&self, region: &str) -> (u16, Value) {
        self.call("POST", "/accounts", region, Some(json!({}))).await
    }

    async fn create(&self, region: &str, body: Value) -> (u16, Value) {
        self.call("POST", "/findingAggregator/create", region, Some(body))
            .await
    }

    /// `CreateFindingAggregator`, keeping the error type the refusal carries.
    async fn create_checked(&self, region: &str, body: Value) -> (u16, String, Value) {
        self.dispatch("POST", "/findingAggregator/create", region, Some(body))
            .await
    }

    async fn list(&self, region: &str, query: &str) -> (u16, Value) {
        let path = if query.is_empty() {
            "/findingAggregator/list".to_string()
        } else {
            format!("/findingAggregator/list?{query}")
        };
        self.call("GET", &path, region, None).await
    }

    async fn get(&self, region: &str, arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/findingAggregator/get/{arn}"), region, None)
            .await
    }

    async fn delete(&self, region: &str, arn: &str) -> (u16, Value) {
        self.call(
            "DELETE",
            &format!("/findingAggregator/delete/{arn}"),
            region,
            None,
        )
        .await
    }

    /// A harness with Security Hub CSPM enabled in each of `regions`.
    async fn enabled_in(regions: &[&str]) -> Self {
        let harness = Self::new();
        for region in regions {
            let (status, payload) = harness.enable(region).await;
            assert_eq!(
                status, 200,
                "EnableSecurityHub in {region}: expected 200, got {status} with {payload}"
            );
        }
        harness
    }
}

/// The error message a refusal carries, under the wire name the restJson1 error
/// payload uses (`Message`), falling back to the lowercase spelling.
fn error_message(payload: &Value) -> String {
    payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The ARNs `ListFindingAggregators` reported, in the order it listed them.
fn listed_arns(payload: &Value) -> Vec<String> {
    payload["FindingAggregators"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry["FindingAggregatorArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: the finding aggregator list is computed from persisted state — empty until a
/// create stores one, carrying exactly the created ARN afterwards, and empty again
/// once the delete removes it.
#[tokio::test]
async fn list_finding_aggregators_reports_persisted_state() {
    let harness = Harness::enabled_in(&[HOME_REGION]).await;

    let (status, payload) = harness.list(HOME_REGION, "").await;
    assert_eq!(
        status, 200,
        "ListFindingAggregators on a fresh hub: expected 200, got {status} with {payload}"
    );
    assert_eq!(
        listed_arns(&payload),
        Vec::<String>::new(),
        "a fresh hub aggregates nothing: expected no aggregators, got {payload}"
    );
    assert!(
        payload["NextToken"].is_null(),
        "an empty page has no NextToken, got {:?}",
        payload["NextToken"]
    );

    let (status, created) = harness
        .create(HOME_REGION, json!({ "RegionLinkingMode": "ALL_REGIONS" }))
        .await;
    assert_eq!(
        status, 200,
        "CreateFindingAggregator: expected 200, got {status} with {created}"
    );
    let arn = created["FindingAggregatorArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        arn.starts_with(&format!(
            "arn:aws:securityhub:{HOME_REGION}:{ACCOUNT_ID}:finding-aggregator/"
        )),
        "the aggregator ARN names its home Region, account and resource type, got {arn:?}"
    );
    assert_eq!(
        created["FindingAggregationRegion"].as_str(),
        Some(HOME_REGION),
        "the home Region is the Region the create was signed for, got {created}"
    );

    let (status, payload) = harness.list(HOME_REGION, "").await;
    assert_eq!(
        status, 200,
        "ListFindingAggregators after create: expected 200, got {status} with {payload}"
    );
    assert_eq!(
        listed_arns(&payload),
        vec![arn.clone()],
        "the list must report the created aggregator, got {payload}"
    );

    // Round trip: the ARN the list reports reads back as the same resource.
    let (status, fetched) = harness.get(HOME_REGION, &arn).await;
    assert_eq!(
        status, 200,
        "GetFindingAggregator for a listed ARN: expected 200, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["FindingAggregatorArn"].as_str(),
        Some(arn.as_str()),
        "GetFindingAggregator must report the ARN it was asked for, got {fetched}"
    );
    assert_eq!(
        fetched["RegionLinkingMode"].as_str(),
        Some("ALL_REGIONS"),
        "GetFindingAggregator must report the stored RegionLinkingMode, got {fetched}"
    );

    // A read leaves the list exactly as it found it.
    let (_, after_read) = harness.list(HOME_REGION, "").await;
    assert_eq!(
        listed_arns(&after_read),
        vec![arn.clone()],
        "reading an aggregator must not change what a later list sees, got {after_read}"
    );

    let (status, payload) = harness.delete(HOME_REGION, &arn).await;
    assert_eq!(
        status, 200,
        "DeleteFindingAggregator: expected 200, got {status} with {payload}"
    );
    let (status, payload) = harness.list(HOME_REGION, "").await;
    assert_eq!(
        status, 200,
        "ListFindingAggregators after delete: expected 200, got {status} with {payload}"
    );
    assert_eq!(
        listed_arns(&payload),
        Vec::<String>::new(),
        "a deleted aggregator must leave the list empty, got {payload}"
    );
    let (status, payload) = harness.get(HOME_REGION, &arn).await;
    assert_eq!(
        status, 404,
        "GetFindingAggregator for a deleted ARN: expected 404, got {status} with {payload}"
    );
}

/// Every `RegionLinkingMode` the model documents, with the `Regions` list each one
/// consumes. Source: `smithy_query(query="doc",
/// shape="CreateFindingAggregatorRequest", member="RegionLinkingMode")` lists
/// `ALL_REGIONS`, `ALL_REGIONS_EXCEPT_SPECIFIED`, `SPECIFIED_REGIONS` and
/// `NO_REGIONS`; the `Regions` member documents the list as read by the two
/// `*_SPECIFIED*` modes only.
const REGION_LINKING_MODE_CASES: [(&str, &[&str]); 5] = [
    ("ALL_REGIONS", &[]),
    ("ALL_REGIONS_EXCEPT_SPECIFIED", &["us-east-1"]),
    (
        "ALL_REGIONS_EXCEPT_SPECIFIED",
        &["us-east-1", "eu-west-1", "ap-south-1"],
    ),
    ("SPECIFIED_REGIONS", &["eu-central-1"]),
    ("NO_REGIONS", &[]),
];

/// RULE: a create stores the submitted `RegionLinkingMode` and the `Regions` list the
/// mode consumes, and every later read of the aggregator reports them unchanged.
#[tokio::test]
async fn every_documented_region_linking_mode_round_trips() {
    for (mode, regions) in REGION_LINKING_MODE_CASES {
        let case = format!("{mode} with {regions:?}");
        let harness = Harness::enabled_in(&[HOME_REGION]).await;

        let mut body = json!({ "RegionLinkingMode": mode });
        if !regions.is_empty() {
            body["Regions"] = json!(regions);
        }
        let (status, created) = harness.create(HOME_REGION, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateFindingAggregator expected 200, got {status} with {created}"
        );
        let arn = created["FindingAggregatorArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let expected_regions: Option<Vec<String>> = (!regions.is_empty())
            .then(|| regions.iter().map(|r| (*r).to_string()).collect());
        for (source, payload) in [
            ("CreateFindingAggregator", created.clone()),
            (
                "GetFindingAggregator",
                harness.get(HOME_REGION, &arn).await.1,
            ),
        ] {
            assert_eq!(
                payload["RegionLinkingMode"].as_str(),
                Some(mode),
                "case {case:?}: {source} must report the submitted mode, got {payload}"
            );
            let actual: Option<Vec<String>> = payload["Regions"].as_array().map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default().to_string())
                    .collect()
            });
            assert_eq!(
                actual, expected_regions,
                "case {case:?}: {source} must report the stored Regions, got {payload}"
            );
        }

        let (_, listed) = harness.list(HOME_REGION, "").await;
        assert_eq!(
            listed_arns(&listed),
            vec![arn],
            "case {case:?}: the created aggregator must be listed, got {listed}"
        );
    }
}

/// Requests whose `RegionLinkingMode` is not a documented value, that disagree with
/// the `Regions` list the mode consumes, or whose `Regions` list exceeds the bound
/// this service keeps on an unmodelled list (64 entries, 64 characters each). Each is
/// a bad request, so each is refused with `InvalidInputException` (HTTP 400) and
/// nothing is stored.
const INVALID_CREATE_CASES: [&str; 10] = [
    "unknown mode",
    "lowercase mode",
    "empty mode",
    "ALL_REGIONS with a Regions list",
    "NO_REGIONS with a Regions list",
    "SPECIFIED_REGIONS without Regions",
    "ALL_REGIONS_EXCEPT_SPECIFIED with an empty Regions list",
    "SPECIFIED_REGIONS with a blank Region name",
    "SPECIFIED_REGIONS with more Regions than the bound",
    "SPECIFIED_REGIONS with an over-long Region name",
];

/// The request body for each case named in [`INVALID_CREATE_CASES`].
fn invalid_create_body(case: &str) -> Value {
    match case {
        "unknown mode" => json!({ "RegionLinkingMode": "EVERY_REGION" }),
        "lowercase mode" => json!({ "RegionLinkingMode": "all_regions" }),
        "empty mode" => json!({ "RegionLinkingMode": "" }),
        "ALL_REGIONS with a Regions list" => {
            json!({ "RegionLinkingMode": "ALL_REGIONS", "Regions": ["us-east-1"] })
        }
        "NO_REGIONS with a Regions list" => {
            json!({ "RegionLinkingMode": "NO_REGIONS", "Regions": ["us-east-1"] })
        }
        "SPECIFIED_REGIONS without Regions" => json!({ "RegionLinkingMode": "SPECIFIED_REGIONS" }),
        "ALL_REGIONS_EXCEPT_SPECIFIED with an empty Regions list" => {
            json!({ "RegionLinkingMode": "ALL_REGIONS_EXCEPT_SPECIFIED", "Regions": [] })
        }
        "SPECIFIED_REGIONS with a blank Region name" => {
            json!({ "RegionLinkingMode": "SPECIFIED_REGIONS", "Regions": ["  "] })
        }
        // One past the 64-entry bound `crate::state::MAX_AGGREGATOR_REGIONS` keeps on
        // the stored list.
        "SPECIFIED_REGIONS with more Regions than the bound" => {
            let regions: Vec<String> = (0..65).map(|index| format!("eu-west-{index}")).collect();
            json!({ "RegionLinkingMode": "SPECIFIED_REGIONS", "Regions": regions })
        }
        // One past the 64-character bound `crate::state::MAX_REGION_NAME_LEN` keeps on
        // each entry.
        "SPECIFIED_REGIONS with an over-long Region name" => {
            json!({ "RegionLinkingMode": "SPECIFIED_REGIONS", "Regions": ["e".repeat(65)] })
        }
        other => panic!("no body for case {other:?}"),
    }
}

/// RULE: a create whose `RegionLinkingMode` is not a documented value, or whose
/// `Regions` list disagrees with the mode that reads it, is refused as a bad request
/// (HTTP 400) and stores nothing.
#[tokio::test]
async fn a_create_that_contradicts_the_model_is_refused() {
    for case in INVALID_CREATE_CASES {
        let harness = Harness::enabled_in(&[HOME_REGION]).await;
        let (status, error_type, payload) = harness
            .create_checked(HOME_REGION, invalid_create_body(case))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {payload}"
        );
        // Two rows break a MODELLED constraint (`RegionLinkingMode` and the
        // `Regions` member are `pattern \S`), which the boundary validator refuses as
        // `ValidationException` before the handler runs; every other row is this
        // handler's own refusal, which is `InvalidInputException`.
        let modelled_constraint = matches!(
            case,
            "empty mode" | "SPECIFIED_REGIONS with a blank Region name"
        );
        let expected_type = if modelled_constraint {
            "ValidationException"
        } else {
            "InvalidInputException"
        };
        assert!(
            error_type.contains(expected_type),
            "case {case:?}: expected error type {expected_type}, got {error_type:?} with {payload}"
        );
        let (_, listed) = harness.list(HOME_REGION, "").await;
        assert_eq!(
            listed_arns(&listed),
            Vec::<String>::new(),
            "case {case:?}: a refused create must store nothing, got {listed}"
        );
    }
}

/// RULE: cross-Region aggregation is one account-wide setting, so while an account
/// holds a finding aggregator every further create is refused as a bad request naming
/// the account — and a create succeeds again once that aggregator is deleted.
#[tokio::test]
async fn an_account_holds_at_most_one_finding_aggregator() {
    let harness = Harness::enabled_in(&[HOME_REGION]).await;
    let (status, created) = harness
        .create(HOME_REGION, json!({ "RegionLinkingMode": "NO_REGIONS" }))
        .await;
    assert_eq!(
        status, 200,
        "first CreateFindingAggregator: expected 200, got {status} with {created}"
    );
    let first_arn = created["FindingAggregatorArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A second create is refused however it is parameterised, and the stored
    // aggregator is untouched by the refusal.
    let second_attempts = [
        ("same mode", json!({ "RegionLinkingMode": "NO_REGIONS" })),
        ("other mode", json!({ "RegionLinkingMode": "ALL_REGIONS" })),
        (
            "mode with regions",
            json!({ "RegionLinkingMode": "SPECIFIED_REGIONS", "Regions": ["us-east-1"] }),
        ),
    ];
    for (case, body) in second_attempts {
        let (status, error_type, payload) = harness.create_checked(HOME_REGION, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second create expected 400, got {status} with {payload}"
        );
        // The CODE is the load-bearing part of the refusal: Security Hub CSPM answers
        // the duplicate create as a bad request, not as a quota failure.
        assert!(
            error_type.contains("InvalidInputException"),
            "case {case:?}: expected error type InvalidInputException, got {error_type:?} with {payload}"
        );
        let message = error_message(&payload);
        assert!(
            message.contains(ACCOUNT_ID) && message.contains("already exists"),
            "case {case:?}: the refusal must name the account that already aggregates, got {message:?}"
        );
        let (_, listed) = harness.list(HOME_REGION, "").await;
        assert_eq!(
            listed_arns(&listed),
            vec![first_arn.clone()],
            "case {case:?}: a refused create must leave the stored aggregator alone, got {listed}"
        );
    }

    let (status, payload) = harness.delete(HOME_REGION, &first_arn).await;
    assert_eq!(
        status, 200,
        "DeleteFindingAggregator: expected 200, got {status} with {payload}"
    );
    let (status, recreated) = harness
        .create(HOME_REGION, json!({ "RegionLinkingMode": "ALL_REGIONS" }))
        .await;
    assert_eq!(
        status, 200,
        "create after delete: expected 200, got {status} with {recreated}"
    );
    let second_arn = recreated["FindingAggregatorArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        second_arn, first_arn,
        "a new aggregator is a new resource: expected an ARN other than {first_arn:?}"
    );
    let (_, listed) = harness.list(HOME_REGION, "").await;
    assert_eq!(
        listed_arns(&listed),
        vec![second_arn],
        "the list must report the re-created aggregator, got {listed}"
    );
}

/// RULE: "You can run this operation from any Amazon Web Services Region"
/// (`smithy_query(query="operation", operation="ListFindingAggregators")`) — the
/// account's aggregator is reported in every Region where it is subscribed, always
/// naming its one home Region.
#[tokio::test]
async fn the_aggregator_is_reported_from_every_region() {
    let regions = [HOME_REGION, "us-east-1", "eu-west-1", "ap-southeast-2"];
    let harness = Harness::enabled_in(&regions).await;
    let (status, created) = harness
        .create(
            HOME_REGION,
            json!({ "RegionLinkingMode": "SPECIFIED_REGIONS", "Regions": ["us-east-1"] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateFindingAggregator: expected 200, got {status} with {created}"
    );
    let arn = created["FindingAggregatorArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for region in regions {
        let (status, listed) = harness.list(region, "").await;
        assert_eq!(
            status, 200,
            "case {region:?}: ListFindingAggregators expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed_arns(&listed),
            vec![arn.clone()],
            "case {region:?}: every Region reports the account's aggregator, got {listed}"
        );
        let (status, fetched) = harness.get(region, &arn).await;
        assert_eq!(
            status, 200,
            "case {region:?}: GetFindingAggregator expected 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["FindingAggregationRegion"].as_str(),
            Some(HOME_REGION),
            "case {region:?}: the home Region never changes with the caller's Region, got {fetched}"
        );
    }
}

/// RULE: `MaxResults` is modelled `@range(min: 1, max: 100)`, so a value inside the
/// range is served and a value outside it is refused; a `NextToken` this operation
/// did not issue is refused too.
#[tokio::test]
async fn list_paging_parameters_follow_the_model() {
    let harness = Harness::enabled_in(&[HOME_REGION]).await;
    let (status, created) = harness
        .create(HOME_REGION, json!({ "RegionLinkingMode": "ALL_REGIONS" }))
        .await;
    assert_eq!(
        status, 200,
        "CreateFindingAggregator: expected 200, got {status} with {created}"
    );
    let arn = created["FindingAggregatorArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Both published bounds, and the values immediately past each one.
    let max_results_cases = [("1", 200), ("2", 200), ("100", 200), ("0", 400), ("101", 400)];
    for (value, expected) in max_results_cases {
        let (status, payload) = harness
            .list(HOME_REGION, &format!("MaxResults={value}"))
            .await;
        assert_eq!(
            status, expected,
            "case MaxResults={value}: expected {expected}, got {status} with {payload}"
        );
        if expected == 200 {
            assert_eq!(
                listed_arns(&payload),
                vec![arn.clone()],
                "case MaxResults={value}: the single aggregator fits in the page, got {payload}"
            );
            assert!(
                payload["NextToken"].is_null(),
                "case MaxResults={value}: a complete page has no NextToken, got {payload}"
            );
        }
    }

    let bad_tokens = [
        ("not hex at all", "not-a-token".to_string()),
        (
            "a cursor minted for another list",
            hex::encode("securityhub:findings:0"),
        ),
        (
            "this list's own cursor past the end",
            hex::encode("securityhub:finding-aggregators:5"),
        ),
    ];
    for (case, token) in bad_tokens {
        let (status, payload) = harness
            .list(HOME_REGION, &format!("NextToken={token}"))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({token}): expected 400, got {status} with {payload}"
        );
    }
}

/// RULE: the `FindingAggregatorArn` selects which aggregator to read or delete, and
/// an account holds at most one — so any other ARN names a resource it does not have
/// (HTTP 404), and the refusal names the ARN that was supplied.
#[tokio::test]
async fn an_arn_that_names_no_aggregator_is_not_found() {
    let harness = Harness::enabled_in(&[HOME_REGION]).await;
    let (status, created) = harness
        .create(HOME_REGION, json!({ "RegionLinkingMode": "ALL_REGIONS" }))
        .await;
    assert_eq!(
        status, 200,
        "CreateFindingAggregator: expected 200, got {status} with {created}"
    );

    let unknown_arns = [
        "arn:aws:securityhub:us-west-2:000000000000:finding-aggregator/00000000-0000-4000-8000-000000000000",
        "arn:aws:securityhub:eu-west-1:000000000000:finding-aggregator/other",
        "arn:aws:securityhub:us-west-2:000000000000:hub/default",
        "not-an-arn",
    ];
    for arn in unknown_arns {
        let (status, payload) = harness.get(HOME_REGION, arn).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: GetFindingAggregator expected 404, got {status} with {payload}"
        );
        assert!(
            error_message(&payload).contains(arn),
            "case {arn:?}: the refusal must name the supplied ARN, got {:?}",
            error_message(&payload)
        );
        let (status, payload) = harness.delete(HOME_REGION, arn).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: DeleteFindingAggregator expected 404, got {status} with {payload}"
        );
    }

    // The stored aggregator survived every miss.
    let (_, listed) = harness.list(HOME_REGION, "").await;
    assert_eq!(
        listed_arns(&listed).len(),
        1,
        "a not-found read or delete must not touch the stored aggregator, got {listed}"
    );
}

/// RULE: every finding aggregator operation is answered for an account subscribed to
/// Security Hub CSPM in the Region the request is signed for; an unsubscribed account
/// is refused with `InvalidAccessException` (HTTP 401).
#[tokio::test]
async fn an_unsubscribed_account_is_refused() {
    let arn = format!(
        "arn:aws:securityhub:{HOME_REGION}:{ACCOUNT_ID}:finding-aggregator/00000000-0000-4000-8000-000000000000"
    );
    let probes: [(&str, &str, String, Option<Value>); 4] = [
        (
            "CreateFindingAggregator",
            "POST",
            "/findingAggregator/create".to_string(),
            Some(json!({ "RegionLinkingMode": "ALL_REGIONS" })),
        ),
        (
            "ListFindingAggregators",
            "GET",
            "/findingAggregator/list".to_string(),
            None,
        ),
        (
            "GetFindingAggregator",
            "GET",
            format!("/findingAggregator/get/{arn}"),
            None,
        ),
        (
            "DeleteFindingAggregator",
            "DELETE",
            format!("/findingAggregator/delete/{arn}"),
            None,
        ),
    ];

    let harness = Harness::new();
    for (case, method, path, body) in probes {
        let (status, payload) = harness.call(method, &path, HOME_REGION, body).await;
        assert_eq!(
            status, 401,
            "case {case:?} without a subscription: expected 401, got {status} with {payload}"
        );
    }
}
