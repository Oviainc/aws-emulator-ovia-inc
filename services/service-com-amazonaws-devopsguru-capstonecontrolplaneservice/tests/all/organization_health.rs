// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeOrganizationHealth`, the organization-wide health read.
//!
//! Driven over the wire through the same [`Emulator`] harness the configuration-lifecycle
//! suite provides, so every assertion is about what a client sees. The insights and
//! anomalies are seeded the way DevOps Guru's own analysis would write them — no modelled
//! operation mints either — while the coverage is declared through
//! `UpdateResourceCollection`'s own wire path.
//!
//! RULES pinned here:
//!
//! 1. An organization-wide read belongs to the account that ADMINISTERS DevOps Guru for
//!    the organization, so every other caller is refused with the operation's HTTP 403
//!    `AccessDeniedException` — which is every caller a client of this emulator can
//!    present, because nothing registers an administrator.
//! 2. For that administering caller each of the four figures is a TALLY over the records
//!    the store holds: the `ONGOING` insights of each `InsightType`, the distinct metrics
//!    the stored anomalies name as the evidence the analysis read, and one
//!    resource-analysis hour per monitored resource of the persisted coverage. So this
//!    read agrees with `DescribeAccountHealth` over the same store, and a figure is zero
//!    only while the set it tallies is empty.
//! 3. `AccountIds` and `OrganizationalUnitIds` NARROW the report to the member accounts
//!    they name; a filter naming none restricts nothing, and the one member account this
//!    store holds belongs to no organizational unit.
//! 4. The report is a pure read: it leaves the store exactly as it was.

use serde_json::{json, Value};

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::{
    AnomalySourceDetails, CloudWatchMetricsDetail, CloudWatchMetricsDimension,
};
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    AnomalyRecord, InsightRecord,
};

use super::configuration_lifecycle::{Emulator, Reply};
use super::resource_collection_health::{insight, CLOSED, ONGOING, PROACTIVE, REACTIVE};

/// `DescribeOrganizationHealth`'s http binding, from
/// `smithy_query(query="operation",operation="DescribeOrganizationHealth")`:
/// `POST /organization/health` with every member in the body.
async fn describe_organization_health(emulator: &Emulator, body: &Value) -> Reply {
    emulator
        .call("POST", "/organization/health", Some(body))
        .await
}

/// `DescribeAccountHealth` over the same store, for the agreement rule.
async fn describe_account_health(emulator: &Emulator) -> Reply {
    emulator.call("GET", "/accounts/health", None).await
}

/// The account this emulator serves, which is the one member account whose analysis
/// output the store holds. `AwsAccountId` is modelled `^\d{12}$`.
fn served_account() -> String {
    engine::emulator_account_id().to_string()
}

/// Twelve-digit account ids that are NOT the served account — `AwsAccountId`'s pattern,
/// from `smithy_query(query="shape",shape="AccountIdList")`.
const OTHER_ACCOUNTS: [&str; 4] = ["210987654321", "111122223333", "444455556666", "777788889999"];

/// Ids matching `OrganizationalUnitId`'s pattern `^ou-[0-9a-z]{4,32}-[a-z0-9]{8,32}$`, from
/// `smithy_query(query="shape",shape="OrganizationalUnitIdList")`.
const ORGANIZATIONAL_UNITS: [&str; 2] = ["ou-abc1-12345678", "ou-9xyz-abcdef0123456789"];

/// The four figures one reply reports, as a client reads them: open proactive insights,
/// open reactive insights, metrics analysed, resource hours. Every member is modelled
/// required, so an absent one is `None` and fails the row rather than reading as a zero.
type Figures = (Option<i64>, Option<i64>, Option<i64>, Option<i64>);

/// Every figure absent — what a refusal must carry.
const NO_FIGURES: Figures = (None, None, None, None);

fn figures(reply: &Reply) -> Figures {
    (
        number_at(reply, "/OpenProactiveInsights"),
        number_at(reply, "/OpenReactiveInsights"),
        number_at(reply, "/MetricsAnalyzed"),
        number_at(reply, "/ResourceHours"),
    )
}

