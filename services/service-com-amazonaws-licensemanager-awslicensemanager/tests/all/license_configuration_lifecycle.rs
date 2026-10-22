// SPDX-License-Identifier: CC0-1.0

//! The self-managed license-configuration lifecycle over the wire:
//! `CreateLicenseConfiguration`, `GetLicenseConfiguration`,
//! `ListLicenseConfigurations`, `UpdateLicenseConfiguration`, `TagResource`,
//! `UntagResource`, `ListTagsForResource` and `DeleteLicenseConfiguration`, driven as
//! awsJson1_1 requests (`POST /`, `X-Amz-Target: AWSLicenseManager.<Operation>`)
//! through the same `MultiServiceRouter` the production emulator mounts.
//!
//! The rule under test: each of these operations answers from the ONE persisted
//! license-configuration record, so whatever a create or an update stored is what a
//! later Get, List or ListTagsForResource reports, and an ARN that was never stored
//! (or has since been deleted) is a modelled 400 `InvalidParameterValueException`.
//!
//! Every assertion is about STORED state: a response is checked against what an
//! earlier call persisted, never against a value this file hands the service and
//! reads straight back out of the same response.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name,
/// `com.amazonaws.licensemanager#AWSLicenseManager`
/// (`smithy_query(query="service_info")` reports `AWSLicenseManager`).
const TARGET_PREFIX: &str = "AWSLicenseManager";

/// The region the harness signs for unless a case picks another.
const DEFAULT_REGION: &str = "us-east-1";

/// The canonical account id every emulated ARN carries
/// (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// The dimensions a license configuration can count by.
///
/// The catalog is the modelled `LicenseCountingType` enum
/// (`smithy_query(query="shape",shape="LicenseCountingType")`): `vCPU`, `Instance`,
/// `Core`, `Socket`.
const LICENSE_COUNTING_TYPES: &[&str] = &["vCPU", "Instance", "Core", "Socket"];

/// One emulator: a single shared engine behind the router, exactly as
/// `core/router` assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The harness signs nothing, so authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_licensemanager_awslicensemanager::EmulatedService::new(engine),
    );
    router
}

/// Send one awsJson1_1 request signed for `region` and return its status and body.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/{region}/license-manager/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation}: response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Send one request in [`DEFAULT_REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in(router, DEFAULT_REGION, operation, body).await
}

/// The wire error code of a failure body: the last segment of awsJson's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|t| t.split('#').next_back().unwrap_or(t).to_string())
        .unwrap_or_else(|| format!("<no __type in {body}>"))
}

/// Whether `arn` is the ARN of a license configuration in `region`:
/// `arn:aws:license-manager:<region>:<account>:license-configuration:lic-<32 hex>`.
fn is_configuration_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:license-manager:{region}:{ACCOUNT_ID}:license-configuration:");
    arn.strip_prefix(&prefix)
        .and_then(|id| id.strip_prefix("lic-"))
        .is_some_and(|hex| hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Create one configuration and return the ARN the service minted, failing the case
/// when the call did not succeed.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = call(router, "CreateLicenseConfiguration", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateLicenseConfiguration to succeed, got {status} {response}"
    );
    response["LicenseConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: no LicenseConfigurationArn in {response}")
        })
        .to_string()
}

/// `GetLicenseConfiguration` for `arn`, failing the case when it did not succeed.
async fn get_ok(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, response) = call(
        router,
        "GetLicenseConfiguration",
        &json!({"LicenseConfigurationArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetLicenseConfiguration to succeed, got {status} {response}"
    );
    response
}

/// A `Tags` list from a response, as a key -> value map.
fn tag_map(case: &str, tags: &Value) -> BTreeMap<String, String> {
    tags.as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Tags list, got {tags}"))
        .iter()
        .map(|tag| {
            let key = tag["Key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag has no Key: {tag}"));
            let value = tag["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag has no Value: {tag}"));
            (key.to_string(), value.to_string())
        })
        .collect()
}

/// `pairs` as the modelled `TagList`.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// Every ARN `ListLicenseConfigurations` reports for `body`, paging with `max_results`
/// until the service stops handing out a `NextToken`.
///
/// Also asserts the pagination invariants as it walks: no page is larger than the
/// page size, only the last page carries no token, and the walk terminates.
async fn list_all_arns(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
    body: &Value,
) -> Vec<String> {
    let page_cap = max_results.map_or(usize::MAX, |n| n.max(1) as usize);
    let mut arns = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 0..64 {
        let mut request = body.clone();
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let (status, response) = call(router, "ListLicenseConfigurations", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListLicenseConfigurations to succeed, got {status} {response}"
        );
        let page_items = response["LicenseConfigurations"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: no LicenseConfigurations list in {response}")
            })
            .clone();
        assert!(
            page_items.len() <= page_cap,
            "case {case:?}: page {page} holds {} items, more than the requested {page_cap}",
            page_items.len()
        );
        for item in &page_items {
            arns.push(
                item["LicenseConfigurationArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a listed item has no ARN: {item}")
                    })
                    .to_string(),
            );
        }
        next_token = response
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match &next_token {
            None => return arns,
            Some(token) => assert!(
                !token.is_empty(),
                "case {case:?}: page {page} handed back an empty NextToken"
            ),
        }
    }
    panic!("case {case:?}: ListLicenseConfigurations never stopped handing out a NextToken");
}

