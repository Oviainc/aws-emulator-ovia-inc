// SPDX-License-Identifier: CC0-1.0

//! The Lightsail alarm — put, read, re-put and delete — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: an alarm is ONE persisted record per alarm name.
//! `PutAlarm` stores the configuration the request carries verbatim — with the
//! documented default for each member it omits — against the metric it names on
//! the monitored resource it names, and computes every member the request does
//! NOT carry from that record plus this service's own allocators and metric
//! catalog: the ARN and support code from a minted id and the monitored
//! resource's region, the location from that resource, the `period`, `statistic`
//! and `unit` from the metric, and the state from the fact that a new alarm has
//! no data yet. So a later `GetAlarms` reports exactly that record; re-putting an
//! existing name overwrites the configuration while the alarm keeps its identity
//! and its state; `DeleteAlarm` removes it and cannot be repeated; and a request
//! naming a resource this account does not hold, a metric that resource's type
//! does not publish, an evaluation window outside the documented range or more
//! alarms on one metric than the service accepts is refused and stores nothing.

use std::collections::BTreeSet;

use serde_json::{json, Value};

// The harness, the wire error code and the minted-id check this test target already
// exposes: a sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// `Alarm$period`: the five minutes one evaluation period covers, in seconds
/// ("Each evaluation period is five minutes long", `PutAlarmRequest`
/// `$evaluationPeriods`).
const ALARM_PERIOD_SECONDS: i64 = 300;

/// The state a new alarm is in ("When this action creates an alarm, the alarm
/// state is immediately set to INSUFFICIENT_DATA", `PutAlarm`).
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The blueprint and bundle every instance in this suite is created from: the
/// monitored resource's own configuration is not what any case is about.
const INSTANCE_BLUEPRINT: &str = "amazon_linux_2023";
const INSTANCE_BUNDLE: &str = "micro_3_0";

// ---------------------------------------------------------------------------
// Monitored resources
// ---------------------------------------------------------------------------

/// Which kind of resource a case's alarm watches.
///
/// From `PutAlarmRequest$monitoredResourceName`: "Instances, load balancers, and
/// relational databases are the only Lightsail resources that can currently be
/// monitored by alarms." Of those three, only these two can be created in this
/// emulator — `CreateLoadBalancer` is still a scaffold with its own issue — so a
/// load balancer metric is exercised through the refusals instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Monitored {
    Instance,
    RelationalDatabase,
}

impl Monitored {
    /// The modelled `ResourceType` value a resource of this kind reports.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Instance => "Instance",
            Self::RelationalDatabase => "RelationalDatabase",
        }
    }
}

/// Create the monitored resource `name` of `kind` in `zone`, answering the ARN
/// its own read reports.
async fn monitored_resource(
    emulator: &Emulator,
    kind: Monitored,
    name: &str,
    zone: &str,
    case: &str,
) -> String {
    match kind {
        Monitored::Instance => {
            emulator
                .ok(
                    "CreateInstances",
                    json!({
                        "instanceNames": [name],
                        "availabilityZone": zone,
                        "blueprintId": INSTANCE_BLUEPRINT,
                        "bundleId": INSTANCE_BUNDLE,
                    }),
                )
                .await;
            let instance = emulator
                .ok("GetInstance", json!({"instanceName": name}))
                .await["instance"]
                .clone();
            instance["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected the instance to carry an arn"))
                .to_string()
        }
        Monitored::RelationalDatabase => {
            emulator
                .ok(
                    "CreateRelationalDatabase",
                    json!({
                        "relationalDatabaseName": name,
                        "availabilityZone": zone,
                        "relationalDatabaseBlueprintId": "mysql_8_4",
                        "relationalDatabaseBundleId": "micro_1_0",
                        "masterDatabaseName": "appdb",
                        "masterUsername": "dbadmin",
                    }),
                )
                .await;
            let database = emulator
                .ok(
                    "GetRelationalDatabase",
                    json!({"relationalDatabaseName": name}),
                )
                .await["relationalDatabase"]
                .clone();
            database["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected the database to carry an arn"))
                .to_string()
        }
    }
}

/// The alarms `GetAlarms` reports for `filters`, walking every page.
async fn alarms(emulator: &Emulator, filters: Value) -> (Vec<Value>, Vec<usize>) {
    let mut found = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = filters.clone();
        if let Some(token) = &token {
            body["pageToken"] = json!(token);
        }
        let response = emulator.ok("GetAlarms", body).await;
        let page = response["alarms"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case listing alarms {filters:?}: expected an alarms array, got {response:?}")
            })
            .clone();
        page_sizes.push(page.len());
        found.extend(page);
        match response["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            page_sizes.len() < 100,
            "case listing alarms {filters:?}: the walk did not terminate after {} pages",
            page_sizes.len()
        );
    }
    (found, page_sizes)
}

/// The one alarm `name` names, or a panic naming `case`.
async fn alarm(emulator: &Emulator, name: &str, case: &str) -> Value {
    let (found, _) = alarms(emulator, json!({"alarmName": name})).await;
    assert_eq!(
        found.len(),
        1,
        "case {case}: expected exactly one alarm named {name}, got {found:?}"
    );
    found[0].clone()
}

