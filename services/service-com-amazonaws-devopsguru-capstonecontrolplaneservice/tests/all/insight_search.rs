// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `SearchInsights`.
//!
//! Every test drives the service the way a client does — a buffered HTTP request through
//! the same [`smithy_server_runtime::MultiServiceRouter`] the emulator boots — so the
//! assertions are about wire behaviour, not handler internals. The harness is the
//! `configuration_lifecycle` one and the instants come from the `insight_listing` grid,
//! so the search is asserted over the same kind of store the other insight reads use.
//!
//! DevOps Guru MINTS an insight itself, from anomalies in the covered resources; no
//! modelled operation creates one. So a test that needs insights seeds the store the
//! analysis would write (`state.insights`), exactly as the handler reads it, and then
//! asks the service over HTTP.
//!
//! RULES under test:
//!
//! 1. The search reports the stored insights of the ONE `InsightType` the request names
//!    whose behaviour STARTED inside the required `StartTimeRange` (both bounds
//!    inclusive, an absent bound unbounded), each on the response list of its own type.
//! 2. Every filter type the request carries NARROWS that selection — an insight is
//!    reported only while each of them admits it (`AND` across filter types), and a
//!    filter is satisfied by ANY ONE of the values it names (`OR` within one). A filter
//!    naming no value restricts nothing.
//! 3. A filter value outside the enum the model declares for it is a client error, not an
//!    empty answer.
//! 4. The search pages at the `MaxResults` the request asks for, its cursor resolves only
//!    against the selection it was issued for, and reading changes nothing a later caller
//!    sees.
//! 5. A searched insight is the SAME record `DescribeInsight` reports, because both read
//!    one store.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    InsightRecord, ResourceCoverage,
};

use crate::configuration_lifecycle::{Emulator, Reply};
use crate::insight_listing::{epoch_seconds, BASE_MILLIS, MINUTE_MILLIS};

/// `SearchInsights`'s HTTP binding, from
/// `smithy_query(query="operation",operation="SearchInsights")`: `POST /insights/search`,
/// with every member in the body.
const SEARCH_PATH: &str = "/insights/search";

/// `ListInsights`'s binding — `POST /insights` — so a row can show that one read's cursor
/// does not resolve against the other's.
const LIST_PATH: &str = "/insights";

/// Both response members a searched insight can be reported on.
const INSIGHT_LISTS: [&str; 2] = ["/ProactiveInsights", "/ReactiveInsights"];

/// The app-boundary key the seeded insights are tagged under. `AppBoundaryKey` is
/// length 1..128 matching `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from
/// `smithy_query(query="shape",shape="TagCollection")`.
const APP_BOUNDARY_KEY: &str = "devops-guru-app";

/// An instant of the grid, `minutes` from [`BASE_MILLIS`].
fn at(minutes: i64) -> i64 {
    BASE_MILLIS + minutes * MINUTE_MILLIS
}

/// One seeded insight: the members a search selects on, with everything else left at its
/// default so a difference in the answer can only come from these.
struct Seed<'a> {
    id: &'a str,
    /// One of `INSIGHT_TYPE_VALUES`.
    insight_type: &'a str,
    /// One of `INSIGHT_STATUS_VALUES`.
    status: &'a str,
    /// One of `INSIGHT_SEVERITY_VALUES`.
    severity: &'a str,
    /// `InsightTimeRange.StartTime`, in epoch milliseconds.
    started: i64,
    /// The CloudFormation stacks this insight is about.
    stacks: &'a [&'a str],
    /// The tag values, under [`APP_BOUNDARY_KEY`], this insight is about.
    tag_values: &'a [&'a str],
    /// `ServiceCollection.ServiceNames`, from `SERVICE_NAME_VALUES`.
    services: &'a [&'a str],
}

impl Default for Seed<'_> {
    fn default() -> Self {
        Self {
            id: "",
            insight_type: "PROACTIVE",
            status: "ONGOING",
            severity: "HIGH",
            started: BASE_MILLIS,
            stacks: &[],
            tag_values: &[],
            services: &[],
        }
    }
}

