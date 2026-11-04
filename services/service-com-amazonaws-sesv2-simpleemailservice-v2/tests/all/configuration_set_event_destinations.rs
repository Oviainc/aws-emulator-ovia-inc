// SPDX-License-Identifier: CC0-1.0

//! The event destinations of a configuration set, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: a configuration set created by name stays addressable by
//! that name — an event destination attached to it is persisted exactly as the
//! definition described it and reported by a read of that set's destinations, repeated
//! reads report the same thing, a delete of a destination by name removes exactly that
//! destination from exactly that set and frees the name for a new definition, and a
//! delete of the set removes it together with everything configured under it — while a
//! call naming a set the account does not hold is refused `NotFoundException`, as is one
//! naming a destination the set does not hold, a destination name the set already holds
//! `AlreadyExistsException`, and a name or enum value outside its documented rule
//! `BadRequestException`.
//!
//! The tables below span what the rule generalises over: which destination object the
//! definition carries (each of the five modelled kinds), whether it is enabled, how many
//! event types it matches (none, one, several, every modelled value), how many
//! destinations one set holds, which of them a delete addresses, and the refusal legs of
//! each operation that takes a `ConfigurationSetName`.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};

/// Convenience calls for the event-destination surface, over the shared harness.
///
/// Shared with the sibling suites of this test target, so every case drives the same
/// wire path for a given operation.
pub(crate) trait EventDestinations {
    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Reply;
    async fn get_destinations(&self, set: &str) -> Reply;
    async fn delete_destination(&self, set: &str, name: &str) -> Reply;
    async fn delete_set(&self, set: &str) -> Reply;
}

impl EventDestinations for Ses {
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

    async fn delete_destination(&self, set: &str, name: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/v2/email/configuration-sets/{set}/event-destinations/{name}"),
            None,
        )
        .await
    }

    async fn delete_set(&self, set: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/v2/email/configuration-sets/{set}"),
            None,
        )
        .await
    }
}

/// The reported destinations of `set`, which must be a 200.
///
/// Shared with the sibling suites of this test target, so every case reads a set's
/// destinations the same way.
pub(crate) async fn reported_destinations(ses: &Ses, set: &str) -> Vec<Value> {
    let reply = ses.get_destinations(set).await;
    assert_eq!(
        reply.status,
        200,
        "read of {set}'s event destinations: expected 200, got {} {}",
        reply.status,
        reply.message()
    );
    match reply.member("EventDestinations") {
        Value::Array(destinations) => destinations,
        other => panic!("read of {set}'s event destinations: expected an array, got {other:?}"),
    }
}

