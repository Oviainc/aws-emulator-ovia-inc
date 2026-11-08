// SPDX-License-Identifier: CC0-1.0

//! The LOG RECORD a delivery to a CloudWatch Logs log-group target writes, and
//! the stream it is written into.
//!
//! A rule target whose `Arn` names a log group is delivered to by WRITING one
//! log event into that group, so the handover has to turn the text the target
//! receives into the `{timestamp, message}` pair `InputLogEvent` carries
//! (`timestamp`: "the time the event occurred, expressed as the number of
//! milliseconds after Jan 1, 1970 00:00:00 UTC"; `message`: "the raw event
//! message"). That mapping is a pure reading of the delivered text, so it lives
//! here rather than beside the call that makes the write
//! (`crate::ops::put_events`), and the delivery states only which group and
//! which stream.
//!
//! The mapping follows WHAT the target asked to receive: a target taking the
//! event as published is handed the whole envelope, so the record's message is
//! that envelope and its instant is the envelope's own `time` — and a target
//! that reshaped its input (`Target$Input`, `Target$InputPath`,
//! `Target$InputTransformer`) is handed what it asked for, so the record is read
//! OUT of that text: a log-group target's reshaped input is the log record
//! itself, naming the `timestamp` and the `message` to write, and an input that
//! names neither is no log record at all, so nothing is written for it.

use super::*;

/// The ARN namespace of Amazon CloudWatch Logs, whose log groups a rule
/// delivers to (`smithy_query(query="service_info")` of that service:
/// `arnNamespace` `logs`).
pub const LOGS_ARN_NAMESPACE: &str = "logs";

/// The resource type a log group's ARN names
/// (`arn:<partition>:logs:<region>:<account>:log-group:<name>`).
pub const LOG_GROUP_RESOURCE_TYPE: &str = "log-group";

/// The resource type a log STREAM's ARN names, under its group's
/// (`…:log-group:<name>:log-stream:<stream>`) — the resource `logs:PutLogEvents`
/// is taken on, and so the one a grant for the write is decided against.
pub const LOG_STREAM_RESOURCE_TYPE: &str = "log-stream";

/// The IAM action the write a log-group target's delivery makes performs.
pub const PUT_LOG_EVENTS_ACTION: &str = "logs:PutLogEvents";

/// The IAM action the OPEN of the stream that write goes into performs: a log
/// event can only be put into a stream that exists, so the delivery makes this
/// call before the write and needs it granted on its own.
pub const CREATE_LOG_STREAM_ACTION: &str = "logs:CreateLogStream";

/// Whether the documents in force over a target admit the invocation `attempt`
/// describes — the decision over a SET of documents rather than over one
/// ([`target_policy_admits`]).
///
/// A write into a log group is decided by more than one document: the resource
/// policy attached to that group and the account's own resource policies are
/// each in force over it, and a client grants a delivery through either. IAM's
/// order spans the whole set: an explicit `Deny` about the invocation in ANY of
/// them refuses it whatever another grants, and otherwise the invocation needs
/// an `Allow` about it in one of them — a set that states nothing has granted
/// nothing, which is IAM's implicit deny.
pub fn documents_admit(
    documents: &[String],
    attempt: &DeliveryAttempt<'_>,
    rule_arn: &str,
) -> bool {
    let mut admitted = false;
    for document in documents {
        match target_policy_decision(document, attempt, rule_arn) {
            ::service_common::iam_authorization::Decision::Denied => return false,
            ::service_common::iam_authorization::Decision::Allowed => admitted = true,
            ::service_common::iam_authorization::Decision::NotStated => {}
        }
    }
    admitted
}

/// The suffix the ARN of a log group carries when it names the group TOGETHER
/// with everything under it, which is the form `DescribeLogGroups` reports a
/// group's `arn` in.
const EVERYTHING_UNDER_THE_GROUP: &str = ":*";

/// The name of the log group `arn` names, or `None` when it names anything else.
///
/// Both forms of a log group's ARN name the same group — the group on its own
/// and the group with everything under it ([`EVERYTHING_UNDER_THE_GROUP`], which
/// is how a client reads an ARN off `DescribeLogGroups`) — so a target carrying
/// either is delivered to the one group. The ARN is read through the ONE shared
/// parse of a resource ARN (`service_common::arn`), and what is left has to be a
/// log group NAME: the model bounds one to `[\.\-_/#A-Za-z0-9]+`
/// (`PutLogEventsRequest$logGroupName`), so text carrying a further `:` or a `*`
/// addresses something other than a group and no group is written to for it.
pub fn log_group_name(arn: &str) -> Option<&str> {
    let resource = ::service_common::arn::resource_id_of_type(arn, LOG_GROUP_RESOURCE_TYPE)?;
    let name = resource
        .strip_suffix(EVERYTHING_UNDER_THE_GROUP)
        .unwrap_or(resource);
    (!name.is_empty() && !name.contains([':', '*'])).then_some(name)
}