/// Every alarm name in `found`, in the order they were reported.
fn names(found: &[Value]) -> Vec<String> {
    found
        .iter()
        .map(|alarm| alarm["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// The put/read table
// ---------------------------------------------------------------------------

/// One `PutAlarm` request the service must serve.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// monitored resource, the metric (and so the unit and statistic the service
/// derives), the comparison operator (all four modelled members across the
/// table), the threshold (whole, fractional, zero, large), the number of
/// evaluation periods (both published bounds and values between them), whether
/// the "M out of N" datapoint count is given, which `treatMissingData` member is
/// named (all four, plus omitted), how many contact protocols and notification
/// triggers are named, whether notifications are explicitly enabled, disabled or
/// left to default, the number of tags, the region, and the form of the alarm
/// name.
struct Row {
    case: &'static str,
    alarm_name: &'static str,
    monitored: Monitored,
    /// The region the monitored resource's Availability Zone belongs to, which is
    /// the region the alarm reports and the one its ARN carries.
    region: &'static str,
    zone: &'static str,
    metric_name: &'static str,
    /// The published unit of `metric_name`, and the statistic an alarm on it
    /// reports — neither is in the request.
    unit: &'static str,
    statistic: &'static str,
    comparison_operator: &'static str,
    threshold: f64,
    evaluation_periods: i64,
    datapoints_to_alarm: Option<i64>,
    treat_missing_data: Option<&'static str>,
    /// What `treatMissingData` reads back as: the member when it is named, and
    /// the documented default `missing` when it is not.
    effective_treat_missing_data: &'static str,
    contact_protocols: &'static [&'static str],
    notification_triggers: &'static [&'static str],
    notification_enabled: Option<bool>,
    /// What `notificationEnabled` reads back as: the member when it is named,
    /// and the documented default `true` when it is not.
    effective_notification_enabled: bool,
    tags: &'static [(&'static str, &'static str)],
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "instance CPU at the smallest evaluation window, nothing optional",
        alarm_name: "cpu-high",
        monitored: Monitored::Instance,
        region: "us-east-1",
        zone: "us-east-1a",
        metric_name: "CPUUtilization",
        unit: "Percent",
        statistic: "Average",
        comparison_operator: "GreaterThanOrEqualToThreshold",
        threshold: 80.0,
        // The published minimum: "a minimum valuation period of 1 (5 minutes)".
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        treat_missing_data: None,
        effective_treat_missing_data: "missing",
        contact_protocols: &[],
        notification_triggers: &[],
        notification_enabled: None,
        effective_notification_enabled: true,
        tags: &[],
    },
    Row {
        case: "instance egress at the largest evaluation window, M equal to N",
        alarm_name: "net_out_288",
        monitored: Monitored::Instance,
        region: "us-west-2",
        zone: "us-west-2b",
        metric_name: "NetworkOut",
        unit: "Bytes",
        statistic: "Sum",
        comparison_operator: "GreaterThanThreshold",
        threshold: 1_000_000.0,
        // The published maximum: "a maximum evaluation period of 288 (24 hours)".
        evaluation_periods: 288,
        datapoints_to_alarm: Some(288),
        treat_missing_data: Some("breaching"),
        effective_treat_missing_data: "breaching",
        contact_protocols: &["Email"],
        notification_triggers: &["ALARM"],
        notification_enabled: Some(true),
        effective_notification_enabled: true,
        tags: &[("stage", "prod")],
    },
    Row {
        case: "instance disk queue, fractional threshold, every trigger, notifications off",
        alarm_name: "disk-queue-3",
        monitored: Monitored::Instance,
        region: "eu-west-3",
        zone: "eu-west-3a",
        metric_name: "DiskQueueLength",
        unit: "Count",
        statistic: "Average",
        comparison_operator: "LessThanThreshold",
        threshold: 0.5,
        evaluation_periods: 5,
        datapoints_to_alarm: Some(3),
        treat_missing_data: Some("ignore"),
        effective_treat_missing_data: "ignore",
        contact_protocols: &["Email", "SMS"],
        notification_triggers: &["ALARM", "OK", "INSUFFICIENT_DATA"],
        notification_enabled: Some(false),
        effective_notification_enabled: false,
        tags: &[("owner", "platform"), ("app", "reports")],
    },
    Row {
        case: "instance system status check, zero threshold, SMS only",
        alarm_name: "status_check_system",
        monitored: Monitored::Instance,
        region: "ap-northeast-1",
        zone: "ap-northeast-1c",
        metric_name: "StatusCheckFailed_System",
        unit: "Count",
        statistic: "Sum",
        comparison_operator: "LessThanOrEqualToThreshold",
        threshold: 0.0,
        evaluation_periods: 2,
        datapoints_to_alarm: None,
        treat_missing_data: Some("notBreaching"),
        effective_treat_missing_data: "notBreaching",
        contact_protocols: &["SMS"],
        notification_triggers: &["ALARM", "INSUFFICIENT_DATA"],
        notification_enabled: None,
        effective_notification_enabled: true,
        tags: &[],
    },
    Row {
        case: "database connections, the statistic a database metric publishes",
        alarm_name: "db-connections-42",
        monitored: Monitored::RelationalDatabase,
        region: "us-east-2",
        zone: "us-east-2a",
        metric_name: "DatabaseConnections",
        unit: "Count",
        statistic: "Maximum",
        comparison_operator: "GreaterThanOrEqualToThreshold",
        threshold: 42.0,
        evaluation_periods: 3,
        datapoints_to_alarm: Some(2),
        treat_missing_data: None,
        effective_treat_missing_data: "missing",
        contact_protocols: &[],
        notification_triggers: &["ALARM"],
        notification_enabled: Some(true),
        effective_notification_enabled: true,
        tags: &[("team", "data")],
    },
    Row {
        case: "database ingress throughput, a rate unit",
        alarm_name: "dbNetworkIn12",
        monitored: Monitored::RelationalDatabase,
        region: "eu-central-1",
        zone: "eu-central-1a",
        metric_name: "NetworkReceiveThroughput",
        unit: "Bytes/Second",
        statistic: "Average",
        comparison_operator: "GreaterThanThreshold",
        threshold: 1_500_000.0,
        evaluation_periods: 12,
        datapoints_to_alarm: None,
        treat_missing_data: Some("missing"),
        effective_treat_missing_data: "missing",
        contact_protocols: &["Email"],
        notification_triggers: &["ALARM", "OK"],
        notification_enabled: Some(false),
        effective_notification_enabled: false,
        tags: &[],
    },
    Row {
        case: "the same metric name on the other resource type",
        alarm_name: "db_cpu_alarm_2",
        monitored: Monitored::RelationalDatabase,
        region: "sa-east-1",
        zone: "sa-east-1a",
        metric_name: "CPUUtilization",
        unit: "Percent",
        statistic: "Average",
        comparison_operator: "LessThanThreshold",
        threshold: 90.25,
        evaluation_periods: 1,
        datapoints_to_alarm: Some(1),
        treat_missing_data: None,
        effective_treat_missing_data: "missing",
        contact_protocols: &["Email", "SMS"],
        notification_triggers: &["ALARM"],
        notification_enabled: None,
        effective_notification_enabled: true,
        tags: &[("key", "value")],
    },
];

