// SPDX-License-Identifier: CC0-1.0

//! The dashboard lifecycle, end to end over the wire: `CreateDashboard`,
//! `GetDashboard`, `ListDashboards`, `UpdateDashboard`, `DeleteDashboard`.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSBCMDashboardsService.<Op>`, awsJson1_0), so what these tests
//! assert is what an SDK client sees: the response is built from the persisted
//! record, not from the request.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::MAX_DASHBOARDS;
use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService;

/// A router serving this one service over a fresh engine, as `build_router` does
/// in production. Shared with the sibling audit suite.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return `(http status, parsed body)`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        // The routing signal an SDK request carries: the SigV4 credential scope names
        // this service's signing name (`bcm-dashboards`). Request authentication is off
        // for this router, so the signature itself is never checked.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20250818/us-east-1/bcm-dashboards/aws4_request, \
             SignedHeaders=host;x-amz-date;x-amz-target, Signature=0000",
        )
        .header("host", "bcm-dashboards.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSBCMDashboardsService.{operation}"))
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
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Succeed-or-explain: the call must answer 200.
pub(crate) async fn ok_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} expected 200, got {status}: {parsed}");
    parsed
}

/// The access key id the requests [`call`] builds present, which an audit record must
/// attribute the call to.
pub(crate) const PRESENTED_AKID: &str = "AKIDEXAMPLE";

/// The `Host` the requests [`call`] builds address, which an audit record reports as the
/// request's source.
pub(crate) const REQUEST_SOURCE: &str = "bcm-dashboards.us-east-1.amazonaws.com";

/// The wire error code (`__type`) of an error body.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// `DashboardArn`'s modelled `@pattern`, checked by hand because this crate has no
/// regex dependency. Source: the `DashboardArn` member pattern reported by
/// `smithy_query(query="operation",operation="CreateDashboard")` and OPERATIONS.md —
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$`, length
/// 20..2048.
fn is_dashboard_arn(arn: &str) -> bool {
    fn lower_or_digit_or_dash(s: &str) -> bool {
        s.chars()
            .all(|c| c == '-' || c.is_ascii_lowercase() || c.is_ascii_digit())
    }
    let Some(rest) = arn.strip_prefix("arn:aws") else {
        return false;
    };
    let Some((partition_suffix, rest)) = rest.split_once(':') else {
        return false;
    };
    if !lower_or_digit_or_dash(partition_suffix) {
        return false;
    }
    let Some(rest) = rest.strip_prefix("bcm-dashboards::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some(id) = resource.strip_prefix("dashboard/") else {
        return false;
    };
    let id_ok = id == "*" || (!id.is_empty() && lower_or_digit_or_dash(id));
    id_ok && (20..=2048).contains(&arn.len())
}

/// `WidgetId`'s modelled `@pattern` `^[0-9a-f]{32}$` (source: `smithy_query
/// shape=Widget`).
fn is_widget_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// A widget config whose data comes from a cost-and-usage query and is displayed as a
/// graph. `metric` is a `MetricName` value and `visual` a `VisualType` value (sources:
/// the `MetricName` / `VisualType` / `Granularity` enums in the model).
pub(crate) fn graph_config(metric: &str, visual: &str, granularity: &str) -> Value {
    json!({
        "queryParameters": {
            "costAndUsage": {
                "metrics": [metric],
                "timeRange": {
                    "startTime": {"type": "RELATIVE", "value": "-P3M"},
                    "endTime": {"type": "RELATIVE", "value": "P0D"}
                },
                "granularity": granularity
            }
        },
        "displayConfig": {"graph": {metric: {"visualType": visual}}}
    })
}

/// A widget config displayed as a table, over an absolute time range.
fn table_config(metric: &str) -> Value {
    json!({
        "queryParameters": {
            "costAndUsage": {
                "metrics": [metric],
                "timeRange": {
                    "startTime": {"type": "ABSOLUTE", "value": "2025-01-01"},
                    "endTime": {"type": "ABSOLUTE", "value": "2025-07-01"}
                },
                "granularity": "DAILY"
            }
        },
        "displayConfig": {"table": {}}
    })
}