fn number_at(reply: &Reply, pointer: &str) -> Option<i64> {
    reply.at(pointer).and_then(Value::as_i64)
}

/// The four figures a served reply is expected to carry.
fn expected(
    open_proactive: i64,
    open_reactive: i64,
    metrics_analyzed: i64,
    resource_hours: i64,
) -> Figures {
    (
        Some(open_proactive),
        Some(open_reactive),
        Some(metrics_analyzed),
        Some(resource_hours),
    )
}

/// One anomaly naming ONE CloudWatch metric as the evidence the analysis read, which is
/// how a metric enters this store at all. The metric-identity rule itself is pinned by the
/// account-health suite; these rows only need metrics that are distinct from each other.
fn metric_anomaly(id: &str, namespace: &str, metric_name: &str) -> AnomalyRecord {
    AnomalyRecord {
        id: id.to_string(),
        severity: "HIGH".to_string(),
        status: ONGOING.to_string(),
        source_details: Some(AnomalySourceDetails {
            cloud_watch_metrics: Some(vec![CloudWatchMetricsDetail {
                metric_name: Some(metric_name.to_string()),
                namespace: Some(namespace.to_string()),
                dimensions: Some(vec![CloudWatchMetricsDimension {
                    name: Some("FunctionName".to_string()),
                    value: Some(id.to_string()),
                }]),
                stat: Some("Average".to_string()),
                period: 60,
                ..CloudWatchMetricsDetail::default()
            }]),
            performance_insights_metrics: None,
        }),
        ..AnomalyRecord::default()
    }
}

/// An `UpdateResourceCollection` filter carrying the submitted stacks and tag entries.
fn collection(stacks: &[&str], tag_entries: &[(&str, &[&str])]) -> Value {
    let mut filter = json!({});
    if !stacks.is_empty() {
        filter["CloudFormation"] = json!({ "StackNames": stacks });
    }
    if !tag_entries.is_empty() {
        filter["Tags"] = tag_entries
            .iter()
            .map(|(key, values)| json!({ "AppBoundaryKey": key, "TagValues": values }))
            .collect();
    }
    filter
}

/// Declare `stacks` and `tag_entries` as the account's coverage, over
/// `UpdateResourceCollection`'s own wire path, so the monitored population a row needs is
/// persisted the way a client persists it.
async fn cover(emulator: &Emulator, stacks: &[&str], tag_entries: &[(&str, &[&str])]) {
    if stacks.is_empty() && tag_entries.is_empty() {
        return;
    }
    let reply = emulator
        .update_resource_collection("ADD", collection(stacks, tag_entries))
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"coverage {stacks:?} {tag_entries:?}\": UpdateResourceCollection must succeed, got {} {}",
        reply.status, reply.raw
    );
}

/// The insights the filter and agreement rows are asserted over: both types, both
/// statuses, so the tallies differ from each other and from the record count.
fn fixture_insights() -> Vec<InsightRecord> {
    vec![
        insight("p-open-1", PROACTIVE, ONGOING, None, &["alpha"], &[], &[]),
        insight("p-open-2", PROACTIVE, ONGOING, None, &["beta"], &[], &[]),
        insight("p-closed", PROACTIVE, CLOSED, Some(3_600_000), &["alpha"], &[], &[]),
        insight("r-open-1", REACTIVE, ONGOING, None, &[], &[("boundary", "prod")], &[]),
        insight("r-closed", REACTIVE, CLOSED, Some(7_200_000), &["beta"], &[], &[]),
    ]
}

/// The anomalies those rows are asserted over: three distinct metrics.
fn fixture_anomalies() -> Vec<AnomalyRecord> {
    vec![
        metric_anomaly("a-1", "AWS/Lambda", "Duration"),
        metric_anomaly("a-2", "AWS/Lambda", "Errors"),
        metric_anomaly("a-3", "AWS/RDS", "CPUUtilization"),
    ]
}

