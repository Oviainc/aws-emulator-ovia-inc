// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetUsageStatistics`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, reusing the
//! harness in [`super::detector_lifecycle`], so routing, restJson1 (de)serialization and
//! the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. The answer carries the ONE statistic `usageStatisticsType` names and leaves every
//!    other `UsageStatistics` member absent: "if a UsageStatisticType is provided the
//!    result for other types will be null"
//!    (`smithy_query(query="doc",shape="UsageStatistics")`).
//! 2. Every amount is COMPUTED from the work the service recorded — the occurrences of
//!    the activity behind each stored finding, attributed to the feature whose data
//!    source generates findings of that type, and each malware scan it ran — at ONE rate
//!    shared by every statistic. So the statistics agree with each other: a
//!    `SUM_BY_ACCOUNT` row is the sum of that account's `SUM_BY_FEATURES` rows, and
//!    recording more activity moves exactly the rows that activity belongs to.
//! 3. `usageCriteria` restricts the records the statistics cover — by account, by
//!    feature, by the deprecated data-source view of a feature, and by resource.
//! 4. `maxResults` / `nextToken` page the rows, each row appearing exactly once, and a
//!    token this service did not issue is refused.
//! 5. The amounts are reported in the requested currency, and "current valid values are
//!    USD" (`smithy_query(query="doc",shape="GetUsageStatisticsRequest",member="Unit")`),
//!    so any other currency is refused.
//! 6. A `detectorId` this account/Region does not hold is refused with the modelled
//!    `BadRequestException` envelope under the `InvalidInputException` class.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::detector_lifecycle::{
    all_finding_ids, call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The marker for a finding type whose feature `UsageFeature` carries no member for, so
/// its usage is reported under no feature at all.
const NO_FEATURE: &str = "";

/// Finding types spanning every feature this service attributes usage to, with the
/// `UsageFeature` each one's usage belongs to.
///
/// Source: the GuardDuty finding-types reference, which groups the finding types into one
/// table per feature — the EC2 table (VPC flow logs, with the `!DNS` suffix marking the
/// types generated from DNS query data), the IAM table (CloudTrail management events),
/// the S3 table (S3 data events), the Kubernetes audit-log table, the Malware Protection
/// table (`…/MaliciousFile`), the RDS table, the Lambda table and the Runtime Monitoring
/// table. The types themselves come from this service's sample-finding catalogue, which
/// is what `CreateSampleFindings` can generate.
///
/// `Object:S3/MaliciousFile` is the row that must contribute to NOTHING: Malware
/// Protection for S3 is a feature of its own and `UsageFeature` carries no member for it
/// (`smithy_query(shape="UsageFeature")`).
const ATTRIBUTION: &[(&str, &str)] = &[
    ("Recon:EC2/Portscan", "FLOW_LOGS"),
    ("Impact:EC2/PortSweep", "FLOW_LOGS"),
    ("Backdoor:EC2/C&CActivity.B!DNS", "DNS_LOGS"),
    ("PenTest:IAMUser/PentooLinux", "CLOUD_TRAIL"),
    ("AttackSequence:IAM/CompromisedCredentials", "CLOUD_TRAIL"),
    ("Discovery:S3/MaliciousIPCaller", "S3_DATA_EVENTS"),
    ("Policy:Kubernetes/ExposedDashboard", "EKS_AUDIT_LOGS"),
    ("Discovery:RDS/TorIPCaller", "RDS_LOGIN_EVENTS"),
    ("Trojan:Lambda/DropPoint", "LAMBDA_NETWORK_LOGS"),
    ("Execution:Runtime/ReverseShell", "EC2_RUNTIME_MONITORING"),
    ("Execution:EC2/MaliciousFile", "EBS_MALWARE_PROTECTION"),
    ("Object:S3/MaliciousFile", NO_FEATURE),
];

/// The feature a malware scan's usage belongs to: Malware Protection for EC2 is what
/// scans the EC2 and EBS resources.
const EBS_MALWARE_PROTECTION: &str = "EBS_MALWARE_PROTECTION";

/// The feature `TOP_ACCOUNTS_BY_FEATURE` leaves out: "currently, this doesn't support
/// RDS_LOGIN_EVENTS"
/// (`smithy_query(query="doc",shape="UsageStatistics",member="TopAccountsByFeature")`).
const RDS_LOGIN_EVENTS: &str = "RDS_LOGIN_EVENTS";