/// What a widget must look like once stored, for one request widget.
struct WidgetExpectation {
    /// `Some(id)` when the request supplied one (it must survive), `None` when the
    /// service must mint one.
    id: Option<&'static str>,
    width: i64,
    height: i64,
    horizontal_offset: i64,
}

struct CreateCase {
    label: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    widgets: Vec<Value>,
    expected: Vec<WidgetExpectation>,
    tags: Option<Value>,
}

fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "no widgets, no description, no tags",
            name: "Em",
            description: None,
            widgets: vec![],
            expected: vec![],
            tags: None,
        },
        CreateCase {
            label: "one widget taking every size default",
            name: "Monthly Spend Overview",
            description: Some("Spend, by service."),
            widgets: vec![json!({
                "title": "Spend",
                "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]
            })],
            // Widget defaults from the model: width 4, height 7, horizontalOffset 0.
            expected: vec![WidgetExpectation {
                id: None,
                width: 4,
                height: 7,
                horizontal_offset: 0,
            }],
            tags: Some(json!([{"key": "team", "value": "finops"}])),
        },
        CreateCase {
            label: "client-supplied widget id, explicit sizes, two configs",
            name: "Coverage-and-Utilization",
            description: Some("Two configs on one widget."),
            widgets: vec![json!({
                "id": "0123456789abcdef0123456789abcdef",
                "title": "Coverage",
                "description": "Savings plans coverage",
                "width": 6,
                "height": 10,
                "horizontalOffset": 3,
                "configs": [
                    graph_config("SpendCoveredBySavingsPlans", "STACK", "DAILY"),
                    table_config("AmortizedCost")
                ]
            })],
            expected: vec![WidgetExpectation {
                id: Some("0123456789abcdef0123456789abcdef"),
                width: 6,
                height: 10,
                horizontal_offset: 3,
            }],
            tags: Some(json!([
                {"key": "team", "value": "finops"},
                {"key": "empty-value-is-legal", "value": ""}
            ])),
        },
        CreateCase {
            label: "three widgets: supplied id, minted id, minimum sizes",
            name: "A_b-C 1",
            description: None,
            widgets: vec![
                json!({
                    "id": "ffffffffffffffffffffffffffffffff",
                    "title": "Hourly",
                    "width": 2,
                    "height": 4,
                    "configs": [graph_config("Cost", "BAR", "HOURLY")]
                }),
                json!({
                    "title": "Usage-Quantity",
                    "configs": [table_config("UsageQuantity")]
                }),
                json!({
                    "title": "Net_Amortized",
                    "horizontalOffset": 5,
                    "configs": [graph_config("NetAmortizedCost", "LINE", "MONTHLY")]
                }),
            ],
            expected: vec![
                WidgetExpectation {
                    id: Some("ffffffffffffffffffffffffffffffff"),
                    width: 2,
                    height: 4,
                    horizontal_offset: 0,
                },
                WidgetExpectation {
                    id: None,
                    width: 4,
                    height: 7,
                    horizontal_offset: 0,
                },
                WidgetExpectation {
                    id: None,
                    width: 4,
                    height: 7,
                    horizontal_offset: 5,
                },
            ],
            tags: None,
        },
    ]
}

fn create_request(case: &CreateCase) -> Value {
    let mut request = json!({"name": case.name, "widgets": case.widgets});
    if let Some(description) = case.description {
        request["description"] = json!(description);
    }
    if let Some(tags) = &case.tags {
        request["resourceTags"] = tags.clone();
    }
    request
}

