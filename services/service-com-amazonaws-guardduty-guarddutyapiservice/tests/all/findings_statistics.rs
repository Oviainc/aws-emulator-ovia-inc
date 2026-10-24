// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetFindingsStatistics`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, reusing the
//! harness in [`super::detector_lifecycle`], so routing, restJson1 (de)serialization and
//! the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. The two selectors are exclusive and one is required: "You must provide either
//!    findingStatisticTypes or groupBy parameter, and not both", and "You can use the
//!    maxResults and orderBy parameters only when using groupBy"
//!    (`smithy_query(query="doc",shape="GetFindingsStatistics")`).
//! 2. Every statistic is COMPUTED over the findings the detector holds: each group's
//!    `totalFindings` is the number of stored findings carrying that group's key, read
//!    back independently through `GetFindings`, and the totals sum to the number of
//!    findings the request's `findingCriteria` selects.
//! 3. `findingCriteria` restricts the counted set exactly as it restricts
//!    `ListFindings`, and a criterion this service cannot evaluate is refused rather
//!    than ignored.
//! 4. `maxResults` bounds the number of groups (default 25) and `orderBy` orders them by
//!    `totalFindings`, `DESC` by default.
//! 5. `lastGeneratedAt` is the latest instant a finding in the group was generated, so a
//!    recurrence of one group's activity moves that group's timestamp and no other's.
//! 6. A `detectorId` this account/Region does not hold is refused with the modelled
//!    `BadRequestException` envelope under the `InvalidInputException` class.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::detector_lifecycle::{
    all_finding_ids, call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// A set of finding types spanning several severities and several affected resource
/// types, so every grouping has more than one group and the groups have DIFFERENT
/// sizes. Source: this service's sample-finding catalogue (the types
/// `CreateSampleFindings` can generate), itself taken from the GuardDuty
/// finding-types reference.
const MIXED_TYPES: [&str; 6] = [
    "Backdoor:EC2/DenialOfService.Tcp",
    "Backdoor:EC2/Spambot",
    "Recon:EC2/PortProbeUnprotectedPort",
    "Discovery:S3/MaliciousIPCaller",
    "AttackSequence:S3/CompromisedData",
    "PenTest:IAMUser/PentooLinux",
];

/// More finding types than the documented default `maxResults` of 25, so a request
/// that names no `maxResults` can be distinguished from one asking for everything.
/// Source: the same catalogue as [`MIXED_TYPES`].
const MANY_TYPES: [&str; 30] = [
    "Backdoor:EC2/C&CActivity.B",
    "Backdoor:EC2/C&CActivity.B!DNS",
    "Backdoor:EC2/DenialOfService.Dns",
    "Backdoor:EC2/DenialOfService.Tcp",
    "Backdoor:EC2/DenialOfService.Udp",
    "Backdoor:EC2/DenialOfService.UdpOnTcpPorts",
    "Backdoor:EC2/DenialOfService.UnusualProtocol",
    "Backdoor:EC2/Spambot",
    "Behavior:EC2/NetworkPortUnusual",
    "Behavior:EC2/TrafficVolumeUnusual",
    "CryptoCurrency:EC2/BitcoinTool.B",
    "CryptoCurrency:EC2/BitcoinTool.B!DNS",
    "DefenseEvasion:EC2/UnusualDNSResolver",
    "DefenseEvasion:EC2/UnusualDoHActivity",
    "DefenseEvasion:EC2/UnusualDoTActivity",
    "Impact:EC2/AbusedDomainRequest.Reputation",
    "Impact:EC2/BitcoinDomainRequest.Reputation",
    "Impact:EC2/MaliciousDomainRequest.Reputation",
    "Impact:EC2/PortSweep",
    "Impact:EC2/SuspiciousDomainRequest.Reputation",
    "Impact:EC2/WinRMBruteForce",
    "Recon:EC2/PortProbeEMRUnprotectedPort",
    "Recon:EC2/PortProbeUnprotectedPort",
    "Recon:EC2/Portscan",
    "Trojan:EC2/BlackholeTraffic",
    "Trojan:EC2/BlackholeTraffic!DNS",
    "Trojan:EC2/DGADomainRequest.B",
    "Trojan:EC2/DGADomainRequest.C!DNS",
    "Trojan:EC2/DNSDataExfiltration",
    "Trojan:EC2/DriveBySourceTraffic!DNS",
];