/// One row of the create/get round-trip table: the request body, and the name the
/// failure messages use.
struct RoundTripCase {
    case: &'static str,
    body: Value,
}

/// One step of the tag lifecycle: the tagging call to make (none, for the state a
/// create left behind), and the tags the resource must carry afterwards.
struct TagStep {
    case: &'static str,
    operation: Option<&'static str>,
    body: Value,
    expected: Vec<(&'static str, &'static str)>,
}

/// One row of the list-scope or list-filter table: what the request scopes on, and
/// the ARNs the service must then report.
struct ListCase<T> {
    case: &'static str,
    scope: T,
    expected: BTreeSet<String>,
}

/// The table spans what the round-trip rule generalises over: every counting type in
/// the modelled catalog, a present and an absent value for each optional scalar, and
/// a list member empty, of one entry and of several.
fn round_trip_cases() -> Vec<RoundTripCase> {
    let mut cases = vec![
        RoundTripCase {
            case: "only the two required members",
            body: json!({"Name": "minimal", "LicenseCountingType": "Core"}),
        },
        RoundTripCase {
            case: "every member set",
            body: json!({
                "Name": "fully-populated",
                "LicenseCountingType": "vCPU",
                "Description": "a configuration carrying every member",
                "LicenseCount": 10,
                "LicenseCountHardLimit": true,
                "LicenseRules": ["#allowedTenancy=EC2-Default", "#minimumVcpus=2", "#maximumVcpus=16"],
                "DisassociateWhenNotFound": true,
                "LicenseExpiry": 1_800_000_000_i64,
                "Tags": tag_list(&[("env", "dev"), ("owner", "platform")]),
                "ProductInformationList": [{
                    "ResourceType": "SSM_MANAGED",
                    "ProductInformationFilterList": [{
                        "ProductInformationFilterName": "Application Name",
                        "ProductInformationFilterValue": ["SQL Server Enterprise"],
                        "ProductInformationFilterComparator": "EQUALS"
                    }]
                }]
            }),
        },
        RoundTripCase {
            case: "soft limit, one rule, one tag",
            body: json!({
                "Name": "soft-limited",
                "LicenseCountingType": "Socket",
                "LicenseCount": 0,
                "LicenseCountHardLimit": false,
                "LicenseRules": ["#licenseAffinityToHost=90"],
                "DisassociateWhenNotFound": false,
                "Tags": tag_list(&[("team", "licensing")])
            }),
        },
        RoundTripCase {
            case: "a very large license count",
            body: json!({
                "Name": "huge",
                "LicenseCountingType": "Instance",
                "LicenseCount": 9_007_199_254_740_991_i64
            }),
        },
    ];
    // Every entry of the modelled catalog is exercised, including the ones the issue
    // did not cite.
    for counting_type in LICENSE_COUNTING_TYPES {
        cases.push(RoundTripCase {
            case: "one counting type per catalog entry",
            body: json!({
                "Name": format!("by-{counting_type}"),
                "LicenseCountingType": counting_type,
                "LicenseCount": 3
            }),
        });
    }
    cases
}

#[tokio::test]
async fn create_persists_every_member_a_later_get_reports() {
    let router = emulator();
    for RoundTripCase { case, body } in round_trip_cases() {
        let arn = create_ok(&router, case, &body).await;
        assert!(
            is_configuration_arn(&arn, DEFAULT_REGION),
            "case {case:?}: expected a license-configuration ARN, got {arn:?}"
        );

        let stored = get_ok(&router, case, &arn).await;

        // The id is the ARN's last segment, so the two agree by construction.
        let id = stored["LicenseConfigurationId"].as_str().unwrap_or("");
        assert!(
            arn.ends_with(id) && id.starts_with("lic-"),
            "case {case:?}: expected the id to be the ARN's last segment, got id {id:?} for {arn:?}"
        );
        assert_eq!(
            stored["LicenseConfigurationArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: Get reported another ARN: {stored}"
        );

        // Every member the create supplied is reported back as stored.
        for member in [
            "Name",
            "LicenseCountingType",
            "Description",
            "LicenseCount",
            "LicenseCountHardLimit",
            "LicenseRules",
            "DisassociateWhenNotFound",
            "LicenseExpiry",
            "ProductInformationList",
        ] {
            if let Some(supplied) = body.get(member) {
                assert_eq!(
                    stored.get(member),
                    Some(supplied),
                    "case {case:?}: {member} was not stored as supplied, got {stored}"
                );
            }
        }
        // A list member the create omitted reads back as the empty list, not as a
        // missing member.
        for member in ["LicenseRules", "ProductInformationList"] {
            assert!(
                stored[member].is_array(),
                "case {case:?}: expected {member} to be a list, got {}",
                stored[member]
            );
        }

        // Tags are reported from the same ARN-keyed store the tagging operations use.
        let expected_tags = body
            .get("Tags")
            .map(|tags| tag_map(case, tags))
            .unwrap_or_default();
        assert_eq!(
            tag_map(case, &stored["Tags"]),
            expected_tags,
            "case {case:?}: Get reported other tags: {stored}"
        );

        // Computed members: a fresh configuration is AVAILABLE, owned by the
        // emulator account, and consumes nothing because nothing is associated.
        assert_eq!(
            stored["Status"].as_str(),
            Some("AVAILABLE"),
            "case {case:?}: expected Status AVAILABLE, got {}",
            stored["Status"]
        );
        assert_eq!(
            stored["OwnerAccountId"].as_str(),
            Some(ACCOUNT_ID),
            "case {case:?}: expected OwnerAccountId {ACCOUNT_ID}, got {}",
            stored["OwnerAccountId"]
        );
        assert_eq!(
            stored["ConsumedLicenses"].as_i64(),
            Some(0),
            "case {case:?}: expected ConsumedLicenses 0, got {}",
            stored["ConsumedLicenses"]
        );
        for member in ["ConsumedLicenseSummaryList", "ManagedResourceSummaryList"] {
            assert_eq!(
                stored[member].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: expected {member} to be empty, got {}",
                stored[member]
            );
        }
    }
}

#[tokio::test]
async fn each_create_mints_its_own_configuration() {
    let router = emulator();
    let mut arns = BTreeSet::new();
    let mut ids = BTreeSet::new();
    // The same request body twice over: two creates are two configurations even when
    // nothing distinguishes the requests.
    for round in 0..6 {
        let case = "repeated identical create";
        let arn = create_ok(
            &router,
            case,
            &json!({"Name": "same-name", "LicenseCountingType": "Core", "LicenseCount": 1}),
        )
        .await;
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: round {round} reused the ARN {arn:?}"
        );
        let stored = get_ok(&router, case, &arn).await;
        let id = stored["LicenseConfigurationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: round {round} reused the id {id:?}"
        );
    }
    let listed: BTreeSet<String> = list_all_arns(&router, "repeated identical create", None, &json!({}))
        .await
        .into_iter()
        .collect();
    assert_eq!(
        listed, arns,
        "every created configuration must be listed exactly once"
    );
}