/// Assert `widgets` (as read back) matches `case`, and return the ids it carries.
fn assert_widgets(case: &CreateCase, widgets: &Value, request_widgets: &[Value]) -> Vec<String> {
    let widgets = widgets
        .as_array()
        .unwrap_or_else(|| panic!("case {}: widgets is not an array: {widgets}", case.label));
    assert_eq!(
        widgets.len(),
        request_widgets.len(),
        "case {}: expected {} widgets, got {widgets:?}",
        case.label,
        request_widgets.len()
    );

    let mut ids = Vec::new();
    for (index, (stored, expectation)) in widgets.iter().zip(case.expected.iter()).enumerate() {
        let requested = &request_widgets[index];
        let label = format!("case {} widget {index}", case.label);

        let id = stored
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{label}: no id was stored: {stored}"));
        assert!(
            is_widget_id(id),
            "{label}: expected a 32-hex-character widget id, got {id:?}"
        );
        if let Some(supplied) = expectation.id {
            assert_eq!(
                id, supplied,
                "{label}: the supplied widget id must survive, got {id:?}"
            );
        }
        ids.push(id.to_string());

        assert_eq!(
            stored.get("title"),
            requested.get("title"),
            "{label}: title must be the stored one, got {stored}"
        );
        assert_eq!(
            stored.get("description"),
            requested.get("description"),
            "{label}: description must be the stored one, got {stored}"
        );
        assert_eq!(
            stored.get("width").and_then(Value::as_i64),
            Some(expectation.width),
            "{label}: expected width {}, got {stored}",
            expectation.width
        );
        assert_eq!(
            stored.get("height").and_then(Value::as_i64),
            Some(expectation.height),
            "{label}: expected height {}, got {stored}",
            expectation.height
        );
        assert_eq!(
            stored.get("horizontalOffset").and_then(Value::as_i64),
            Some(expectation.horizontal_offset),
            "{label}: expected horizontalOffset {}, got {stored}",
            expectation.horizontal_offset
        );
        assert_eq!(
            stored.get("configs"),
            requested.get("configs"),
            "{label}: configs must be stored verbatim, got {stored}"
        );
    }
    ids
}

pub(crate) fn timestamp(body: &Value, member: &str, label: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("{label}: {member} is not an epoch-seconds number: {body}"))
}

/// CreateDashboard mints and stores a dashboard, and GetDashboard reads that record
/// back — across a table of dashboards differing in widget count, widget ids,
/// widget sizes, config count, display type, description and tags.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_dashboard() {
    let router = router();
    let mut seen_arns: BTreeSet<String> = BTreeSet::new();
    let mut seen_widget_ids: BTreeSet<String> = BTreeSet::new();

    for case in create_cases() {
        let request = create_request(&case);
        let created = ok_call(&router, "CreateDashboard", &request).await;
        let arn = created
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {}: CreateDashboard returned no arn: {created}", case.label))
            .to_string();
        assert!(
            is_dashboard_arn(&arn),
            "case {}: {arn:?} is not a DashboardArn",
            case.label
        );
        assert!(
            seen_arns.insert(arn.clone()),
            "case {}: CreateDashboard reused the arn {arn}",
            case.label
        );

        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            fetched.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {}: GetDashboard must report the stored arn: {fetched}",
            case.label
        );
        assert_eq!(
            fetched.get("name").and_then(Value::as_str),
            Some(case.name),
            "case {}: GetDashboard must report the stored name: {fetched}",
            case.label
        );
        assert_eq!(
            fetched.get("description").and_then(Value::as_str),
            case.description,
            "case {}: GetDashboard must report the stored description: {fetched}",
            case.label
        );
        assert_eq!(
            fetched.get("type").and_then(Value::as_str),
            Some("CUSTOM"),
            "case {}: a dashboard created through this API is CUSTOM: {fetched}",
            case.label
        );

        let created_at = timestamp(&fetched, "createdAt", case.label);
        let updated_at = timestamp(&fetched, "updatedAt", case.label);
        assert_eq!(
            created_at, updated_at,
            "case {}: a dashboard that was never updated has createdAt == updatedAt",
            case.label
        );
        assert!(
            created_at > 1_700_000_000.0,
            "case {}: createdAt {created_at} is not a plausible epoch-seconds instant",
            case.label
        );

        let widgets = fetched
            .get("widgets")
            .unwrap_or_else(|| panic!("case {}: GetDashboard returned no widgets: {fetched}", case.label));
        let ids = assert_widgets(&case, widgets, &case.widgets);
        for id in ids {
            assert!(
                seen_widget_ids.insert(id.clone()),
                "case {}: widget id {id} was minted twice",
                case.label
            );
        }

        // A read must not change what the next caller sees.
        let again = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            again, fetched,
            "case {}: GetDashboard is a read: two calls must answer identically",
            case.label
        );
    }
}