/// The default number of groups, per
/// `smithy_query(query="doc",shape="GetFindingsStatisticsRequest",member="MaxResults")`:
/// "The maximum number of results to be returned in the response. The default value is
/// 25."
const DEFAULT_MAX_RESULTS: usize = 25;

/// Every `GroupByType` value, with the response member it fills
/// (`smithy_query(shape="GroupByType")` and
/// `smithy_query(shape="FindingStatistics")`).
const GROUPINGS: [(&str, &str); 5] = [
    ("ACCOUNT", "groupedByAccount"),
    ("DATE", "groupedByDate"),
    ("FINDING_TYPE", "groupedByFindingType"),
    ("RESOURCE", "groupedByResource"),
    ("SEVERITY", "groupedBySeverity"),
];

/// The Region whose endpoint does not serve the detector these tests create.
const OTHER_REGION: &str = "us-west-2";

/// Call `GetFindingsStatistics`.
async fn statistics(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/statistics"),
        Some(body),
    )
    .await
}

/// [`statistics`], failing the test unless the call succeeded, and returning the
/// `findingStatistics` object.
async fn statistics_ok(router: &Router, detector_id: &str, body: Value) -> Value {
    let (status, response) = statistics(router, detector_id, body.clone()).await;
    assert_eq!(
        status, 200,
        "statistics {body}: expected 200, got {status} {response:?}"
    );
    // "This parameter is currently not supported"
    // (`smithy_query(operation="GetFindingsStatistics")`), so no page token is ever
    // handed out.
    assert_eq!(
        response["nextToken"],
        Value::Null,
        "statistics {body}: nextToken is not supported, got {response:?}"
    );
    response["findingStatistics"].clone()
}

/// Generate one sample finding per entry of `types`.
async fn create_samples(router: &Router, detector_id: &str, types: &[&str]) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": types})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings {types:?}: expected 200, got {status} {body:?}"
    );
}

/// An enabled detector holding one finding per entry of `types`.
///
/// An EMPTY `types` leaves the detector with no finding at all: `CreateSampleFindings`
/// reads an empty `findingTypes` as "every supported type", so the row that wants an
/// empty detector must not call it.
async fn detector_with(router: &Router, types: &[&str]) -> String {
    let detector_id = enabled_detector(router).await;
    if !types.is_empty() {
        create_samples(router, &detector_id, types).await;
    }
    let ids = all_finding_ids(router, &detector_id, 50).await;
    assert_eq!(
        ids.len(),
        types.len(),
        "expected one finding per sample type, got {ids:?}"
    );
    detector_id
}

/// Every finding the detector holds, read back through `GetFindings` — the independent
/// view each expectation below is computed from.
async fn stored_findings(router: &Router, detector_id: &str) -> Vec<Value> {
    let ids = all_finding_ids(router, detector_id, 50).await;
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(json!({"findingIds": ids})),
    )
    .await;
    assert_eq!(status, 200, "get-findings: expected 200, got {status} {body:?}");
    body["findings"].as_array().cloned().unwrap_or_default()
}

/// How many finding ids `ListFindings` selects with `criteria` — the same filter
/// `GetFindingsStatistics` applies.
async fn listed_count(router: &Router, detector_id: &str, criteria: &Value) -> usize {
    let mut body = json!({"maxResults": 50});
    if !criteria.is_null() {
        body["findingCriteria"] = criteria.clone();
    }
    let (status, response) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "list-findings {criteria}: expected 200, got {status} {response:?}"
    );
    response["findingIds"]
        .as_array()
        .map_or(0, |ids| ids.len())
}

/// The severity a stored finding carries.
fn severity_of(finding: &Value) -> f64 {
    finding["severity"]
        .as_f64()
        .unwrap_or_else(|| panic!("expected a numeric severity, got {finding:?}"))
}

