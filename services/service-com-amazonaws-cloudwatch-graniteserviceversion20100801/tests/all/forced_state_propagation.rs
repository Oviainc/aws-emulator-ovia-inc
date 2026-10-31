// SPDX-License-Identifier: CC0-1.0

//! What a state a caller FORCES is visible to, and the regional identity of a metric stream.
//!
//! Two rules, each asserted over a table:
//!
//! 1. **A state `SetAlarmState` forces stands for its own span, and every alarm whose rule
//!    READS that alarm is evaluated over it** — "Temporarily sets the state of an alarm for
//!    testing purposes… Metric alarms returns to their actual state quickly, often within
//!    seconds." (`smithy_query(query="operation",operation="SetAlarmState")`) — so a composite
//!    alarm's `AlarmRule` sees the forced state, and the reclaim that follows it moves the
//!    composite back. The rows differ in the rule, in which children are forced and in the
//!    state they are forced into, so the composite's state is its rule over the children's and
//!    nothing else.
//! 2. **A metric stream is a resource of the Region it was created in**, so one NAME in two
//!    Regions is two streams with their own filters, their own state and their own ARN
//!    (`arn:aws:cloudwatch:Region:account-id:metric-stream/metric-stream-name`,
//!    `smithy_query(query="doc",shape="TagResourceInput",member="ResourceARN")`), and a
//!    transition stamps the `LastUpdateDate` of the one stream it moved. The rows differ in the
//!    Region pair and in which of the two streams the call names.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call_in_region, ok, wait_for_evaluation, DEFAULT_REGION};

/// The namespace the children of the composite rules watch.
const NAMESPACE: &str = "MyApp/Composite";

/// A child metric alarm of `name`, over a series of its own that nothing publishes into.
async fn given_child(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": NAMESPACE,
            "MetricName": "Load",
            "Dimensions": [{"Name": "Child", "Value": name}],
            "Statistic": "Maximum",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 10.0,
            "ComparisonOperator": "GreaterThanThreshold",
        }),
    )
    .await;
}