/// The name is not the identity: dashboards created with the same name are distinct
/// resources with distinct ARNs, each readable on its own.
#[tokio::test]
async fn same_name_creates_distinct_dashboards() {
    let router = router();
    let request = json!({
        "name": "Duplicate Name",
        "widgets": [{"title": "Spend", "configs": [table_config("BlendedCost")]}]
    });

    let first = ok_call(&router, "CreateDashboard", &request).await;
    let second = ok_call(&router, "CreateDashboard", &request).await;
    let first_arn = first["arn"].as_str().expect("an arn").to_string();
    let second_arn = second["arn"].as_str().expect("an arn").to_string();
    assert_ne!(
        first_arn, second_arn,
        "two CreateDashboard calls with one name must mint different arns"
    );

    for arn in [&first_arn, &second_arn] {
        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            fetched["arn"].as_str(),
            Some(arn.as_str()),
            "case {arn}: each dashboard must be readable under its own arn: {fetched}"
        );
    }
}

/// Widget `width` and `height` are refused outside their modelled ranges and accepted
/// on each published bound. Source: `smithy_query shape=Widget` — width range 2..6,
/// height range 4..10.
#[tokio::test]
async fn widget_sizes_are_held_to_their_modelled_range() {
    let router = router();
    // (label, width, height, accepted)
    let cases: &[(&str, i64, i64, bool)] = &[
        ("width at min", 2, 7, true),
        ("width at max", 6, 7, true),
        ("width below min", 1, 7, false),
        ("width above max", 7, 7, false),
        ("height at min", 4, 4, true),
        ("height at max", 4, 10, true),
        ("height below min", 4, 3, false),
        ("height above max", 4, 11, false),
    ];

    for (label, width, height, accepted) in cases {
        let request = json!({
            "name": "Sized Widgets",
            "widgets": [{
                "title": "Spend",
                "width": width,
                "height": height,
                "configs": [table_config("Cost")]
            }]
        });
        let (status, body) = call(&router, "CreateDashboard", &request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {label}: width {width}/height {height} is inside the modelled range, got {status}: {body}"
            );
            let arn = body["arn"].as_str().expect("an arn");
            let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
            assert_eq!(
                fetched["widgets"][0]["width"].as_i64(),
                Some(*width),
                "case {label}: the stored width must be the supplied one: {fetched}"
            );
            assert_eq!(
                fetched["widgets"][0]["height"].as_i64(),
                Some(*height),
                "case {label}: the stored height must be the supplied one: {fetched}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label}: width {width}/height {height} is outside the modelled range, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {label}: expected ValidationException, got {}",
                error_code(&body)
            );
        }
    }
}

/// ListDashboards pages for real: every page holds at most `maxResults` references
/// projected from the stored records, `nextToken` appears exactly when dashboards
/// remain, and resuming from it walks the whole set exactly once — for every page
/// size, and for the default page size when `maxResults` is absent.
#[tokio::test]
async fn list_dashboards_pages_over_the_stored_dashboards() {
    let router = router();
    let names = [
        "Alpha Costs",
        "Bravo Costs",
        "Charlie Costs",
        "Delta Costs",
        "Echo Costs",
    ];
    let mut expected: Vec<String> = Vec::new();
    for name in names {
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": name,
                "widgets": [{"title": "Spend", "configs": [table_config("Cost")]}]
            }),
        )
        .await;
        expected.push(created["arn"].as_str().expect("an arn").to_string());
    }
    // The store is keyed by ARN, so the listing order is ARN order.
    expected.sort();

    for page_size in [1_usize, 2, 3, 5, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let page = ok_call(&router, "ListDashboards", &request).await;
            pages += 1;
            let items = page["dashboards"]
                .as_array()
                .unwrap_or_else(|| panic!("case page size {page_size}: no dashboards: {page}"))
                .clone();
            assert!(
                items.len() <= page_size,
                "case page size {page_size}: a page returned {} items",
                items.len()
            );
            for item in &items {
                let arn = item["arn"].as_str().expect("an arn").to_string();
                // Each reference is the projection of the stored record.
                let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
                for member in ["arn", "name", "type", "createdAt", "updatedAt"] {
                    assert_eq!(
                        item.get(member),
                        fetched.get(member),
                        "case page size {page_size}: reference member {member} must match the stored record: {item} vs {fetched}"
                    );
                }
                collected.push(arn);
            }
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= names.len(),
                "case page size {page_size}: pagination did not terminate"
            );
        }

        assert_eq!(
            collected, expected,
            "case page size {page_size}: paging must walk every dashboard exactly once, in order"
        );
        let want_pages = names.len().div_ceil(page_size);
        assert_eq!(
            pages, want_pages,
            "case page size {page_size}: expected {want_pages} pages, walked {pages}"
        );
    }

    // `maxResults` absent: the modelled default (20) covers all five, so no cursor.
    let defaulted = ok_call(&router, "ListDashboards", &json!({})).await;
    let items = defaulted["dashboards"].as_array().expect("dashboards");
    assert_eq!(
        items.len(),
        names.len(),
        "case default page size: expected every dashboard on one page: {defaulted}"
    );
    assert!(
        defaulted.get("nextToken").is_none(),
        "case default page size: the last page carries no nextToken: {defaulted}"
    );
}