/// Every `UsageFeature` the deprecated `DataSource` view names, with that name. Source:
/// `smithy_query(shape="DataSource")` (six members) beside
/// `smithy_query(shape="UsageFeature")` (eighteen): a feature outside this table has no
/// data-source name, so `SUM_BY_DATA_SOURCE` cannot report it.
const DATA_SOURCE_NAMES: &[(&str, &str)] = &[
    ("FLOW_LOGS", "FLOW_LOGS"),
    ("CLOUD_TRAIL", "CLOUD_TRAIL"),
    ("DNS_LOGS", "DNS_LOGS"),
    ("S3_DATA_EVENTS", "S3_LOGS"),
    ("EKS_AUDIT_LOGS", "KUBERNETES_AUDIT_LOGS"),
    ("EBS_MALWARE_PROTECTION", "EC2_MALWARE_SCAN"),
];

/// Every `UsageStatisticType` with the `UsageStatistics` member it fills
/// (`smithy_query(shape="UsageStatisticType")` and
/// `smithy_query(shape="UsageStatistics")`).
const STATISTIC_MEMBERS: [(&str, &str); 6] = [
    ("SUM_BY_ACCOUNT", "sumByAccount"),
    ("SUM_BY_DATA_SOURCE", "sumByDataSource"),
    ("SUM_BY_FEATURES", "sumByFeature"),
    ("SUM_BY_RESOURCE", "sumByResource"),
    ("TOP_ACCOUNTS_BY_FEATURE", "topAccountsByFeature"),
    ("TOP_RESOURCES", "topResources"),
];

/// Two EC2/EBS resources a malware scan can run over, in the ARN forms
/// `StartMalwareScan` accepts. The volume sorts AFTER the instance as text, so a listing
/// ordered by resource name and one ordered by cost can be told apart.
const VOLUME_ARN: &str = "arn:aws:ec2:us-east-1:123456789012:volume/vol-0abc123def4567890";
/// See [`VOLUME_ARN`].
const INSTANCE_ARN: &str = "arn:aws:ec2:us-east-1:123456789012:instance/i-0abc123def4567890";

/// An account id of the modelled `len 12..12` form that owns none of the recorded work.
const OTHER_ACCOUNT_ID: &str = "210987654321";

/// The Region whose endpoint does not serve the detector these tests create.
const OTHER_REGION: &str = "us-west-2";

/// The most rows a `TOP_*` statistic reports: "lists the top 50 …"
/// (`smithy_query(query="doc",shape="UsageStatistics",member="TopResources")` and
/// `…member="TopAccountsByFeature"`).
const TOP_ROWS: usize = 50;

/// Call `GetUsageStatistics`.
async fn usage(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/usage/statistics"),
        Some(body),
    )
    .await
}

/// [`usage`], failing the test unless the call succeeded.
async fn usage_ok(router: &Router, detector_id: &str, body: Value, case: &str) -> Value {
    let (status, response) = usage(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-usage-statistics expected 200, got {status} {response:?}"
    );
    response
}

/// Ask for one statistic over every record the detector holds.
async fn statistic(router: &Router, detector_id: &str, statistic_type: &str) -> Value {
    usage_ok(
        router,
        detector_id,
        json!({"usageStatisticsType": statistic_type, "usageCriteria": {}}),
        statistic_type,
    )
    .await
}

/// An enabled detector holding one finding per [`ATTRIBUTION`] row, plus one malware scan
/// per entry of `scan_arns` — the recorded work every expectation below is computed from.
async fn detector_with_recorded_usage(router: &Router, scan_arns: &[&str]) -> String {
    let detector_id = enabled_detector(router).await;
    let types: Vec<&str> = ATTRIBUTION
        .iter()
        .map(|(finding_type, _)| *finding_type)
        .collect();
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": types})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings: expected 200, got {status} {body:?}"
    );
    let ids = all_finding_ids(router, &detector_id, 50).await;
    assert_eq!(
        ids.len(),
        ATTRIBUTION.len(),
        "expected one finding per attribution row, got {ids:?}"
    );
    for arn in scan_arns {
        let (status, body) = call(
            router,
            "POST",
            "/malware-scan/start",
            Some(json!({"resourceArn": arn})),
        )
        .await;
        assert_eq!(
            status, 200,
            "start-malware-scan {arn}: expected 200, got {status} {body:?}"
        );
    }
    detector_id
}

