// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the CloudWatch resource-tagging loop: `TagResource` writing tags
//! onto a resource, `ListTagsForResource` reading them back, and `UntagResource` removing
//! the keys it names.
//!
//! The rule under test is that every taggable resource carries ONE tag set, keyed by the
//! resource its ARN names: a key already held takes the new value in place, a key not held
//! is appended, `UntagResource` removes only the keys it names, and a resource's writer
//! leaves that set alone once the resource exists. Each table's rows differ along what the
//! rule generalises over — the KIND of resource the ARN names (metric, composite and log
//! alarms, metric streams and Contributor Insights rules), how many tags the resource was
//! created with, the keys and values a request carries, and how many keys it removes — so a
//! recorded tag/list/untag sequence is one row among many rather than the specification.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The Region the harness' SigV4 credential scope names, and so the one the ARNs of the
/// resources these tests create carry.
const REGION: &str = "us-east-1";

/// The three operations that take a `ResourceARN`, for the tables that assert how all of
/// them answer one ARN.
const TAGGING_OPERATIONS: [&str; 3] = ["TagResource", "ListTagsForResource", "UntagResource"];

/// Every resource kind this service can tag: "Currently, the only CloudWatch resources that
/// can be tagged are alarms, dashboards, metric streams and Contributor Insights rules."
/// (`TagResource`) — the alarm kinds are enumerated separately because each is written by a
/// different operation into a different store.
const TAGGABLE_KINDS: [&str; 6] = [
    "metric alarm",
    "composite alarm",
    "log alarm",
    "metric stream",
    "Contributor Insights rule",
    "dashboard",
];

/// One tag set, as a table spells it out: (key, value) pairs in report order.
type Tags = Vec<(String, String)>;

/// One row of the merge table: the case, the tags the resource holds, the tags the request
/// carries, and the set expected afterwards.
type MergeCase = (&'static str, Tags, Tags, Tags);

/// One row of the removal table: the case, the tags the resource holds, the keys the request
/// names, and the set expected afterwards.
type RemovalCase = (&'static str, Tags, Vec<&'static str>, Tags);

/// One tag, as the wire carries it.
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// A tag list body member built from (key, value) pairs.
fn tag_list(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| tag(key, value))
            .collect::<Vec<Value>>(),
    )
}

/// (key, value) pairs, for the tables that spell their tags out as literals.
fn pairs(tags: &[(&str, &str)]) -> Tags {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
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

/// The tags `ListTagsForResource` reports for `arn`, requiring a 200.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Tags {
    let body = ok(router, "ListTagsForResource", &json!({"ResourceARN": arn})).await;
    reported_tags(&body)
}

/// Assign `tags` to `arn`, requiring a 200.
async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: &[(String, String)]) {
    ok(
        router,
        "TagResource",
        &json!({"ResourceARN": arn, "Tags": tag_list(tags)}),
    )
    .await;
}

/// Remove `keys` from `arn`, requiring a 200.
async fn untag_resource(router: &MultiServiceRouter, arn: &str, keys: &[&str]) {
    ok(
        router,
        "UntagResource",
        &json!({"ResourceARN": arn, "TagKeys": keys}),
    )
    .await;
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

/// The ARN `DescribeAlarms` reports for the alarm `name`, read out of the `reported` member
/// its kind is listed under.
async fn described_alarm_arn(
    router: &MultiServiceRouter,
    name: &str,
    alarm_type: &str,
    reported: &str,
) -> String {
    let body = ok(
        router,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": [alarm_type]}),
    )
    .await;
    let alarms = body
        .get(reported)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        alarms.len(),
        1,
        "{alarm_type} {name}: expected exactly one described alarm, got {alarms:?}"
    );
    alarms[0]
        .get("AlarmArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{alarm_type} {name}: the described alarm carries an AlarmArn"))
        .to_string()
}