/// `maxResults` outside its modelled 1..100 range, and a `nextToken` this service
/// never minted, are both refused with `ValidationException`.
#[tokio::test]
async fn list_dashboards_refuses_a_bad_page_request() {
    let router = router();
    ok_call(
        &router,
        "CreateDashboard",
        &json!({"name": "Only One", "widgets": []}),
    )
    .await;

    // Source: `maxResults` range 1..100 — each published bound, and one past it.
    let max_results_cases: &[(&str, i64, bool)] = &[
        ("at min", 1, true),
        ("at max", 100, true),
        ("below min", 0, false),
        ("above max", 101, false),
        ("negative", -1, false),
    ];
    for (label, max_results, accepted) in max_results_cases {
        let (status, body) = call(
            &router,
            "ListDashboards",
            &json!({"maxResults": max_results}),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case maxResults {label}: {max_results} is inside 1..100, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case maxResults {label}: {max_results} is outside 1..100, got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case maxResults {label}: expected ValidationException, got {}",
                error_code(&body)
            );
        }
    }

    for token in [
        "not-a-token",
        "v1:",
        "v1:zzzz",
        "v1:6e6f",
        "deadbeef",
        "{\"page\": 2}",
    ] {
        let (status, body) = call(&router, "ListDashboards", &json!({"nextToken": token})).await;
        assert_eq!(
            status, 400,
            "case token {token:?}: an uninterpretable nextToken must be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case token {token:?}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}

/// UpdateDashboard mutates the stored record in place: the members it carries are
/// replaced, absent `widgets` leave the stored widgets alone, `updatedAt` moves
/// forward and `createdAt` does not.
#[tokio::test]
async fn update_dashboard_mutates_the_stored_record() {
    let original_widgets = json!([
        {"title": "Spend", "configs": [table_config("Cost")]},
        {"title": "Coverage", "configs": [graph_config("UnblendedCost", "BAR", "DAILY")]}
    ]);

    // (label, the update's extra members beyond `arn`+`name`, expected widget titles)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "name only, widgets absent",
            json!({}),
            vec!["Spend", "Coverage"],
        ),
        (
            "name and description",
            json!({"description": "Now with a description"}),
            vec!["Spend", "Coverage"],
        ),
        (
            "widgets replaced wholesale by one",
            json!({"widgets": [{"title": "Only-One", "configs": [table_config("AmortizedCost")]}]}),
            vec!["Only-One"],
        ),
        (
            "widgets replaced by none",
            json!({"widgets": []}),
            vec![],
        ),
        (
            "widgets replaced by three",
            json!({"widgets": [
                {"title": "One", "width": 2, "configs": [table_config("Cost")]},
                {"title": "Two", "height": 10, "configs": [table_config("Hour")]},
                {"title": "Three", "configs": [graph_config("Unit", "STACK", "HOURLY")]}
            ]}),
            vec!["One", "Two", "Three"],
        ),
    ];

    for (label, extra, expected_titles) in cases {
        let router = router();
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": "Before Update",
                "description": "The original description",
                "widgets": original_widgets
            }),
        )
        .await;
        let arn = created["arn"].as_str().expect("an arn").to_string();
        let before = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;

        // The clock must have moved on, so `updatedAt` is comparable.
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut request = json!({"arn": arn, "name": "After Update"});
        if let Value::Object(members) = &extra {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        let updated = ok_call(&router, "UpdateDashboard", &request).await;
        assert_eq!(
            updated["arn"].as_str(),
            Some(arn.as_str()),
            "case {label}: UpdateDashboard must answer the arn it updated: {updated}"
        );

        let after = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            after["name"].as_str(),
            Some("After Update"),
            "case {label}: the new name must be stored: {after}"
        );
        assert_eq!(
            after.get("description").and_then(Value::as_str),
            extra.get("description").and_then(Value::as_str),
            "case {label}: description must be the updated one (absent clears it): {after}"
        );
        let titles: Vec<&str> = after["widgets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: no widgets: {after}"))
            .iter()
            .map(|widget| widget["title"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            titles, expected_titles,
            "case {label}: expected widgets {expected_titles:?}, got {after}"
        );
        for widget in after["widgets"].as_array().expect("widgets") {
            let id = widget["id"].as_str().unwrap_or("<none>");
            assert!(
                is_widget_id(id),
                "case {label}: every stored widget carries a 32-hex id, got {id:?}"
            );
        }

        assert_eq!(
            timestamp(&after, "createdAt", label),
            timestamp(&before, "createdAt", label),
            "case {label}: createdAt must not move on update"
        );
        assert!(
            timestamp(&after, "updatedAt", label) > timestamp(&before, "updatedAt", label),
            "case {label}: updatedAt must move forward on update: {before} -> {after}"
        );

        // And the listing sees the update too.
        let listed = ok_call(&router, "ListDashboards", &json!({})).await;
        assert_eq!(
            listed["dashboards"][0]["name"].as_str(),
            Some("After Update"),
            "case {label}: ListDashboards projects the updated record: {listed}"
        );
    }
}