/// The `PutAlarm` body `row` describes, watching `monitored_name`.
fn put_body(row: &Row, monitored_name: &str) -> Value {
    let mut body = json!({
        "alarmName": row.alarm_name,
        "monitoredResourceName": monitored_name,
        "metricName": row.metric_name,
        "comparisonOperator": row.comparison_operator,
        "threshold": row.threshold,
        "evaluationPeriods": row.evaluation_periods,
    });
    if let Some(datapoints) = row.datapoints_to_alarm {
        body["datapointsToAlarm"] = json!(datapoints);
    }
    if let Some(treat_missing_data) = row.treat_missing_data {
        body["treatMissingData"] = json!(treat_missing_data);
    }
    if !row.contact_protocols.is_empty() {
        body["contactProtocols"] = json!(row.contact_protocols);
    }
    if !row.notification_triggers.is_empty() {
        body["notificationTriggers"] = json!(row.notification_triggers);
    }
    if let Some(enabled) = row.notification_enabled {
        body["notificationEnabled"] = json!(enabled);
    }
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    body
}

// ---------------------------------------------------------------------------
// Put and read back
// ---------------------------------------------------------------------------

/// RULE: every member `GetAlarms` reports comes from the one stored alarm — the
/// caller's own configuration verbatim, the documented default for each member
/// the request omitted, and every server-assigned member computed from the
/// monitored resource, the metric catalog and the store's allocators.
#[tokio::test]
async fn an_alarm_reads_back_as_it_was_put() {
    let emulator = Emulator::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let monitored_name = format!("watched-{index}");
        let monitored_arn =
            monitored_resource(&emulator, row.monitored, &monitored_name, row.zone, case).await;

        let put = emulator
            .ok("PutAlarm", put_body(row, &monitored_name))
            .await;

        // The put answers ONE operation-log entry, terminal: the alarm exists,
        // configured as asked, by the time the call returns.
        let operations = put["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {put:?}"));
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation, got {put:?}"
        );
        let operation = &operations[0];
        for (member, want) in [
            ("operationType", "PutAlarm"),
            ("resourceType", "Alarm"),
            ("resourceName", row.alarm_name),
            ("status", "Succeeded"),
        ] {
            assert_eq!(
                operation[member].as_str(),
                Some(want),
                "case {case}: expected {member} {want}, got {operation:?}"
            );
        }
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: the alarm is configured when the put answers, got {operation:?}"
        );
        assert!(
            operation["id"].as_str().is_some_and(is_uuid_shaped),
            "case {case}: expected a uuid-shaped operation id, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the operation in the monitored resource's region, \
             got {operation:?}"
        );

        let stored = alarm(&emulator, row.alarm_name, case).await;

        // The caller's own configuration, verbatim.
        assert_eq!(
            stored["name"].as_str(),
            Some(row.alarm_name),
            "case {case}: expected the requested name, got {stored:?}"
        );
        assert_eq!(
            stored["metricName"].as_str(),
            Some(row.metric_name),
            "case {case}: expected the requested metric, got {stored:?}"
        );
        assert_eq!(
            stored["comparisonOperator"].as_str(),
            Some(row.comparison_operator),
            "case {case}: expected the requested comparison operator, got {stored:?}"
        );
        assert_eq!(
            stored["threshold"].as_f64(),
            Some(row.threshold),
            "case {case}: expected the requested threshold, got {stored:?}"
        );
        assert_eq!(
            stored["evaluationPeriods"].as_i64(),
            Some(row.evaluation_periods),
            "case {case}: expected the requested evaluation periods, got {stored:?}"
        );
        assert_eq!(
            stored["datapointsToAlarm"].as_i64(),
            row.datapoints_to_alarm,
            "case {case}: expected the requested datapoints to alarm, got {stored:?}"
        );
        assert_eq!(
            stored["treatMissingData"].as_str(),
            Some(row.effective_treat_missing_data),
            "case {case}: expected treatMissingData {}, got {stored:?}",
            row.effective_treat_missing_data
        );
        assert_eq!(
            stored["contactProtocols"],
            json!(row.contact_protocols),
            "case {case}: expected the requested contact protocols, got {stored:?}"
        );
        assert_eq!(
            stored["notificationTriggers"],
            json!(row.notification_triggers),
            "case {case}: expected the requested notification triggers, got {stored:?}"
        );
        assert_eq!(
            stored["notificationEnabled"].as_bool(),
            Some(row.effective_notification_enabled),
            "case {case}: expected notificationEnabled {}, got {stored:?}",
            row.effective_notification_enabled
        );
        let want_tags = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
        assert_eq!(
            stored["tags"], want_tags,
            "case {case}: expected the requested tags, got {stored:?}"
        );

        // What the metric catalog, not the request, decides.
        assert_eq!(
            stored["unit"].as_str(),
            Some(row.unit),
            "case {case}: expected the published unit of {}, got {stored:?}",
            row.metric_name
        );
        assert_eq!(
            stored["statistic"].as_str(),
            Some(row.statistic),
            "case {case}: expected the published statistic of {}, got {stored:?}",
            row.metric_name
        );
        assert_eq!(
            stored["period"].as_i64(),
            Some(ALARM_PERIOD_SECONDS),
            "case {case}: expected a five-minute period, got {stored:?}"
        );

        // What the store and the monitored resource decide.
        assert_eq!(
            stored["resourceType"].as_str(),
            Some("Alarm"),
            "case {case}: expected resourceType Alarm, got {stored:?}"
        );
        assert_eq!(
            stored["state"].as_str(),
            Some(INSUFFICIENT_DATA),
            "case {case}: a new alarm has no data to evaluate, got {stored:?}"
        );
        assert_eq!(
            stored["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the monitored resource's region, got {stored:?}"
        );
        assert!(
            stored["location"]["availabilityZone"].is_null(),
            "case {case}: an alarm is not zonal, got {stored:?}"
        );
        assert_eq!(
            stored["monitoredResourceInfo"]["name"].as_str(),
            Some(monitored_name.as_str()),
            "case {case}: expected the monitored resource's name, got {stored:?}"
        );
        assert_eq!(
            stored["monitoredResourceInfo"]["resourceType"].as_str(),
            Some(row.monitored.resource_type()),
            "case {case}: expected the monitored resource's type, got {stored:?}"
        );
        assert_eq!(
            stored["monitoredResourceInfo"]["arn"].as_str(),
            Some(monitored_arn.as_str()),
            "case {case}: expected the arn the monitored resource's own read reports, \
             got {stored:?}"
        );
        let arn = stored["arn"].as_str().unwrap_or_default();
        let prefix = format!("arn:aws:lightsail:{}:{ACCOUNT}:Alarm/", row.region);
        assert!(
            arn.starts_with(&prefix),
            "case {case}: expected an arn under {prefix}, got {arn:?}"
        );
        assert!(
            is_uuid_shaped(&arn[prefix.len()..]),
            "case {case}: expected a uuid-shaped alarm id in {arn:?}"
        );
        assert!(
            arns.insert(arn.to_string()),
            "case {case}: {arn} was already handed to another alarm"
        );
        let support_code = stored["supportCode"].as_str().unwrap_or_default();
        assert!(
            support_code.starts_with(&format!("{ACCOUNT}/")),
            "case {case}: expected a support code under this account, got {support_code:?}"
        );
        assert!(
            stored["createdAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {case}: expected a creation timestamp, got {stored:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Re-putting one name
// ---------------------------------------------------------------------------

/// RULE: putting a name this account already holds UPDATES that one alarm — the
/// configuration is overwritten whole, so a member the second request omits
/// reverts to its default, while the alarm's identity and its state survive
/// ("the update completely overwrites the previous configuration of the alarm";
/// "when you update an existing alarm, its state is left unchanged").
#[tokio::test]
async fn re_putting_an_alarm_overwrites_its_configuration_and_keeps_its_identity() {
    let case = "re-put with a smaller configuration";
    let emulator = Emulator::new();
    monitored_resource(&emulator, Monitored::Instance, "box", "us-west-2a", case).await;

    emulator
        .ok(
            "PutAlarm",
            json!({
                "alarmName": "one-alarm",
                "monitoredResourceName": "box",
                "metricName": "CPUUtilization",
                "comparisonOperator": "GreaterThanThreshold",
                "threshold": 75.0,
                "evaluationPeriods": 6,
                "datapointsToAlarm": 4,
                "treatMissingData": "breaching",
                "contactProtocols": ["Email", "SMS"],
                "notificationTriggers": ["ALARM", "OK"],
                "notificationEnabled": false,
                "tags": [{"key": "stage", "value": "dev"}],
            }),
        )
        .await;
    let first = alarm(&emulator, "one-alarm", case).await;

    emulator
        .ok(
            "PutAlarm",
            json!({
                "alarmName": "one-alarm",
                "monitoredResourceName": "box",
                "metricName": "NetworkIn",
                "comparisonOperator": "LessThanThreshold",
                "threshold": 1.5,
                "evaluationPeriods": 2,
            }),
        )
        .await;
    let second = alarm(&emulator, "one-alarm", case).await;

    // Identity and state are not configuration.
    for member in ["arn", "supportCode", "createdAt", "state"] {
        assert_eq!(
            first[member], second[member],
            "case {case}: expected {member} to survive the update, \
             got {:?} then {:?}",
            first[member], second[member]
        );
    }
    assert_eq!(
        second["state"].as_str(),
        Some(INSUFFICIENT_DATA),
        "case {case}: expected the state left unchanged, got {second:?}"
    );

    // The configuration is the second request's, whole.
    for (member, want) in [
        ("metricName", json!("NetworkIn")),
        ("unit", json!("Bytes")),
        ("statistic", json!("Sum")),
        ("comparisonOperator", json!("LessThanThreshold")),
        ("threshold", json!(1.5)),
        ("evaluationPeriods", json!(2)),
        ("treatMissingData", json!("missing")),
        ("notificationEnabled", json!(true)),
        ("contactProtocols", json!([])),
        ("notificationTriggers", json!([])),
        ("tags", json!([])),
        ("datapointsToAlarm", Value::Null),
    ] {
        assert_eq!(
            second[member], want,
            "case {case}: expected {member} {want:?} after the overwrite, got {second:?}"
        );
    }

    // One name is one record: the update did not add a second alarm.
    let (found, _) = alarms(&emulator, json!({})).await;
    assert_eq!(
        names(&found),
        vec!["one-alarm".to_string()],
        "case {case}: expected the account to hold exactly one alarm, got {found:?}"
    );
}

// ---------------------------------------------------------------------------
// Two alarms per metric
// ---------------------------------------------------------------------------

/// RULE: one metric of one monitored resource carries at most the two alarms the
/// service documents ("You can configure up to two alarms per metric"); another
/// metric of the same resource is unaffected, and re-putting one of the two is an
/// update, not a third alarm.
#[tokio::test]
async fn one_metric_carries_at_most_two_alarms() {
    let case = "two alarms on one metric, then a third";
    let emulator = Emulator::new();
    monitored_resource(&emulator, Monitored::Instance, "box", "us-east-1a", case).await;

    let put = |name: &'static str, metric: &'static str| {
        let emulator = &emulator;
        async move {
            emulator
                .call(
                    "PutAlarm",
                    json!({
                        "alarmName": name,
                        "monitoredResourceName": "box",
                        "metricName": metric,
                        "comparisonOperator": "GreaterThanThreshold",
                        "threshold": 1.0,
                        "evaluationPeriods": 1,
                    }),
                )
                .await
        }
    };

    for name in ["cpu-one", "cpu-two"] {
        let (status, body) = put(name, "CPUUtilization").await;
        assert_eq!(
            status, 200,
            "case {case}: expected {name} to be accepted, got {body:?}"
        );
    }

    let (status, body) = put("cpu-three", "CPUUtilization").await;
    assert_eq!(
        status, 400,
        "case {case}: expected the third alarm on one metric to be refused, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case {case}: expected InvalidInputException, got {body:?}"
    );

    // A different metric of the same resource is a different metric.
    let (status, body) = put("net-one", "NetworkIn").await;
    assert_eq!(
        status, 200,
        "case {case}: expected another metric to be unaffected, got {body:?}"
    );

    // Re-putting an existing alarm overwrites it rather than taking a third slot.
    let (status, body) = put("cpu-two", "CPUUtilization").await;
    assert_eq!(
        status, 200,
        "case {case}: expected the re-put of an existing alarm to be accepted, got {body:?}"
    );

    let (found, _) = alarms(&emulator, json!({"monitoredResourceName": "box"})).await;
    assert_eq!(
        names(&found),
        vec![
            "cpu-one".to_string(),
            "cpu-two".to_string(),
            "net-one".to_string()
        ],
        "case {case}: expected the refused put to have stored nothing, got {found:?}"
    );
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// RULE: `DeleteAlarm` removes the one alarm it names from every later read, and
/// a name no alarm is stored under is refused with the service's `DoesNotExist`
/// envelope — so deleting twice fails the second time rather than reporting a
/// deletion that did not happen.
#[tokio::test]
async fn deleting_an_alarm_removes_it_from_every_read() {
    let case = "delete one of two alarms, then delete it again";
    let emulator = Emulator::new();
    monitored_resource(&emulator, Monitored::Instance, "box", "eu-west-1a", case).await;

    for (name, metric) in [("cpu", "CPUUtilization"), ("net", "NetworkOut")] {
        emulator
            .ok(
                "PutAlarm",
                json!({
                    "alarmName": name,
                    "monitoredResourceName": "box",
                    "metricName": metric,
                    "comparisonOperator": "GreaterThanThreshold",
                    "threshold": 10.0,
                    "evaluationPeriods": 1,
                }),
            )
            .await;
    }

    let deleted = emulator.ok("DeleteAlarm", json!({"alarmName": "cpu"})).await;
    let operations = deleted["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {deleted:?}"));
    assert_eq!(
        operations.len(),
        1,
        "case {case}: expected exactly one operation, got {deleted:?}"
    );
    let operation = &operations[0];
    for (member, want) in [
        ("operationType", "DeleteAlarm"),
        ("resourceType", "Alarm"),
        ("resourceName", "cpu"),
        ("status", "Succeeded"),
    ] {
        assert_eq!(
            operation[member].as_str(),
            Some(want),
            "case {case}: expected {member} {want}, got {operation:?}"
        );
    }
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: the alarm is gone when the delete answers, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some("eu-west-1"),
        "case {case}: expected the deleted alarm's own region, got {operation:?}"
    );

    let (remaining, _) = alarms(&emulator, json!({})).await;
    assert_eq!(
        names(&remaining),
        vec!["net".to_string()],
        "case {case}: expected only the other alarm to remain, got {remaining:?}"
    );
    let (gone, _) = alarms(&emulator, json!({"alarmName": "cpu"})).await;
    assert!(
        gone.is_empty(),
        "case {case}: expected the deleted name to match no alarm, got {gone:?}"
    );

    for name in ["cpu", "never-existed"] {
        let (status, body) = emulator
            .call("DeleteAlarm", json!({"alarmName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected deleting {name} to be refused, got {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected NotFoundException for {name}, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the DoesNotExist envelope for {name}, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters and paging
// ---------------------------------------------------------------------------

/// RULE: `alarmName` and `monitoredResourceName` are FILTERS over the stored
/// alarms — an alarm is reported when it satisfies every filter the request
/// names, a filter that matches nothing reports an empty list, and the walk pages
/// through the store's own total order without skipping or repeating an alarm.
#[tokio::test]
async fn alarms_are_filtered_and_paged() {
    let case = "twelve alarms on an instance and three on a database";
    let emulator = Emulator::new();
    monitored_resource(&emulator, Monitored::Instance, "box", "us-east-1a", case).await;
    monitored_resource(
        &emulator,
        Monitored::RelationalDatabase,
        "db",
        "us-east-1a",
        case,
    )
    .await;

    // Six instance metrics, two alarms each: the most one metric accepts.
    let instance_metrics = [
        "CPUUtilization",
        "NetworkIn",
        "NetworkOut",
        "DiskReadOps",
        "DiskWriteOps",
        "BurstCapacityTime",
    ];
    let mut instance_alarms = Vec::new();
    for metric in instance_metrics {
        for suffix in ["a", "b"] {
            let name = format!("box-{metric}-{suffix}");
            emulator
                .ok(
                    "PutAlarm",
                    json!({
                        "alarmName": name,
                        "monitoredResourceName": "box",
                        "metricName": metric,
                        "comparisonOperator": "GreaterThanThreshold",
                        "threshold": 1.0,
                        "evaluationPeriods": 1,
                    }),
                )
                .await;
            instance_alarms.push(name);
        }
    }
    let mut database_alarms = Vec::new();
    for metric in ["CPUUtilization", "DiskQueueDepth", "FreeStorageSpace"] {
        let name = format!("db-{metric}");
        emulator
            .ok(
                "PutAlarm",
                json!({
                    "alarmName": name,
                    "monitoredResourceName": "db",
                    "metricName": metric,
                    "comparisonOperator": "LessThanThreshold",
                    "threshold": 2.0,
                    "evaluationPeriods": 1,
                }),
            )
            .await;
        database_alarms.push(name);
    }

    let mut every_name: Vec<String> = instance_alarms
        .iter()
        .chain(database_alarms.iter())
        .cloned()
        .collect();
    every_name.sort();

    let (all, page_sizes) = alarms(&emulator, json!({})).await;
    assert_eq!(
        names(&all),
        every_name,
        "case {case}: expected every alarm exactly once in name order, got {:?}",
        names(&all)
    );
    assert!(
        page_sizes.len() > 1 && page_sizes.iter().all(|size| *size <= 10),
        "case {case}: expected a bounded multi-page walk, got pages {page_sizes:?}"
    );

    let mut want_instance = instance_alarms.clone();
    want_instance.sort();
    let (on_instance, instance_pages) =
        alarms(&emulator, json!({"monitoredResourceName": "box"})).await;
    assert_eq!(
        names(&on_instance),
        want_instance,
        "case {case}: expected only the instance's alarms, got {:?}",
        names(&on_instance)
    );
    assert!(
        instance_pages.iter().all(|size| *size <= 10),
        "case {case}: expected bounded pages, got {instance_pages:?}"
    );

    let (one, _) = alarms(&emulator, json!({"alarmName": "db-DiskQueueDepth"})).await;
    assert_eq!(
        names(&one),
        vec!["db-DiskQueueDepth".to_string()],
        "case {case}: expected the one named alarm, got {one:?}"
    );

    // Both filters apply: this alarm exists, but not on that resource.
    let (neither, _) = alarms(
        &emulator,
        json!({"alarmName": "db-DiskQueueDepth", "monitoredResourceName": "box"}),
    )
    .await;
    assert!(
        neither.is_empty(),
        "case {case}: expected both filters to apply, got {neither:?}"
    );

    for filters in [
        json!({"alarmName": "no-such-alarm"}),
        json!({"monitoredResourceName": "no-such-resource"}),
    ] {
        let (empty, pages) = alarms(&emulator, filters.clone()).await;
        assert!(
            empty.is_empty() && pages == vec![0],
            "case {case}: expected {filters:?} to report an empty list, got {empty:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The published metric catalog
// ---------------------------------------------------------------------------

/// Every metric an alarm can watch on a resource this emulator can create, with
/// the published `unit` and the statistic an alarm on it reports.
///
/// WHICH metrics each resource type publishes is the list in
/// `smithy_query(query="doc",shape="PutAlarmRequest",member="metricName")`; the
/// unit and the statistic of each are the "published unit value" and the most
/// useful statistic stated by
/// `smithy_query(query="doc",shape="GetInstanceMetricDataRequest",member="metricName")`
/// and the same for `GetRelationalDatabaseMetricDataRequest`.
const CATALOG: &[(Monitored, &str, &str, &str)] = &[
    (
        Monitored::Instance,
        "BurstCapacityPercentage",
        "Percent",
        "Average",
    ),
    (
        Monitored::Instance,
        "BurstCapacityTime",
        "Seconds",
        "Average",
    ),
    (Monitored::Instance, "CPUUtilization", "Percent", "Average"),
    (Monitored::Instance, "NetworkIn", "Bytes", "Sum"),
    (Monitored::Instance, "NetworkOut", "Bytes", "Sum"),
    (Monitored::Instance, "StatusCheckFailed", "Count", "Sum"),
    (
        Monitored::Instance,
        "StatusCheckFailed_Instance",
        "Count",
        "Sum",
    ),
    (
        Monitored::Instance,
        "StatusCheckFailed_System",
        "Count",
        "Sum",
    ),
    (Monitored::Instance, "DiskReadOps", "Count", "Sum"),
    (Monitored::Instance, "DiskWriteOps", "Count", "Sum"),
    (Monitored::Instance, "DiskReadBytes", "Bytes", "Sum"),
    (Monitored::Instance, "DiskWriteBytes", "Bytes", "Sum"),
    (
        Monitored::Instance,
        "DiskTotalReadTime",
        "Seconds",
        "Average",
    ),
    (
        Monitored::Instance,
        "DiskTotalWriteTime",
        "Seconds",
        "Average",
    ),
    (Monitored::Instance, "DiskQueueLength", "Count", "Average"),
    (
        Monitored::RelationalDatabase,
        "CPUUtilization",
        "Percent",
        "Average",
    ),
    (
        Monitored::RelationalDatabase,
        "DatabaseConnections",
        "Count",
        "Maximum",
    ),
    (
        Monitored::RelationalDatabase,
        "DiskQueueDepth",
        "Count",
        "Sum",
    ),
    (
        Monitored::RelationalDatabase,
        "FreeStorageSpace",
        "Bytes",
        "Sum",
    ),
    (
        Monitored::RelationalDatabase,
        "NetworkReceiveThroughput",
        "Bytes/Second",
        "Average",
    ),
    (
        Monitored::RelationalDatabase,
        "NetworkTransmitThroughput",
        "Bytes/Second",
        "Average",
    ),
];

/// RULE: the `unit` and `statistic` of an alarm are properties of its metric —
/// `PutAlarm` takes neither — so each published metric of each monitorable
/// resource type reports its own published pair.
#[tokio::test]
async fn each_published_metric_reports_its_own_unit_and_statistic() {
    let emulator = Emulator::new();
    monitored_resource(
        &emulator,
        Monitored::Instance,
        "box",
        "us-east-1a",
        "the metric catalog",
    )
    .await;
    monitored_resource(
        &emulator,
        Monitored::RelationalDatabase,
        "db",
        "us-east-1a",
        "the metric catalog",
    )
    .await;

    for (kind, metric, unit, statistic) in CATALOG {
        let case = format!("{metric} on a {}", kind.resource_type());
        let name = format!("alarm-{}-{metric}", kind.resource_type());
        let monitored = match kind {
            Monitored::Instance => "box",
            Monitored::RelationalDatabase => "db",
        };
        emulator
            .ok(
                "PutAlarm",
                json!({
                    "alarmName": name,
                    "monitoredResourceName": monitored,
                    "metricName": metric,
                    "comparisonOperator": "GreaterThanOrEqualToThreshold",
                    "threshold": 1.0,
                    "evaluationPeriods": 1,
                }),
            )
            .await;
        let stored = alarm(&emulator, &name, &case).await;
        assert_eq!(
            stored["unit"].as_str(),
            Some(*unit),
            "case {case}: expected unit {unit}, got {stored:?}"
        );
        assert_eq!(
            stored["statistic"].as_str(),
            Some(*statistic),
            "case {case}: expected statistic {statistic}, got {stored:?}"
        );
        assert_eq!(
            stored["monitoredResourceInfo"]["resourceType"].as_str(),
            Some(kind.resource_type()),
            "case {case}: expected the monitored resource's type, got {stored:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One request the service must refuse, and the wire error code it answers with.
struct Refusal {
    case: &'static str,
    /// The `PutAlarm` body, built over the instance `box` and the database `db`
    /// this suite's refusal case creates.
    body: fn() -> Value,
    error_code: &'static str,
}

/// A `PutAlarm` body watching `monitored` with everything else valid, so each
/// refusal row differs in exactly the member it is about.
fn refusal_body(monitored: &str, metric: &str) -> Value {
    json!({
        "alarmName": "refused",
        "monitoredResourceName": monitored,
        "metricName": metric,
        "comparisonOperator": "GreaterThanThreshold",
        "threshold": 1.0,
        "evaluationPeriods": 4,
    })
}

/// The refusal table.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a monitored resource this account does not hold",
        body: || refusal_body("no-such-resource", "CPUUtilization"),
        error_code: "NotFoundException",
    },
    Refusal {
        case: "a load balancer metric on an instance",
        body: || refusal_body("box", "HealthyHostCount"),
        error_code: "InvalidInputException",
    },
    Refusal {
        case: "a database metric on an instance",
        body: || refusal_body("box", "DatabaseConnections"),
        error_code: "InvalidInputException",
    },
    Refusal {
        case: "an instance metric on a database",
        body: || refusal_body("db", "NetworkOut"),
        error_code: "InvalidInputException",
    },
    Refusal {
        case: "a metric the alarm documentation lists for no resource type",
        body: || refusal_body("db", "FreeableMemory"),
        error_code: "InvalidInputException",
    },
    Refusal {
        case: "one evaluation period below the published minimum",
        body: || {
            let mut body = refusal_body("box", "CPUUtilization");
            body["evaluationPeriods"] = json!(0);
            body
        },
        error_code: "InvalidInputException",
    },
    Refusal {
        case: "one evaluation period past the published maximum",
        body: || {
            let mut body = refusal_body("box", "CPUUtilization");
            body["evaluationPeriods"] = json!(289);
            body
        },
        error_code: "InvalidInputException",
    },
    Refusal {
        case: "an M of zero in an M out of N alarm",
        body: || {
            let mut body = refusal_body("box", "CPUUtilization");
            body["datapointsToAlarm"] = json!(0);
            body
        },
        error_code: "InvalidInputException",
    },
    Refusal {
        case: "an M larger than N in an M out of N alarm",
        body: || {
            let mut body = refusal_body("box", "CPUUtilization");
            body["datapointsToAlarm"] = json!(5);
            body
        },
        error_code: "InvalidInputException",
    },
    Refusal {
        case: "notification triggers that do not name the ALARM state",
        body: || {
            let mut body = refusal_body("box", "CPUUtilization");
            body["notificationTriggers"] = json!(["OK", "INSUFFICIENT_DATA"]);
            body
        },
        error_code: "InvalidInputException",
    },
    Refusal {
        case: "a metric name the model does not admit",
        body: || {
            let mut body = refusal_body("box", "CPUUtilization");
            body["metricName"] = json!("NotAMetric");
            body
        },
        error_code: "ValidationException",
    },
    Refusal {
        case: "a comparison operator the model does not admit",
        body: || {
            let mut body = refusal_body("box", "CPUUtilization");
            body["comparisonOperator"] = json!("EqualToThreshold");
            body
        },
        error_code: "ValidationException",
    },
];

/// RULE: every refusal is answered BEFORE the first write, so a request the
/// service cannot serve leaves the store exactly as it was.
#[tokio::test]
async fn an_unservable_put_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    monitored_resource(&emulator, Monitored::Instance, "box", "us-east-1a", "setup").await;
    monitored_resource(
        &emulator,
        Monitored::RelationalDatabase,
        "db",
        "us-east-1a",
        "setup",
    )
    .await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator.call("PutAlarm", (refusal.body)()).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the request to be refused with 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            refusal.error_code,
            "case {case}: expected {}, got {body:?}",
            refusal.error_code
        );
        let (stored, _) = alarms(&emulator, json!({})).await;
        assert!(
            stored.is_empty(),
            "case {case}: expected the refusal to store nothing, got {stored:?}"
        );
    }

    // A tag list past the bound this service accepts is refused the same way.
    let mut body = refusal_body("box", "CPUUtilization");
    body["tags"] = Value::Array(
        (0..51)
            .map(|index| json!({"key": format!("key-{index}"), "value": "v"}))
            .collect(),
    );
    let (status, response) = emulator.call("PutAlarm", body).await;
    assert_eq!(
        status, 400,
        "case 51 tags: expected the request to be refused, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidInputException",
        "case 51 tags: expected InvalidInputException, got {response:?}"
    );
    let (stored, _) = alarms(&emulator, json!({})).await;
    assert!(
        stored.is_empty(),
        "case 51 tags: expected the refusal to store nothing, got {stored:?}"
    );
}