/// The reported destination named `name`, or `None` when the set reports none.
///
/// Shared with the sibling suites of this test target, so every case reads one stored
/// destination the same way.
pub(crate) async fn reported_destination(ses: &Ses, set: &str, name: &str) -> Option<Value> {
    reported_destinations(ses, set)
        .await
        .into_iter()
        .find(|destination| destination.get("Name").and_then(Value::as_str) == Some(name))
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

/// A member of a reported destination, or `Value::Null` when absent.
fn member(destination: &Value, name: &str) -> Value {
    destination.get(name).cloned().unwrap_or(Value::Null)
}

/// Every value of the `EventType` enum, from
/// `smithy_query(query="shape",shape="EventType")`: the full set a destination may
/// match on.
///
/// Shared with the sibling suites of this test target, so the model's order is stated
/// once.
pub(crate) const EVENT_TYPES: &[&str] = &[
    "SEND",
    "REJECT",
    "BOUNCE",
    "COMPLAINT",
    "DELIVERY",
    "OPEN",
    "CLICK",
    "RENDERING_FAILURE",
    "DELIVERY_DELAY",
    "SUBSCRIPTION",
];

/// One row of the create-and-read table: the definition a create carries, and what a
/// read of the stored destination must report afterwards.
pub(crate) struct DestinationCase {
    pub(crate) case: &'static str,
    pub(crate) name: &'static str,
    pub(crate) definition: Value,
    enabled: bool,
    /// The event types a read must report: the ones the definition named, in the order
    /// SES reports them, which is ascending by name rather than the order they were sent
    /// in.
    matching_event_types: Vec<String>,
    /// The destination member a read must report, as `(member, value)`.
    destination_member: (&'static str, Value),
}

/// The rows of the create-and-read table: one per modelled destination kind, differing
/// also in the enabled flag and in how many event types they match.
pub(crate) fn destination_cases() -> Vec<DestinationCase> {
    // Every modelled event type, ascending by name — the order a read reports a
    // destination's types in, written out rather than derived from the model's own order.
    let every_event_type: Vec<String> = [
        "BOUNCE",
        "CLICK",
        "COMPLAINT",
        "DELIVERY",
        "DELIVERY_DELAY",
        "OPEN",
        "REJECT",
        "RENDERING_FAILURE",
        "SEND",
        "SUBSCRIPTION",
    ]
    .iter()
    .map(|value| value.to_string())
    .collect();
    vec![
        DestinationCase {
            case: "enabled Kinesis Firehose destination matching one event type",
            name: "firehose",
            definition: json!({
                "Enabled": true,
                "MatchingEventTypes": ["SEND"],
                "KinesisFirehoseDestination": {
                    "IamRoleArn": "arn:aws:iam::123456789012:role/ses-to-firehose",
                    "DeliveryStreamArn": "arn:aws:firehose:us-east-1:123456789012:deliverystream/ses-events",
                },
            }),
            enabled: true,
            matching_event_types: vec!["SEND".to_string()],
            destination_member: (
                "KinesisFirehoseDestination",
                json!({
                    "IamRoleArn": "arn:aws:iam::123456789012:role/ses-to-firehose",
                    "DeliveryStreamArn": "arn:aws:firehose:us-east-1:123456789012:deliverystream/ses-events",
                }),
            ),
        },
        DestinationCase {
            case: "disabled SNS destination matching several event types",
            name: "sns-paused",
            definition: json!({
                "Enabled": false,
                "MatchingEventTypes": ["BOUNCE", "COMPLAINT", "DELIVERY"],
                "SnsDestination": {"TopicArn": "arn:aws:sns:us-east-1:123456789012:ses-events"},
            }),
            enabled: false,
            matching_event_types: vec![
                "BOUNCE".to_string(),
                "COMPLAINT".to_string(),
                "DELIVERY".to_string(),
            ],
            destination_member: (
                "SnsDestination",
                json!({"TopicArn": "arn:aws:sns:us-east-1:123456789012:ses-events"}),
            ),
        },
        DestinationCase {
            case: "CloudWatch destination matching every modelled event type",
            name: "cloudwatch",
            definition: json!({
                "Enabled": true,
                "MatchingEventTypes": EVENT_TYPES,
                "CloudWatchDestination": {
                    "DimensionConfigurations": [{
                        "DimensionName": "campaign",
                        "DimensionValueSource": "MESSAGE_TAG",
                        "DefaultDimensionValue": "none",
                    }],
                },
            }),
            enabled: true,
            matching_event_types: every_event_type,
            destination_member: (
                "CloudWatchDestination",
                json!({
                    "DimensionConfigurations": [{
                        "DimensionName": "campaign",
                        "DimensionValueSource": "MESSAGE_TAG",
                        "DefaultDimensionValue": "none",
                    }],
                }),
            ),
        },
        DestinationCase {
            // `Enabled` is modelled with the default `false` and `MatchingEventTypes` is
            // optional on a definition, so a definition that names neither is a
            // destination that is off and matches nothing.
            case: "EventBridge destination with neither flag nor event types",
            name: "eventbridge",
            definition: json!({
                "EventBridgeDestination": {
                    "EventBusArn": "arn:aws:events:us-east-1:123456789012:event-bus/default",
                },
            }),
            enabled: false,
            matching_event_types: Vec::new(),
            destination_member: (
                "EventBridgeDestination",
                json!({"EventBusArn": "arn:aws:events:us-east-1:123456789012:event-bus/default"}),
            ),
        },
        DestinationCase {
            case: "Pinpoint destination matching the engagement event types",
            name: "pinpoint",
            definition: json!({
                "Enabled": true,
                "MatchingEventTypes": ["OPEN", "CLICK"],
                "PinpointDestination": {
                    "ApplicationArn": "arn:aws:mobiletargeting:us-east-1:123456789012:apps/1",
                },
            }),
            enabled: true,
            matching_event_types: vec!["CLICK".to_string(), "OPEN".to_string()],
            destination_member: (
                "PinpointDestination",
                json!({"ApplicationArn": "arn:aws:mobiletargeting:us-east-1:123456789012:apps/1"}),
            ),
        },
    ]
}

#[tokio::test]
async fn create_persists_the_destination_and_get_reads_it_back() {
    for case in destination_cases() {
        let ses = Ses::new();
        let set = "set-under-test";
        let created_set = ses.create_set(json!({"ConfigurationSetName": set})).await;
        assert_eq!(
            created_set.status,
            200,
            "case {:?}: creating the configuration set: expected 200, got {} {}",
            case.case,
            created_set.status,
            created_set.message()
        );

        let created = ses.create_destination(set, case.name, case.definition).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: expected 200, got {} {} ({})",
            case.case,
            created.status,
            created.error_code(),
            created.message()
        );
        assert!(
            created.is_empty_body(),
            "case {:?}: CreateConfigurationSetEventDestinationResponse declares no members, so \
             the body must be empty, got {:?}",
            case.case,
            created.member("EventDestinations")
        );

        let reported = reported_destination(&ses, set, case.name)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the created destination {:?} is not reported at all",
                    case.case, case.name
                )
            });
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
            case.matching_event_types,
            "case {:?}: expected MatchingEventTypes {:?}, got {:?}",
            case.case,
            case.matching_event_types,
            reported
        );
        let (destination_member, expected) = case.destination_member;
        assert_eq!(
            member(&reported, destination_member),
            expected,
            "case {:?}: expected {destination_member} {expected}, got {:?}",
            case.case,
            reported
        );

        // A read reports, it does not change: a second read answers the same thing.
        let read_again = reported_destination(&ses, set, case.name).await;
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
async fn destinations_accumulate_under_one_set_and_a_duplicate_name_is_refused() {
    let ses = Ses::new();
    let set = "many-destinations";
    assert_eq!(
        ses.create_set(json!({"ConfigurationSetName": set}))
            .await
            .status,
        200,
        "creating the configuration set must answer 200"
    );

    let cases = destination_cases();
    let expected_names: Vec<&str> = cases.iter().map(|case| case.name).collect();
    for case in destination_cases() {
        let created = ses.create_destination(set, case.name, case.definition).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: attaching another destination to the same set: expected 200, got {} {}",
            case.case,
            created.status,
            created.message()
        );
    }

    let mut reported: Vec<String> = reported_destinations(&ses, set)
        .await
        .iter()
        .map(|destination| {
            destination
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    reported.sort();
    let mut expected: Vec<String> = expected_names.iter().map(|name| name.to_string()).collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "a set holding {} destinations must report all of them, got {reported:?}",
        expected.len()
    );

    // The destination name identifies the destination WITHIN the set, so the second
    // create of a name the set already holds is the modelled duplicate refusal.
    let duplicate = ses
        .create_destination(set, expected_names[0], json!({"Enabled": true}))
        .await;
    assert_eq!(
        duplicate.status,
        400,
        "a duplicate destination name: expected 400, got {} {}",
        duplicate.status,
        duplicate.message()
    );
    assert_eq!(
        duplicate.error_code(),
        "AlreadyExistsException",
        "a duplicate destination name: expected AlreadyExistsException, got {:?} ({})",
        duplicate.error_code(),
        duplicate.message()
    );
    assert!(
        duplicate.message().contains(expected_names[0]) && duplicate.message().contains(set),
        "a duplicate destination name: the refusal must name the destination and the set, got {:?}",
        duplicate.message()
    );

    // The refused create changed nothing: the set still reports exactly what it held.
    let after: Vec<String> = reported_destinations(&ses, set)
        .await
        .iter()
        .map(|destination| {
            destination
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        after.len(),
        expected.len(),
        "after the refused duplicate the set must still hold {} destinations, got {after:?}",
        expected.len()
    );
}

#[tokio::test]
async fn deleting_the_set_removes_it_and_everything_under_it() {
    let ses = Ses::new();
    let set = "disposable-set";
    assert_eq!(
        ses.create_set(json!({"ConfigurationSetName": set}))
            .await
            .status,
        200,
        "creating the configuration set must answer 200"
    );
    assert_eq!(
        ses.create_destination(set, "firehose", json!({"Enabled": true, "MatchingEventTypes": ["SEND"], "SnsDestination": {"TopicArn": "arn:aws:sns:us-east-1:123456789012:t"}}))
            .await
            .status,
        200,
        "attaching a destination must answer 200"
    );

    let deleted = ses.delete_set(set).await;
    assert_eq!(
        deleted.status,
        200,
        "deleting the set: expected 200, got {} {}",
        deleted.status,
        deleted.message()
    );
    assert!(
        deleted.is_empty_body(),
        "DeleteConfigurationSetResponse declares no members, so the body must be empty"
    );

    for (case, reply) in [
        ("GetConfigurationSet", ses.get_set(set).await),
        (
            "GetConfigurationSetEventDestinations",
            ses.get_destinations(set).await,
        ),
        ("DeleteConfigurationSet again", ses.delete_set(set).await),
    ] {
        assert_eq!(
            reply.status,
            404,
            "case {case:?}: after the delete, expected 404, got {} {}",
            reply.status,
            reply.message()
        );
        assert_eq!(
            reply.error_code(),
            "NotFoundException",
            "case {case:?}: after the delete, expected NotFoundException, got {:?}",
            reply.error_code()
        );
    }

    // Re-creating the same name is a NEW set: it holds nothing the deleted one held.
    assert_eq!(
        ses.create_set(json!({"ConfigurationSetName": set}))
            .await
            .status,
        200,
        "re-creating the deleted name must answer 200"
    );
    let reported = reported_destinations(&ses, set).await;
    assert!(
        reported.is_empty(),
        "a re-created set must hold no event destination, got {reported:?}"
    );
}

#[tokio::test]
async fn a_set_the_account_does_not_hold_is_refused_as_not_found() {
    let ses = Ses::new();
    // One set exists, so a refusal is about the name asked for and not about an empty
    // account.
    assert_eq!(
        ses.create_set(json!({"ConfigurationSetName": "present"}))
            .await
            .status,
        200,
        "creating the configuration set must answer 200"
    );

    for missing in ["absent", "absent-2", "Absent_3"] {
        for (case, reply) in [
            (
                "CreateConfigurationSetEventDestination",
                ses.create_destination(missing, "d", json!({"Enabled": true}))
                    .await,
            ),
            (
                "GetConfigurationSetEventDestinations",
                ses.get_destinations(missing).await,
            ),
            ("DeleteConfigurationSet", ses.delete_set(missing).await),
        ] {
            assert_eq!(
                reply.status,
                404,
                "case {case:?} on {missing:?}: expected 404, got {} {}",
                reply.status,
                reply.message()
            );
            assert_eq!(
                reply.error_code(),
                "NotFoundException",
                "case {case:?} on {missing:?}: expected NotFoundException, got {:?} ({})",
                reply.error_code(),
                reply.message()
            );
            assert!(
                reply.message().contains(missing),
                "case {case:?} on {missing:?}: the refusal must name the set, got {:?}",
                reply.message()
            );
        }
    }

    // Nothing was created by the refused calls.
    let reported = reported_destinations(&ses, "present").await;
    assert!(
        reported.is_empty(),
        "the set that does exist must still hold no destination, got {reported:?}"
    );
}

#[tokio::test]
async fn a_name_or_enum_value_outside_its_documented_rule_is_refused() {
    let ses = Ses::new();
    let set = "validating-set";
    assert_eq!(
        ses.create_set(json!({"ConfigurationSetName": set}))
            .await
            .status,
        200,
        "creating the configuration set must answer 200"
    );

    // `ConfigurationSetName`: "The name can contain up to 64 alphanumeric characters,
    // including letters, numbers, hyphens (-) and underscores (_) only" — so a name with
    // another character, or one character past the bound, is outside the rule, while the
    // bound itself is inside it.
    let past_the_bound = "x".repeat(65);
    let at_the_bound = "x".repeat(64);
    let mut name_cases: Vec<(String, u16, &str)> = vec![
        ("has.a.dot".to_string(), 400, "BadRequestException"),
        ("has~a~tilde".to_string(), 400, "BadRequestException"),
        (past_the_bound, 400, "BadRequestException"),
        // Inside the rule, so the refusal is about the set not existing instead.
        (at_the_bound, 404, "NotFoundException"),
    ];
    name_cases.push(("ok_name-1".to_string(), 404, "NotFoundException"));
    for (name, status, code) in name_cases {
        for (case, reply) in [
            (
                "CreateConfigurationSetEventDestination",
                ses.create_destination(&name, "d", json!({"Enabled": true}))
                    .await,
            ),
            (
                "GetConfigurationSetEventDestinations",
                ses.get_destinations(&name).await,
            ),
            ("DeleteConfigurationSet", ses.delete_set(&name).await),
        ] {
            assert_eq!(
                reply.status, status,
                "case {case:?} on {name:?}: expected {status}, got {} {}",
                reply.status,
                reply.message()
            );
            assert_eq!(
                reply.error_code(),
                code,
                "case {case:?} on {name:?}: expected {code}, got {:?} ({})",
                reply.error_code(),
                reply.message()
            );
        }
    }

    // `MatchingEventTypes` carries `EventType` values and
    // `CloudWatchDimensionConfiguration$DimensionValueSource` carries
    // `DimensionValueSource` values: a value outside its own set is invalid input.
    for (case, definition) in [
        (
            "an event type that is not a modelled value",
            json!({"MatchingEventTypes": ["SENT"]}),
        ),
        (
            "a modelled event type in the wrong case",
            json!({"MatchingEventTypes": ["send"]}),
        ),
        (
            "one bad event type beside good ones",
            json!({"MatchingEventTypes": ["SEND", "DELIVERED", "BOUNCE"]}),
        ),
        (
            "a dimension value source that is not a modelled value",
            json!({
                "MatchingEventTypes": ["SEND"],
                "CloudWatchDestination": {"DimensionConfigurations": [{
                    "DimensionName": "d",
                    "DimensionValueSource": "MESSAGE-TAG",
                    "DefaultDimensionValue": "v",
                }]},
            }),
        ),
    ] {
        let reply = ses.create_destination(set, "bad-value", definition).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected 400, got {} {}",
            reply.status,
            reply.message()
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?} ({})",
            reply.error_code(),
            reply.message()
        );
    }

    // None of the refused creates stored anything.
    let reported = reported_destinations(&ses, set).await;
    assert!(
        reported.is_empty(),
        "no refused create may store a destination, got {reported:?}"
    );
}