impl Seed<'_> {
    fn record(&self) -> InsightRecord {
        let mut coverage = ResourceCoverage {
            cloudformation_stack_names: self.stacks.iter().map(ToString::to_string).collect(),
            tag_collections: BTreeMap::new(),
        };
        if !self.tag_values.is_empty() {
            coverage.tag_collections.insert(
                APP_BOUNDARY_KEY.to_string(),
                self.tag_values.iter().map(ToString::to_string).collect(),
            );
        }
        InsightRecord {
            id: self.id.to_string(),
            name: format!("DevOps Guru insight {}", self.id),
            insight_type: self.insight_type.to_string(),
            severity: self.severity.to_string(),
            status: self.status.to_string(),
            start_time_millis: self.started,
            // A closed insight's behaviour ended; an ongoing one's has not.
            end_time_millis: (self.status == "CLOSED").then(|| self.started + MINUTE_MILLIS),
            resource_collection: (!self.stacks.is_empty() || !self.tag_values.is_empty())
                .then_some(coverage),
            service_names: self.services.iter().map(ToString::to_string).collect(),
            ..InsightRecord::default()
        }
    }
}

/// The eight insights every row is asserted against: both types, both statuses, all three
/// severities, start instants spread over a known grid, and resource collections that are
/// a stack, a tag pair, or nothing at all.
fn search_fixture() -> Vec<InsightRecord> {
    [
        Seed {
            id: "proactive-early-low-ongoing-gamma",
            severity: "LOW",
            started: at(-30),
            stacks: &["gamma"],
            ..Seed::default()
        },
        Seed {
            id: "proactive-high-closed-late",
            status: "CLOSED",
            started: at(90),
            ..Seed::default()
        },
        Seed {
            id: "proactive-high-ongoing-alpha",
            started: at(0),
            stacks: &["alpha"],
            services: &["LAMBDA"],
            ..Seed::default()
        },
        Seed {
            id: "proactive-low-closed-beta",
            status: "CLOSED",
            severity: "LOW",
            started: at(10),
            stacks: &["beta"],
            services: &["RDS"],
            ..Seed::default()
        },
        Seed {
            id: "proactive-medium-ongoing-payments",
            severity: "MEDIUM",
            started: at(20),
            tag_values: &["payments"],
            services: &["DYNAMO_DB", "LAMBDA"],
            ..Seed::default()
        },
        Seed {
            id: "reactive-high-ongoing-alpha",
            insight_type: "REACTIVE",
            started: at(0),
            stacks: &["alpha"],
            services: &["EC2"],
            ..Seed::default()
        },
        Seed {
            id: "reactive-low-ongoing-bare",
            insight_type: "REACTIVE",
            severity: "LOW",
            started: at(45),
            ..Seed::default()
        },
        Seed {
            id: "reactive-medium-closed-checkout",
            insight_type: "REACTIVE",
            status: "CLOSED",
            severity: "MEDIUM",
            started: at(30),
            tag_values: &["checkout"],
            ..Seed::default()
        },
    ]
    .iter()
    .map(Seed::record)
    .collect()
}

/// An emulator holding [`search_fixture`].
fn seeded_emulator() -> Emulator {
    let emulator = Emulator::new();
    emulator.seed_insights(search_fixture());
    emulator
}

/// A `SearchInsights` request body: the two required members, plus whatever else a row
/// wants.
fn request(insight_type: &str, from: Option<i64>, to: Option<i64>) -> Value {
    let mut range = json!({});
    if let Some(from) = from {
        range["FromTime"] = json!(epoch_seconds(from));
    }
    if let Some(to) = to {
        range["ToTime"] = json!(epoch_seconds(to));
    }
    json!({ "Type": insight_type, "StartTimeRange": range })
}

/// The simplest well-formed request for `insight_type`: an unbounded start-time range.
fn every_instant(insight_type: &str) -> Value {
    request(insight_type, None, None)
}

async fn search(emulator: &Emulator, body: &Value) -> Reply {
    emulator.call("POST", SEARCH_PATH, Some(body)).await
}