/// The account the stored findings belong to, read back through `GetFindings` — the
/// independent view the account expectations are computed from.
async fn recorded_account(router: &Router, detector_id: &str) -> String {
    let ids = all_finding_ids(router, detector_id, 50).await;
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(json!({"findingIds": ids})),
    )
    .await;
    assert_eq!(status, 200, "get-findings: expected 200, got {status} {body:?}");
    let accounts: BTreeSet<String> = body["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|finding| finding["accountId"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        accounts.len(),
        1,
        "expected one account across the stored findings, got {accounts:?}"
    );
    accounts.into_iter().next().unwrap_or_default()
}

/// Render a cost the way the response renders `Total.Amount`: a decimal string in
/// hundredths of the currency unit. One recorded observation is one hundredth, so an
/// expectation is stated as the number of observations the records hold.
fn amount(observations: i64) -> String {
    format!("{}.{:02}", observations / 100, observations % 100)
}

/// The rows one statistic reports, in the order it reports them: each row's key and its
/// amount, with the currency asserted on every row.
fn reported_rows(
    response: &Value,
    member: &str,
    key_member: &str,
    unit: &str,
    case: &str,
) -> Vec<(String, String)> {
    let rows = response["usageStatistics"][member]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {member} to be a list, got {response:?}")
        });
    rows.iter()
        .map(|row| {
            let key = row[key_member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected {key_member} on {row:?}"))
                .to_string();
            assert_eq!(
                row["total"]["unit"].as_str(),
                Some(unit),
                "case {case:?}: expected the amount in {unit}, got {row:?}"
            );
            let amount = row["total"]["amount"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an amount on {row:?}"))
                .to_string();
            assert!(
                amount
                    .split_once('.')
                    .is_some_and(|(_, hundredths)| hundredths.len() == 2),
                "case {case:?}: expected a decimal amount, got {amount:?}"
            );
            (key, amount)
        })
        .collect()
}

/// [`reported_rows`] as a key -> amount map, for an expectation that does not depend on
/// the order.
fn reported_totals(
    response: &Value,
    member: &str,
    key_member: &str,
    case: &str,
) -> BTreeMap<String, String> {
    reported_rows(response, member, key_member, "USD", case)
        .into_iter()
        .collect()
}

/// The amounts every feature owes for one finding per [`ATTRIBUTION`] row plus `scans`
/// malware scans: one observation per finding occurrence, one per scan.
fn expected_feature_observations(scans: i64) -> BTreeMap<String, i64> {
    let mut expected: BTreeMap<String, i64> = BTreeMap::new();
    for (_, feature) in ATTRIBUTION.iter().filter(|(_, f)| *f != NO_FEATURE) {
        *expected.entry((*feature).to_string()).or_insert(0) += 1;
    }
    if scans > 0 {
        *expected
            .entry(EBS_MALWARE_PROTECTION.to_string())
            .or_insert(0) += scans;
    }
    expected
}

/// The expectation as the response renders it.
fn as_amounts(expected: &BTreeMap<String, i64>) -> BTreeMap<String, String> {
    expected
        .iter()
        .map(|(key, observations)| (key.clone(), amount(*observations)))
        .collect()
}

/// The entries of `expected` the keys in `keys` name.
fn only(expected: &BTreeMap<String, i64>, keys: &[&str]) -> BTreeMap<String, i64> {
    keys.iter()
        .filter_map(|key| expected.get_key_value(*key))
        .map(|(key, observations)| (key.clone(), *observations))
        .collect()
}

/// An expectation written out by hand, for a case whose amounts are not a subset of the
/// unrestricted ones.
fn observations(rows: &[(&str, i64)]) -> BTreeMap<String, i64> {
    rows.iter()
        .map(|(key, observations)| ((*key).to_string(), *observations))
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: one statistic type, one member.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_statistic_type_fills_only_its_own_member() {
    let router = new_router();
    let detector_id = detector_with_recorded_usage(&router, &[VOLUME_ARN]).await;

    for (statistic_type, member) in STATISTIC_MEMBERS {
        let response = statistic(&router, &detector_id, statistic_type).await;
        assert!(
            response["usageStatistics"][member].is_array(),
            "case {statistic_type:?}: expected {member} to carry the rows, got {response:?}"
        );
        for (_, other) in STATISTIC_MEMBERS.iter().filter(|(_, m)| *m != member) {
            assert!(
                response["usageStatistics"][other].is_null(),
                "case {statistic_type:?}: expected {other} absent, got {response:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: every amount is computed from the recorded work.
// ---------------------------------------------------------------------------

/// `SUM_BY_FEATURES` is the cost of the recorded activity per feature: one observation
/// per finding occurrence, attributed to the feature whose data source generates findings
/// of that type, plus one per malware scan. The rows span every feature the catalogue
/// reaches, a detector with no recorded work at all, and the finding type that belongs to
/// no feature.
#[tokio::test]
async fn sum_by_features_is_the_cost_of_the_recorded_activity() {
    let router = new_router();

    // An empty detector reports no row — not a canned one.
    let empty_detector = enabled_detector(&router).await;
    let response = statistic(&router, &empty_detector, "SUM_BY_FEATURES").await;
    assert_eq!(
        reported_totals(&response, "sumByFeature", "feature", "no recorded work"),
        BTreeMap::new(),
        "case \"no recorded work\": expected no row, got {response:?}"
    );

    for scans in [0_i64, 1, 2] {
        let case = format!("{scans} malware scans");
        let router = new_router();
        let arns: Vec<&str> = std::iter::repeat_n(VOLUME_ARN, usize::try_from(scans).unwrap_or(0))
            .collect::<Vec<&str>>();
        let detector_id = detector_with_recorded_usage(&router, &arns).await;

        let response = statistic(&router, &detector_id, "SUM_BY_FEATURES").await;
        let got = reported_totals(&response, "sumByFeature", "feature", &case);
        assert_eq!(
            got,
            as_amounts(&expected_feature_observations(scans)),
            "case {case:?}: expected the per-feature cost of the recorded work, got {got:?}"
        );
    }
}

/// Recording MORE of one activity moves exactly that activity's row: a repeated sample of
/// one finding type is another observation of it (`Service.Count` grows), so its feature
/// costs one observation more and every other feature stays where it was.
#[tokio::test]
async fn another_observation_of_one_activity_moves_only_its_own_feature() {
    let router = new_router();
    let detector_id = detector_with_recorded_usage(&router, &[]).await;
    let before = reported_totals(
        &statistic(&router, &detector_id, "SUM_BY_FEATURES").await,
        "sumByFeature",
        "feature",
        "before",
    );

    // One more observation of a CLOUD_TRAIL activity, and one of an S3 one.
    for finding_type in ["PenTest:IAMUser/PentooLinux", "Discovery:S3/MaliciousIPCaller"] {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings/create"),
            Some(json!({"findingTypes": [finding_type]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {finding_type:?}: create-sample-findings expected 200, got {status} {body:?}"
        );
    }

    let after = reported_totals(
        &statistic(&router, &detector_id, "SUM_BY_FEATURES").await,
        "sumByFeature",
        "feature",
        "after",
    );
    let mut expected = expected_feature_observations(0);
    *expected.entry("CLOUD_TRAIL".to_string()).or_insert(0) += 1;
    *expected.entry("S3_DATA_EVENTS".to_string()).or_insert(0) += 1;
    assert_eq!(
        after,
        as_amounts(&expected),
        "case \"another observation\": expected only the regenerated features to move, \
         before {before:?}, got {after:?}"
    );
}

/// `SUM_BY_ACCOUNT` aggregates the SAME records as `SUM_BY_FEATURES`, so its row is that
/// account's per-feature rows summed, and the account it names is the account the stored
/// findings belong to. The rows differ in how much recorded work the detector holds.
#[tokio::test]
async fn sum_by_account_sums_the_same_records_as_sum_by_features() {
    for scans in [0_i64, 2] {
        let case = format!("{scans} malware scans");
        let router = new_router();
        let arns: Vec<&str> = std::iter::repeat_n(INSTANCE_ARN, usize::try_from(scans).unwrap_or(0))
            .collect::<Vec<&str>>();
        let detector_id = detector_with_recorded_usage(&router, &arns).await;
        let account = recorded_account(&router, &detector_id).await;

        let per_account = reported_totals(
            &statistic(&router, &detector_id, "SUM_BY_ACCOUNT").await,
            "sumByAccount",
            "accountId",
            &case,
        );
        let total: i64 = expected_feature_observations(scans).values().sum();
        let expected: BTreeMap<String, String> =
            [(account.clone(), amount(total))].into_iter().collect();
        assert_eq!(
            per_account, expected,
            "case {case:?}: expected the account's rows to sum to the per-feature cost, \
             got {per_account:?}"
        );
    }
}

/// `SUM_BY_DATA_SOURCE` reports the same records under the deprecated data-source names,
/// and reports NOTHING for a feature that view has no member for.
#[tokio::test]
async fn sum_by_data_source_reports_only_the_features_that_view_names() {
    let router = new_router();
    let detector_id = detector_with_recorded_usage(&router, &[VOLUME_ARN]).await;
    let expected_features = expected_feature_observations(1);

    let response = statistic(&router, &detector_id, "SUM_BY_DATA_SOURCE").await;
    let got = reported_totals(&response, "sumByDataSource", "dataSource", "every data source");

    let expected: BTreeMap<String, String> = DATA_SOURCE_NAMES
        .iter()
        .filter_map(|(feature, data_source)| {
            expected_features
                .get(*feature)
                .map(|observations| ((*data_source).to_string(), amount(*observations)))
        })
        .collect();
    assert_eq!(
        got, expected,
        "case \"every data source\": expected one row per feature the view names, got {got:?}"
    );

    // The features with no data-source member are reported by name and never under an
    // invented one.
    for feature in expected_features.keys() {
        if DATA_SOURCE_NAMES.iter().any(|(named, _)| named == feature) {
            continue;
        }
        assert!(
            !got.contains_key(feature.as_str()),
            "case \"every data source\": {feature} has no DataSource member, got {got:?}"
        );
    }
}

/// The resource statistics report the resources the store IDENTIFIES — the ones the
/// malware scans named — summed per resource. `SUM_BY_RESOURCE` reports them in resource
/// order and `TOP_RESOURCES` "in order from most to least expensive"
/// (`smithy_query(query="doc",shape="UsageStatistics",member="TopResources")`), so the
/// population gives the two resources different costs.
#[tokio::test]
async fn the_resource_statistics_report_the_scanned_resources() {
    let router = new_router();
    let detector_id =
        detector_with_recorded_usage(&router, &[VOLUME_ARN, VOLUME_ARN, INSTANCE_ARN]).await;

    let expected = observations(&[(VOLUME_ARN, 2), (INSTANCE_ARN, 1)]);
    for statistic_type in ["SUM_BY_RESOURCE", "TOP_RESOURCES"] {
        let member = if statistic_type == "TOP_RESOURCES" {
            "topResources"
        } else {
            "sumByResource"
        };
        let response = statistic(&router, &detector_id, statistic_type).await;
        let rows = reported_rows(&response, member, "resource", "USD", statistic_type);
        let got: BTreeMap<String, String> = rows.iter().cloned().collect();
        assert_eq!(
            got,
            as_amounts(&expected),
            "case {statistic_type:?}: expected one row per scanned resource, got {got:?}"
        );

        let order: Vec<String> = rows.iter().map(|(resource, _)| resource.clone()).collect();
        let expected_order: Vec<String> = if statistic_type == "TOP_RESOURCES" {
            // Most expensive first: the volume was scanned twice.
            vec![VOLUME_ARN.to_string(), INSTANCE_ARN.to_string()]
        } else {
            // The resource's own order, which puts the instance first.
            vec![INSTANCE_ARN.to_string(), VOLUME_ARN.to_string()]
        };
        assert_eq!(
            order, expected_order,
            "case {statistic_type:?}: expected the documented order, got {order:?}"
        );
    }
}

/// `TOP_ACCOUNTS_BY_FEATURE` reports, per feature, the accounts that owe its usage, most
/// expensive first — and leaves out the one feature the member documents it does not
/// support.
#[tokio::test]
async fn top_accounts_by_feature_reports_each_feature_except_the_unsupported_one() {
    let router = new_router();
    let detector_id = detector_with_recorded_usage(&router, &[VOLUME_ARN, INSTANCE_ARN]).await;
    let account = recorded_account(&router, &detector_id).await;
    let expected_features = expected_feature_observations(2);

    let response = statistic(&router, &detector_id, "TOP_ACCOUNTS_BY_FEATURE").await;
    let rows = response["usageStatistics"]["topAccountsByFeature"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("expected topAccountsByFeature rows, got {response:?}"));

    let reported: Vec<String> = rows
        .iter()
        .map(|row| {
            row["feature"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a feature on {row:?}"))
                .to_string()
        })
        .collect();
    let expected: BTreeSet<String> = expected_features
        .keys()
        .filter(|feature| *feature != RDS_LOGIN_EVENTS)
        .cloned()
        .collect();
    assert_eq!(
        reported.iter().cloned().collect::<BTreeSet<String>>(),
        expected,
        "case \"top accounts by feature\": expected every feature but {RDS_LOGIN_EVENTS}, \
         got {reported:?}"
    );

    // Each feature's accounts carry that feature's cost, and the most expensive feature
    // comes first.
    let mut previous = i64::MAX;
    for row in &rows {
        let feature = row["feature"].as_str().unwrap_or_default().to_string();
        let observations = *expected_features.get(&feature).unwrap_or(&0);
        let accounts = row["accounts"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {feature:?}: expected accounts on {row:?}"));
        let got: Vec<(String, String)> = accounts
            .iter()
            .map(|entry| {
                (
                    entry["accountId"].as_str().unwrap_or_default().to_string(),
                    entry["total"]["amount"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![(account.clone(), amount(observations))],
            "case {feature:?}: expected the account that owes this feature's usage, got {got:?}"
        );
        assert!(
            observations <= previous,
            "case {feature:?}: expected the features most expensive first, \
             {observations} came after {previous}"
        );
        previous = observations;
    }
}

// ---------------------------------------------------------------------------
// Rule 3: usageCriteria restricts the records the statistics cover.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn usage_criteria_restricts_the_records_the_statistics_cover() {
    let router = new_router();
    let detector_id = detector_with_recorded_usage(&router, &[VOLUME_ARN]).await;
    let account = recorded_account(&router, &detector_id).await;
    let every_feature = expected_feature_observations(1);

    let cases: Vec<(String, Value, BTreeMap<String, i64>)> = vec![
        (
            "no criterion at all".to_string(),
            json!({}),
            every_feature.clone(),
        ),
        (
            "an empty feature list restricts nothing".to_string(),
            json!({"features": []}),
            every_feature.clone(),
        ),
        (
            "one feature".to_string(),
            json!({"features": ["FLOW_LOGS"]}),
            only(&every_feature, &["FLOW_LOGS"]),
        ),
        (
            "two features".to_string(),
            json!({"features": ["FLOW_LOGS", "DNS_LOGS"]}),
            only(&every_feature, &["FLOW_LOGS", "DNS_LOGS"]),
        ),
        (
            "a feature nothing was recorded for".to_string(),
            json!({"features": ["AI_PROTECTION"]}),
            BTreeMap::new(),
        ),
        (
            "a data source names the feature behind it".to_string(),
            json!({"dataSources": ["S3_LOGS"]}),
            only(&every_feature, &["S3_DATA_EVENTS"]),
        ),
        (
            "a data source and a feature together".to_string(),
            json!({"dataSources": ["EC2_MALWARE_SCAN"], "features": ["CLOUD_TRAIL"]}),
            only(&every_feature, &["EBS_MALWARE_PROTECTION", "CLOUD_TRAIL"]),
        ),
        (
            "the account the records belong to".to_string(),
            json!({"accountIds": [account.clone()]}),
            every_feature.clone(),
        ),
        (
            "an account that owns no record".to_string(),
            json!({"accountIds": [OTHER_ACCOUNT_ID]}),
            BTreeMap::new(),
        ),
        (
            // Only the scan names a resource, so a resource criterion drops the findings:
            // "the resources to aggregate usage statistics from"
            // (`smithy_query(query="doc",shape="UsageCriteria",member="Resources")`).
            "a resource the store identifies".to_string(),
            json!({"resources": [VOLUME_ARN]}),
            observations(&[(EBS_MALWARE_PROTECTION, 1)]),
        ),
        (
            "a resource nothing was recorded for".to_string(),
            json!({"resources": [INSTANCE_ARN]}),
            BTreeMap::new(),
        ),
    ];

    for (case, criteria, expected) in cases {
        let response = usage_ok(
            &router,
            &detector_id,
            json!({"usageStatisticsType": "SUM_BY_FEATURES", "usageCriteria": criteria}),
            &case,
        )
        .await;
        let got = reported_totals(&response, "sumByFeature", "feature", &case);
        assert_eq!(
            got,
            as_amounts(&expected),
            "case {case:?}: expected the criteria to select exactly those records, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: paging.
// ---------------------------------------------------------------------------

/// Paging with any `maxResults` yields each row exactly once, in the order the
/// unpaginated answer reports them. The rows span both ends of the modelled `@range`
/// 1..50 the population can distinguish.
#[tokio::test]
async fn paging_the_statistics_yields_each_row_exactly_once() {
    let router = new_router();
    let detector_id = detector_with_recorded_usage(&router, &[VOLUME_ARN]).await;

    let unpaginated: Vec<(String, String)> = reported_rows(
        &statistic(&router, &detector_id, "SUM_BY_FEATURES").await,
        "sumByFeature",
        "feature",
        "USD",
        "unpaginated",
    );
    assert!(
        unpaginated.len() > 1,
        "expected more than one feature row to page over, got {unpaginated:?}"
    );

    for page_size in [1_i64, 2, 3, 4, 50] {
        let case = format!("maxResults {page_size}");
        let mut collected: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body =
                json!({"usageStatisticsType": "SUM_BY_FEATURES", "usageCriteria": {}, "maxResults": page_size});
            if let Some(token) = token.as_deref() {
                body["nextToken"] = Value::String(token.to_string());
            }
            let response = usage_ok(&router, &detector_id, body, &case).await;
            let page = reported_rows(&response, "sumByFeature", "feature", "USD", &case);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                "case {case:?}: expected at most {page_size} rows, got {page:?}"
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= unpaginated.len() + 1,
                "case {case:?}: paging did not terminate, collected {collected:?}"
            );
            match response["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, unpaginated,
            "case {case:?}: expected every row exactly once in order, got {collected:?}"
        );
    }
}

/// A `nextToken` this service did not issue is refused rather than answered with the
/// first page. The rows span the ways a token can fail to resolve.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = new_router();
    let detector_id = detector_with_recorded_usage(&router, &[VOLUME_ARN]).await;

    for token in ["not-a-token", "-1", "9999", ""] {
        let case = format!("nextToken {token:?}");
        let (status, response) = usage(
            &router,
            &detector_id,
            json!({"usageStatisticsType": "SUM_BY_FEATURES", "usageCriteria": {}, "nextToken": token}),
        )
        .await;
        if token.is_empty() {
            // An empty token names no page, so it reads as "the first page".
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {response:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidInputException",
            "case {case:?}: expected the InvalidInputException class, got {response:?}"
        );
    }
}

/// Every row one statistic reports, walked over its pages with `page_size`.
async fn paged_rows(
    router: &Router,
    detector_id: &str,
    statistic_type: &str,
    member: &str,
    key_member: &str,
    page_size: i64,
    case: &str,
) -> Vec<(String, String)> {
    let mut collected: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({
            "usageStatisticsType": statistic_type,
            "usageCriteria": {},
            "maxResults": page_size,
        });
        if let Some(token) = token.as_deref() {
            body["nextToken"] = Value::String(token.to_string());
        }
        let response = usage_ok(router, detector_id, body, case).await;
        collected.extend(reported_rows(&response, member, key_member, "USD", case));
        match response["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            collected.len() < 500,
            "case {case:?}: paging did not terminate, collected {} rows",
            collected.len()
        );
    }
    collected
}

/// A `TOP_*` statistic reports at most the documented top 50 rows, and paging cannot walk
/// past that: the cap is the STATISTIC, not the page. `SUM_BY_RESOURCE` reports every
/// resource instead, so the two differ once the store holds more than 50 of them. The
/// rows span both statistics and page sizes above and below the cap.
#[tokio::test]
async fn a_top_statistic_reports_at_most_the_documented_fifty_rows() {
    const SCANNED_RESOURCES: usize = 55;

    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    for index in 0..SCANNED_RESOURCES {
        let arn = format!("arn:aws:ec2:us-east-1:123456789012:volume/vol-{index:017}");
        let (status, body) = call(
            &router,
            "POST",
            "/malware-scan/start",
            Some(json!({"resourceArn": arn})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn:?}: start-malware-scan expected 200, got {status} {body:?}"
        );
    }

    let cases: [(&str, &str, usize); 2] = [
        ("SUM_BY_RESOURCE", "sumByResource", SCANNED_RESOURCES),
        ("TOP_RESOURCES", "topResources", TOP_ROWS),
    ];
    for (statistic_type, member, expected) in cases {
        for page_size in [7_i64, 50] {
            let case = format!("{statistic_type} with maxResults {page_size}");
            let rows = paged_rows(
                &router,
                &detector_id,
                statistic_type,
                member,
                "resource",
                page_size,
                &case,
            )
            .await;
            assert_eq!(
                rows.len(),
                expected,
                "case {case:?}: expected {expected} rows, got {}",
                rows.len()
            );
            let distinct: BTreeSet<String> =
                rows.iter().map(|(resource, _)| resource.clone()).collect();
            assert_eq!(
                distinct.len(),
                expected,
                "case {case:?}: expected each resource once, got {rows:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the currency unit.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_amounts_are_reported_in_the_requested_currency() {
    let router = new_router();
    let detector_id = detector_with_recorded_usage(&router, &[VOLUME_ARN]).await;

    // `None` as the expectation means the request is refused.
    let cases: [(&str, Option<&str>, Option<&str>); 6] = [
        ("no unit at all", None, Some("USD")),
        ("the documented USD", Some("USD"), Some("USD")),
        ("a lower-cased currency code", Some("usd"), Some("USD")),
        ("an empty unit", Some(""), Some("USD")),
        ("a currency this service does not report", Some("EUR"), None),
        ("a unit that is not a currency", Some("GB"), None),
    ];

    for (case, requested, expected) in cases {
        let mut body = json!({"usageStatisticsType": "SUM_BY_FEATURES", "usageCriteria": {}});
        if let Some(unit) = requested {
            body["unit"] = Value::String(unit.to_string());
        }
        let (status, response) = usage(&router, &detector_id, body).await;
        match expected {
            Some(unit) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {response:?}"
                );
                let rows = reported_rows(&response, "sumByFeature", "feature", unit, case);
                assert!(
                    !rows.is_empty(),
                    "case {case:?}: expected the rows in {unit}, got {response:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status} {response:?}"
                );
                assert_eq!(
                    error_code(&response),
                    "InvalidInputException",
                    "case {case:?}: expected the InvalidInputException class, got {response:?}"
                );
                assert_eq!(
                    error_shape(&response),
                    "BadRequestException",
                    "case {case:?}: expected the modelled envelope, got {response:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 6: the detector the endpoint holds.
// ---------------------------------------------------------------------------

/// A detector is a regional resource owned by one account, so a `detectorId` the endpoint
/// does not resolve is refused rather than answered with hollow statistics. The rows
/// differ along the ways an id can fail to resolve.
#[tokio::test]
async fn a_detector_id_the_endpoint_does_not_hold_is_refused() {
    let router = new_router();
    let detector_id = detector_with_recorded_usage(&router, &[VOLUME_ARN]).await;
    let upper_cased = detector_id.to_uppercase();

    let cases: [(&str, &str, &str); 5] = [
        ("well formed but never created", ABSENT_DETECTOR_ID, "us-east-1"),
        ("not a detector id at all", "not-a-detector-id", "us-east-1"),
        ("an empty-looking id", "0", "us-east-1"),
        ("the id upper-cased", upper_cased.as_str(), "us-east-1"),
        (
            "this detector, through another Region's endpoint",
            detector_id.as_str(),
            OTHER_REGION,
        ),
    ];

    for (case, id, region) in cases {
        let (status, response) = call_in(
            &router,
            region,
            "POST",
            &format!("/detector/{id}/usage/statistics"),
            Some(json!({"usageStatisticsType": "SUM_BY_ACCOUNT", "usageCriteria": {}})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidInputException",
            "case {case:?}: expected the InvalidInputException class, got {response:?}"
        );
        assert_eq!(
            error_shape(&response),
            "BadRequestException",
            "case {case:?}: expected the modelled envelope, got {response:?}"
        );
    }
}
