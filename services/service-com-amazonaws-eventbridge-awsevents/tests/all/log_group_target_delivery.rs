// SPDX-License-Identifier: CC0-1.0

//! A rule target that is a CloudWatch Logs LOG GROUP is delivered to by WRITING the
//! matched event into that group, end to end over the wire (`awsJson1_1`, `POST /`
//! with `X-Amz-Target: <prefix>.<Operation>`) through the real `MultiServiceRouter`,
//! with CloudWatch Logs and CloudWatch mounted on the same engine so a delivery is
//! asserted where a client observes one: the log events `FilterLogEvents` reports,
//! and the rule's own invocation metrics.
//!
//! The RULE asserted here: for every enabled rule whose `EventPattern` matches a
//! published event, a target whose `Arn` names a log group receives that event as
//! exactly ONE log event written into the group — its `message` the text the target
//! receives (the whole event envelope, or the `timestamp`/`message` a reshaped input
//! names) and its `timestamp` that text's own instant in the epoch milliseconds
//! `InputLogEvent$timestamp` carries. The write is `logs:PutLogEvents` made as the
//! service principal `events.amazonaws.com`, which holds no identity-based policies,
//! so the resource policies in force over the GROUP are the only documents over the
//! delivery and what they state decides it: a group no document grants the write on
//! receives nothing and the attempt is counted on the rule's `FailedInvocations`,
//! where a delivery that was written is counted on `Invocations`.
//!
//! Each table spans the class rather than the one recorded case: the rows differ
//! along the FORM of the target's ARN (the group on its own and the group with
//! everything under it), the shape of the group's name, the detail the event carries,
//! whether the event matches the rule at all, what the group's documents state (which
//! scope they are attached at, which principal, action and resource they are about,
//! which `Effect` and which condition), and what the target asked to receive. Every
//! row is a FRESH emulator, so one row's document never decides another row's write.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, DEFAULT_EVENT_BUS_NAME, FAILED_INVOCATIONS_METRIC, INVOCATIONS_METRIC,
    SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    emulator, events, given_rule, metric_sum_over, rpc, rule_name_dimensions, Emulator,
    DETAIL_TYPE, REGION, SOURCE,
};

/// The one rule every test here publishes through, named so a row's condition can
/// scope a grant to it.
const RULE: &str = "log-group-rule";

/// The id the rule's single target carries.
const TARGET_ID: &str = "log-group";

/// The actions a delivery to a log-group target needs on the group: the stream it
/// writes into has to be opened, and the event has to be put
/// (`logs:CreateLogStream` / `logs:PutLogEvents`, the two calls the write is made
/// of).
const CREATE_LOG_STREAM: &str = "logs:CreateLogStream";
const PUT_LOG_EVENTS: &str = "logs:PutLogEvents";

/// The `version` every published event's envelope carries.
const EVENT_VERSION: &str = "0";

/// The account every resource below belongs to.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// One CloudWatch Logs call (`Logs_20140328`, awsJson1_1).
async fn logs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "logs", "Logs_20140328", "1.1", operation, body).await
}