/// The `Id`s one reply reported, proactive insights first.
fn reported_ids(reply: &Reply) -> Vec<String> {
    INSIGHT_LISTS
        .iter()
        .flat_map(|list| {
            reply
                .at(list)
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|item| {
                    item.get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect::<Vec<String>>()
        })
        .collect()
}

/// The cursor a reply carries, if it has one.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .at("/NextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// `expected`, in the ascending `InsightId` order the store keeps and therefore the order
/// a page reports.
fn in_store_order(expected: &[&str]) -> Vec<String> {
    expected
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Assert one row: the request selects exactly `expected`.
async fn assert_selects(emulator: &Emulator, case: &str, body: &Value, expected: &[&str]) {
    let reply = search(emulator, body).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: a well-formed search must succeed, got HTTP {} {}",
        reply.status, reply.raw
    );
    assert_eq!(
        reported_ids(&reply),
        in_store_order(expected),
        "case {case:?}: the search must report exactly {expected:?}, got {}",
        reply.raw
    );
    assert!(
        next_token(&reply).is_none(),
        "case {case:?}: a selection that fits one page carries no NextToken, got {}",
        reply.raw
    );
}

// =============================================================================
// 1. Type and the required start-time range
// =============================================================================

/// RULE 1: the search reports the stored insights of the `InsightType` it names whose
/// `InsightTimeRange.StartTime` lies in the `StartTimeRange` — `StartTimeRange` is "a
/// time range used to specify when the behavior of an insight or anomaly started" — with
/// both bounds inclusive and an absent bound unbounded.
///
/// The rows differ in the type searched, in which side of the range is bounded, in how
/// wide the window is (every insight, some, exactly one, none), and in whether the window
/// is inverted — which holds no instant and so selects nothing.
#[tokio::test]
async fn type_and_start_time_range_select_the_insights_searched_for() {
    let emulator = seeded_emulator();

    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "PROACTIVE, unbounded window",
            every_instant("PROACTIVE"),
            vec![
                "proactive-early-low-ongoing-gamma",
                "proactive-high-closed-late",
                "proactive-high-ongoing-alpha",
                "proactive-low-closed-beta",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "PROACTIVE, bounded below only",
            request("PROACTIVE", Some(at(0)), None),
            vec![
                "proactive-high-closed-late",
                "proactive-high-ongoing-alpha",
                "proactive-low-closed-beta",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "PROACTIVE, bounded above only",
            request("PROACTIVE", None, Some(at(0))),
            vec![
                "proactive-early-low-ongoing-gamma",
                "proactive-high-ongoing-alpha",
            ],
        ),
        (
            "PROACTIVE, both bounds",
            request("PROACTIVE", Some(at(0)), Some(at(20))),
            vec![
                "proactive-high-ongoing-alpha",
                "proactive-low-closed-beta",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "PROACTIVE, a window of one instant, inclusive on both sides",
            request("PROACTIVE", Some(at(10)), Some(at(10))),
            vec!["proactive-low-closed-beta"],
        ),
        (
            "PROACTIVE, inverted window",
            request("PROACTIVE", Some(at(60)), Some(at(0))),
            vec![],
        ),
        (
            "REACTIVE, unbounded window",
            every_instant("REACTIVE"),
            vec![
                "reactive-high-ongoing-alpha",
                "reactive-low-ongoing-bare",
                "reactive-medium-closed-checkout",
            ],
        ),
        (
            "REACTIVE, both bounds",
            request("REACTIVE", Some(at(0)), Some(at(30))),
            vec!["reactive-high-ongoing-alpha", "reactive-medium-closed-checkout"],
        ),
        (
            "REACTIVE, a window after every stored insight",
            request("REACTIVE", Some(at(1_000)), None),
            vec![],
        ),
    ];

    for (case, body, expected) in rows {
        assert_selects(&emulator, case, &body, &expected).await;
    }
}

/// RULE 1: each selected insight is reported on the list of its OWN type, so a search for
/// one type reports an empty list for the other rather than omitting it.
#[tokio::test]
async fn each_type_is_reported_on_its_own_list() {
    let emulator = seeded_emulator();

    for (case, insight_type, own, other, expected) in [
        (
            "PROACTIVE",
            "PROACTIVE",
            "/ProactiveInsights",
            "/ReactiveInsights",
            vec![
                "proactive-early-low-ongoing-gamma",
                "proactive-high-closed-late",
                "proactive-high-ongoing-alpha",
                "proactive-low-closed-beta",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "REACTIVE",
            "REACTIVE",
            "/ReactiveInsights",
            "/ProactiveInsights",
            vec![
                "reactive-high-ongoing-alpha",
                "reactive-low-ongoing-bare",
                "reactive-medium-closed-checkout",
            ],
        ),
    ] {
        let reply = search(&emulator, &every_instant(insight_type)).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a well-formed search must succeed, got HTTP {} {}",
            reply.status, reply.raw
        );
        let listed: Vec<String> = reply
            .at(own)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|item| {
                item.get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            listed,
            in_store_order(&expected),
            "case {case:?}: exactly the searched type's insights must be reported on {own}, got {}",
            reply.raw
        );
        assert_eq!(
            reply.at(other).and_then(Value::as_array).map(Vec::len),
            Some(0),
            "case {case:?}: the other type's list must be present and empty, got {}",
            reply.raw
        );
    }
}

// =============================================================================
// 2. Every filter type narrows, and the filter types are AND-ed
// =============================================================================

/// RULE 2: `SearchInsightsFilters` documents "Each filter type contains one or more
/// values to search for. If you specify multiple filter types, the filter types are
/// joined with an AND, and the request returns only results that match all of the
/// specified filters" — so an insight is reported only while every filter type admits it,
/// one filter type is satisfied by any one of its values, and a filter that names no
/// value restricts nothing.
///
/// The rows differ in WHICH filter type is sent, in how many values it names (none, one,
/// several, all of the enum), in the FORM a resource collection takes (a stack, several
/// stacks, a tag pair, both halves at once, an empty collection), in which filter types
/// are combined, and in the insight type they are combined with.
#[tokio::test]
async fn every_filter_type_narrows_and_the_filter_types_are_anded() {
    let emulator = seeded_emulator();
    let every_proactive = vec![
        "proactive-early-low-ongoing-gamma",
        "proactive-high-closed-late",
        "proactive-high-ongoing-alpha",
        "proactive-low-closed-beta",
        "proactive-medium-ongoing-payments",
    ];

    let rows: Vec<(&str, &str, Value, Vec<&str>)> = vec![
        // ---- a filter that names nothing restricts nothing -------------------
        (
            "no Filters member",
            "PROACTIVE",
            Value::Null,
            every_proactive.clone(),
        ),
        (
            "an empty Filters structure",
            "PROACTIVE",
            json!({}),
            every_proactive.clone(),
        ),
        (
            "an empty Severities list",
            "PROACTIVE",
            json!({ "Severities": [] }),
            every_proactive.clone(),
        ),
        (
            "an empty Statuses list",
            "PROACTIVE",
            json!({ "Statuses": [] }),
            every_proactive.clone(),
        ),
        (
            "a ServiceCollection naming no service",
            "PROACTIVE",
            json!({ "ServiceCollection": { "ServiceNames": [] } }),
            every_proactive.clone(),
        ),
        (
            "a ResourceCollection naming no resource",
            "PROACTIVE",
            json!({ "ResourceCollection": {} }),
            every_proactive.clone(),
        ),
        (
            "a ResourceCollection naming a tag key with no value",
            "PROACTIVE",
            json!({ "ResourceCollection": {
                "Tags": [{ "AppBoundaryKey": APP_BOUNDARY_KEY, "TagValues": [] }],
            } }),
            every_proactive.clone(),
        ),
        // ---- Severities ------------------------------------------------------
        (
            "Severities names one value",
            "PROACTIVE",
            json!({ "Severities": ["HIGH"] }),
            vec!["proactive-high-closed-late", "proactive-high-ongoing-alpha"],
        ),
        (
            "Severities names two values",
            "PROACTIVE",
            json!({ "Severities": ["LOW", "MEDIUM"] }),
            vec![
                "proactive-early-low-ongoing-gamma",
                "proactive-low-closed-beta",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "Severities names the whole enum",
            "PROACTIVE",
            json!({ "Severities": ["LOW", "MEDIUM", "HIGH"] }),
            every_proactive.clone(),
        ),
        // ---- Statuses --------------------------------------------------------
        (
            "Statuses names ONGOING",
            "PROACTIVE",
            json!({ "Statuses": ["ONGOING"] }),
            vec![
                "proactive-early-low-ongoing-gamma",
                "proactive-high-ongoing-alpha",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "Statuses names CLOSED",
            "PROACTIVE",
            json!({ "Statuses": ["CLOSED"] }),
            vec!["proactive-high-closed-late", "proactive-low-closed-beta"],
        ),
        (
            "Statuses names both values",
            "PROACTIVE",
            json!({ "Statuses": ["ONGOING", "CLOSED"] }),
            every_proactive.clone(),
        ),
        // ---- ServiceCollection ----------------------------------------------
        (
            "ServiceCollection names one service",
            "PROACTIVE",
            json!({ "ServiceCollection": { "ServiceNames": ["LAMBDA"] } }),
            vec![
                "proactive-high-ongoing-alpha",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "ServiceCollection names two services",
            "PROACTIVE",
            json!({ "ServiceCollection": { "ServiceNames": ["RDS", "DYNAMO_DB"] } }),
            vec![
                "proactive-low-closed-beta",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "ServiceCollection names a service no insight names",
            "PROACTIVE",
            json!({ "ServiceCollection": { "ServiceNames": ["SQS"] } }),
            vec![],
        ),
        (
            "ServiceCollection names a service only a REACTIVE insight names",
            "REACTIVE",
            json!({ "ServiceCollection": { "ServiceNames": ["EC2"] } }),
            vec!["reactive-high-ongoing-alpha"],
        ),
        // ---- ResourceCollection ---------------------------------------------
        (
            "ResourceCollection names one stack",
            "PROACTIVE",
            json!({ "ResourceCollection": { "CloudFormation": { "StackNames": ["alpha"] } } }),
            vec!["proactive-high-ongoing-alpha"],
        ),
        (
            "ResourceCollection names two stacks",
            "PROACTIVE",
            json!({ "ResourceCollection": { "CloudFormation": { "StackNames": ["beta", "gamma"] } } }),
            vec![
                "proactive-early-low-ongoing-gamma",
                "proactive-low-closed-beta",
            ],
        ),
        (
            "ResourceCollection names a stack no insight is about",
            "PROACTIVE",
            json!({ "ResourceCollection": { "CloudFormation": { "StackNames": ["delta"] } } }),
            vec![],
        ),
        (
            "ResourceCollection names one tag pair",
            "PROACTIVE",
            json!({ "ResourceCollection": {
                "Tags": [{ "AppBoundaryKey": APP_BOUNDARY_KEY, "TagValues": ["payments"] }],
            } }),
            vec!["proactive-medium-ongoing-payments"],
        ),
        (
            "ResourceCollection names a tag value of the other insight type",
            "PROACTIVE",
            json!({ "ResourceCollection": {
                "Tags": [{ "AppBoundaryKey": APP_BOUNDARY_KEY, "TagValues": ["checkout"] }],
            } }),
            vec![],
        ),
        (
            "ResourceCollection names a tag pair under an uncovered key",
            "PROACTIVE",
            json!({ "ResourceCollection": {
                "Tags": [{ "AppBoundaryKey": "another-boundary", "TagValues": ["payments"] }],
            } }),
            vec![],
        ),
        (
            "ResourceCollection names a stack AND a tag pair, whose union it is",
            "PROACTIVE",
            json!({ "ResourceCollection": {
                "CloudFormation": { "StackNames": ["alpha"] },
                "Tags": [{ "AppBoundaryKey": APP_BOUNDARY_KEY, "TagValues": ["payments"] }],
            } }),
            vec![
                "proactive-high-ongoing-alpha",
                "proactive-medium-ongoing-payments",
            ],
        ),
        (
            "ResourceCollection names a tag pair of a REACTIVE insight",
            "REACTIVE",
            json!({ "ResourceCollection": {
                "Tags": [{ "AppBoundaryKey": APP_BOUNDARY_KEY, "TagValues": ["checkout"] }],
            } }),
            vec!["reactive-medium-closed-checkout"],
        ),
        // ---- several filter types at once, joined with AND -------------------
        (
            "Severities AND Statuses",
            "PROACTIVE",
            json!({ "Severities": ["HIGH"], "Statuses": ["ONGOING"] }),
            vec!["proactive-high-ongoing-alpha"],
        ),
        (
            "Severities AND a ResourceCollection no insight of that severity is about",
            "PROACTIVE",
            json!({
                "Severities": ["LOW"],
                "ResourceCollection": { "CloudFormation": { "StackNames": ["alpha"] } },
            }),
            vec![],
        ),
        (
            "ResourceCollection AND ServiceCollection, both admitting one insight",
            "PROACTIVE",
            json!({
                "ResourceCollection": { "CloudFormation": { "StackNames": ["alpha"] } },
                "ServiceCollection": { "ServiceNames": ["LAMBDA"] },
            }),
            vec!["proactive-high-ongoing-alpha"],
        ),
        (
            "all four filter types at once",
            "PROACTIVE",
            json!({
                "Severities": ["MEDIUM", "HIGH"],
                "Statuses": ["ONGOING"],
                "ServiceCollection": { "ServiceNames": ["LAMBDA"] },
                "ResourceCollection": {
                    "Tags": [{ "AppBoundaryKey": APP_BOUNDARY_KEY, "TagValues": ["payments"] }],
                },
            }),
            vec!["proactive-medium-ongoing-payments"],
        ),
    ];

    for (case, insight_type, filters, expected) in rows {
        let mut body = every_instant(insight_type);
        if !filters.is_null() {
            body["Filters"] = filters;
        }
        assert_selects(&emulator, case, &body, &expected).await;
    }
}

/// RULE 2 over the start-time range as well: a filter narrows the window's selection
/// rather than replacing it, so an insight outside the window is not reported however the
/// filters are set.
#[tokio::test]
async fn filters_narrow_the_time_window_rather_than_replacing_it() {
    let emulator = seeded_emulator();

    let mut body = request("PROACTIVE", Some(at(0)), Some(at(20)));
    body["Filters"] = json!({ "Severities": ["LOW"] });
    assert_selects(
        &emulator,
        "LOW inside a window the earliest LOW insight is outside of",
        &body,
        &["proactive-low-closed-beta"],
    )
    .await;

    let mut body = request("PROACTIVE", Some(at(-60)), Some(at(20)));
    body["Filters"] = json!({ "Severities": ["LOW"] });
    assert_selects(
        &emulator,
        "LOW inside a window that reaches the earliest LOW insight",
        &body,
        &[
            "proactive-early-low-ongoing-gamma",
            "proactive-low-closed-beta",
        ],
    )
    .await;
}

// =============================================================================
// 3. A value outside its enum is refused
// =============================================================================

/// RULE 3: a `Type`, severity, status or service name outside the enum the model declares
/// is a client error (HTTP 400 `ValidationException`), not an empty answer; so is a
/// `MaxResults` outside the modelled range and a `NextToken` outside the modelled pattern.
///
/// The rows differ in WHICH member carries the bad value, in the FORM of the wrongness
/// (another enum's value, a lowercased value, an unknown value, a value past a numeric
/// bound), and in whether the member is a scalar or one entry of a list.
#[tokio::test]
async fn a_value_outside_its_enum_or_range_is_refused() {
    let emulator = seeded_emulator();

    let rows: Vec<(&str, Value)> = vec![
        ("Type outside InsightType", json!({ "Type": "PREDICTIVE" })),
        ("Type lowercased", json!({ "Type": "proactive" })),
        ("Type empty", json!({ "Type": "" })),
        (
            "Severities carries another enum's value",
            json!({ "Filters": { "Severities": ["ONGOING"] } }),
        ),
        (
            "Severities carries a lowercased value beside a good one",
            json!({ "Filters": { "Severities": ["HIGH", "high"] } }),
        ),
        (
            "Statuses carries a value InsightStatus does not admit",
            json!({ "Filters": { "Statuses": ["ANY"] } }),
        ),
        (
            "ServiceNames carries an unknown service",
            json!({ "Filters": { "ServiceCollection": { "ServiceNames": ["EC3"] } } }),
        ),
        // `SearchInsightsMaxResults` is modelled range 1..100.
        ("MaxResults below the modelled range", json!({ "MaxResults": 0 })),
        ("MaxResults above the modelled range", json!({ "MaxResults": 101 })),
        // `UuidNextToken` is modelled len 36..36 with an 8-4-4-4-12 lowercase-hex pattern.
        ("NextToken outside the modelled pattern", json!({ "NextToken": "not-a-page-token-at-all" })),
    ];

    for (case, overrides) in rows {
        let mut body = every_instant("PROACTIVE");
        for (member, value) in overrides.as_object().expect("the row must be an object") {
            body[member.as_str()] = value.clone();
        }
        let reply = search(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: the request must be refused, got HTTP {} {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: the refusal must be a ValidationException, got {}",
            reply.raw
        );
        for list in INSIGHT_LISTS {
            assert!(
                reply.at(list).is_none(),
                "case {case:?}: a refusal reports no insight on {list}, got {}",
                reply.raw
            );
        }
    }
}

/// RULE 3's siblings: both `Type` and `StartTimeRange` are modelled **required**, so a
/// request missing either is refused rather than answered over a default.
#[tokio::test]
async fn a_request_missing_a_required_member_is_refused() {
    let emulator = seeded_emulator();

    for (case, body) in [
        ("no Type", json!({ "StartTimeRange": {} })),
        ("no StartTimeRange", json!({ "Type": "PROACTIVE" })),
        ("neither required member", json!({})),
    ] {
        let reply = search(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a request missing a required member must be refused, got HTTP {} {}",
            reply.status, reply.raw
        );
    }
}

// =============================================================================
// 4. Pagination, cursor scoping, and reading changes nothing
// =============================================================================

/// Every id a search walks at `max_results` per page, following its cursor to the end.
async fn walk(emulator: &Emulator, body: &Value, max_results: Option<i32>) -> Vec<String> {
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut page = body.clone();
        if let Some(max_results) = max_results {
            page["MaxResults"] = json!(max_results);
        }
        if let Some(token) = token.as_deref() {
            page["NextToken"] = json!(token);
        }
        let reply = search(emulator, &page).await;
        assert_eq!(
            reply.status, 200,
            "walking at {max_results:?} per page must succeed, got HTTP {} {}",
            reply.status, reply.raw
        );
        let ids = reported_ids(&reply);
        assert!(
            ids.len() <= max_results.unwrap_or(i32::MAX) as usize,
            "a page must hold at most the requested MaxResults, got {}",
            reply.raw
        );
        walked.extend(ids);
        match next_token(&reply) {
            Some(next) => token = Some(next),
            None => return walked,
        }
    }
}

/// RULE 4: a walk reports each selected insight exactly once, in the store's order,
/// whatever page size it asks for.
///
/// The rows differ in the page size: one per page, a size that divides the selection, one
/// that does not, the whole selection at once, more than the selection holds, and no
/// `MaxResults` at all (the service's own page size).
#[tokio::test]
async fn a_walk_reports_each_selected_insight_exactly_once() {
    let emulator = seeded_emulator();
    let body = every_instant("PROACTIVE");
    let expected = in_store_order(&[
        "proactive-early-low-ongoing-gamma",
        "proactive-high-closed-late",
        "proactive-high-ongoing-alpha",
        "proactive-low-closed-beta",
        "proactive-medium-ongoing-payments",
    ]);

    for max_results in [Some(1), Some(2), Some(3), Some(5), Some(100), None] {
        let walked = walk(&emulator, &body, max_results).await;
        assert_eq!(
            walked, expected,
            "case {max_results:?} per page: the walk must report the selection exactly once, got {walked:?}"
        );
    }
}

/// RULE 4: a cursor is bound to the SELECTION it was issued for, so a token from another
/// filter set, another insight type, another time window, or from the sibling
/// `ListInsights` walk names no page of this search and is refused.
#[tokio::test]
async fn a_cursor_resolves_only_against_the_selection_it_was_issued_for() {
    let emulator = seeded_emulator();

    // A first page of a PROACTIVE search over the whole window, which leaves more to walk.
    let mut first = every_instant("PROACTIVE");
    first["MaxResults"] = json!(1);
    let reply = search(&emulator, &first).await;
    assert_eq!(
        reply.status, 200,
        "the first page must succeed, got HTTP {} {}",
        reply.status, reply.raw
    );
    let token = next_token(&reply).unwrap_or_else(|| {
        panic!(
            "a selection of five insights paged at one must carry a cursor, got {}",
            reply.raw
        )
    });

    // The same cursor on the same selection resolves.
    let mut same = first.clone();
    same["NextToken"] = json!(token);
    let resumed = search(&emulator, &same).await;
    assert_eq!(
        resumed.status, 200,
        "case \"same selection\": the cursor must resolve, got HTTP {} {}",
        resumed.status, resumed.raw
    );
    assert_eq!(
        reported_ids(&resumed),
        vec!["proactive-high-closed-late".to_string()],
        "case \"same selection\": the cursor must name the page after the first, got {}",
        resumed.raw
    );

    // A filter list names a SET of values, so a cursor issued under one spelling of it
    // resolves under any other: both select exactly the same insights.
    let mut ascending = every_instant("PROACTIVE");
    ascending["MaxResults"] = json!(1);
    ascending["Filters"] = json!({ "Severities": ["LOW", "MEDIUM", "HIGH"] });
    let paged = search(&emulator, &ascending).await;
    assert_eq!(
        paged.status, 200,
        "case \"reordered filter\": the first page must succeed, got HTTP {} {}",
        paged.status, paged.raw
    );
    let filtered_token = next_token(&paged).unwrap_or_else(|| {
        panic!(
            "a selection of five insights paged at one must carry a cursor, got {}",
            paged.raw
        )
    });
    let mut reordered = ascending.clone();
    reordered["Filters"] = json!({ "Severities": ["HIGH", "LOW", "MEDIUM"] });
    reordered["NextToken"] = json!(filtered_token);
    let reply = search(&emulator, &reordered).await;
    assert_eq!(
        reply.status, 200,
        "case \"reordered filter\": the cursor must resolve, got HTTP {} {}",
        reply.status, reply.raw
    );
    assert_eq!(
        reported_ids(&reply),
        vec!["proactive-high-closed-late".to_string()],
        "case \"reordered filter\": the cursor must name the page after the first, got {}",
        reply.raw
    );

    // On any OTHER selection it names no page.
    let mut other_type = every_instant("REACTIVE");
    other_type["MaxResults"] = json!(1);
    let mut other_window = request("PROACTIVE", Some(at(0)), None);
    other_window["MaxResults"] = json!(1);
    let mut other_filters = every_instant("PROACTIVE");
    other_filters["MaxResults"] = json!(1);
    other_filters["Filters"] = json!({ "Statuses": ["ONGOING"] });

    for (case, mut body) in [
        ("another insight type", other_type),
        ("another time window", other_window),
        ("another filter set", other_filters),
    ] {
        body["NextToken"] = json!(token);
        let reply = search(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a cursor of another selection must be refused, got HTTP {} {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: the refusal must be a ValidationException, got {}",
            reply.raw
        );
    }

    // Nor does the sibling listing's cursor resolve here, or this search's cursor there.
    let listing = emulator
        .call(
            "POST",
            LIST_PATH,
            Some(&json!({
                "StatusFilter": { "Any": { "Type": "PROACTIVE", "StartTimeRange": {} } },
                "MaxResults": 1,
            })),
        )
        .await;
    assert_eq!(
        listing.status, 200,
        "the sibling listing must succeed, got HTTP {} {}",
        listing.status, listing.raw
    );
    let listing_token = next_token(&listing).unwrap_or_else(|| {
        panic!(
            "the sibling listing paged at one must carry a cursor, got {}",
            listing.raw
        )
    });
    let mut body = first.clone();
    body["NextToken"] = json!(listing_token);
    let reply = search(&emulator, &body).await;
    assert_eq!(
        reply.status, 400,
        "case \"ListInsights cursor\": another operation's cursor must be refused, got HTTP {} {}",
        reply.status, reply.raw
    );
    assert!(
        reply.names_error("ValidationException"),
        "case \"ListInsights cursor\": the refusal must be a ValidationException, got {}",
        reply.raw
    );
}

/// RULE 4: the search is a REPORT, so repeating it answers the same thing and the store a
/// later caller reads is unchanged — the insights, and the other reads over them, are
/// exactly what they were before the search.
#[tokio::test]
async fn searching_changes_nothing_a_later_caller_sees() {
    let emulator = seeded_emulator();
    let body = every_instant("PROACTIVE");

    let before = search(&emulator, &body).await;
    // A walk of the whole selection, one page at a time, between the two reads.
    let walked = walk(&emulator, &body, Some(2)).await;
    let after = search(&emulator, &body).await;

    assert_eq!(
        before.body, after.body,
        "repeating the search must report the same insights, got {} then {}",
        before.raw, after.raw
    );
    assert_eq!(
        walked,
        reported_ids(&after),
        "the paged walk and the single-page read must report the same insights, got {walked:?}"
    );
}

// =============================================================================
// 5. One store: a searched insight is the one DescribeInsight reports
// =============================================================================

/// RULE 5: the search reads the store the other insight reads read, so every insight it
/// reports can be described, and the two reads agree on its severity, status and start
/// instant.
#[tokio::test]
async fn a_searched_insight_is_the_one_describe_insight_reports() {
    let emulator = seeded_emulator();

    for insight_type in ["PROACTIVE", "REACTIVE"] {
        let reply = search(&emulator, &every_instant(insight_type)).await;
        assert_eq!(
            reply.status, 200,
            "case {insight_type:?}: the search must succeed, got HTTP {} {}",
            reply.status, reply.raw
        );
        let summaries = reply
            .at(if insight_type == "PROACTIVE" {
                "/ProactiveInsights"
            } else {
                "/ReactiveInsights"
            })
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            !summaries.is_empty(),
            "case {insight_type:?}: the fixture holds insights of this type, got {}",
            reply.raw
        );
        for summary in summaries {
            let id = summary
                .get("Id")
                .and_then(Value::as_str)
                .expect("a reported summary carries its Id")
                .to_string();
            let described = emulator
                .call("GET", &format!("/insights/{id}"), None)
                .await;
            assert_eq!(
                described.status, 200,
                "case {id:?}: a searched insight must be describable, got HTTP {} {}",
                described.status, described.raw
            );
            let detail = described
                .at(if insight_type == "PROACTIVE" {
                    "/ProactiveInsight"
                } else {
                    "/ReactiveInsight"
                })
                .cloned()
                .unwrap_or(Value::Null);
            for member in ["Id", "Name", "Severity", "Status"] {
                assert_eq!(
                    detail.get(member),
                    summary.get(member),
                    "case {id:?}: the search and DescribeInsight must agree on {member}, got {} and {}",
                    reply.raw,
                    described.raw
                );
            }
            assert_eq!(
                detail.get("InsightTimeRange"),
                summary.get("InsightTimeRange"),
                "case {id:?}: the search and DescribeInsight must agree on InsightTimeRange, got {} and {}",
                reply.raw,
                described.raw
            );
        }
    }
}