/// The figures [`fixture_insights`] and [`fixture_anomalies`] over the coverage
/// [`administered_emulator`] declares add up to: two open proactive and one open reactive
/// insight, three distinct metrics, and one resource hour per monitored resource (two
/// stacks plus two tag values).
fn fixture_figures() -> Figures {
    expected(2, 1, 3, 4)
}

/// An emulator whose organization is administered by the account it serves, holding the
/// fixture records: the only state in which the REPORTING path is reachable.
async fn administered_emulator() -> Emulator {
    let emulator = Emulator::new();
    emulator.delegate_organization_administration(&served_account());
    cover(
        &emulator,
        &["alpha", "beta"],
        &[("boundary", &["prod", "staging"])],
    )
    .await;
    emulator.seed_insights(fixture_insights());
    emulator.seed_anomalies(fixture_anomalies());
    emulator
}

/// Every shape of scope the operation accepts, as request bodies.
fn every_scope_shape(account: &str, organizational_unit: &str) -> Vec<Value> {
    vec![
        // The recorded call: no filter at all.
        json!({}),
        json!({ "AccountIds": [] }),
        json!({ "OrganizationalUnitIds": [] }),
        json!({ "AccountIds": [account] }),
        json!({ "OrganizationalUnitIds": [organizational_unit] }),
        json!({ "AccountIds": [account], "OrganizationalUnitIds": [organizational_unit] }),
    ]
}

/// RULE 1: an organization-wide read is available to the account that administers DevOps
/// Guru for the organization alone, so a caller that administers none is refused with HTTP
/// 403 `AccessDeniedException` — whatever scope the request carries and whatever the store
/// holds — and the refusal reports none of the figures.
///
/// The rows differ along the shape of the scope, how many ids it names, and whether the
/// store holds analysis output to tally.
#[tokio::test]
async fn health_is_refused_to_a_caller_that_administers_no_organization() {
    let served = served_account();
    let mut bodies = every_scope_shape(&served, ORGANIZATIONAL_UNITS[0]);
    bodies.push(json!({ "AccountIds": OTHER_ACCOUNTS }));
    bodies.push(json!({
        "AccountIds": [served, OTHER_ACCOUNTS[0]],
        "OrganizationalUnitIds": ORGANIZATIONAL_UNITS,
    }));

    for seeded in [false, true] {
        for body in &bodies {
            let emulator = Emulator::new();
            if seeded {
                cover(&emulator, &["alpha"], &[("boundary", &["prod"])]).await;
                emulator.seed_insights(fixture_insights());
                emulator.seed_anomalies(fixture_anomalies());
            }
            let case = format!("DescribeOrganizationHealth({body}) seeded={seeded}");

            let reply = describe_organization_health(&emulator, body).await;
            assert_eq!(
                reply.status, 403,
                "case {case:?}: expected HTTP 403 for a caller that administers no \
                 organization, got {} {}",
                reply.status, reply.raw
            );
            assert!(
                reply.names_error("AccessDeniedException"),
                "case {case:?}: expected AccessDeniedException, got {}",
                reply.raw
            );
            assert_eq!(
                figures(&reply),
                NO_FIGURES,
                "case {case:?}: a refusal must carry no figures, got {}",
                reply.raw
            );
        }
    }
}

/// One row of the tally table: the case name, the coverage declared, the insights and the
/// anomalies seeded, and the four figures expected.
struct TallyCase<'a> {
    name: &'a str,
    stacks: &'a [&'a str],
    tag_entries: &'a [(&'a str, &'a [&'a str])],
    insights: Vec<InsightRecord>,
    anomalies: Vec<AnomalyRecord>,
    figures: Figures,
}