/// The names `set` reports, sorted, so a case compares sets of names and not an order
/// the map does not promise.
pub(crate) async fn reported_names(ses: &Ses, set: &str) -> Vec<String> {
    let mut names: Vec<String> = reported_destinations(ses, set)
        .await
        .iter()
        .map(|destination| {
            destination
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

/// Attach every row of the create-and-read table to `set`, which must already exist.
async fn attach_every_destination(ses: &Ses, set: &str) {
    for case in destination_cases() {
        let created = ses.create_destination(set, case.name, case.definition).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: attaching the destination to {set:?}: expected 200, got {} {}",
            case.case,
            created.status,
            created.message()
        );
    }
}

/// Create `set`, which must answer 200.
pub(crate) async fn create_set(ses: &Ses, set: &str) {
    let created = ses.create_set(json!({"ConfigurationSetName": set})).await;
    assert_eq!(
        created.status,
        200,
        "creating configuration set {set:?}: expected 200, got {} {}",
        created.status,
        created.message()
    );
}

#[tokio::test]
async fn deleting_a_destination_removes_exactly_that_one_from_exactly_that_set() {
    // The rule generalises over which destination of a set is removed and how many the
    // set holds, so every row of the create-and-read table is deleted in turn — the only
    // destination of its set, and one of the five a set holding all of them has.
    for case in destination_cases() {
        for (shape, every) in [("the only destination of the set", false), ("one of five", true)] {
            let ses = Ses::new();
            let set = "set-under-test";
            // A second set holds a destination under the SAME name, so a delete that
            // reached beyond the set the URI names would show up here.
            let other_set = "other-set";
            create_set(&ses, set).await;
            create_set(&ses, other_set).await;
            if every {
                attach_every_destination(&ses, set).await;
            } else {
                let created = ses
                    .create_destination(set, case.name, case.definition.clone())
                    .await;
                assert_eq!(
                    created.status,
                    200,
                    "case {:?} ({shape}): attaching the destination: expected 200, got {} {}",
                    case.case,
                    created.status,
                    created.message()
                );
            }
            let created_elsewhere = ses
                .create_destination(other_set, case.name, case.definition.clone())
                .await;
            assert_eq!(
                created_elsewhere.status,
                200,
                "case {:?} ({shape}): attaching the same name to another set: expected 200, got \
                 {} {}",
                case.case,
                created_elsewhere.status,
                created_elsewhere.message()
            );

            let deleted = ses.delete_destination(set, case.name).await;
            assert_eq!(
                deleted.status,
                200,
                "case {:?} ({shape}): expected 200, got {} {} ({})",
                case.case,
                deleted.status,
                deleted.error_code(),
                deleted.message()
            );
            assert!(
                deleted.is_empty_body(),
                "case {:?} ({shape}): DeleteConfigurationSetEventDestinationResponse declares no \
                 members, so the body must be empty, got {:?}",
                case.case,
                deleted.member("EventDestinations")
            );

            // The deleted name is gone immediately, and only that name.
            let mut expected: Vec<String> = if every {
                destination_cases()
                    .iter()
                    .map(|row| row.name.to_string())
                    .filter(|name| name != case.name)
                    .collect()
            } else {
                Vec::new()
            };
            expected.sort();
            let remaining = reported_names(&ses, set).await;
            assert_eq!(
                remaining, expected,
                "case {:?} ({shape}): after the delete the set must report {expected:?}, got \
                 {remaining:?}",
                case.case
            );
            assert!(
                reported_destination(&ses, set, case.name).await.is_none(),
                "case {:?} ({shape}): the deleted destination is still reported",
                case.case
            );

            // The set the URI did not name kept its identically named destination.
            assert_eq!(
                reported_names(&ses, other_set).await,
                vec![case.name.to_string()],
                "case {:?} ({shape}): another set's destination of the same name must survive",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_destination_the_set_does_not_hold_is_refused_as_not_found() {
    let ses = Ses::new();
    let set = "holds-one";
    let empty_set = "holds-nothing";
    create_set(&ses, set).await;
    create_set(&ses, empty_set).await;
    let present = "kept";
    assert_eq!(
        ses.create_destination(set, present, json!({"Enabled": true}))
            .await
            .status,
        200,
        "attaching the destination that must survive: expected 200"
    );
    // One delete succeeds, so the refusals below are about the name asked for and not
    // about the operation refusing everything.
    let removed = "removed";
    assert_eq!(
        ses.create_destination(set, removed, json!({"Enabled": false}))
            .await
            .status,
        200,
        "attaching the destination to delete: expected 200"
    );
    assert_eq!(
        ses.delete_destination(set, removed).await.status,
        200,
        "the first delete of {removed:?}: expected 200"
    );

    for (case, target_set, name) in [
        ("a name the set never held", set, "never-there"),
        ("a name an earlier delete removed", set, removed),
        ("a name only another set holds", empty_set, present),
        ("a name differing only in case", set, "KEPT"),
    ] {
        let reply = ses.delete_destination(target_set, name).await;
        assert_eq!(
            reply.status,
            404,
            "case {case:?} ({target_set}/{name}): expected 404, got {} {}",
            reply.status,
            reply.message()
        );
        assert_eq!(
            reply.error_code(),
            "NotFoundException",
            "case {case:?} ({target_set}/{name}): expected NotFoundException, got {:?} ({})",
            reply.error_code(),
            reply.message()
        );
        assert_eq!(
            reply.message(),
            format!("No EventDestination found for {target_set}"),
            "case {case:?} ({target_set}/{name}): the refusal must answer the service's fixed \
             sentence naming the set, got {:?}",
            reply.message()
        );
    }

    // A set the account does not hold is refused for the set, whichever destination name
    // the URI carries.
    for missing in ["absent", "absent-2", "Absent_3"] {
        let reply = ses.delete_destination(missing, present).await;
        assert_eq!(
            reply.status,
            404,
            "a delete under the absent set {missing:?}: expected 404, got {} {}",
            reply.status,
            reply.message()
        );
        assert_eq!(
            reply.error_code(),
            "NotFoundException",
            "a delete under the absent set {missing:?}: expected NotFoundException, got {:?} ({})",
            reply.error_code(),
            reply.message()
        );
        assert!(
            reply.message().contains(missing),
            "a delete under the absent set {missing:?}: the refusal must name the set, got {:?}",
            reply.message()
        );
    }

    // None of the refused deletes removed anything.
    assert_eq!(
        reported_names(&ses, set).await,
        vec![present.to_string()],
        "a refused delete must leave the set's destinations alone"
    );
}

#[tokio::test]
async fn a_set_name_outside_its_documented_rule_is_refused_by_a_destination_delete() {
    let ses = Ses::new();
    // `ConfigurationSetName`: "The name can contain up to 64 alphanumeric characters,
    // including letters, numbers, hyphens (-) and underscores (_) only" — so a name with
    // another character, or one character past the bound, is outside the rule, while the
    // bound itself is inside it and the refusal is then about the set not existing.
    let past_the_bound = "x".repeat(65);
    let at_the_bound = "x".repeat(64);
    for (name, status, code) in [
        ("has.a.dot".to_string(), 400, "BadRequestException"),
        ("has~a~tilde".to_string(), 400, "BadRequestException"),
        (past_the_bound, 400, "BadRequestException"),
        (at_the_bound, 404, "NotFoundException"),
        ("ok_name-1".to_string(), 404, "NotFoundException"),
    ] {
        let reply = ses.delete_destination(&name, "d").await;
        assert_eq!(
            reply.status, status,
            "DeleteConfigurationSetEventDestination on {name:?}: expected {status}, got {} {}",
            reply.status,
            reply.message()
        );
        assert_eq!(
            reply.error_code(),
            code,
            "DeleteConfigurationSetEventDestination on {name:?}: expected {code}, got {:?} ({})",
            reply.error_code(),
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_deleted_name_can_be_attached_again_as_a_new_destination() {
    // The name identifies a destination within the set, so once the delete frees it the
    // same name takes a different definition — the rewire leg of an eventing workflow.
    let ses = Ses::new();
    let set = "rewired";
    create_set(&ses, set).await;
    let name = "events";

    for case in destination_cases() {
        let created = ses
            .create_destination(set, name, case.definition.clone())
            .await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: re-attaching {name:?}: expected 200, got {} {}",
            case.case,
            created.status,
            created.message()
        );

        let reported = reported_destination(&ses, set, name)
            .await
            .unwrap_or_else(|| panic!("case {:?}: {name:?} is not reported at all", case.case));
        let (destination_member, expected) = case.destination_member;
        assert_eq!(
            member(&reported, destination_member),
            expected,
            "case {:?}: after the re-attach the set must report {destination_member} {expected}, \
             got {reported:?}",
            case.case
        );
        assert_eq!(
            list(&reported, "MatchingEventTypes"),
            case.matching_event_types,
            "case {:?}: after the re-attach, expected MatchingEventTypes {:?}, got {reported:?}",
            case.case,
            case.matching_event_types
        );
        assert_eq!(
            flag(&reported, "Enabled"),
            case.enabled,
            "case {:?}: after the re-attach, expected Enabled {}, got {reported:?}",
            case.case,
            case.enabled
        );

        let deleted = ses.delete_destination(set, name).await;
        assert_eq!(
            deleted.status,
            200,
            "case {:?}: deleting {name:?} again: expected 200, got {} {}",
            case.case,
            deleted.status,
            deleted.message()
        );
        assert!(
            reported_names(&ses, set).await.is_empty(),
            "case {:?}: the set must hold nothing after the delete",
            case.case
        );
    }
}