#[tokio::test]
async fn a_configuration_arn_names_the_region_the_request_was_signed_for() {
    // The ARN's region field is read off the request's own SigV4 credential scope,
    // so it is not fixed to one region.
    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        let router = emulator();
        let (status, response) = call_in(
            &router,
            region,
            "CreateLicenseConfiguration",
            &json!({"Name": "regional", "LicenseCountingType": "Instance"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected the create to succeed, got {status} {response}"
        );
        let arn = response["LicenseConfigurationArn"].as_str().unwrap_or("");
        assert!(
            is_configuration_arn(arn, region),
            "case {region:?}: expected an ARN in {region}, got {arn:?}"
        );
    }
}

#[tokio::test]
async fn list_pages_cover_every_configuration_exactly_once() {
    let router = emulator();
    let mut created = BTreeSet::new();
    for n in 0..5 {
        created.insert(
            create_ok(
                &router,
                "pagination",
                &json!({"Name": format!("config-{n}"), "LicenseCountingType": "Core"}),
            )
            .await,
        );
    }

    // The rule generalises over the page size: a page smaller than, equal to and
    // larger than the store.
    for max_results in [Some(1), Some(2), Some(3), Some(5), Some(7), None] {
        let case = format!("MaxResults={max_results:?}");
        let walked = list_all_arns(&router, &case, max_results, &json!({})).await;
        assert_eq!(
            walked.len(),
            created.len(),
            "case {case:?}: the walk reported {} ARNs for {} configurations: {walked:?}",
            walked.len(),
            created.len()
        );
        assert_eq!(
            walked.iter().cloned().collect::<BTreeSet<String>>(),
            created,
            "case {case:?}: the walk did not cover every configuration exactly once: {walked:?}"
        );
    }
}

#[tokio::test]
async fn list_scopes_to_the_requested_arns() {
    let router = emulator();
    let mut arns = Vec::new();
    for n in 0..4 {
        arns.push(
            create_ok(
                &router,
                "ARN scope",
                &json!({"Name": format!("scoped-{n}"), "LicenseCountingType": "Socket"}),
            )
            .await,
        );
    }

    // The rule generalises over how many ARNs are named, and over an ARN that names
    // nothing.
    let unknown = format!(
        "arn:aws:license-manager:{DEFAULT_REGION}:{ACCOUNT_ID}:license-configuration:lic-{}",
        "f".repeat(32)
    );
    let rows = vec![
        ListCase {
            case: "one ARN",
            scope: vec![arns[1].clone()],
            expected: [arns[1].clone()].into(),
        },
        ListCase {
            case: "two ARNs",
            scope: vec![arns[0].clone(), arns[3].clone()],
            expected: [arns[0].clone(), arns[3].clone()].into(),
        },
        ListCase {
            case: "every ARN",
            scope: arns.clone(),
            expected: arns.iter().cloned().collect::<BTreeSet<String>>(),
        },
        ListCase {
            case: "an ARN that names nothing",
            scope: vec![unknown.clone()],
            expected: BTreeSet::new(),
        },
        ListCase {
            case: "a known and an unknown ARN",
            scope: vec![arns[2].clone(), unknown.clone()],
            expected: [arns[2].clone()].into(),
        },
    ];

    for ListCase {
        case,
        scope: requested,
        expected,
    } in rows
    {
        let walked = list_all_arns(
            &router,
            case,
            Some(1),
            &json!({"LicenseConfigurationArns": requested}),
        )
        .await;
        assert_eq!(
            walked.iter().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {case:?}: scoping to {requested:?} reported {walked:?}"
        );
    }
}

#[tokio::test]
async fn list_filters_scope_on_the_documented_names() {
    let router = emulator();
    // Four configurations that differ exactly along what the three documented
    // filters scope on.
    let hard_vcpu = create_ok(
        &router,
        "filters",
        &json!({"Name": "hard-vcpu", "LicenseCountingType": "vCPU", "LicenseCount": 4, "LicenseCountHardLimit": true}),
    )
    .await;
    let soft_vcpu = create_ok(
        &router,
        "filters",
        &json!({"Name": "soft-vcpu", "LicenseCountingType": "vCPU", "LicenseCount": 4, "LicenseCountHardLimit": false}),
    )
    .await;
    let hard_socket = create_ok(
        &router,
        "filters",
        &json!({"Name": "hard-socket", "LicenseCountingType": "Socket", "LicenseCountHardLimit": true}),
    )
    .await;
    let plain_core = create_ok(
        &router,
        "filters",
        &json!({"Name": "plain-core", "LicenseCountingType": "Core"}),
    )
    .await;

    // Names and values from the `Filters` member's own documentation.
    let rows = vec![
        ListCase {
            case: "licenseCountingType=vCPU",
            scope: json!([{"Name": "licenseCountingType", "Values": ["vCPU"]}]),
            expected: [hard_vcpu.clone(), soft_vcpu.clone()].into(),
        },
        ListCase {
            case: "licenseCountingType=Socket",
            scope: json!([{"Name": "licenseCountingType", "Values": ["Socket"]}]),
            expected: [hard_socket.clone()].into(),
        },
        ListCase {
            case: "licenseCountingType=Core",
            scope: json!([{"Name": "licenseCountingType", "Values": ["Core"]}]),
            expected: [plain_core.clone()].into(),
        },
        ListCase {
            case: "licenseCountingType=Instance matches nothing",
            scope: json!([{"Name": "licenseCountingType", "Values": ["Instance"]}]),
            expected: BTreeSet::new(),
        },
        ListCase {
            case: "enforceLicenseCount=true",
            scope: json!([{"Name": "enforceLicenseCount", "Values": ["true"]}]),
            expected: [hard_vcpu.clone(), hard_socket.clone()].into(),
        },
        ListCase {
            case: "enforceLicenseCount=false",
            scope: json!([{"Name": "enforceLicenseCount", "Values": ["false"]}]),
            expected: [soft_vcpu.clone(), plain_core.clone()].into(),
        },
        ListCase {
            case: "usagelimitExceeded=false",
            scope: json!([{"Name": "usagelimitExceeded", "Values": ["false"]}]),
            expected: [
                hard_vcpu.clone(),
                soft_vcpu.clone(),
                hard_socket.clone(),
                plain_core.clone(),
            ]
            .into(),
        },
        ListCase {
            case: "usagelimitExceeded=true",
            scope: json!([{"Name": "usagelimitExceeded", "Values": ["true"]}]),
            expected: BTreeSet::new(),
        },
        ListCase {
            case: "two filters together",
            scope: json!([
                {"Name": "licenseCountingType", "Values": ["vCPU"]},
                {"Name": "enforceLicenseCount", "Values": ["true"]}
            ]),
            expected: [hard_vcpu.clone()].into(),
        },
    ];

    for ListCase {
        case,
        scope: filters,
        expected,
    } in rows
    {
        let walked = list_all_arns(&router, case, Some(2), &json!({"Filters": filters})).await;
        assert_eq!(
            walked.iter().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {case:?}: the filter reported {walked:?}"
        );
    }
}

#[tokio::test]
async fn list_refuses_a_filter_or_token_it_did_not_issue() {
    let router = emulator();
    create_ok(
        &router,
        "refusals",
        &json!({"Name": "only", "LicenseCountingType": "Core"}),
    )
    .await;

    // Filter names are case-sensitive, per the member's own documentation, and only
    // one value may be given per filter.
    let rows: Vec<(&str, Value)> = vec![
        (
            "an unknown filter name",
            json!({"Filters": [{"Name": "licenseName", "Values": ["x"]}]}),
        ),
        (
            "a documented name in the wrong case",
            json!({"Filters": [{"Name": "LicenseCountingType", "Values": ["Core"]}]}),
        ),
        (
            "two values for one filter",
            json!({"Filters": [{"Name": "licenseCountingType", "Values": ["Core", "vCPU"]}]}),
        ),
        (
            "no value for one filter",
            json!({"Filters": [{"Name": "licenseCountingType", "Values": []}]}),
        ),
        (
            "a Boolean filter given something that is not a Boolean",
            json!({"Filters": [{"Name": "enforceLicenseCount", "Values": ["yes"]}]}),
        ),
        (
            "a Boolean filter given a Boolean in the wrong case",
            json!({"Filters": [{"Name": "usagelimitExceeded", "Values": ["TRUE"]}]}),
        ),
        (
            "a Boolean filter given a number",
            json!({"Filters": [{"Name": "enforceLicenseCount", "Values": ["1"]}]}),
        ),
        (
            "licenseCountingType given a dimension outside the documented set",
            json!({"Filters": [{"Name": "licenseCountingType", "Values": ["CPU"]}]}),
        ),
        (
            "licenseCountingType given a documented dimension in the wrong case",
            json!({"Filters": [{"Name": "licenseCountingType", "Values": ["core"]}]}),
        ),
        ("MaxResults of zero", json!({"MaxResults": 0})),
        ("a negative MaxResults", json!({"MaxResults": -1})),
        (
            "a MaxResults past the page ceiling",
            json!({"MaxResults": 101}),
        ),
        (
            "a NextToken this operation never issued",
            json!({"NextToken": "not-a-token"}),
        ),
    ];

    for (case, body) in rows {
        let (status, response) = call(&router, "ListLicenseConfigurations", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {response}"
        );
    }
}

#[tokio::test]
async fn a_page_token_resumes_only_the_query_it_was_issued_for() {
    let router = emulator();
    for n in 0..4 {
        create_ok(
            &router,
            "token scope",
            &json!({
                "Name": format!("scoped-{n}"),
                // Two of each dimension, so every scope below has more than one page.
                "LicenseCountingType": if n % 2 == 0 { "Core" } else { "vCPU" }
            }),
        )
        .await;
    }

    // A token is issued under one scope, then presented under another. The rule
    // generalises over what changed about the scope: the filters, the ARN list, or
    // the absence of either.
    let core_only = json!({"Filters": [{"Name": "licenseCountingType", "Values": ["Core"]}]});
    let (status, first_page) = call(
        &router,
        "ListLicenseConfigurations",
        &json!({"MaxResults": 1, "Filters": core_only["Filters"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the first page to succeed, got {status} {first_page}"
    );
    let token = first_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a NextToken on the first page: {first_page}"))
        .to_string();
    let resumed_arn = first_page["LicenseConfigurations"][0]["LicenseConfigurationArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Presented under the SAME scope it was issued for, it resumes.
    let (status, second_page) = call(
        &router,
        "ListLicenseConfigurations",
        &json!({"MaxResults": 1, "Filters": core_only["Filters"], "NextToken": token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"same scope\": expected the token to resume, got {status} {second_page}"
    );
    let next_arn = second_page["LicenseConfigurations"][0]["LicenseConfigurationArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        next_arn, resumed_arn,
        "case \"same scope\": the second page repeated the first page's configuration"
    );

    // Presented under any other scope, it is refused rather than silently resuming
    // a different walk at that ARN.
    let other_scopes: Vec<(&str, Value)> = vec![
        ("no filter at all", json!({})),
        (
            "another dimension",
            json!({"Filters": [{"Name": "licenseCountingType", "Values": ["vCPU"]}]}),
        ),
        (
            "an extra filter",
            json!({"Filters": [
                {"Name": "licenseCountingType", "Values": ["Core"]},
                {"Name": "enforceLicenseCount", "Values": ["false"]}
            ]}),
        ),
        (
            "an ARN scope added",
            json!({
                "Filters": [{"Name": "licenseCountingType", "Values": ["Core"]}],
                "LicenseConfigurationArns": [resumed_arn]
            }),
        ),
    ];
    for (case, mut body) in other_scopes {
        body["MaxResults"] = json!(1);
        body["NextToken"] = json!(token);
        let (status, response) = call(&router, "ListLicenseConfigurations", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {response}"
        );
    }
}

#[tokio::test]
async fn update_replaces_only_the_members_it_carries() {
    // Every member `UpdateLicenseConfiguration` accepts, one row each: the row's
    // member must change in the store and every other member must be untouched.
    let rows: Vec<(&str, Value)> = vec![
        ("Name", json!({"Name": "renamed"})),
        ("Description", json!({"Description": "a new description"})),
        ("LicenseCount", json!({"LicenseCount": 20})),
        ("LicenseCountHardLimit", json!({"LicenseCountHardLimit": true})),
        (
            "LicenseRules",
            json!({"LicenseRules": ["#licenseAffinityToHost=180"]}),
        ),
        ("DisassociateWhenNotFound", json!({"DisassociateWhenNotFound": true})),
        ("LicenseExpiry", json!({"LicenseExpiry": 1_900_000_000_i64})),
        ("Status", json!({"LicenseConfigurationStatus": "DISABLED"})),
        (
            "ProductInformationList",
            json!({"ProductInformationList": [{
                "ResourceType": "RDS",
                "ProductInformationFilterList": [{
                    "ProductInformationFilterName": "Engine Edition",
                    "ProductInformationFilterValue": ["sqlserver-ee"],
                    "ProductInformationFilterComparator": "EQUALS"
                }]
            }]}),
        ),
        ("nothing at all", json!({})),
    ];

    for (case, patch) in rows {
        let router = emulator();
        let arn = create_ok(
            &router,
            case,
            &json!({
                "Name": "before",
                "LicenseCountingType": "Core",
                "Description": "the original description",
                "LicenseCount": 5,
                "LicenseCountHardLimit": false,
                "LicenseRules": ["#minimumCores=1"],
                "DisassociateWhenNotFound": false,
                "LicenseExpiry": 1_700_000_000_i64,
                "Tags": tag_list(&[("env", "prod")])
            }),
        )
        .await;
        let before = get_ok(&router, case, &arn).await;

        let mut request = patch.clone();
        request["LicenseConfigurationArn"] = json!(arn);
        let (status, response) = call(&router, "UpdateLicenseConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to succeed, got {status} {response}"
        );

        let after = get_ok(&router, case, &arn).await;

        // What the patch named changed, read back out of the store.
        for (member, value) in patch.as_object().expect("the patch is an object") {
            let reported_as = if member == "LicenseConfigurationStatus" {
                "Status"
            } else {
                member.as_str()
            };
            assert_eq!(
                after.get(reported_as),
                Some(value),
                "case {case:?}: {reported_as} was not updated to {value}, got {after}"
            );
        }

        // Everything the patch did not name is exactly as it was.
        let patched = patch.as_object().expect("the patch is an object");
        // The output member each patched member is reported under.
        let patched_members: BTreeSet<&str> = patched
            .keys()
            .map(|member| {
                if member == "LicenseConfigurationStatus" {
                    "Status"
                } else {
                    member.as_str()
                }
            })
            .collect();
        let untouched: Vec<&str> = before
            .as_object()
            .expect("Get answers an object")
            .keys()
            .map(String::as_str)
            .filter(|member| !patched_members.contains(member))
            .collect();
        for member in untouched {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case:?}: {member} changed from {:?} to {:?}",
                before.get(member),
                after.get(member)
            );
        }
    }
}

#[tokio::test]
async fn tags_round_trip_through_the_tagging_operations() {
    let router = emulator();
    let case = "tag lifecycle";
    let arn = create_ok(
        &router,
        case,
        &json!({
            "Name": "tagged",
            "LicenseCountingType": "vCPU",
            "Tags": tag_list(&[("env", "dev")])
        }),
    )
    .await;

    // What each step must leave stored for the ARN, read back through
    // ListTagsForResource and GetLicenseConfiguration.
    let steps = vec![
        TagStep {
            case: "the create-time tag",
            operation: None,
            body: json!({}),
            expected: vec![("env", "dev")],
        },
        TagStep {
            case: "a second tag added",
            operation: Some("TagResource"),
            body: json!({"Tags": tag_list(&[("team", "plat")])}),
            expected: vec![("env", "dev"), ("team", "plat")],
        },
        TagStep {
            case: "two more tags at once",
            operation: Some("TagResource"),
            body: json!({"Tags": tag_list(&[("tier", "gold"), ("cost", "42")])}),
            expected: vec![
                ("env", "dev"),
                ("team", "plat"),
                ("tier", "gold"),
                ("cost", "42"),
            ],
        },
        TagStep {
            case: "an existing key given a new value",
            operation: Some("TagResource"),
            body: json!({"Tags": tag_list(&[("env", "staging")])}),
            expected: vec![
                ("env", "staging"),
                ("team", "plat"),
                ("tier", "gold"),
                ("cost", "42"),
            ],
        },
        TagStep {
            case: "one key removed",
            operation: Some("UntagResource"),
            body: json!({"TagKeys": ["tier"]}),
            expected: vec![("env", "staging"), ("team", "plat"), ("cost", "42")],
        },
        TagStep {
            case: "a key the resource does not carry removed",
            operation: Some("UntagResource"),
            body: json!({"TagKeys": ["tier"]}),
            expected: vec![("env", "staging"), ("team", "plat"), ("cost", "42")],
        },
        TagStep {
            case: "every remaining key removed",
            operation: Some("UntagResource"),
            body: json!({"TagKeys": ["env", "team", "cost"]}),
            expected: Vec::new(),
        },
    ];

    for TagStep {
        case: step,
        operation,
        mut body,
        expected,
    } in steps
    {
        let expected: BTreeMap<String, String> = expected
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        if let Some(operation) = operation {
            body["ResourceArn"] = json!(arn);
            let (status, response) = call(&router, operation, &body).await;
            assert_eq!(
                status, 200,
                "case {step:?}: expected {operation} to succeed, got {status} {response}"
            );
        }

        let (status, response) = call(
            &router,
            "ListTagsForResource",
            &json!({"ResourceArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {step:?}: expected ListTagsForResource to succeed, got {status} {response}"
        );
        assert_eq!(
            tag_map(step, &response["Tags"]),
            expected,
            "case {step:?}: ListTagsForResource reported {response}"
        );

        // The same tags reach the configuration's own view.
        let stored = get_ok(&router, step, &arn).await;
        assert_eq!(
            tag_map(step, &stored["Tags"]),
            expected,
            "case {step:?}: GetLicenseConfiguration reported other tags: {stored}"
        );
    }
}

#[tokio::test]
async fn delete_removes_the_configuration_from_every_later_read() {
    let router = emulator();
    let case = "delete";
    let kept = create_ok(
        &router,
        case,
        &json!({"Name": "kept", "LicenseCountingType": "Core"}),
    )
    .await;
    let doomed = create_ok(
        &router,
        case,
        &json!({"Name": "doomed", "LicenseCountingType": "Core", "Tags": tag_list(&[("a", "b")])}),
    )
    .await;

    let (status, response) = call(
        &router,
        "DeleteLicenseConfiguration",
        &json!({"LicenseConfigurationArn": doomed}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to succeed, got {status} {response}"
    );

    let listed: BTreeSet<String> = list_all_arns(&router, case, Some(1), &json!({}))
        .await
        .into_iter()
        .collect();
    assert_eq!(
        listed,
        [kept.clone()].into(),
        "case {case:?}: the deleted configuration is still listed"
    );

    // Every operation that reaches the record now refuses the ARN, including a
    // second delete — the tags went with it.
    for (operation, body) in [
        (
            "GetLicenseConfiguration",
            json!({"LicenseConfigurationArn": doomed}),
        ),
        (
            "DeleteLicenseConfiguration",
            json!({"LicenseConfigurationArn": doomed}),
        ),
        (
            "UpdateLicenseConfiguration",
            json!({"LicenseConfigurationArn": doomed, "Name": "zombie"}),
        ),
        ("ListTagsForResource", json!({"ResourceArn": doomed})),
    ] {
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to answer 400 after the delete, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException from {operation}, got {response}"
        );
    }
}

#[tokio::test]
async fn an_arn_the_store_does_not_hold_is_a_modelled_400() {
    let router = emulator();
    // The rule holds for any ARN the store does not hold, whatever its shape: a
    // well-formed one that was never minted, one for another region or account, one
    // naming another service's resource, and text that is no ARN at all.
    let unknown_arns = [
        format!(
            "arn:aws:license-manager:{DEFAULT_REGION}:{ACCOUNT_ID}:license-configuration:lic-{}",
            "0".repeat(32)
        ),
        format!(
            "arn:aws:license-manager:eu-west-1:{ACCOUNT_ID}:license-configuration:lic-{}",
            "a".repeat(32)
        ),
        format!(
            "arn:aws:license-manager:{DEFAULT_REGION}:123456789012:license-configuration:lic-{}",
            "b".repeat(32)
        ),
        format!("arn:aws:s3:::a-bucket/{ACCOUNT_ID}"),
        "lic-not-an-arn".to_string(),
        String::new(),
    ];

    for arn in unknown_arns {
        for (operation, body) in [
            (
                "GetLicenseConfiguration",
                json!({"LicenseConfigurationArn": arn}),
            ),
            (
                "UpdateLicenseConfiguration",
                json!({"LicenseConfigurationArn": arn, "LicenseCount": 1}),
            ),
            (
                "DeleteLicenseConfiguration",
                json!({"LicenseConfigurationArn": arn}),
            ),
            (
                "TagResource",
                json!({"ResourceArn": arn, "Tags": tag_list(&[("k", "v")])}),
            ),
            (
                "UntagResource",
                json!({"ResourceArn": arn, "TagKeys": ["k"]}),
            ),
            ("ListTagsForResource", json!({"ResourceArn": arn})),
        ] {
            let case = format!("{operation} with {arn:?}");
            let (status, response) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a 400, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidParameterValueException",
                "case {case:?}: expected InvalidParameterValueException, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn a_counting_type_or_status_outside_the_modelled_catalog_is_refused() {
    let router = emulator();
    // Anything outside the modelled `LicenseCountingType` catalog is refused with a
    // 400, and nothing is stored for it. The catalog is a modelled enum, so the
    // centralized constraint walk is what words the refusal.
    for counting_type in ["CPU", "vcpu", "INSTANCE", "Cores", ""] {
        let case = format!("LicenseCountingType={counting_type:?}");
        let (status, response) = call(
            &router,
            "CreateLicenseConfiguration",
            &json!({"Name": "rejected", "LicenseCountingType": counting_type}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
    assert!(
        list_all_arns(&router, "nothing stored", None, &json!({}))
            .await
            .is_empty(),
        "a refused create must store nothing"
    );

    // Same rule for the modelled `LicenseConfigurationStatus` catalog on update.
    let arn = create_ok(
        &router,
        "status catalog",
        &json!({"Name": "live", "LicenseCountingType": "Core"}),
    )
    .await;
    for status_value in ["ENABLED", "available", "DELETED", ""] {
        let case = format!("LicenseConfigurationStatus={status_value:?}");
        let (status, response) = call(
            &router,
            "UpdateLicenseConfiguration",
            &json!({"LicenseConfigurationArn": arn, "LicenseConfigurationStatus": status_value}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        // The refused update left the stored status alone.
        let stored = get_ok(&router, &case, &arn).await;
        assert_eq!(
            stored["Status"].as_str(),
            Some("AVAILABLE"),
            "case {case:?}: a refused update changed the stored Status: {stored}"
        );
    }
    // Both catalog values are accepted and persisted.
    for status_value in ["DISABLED", "AVAILABLE"] {
        let case = format!("LicenseConfigurationStatus={status_value:?}");
        let (status, response) = call(
            &router,
            "UpdateLicenseConfiguration",
            &json!({"LicenseConfigurationArn": arn, "LicenseConfigurationStatus": status_value}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to succeed, got {status} {response}"
        );
        let stored = get_ok(&router, &case, &arn).await;
        assert_eq!(
            stored["Status"].as_str(),
            Some(status_value),
            "case {case:?}: Status was not persisted, got {stored}"
        );
    }
}

#[tokio::test]
async fn a_collection_past_the_store_bound_is_refused() {
    let router = emulator();
    let over = service_com_amazonaws_licensemanager_awslicensemanager::state::MAX_TAGS + 1;
    let pairs: Vec<(String, String)> = (0..over)
        .map(|n| (format!("key-{n}"), format!("value-{n}")))
        .collect();
    let tags = Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    );

    let case = "more tags than the store accepts";
    let (status, response) = call(
        &router,
        "CreateLicenseConfiguration",
        &json!({"Name": "over-tagged", "LicenseCountingType": "Core", "Tags": tags}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidParameterValueException",
        "case {case:?}: expected InvalidParameterValueException, got {response}"
    );
    assert!(
        list_all_arns(&router, case, None, &json!({})).await.is_empty(),
        "case {case:?}: a refused create must store nothing"
    );

    // The same rule on TagResource: a merge that would take the resource past the
    // cap changes nothing, so the tags it already carried are still exactly there.
    let arn = create_ok(
        &router,
        case,
        &json!({"Name": "tagged", "LicenseCountingType": "Core", "Tags": tag_list(&[("env", "dev")])}),
    )
    .await;
    let bulk = Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    );
    let (status, response) = call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": bulk}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected TagResource to answer 400, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidParameterValueException",
        "case {case:?}: expected InvalidParameterValueException from TagResource, got {response}"
    );
    let (_, tags_after) = call(
        &router,
        "ListTagsForResource",
        &json!({"ResourceArn": arn}),
    )
    .await;
    assert_eq!(
        tag_map(case, &tags_after["Tags"]),
        [("env".to_string(), "dev".to_string())].into(),
        "case {case:?}: a refused TagResource changed the stored tags: {tags_after}"
    );
}

#[tokio::test]
async fn a_member_outside_the_store_rule_is_refused_on_create_and_on_update() {
    let router = emulator();
    let arn = create_ok(
        &router,
        "member rules",
        &json!({"Name": "baseline", "LicenseCountingType": "Core", "LicenseCount": 5}),
    )
    .await;
    let before = get_ok(&router, "member rules", &arn).await;

    // The rule generalises over the member and over how it is out of bounds: a
    // count or an instant below zero, and a string past the length the store takes.
    let over_long = "x".repeat(
        service_com_amazonaws_licensemanager_awslicensemanager::state::MAX_TEXT_LEN + 1,
    );
    let rows: Vec<(&str, Value)> = vec![
        ("a negative LicenseCount", json!({"LicenseCount": -1})),
        (
            "a large negative LicenseCount",
            json!({"LicenseCount": -9_007_199_254_740_991_i64}),
        ),
        ("a negative LicenseExpiry", json!({"LicenseExpiry": -1})),
        ("an over-long Name", json!({"Name": over_long.clone()})),
        (
            "an over-long Description",
            json!({"Description": over_long.clone()}),
        ),
        (
            "an over-long LicenseRules entry",
            json!({"LicenseRules": [over_long.clone()]}),
        ),
        (
            "an over-long ProductInformationFilterName",
            json!({"ProductInformationList": [{
                "ResourceType": "RDS",
                "ProductInformationFilterList": [{
                    "ProductInformationFilterName": over_long.clone(),
                    "ProductInformationFilterComparator": "EQUALS"
                }]
            }]}),
        ),
        (
            "an over-long ProductInformationFilterValue entry",
            json!({"ProductInformationList": [{
                "ResourceType": "RDS",
                "ProductInformationFilterList": [{
                    "ProductInformationFilterName": "Engine Edition",
                    "ProductInformationFilterValue": [over_long.clone()],
                    "ProductInformationFilterComparator": "EQUALS"
                }]
            }]}),
        ),
    ];

    for (case, member) in rows {
        // Create and update persist the same members into the same record, so both
        // apply the same rule.
        let mut create = member.clone();
        create["Name"] = create
            .get("Name")
            .cloned()
            .unwrap_or_else(|| json!("rejected"));
        create["LicenseCountingType"] = json!("Core");
        let (status, response) = call(&router, "CreateLicenseConfiguration", &create).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to answer 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException from the create, got {response}"
        );

        let mut update = member.clone();
        update["LicenseConfigurationArn"] = json!(arn);
        let (status, response) = call(&router, "UpdateLicenseConfiguration", &update).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to answer 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException from the update, got {response}"
        );
    }

    // Nothing the refusals touched reached the store.
    assert_eq!(
        get_ok(&router, "member rules", &arn).await,
        before,
        "a refused create or update must leave the stored configuration unchanged"
    );
    assert_eq!(
        list_all_arns(&router, "member rules", None, &json!({})).await,
        vec![arn],
        "a refused create must store nothing"
    );
}

#[tokio::test]
async fn a_create_past_the_store_cap_is_refused_and_the_store_stays_at_its_cap() {
    let router = emulator();
    let cap = service_com_amazonaws_licensemanager_awslicensemanager::state::MAX_LICENSE_CONFIGURATIONS;
    let case = "the store at its cap";

    // Fill the store to exactly its cap: every one of these must succeed.
    for n in 0..cap {
        let (status, response) = call(
            &router,
            "CreateLicenseConfiguration",
            &json!({"Name": format!("filler-{n}"), "LicenseCountingType": "Core"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: create {n} of {cap} must succeed, got {status} {response}"
        );
    }

    // The next few are refused with the modelled resource-limit error, and the
    // store neither grows nor loses anything.
    for attempt in 0..3 {
        let (status, response) = call(
            &router,
            "CreateLicenseConfiguration",
            &json!({"Name": format!("over-{attempt}"), "LicenseCountingType": "Core"}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: attempt {attempt} past the cap must answer 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceLimitExceededException",
            "case {case:?}: attempt {attempt} expected ResourceLimitExceededException, got {response}"
        );
    }

    let listed = list_all_arns(&router, case, Some(100), &json!({})).await;
    assert_eq!(
        listed.len(),
        cap,
        "case {case:?}: the store holds {} configurations, not the {cap} it was filled to",
        listed.len()
    );
    assert_eq!(
        listed.iter().collect::<BTreeSet<&String>>().len(),
        cap,
        "case {case:?}: the store reported a duplicate ARN"
    );

    // A delete frees exactly one slot, and the next create takes it.
    let (status, response) = call(
        &router,
        "DeleteLicenseConfiguration",
        &json!({"LicenseConfigurationArn": listed[0]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to succeed, got {status} {response}"
    );
    let (status, response) = call(
        &router,
        "CreateLicenseConfiguration",
        &json!({"Name": "after-delete", "LicenseCountingType": "Core"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a create into the freed slot must succeed, got {status} {response}"
    );
    assert_eq!(
        list_all_arns(&router, case, Some(100), &json!({})).await.len(),
        cap,
        "case {case:?}: the store must be back at its cap"
    );
}
