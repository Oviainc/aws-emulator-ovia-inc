// SPDX-License-Identifier: CC0-1.0

//! Publishing a configuration set's email events to an Amazon SNS topic, end to end
//! over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: `CreateConfigurationSet` is served from the store — it
//! persists the set under the name the request carried and answers HTTP 200 with the
//! empty body its output shape declares — and the set it stored is the one every
//! operation that names a configuration set then resolves, so an SNS event destination
//! attached to it is persisted with the topic the definition named and reported by a
//! read of that set's destinations. No call on this path answers a status or an error
//! code this service's model does not declare.
//!
//! The table below spans what the rule generalises over in the member the sibling
//! suites hold fixed: the `SnsDestination$TopicArn` value, whose target
//! `AmazonResourceName` is an unpatterned string of length at least 1
//! (`smithy_query(query="shape",shape="SnsDestination,AmazonResourceName")`), so the
//! rows differ in partition, Region, account id and topic-name form — including a FIFO
//! topic — beside the enabled flag and how many event types each matches. The recorded
//! case is one row of it.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};

/// Convenience calls for the path these cases drive, over the shared harness.
trait SnsEventDestinations {
    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Reply;
    async fn get_destinations(&self, set: &str) -> Reply;
}

impl SnsEventDestinations for Ses {
    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Reply {
        self.call(
            "POST",
            &format!("/v2/email/configuration-sets/{set}/event-destinations"),
            Some(json!({"EventDestinationName": name, "EventDestination": definition})),
        )
        .await
    }

    async fn get_destinations(&self, set: &str) -> Reply {
        self.call(
            "GET",
            &format!("/v2/email/configuration-sets/{set}/event-destinations"),
            None,
        )
        .await
    }
}

/// Assert `reply` carries neither the status nor the error code this service's model
/// declares nowhere, so a case that fails says which of the two it saw.
fn assert_modelled_answer(case: &str, reply: &Reply) {
    assert_ne!(
        reply.status, 501,
        "case {case:?}: this service's model declares no 501 for any operation, got {} ({})",
        reply.error_code(),
        reply.message()
    );
    assert_ne!(
        reply.error_code(),
        "NotImplemented",
        "case {case:?}: this service's model declares no NotImplemented error, got status {} ({})",
        reply.status,
        reply.message()
    );
}

/// Assert `reply` is the empty 200 the operation's output shape declares.
fn assert_empty_success(case: &str, reply: &Reply) {
    assert_modelled_answer(case, reply);
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected 200, got {} {} ({})",
        reply.status,
        reply.error_code(),
        reply.message()
    );
    assert!(
        reply.is_empty_body(),
        "case {case:?}: the output shape declares no members, so the body must be empty, got \
         {:?}",
        reply.member("EventDestinations")
    );
}

/// The destinations `set` reports, which must be a 200.
async fn reported_destinations(ses: &Ses, set: &str) -> Vec<Value> {
    let reply = ses.get_destinations(set).await;
    assert_modelled_answer(&format!("read of {set}'s event destinations"), &reply);
    assert_eq!(
        reply.status,
        200,
        "read of {set}'s event destinations: expected 200, got {} {} ({})",
        reply.status,
        reply.error_code(),
        reply.message()
    );
    match reply.member("EventDestinations") {
        Value::Array(destinations) => destinations,
        other => panic!("read of {set}'s event destinations: expected an array, got {other:?}"),
    }
}

/// The destination `set` reports under `name`, or `None` when it reports none.
async fn reported_destination(ses: &Ses, set: &str, name: &str) -> Option<Value> {
    reported_destinations(ses, set)
        .await
        .into_iter()
        .find(|destination| destination.get("Name").and_then(Value::as_str) == Some(name))
}

/// One row of the table: the topic an SNS destination publishes to, and the rest of the
/// definition that carries it.
struct TopicCase {
    case: &'static str,
    /// The configuration set the row creates and attaches to.
    set: &'static str,
    /// The `EventDestinationName` the destination is attached under.
    destination: &'static str,
    /// The `SnsDestination$TopicArn` the definition names.
    topic_arn: &'static str,
    enabled: bool,
    /// The event types the definition names, in the order it names them.
    matching_event_types: &'static [&'static str],
    /// The same types as a read reports them: ascending by name, which is the order SES
    /// reports a destination's matching event types in. Written out per row rather than
    /// derived, so the expectation does not come from the behaviour under test.
    reported_event_types: &'static [&'static str],
}

