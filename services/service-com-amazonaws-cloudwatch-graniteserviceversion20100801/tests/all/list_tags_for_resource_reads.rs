// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` as a REPORT: "Displays the tags associated with a CloudWatch
//! resource" (`ListTagsForResource`).
//!
//! The rule under test is that the operation reports the one tag set the resource its
//! `ResourceARN` names holds and changes nothing a later caller sees: reading it any number
//! of times answers the same tag set, in the same order, and leaves the set the tagging
//! writers go on to merge into exactly as it was — and a resource that holds no tags answers
//! an EMPTY list, because the resource exists and the answer is that it carries nothing.
//!
//! Each table's rows differ along what the rule generalises over: the KIND of resource the
//! ARN names (which writer created it, and so which store its tag set lives in) and HOW MANY
//! tags it carries — none, one, a handful, and the published maximum — so the recorded
//! tag-then-list sequence is one row among many rather than the specification.

use crate::harness::{build_router, described_alarm, merged, ok, AlarmKind, QUERY_ROLE};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The Region the harness' SigV4 credential scope names, and so the one the ARNs of the
/// resources these tests create carry.
const REGION: &str = "us-east-1";

/// "You can associate as many as 50 tags with a CloudWatch resource." (`TagResource`)
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many times a row reads the tag set back: more than once, because reading twice is
/// what a report that consumed or moved anything would differ on.
const READS_PER_ROW: usize = 3;

/// One tag set, as a table spells it out: (key, value) pairs in report order.
type Tags = Vec<(String, String)>;

/// Every resource kind this service can tag: "Currently, alarms, dashboards, metric streams
/// and Contributor Insights rules support tagging." (`ListTagsForResource`) — the alarm kinds
/// are written by different operations into different stores, so each is its own row.
const TAGGABLE_KINDS: [&str; 6] = [
    "metric alarm",
    "composite alarm",
    "log alarm",
    "metric stream",
    "Contributor Insights rule",
    "dashboard",
];

/// One tag, as the wire carries it.
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// A tag list body member built from (key, value) pairs.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(tags.iter().map(|(key, value)| tag(key, value)).collect())
}

/// `count` distinct tags, so a row can carry any size of tag set up to the published maximum
/// without spelling each pair out.
fn generated_tags(count: usize) -> Tags {
    (0..count)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect()
}

/// The tags a `ListTagsForResource` body reports, in the order it reported them.
fn reported_tags(body: &Value) -> Tags {
    body.get("Tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|entry| {
                    (
                        entry
                            .get("Key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        entry
                            .get("Value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `ListTagsForResource` body for `arn`, requiring a 200.
async fn list_tags_body(router: &MultiServiceRouter, arn: &str) -> Value {
    ok(router, "ListTagsForResource", &json!({"ResourceARN": arn})).await
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Tags {
    reported_tags(&list_tags_body(router, arn).await)
}

/// A Contributor Insights rule definition in the rule syntax, which `PutInsightRule`
/// requires.
fn insight_rule_definition() -> String {
    json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": ["API-Gateway-Access-Logs"],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.ip"]},
    })
    .to_string()
}

/// The ARN `DescribeAlarms` reports for the alarm `name` of `kind`, read through the shared
/// harness' one alarm reader.
async fn described_alarm_arn(router: &MultiServiceRouter, kind: AlarmKind, name: &str) -> String {
    let alarm = described_alarm(router, kind, name)
        .await
        .unwrap_or_else(|| panic!("case {kind:?}/{name}: DescribeAlarms reports the alarm"));
    alarm
        .get("AlarmArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {kind:?}/{name}: the alarm carries an AlarmArn"))
        .to_string()
}

/// Create the resource of `kind` named `name` carrying `tags`, and answer its ARN.
///
/// A row that names NO tags leaves the `Tags` member out of the create request entirely —
/// `merged` drops the member when the patch leaves it null — so an untagged resource is one
/// whose writer never named the member, not one that named an empty list.
async fn create_resource(
    router: &MultiServiceRouter,
    kind: &str,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let member = if tags.is_empty() {
        Value::Null
    } else {
        tag_list(tags)
    };
    let tagged = |request: Value| merged(request, &json!({"Tags": member.clone()}));
    match kind {
        "metric alarm" => {
            ok(
                router,
                "PutMetricAlarm",
                &tagged(json!({
                    "AlarmName": name,
                    "Namespace": "MyApp",
                    "MetricName": "Latency",
                    "Statistic": "Average",
                    "Period": 60,
                    "EvaluationPeriods": 1,
                    "Threshold": 1.0,
                    "ComparisonOperator": "GreaterThanThreshold",
                })),
            )
            .await;
            described_alarm_arn(router, AlarmKind::Metric, name).await
        }
        "composite alarm" => {
            ok(
                router,
                "PutCompositeAlarm",
                // A literal rule: these rows are about the TAGS a composite alarm carries,
                // and a rule naming an alarm is refused unless that alarm exists.
                &tagged(json!({"AlarmName": name, "AlarmRule": "TRUE"})),
            )
            .await;
            described_alarm_arn(router, AlarmKind::Composite, name).await
        }
        "log alarm" => {
            ok(
                router,
                "PutLogAlarm",
                &tagged(json!({
                    "AlarmName": name,
                    "QueryResultsToEvaluate": 3,
                    "QueryResultsToAlarm": 2,
                    "Threshold": 10.0,
                    "ComparisonOperator": "GreaterThanThreshold",
                    "ScheduledQueryConfiguration": {
                        "QueryString": "fields @message | filter @message like /ERROR/",
                        "ScheduledQueryRoleARN": QUERY_ROLE,
                        "AggregationExpression": "count(*)",
                        "ScheduleConfiguration": {
                            "ScheduleExpression": "rate(5 minutes)",
                            "StartTimeOffset": 360,
                        },
                    },
                })),
            )
            .await;
            described_alarm_arn(router, AlarmKind::Log, name).await
        }
        "metric stream" => {
            let body = ok(
                router,
                "PutMetricStream",
                &tagged(json!({
                    "Name": name,
                    "FirehoseArn": format!("arn:aws:firehose:{REGION}:{ACCOUNT}:deliverystream/metrics"),
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/MetricStreamRole"),
                    "OutputFormat": "json",
                })),
            )
            .await;
            body.get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case metric stream/{name}: PutMetricStream answers an Arn"))
                .to_string()
        }
        "Contributor Insights rule" => {
            ok(
                router,
                "PutInsightRule",
                &tagged(json!({
                    "RuleName": name,
                    "RuleDefinition": insight_rule_definition(),
                })),
            )
            .await;
            // "The ARN format of a Contributor Insights rule is
            // arn:aws:cloudwatch:Region:account-id:insight-rule/insight-rule-name"
            // (`ListTagsForResourceInput$ResourceARN`) — no reader of this service reports
            // it, so a caller builds it from the documented format, as this does.
            format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:insight-rule/{name}")
        }
        "dashboard" => {
            ok(
                router,
                "PutDashboard",
                &tagged(json!({
                    "DashboardName": name,
                    "DashboardBody": "{\"widgets\":[]}",
                })),
            )
            .await;
            let body = ok(router, "GetDashboard", &json!({"DashboardName": name})).await;
            body.get("DashboardArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case dashboard/{name}: GetDashboard answers an ARN"))
                .to_string()
        }
        other => panic!("the table names no resource kind {other}"),
    }
}