/// RULE 2: each figure is a tally over the records the store holds — the `ONGOING`
/// insights of each type, the distinct metrics the stored anomalies name, and one
/// resource-analysis hour per monitored resource of the coverage — so a figure is zero
/// only while the set it tallies is empty.
///
/// The rows differ along which set is non-empty, its size, the insight types and statuses
/// held, which half of the coverage is declared, and how the metrics are spread over
/// anomalies.
#[tokio::test]
async fn the_figures_are_the_tallies_over_the_store() {
    let cases = vec![
        TallyCase {
            name: "an empty store: every set is empty",
            stacks: &[],
            tag_entries: &[],
            insights: vec![],
            anomalies: vec![],
            figures: expected(0, 0, 0, 0),
        },
        TallyCase {
            name: "coverage only: resource hours, nothing analysed",
            stacks: &["alpha", "beta", "gamma"],
            tag_entries: &[],
            insights: vec![],
            anomalies: vec![],
            figures: expected(0, 0, 0, 3),
        },
        TallyCase {
            name: "tag coverage counts one resource per (key, value) pair",
            stacks: &[],
            tag_entries: &[("boundary", &["prod", "staging", "dev"])],
            insights: vec![],
            anomalies: vec![],
            figures: expected(0, 0, 0, 3),
        },
        TallyCase {
            name: "both halves of the coverage",
            stacks: &["alpha", "beta"],
            tag_entries: &[("boundary", &["prod"])],
            insights: vec![],
            anomalies: vec![],
            figures: expected(0, 0, 0, 3),
        },
        TallyCase {
            name: "one open insight of each type, no coverage",
            stacks: &[],
            tag_entries: &[],
            insights: vec![
                insight("p-1", PROACTIVE, ONGOING, None, &[], &[], &[]),
                insight("r-1", REACTIVE, ONGOING, None, &[], &[], &[]),
            ],
            anomalies: vec![],
            figures: expected(1, 1, 0, 0),
        },
        TallyCase {
            name: "a closed insight is not open, whatever its type",
            stacks: &[],
            tag_entries: &[],
            insights: vec![
                insight("p-1", PROACTIVE, CLOSED, Some(60_000), &[], &[], &[]),
                insight("r-1", REACTIVE, CLOSED, Some(60_000), &[], &[], &[]),
            ],
            anomalies: vec![],
            figures: expected(0, 0, 0, 0),
        },
        TallyCase {
            name: "three open proactive insights and one open reactive one",
            stacks: &[],
            tag_entries: &[],
            insights: vec![
                insight("p-1", PROACTIVE, ONGOING, None, &[], &[], &[]),
                insight("p-2", PROACTIVE, ONGOING, None, &[], &[], &[]),
                insight("p-3", PROACTIVE, ONGOING, None, &[], &[], &[]),
                insight("r-1", REACTIVE, ONGOING, None, &[], &[], &[]),
                insight("r-2", REACTIVE, CLOSED, Some(60_000), &[], &[], &[]),
            ],
            anomalies: vec![],
            figures: expected(3, 1, 0, 0),
        },
        TallyCase {
            name: "anomalies only: one metric each",
            stacks: &[],
            tag_entries: &[],
            insights: vec![],
            anomalies: vec![
                metric_anomaly("a-1", "AWS/Lambda", "Duration"),
                metric_anomaly("a-2", "AWS/RDS", "CPUUtilization"),
            ],
            figures: expected(0, 0, 2, 0),
        },
        TallyCase {
            name: "an anomaly naming no metric analyses none",
            stacks: &[],
            tag_entries: &[],
            insights: vec![],
            anomalies: vec![AnomalyRecord {
                id: "a-bare".to_string(),
                severity: "LOW".to_string(),
                status: ONGOING.to_string(),
                ..AnomalyRecord::default()
            }],
            figures: expected(0, 0, 0, 0),
        },
        TallyCase {
            name: "every set non-empty at once: the fixture store",
            stacks: &["alpha", "beta"],
            tag_entries: &[("boundary", &["prod", "staging"])],
            insights: fixture_insights(),
            anomalies: fixture_anomalies(),
            figures: fixture_figures(),
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        emulator.delegate_organization_administration(&served_account());
        cover(&emulator, case.stacks, case.tag_entries).await;
        emulator.seed_insights(case.insights);
        emulator.seed_anomalies(case.anomalies);

        let reply = describe_organization_health(&emulator, &json!({})).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: the report must be served to the administering caller, got {} {}",
            case.name, reply.status, reply.raw
        );
        assert_eq!(
            figures(&reply),
            case.figures,
            "case {:?}: expected {:?} as (OpenProactiveInsights, OpenReactiveInsights, \
             MetricsAnalyzed, ResourceHours), got {}",
            case.name,
            case.figures,
            reply.raw
        );
    }
}