/// The ARN of the stream `stream` of the log group `arn` names: the group's own
/// ARN with that stream under it.
///
/// This is the resource the write is taken on, so it is what a grant for
/// `logs:PutLogEvents` is decided against — and it is built from the TARGET's
/// own ARN, so the partition, Region and account a delivery is authorized under
/// are the group's rather than this emulator's.
pub fn log_stream_arn(arn: &str, stream: &str) -> String {
    let group = arn
        .strip_suffix(EVERYTHING_UNDER_THE_GROUP)
        .unwrap_or(arn);
    format!("{group}:{LOG_STREAM_RESOURCE_TYPE}:{stream}")
}

/// ONE log event, as CloudWatch Logs takes it (`InputLogEvent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    /// `InputLogEvent$timestamp`: the instant the event occurred, in epoch
    /// milliseconds.
    pub timestamp: i64,
    /// `InputLogEvent$message`: the raw event message.
    pub message: String,
}

/// The member a reshaped input names the log event's instant in.
const TIMESTAMP_MEMBER: &str = "timestamp";

/// The member a reshaped input names the log event's message in.
const MESSAGE_MEMBER: &str = "message";

/// The member of the event envelope that carries the event's own instant, which
/// is the instant the log event of an UNRESHAPED delivery is written under.
const ENVELOPE_TIME_MEMBER: &str = "time";

/// How many milliseconds there are in a second, the unit the envelope's `time`
/// is read in and the one `InputLogEvent$timestamp` is written in.
const MILLIS_PER_SECOND: i64 = 1_000;

/// The log event the text `delivered` is written as — `Err(reason)` when that
/// text is no log event, which is a delivery that writes nothing.
///
/// `reshaped` is whether the target asked for something other than the event as
/// published, which is what decides how the text is read: the whole envelope is
/// the message of a record carrying the envelope's own `time`, while a reshaped
/// input IS the record and names both of its members. A reshaped input that
/// names neither, or that is not one JSON object at all, is no log record, and
/// neither is one whose message is empty — `EventMessage` is bounded
/// `length 1..`, so there would be nothing to write.
pub fn log_record(delivered: &str, reshaped: bool) -> Result<LogRecord, String> {
    let document = parse_delivered_event(delivered);
    if !reshaped {
        let timestamp = document
            .as_ref()
            .and_then(|event| event.get(ENVELOPE_TIME_MEMBER))
            .and_then(::serde_json::Value::as_str)
            .and_then(epoch_millis_of_instant)
            .ok_or_else(|| NO_EVENT_TIME.to_string())?;
        return Ok(LogRecord {
            timestamp,
            message: delivered.to_string(),
        });
    }
    let members = document
        .as_ref()
        .and_then(::serde_json::Value::as_object)
        .ok_or_else(|| NOT_A_LOG_RECORD.to_string())?;
    let timestamp = members
        .get(TIMESTAMP_MEMBER)
        .and_then(rendered_timestamp)
        .ok_or_else(|| NOT_A_LOG_RECORD.to_string())?;
    let message = members
        .get(MESSAGE_MEMBER)
        .map(rendered_message)
        .filter(|message| !message.is_empty())
        .ok_or_else(|| NOT_A_LOG_RECORD.to_string())?;
    Ok(LogRecord { timestamp, message })
}

/// Why nothing is written for a reshaped input that is no log record: it states
/// the two members such an input has to name, so the detail a dead-letter
/// message carries says what was missing rather than merely that something was.
pub const NOT_A_LOG_RECORD: &str = "The input a CloudWatch Logs log group target receives must be \
                                    a JSON object naming the log event's timestamp and message.";

/// Why nothing is written for an envelope carrying no readable instant: the log
/// event's own timestamp is that instant, so there is none to write it under.
pub const NO_EVENT_TIME: &str =
    "The event carries no time to write the log event's timestamp from.";