#[tokio::test]
async fn list_tags_for_resource_reports_the_held_set_however_often_it_is_read() {
    // Rows differ along the resource kind the ARN names and along how many tags that
    // resource holds: one, a handful, and the published maximum of 50.
    let cases: Vec<(&str, usize)> = vec![
        ("metric alarm", 2),
        ("composite alarm", 1),
        ("log alarm", 4),
        ("metric stream", 3),
        ("Contributor Insights rule", MAX_TAGS_PER_RESOURCE),
        ("dashboard", 1),
    ];

    for (kind, count) in cases {
        let case = format!("{kind} with {count} tag(s)");
        let router = build_router();
        let created = generated_tags(count);
        let arn = create_resource(&router, kind, "read-many-times", &created).await;

        // Reading the set repeatedly answers the same tags in the same order: the report
        // neither consumes nor reorders what the resource holds.
        for read in 1..=READS_PER_ROW {
            let reported = list_tags(&router, &arn).await;
            assert_eq!(
                reported, created,
                "case {case:?}: read {read} expected the held tags {created:?}, got {reported:?}"
            );
        }

        // And the set the reads reported is still the set the writers merge into: a key
        // already held takes the new value in place, which only holds if the reads left the
        // stored set — its keys and their positions — alone.
        let (held_key, held_value) = created
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the row creates at least one tag"));
        ok(
            &router,
            "TagResource",
            &json!({"ResourceARN": arn, "Tags": [tag(&held_key, "replaced")]}),
        )
        .await;
        let mut expected = created.clone();
        expected[0].1 = "replaced".to_string();
        let reported = list_tags(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} after TagResource, got {reported:?}"
        );

        // Putting that value back leaves the set the reads reported, so nothing the reads
        // did survived into the writers' view of it.
        ok(
            &router,
            "TagResource",
            &json!({"ResourceARN": arn, "Tags": [tag(&held_key, &held_value)]}),
        )
        .await;
        let reported = list_tags(&router, &arn).await;
        assert_eq!(
            reported, created,
            "case {case:?}: expected the held tags {created:?} once the row's own write was \
             undone, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_holding_no_tags_reports_an_empty_tag_list() {
    // One row per taggable resource kind, each created WITHOUT a `Tags` member at all: the
    // resource exists, so the report is an empty list rather than a refusal — and reading it
    // again still reports nothing, rather than materialising a tag set.
    for kind in TAGGABLE_KINDS {
        let router = build_router();
        let arn = create_resource(&router, kind, "untagged-resource", &[]).await;

        for read in 1..=READS_PER_ROW {
            let body = list_tags_body(&router, &arn).await;
            let tags = body.get("Tags");
            assert!(
                tags.is_some_and(|tags| tags.as_array().is_some_and(|tags| tags.is_empty())),
                "case {kind:?}: read {read} expected an empty Tags list, got {body:?}"
            );
        }

        // A tag written after those reads is the FIRST tag of that set, which is only true
        // if the reads added none of their own.
        ok(
            &router,
            "TagResource",
            &json!({"ResourceARN": arn, "Tags": [tag("Env", "prod")]}),
        )
        .await;
        let reported = list_tags(&router, &arn).await;
        let expected: Tags = vec![("Env".to_string(), "prod".to_string())];
        assert_eq!(
            reported, expected,
            "case {kind:?}: expected {expected:?} after the first TagResource, got {reported:?}"
        );
    }
}