/// DeleteDashboard removes the record: the ARN stops resolving and stops being listed.
#[tokio::test]
async fn delete_dashboard_removes_the_record() {
    let router = router();
    let mut arns = Vec::new();
    for name in ["Keep Me", "Delete Me", "Keep Me Too"] {
        let created = ok_call(
            &router,
            "CreateDashboard",
            &json!({
                "name": name,
                "widgets": [{"title": "Spend", "configs": [table_config("Cost")]}],
                "resourceTags": [{"key": "team", "value": "finops"}]
            }),
        )
        .await;
        arns.push(created["arn"].as_str().expect("an arn").to_string());
    }
    let doomed = arns[1].clone();

    let deleted = ok_call(&router, "DeleteDashboard", &json!({"arn": doomed})).await;
    assert_eq!(
        deleted["arn"].as_str(),
        Some(doomed.as_str()),
        "DeleteDashboard must answer the arn it removed: {deleted}"
    );

    let (status, body) = call(&router, "GetDashboard", &json!({"arn": doomed})).await;
    assert_eq!(
        status, 404,
        "a deleted dashboard must not resolve, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {}",
        error_code(&body)
    );

    let listed = ok_call(&router, "ListDashboards", &json!({})).await;
    let remaining: Vec<&str> = listed["dashboards"]
        .as_array()
        .expect("dashboards")
        .iter()
        .map(|item| item["arn"].as_str().unwrap_or("<none>"))
        .collect();
    assert_eq!(
        remaining,
        vec![arns[0].as_str(), arns[2].as_str()],
        "ListDashboards must list exactly the surviving dashboards: {listed}"
    );

    // The record is gone, so deleting it again cannot report a deletion: this
    // operation models no not-found error, so it is refused as invalid input.
    let (status, body) = call(&router, "DeleteDashboard", &json!({"arn": doomed})).await;
    assert_eq!(
        status, 400,
        "deleting an absent dashboard must not answer 200, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "expected ValidationException, got {}",
        error_code(&body)
    );

    // Updating it is a not-found, which that operation does model.
    let (status, body) = call(
        &router,
        "UpdateDashboard",
        &json!({"arn": doomed, "name": "Back From The Dead"}),
    )
    .await;
    assert_eq!(
        status, 404,
        "updating an absent dashboard is a not-found, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {}",
        error_code(&body)
    );
}