/// A log group called `name`, answered with the ARN `DescribeLogGroups` reports for
/// it — the form carrying everything under the group, which is the ARN a client
/// reads off that listing.
async fn given_log_group(emulator: &Emulator, case: &str, name: &str) -> String {
    let (status, body) = logs(emulator, "CreateLogGroup", &json!({ "logGroupName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLogGroup answered {status} {body}"
    );
    let (status, body) = logs(
        emulator,
        "DescribeLogGroups",
        &json!({ "logGroupNamePrefix": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeLogGroups answered {status} {body}"
    );
    body["logGroups"][0]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeLogGroups reported no ARN: {body}"))
        .to_string()
}

/// Write `document` as the resource policy `name` — attached to `resource_arn` when
/// the row names one, and at the account's own scope when it does not.
async fn given_resource_policy(
    emulator: &Emulator,
    case: &str,
    name: &str,
    document: &str,
    resource_arn: Option<&str>,
) {
    let mut request = json!({
        "policyName": name,
        "policyDocument": document,
    });
    if let Some(resource_arn) = resource_arn {
        request["resourceArn"] = json!(resource_arn);
    }
    let (status, body) = logs(emulator, "PutResourcePolicy", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutResourcePolicy answered {status} {body}"
    );
}

/// Every log event the group `group` holds, as `FilterLogEvents` reports them.
async fn written(emulator: &Emulator, case: &str, group: &str) -> Vec<Value> {
    let (status, body) = logs(
        emulator,
        "FilterLogEvents",
        &json!({ "logGroupName": group }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: FilterLogEvents answered {status} {body}"
    );
    body["events"].as_array().cloned().unwrap_or_default()
}

/// A statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// An `Allow` of the two calls the write is made of to the EventBridge service
/// principal on `resource`, scoped by `condition` when the row states one.
fn grant(resource: &str, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": { "Service": SERVICE_PRINCIPAL },
        "Action": [CREATE_LOG_STREAM, PUT_LOG_EVENTS],
        "Resource": resource,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// The document a log group carries for the deliveries of the rule below: the
/// unconditional grant on everything under the group, which is the form a client
/// writes.
fn granting_events(group_arn: &str) -> String {
    document(json!([grant(&everything_under(group_arn), None)]))
}

/// A `Deny` of the write to the EventBridge service principal on everything under
/// the group.
fn deny(group_arn: &str) -> Value {
    json!({
        "Effect": "Deny",
        "Principal": { "Service": SERVICE_PRINCIPAL },
        "Action": PUT_LOG_EVENTS,
        "Resource": everything_under(group_arn),
    })
}

/// `group_arn` naming the group together with everything under it, which is the
/// resource a grant for the write is written over.
fn everything_under(group_arn: &str) -> String {
    let bare = group_arn.strip_suffix(":*").unwrap_or(group_arn);
    format!("{bare}:*")
}

/// `group_arn` naming the group alone, which is the ARN a rule's target carries.
fn bare(group_arn: &str) -> String {
    group_arn.strip_suffix(":*").unwrap_or(group_arn).to_string()
}

/// The ARN of a rule of this account and Region that is NOT the one publishing
/// below.
fn another_rule_arn() -> String {
    rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, "somebody-elses-rule")
}

/// Publish ONE entry carrying `source`, `detail_type` and `detail`, timestamped at
/// `at` (epoch seconds), and answer its `EventId`.
async fn publish_at(
    emulator: &Emulator,
    case: &str,
    source: &str,
    detail_type: &str,
    detail: &str,
    at: i64,
) -> String {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "Source": source,
                "DetailType": detail_type,
                "Detail": detail,
                "Time": at,
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutEvents reported a failed entry: {body}"
    );
    body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: PutEvents minted no EventId: {body}"))
        .to_string()
}

/// An instant inside the window CloudWatch Logs holds events for, read off the same
/// clock the emulator stamps its own records with, so the assertions below never
/// depend on a fixed date.
fn moments_ago() -> i64 {
    let now = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    i64::try_from(now).unwrap_or_default() - 30
}

/// One row of the envelope table: the group's name, which form of its ARN the
/// target names, the event published, and whether that event matches the rule.
struct Delivered {
    case: &'static str,
    group: &'static str,
    /// Whether the target names the group WITH everything under it, as a client
    /// reads the ARN off `DescribeLogGroups`, rather than the group alone.
    target_names_everything_under: bool,
    source: &'static str,
    detail_type: &'static str,
    detail: &'static str,
    delivered: bool,
}

/// A matched event is written into the target log group as ONE log event carrying
/// the whole envelope under the event's own instant — and an event the rule does not
/// match is written nowhere.
#[tokio::test]
async fn a_matched_event_is_written_into_its_log_group_target() {
    let rows = [
        Delivered {
            case: "a group under the events prefix, the target naming the group alone",
            group: "/aws/events/audit",
            target_names_everything_under: false,
            source: SOURCE,
            detail_type: DETAIL_TYPE,
            detail: r#"{"orderId":"A-1","amount":7}"#,
            delivered: true,
        },
        Delivered {
            case: "the target naming the group with everything under it",
            group: "/aws/events/audit",
            target_names_everything_under: true,
            source: SOURCE,
            detail_type: DETAIL_TYPE,
            detail: r#"{"orderId":"A-1","amount":7}"#,
            delivered: true,
        },
        Delivered {
            case: "a group whose name carries no path at all",
            group: "orders-audit",
            target_names_everything_under: false,
            source: SOURCE,
            detail_type: DETAIL_TYPE,
            detail: r#"{"orderId":"A-2"}"#,
            delivered: true,
        },
        Delivered {
            case: "an event whose detail nests documents and lists",
            group: "/aws/events/nested",
            target_names_everything_under: false,
            source: SOURCE,
            detail_type: DETAIL_TYPE,
            detail: r#"{"order":{"id":"A-3","lines":[{"sku":"x","qty":2}]},"tags":["eu","web"]}"#,
            delivered: true,
        },
        Delivered {
            case: "an event whose detail is an empty document",
            group: "/aws/events/empty",
            target_names_everything_under: false,
            source: SOURCE,
            detail_type: DETAIL_TYPE,
            detail: "{}",
            delivered: true,
        },
        Delivered {
            case: "a near miss: a detail type differing only in letter case",
            group: "/aws/events/near-miss",
            target_names_everything_under: false,
            source: SOURCE,
            detail_type: "ORDER CREATED",
            detail: r#"{"orderId":"A-4"}"#,
            delivered: false,
        },
        Delivered {
            case: "a near miss: another source entirely",
            group: "/aws/events/other-source",
            target_names_everything_under: false,
            source: "com.demo.invoices",
            detail_type: DETAIL_TYPE,
            detail: r#"{"orderId":"A-5"}"#,
            delivered: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let emulator = emulator();
        let group_arn = given_log_group(&emulator, case, row.group).await;
        given_resource_policy(
            &emulator,
            case,
            "group-delivery",
            &granting_events(&group_arn),
            Some(&group_arn),
        )
        .await;
        let target_arn = if row.target_names_everything_under {
            everything_under(&group_arn)
        } else {
            bare(&group_arn)
        };
        given_rule(
            &emulator,
            RULE,
            json!([{ "Id": TARGET_ID, "Arn": target_arn }]),
        )
        .await;

        let at = moments_ago();
        let event_id =
            publish_at(&emulator, case, row.source, row.detail_type, row.detail, at).await;
        let events = written(&emulator, case, row.group).await;
        let expected = usize::from(row.delivered);
        assert_eq!(
            events.len(),
            expected,
            "case {case:?}: expected {expected} log event(s) in {}, got {events:?}",
            row.group
        );
        if !row.delivered {
            continue;
        }
        let event = &events[0];
        assert_eq!(
            event["timestamp"],
            json!(at * 1_000),
            "case {case:?}: expected the log event's timestamp to be the event's own instant \
             in epoch milliseconds ({}), got {event:?}",
            at * 1_000
        );
        let message = event["message"].as_str().unwrap_or_default();
        let envelope: Value = serde_json::from_str(message).unwrap_or_else(|error| {
            panic!("case {case:?}: the log event's message is not the event envelope ({error}): {message:?}")
        });
        let detail: Value = serde_json::from_str(row.detail)
            .unwrap_or_else(|error| panic!("case {case:?}: the row's detail is not JSON: {error}"));
        let expected = json!({
            "version": EVENT_VERSION,
            "id": event_id,
            "detail-type": row.detail_type,
            "source": row.source,
            "account": account(),
            "time": envelope["time"],
            "region": REGION,
            "resources": [],
            "detail": detail,
        });
        assert_eq!(
            envelope, expected,
            "case {case:?}: expected the envelope {expected}, got {envelope}"
        );
    }
}

/// How a row writes a document, from the group's own ARN.
type PolicyFor = Box<dyn Fn(&str) -> String>;

/// One row of the policy table: the documents in force over the group — the one
/// attached to the GROUP and the ACCOUNT's own, either or both — and whether the
/// write happens.
struct Row {
    case: &'static str,
    /// The document attached to the group itself; `None` when it carries none.
    attached: Option<PolicyFor>,
    /// The account's own document; `None` when the account carries none.
    account: Option<PolicyFor>,
    written: bool,
}

/// The documents in force over the log group decide each write, over a table of
/// documents — and a withheld write is a delivery that FAILED, so the rule counts it
/// on `FailedInvocations` and not on `Invocations`.
#[tokio::test]
async fn only_what_the_log_groups_resource_policies_allow_is_written() {
    let rows: Vec<Row> = vec![
        Row {
            case: "no resource policy at all",
            attached: None,
            account: None,
            written: false,
        },
        Row {
            case: "a grant attached to the group, over everything under it",
            attached: Some(Box::new(|arn: &str| granting_events(arn))),
            account: None,
            written: true,
        },
        Row {
            case: "a grant attached to the group, over its log streams",
            attached: Some(Box::new(|arn: &str| {
                let streams = format!("{}:log-stream:*", bare(arn));
                document(json!([grant(&streams, None)]))
            })),
            account: None,
            written: true,
        },
        Row {
            case: "an account-wide grant over every log group of the account",
            attached: None,
            account: Some(Box::new(|_arn: &str| {
                let every = format!("arn:aws:logs:{REGION}:{}:log-group:*", account());
                document(json!([grant(&every, None)]))
            })),
            written: true,
        },
        Row {
            case: "a grant naming another service principal",
            attached: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": { "Service": "delivery.logs.amazonaws.com" },
                    "Action": [CREATE_LOG_STREAM, PUT_LOG_EVENTS],
                    "Resource": everything_under(arn),
                }]))
            })),
            account: None,
            written: false,
        },
        Row {
            case: "a grant of opening a stream but not of writing one",
            attached: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": CREATE_LOG_STREAM,
                    "Resource": everything_under(arn),
                }]))
            })),
            account: None,
            written: false,
        },
        Row {
            case: "a grant of writing but not of opening a stream",
            attached: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": PUT_LOG_EVENTS,
                    "Resource": everything_under(arn),
                }]))
            })),
            account: None,
            // The write is admitted and the open is not, so no stream is opened
            // and the put lands on a group holding none: nothing is written, and
            // the delivery fails on CloudWatch Logs' own answer.
            written: false,
        },
        Row {
            case: "a grant whose Resource names another group",
            attached: Some(Box::new(|_arn: &str| {
                let other = format!("arn:aws:logs:{REGION}:{}:log-group:/other:*", account());
                document(json!([grant(&other, None)]))
            })),
            account: None,
            written: false,
        },
        Row {
            case: "a grant scoped by ArnEquals to the publishing rule",
            attached: Some(Box::new(|arn: &str| {
                let mine = rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE);
                document(json!([grant(
                    &everything_under(arn),
                    Some(json!({ "ArnEquals": { "aws:SourceArn": mine } }))
                )]))
            })),
            account: None,
            written: true,
        },
        Row {
            case: "a grant scoped by ArnEquals to another rule",
            attached: Some(Box::new(|arn: &str| {
                document(json!([grant(
                    &everything_under(arn),
                    Some(json!({ "ArnEquals": { "aws:SourceArn": another_rule_arn() } }))
                )]))
            })),
            account: None,
            written: false,
        },
        Row {
            case: "a grant scoped by StringEquals to this account",
            attached: Some(Box::new(|arn: &str| {
                document(json!([grant(
                    &everything_under(arn),
                    Some(json!({ "StringEquals": { "aws:SourceAccount": account() } }))
                )]))
            })),
            account: None,
            written: true,
        },
        Row {
            case: "an explicit deny written after the grant",
            attached: Some(Box::new(|arn: &str| {
                document(json!([grant(&everything_under(arn), None), deny(arn)]))
            })),
            account: None,
            written: false,
        },
        Row {
            case: "a deny attached to the group beside an account-wide grant",
            attached: Some(Box::new(|arn: &str| document(json!([deny(arn)])))),
            account: Some(Box::new(|_arn: &str| {
                let every = format!("arn:aws:logs:{REGION}:{}:log-group:*", account());
                document(json!([grant(&every, None)]))
            })),
            written: false,
        },
        Row {
            case: "an account-wide deny beside a grant attached to the group",
            attached: Some(Box::new(|arn: &str| granting_events(arn))),
            account: Some(Box::new(|_arn: &str| {
                let every = format!("arn:aws:logs:{REGION}:{}:log-group:*", account());
                document(json!([{
                    "Effect": "Deny",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": PUT_LOG_EVENTS,
                    "Resource": every,
                }]))
            })),
            written: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let group = format!("/aws/events/policy-case-{index}");
        let group_arn = given_log_group(&emulator, case, &group).await;
        if let Some(policy) = &row.attached {
            given_resource_policy(
                &emulator,
                case,
                "group-delivery",
                &policy(&group_arn),
                Some(&group_arn),
            )
            .await;
        }
        if let Some(policy) = &row.account {
            given_resource_policy(
                &emulator,
                case,
                "account-delivery",
                &policy(&group_arn),
                None,
            )
            .await;
        }
        given_rule(
            &emulator,
            RULE,
            json!([{ "Id": TARGET_ID, "Arn": bare(&group_arn) }]),
        )
        .await;

        let at = moments_ago();
        publish_at(&emulator, case, SOURCE, DETAIL_TYPE, r#"{"orderId":"A-1"}"#, at).await;
        let events = written(&emulator, case, &group).await;
        let expected = usize::from(row.written);
        assert_eq!(
            events.len(),
            expected,
            "case {case:?}: expected {expected} log event(s) in {group}, got {events:?}"
        );

        for (metric, expected) in [
            (INVOCATIONS_METRIC, f64::from(u8::from(row.written))),
            (FAILED_INVOCATIONS_METRIC, f64::from(u8::from(!row.written))),
        ] {
            let total = metric_sum_over(&emulator, metric, &rule_name_dimensions(RULE)).await;
            assert_eq!(
                total, expected,
                "case {case:?}: expected {metric} to total {expected}, got {total}"
            );
        }
    }
}