/// One row of the filter table: the case name, the `AccountIds` and the
/// `OrganizationalUnitIds` the request sends (`None` meaning the member is absent), and
/// whether the scope holds the one member account the store carries.
type FilterCase<'a> = (&'a str, Option<Vec<String>>, Option<Vec<String>>, bool);

/// RULE 3: the two filters NARROW the report to the member accounts they name — an account
/// is in scope exactly while every filter the request carries selects it — and a filter
/// naming none restricts nothing. The store records no organizational-unit membership for
/// the account it serves, so a unit filter leaves no member in scope and every figure is
/// then zero.
///
/// The rows differ along which filter is sent, how many ids it names, whether the served
/// account is among them, and whether the list is empty.
#[tokio::test]
async fn the_account_and_unit_filters_narrow_the_report() {
    let served = served_account();
    let cases: Vec<FilterCase> = vec![
        ("no filter at all", None, None, true),
        (
            "AccountIds naming the served account",
            Some(vec![served.clone()]),
            None,
            true,
        ),
        (
            "AccountIds naming another account",
            Some(vec![OTHER_ACCOUNTS[0].to_string()]),
            None,
            false,
        ),
        (
            "AccountIds naming four other accounts",
            Some(OTHER_ACCOUNTS.iter().map(ToString::to_string).collect()),
            None,
            false,
        ),
        (
            "AccountIds at the modelled maximum of five, the served account among them",
            Some(
                OTHER_ACCOUNTS
                    .iter()
                    .map(ToString::to_string)
                    .chain([served.clone()])
                    .collect(),
            ),
            None,
            true,
        ),
        (
            "an empty AccountIds list restricts nothing",
            Some(vec![]),
            None,
            true,
        ),
        (
            "OrganizationalUnitIds naming a unit the member belongs to none of",
            None,
            Some(vec![ORGANIZATIONAL_UNITS[0].to_string()]),
            false,
        ),
        (
            "OrganizationalUnitIds naming two units",
            None,
            Some(ORGANIZATIONAL_UNITS.iter().map(ToString::to_string).collect()),
            false,
        ),
        (
            "an empty OrganizationalUnitIds list restricts nothing",
            None,
            Some(vec![]),
            true,
        ),
        (
            "both filters: the account matches, the unit does not",
            Some(vec![served.clone()]),
            Some(vec![ORGANIZATIONAL_UNITS[1].to_string()]),
            false,
        ),
        (
            "both filters, both naming nothing",
            Some(vec![]),
            Some(vec![]),
            true,
        ),
    ];

    for (case, account_ids, organizational_unit_ids, in_scope) in cases {
        let emulator = administered_emulator().await;
        let mut body = json!({});
        if let Some(account_ids) = account_ids {
            body["AccountIds"] = json!(account_ids);
        }
        if let Some(organizational_unit_ids) = organizational_unit_ids {
            body["OrganizationalUnitIds"] = json!(organizational_unit_ids);
        }

        let reply = describe_organization_health(&emulator, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the report must be served to the administering caller, got {} {}",
            reply.status, reply.raw
        );
        let wanted = if in_scope {
            fixture_figures()
        } else {
            expected(0, 0, 0, 0)
        };
        assert_eq!(
            figures(&reply),
            wanted,
            "case {case:?}: expected {wanted:?}, got {}",
            reply.raw
        );
    }
}

/// One row of the agreement table: the case name, the coverage declared, and the insights
/// and anomalies seeded.
type AgreementCase<'a> = (
    &'a str,
    &'a [&'a str],
    &'a [(&'a str, &'a [&'a str])],
    Vec<InsightRecord>,
    Vec<AnomalyRecord>,
);