/// The rows of the table: the same attach, differing in the topic it publishes to.
///
/// The event types come from the `EventType` enum
/// (`smithy_query(query="shape",shape="EventType")`); the rows take one, several and
/// none of them.
const TOPIC_CASES: &[TopicCase] = &[
    TopicCase {
        case: "a topic in the commercial partition",
        set: "commercial-set",
        destination: "sns-gate",
        topic_arn: "arn:aws:sns:us-east-1:123456789012:ses-events",
        enabled: true,
        matching_event_types: &["SEND"],
        reported_event_types: &["SEND"],
    },
    TopicCase {
        case: "a topic in another Region, under another account, named with underscores",
        set: "other_region_set",
        destination: "sns_gate_2",
        topic_arn: "arn:aws:sns:eu-west-2:210987654321:ses_events_2",
        enabled: true,
        matching_event_types: &["BOUNCE", "COMPLAINT", "DELIVERY"],
        reported_event_types: &["BOUNCE", "COMPLAINT", "DELIVERY"],
    },
    TopicCase {
        case: "a topic in the GovCloud partition",
        set: "govcloud-set",
        destination: "sns-gate-gov",
        topic_arn: "arn:aws-us-gov:sns:us-gov-west-1:123456789012:ses-events-gov",
        enabled: false,
        matching_event_types: &["REJECT", "RENDERING_FAILURE"],
        reported_event_types: &["REJECT", "RENDERING_FAILURE"],
    },
    TopicCase {
        case: "a topic in the China partition, named with digits and mixed case",
        set: "China_Set_9",
        destination: "SNSGate9",
        topic_arn: "arn:aws-cn:sns:cn-north-1:123456789012:SESEvents9",
        enabled: true,
        matching_event_types: &["OPEN", "CLICK", "SUBSCRIPTION"],
        reported_event_types: &["CLICK", "OPEN", "SUBSCRIPTION"],
    },
    TopicCase {
        case: "a FIFO topic",
        set: "fifo-set",
        destination: "sns-fifo",
        topic_arn: "arn:aws:sns:ap-southeast-2:123456789012:ses-events.fifo",
        enabled: true,
        matching_event_types: &["DELIVERY_DELAY"],
        reported_event_types: &["DELIVERY_DELAY"],
    },
    TopicCase {
        // `EventDestinationDefinition$MatchingEventTypes` is optional and `Enabled` is
        // modelled with the default `false`, so this row is a destination that is off
        // and matches nothing — the topic is still what the definition named.
        case: "a topic with a one-character name, matching nothing",
        set: "s",
        destination: "d",
        topic_arn: "arn:aws:sns:us-west-2:123456789012:t",
        enabled: false,
        matching_event_types: &[],
        reported_event_types: &[],
    },
];

/// The definition a row attaches.
fn definition(case: &TopicCase) -> Value {
    json!({
        "Enabled": case.enabled,
        "MatchingEventTypes": case.matching_event_types,
        "SnsDestination": {"TopicArn": case.topic_arn},
    })
}