/// Write the resource of `kind` named `name` with `tags`, and answer its ARN.
///
/// Calling this twice for one `kind` and `name` is that kind's writer UPDATING the resource,
/// which is what the update table drives.
async fn write_resource(
    router: &MultiServiceRouter,
    kind: &str,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let tags = tag_list(tags);
    match kind {
        "metric alarm" => {
            ok(
                router,
                "PutMetricAlarm",
                &json!({
                    "AlarmName": name,
                    "Namespace": "MyApp",
                    "MetricName": "Latency",
                    "Statistic": "Average",
                    "Period": 60,
                    "EvaluationPeriods": 1,
                    "Threshold": 1.0,
                    "ComparisonOperator": "GreaterThanThreshold",
                    "Tags": tags,
                }),
            )
            .await;
            described_alarm_arn(router, name, "MetricAlarm", "MetricAlarms").await
        }
        "composite alarm" => {
            ok(
                router,
                "PutCompositeAlarm",
                &json!({
                    "AlarmName": name,
                    "AlarmRule": "ALARM(\"a-child-alarm\")",
                    "Tags": tags,
                }),
            )
            .await;
            described_alarm_arn(router, name, "CompositeAlarm", "CompositeAlarms").await
        }
        "log alarm" => {
            ok(
                router,
                "PutLogAlarm",
                &json!({
                    "AlarmName": name,
                    "QueryResultsToEvaluate": 3,
                    "QueryResultsToAlarm": 2,
                    "Threshold": 10.0,
                    "ComparisonOperator": "GreaterThanThreshold",
                    "ScheduledQueryConfiguration": {
                        "QueryString": "fields @message | filter @message like /ERROR/",
                        "ScheduledQueryRoleARN": "arn:aws:iam::000000000000:role/cwl-query",
                        "AggregationExpression": "count(*)",
                        "ScheduleConfiguration": {
                            "ScheduleExpression": "rate(5 minutes)",
                            "StartTimeOffset": 360,
                        },
                    },
                    "Tags": tags,
                }),
            )
            .await;
            described_alarm_arn(router, name, "LogAlarm", "LogAlarms").await
        }
        "metric stream" => {
            let body = ok(
                router,
                "PutMetricStream",
                &json!({
                    "Name": name,
                    "FirehoseArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/metrics",
                    "RoleArn": "arn:aws:iam::000000000000:role/MetricStreamRole",
                    "OutputFormat": "json",
                    "Tags": tags,
                }),
            )
            .await;
            body.get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("metric stream {name}: PutMetricStream answers an Arn"))
                .to_string()
        }
        "Contributor Insights rule" => {
            ok(
                router,
                "PutInsightRule",
                &json!({
                    "RuleName": name,
                    "RuleDefinition": insight_rule_definition(),
                    "Tags": tags,
                }),
            )
            .await;
            // "The ARN format of a Contributor Insights rule is
            // arn:aws:cloudwatch:Region:account-id:insight-rule/insight-rule-name"
            // (`TagResourceInput$ResourceARN`) — no reader of this service reports it, so a
            // caller builds it from the documented format, as this does.
            format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:insight-rule/{name}")
        }
        "dashboard" => {
            ok(
                router,
                "PutDashboard",
                &json!({
                    "DashboardName": name,
                    "DashboardBody": "{\"widgets\":[]}",
                    "Tags": tags,
                }),
            )
            .await;
            let body = ok(router, "GetDashboard", &json!({"DashboardName": name})).await;
            body.get("DashboardArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("dashboard {name}: GetDashboard answers a DashboardArn"))
                .to_string()
        }
        other => panic!("the table names no resource kind {other}"),
    }
}

/// The request each tagging operation takes for `arn`, carrying only the members its own
/// input shape declares.
fn tagging_request(operation: &str, arn: &str) -> Value {
    match operation {
        "TagResource" => json!({"ResourceARN": arn, "Tags": [tag("Env", "prod")]}),
        "UntagResource" => json!({"ResourceARN": arn, "TagKeys": ["Env"]}),
        _ => json!({"ResourceARN": arn}),
    }
}

/// A key no row attaches, so a request naming it adds or removes nothing held.
const UNATTACHED_KEY: &str = "never-attached";