/// RULE 2's agreement half: the organization-wide report over the one member account this
/// store holds reports the same four figures as `DescribeAccountHealth` over the same
/// store, because both are tallies over the same records.
///
/// The rows differ along what the store holds, so the agreement is asserted on several
/// different sets of figures rather than only on zeros.
#[tokio::test]
async fn the_report_agrees_with_the_account_health_read_over_the_same_store() {
    // (case, stacks, tag entries, insights, anomalies)
    let cases: Vec<AgreementCase> = vec![
        ("an empty store", &[], &[], vec![], vec![]),
        (
            "coverage only",
            &["alpha", "beta"],
            &[],
            vec![],
            vec![],
        ),
        (
            "the fixture store",
            &["alpha", "beta"],
            &[("boundary", &["prod", "staging"])],
            fixture_insights(),
            fixture_anomalies(),
        ),
    ];

    for (case, stacks, tag_entries, insights, anomalies) in cases {
        let emulator = Emulator::new();
        emulator.delegate_organization_administration(&served_account());
        cover(&emulator, stacks, tag_entries).await;
        emulator.seed_insights(insights);
        emulator.seed_anomalies(anomalies);

        let organization = describe_organization_health(&emulator, &json!({})).await;
        let account = describe_account_health(&emulator).await;
        assert_eq!(
            account.status, 200,
            "case {case:?}: DescribeAccountHealth must succeed, got {} {}",
            account.status, account.raw
        );
        assert_eq!(
            figures(&organization),
            figures(&account),
            "case {case:?}: the organization-wide figures {:?} must equal the account's {:?}",
            figures(&organization),
            figures(&account)
        );
    }
}

/// RULE 4: the report is a pure read, so reading it changes nothing a later caller sees:
/// repeating it answers the same figures, a narrowed read does not disturb the full one,
/// and the coverage and insights are still exactly what they were afterwards.
#[tokio::test]
async fn reading_the_report_changes_nothing_a_later_caller_sees() {
    let emulator = administered_emulator().await;
    let narrowed = json!({ "AccountIds": [OTHER_ACCOUNTS[0]] });

    let first = describe_organization_health(&emulator, &json!({})).await;
    let narrow = describe_organization_health(&emulator, &narrowed).await;
    let again = describe_organization_health(&emulator, &json!({})).await;

    assert_eq!(
        figures(&first),
        fixture_figures(),
        "case \"first read\": expected {:?}, got {}",
        fixture_figures(),
        first.raw
    );
    assert_eq!(
        figures(&narrow),
        expected(0, 0, 0, 0),
        "case \"narrowed read\": a scope holding no member reports zeros, got {}",
        narrow.raw
    );
    assert_eq!(
        figures(&again),
        figures(&first),
        "case \"repeated read\": expected {:?}, got {}",
        figures(&first),
        again.raw
    );

    // The coverage the reads reported on is still the coverage that was declared.
    let coverage = emulator
        .get_resource_collection("AWS_CLOUD_FORMATION", None)
        .await;
    assert_eq!(
        coverage.status, 200,
        "case \"coverage afterwards\": the read must succeed, got {} {}",
        coverage.status, coverage.raw
    );
    let mut stacks = coverage.strings_at("/ResourceCollection/CloudFormation/StackNames");
    stacks.sort();
    assert_eq!(
        stacks,
        vec!["alpha".to_string(), "beta".to_string()],
        "case \"coverage afterwards\": the declared stacks must be unchanged, got {}",
        coverage.raw
    );

    // And every seeded insight is still stored, so no status was moved and no count
    // consumed by the reads above.
    let listed = emulator
        .call(
            "POST",
            "/insights",
            Some(&json!({
                "StatusFilter": { "Any": { "Type": PROACTIVE, "StartTimeRange": {} } }
            })),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case \"listing afterwards\": the listing must succeed, got {} {}",
        listed.status, listed.raw
    );
    let mut ids: Vec<String> = listed
        .at("/ProactiveInsights")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "p-closed".to_string(),
            "p-open-1".to_string(),
            "p-open-2".to_string(),
        ],
        "case \"listing afterwards\": every seeded proactive insight must still be stored, got {}",
        listed.raw
    );
}