/// The state `DescribeAlarms` reports for the composite alarm `name`.
async fn composite_state(router: &MultiServiceRouter, name: &str) -> String {
    let body = ok(
        router,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": ["CompositeAlarm"]}),
    )
    .await;
    body["CompositeAlarms"][0]
        .get("StateValue")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_composite_rule_is_evaluated_over_the_state_a_caller_forced_on_its_children() {
    // (case, the rule, the state each of the two children is forced into, the state the rule
    //  then puts the composite in)
    let rows: Vec<(&str, &str, [&str; 2], &str)> = vec![
        // The recorded shape: an AND rule over two children, which needs both of them.
        (
            "an AND rule with both children in ALARM",
            "ALARM(\"kid-a\") AND ALARM(\"kid-b\")",
            ["ALARM", "ALARM"],
            "ALARM",
        ),
        (
            "an AND rule with one child in ALARM",
            "ALARM(\"kid-a\") AND ALARM(\"kid-b\")",
            ["ALARM", "OK"],
            "OK",
        ),
        (
            "an AND rule with neither child in ALARM",
            "ALARM(\"kid-a\") AND ALARM(\"kid-b\")",
            ["OK", "OK"],
            "OK",
        ),
        // An OR rule needs only one of them, which is the other half of the boolean table.
        (
            "an OR rule with one child in ALARM",
            "ALARM(\"kid-a\") OR ALARM(\"kid-b\")",
            ["OK", "ALARM"],
            "ALARM",
        ),
        // The other state functions read the same forced states.
        (
            "an OK rule over both children",
            "OK(\"kid-a\") AND OK(\"kid-b\")",
            ["OK", "OK"],
            "ALARM",
        ),
        (
            "an INSUFFICIENT_DATA rule over one child",
            "INSUFFICIENT_DATA(\"kid-a\")",
            ["INSUFFICIENT_DATA", "OK"],
            "ALARM",
        ),
        // A NOT over a forced state, and an unquoted reference: the rule's shape does not
        // change what it is evaluated over.
        (
            "a negated rule over an unquoted reference",
            "NOT ALARM(kid-a)",
            ["OK", "OK"],
            "ALARM",
        ),
    ];

    for (index, (case, rule, forced, expected)) in rows.into_iter().enumerate() {
        let router = build_router();
        let name = format!("composite-{index}");
        given_child(&router, "kid-a").await;
        given_child(&router, "kid-b").await;
        ok(
            &router,
            "PutCompositeAlarm",
            &json!({"AlarmName": name, "AlarmRule": rule}),
        )
        .await;
        for (child, state) in ["kid-a", "kid-b"].iter().zip(forced) {
            ok(
                &router,
                "SetAlarmState",
                &json!({
                    "AlarmName": child,
                    "StateValue": state,
                    "StateReason": "driven by the test",
                }),
            )
            .await;
        }

        // The composite's own evaluation is owed a span after the write that configured it and
        // after each child's transition, so the state it reports until then is the one its
        // creation left it in.
        assert_eq!(
            composite_state(&router, &name).await,
            "INSUFFICIENT_DATA",
            "case {case:?}: a composite alarm reports INSUFFICIENT_DATA until its first \
             evaluation is due"
        );
        wait_for_evaluation();
        let evaluated = composite_state(&router, &name).await;
        assert_eq!(
            evaluated, expected,
            "case {case:?} (rule {rule:?}, children {forced:?}): expected the rule to put the \
             composite in {expected}, got {evaluated:?}"
        );
    }
}

/// A `PutMetricStream` request for `name` in `region`, whose filters are the row's own.
///
/// The destination and the role are in the stream's own Region and account, which every
/// metric stream's configuration has to be ("FirehoseArn must be in the same region and AWS
/// partition as the Metric Stream").
fn stream_request(name: &str, region: &str, filters: Value) -> Value {
    let mut request = json!({
        "Name": name,
        "FirehoseArn": format!("arn:aws:firehose:{region}:000000000000:deliverystream/metrics"),
        "RoleArn": "arn:aws:iam::000000000000:role/MetricStreamRole",
        "OutputFormat": "json",
    });
    for (member, value) in filters.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    request
}

/// The stream `name` as the Region `region` reports it, requiring the read to succeed.
async fn stream_in(router: &MultiServiceRouter, region: &str, name: &str) -> Value {
    let (status, body) =
        call_in_region(router, region, "GetMetricStream", &json!({"Name": name})).await;
    assert_eq!(
        status, 200,
        "GetMetricStream in {region}: expected HTTP 200, got {status} with {body}"
    );
    body
}

#[tokio::test]
async fn one_metric_stream_name_in_two_regions_is_two_independent_streams() {
    // (case, the two Regions, the filters each stream is configured with)
    let rows: Vec<(&str, [&str; 2], [Value; 2])> = vec![
        (
            "the recorded pair, each Region filtering a namespace of its own",
            [DEFAULT_REGION, "us-west-2"],
            [
                json!({"IncludeFilters": [{"Namespace": "AWS/Lambda"}]}),
                json!({"ExcludeFilters": [{"Namespace": "AWS/EC2", "MetricNames": ["NetworkIn"]}]}),
            ],
        ),
        (
            "two Regions neither of which is the endpoint's default",
            ["eu-west-1", "ap-southeast-2"],
            [
                json!({"IncludeFilters": [{"Namespace": "AWS/S3"}]}),
                json!({"IncludeFilters": [{"Namespace": "AWS/SQS", "MetricNames": ["NumberOfMessagesSent"]}]}),
            ],
        ),
        (
            "one Region filtering nothing at all",
            [DEFAULT_REGION, "sa-east-1"],
            [json!({}), json!({"IncludeFilters": [{"Namespace": "AWS/RDS"}]})],
        ),
    ];

    for (index, (case, regions, filters)) in rows.into_iter().enumerate() {
        let router = build_router();
        let name = format!("shared-name-{index}");
        for (region, configured) in regions.iter().zip(&filters) {
            let (status, body) = call_in_region(
                &router,
                region,
                "PutMetricStream",
                &stream_request(&name, region, configured.clone()),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the stream to be created in {region}, got {status} \
                 {body}"
            );
        }

        // Each Region reports its OWN stream: its own ARN and its own filters.
        for (region, configured) in regions.iter().zip(&filters) {
            let stream = stream_in(&router, region, &name).await;
            assert_eq!(
                stream.get("Arn").and_then(Value::as_str),
                Some(
                    format!("arn:aws:cloudwatch:{region}:000000000000:metric-stream/{name}")
                        .as_str()
                ),
                "case {case:?}: the stream of {region} must carry that Region's ARN, got \
                 {stream}"
            );
            for member in ["IncludeFilters", "ExcludeFilters"] {
                assert_eq!(
                    stream.get(member),
                    configured.get(member),
                    "case {case:?}: the stream of {region} must carry its own {member}, got \
                     {stream}"
                );
            }
        }

        // A transition in one Region moves THAT stream only, and stamps its own
        // `LastUpdateDate`: "The date that the configuration of this metric stream was most
        // recently updated." (`GetMetricStreamOutput$LastUpdateDate`)
        let before: Vec<Value> = {
            let mut held = Vec::new();
            for region in &regions {
                held.push(stream_in(&router, region, &name).await);
            }
            held
        };
        // Two calls inside one millisecond stamp the same instant, so the stop below waits for
        // the clock to leave the millisecond the creation was stamped in.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let (status, body) = call_in_region(
            &router,
            regions[1],
            "StopMetricStreams",
            &json!({"Names": [name]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stop to be accepted, got {status} {body}"
        );

        let untouched = stream_in(&router, regions[0], &name).await;
        assert_eq!(
            untouched, before[0],
            "case {case:?}: stopping the stream of {} must leave the stream of {} exactly as \
             it was, got {untouched}",
            regions[1], regions[0]
        );
        let stopped = stream_in(&router, regions[1], &name).await;
        assert_eq!(
            stopped.get("State").and_then(Value::as_str),
            Some("stopped"),
            "case {case:?}: the stream the stop named must be stopped, got {stopped}"
        );
        let stamped = stopped.get("LastUpdateDate").and_then(Value::as_f64);
        assert!(
            stamped > stopped.get("CreationDate").and_then(Value::as_f64),
            "case {case:?}: the transition must advance LastUpdateDate past the creation date, \
             got {stopped}"
        );
    }
}