/// The instant a reshaped input names, in the epoch milliseconds a log event
/// carries: a number is those milliseconds already, and text is the instant it
/// spells.
///
/// A rendered template is TEXT wherever it interpolated a placeholder, so the
/// instant an input transformer renders from the event (`time`, or the delivery's
/// own ingestion time) arrives as the string that envelope member carries; a
/// template that computed a number arrives as one.
fn rendered_timestamp(value: &::serde_json::Value) -> Option<i64> {
    match value {
        ::serde_json::Value::Number(number) => number.as_i64().filter(|millis| *millis >= 0),
        ::serde_json::Value::String(text) => epoch_millis_of_instant(text),
        _ => None,
    }
}

/// The message a reshaped input names: its text when the member carries text,
/// and otherwise that value's own JSON — a log event's message is a string, so a
/// template that rendered a document into the member writes that document.
fn rendered_message(value: &::serde_json::Value) -> String {
    match value {
        ::serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `text` as the epoch milliseconds a log event's timestamp carries, when it
/// spells an instant.
///
/// The form is the one an event envelope's `time` is written in
/// ([`to_event_time`]), read back through the same timestamp reader that wrote
/// it (`aws_smithy_types::DateTime`), so an instant this service rendered is the
/// instant it is read as. A negative instant is not one a log event can carry
/// (`InputLogEvent$timestamp` is bounded `range 0..`), so it names none.
fn epoch_millis_of_instant(text: &str) -> Option<i64> {
    let at = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    let millis = at
        .secs()
        .checked_mul(MILLIS_PER_SECOND)?
        .checked_add(i64::from(at.subsec_nanos() / 1_000_000))?;
    (millis >= 0).then_some(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both forms of a log group's ARN name the same group, and an ARN that
    /// addresses anything else names none.
    #[test]
    fn a_log_group_arn_names_the_group_in_either_of_its_two_forms() {
        // (case, the ARN, the group it names)
        let cases: &[(&str, &str, Option<&str>)] = &[
            (
                "the group on its own",
                "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/events/audit",
                Some("/aws/events/audit"),
            ),
            (
                "the group with everything under it",
                "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/events/audit:*",
                Some("/aws/events/audit"),
            ),
            (
                "a stream under the group",
                "arn:aws:logs:eu-west-2:123456789012:log-group:/g:log-stream:s",
                None,
            ),
            (
                "another resource of the same service",
                "arn:aws:logs:eu-west-2:123456789012:destination:my-destination",
                None,
            ),
            (
                "a group with no name at all",
                "arn:aws:logs:eu-west-2:123456789012:log-group:",
                None,
            ),
            ("text that is no ARN", "/aws/events/audit", None),
        ];
        for (case, arn, expected) in cases {
            let actual = log_group_name(arn);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The record is read from the delivered text: the whole envelope writes
    /// itself under the envelope's own instant, a reshaped input writes the
    /// members it names, and an input naming no log event writes nothing.
    #[test]
    fn the_log_record_is_read_from_what_the_target_receives() {
        let envelope = ::serde_json::json!({
            "version": "0",
            "id": "a-b-c",
            "detail-type": "Order Created",
            "source": "com.demo.orders",
            "account": "123456789012",
            "time": "2026-11-07T23:34:09Z",
            "region": "eu-west-2",
            "resources": [],
            "detail": {"orderId": "A-1"},
        })
        .to_string();
        // The envelope's own instant, in the milliseconds a log event carries.
        let envelope_millis = 1_794_094_449_000;

        let unreshaped = log_record(&envelope, false);
        assert_eq!(
            unreshaped,
            Ok(LogRecord {
                timestamp: envelope_millis,
                message: envelope.clone(),
            }),
            "case {:?}: expected the envelope written under its own time, got {unreshaped:?}",
            "the event as published"
        );

        // One row of the reshaped-input table: the text the target receives, and
        // the record it is written as — no record at all when the input names
        // none, which is the refusal [`NOT_A_LOG_RECORD`] states.
        struct Reshaped {
            case: &'static str,
            delivered: &'static str,
            /// The instant and the message the log event carries, or `None` when
            /// nothing is written.
            written: Option<(i64, &'static str)>,
        }
        let cases: &[Reshaped] = &[
            Reshaped {
                case: "a rendered record naming an instant",
                delivered: r#"{"timestamp":"2026-11-07T23:34:09Z","message":"order A-1 placed"}"#,
                written: Some((1_794_094_449_000, "order A-1 placed")),
            },
            Reshaped {
                case: "a rendered record naming epoch milliseconds",
                delivered: r#"{"timestamp":1794094449000,"message":"order A-1 placed"}"#,
                written: Some((1_794_094_449_000, "order A-1 placed")),
            },
            Reshaped {
                case: "a rendered record whose message is a document",
                delivered: r#"{"timestamp":1794094449000,"message":{"orderId":"A-1"}}"#,
                written: Some((1_794_094_449_000, r#"{"orderId":"A-1"}"#)),
            },
            Reshaped {
                case: "a rendered record naming no message",
                delivered: r#"{"timestamp":1794094449000}"#,
                written: None,
            },
            Reshaped {
                case: "a rendered record naming no timestamp",
                delivered: r#"{"message":"order A-1 placed"}"#,
                written: None,
            },
            Reshaped {
                case: "a rendered record naming neither",
                delivered: r#"{"orderId":"A-1"}"#,
                written: None,
            },
            Reshaped {
                case: "a rendered record whose message is empty",
                delivered: r#"{"timestamp":1794094449000,"message":""}"#,
                written: None,
            },
            Reshaped {
                case: "a rendered record whose timestamp is no instant",
                delivered: r#"{"timestamp":"half past four","message":"order A-1 placed"}"#,
                written: None,
            },
            Reshaped {
                case: "an input that is not one JSON object",
                delivered: "order A-1 placed",
                written: None,
            },
        ];
        for row in cases {
            let case = row.case;
            let actual = log_record(row.delivered, true);
            let wanted = match row.written {
                Some((timestamp, message)) => Ok(LogRecord {
                    timestamp,
                    message: message.to_string(),
                }),
                None => Err(NOT_A_LOG_RECORD.to_string()),
            };
            assert_eq!(
                actual, wanted,
                "case {case:?}: expected {wanted:?}, got {actual:?}"
            );
        }
    }

    /// The documents in force over a target are decided TOGETHER: an explicit
    /// deny in any of them refuses the write whatever another grants, and a grant
    /// in any of them admits it.
    #[test]
    fn the_documents_in_force_over_a_group_are_decided_together() {
        const RULE_ARN: &str = "arn:aws:events:eu-west-2:123456789012:rule/audit";
        const STREAM_ARN: &str =
            "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/events/audit:log-stream:audit";

        let statement = |effect: &str| {
            ::serde_json::json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": effect,
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": PUT_LOG_EVENTS_ACTION,
                    "Resource": "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/events/audit:*",
                }],
            })
            .to_string()
        };
        let about_something_else = ::serde_json::json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": SERVICE_PRINCIPAL },
                "Action": PUT_LOG_EVENTS_ACTION,
                "Resource": "arn:aws:logs:eu-west-2:123456789012:log-group:/other:*",
            }],
        })
        .to_string();

        // One row: the case, the documents in force, and whether they admit.
        struct Decided {
            case: &'static str,
            documents: Vec<String>,
            admitted: bool,
        }
        let rows = [
            Decided {
                case: "no document at all",
                documents: Vec::new(),
                admitted: false,
            },
            Decided {
                case: "one document granting the write",
                documents: vec![statement("Allow")],
                admitted: true,
            },
            Decided {
                case: "one document stating nothing about it",
                documents: vec![about_something_else.clone()],
                admitted: false,
            },
            Decided {
                case: "a grant beside a document about something else",
                documents: vec![about_something_else.clone(), statement("Allow")],
                admitted: true,
            },
            Decided {
                case: "a deny in the first document and a grant in the second",
                documents: vec![statement("Deny"), statement("Allow")],
                admitted: false,
            },
            Decided {
                case: "a grant in the first document and a deny in the second",
                documents: vec![statement("Allow"), statement("Deny")],
                admitted: false,
            },
        ];
        for row in &rows {
            let case = row.case;
            let attempt = DeliveryAttempt {
                rule: "audit",
                target_id: "log-group",
                action: PUT_LOG_EVENTS_ACTION,
                resource: STREAM_ARN,
            };
            let actual = documents_admit(&row.documents, &attempt, RULE_ARN);
            assert_eq!(
                actual, row.admitted,
                "case {case:?}: expected {}, got {actual}",
                row.admitted
            );
        }
    }

    /// The stream's ARN is the group's with that stream under it, from either
    /// form of the group's own ARN.
    #[test]
    fn a_streams_arn_is_built_from_the_targets_own_arn() {
        // (case, the target's ARN, the stream, the ARN the write is authorized on)
        let cases: &[(&str, &str, &str, &str)] = &[
            (
                "a target naming the group on its own",
                "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/events/audit",
                "my-rule",
                "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/events/audit:log-stream:\
                 my-rule",
            ),
            (
                "a target naming the group with everything under it",
                "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/events/audit:*",
                "my-rule",
                "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/events/audit:log-stream:\
                 my-rule",
            ),
        ];
        for (case, arn, stream, expected) in cases {
            let actual = log_stream_arn(arn, stream);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