/// An ARN that names no dashboard is a not-found; an ARN that is not a `DashboardArn`
/// at all is a validation failure.
#[tokio::test]
async fn unknown_and_malformed_arns_are_told_apart() {
    let router = router();

    // Well-formed (it satisfies the modelled pattern) but never created.
    let never_created = "arn:aws:bcm-dashboards::000000000000:dashboard/does-not-exist";
    let (status, body) = call(&router, "GetDashboard", &json!({"arn": never_created})).await;
    assert_eq!(
        status, 404,
        "case well-formed but unknown: expected 404, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "case well-formed but unknown: expected ResourceNotFoundException, got {}",
        error_code(&body)
    );

    // Each row breaks the `DashboardArn` pattern or its length in a different way.
    let malformed: &[(&str, &str)] = &[
        ("not an arn at all", "not-an-arn"),
        (
            "another service",
            "arn:aws:bcm-dataexports::000000000000:dashboard/abc",
        ),
        (
            "a region where the model allows none",
            "arn:aws:bcm-dashboards:us-east-1:000000000000:dashboard/abc",
        ),
        (
            "an account id that is not 12 digits",
            "arn:aws:bcm-dashboards::12345:dashboard/abc",
        ),
        (
            "an uppercase resource id",
            "arn:aws:bcm-dashboards::000000000000:dashboard/ABC",
        ),
        (
            "the wrong resource type",
            "arn:aws:bcm-dashboards::000000000000:report/abc",
        ),
        ("an empty arn", ""),
    ];
    for (label, arn) in malformed {
        assert!(
            !is_dashboard_arn(arn),
            "case {label}: the test's own ARN check disagrees that {arn:?} is malformed"
        );
        let (status, body) = call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            status, 400,
            "case {label}: {arn:?} must be refused as invalid, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {label}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}

/// The store a caller can grow is bounded: at the dashboard quota CreateDashboard
/// answers the modelled `ServiceQuotaExceededException` (402), the dashboards already
/// stored are untouched, and deleting one makes room for exactly one more.
#[tokio::test]
async fn create_dashboard_stops_at_the_dashboard_quota() {
    let router = router();
    let request = json!({"name": "Quota Filler", "widgets": []});

    let mut first_arn = None;
    for index in 0..MAX_DASHBOARDS {
        let created = ok_call(&router, "CreateDashboard", &request).await;
        let arn = created["arn"].as_str().expect("an arn").to_string();
        if index == 0 {
            first_arn = Some(arn);
        }
    }
    let first_arn = first_arn.expect("the first dashboard's arn");

    let (status, body) = call(&router, "CreateDashboard", &request).await;
    assert_eq!(
        status, 402,
        "case at the quota: CreateDashboard must refuse past {MAX_DASHBOARDS} dashboards, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("ServiceQuotaExceededException"),
        "case at the quota: expected ServiceQuotaExceededException, got {}",
        error_code(&body)
    );

    // The refusal stored nothing and removed nothing.
    let page = ok_call(&router, "ListDashboards", &json!({"maxResults": 100})).await;
    assert_eq!(
        page["dashboards"].as_array().expect("dashboards").len(),
        100,
        "case after the refusal: the stored dashboards must be untouched"
    );
    ok_call(&router, "GetDashboard", &json!({"arn": first_arn})).await;

    // Freeing one record makes room for exactly one more.
    ok_call(&router, "DeleteDashboard", &json!({"arn": first_arn})).await;
    ok_call(&router, "CreateDashboard", &request).await;
    let (status, body) = call(&router, "CreateDashboard", &request).await;
    assert_eq!(
        status, 402,
        "case back at the quota: CreateDashboard must refuse again, got {status}: {body}"
    );
}