/// One row of the reshaped-input table: what the target asks to receive, the log
/// event it is written as, and whether anything is written at all.
struct Reshaped {
    case: &'static str,
    /// The members the target carries beside its `Id` and `Arn`.
    asked: fn(i64) -> Value,
    /// The message the one written log event carries, and whether its timestamp is
    /// the event's own instant; `None` when nothing is written.
    message: Option<&'static str>,
}

/// A target that RESHAPED its input is handed the log record that input names: its
/// `timestamp` and `message` are what the log event carries, and an input naming
/// neither is no log record, so nothing is written and the attempt counts as a
/// failed invocation.
#[tokio::test]
async fn a_reshaped_input_is_written_as_the_log_record_it_names() {
    let rows = [
        Reshaped {
            case: "an input transformer rendering the event's instant and a message",
            asked: |_at| {
                json!({
                    "InputTransformer": {
                        "InputPathsMap": { "orderId": "$.detail.orderId" },
                        "InputTemplate": "{\"timestamp\":\"<aws.events.event.ingestion-time>\",\
                                           \"message\":\"order <orderId> placed\"}",
                    },
                })
            },
            message: Some("order A-1 placed"),
        },
        Reshaped {
            case: "a constant Input naming the instant in epoch milliseconds",
            asked: |at| {
                json!({
                    "Input": json!({
                        "timestamp": at * 1_000,
                        "message": "a constant record",
                    })
                    .to_string(),
                })
            },
            message: Some("a constant record"),
        },
        Reshaped {
            case: "an input transformer rendering neither member",
            asked: |_at| {
                json!({
                    "InputTransformer": {
                        "InputPathsMap": { "orderId": "$.detail.orderId" },
                        "InputTemplate": "{\"order\":\"<orderId>\"}",
                    },
                })
            },
            message: None,
        },
        Reshaped {
            case: "an input path selecting a part of the event that is no log record",
            asked: |_at| json!({ "InputPath": "$.detail" }),
            message: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let group = format!("/aws/events/reshaped-case-{index}");
        let group_arn = given_log_group(&emulator, case, &group).await;
        given_resource_policy(
            &emulator,
            case,
            "group-delivery",
            &granting_events(&group_arn),
            Some(&group_arn),
        )
        .await;
        let at = moments_ago();
        let mut target = json!({ "Id": TARGET_ID, "Arn": bare(&group_arn) });
        for (member, value) in (row.asked)(at)
            .as_object()
            .expect("the row's target members are an object")
        {
            target[member] = value.clone();
        }
        given_rule(&emulator, RULE, json!([target])).await;

        publish_at(&emulator, case, SOURCE, DETAIL_TYPE, r#"{"orderId":"A-1"}"#, at).await;
        let events = written(&emulator, case, &group).await;
        let expected = usize::from(row.message.is_some());
        assert_eq!(
            events.len(),
            expected,
            "case {case:?}: expected {expected} log event(s) in {group}, got {events:?}"
        );
        let Some(message) = row.message else {
            let failed =
                metric_sum_over(&emulator, FAILED_INVOCATIONS_METRIC, &rule_name_dimensions(RULE))
                    .await;
            assert_eq!(
                failed, 1.0,
                "case {case:?}: expected the withheld write to count one \
                 {FAILED_INVOCATIONS_METRIC}, got {failed}"
            );
            continue;
        };
        let event = &events[0];
        assert_eq!(
            event["message"],
            json!(message),
            "case {case:?}: expected the message {message:?}, got {event:?}"
        );
        assert_eq!(
            event["timestamp"],
            json!(at * 1_000),
            "case {case:?}: expected the timestamp the input named ({}), got {event:?}",
            at * 1_000
        );
    }
}