#[tokio::test]
async fn tags_round_trip_through_every_taggable_resource_kind() {
    // Rows differ along the KIND of resource the ARN names — which writer created it, and so
    // which store the one tag set lives in — and along how many tags it was created with and
    // what those keys and values look like: none at all, an empty value ("len 0..256",
    // `Tag$Value`), and a key and value at the published maximum lengths ("len 1..128",
    // `Tag$Key`; "len 0..256", `Tag$Value`).
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let cases: Vec<(&str, Tags)> = vec![
        (
            "metric alarm",
            pairs(&[("Team", "payments"), ("Env", "prod")]),
        ),
        ("composite alarm", Vec::new()),
        ("log alarm", pairs(&[("Owner", "")])),
        (
            "metric stream",
            pairs(&[("Team", "observability"), ("Env", "staging"), ("Tier", "1")]),
        ),
        (
            "Contributor Insights rule",
            vec![(longest_key.clone(), longest_value.clone())],
        ),
        ("dashboard", pairs(&[("Env", "prod"), ("Owner", "ops")])),
    ];

    for (kind, created) in cases {
        let router = build_router();
        let arn = write_resource(&router, kind, "tagged-resource", &created).await;

        // The tags the resource was created with are the set the reader reports: there is one
        // tag set per resource, not one per writer.
        let reported = list_tags(&router, &arn).await;
        assert_eq!(
            reported, created,
            "case {kind:?}: expected the created tags {created:?}, got {reported:?}"
        );

        // A request replacing the value of a key already held and adding one that is not.
        let mut request = Vec::new();
        let mut expected = created.clone();
        if let Some((key, _)) = created.first() {
            request.push((key.clone(), "replaced".to_string()));
            expected[0].1 = "replaced".to_string();
        }
        request.push(("Cost-Center".to_string(), "8821".to_string()));
        expected.push(("Cost-Center".to_string(), "8821".to_string()));

        tag_resource(&router, &arn, &request).await;
        let reported = list_tags(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {kind:?}: expected {expected:?} after TagResource, got {reported:?}"
        );

        // Removing a held key and one that was never attached leaves the rest untouched.
        untag_resource(&router, &arn, &["Cost-Center", UNATTACHED_KEY]).await;
        expected.pop();
        let reported = list_tags(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {kind:?}: expected {expected:?} after UntagResource, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn tag_resource_merges_a_request_into_the_tags_already_held() {
    // Rows differ along how many tags the resource holds, how many the request carries, which
    // of them are already held, whether the request names one key twice, and whether a value
    // is replaced with the empty string.
    let cases: Vec<MergeCase> = vec![
        (
            "one tag onto an untagged resource",
            Vec::new(),
            pairs(&[("Env", "prod")]),
            pairs(&[("Env", "prod")]),
        ),
        (
            "three new keys are appended in request order",
            pairs(&[("Env", "prod")]),
            pairs(&[("Team", "payments"), ("Tier", "1"), ("Owner", "ops")]),
            pairs(&[
                ("Env", "prod"),
                ("Team", "payments"),
                ("Tier", "1"),
                ("Owner", "ops"),
            ]),
        ),
        (
            "a held key keeps its position and takes the new value",
            pairs(&[("Env", "staging"), ("Team", "payments")]),
            pairs(&[("Env", "prod")]),
            pairs(&[("Env", "prod"), ("Team", "payments")]),
        ),
        (
            "every held key replaced at once",
            pairs(&[("Env", "staging"), ("Team", "payments")]),
            pairs(&[("Team", "search"), ("Env", "prod")]),
            pairs(&[("Env", "prod"), ("Team", "search")]),
        ),
        (
            "a replacement and an append in one request",
            pairs(&[("Env", "staging")]),
            pairs(&[("Env", "prod"), ("Tier", "2")]),
            pairs(&[("Env", "prod"), ("Tier", "2")]),
        ),
        (
            "one key named twice takes the last value",
            Vec::new(),
            pairs(&[("Env", "staging"), ("Env", "prod")]),
            pairs(&[("Env", "prod")]),
        ),
        (
            "a value replaced with the empty string",
            pairs(&[("Env", "prod")]),
            pairs(&[("Env", "")]),
            pairs(&[("Env", "")]),
        ),
        (
            "keys differing only in case are different keys",
            pairs(&[("Env", "prod")]),
            pairs(&[("env", "dev")]),
            pairs(&[("Env", "prod"), ("env", "dev")]),
        ),
        (
            "a request carrying no tags changes nothing",
            pairs(&[("Env", "prod")]),
            Vec::new(),
            pairs(&[("Env", "prod")]),
        ),
    ];

    for (case, held, request, expected) in cases {
        let router = build_router();
        let arn = write_resource(&router, "metric stream", "merged", &held).await;

        tag_resource(&router, &arn, &request).await;
        let reported = list_tags(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn untag_resource_removes_only_the_keys_it_names() {
    // Rows differ along how many keys the request removes, whether they are held at all,
    // whether one is named twice, and whether the request empties the tag set.
    let cases: Vec<RemovalCase> = vec![
        (
            "one of three keys",
            pairs(&[("Env", "prod"), ("Team", "payments"), ("Tier", "1")]),
            vec!["Team"],
            pairs(&[("Env", "prod"), ("Tier", "1")]),
        ),
        (
            "two of three keys",
            pairs(&[("Env", "prod"), ("Team", "payments"), ("Tier", "1")]),
            vec!["Env", "Tier"],
            pairs(&[("Team", "payments")]),
        ),
        (
            "every key held",
            pairs(&[("Env", "prod"), ("Team", "payments")]),
            vec!["Team", "Env"],
            Vec::new(),
        ),
        (
            "a key the resource does not carry",
            pairs(&[("Env", "prod")]),
            vec![UNATTACHED_KEY],
            pairs(&[("Env", "prod")]),
        ),
        (
            "one key named twice",
            pairs(&[("Env", "prod"), ("Team", "payments")]),
            vec!["Env", "Env"],
            pairs(&[("Team", "payments")]),
        ),
        (
            "no keys at all",
            pairs(&[("Env", "prod")]),
            Vec::new(),
            pairs(&[("Env", "prod")]),
        ),
        (
            "a key differing only in case from a held one",
            pairs(&[("Env", "prod")]),
            vec!["env"],
            pairs(&[("Env", "prod")]),
        ),
    ];

    for (case, held, keys, expected) in cases {
        let router = build_router();
        let arn = write_resource(&router, "metric stream", "untagged", &held).await;

        untag_resource(&router, &arn, &keys).await;
        let reported = list_tags(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );

        // Removal is idempotent: the same call again leaves the same set.
        untag_resource(&router, &arn, &keys).await;
        let reported = list_tags(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} on the second UntagResource, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_writer_leaves_the_tags_of_a_resource_it_updates_alone() {
    // One row per resource kind: the tags a request names when it UPDATES a resource that
    // already exists are ignored, so the set `TagResource` wrote survives the update.
    for kind in TAGGABLE_KINDS {
        let router = build_router();
        let created = pairs(&[("Env", "staging")]);
        let arn = write_resource(&router, kind, "updated-resource", &created).await;

        tag_resource(&router, &arn, &pairs(&[("Team", "payments")])).await;
        let expected = pairs(&[("Env", "staging"), ("Team", "payments")]);

        // The same writer again, naming other tags: an update, not a create.
        let updated = write_resource(&router, kind, "updated-resource", &pairs(&[("Env", "dev")])).await;
        assert_eq!(
            updated, arn,
            "case {kind:?}: expected the update to keep the ARN {arn}, got {updated}"
        );

        let reported = list_tags(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {kind:?}: expected {expected:?} after the update, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_carries_at_most_fifty_tags() {
    // "You can associate as many as 50 tags with a CloudWatch resource." (`TagResource`) —
    // tested at the bound, one past it, and with a request that replaces values without
    // taking the set past it.
    let router = build_router();
    let arn = write_resource(&router, "metric stream", "bounded", &[]).await;

    let fifty: Tags = (0..50)
        .map(|index| (format!("Key{index}"), format!("value-{index}")))
        .collect();
    tag_resource(&router, &arn, &fifty).await;
    let reported = list_tags(&router, &arn).await;
    assert_eq!(
        reported, fifty,
        "case \"fifty tags\": expected {fifty:?}, got {reported:?}"
    );

    // The fifty-first key would take the resource past the bound, so the whole request is
    // refused and the set is left as it was.
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({
            "ResourceARN": arn,
            "Tags": tag_list(&pairs(&[("Key50", "value-50")])),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"fifty-first tag\": expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "InvalidParameterValueException",
        "case \"fifty-first tag\": expected InvalidParameterValueException, got {code:?} \
         with body {body}"
    );
    let reported = list_tags(&router, &arn).await;
    assert_eq!(
        reported, fifty,
        "case \"fifty-first tag\": expected the tags to be unchanged at {fifty:?}, got \
         {reported:?}"
    );

    // A request whose OWN list is longer than the bound is refused on its own count, before
    // anything the resource holds is read: it could never be stored.
    let untagged = write_resource(&router, "metric stream", "over-bounded", &[]).await;
    let fifty_one: Tags = (0..51)
        .map(|index| (format!("Key{index}"), format!("value-{index}")))
        .collect();
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({"ResourceARN": untagged, "Tags": tag_list(&fifty_one)}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"fifty-one tags in one request\": expected HTTP 400, got {status} with body \
         {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "InvalidParameterValueException",
        "case \"fifty-one tags in one request\": expected InvalidParameterValueException, got \
         {code:?} with body {body}"
    );
    let reported = list_tags(&router, &untagged).await;
    assert!(
        reported.is_empty(),
        "case \"fifty-one tags in one request\": expected the resource to hold no tags, got \
         {reported:?}"
    );

    // A request that only replaces values held does not grow the set, so it is accepted at
    // the bound.
    let replacement = pairs(&[("Key0", "replaced")]);
    tag_resource(&router, &arn, &replacement).await;
    let reported = list_tags(&router, &arn).await;
    let mut expected = fifty.clone();
    expected[0].1 = "replaced".to_string();
    assert_eq!(
        reported, expected,
        "case \"replacement at the bound\": expected {expected:?}, got {reported:?}"
    );
}

#[tokio::test]
async fn every_tagging_operation_refuses_an_arn_that_names_no_cloudwatch_resource() {
    // Rows differ along WHY the ARN names no taggable CloudWatch resource: it is not an ARN,
    // it is another service's, it names a CloudWatch resource kind that carries no tags, it
    // carries no resource name, and it is truncated before the resource part.
    let cases: [(&str, &str); 6] = [
        ("not an ARN at all", "an-alarm"),
        (
            "another service's resource",
            "arn:aws:sns:us-east-1:000000000000:alerts",
        ),
        (
            "a CloudWatch resource kind that carries no tags",
            "arn:aws:cloudwatch:us-east-1:000000000000:anomaly-detector/MyApp",
        ),
        (
            "an alarm ARN with no alarm name",
            "arn:aws:cloudwatch:us-east-1:000000000000:alarm:",
        ),
        (
            "a metric stream ARN with no stream name",
            "arn:aws:cloudwatch:us-east-1:000000000000:metric-stream/",
        ),
        (
            "truncated before the resource part",
            "arn:aws:cloudwatch:us-east-1:000000000000",
        ),
    ];

    let router = build_router();
    for (case, arn) in cases {
        for operation in TAGGING_OPERATIONS {
            let request = tagging_request(operation, arn);
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "InvalidParameterValueException",
                "case {case:?} on {operation}: expected InvalidParameterValueException, got \
                 {code:?} with body {body}"
            );
        }
    }
}

#[tokio::test]
async fn every_tagging_operation_reports_a_resource_this_store_does_not_hold() {
    // Rows differ along the resource kind the well-formed ARN names, including a dashboard,
    // whose ARN carries no Region, and an alarm name that belongs to no alarm of any kind.
    let cases: [(&str, &str); 4] = [
        (
            "an alarm no writer created",
            "arn:aws:cloudwatch:us-east-1:000000000000:alarm:no-such-alarm",
        ),
        (
            "a metric stream no writer created",
            "arn:aws:cloudwatch:us-east-1:000000000000:metric-stream/no-such-stream",
        ),
        (
            "a Contributor Insights rule no writer created",
            "arn:aws:cloudwatch:us-east-1:000000000000:insight-rule/no-such-rule",
        ),
        (
            "a dashboard no writer created",
            "arn:aws:cloudwatch::000000000000:dashboard/no-such-dashboard",
        ),
    ];

    let router = build_router();
    // One resource of each kind exists, so a refusal is about the ARN's resource rather than
    // about an empty store.
    for (index, kind) in TAGGABLE_KINDS.iter().enumerate() {
        write_resource(&router, kind, &format!("a-resource-{index}"), &[]).await;
    }

    for (case, arn) in cases {
        for operation in TAGGING_OPERATIONS {
            let request = tagging_request(operation, arn);
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected HTTP 404, got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got {code:?} \
                 with body {body}"
            );
        }
    }
}

#[tokio::test]
async fn one_resource_s_tags_are_its_own() {
    // Two resources of different kinds under the SAME name, so a tag written on one is read
    // back only on that one: the tag set is keyed by the resource, not by the name.
    let router = build_router();
    let stream = write_resource(&router, "metric stream", "shared-name", &[]).await;
    let rule = write_resource(&router, "Contributor Insights rule", "shared-name", &[]).await;

    tag_resource(&router, &stream, &pairs(&[("Env", "prod")])).await;
    tag_resource(&router, &rule, &pairs(&[("Env", "dev"), ("Team", "search")])).await;

    let reported = list_tags(&router, &stream).await;
    let expected = pairs(&[("Env", "prod")]);
    assert_eq!(
        reported, expected,
        "case \"metric stream\": expected {expected:?}, got {reported:?}"
    );

    let reported = list_tags(&router, &rule).await;
    let expected = pairs(&[("Env", "dev"), ("Team", "search")]);
    assert_eq!(
        reported, expected,
        "case \"Contributor Insights rule\": expected {expected:?}, got {reported:?}"
    );

    // Deleting a resource takes its tags with it: the ARN then names nothing to tag.
    ok(
        &router,
        "DeleteMetricStream",
        &json!({"Name": "shared-name"}),
    )
    .await;
    let (status, body) = call(
        &router,
        "ListTagsForResource",
        &json!({"ResourceARN": stream}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"a deleted metric stream\": expected HTTP 404, got {status} with body {body}"
    );
    let reported = list_tags(&router, &rule).await;
    assert_eq!(
        reported, expected,
        "case \"Contributor Insights rule\": expected {expected:?} to survive the stream's \
         deletion, got {reported:?}"
    );
}