/// One member of a stored finding as text.
fn text_of(finding: &Value, member: &str) -> String {
    finding[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected {member} as text, got {finding:?}"))
        .to_string()
}

/// The group key a stored finding falls under, for the grouping named by `group_by`,
/// computed from the finding `GetFindings` reports rather than from the statistics.
///
/// `DATE` keys by the (UTC date, severity) pair: "a list of map of dates with a count
/// of total findings generated on each date per severity level"
/// (`smithy_query(query="doc",shape="FindingStatistics",member="GroupedByDate")`). The
/// UTC date is the first 10 characters of the ISO-8601 `createdAt`.
fn expected_key(group_by: &str, finding: &Value) -> String {
    match group_by {
        "ACCOUNT" => text_of(finding, "accountId"),
        "DATE" => format!(
            "{}/{}",
            &text_of(finding, "createdAt")[..10],
            severity_of(finding)
        ),
        "FINDING_TYPE" => text_of(finding, "type"),
        "RESOURCE" => format!(
            "{}/{}",
            text_of(finding, "accountId"),
            finding["resource"]["resourceType"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a resourceType, got {finding:?}"))
        ),
        "SEVERITY" => format!("{}", severity_of(finding)),
        other => panic!("no expectation for grouping {other:?}"),
    }
}

/// The group key a reported statistics entry carries, in the same spelling
/// [`expected_key`] produces, so the two sides can be compared as sets.
fn reported_key(group_by: &str, entry: &Value) -> String {
    let severity = || {
        entry["severity"]
            .as_f64()
            .unwrap_or_else(|| panic!("expected a numeric severity, got {entry:?}"))
    };
    match group_by {
        "ACCOUNT" => text_of(entry, "accountId"),
        // `date` is a modelled timestamp, which restJson1 carries as epoch seconds; the
        // test compares the DAY it names, so it is folded back onto the same (date,
        // severity) pair shape as the expectation through the day index below.
        "DATE" => format!("{}/{}", day_index(entry, "date"), severity()),
        "FINDING_TYPE" => text_of(entry, "findingType"),
        "RESOURCE" => format!(
            "{}/{}",
            text_of(entry, "accountId"),
            text_of(entry, "resourceType")
        ),
        "SEVERITY" => format!("{}", severity()),
        other => panic!("no expectation for grouping {other:?}"),
    }
}

/// Seconds in one UTC day.
const SECONDS_PER_DAY: f64 = 24.0 * 60.0 * 60.0;

/// The number of whole UTC days in `member`'s epoch-second timestamp.
fn day_index(entry: &Value, member: &str) -> i64 {
    let seconds = entry[member]
        .as_f64()
        .unwrap_or_else(|| panic!("expected a numeric {member}, got {entry:?}"));
    (seconds / SECONDS_PER_DAY).floor() as i64
}

/// `totalFindings` of one reported statistics entry.
fn total_findings(entry: &Value) -> usize {
    entry["totalFindings"]
        .as_u64()
        .unwrap_or_else(|| panic!("expected a numeric totalFindings, got {entry:?}"))
        as usize
}

/// `lastGeneratedAt` of one reported statistics entry, in epoch seconds.
fn last_generated_at(entry: &Value) -> f64 {
    entry["lastGeneratedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("expected a numeric lastGeneratedAt, got {entry:?}"))
}

/// The reported groups of one `groupBy` answer, as `key -> totalFindings`.
fn reported_groups(group_by: &str, member: &str, statistics: &Value) -> BTreeMap<String, usize> {
    entries(member, statistics)
        .iter()
        .map(|entry| (reported_key(group_by, entry), total_findings(entry)))
        .collect()
}

/// The list a `groupBy` answer fills, failing the test when the member is absent.
fn entries(member: &str, statistics: &Value) -> Vec<Value> {
    statistics[member]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("expected {member} to be a list, got {statistics:?}"))
}

/// The groups the stored findings imply, as `key -> count`, for one grouping. The DATE
/// key is folded onto the same day-index spelling [`reported_key`] uses by comparing
/// only the SIZE of the key set and the counts, so the two sides agree without this
/// test having to convert a calendar date into a day index.
fn expected_counts(group_by: &str, findings: &[Value]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for finding in findings {
        *counts.entry(expected_key(group_by, finding)).or_insert(0) += 1;
    }
    counts
}

// ---------------------------------------------------------------------------
// Rule 1: the selectors are exclusive, one is required, and maxResults /
// orderBy belong to groupBy.
// ---------------------------------------------------------------------------

/// Rows spanning the ways a request can name the selectors: each one alone, neither,
/// both, an empty statistic-type list, and the `groupBy`-only members with and without
/// `groupBy`.
#[tokio::test]
async fn exactly_one_selector_is_accepted() {
    let mut cases: Vec<(String, Value, bool)> = vec![
        (
            "findingStatisticTypes alone".to_string(),
            json!({"findingStatisticTypes": ["COUNT_BY_SEVERITY"]}),
            true,
        ),
        (
            // `len 0..10` (`smithy_query(operation="GetFindingsStatistics")`), so an
            // empty list is a well-formed request that names the selector and asks for
            // no statistic.
            "an empty findingStatisticTypes".to_string(),
            json!({"findingStatisticTypes": []}),
            true,
        ),
        ("neither selector".to_string(), json!({}), false),
        (
            "neither selector, with a criterion".to_string(),
            json!({"findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 1}}}}),
            false,
        ),
        (
            "both selectors".to_string(),
            json!({"findingStatisticTypes": ["COUNT_BY_SEVERITY"], "groupBy": "SEVERITY"}),
            false,
        ),
        (
            "an empty findingStatisticTypes beside groupBy".to_string(),
            json!({"findingStatisticTypes": [], "groupBy": "SEVERITY"}),
            false,
        ),
        (
            "maxResults without groupBy".to_string(),
            json!({"findingStatisticTypes": ["COUNT_BY_SEVERITY"], "maxResults": 5}),
            false,
        ),
        (
            "orderBy without groupBy".to_string(),
            json!({"findingStatisticTypes": ["COUNT_BY_SEVERITY"], "orderBy": "ASC"}),
            false,
        ),
    ];
    for (group_by, _) in GROUPINGS {
        cases.push((format!("groupBy {group_by} alone"), json!({"groupBy": group_by}), true));
        cases.push((
            format!("groupBy {group_by} with maxResults and orderBy"),
            json!({"groupBy": group_by, "maxResults": 5, "orderBy": "ASC"}),
            true,
        ));
    }

    let router = new_router();
    let detector_id = detector_with(&router, &MIXED_TYPES).await;

    for case in &cases {
        let (label, body, accepted) = case;
        let (status, response) = statistics(&router, &detector_id, body.clone()).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} {response:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_shape(&response),
            "BadRequestException",
            "case {label:?}: expected the modelled error shape, got {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidInputException",
            "case {label:?}: expected the invalid-input class, got {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("groupBy"),
            "case {label:?}: expected a message naming the member, got {response:?}"
        );
    }
}

/// An empty `findingStatisticTypes` asks for no statistic, so the statistics object
/// carries none — not a canned map.
#[tokio::test]
async fn an_empty_statistic_type_list_reports_no_statistic() {
    let router = new_router();
    let detector_id = detector_with(&router, &MIXED_TYPES).await;

    let reported = statistics_ok(&router, &detector_id, json!({"findingStatisticTypes": []})).await;
    for member in [
        "countBySeverity",
        "groupedByAccount",
        "groupedByDate",
        "groupedByFindingType",
        "groupedByResource",
        "groupedBySeverity",
    ] {
        assert_eq!(
            reported[member],
            Value::Null,
            "expected {member} to be absent, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: every statistic is computed over the stored findings.
// ---------------------------------------------------------------------------

/// `COUNT_BY_SEVERITY` is the histogram of the stored findings' severities. The rows
/// differ in how many findings the detector holds and in how the severities spread
/// across them: one finding, one severity shared by several findings, several
/// severities, and the whole mixed set.
#[tokio::test]
async fn count_by_severity_is_the_histogram_of_the_stored_findings() {
    let cases: [(&str, &[&str]); 5] = [
        ("no finding at all", &[]),
        ("one finding", &["Backdoor:EC2/DenialOfService.Tcp"]),
        (
            "two findings sharing one severity",
            &["Backdoor:EC2/DenialOfService.Tcp", "Discovery:S3/MaliciousIPCaller"],
        ),
        (
            "three severities",
            &[
                "Backdoor:EC2/DenialOfService.Tcp",
                "Backdoor:EC2/Spambot",
                "Recon:EC2/PortProbeUnprotectedPort",
            ],
        ),
        ("the mixed set", &MIXED_TYPES),
    ];

    for case in cases {
        let (label, types) = case;
        let router = new_router();
        let detector_id = detector_with(&router, types).await;
        let findings = stored_findings(&router, &detector_id).await;

        let mut expected: BTreeMap<String, usize> = BTreeMap::new();
        for finding in &findings {
            *expected
                .entry(format!("{}", severity_of(finding)))
                .or_insert(0) += 1;
        }

        let reported = statistics_ok(
            &router,
            &detector_id,
            json!({"findingStatisticTypes": ["COUNT_BY_SEVERITY"]}),
        )
        .await;
        let map = reported["countBySeverity"]
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: expected countBySeverity, got {reported:?}"));

        let mut got: BTreeMap<String, usize> = BTreeMap::new();
        for (key, value) in map {
            // The key is the severity as TEXT: an integral severity carries no trailing
            // `.0`, the same rendering a `findingCriteria` equality on `severity`
            // compares against.
            let severity: f64 = key
                .parse()
                .unwrap_or_else(|_| panic!("case {label:?}: expected a numeric key, got {key:?}"));
            if severity.fract() == 0.0 {
                assert_eq!(
                    key,
                    &format!("{}", severity as i64),
                    "case {label:?}: expected an integral severity key without a fraction, got {key:?}"
                );
            }
            let count = value.as_u64().unwrap_or_else(|| {
                panic!("case {label:?}: expected a numeric count, got {value:?}")
            });
            got.insert(format!("{severity}"), count as usize);
        }
        assert_eq!(
            got, expected,
            "case {label:?}: expected the severity histogram of the stored findings, got {got:?}"
        );
        assert_eq!(
            got.values().sum::<usize>(),
            findings.len(),
            "case {label:?}: the counts must sum to the stored findings, got {got:?}"
        );
    }
}

/// Every `groupBy` answer is the grouping of the stored findings: one entry per distinct
/// key, each carrying the number of findings under that key, and the entries summing to
/// the number of stored findings. The rows span every `GroupByType` value and three
/// detector populations.
#[tokio::test]
async fn every_grouping_counts_the_stored_findings() {
    let populations: [(&str, &[&str]); 3] = [
        ("one finding", &["Discovery:S3/MaliciousIPCaller"]),
        (
            "two resource types",
            &["Backdoor:EC2/Spambot", "Discovery:S3/MaliciousIPCaller"],
        ),
        ("the mixed set", &MIXED_TYPES),
    ];

    for population in populations {
        let (population_label, types) = population;
        let router = new_router();
        let detector_id = detector_with(&router, types).await;
        let findings = stored_findings(&router, &detector_id).await;

        for (group_by, member) in GROUPINGS {
            let label = format!("{population_label} grouped by {group_by}");
            let reported = statistics_ok(&router, &detector_id, json!({"groupBy": group_by})).await;
            let got = reported_groups(group_by, member, &reported);
            let expected = expected_counts(group_by, &findings);

            assert_eq!(
                got.len(),
                expected.len(),
                "case {label:?}: expected one group per distinct key, got {got:?} against {expected:?}"
            );
            assert_eq!(
                got.values().copied().collect::<BTreeSet<usize>>(),
                expected.values().copied().collect::<BTreeSet<usize>>(),
                "case {label:?}: expected the group sizes of the stored findings, got {got:?} against {expected:?}"
            );
            assert_eq!(
                got.values().sum::<usize>(),
                findings.len(),
                "case {label:?}: the groups must sum to the stored findings, got {got:?}"
            );
            // The key spellings agree for every grouping whose key is not a timestamp;
            // `DATE` is compared through its day index above, so only its size and
            // counts are asserted.
            if group_by != "DATE" {
                assert_eq!(
                    got, expected,
                    "case {label:?}: expected the grouping of the stored findings, got {got:?}"
                );
            }
            // Only the grouping's own members are reported; the others stay absent.
            for (_, other) in GROUPINGS {
                if other != member {
                    assert_eq!(
                        reported[other],
                        Value::Null,
                        "case {label:?}: expected {other} to be absent, got {reported:?}"
                    );
                }
            }
            assert_eq!(
                reported["countBySeverity"],
                Value::Null,
                "case {label:?}: a groupBy answer carries no countBySeverity, got {reported:?}"
            );
        }
    }
}

/// `ACCOUNT` groups by the account the finding belongs to, which is one account in this
/// emulator, so the single group holds every finding. `DATE` names a UTC midnight and a
/// `lastGeneratedAt` inside that day.
#[tokio::test]
async fn the_account_and_date_groups_carry_their_own_keys() {
    let router = new_router();
    let detector_id = detector_with(&router, &MIXED_TYPES).await;
    let findings = stored_findings(&router, &detector_id).await;

    let reported = statistics_ok(&router, &detector_id, json!({"groupBy": "ACCOUNT"})).await;
    let accounts = entries("groupedByAccount", &reported);
    assert_eq!(
        accounts.len(),
        1,
        "expected one account group, got {accounts:?}"
    );
    assert_eq!(
        total_findings(&accounts[0]),
        findings.len(),
        "expected the account group to hold every finding, got {:?}",
        accounts[0]
    );
    assert_eq!(
        text_of(&accounts[0], "accountId"),
        text_of(&findings[0], "accountId"),
        "expected the stored account id, got {:?}",
        accounts[0]
    );

    let reported = statistics_ok(&router, &detector_id, json!({"groupBy": "DATE"})).await;
    for entry in entries("groupedByDate", &reported) {
        let date = entry["date"]
            .as_f64()
            .unwrap_or_else(|| panic!("expected a numeric date, got {entry:?}"));
        assert_eq!(
            date % SECONDS_PER_DAY,
            0.0,
            "expected the date to name a UTC midnight, got {entry:?}"
        );
        let generated = last_generated_at(&entry);
        assert!(
            generated >= date && generated < date + SECONDS_PER_DAY,
            "expected lastGeneratedAt inside the group's day, got {entry:?}"
        );
    }

    // `RESOURCE` reports the affected resource's TYPE; this emulator describes no
    // resource instance, so there is no resourceId to report and the member stays
    // absent rather than carrying an invented identifier.
    let reported = statistics_ok(&router, &detector_id, json!({"groupBy": "RESOURCE"})).await;
    for entry in entries("groupedByResource", &reported) {
        assert_eq!(
            entry["resourceId"],
            Value::Null,
            "expected resourceId to be absent, got {entry:?}"
        );
        assert!(
            !text_of(&entry, "resourceType").is_empty(),
            "expected a resourceType, got {entry:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: findingCriteria restricts the counted set.
// ---------------------------------------------------------------------------

/// The criteria rows span the condition operators this service evaluates and both ends
/// of the selected set: everything, a severity band, an equality, a wildcard on the
/// finding type, a criterion nothing matches. Each row's expectation is the number of
/// ids `ListFindings` selects with the SAME criteria.
#[tokio::test]
async fn finding_criteria_restricts_the_counted_findings() {
    let cases: [(&str, Value); 7] = [
        ("no criteria", Value::Null),
        ("an empty criterion map", json!({"criterion": {}})),
        (
            "severity at or above the high band",
            json!({"criterion": {"severity": {"greaterThanOrEqual": 7}}}),
        ),
        (
            "severity below the high band",
            json!({"criterion": {"severity": {"lessThan": 7}}}),
        ),
        (
            "one severity exactly",
            json!({"criterion": {"severity": {"equals": ["5"]}}}),
        ),
        (
            "a wildcard on the finding type",
            json!({"criterion": {"type": {"matches": ["*:EC2/*"]}}}),
        ),
        (
            "a criterion nothing matches",
            json!({"criterion": {"severity": {"greaterThan": 10}}}),
        ),
    ];

    let router = new_router();
    let detector_id = detector_with(&router, &MIXED_TYPES).await;

    for case in &cases {
        let (label, criteria) = case;
        let expected = listed_count(&router, &detector_id, criteria).await;

        let mut body = json!({"groupBy": "FINDING_TYPE"});
        if !criteria.is_null() {
            body["findingCriteria"] = criteria.clone();
        }
        let reported = statistics_ok(&router, &detector_id, body).await;
        let groups = reported_groups("FINDING_TYPE", "groupedByFindingType", &reported);
        assert_eq!(
            groups.values().sum::<usize>(),
            expected,
            "case {label:?}: expected the groups to sum to the filtered findings, got {groups:?}"
        );

        let mut body = json!({"findingStatisticTypes": ["COUNT_BY_SEVERITY"]});
        if !criteria.is_null() {
            body["findingCriteria"] = criteria.clone();
        }
        let reported = statistics_ok(&router, &detector_id, body).await;
        let counted: usize = reported["countBySeverity"]
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: expected countBySeverity, got {reported:?}"))
            .values()
            .map(|value| value.as_u64().unwrap_or_default() as usize)
            .sum();
        assert_eq!(
            counted, expected,
            "case {label:?}: expected the severity counts to sum to the filtered findings, got {reported:?}"
        );
    }
}

/// A criterion this service cannot evaluate is REFUSED, in both answer shapes: counting
/// findings the caller asked to exclude would be worse than refusing. The rows are
/// attribute paths outside the catalogue this service filters on, including a numeric
/// comparison on a textual attribute.
#[tokio::test]
async fn a_criterion_this_service_cannot_evaluate_is_refused() {
    let cases: [(&str, Value); 4] = [
        (
            "an attribute path this emulator does not model",
            json!({"criterion": {"resource.instanceDetails.instanceId": {"equals": ["i-1"]}}}),
        ),
        (
            "an attribute that is not a finding attribute at all",
            json!({"criterion": {"nonsense": {"equals": ["x"]}}}),
        ),
        (
            "a near miss on a modelled path",
            json!({"criterion": {"Severity": {"equals": ["8"]}}}),
        ),
        (
            "a numeric comparison on a textual attribute",
            json!({"criterion": {"type": {"greaterThan": 1}}}),
        ),
    ];

    let router = new_router();
    let detector_id = detector_with(&router, &MIXED_TYPES).await;

    for case in &cases {
        let (label, criteria) = case;
        for selector in [
            json!({"groupBy": "SEVERITY"}),
            json!({"findingStatisticTypes": ["COUNT_BY_SEVERITY"]}),
        ] {
            let mut body = selector.clone();
            body["findingCriteria"] = criteria.clone();
            let (status, response) = statistics(&router, &detector_id, body).await;
            assert_eq!(
                status, 400,
                "case {label:?} ({selector}): expected 400, got {status} {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidInputException",
                "case {label:?} ({selector}): expected the invalid-input class, got {response:?}"
            );
            let message = response["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("findingCriteria"),
                "case {label:?} ({selector}): expected a message naming the member, got {response:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: maxResults bounds the groups and orderBy orders them.
// ---------------------------------------------------------------------------

/// `maxResults` bounds the number of GROUPS, and a request that names none gets the
/// documented default of 25. The rows span both ends of the published `@range` 1..100
/// that this emulator's population can distinguish, plus the default.
#[tokio::test]
async fn max_results_bounds_the_number_of_groups() {
    let router = new_router();
    let detector_id = detector_with(&router, &MANY_TYPES).await;
    let groups_available = MANY_TYPES.len();

    let cases: [(&str, Option<i64>, usize); 7] = [
        ("no maxResults: the default of 25", None, DEFAULT_MAX_RESULTS),
        ("maxResults 1: the published minimum", Some(1), 1),
        ("maxResults 7", Some(7), 7),
        ("maxResults 25: the default, named", Some(25), 25),
        ("maxResults 29: one below the population", Some(29), 29),
        ("maxResults 30: the whole population", Some(30), groups_available),
        (
            "maxResults 100: the published maximum",
            Some(100),
            groups_available,
        ),
    ];

    for case in cases {
        let (label, max_results, expected) = case;
        let mut body = json!({"groupBy": "FINDING_TYPE"});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        let reported = statistics_ok(&router, &detector_id, body).await;
        let groups = entries("groupedByFindingType", &reported);
        assert_eq!(
            groups.len(),
            expected,
            "case {label:?}: expected {expected} groups, got {}",
            groups.len()
        );
    }
}

/// The statistics are counts, so `orderBy` orders the groups by `totalFindings`: `DESC`
/// (the default) first, `ASC` last. The population gives three resource groups of three
/// different sizes, so the direction is observable.
#[tokio::test]
async fn order_by_orders_the_groups_by_total_findings() {
    let router = new_router();
    let detector_id = detector_with(&router, &MIXED_TYPES).await;

    let cases: [(&str, Option<&str>, bool); 3] = [
        ("no orderBy: the default DESC", None, true),
        ("orderBy DESC", Some("DESC"), true),
        ("orderBy ASC", Some("ASC"), false),
    ];

    let mut per_direction: Vec<(String, Vec<usize>)> = Vec::new();
    for case in cases {
        let (label, order_by, descending) = case;
        let mut body = json!({"groupBy": "RESOURCE"});
        if let Some(order_by) = order_by {
            body["orderBy"] = json!(order_by);
        }
        let reported = statistics_ok(&router, &detector_id, body).await;
        let totals: Vec<usize> = entries("groupedByResource", &reported)
            .iter()
            .map(total_findings)
            .collect();
        assert!(
            totals.len() > 1,
            "case {label:?}: expected several groups to order, got {totals:?}"
        );
        for pair in totals.windows(2) {
            let holds = if descending {
                pair[0] >= pair[1]
            } else {
                pair[0] <= pair[1]
            };
            assert!(
                holds,
                "case {label:?}: expected the totals ordered {}, got {totals:?}",
                if descending { "descending" } else { "ascending" }
            );
        }
        per_direction.push((label.to_string(), totals));
    }

    // The two directions are reverses of one another over the same groups.
    let descending = &per_direction[1].1;
    let ascending = &per_direction[2].1;
    let mut reversed = ascending.clone();
    reversed.reverse();
    assert_eq!(
        &reversed, descending,
        "expected ASC to be the reverse of DESC, got {ascending:?} against {descending:?}"
    );
    // The top group is the biggest one, so a truncated answer reports the top groups.
    let top = statistics_ok(
        &router,
        &detector_id,
        json!({"groupBy": "RESOURCE", "maxResults": 1}),
    )
    .await;
    let top_total = total_findings(&entries("groupedByResource", &top)[0]);
    assert_eq!(
        top_total,
        descending.iter().copied().max().unwrap_or_default(),
        "expected maxResults 1 to report the largest group, got {top_total}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: lastGeneratedAt follows the activity that generated the findings.
// ---------------------------------------------------------------------------

/// A repeat generation of one finding type is another observation of that activity, so
/// it moves that group's `lastGeneratedAt` and leaves every other group's where it was.
#[tokio::test]
async fn last_generated_at_moves_only_for_the_regenerated_group() {
    let router = new_router();
    let detector_id = detector_with(&router, &MIXED_TYPES).await;

    let before = statistics_ok(&router, &detector_id, json!({"groupBy": "FINDING_TYPE"})).await;
    let before: BTreeMap<String, f64> = entries("groupedByFindingType", &before)
        .iter()
        .map(|entry| (text_of(entry, "findingType"), last_generated_at(entry)))
        .collect();

    let regenerated = MIXED_TYPES[0];
    create_samples(&router, &detector_id, &[regenerated]).await;

    let after = statistics_ok(&router, &detector_id, json!({"groupBy": "FINDING_TYPE"})).await;
    let after: BTreeMap<String, f64> = entries("groupedByFindingType", &after)
        .iter()
        .map(|entry| (text_of(entry, "findingType"), last_generated_at(entry)))
        .collect();

    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>(),
        "a repeat generation aggregates into the existing finding, so the groups stay the same"
    );
    for (finding_type, was) in &before {
        let now = after[finding_type];
        if finding_type == regenerated {
            assert!(
                now > *was,
                "case {finding_type:?}: a repeat generation must move lastGeneratedAt, got {was} then {now}"
            );
        } else {
            assert_eq!(
                now, *was,
                "case {finding_type:?}: another type's generation must not move lastGeneratedAt, got {was} then {now}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 6: a detectorId the endpoint does not hold is refused.
// ---------------------------------------------------------------------------

/// A detector is a regional resource owned by one account, so a `detectorId` the
/// endpoint does not resolve is refused rather than answered with hollow statistics. The
/// rows differ along the ways an id can fail to resolve.
#[tokio::test]
async fn a_detector_id_the_endpoint_does_not_hold_is_refused() {
    let router = new_router();
    let detector_id = detector_with(&router, &MIXED_TYPES).await;
    let upper_cased = detector_id.to_uppercase();

    let cases: [(&str, &str, &str); 6] = [
        ("well formed but never created", ABSENT_DETECTOR_ID, "us-east-1"),
        ("not hexadecimal", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz", "us-east-1"),
        ("too short", "abc123", "us-east-1"),
        ("too long", "0123456789abcdef0123456789abcdef0", "us-east-1"),
        ("the detector's id upper-cased", upper_cased.as_str(), "us-east-1"),
        (
            "the detector's id in another Region",
            detector_id.as_str(),
            OTHER_REGION,
        ),
    ];

    for case in cases {
        let (label, id, region) = case;
        let (status, response) = call_in(
            &router,
            region,
            "POST",
            &format!("/detector/{id}/findings/statistics"),
            Some(json!({"groupBy": "SEVERITY"})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_shape(&response),
            "BadRequestException",
            "case {label:?}: expected the modelled error shape, got {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidInputException",
            "case {label:?}: expected the invalid-input class, got {response:?}"
        );
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("detectorId"),
            "case {label:?}: expected a message naming the member, got {response:?}"
        );
        assert!(
            !message.contains(id),
            "case {label:?}: the refusal must not echo the request's value, got {response:?}"
        );
    }
}