/// A boolean member of a reported destination, where absent reads as `false`:
/// `EventDestination$Enabled` is modelled with the default `false`, so a serializer that
/// omits a defaulted member and one that writes it out agree here.
fn flag(destination: &Value, member: &str) -> bool {
    destination
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// A list member of a reported destination, where absent reads as empty.
fn list(destination: &Value, member: &str) -> Vec<String> {
    destination
        .get(member)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `SnsDestination$TopicArn` a reported destination carries, or `""` when it carries
/// none.
fn reported_topic_arn(destination: &Value) -> String {
    destination
        .pointer("/SnsDestination/TopicArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_created_set_takes_an_sns_destination_for_the_topic_the_definition_named() {
    for case in TOPIC_CASES {
        let ses = Ses::new();

        // The create is served from the store and answers the empty 200 its output
        // shape declares — not a status or a code the model does not carry.
        let created_set = ses
            .create_set(json!({"ConfigurationSetName": case.set}))
            .await;
        assert_empty_success(
            &format!("case {:?}: CreateConfigurationSet", case.case),
            &created_set,
        );

        // ...and the set it stored is addressable by the name it was created under.
        let read_set = ses.get_set(case.set).await;
        assert_modelled_answer(
            &format!("case {:?}: GetConfigurationSet", case.case),
            &read_set,
        );
        assert_eq!(
            read_set
                .member("ConfigurationSetName")
                .as_str()
                .unwrap_or_default(),
            case.set,
            "case {:?}: expected the set to be reported under {:?}, got status {} {:?}",
            case.case,
            case.set,
            read_set.status,
            read_set.member("ConfigurationSetName")
        );

        // ...so the attach resolves it and is served too.
        let attached = ses
            .create_destination(case.set, case.destination, definition(case))
            .await;
        assert_empty_success(
            &format!("case {:?}: CreateConfigurationSetEventDestination", case.case),
            &attached,
        );

        let reported = reported_destination(&ses, case.set, case.destination)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the attached destination {:?} is not reported at all",
                    case.case, case.destination
                )
            });
        assert_eq!(
            reported_topic_arn(&reported),
            case.topic_arn,
            "case {:?}: expected TopicArn {:?}, got {:?}",
            case.case,
            case.topic_arn,
            reported
        );
        assert_eq!(
            flag(&reported, "Enabled"),
            case.enabled,
            "case {:?}: expected Enabled {}, got {:?}",
            case.case,
            case.enabled,
            reported
        );
        assert_eq!(
            list(&reported, "MatchingEventTypes"),
            case.reported_event_types,
            "case {:?}: expected MatchingEventTypes {:?}, got {:?}",
            case.case,
            case.reported_event_types,
            reported
        );
        // An SNS destination publishes to a topic and to nothing else: no other
        // destination object is invented for it.
        for absent in [
            "KinesisFirehoseDestination",
            "CloudWatchDestination",
            "EventBridgeDestination",
            "PinpointDestination",
        ] {
            assert!(
                reported.get(absent).is_none(),
                "case {:?}: the definition named only an SNS topic, so {absent} must be absent, \
                 got {:?}",
                case.case,
                reported
            );
        }

        // A read reports, it does not change: reading again answers the same thing.
        let read_again = reported_destination(&ses, case.set, case.destination).await;
        assert_eq!(
            read_again.as_ref(),
            Some(&reported),
            "case {:?}: a second read reported {:?}, not the first read's {:?}",
            case.case,
            read_again,
            reported
        );
    }
}

#[tokio::test]
async fn one_destination_name_under_two_sets_is_two_destinations() {
    // `EventDestinationName` "identifies the event destination within the configuration
    // set", so the same name under two sets names two destinations, each publishing to
    // the topic its own definition carried.
    let ses = Ses::new();
    let shared_name = "sns-gate";
    let topics = [
        (
            "first-set",
            "arn:aws:sns:us-east-1:123456789012:ses-events-first",
        ),
        (
            "second_set",
            "arn:aws:sns:eu-central-1:210987654321:ses-events-second",
        ),
    ];

    for (set, topic_arn) in topics {
        let created = ses.create_set(json!({"ConfigurationSetName": set})).await;
        assert_empty_success(&format!("CreateConfigurationSet {set:?}"), &created);
        let attached = ses
            .create_destination(
                set,
                shared_name,
                json!({
                    "Enabled": true,
                    "MatchingEventTypes": ["SEND"],
                    "SnsDestination": {"TopicArn": topic_arn},
                }),
            )
            .await;
        assert_empty_success(
            &format!("CreateConfigurationSetEventDestination under {set:?}"),
            &attached,
        );
    }

    for (set, topic_arn) in topics {
        let reported = reported_destinations(&ses, set).await;
        assert_eq!(
            reported.len(),
            1,
            "set {set:?}: expected to hold exactly the destination attached to it, got {reported:?}"
        );
        assert_eq!(
            reported_topic_arn(&reported[0]),
            topic_arn,
            "set {set:?}: expected TopicArn {topic_arn:?}, got {:?}",
            reported[0]
        );
    }
}
