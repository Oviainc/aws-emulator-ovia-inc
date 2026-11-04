// SPDX-License-Identifier: CC0-1.0

//! The configuration-set / event-publication surface and the account report, end to
//! end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: every operation this service's model binds answers that
//! operation's modelled contract computed from the store — a success with its
//! modelled status and body, or one of the errors the model declares for it with that
//! error's status — and never a status or an error code the model does not declare.
//! Applied to the surface a client needs to publish email events: a configuration set
//! created by name is addressable by that name, an event destination under it is
//! attachable and then UPDATABLE in place, where the definition an update carries
//! becomes that destination's whole configuration, and the account report is readable
//! without having configured anything.
//!
//! The tables below span what the rule generalises over: the form of the configuration
//! set name, which option objects a create carries, which member of a destination an
//! update changes (its enabled flag, how many event types it matches, which service it
//! publishes to, the identity of that service's resource), an update that changes
//! nothing, and the refusal legs of each operation — a set the account does not hold, a
//! destination the set does not hold, a name outside the documented character rule and
//! an enum value outside its modelled value set.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};

/// Every value of the `EventType` enum, from
/// `smithy_query(query="shape",shape="EventType")`: the full set a destination may
/// match on.
const EVENT_TYPES: &[&str] = &[
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
];

/// Convenience calls for the surface these cases drive, over the shared harness.
trait EventPublicationSurface {
    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Reply;
    async fn update_destination(&self, set: &str, name: &str, definition: Value) -> Reply;
    async fn get_destinations(&self, set: &str) -> Reply;
    async fn get_account(&self) -> Reply;
}

impl EventPublicationSurface for Ses {
    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Reply {
        self.call(
            "POST",
            &format!("/v2/email/configuration-sets/{set}/event-destinations"),
            Some(json!({"EventDestinationName": name, "EventDestination": definition})),
        )
        .await
    }

    async fn update_destination(&self, set: &str, name: &str, definition: Value) -> Reply {
        self.call(
            "PUT",
            &format!("/v2/email/configuration-sets/{set}/event-destinations/{name}"),
            Some(json!({"EventDestination": definition})),
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

    async fn get_account(&self) -> Reply {
        self.call("GET", "/v2/email/account", None).await
    }
}

/// The destination named `name` as a read of `set`'s destinations reports it, or
/// `None` when the set reports no such destination.
async fn reported_destination(ses: &Ses, set: &str, name: &str) -> Option<Value> {
    let reply = ses.get_destinations(set).await;
    assert_eq!(
        reply.status,
        200,
        "read of {set:?}'s event destinations: expected 200, got {} {} ({})",
        reply.status,
        reply.error_code(),
        reply.message()
    );
    match reply.member("EventDestinations") {
        Value::Array(destinations) => destinations
            .into_iter()
            .find(|destination| destination.get("Name").and_then(Value::as_str) == Some(name)),
        other => panic!("read of {set:?}'s event destinations: expected an array, got {other:?}"),
    }
}

/// The destination named `name`, which must be reported.
async fn destination(ses: &Ses, set: &str, name: &str) -> Value {
    reported_destination(ses, set, name)
        .await
        .unwrap_or_else(|| panic!("configuration set {set:?} reports no destination {name:?}"))
}

/// A boolean member of a reported destination, where absent reads as `false`:
/// `EventDestination$Enabled` is modelled with the default `false`, so a serializer
/// that omits a defaulted member and one that writes it out agree here.
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

/// An SNS destination publishing to `topic`.
fn sns(topic: &str) -> Value {
    json!({"TopicArn": format!("arn:aws:sns:us-east-1:123456789012:{topic}")})
}

/// A Kinesis Data Firehose destination publishing to `stream`.
fn firehose(stream: &str) -> Value {
    json!({
        "IamRoleArn": "arn:aws:iam::123456789012:role/ses-events",
        "DeliveryStreamArn":
            format!("arn:aws:firehose:us-east-1:123456789012:deliverystream/{stream}"),
    })
}

/// What a call must answer: the modelled success, or the modelled error for its
/// reason.
enum Expected {
    /// HTTP 200 with the operation's output body.
    Success,
    /// The modelled error code, with the status the model gives it.
    Refusal(u16, &'static str),
}

/// Assert `reply` is what the model says the call answers, and in particular that it
/// is not a status or an error code this service's model does not declare.
fn assert_modelled(case: &str, reply: &Reply, expected: &Expected) {
    assert_ne!(
        reply.status, 501,
        "case {case:?}: this service's model declares no 501 for any operation, got \
         {} ({})",
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
    match expected {
        Expected::Success => assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected 200, got {} {} ({})",
            reply.status,
            reply.error_code(),
            reply.message()
        ),
        Expected::Refusal(status, code) => {
            assert_eq!(
                reply.status, *status,
                "case {case:?}: expected {status}, got {} {} ({})",
                reply.status,
                reply.error_code(),
                reply.message()
            );
            assert_eq!(
                reply.error_code(),
                *code,
                "case {case:?}: expected {code}, got {:?} ({})",
                reply.error_code(),
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn the_whole_surface_answers_its_modelled_contract() {
    let ses = Ses::new();

    // The set names span the form the documented rule generalises over: one
    // character, the longest allowed, digits, hyphens and underscores.
    let longest = "s".repeat(64);
    let set_names: Vec<String> = vec![
        "s".to_string(),
        "sim-cs-1".to_string(),
        "set_with_underscores".to_string(),
        "SET0123456789".to_string(),
        longest,
    ];

    for name in &set_names {
        // The option objects a create may carry, one row with none and one with
        // several, so the accepted surface is not just the bare required member.
        let body = if name.len() % 2 == 0 {
            json!({"ConfigurationSetName": name})
        } else {
            json!({
                "ConfigurationSetName": name,
                "DeliveryOptions": {"TlsPolicy": "REQUIRE", "MaxDeliverySeconds": 300},
                "ReputationOptions": {"ReputationMetricsEnabled": true},
                "SendingOptions": {"SendingEnabled": true},
                "SuppressionOptions": {"SuppressedReasons": ["BOUNCE", "COMPLAINT"]},
                "TrackingOptions": {"CustomRedirectDomain": "click.example.com"},
                "Tags": [{"Key": "team", "Value": "mail"}],
            })
        };
        let created = ses.create_set(body).await;
        assert_modelled(
            &format!("CreateConfigurationSet {name:?}"),
            &created,
            &Expected::Success,
        );
        assert!(
            created.is_empty_body(),
            "case {name:?}: CreateConfigurationSetResponse declares no members, so the body must \
             be empty, got {:?}",
            created.member("ConfigurationSetName")
        );

        // The created set is addressable by the name it was created under, by the
        // reader of the set and by the reader of its event destinations.
        for (case, reply) in [
            ("GetConfigurationSet", ses.get_set(name).await),
            (
                "GetConfigurationSetEventDestinations",
                ses.get_destinations(name).await,
            ),
        ] {
            assert_modelled(
                &format!("{case} {name:?}"),
                &reply,
                &Expected::Success,
            );
        }

        // ...and it is extendable: a destination attaches to it and is then updatable
        // in place.
        let attached = ses
            .create_destination(
                name,
                "gate",
                json!({
                    "Enabled": false,
                    "MatchingEventTypes": ["SEND", "DELIVERY"],
                    "SnsDestination": sns("ses-events"),
                }),
            )
            .await;
        assert_modelled(
            &format!("CreateConfigurationSetEventDestination {name:?}"),
            &attached,
            &Expected::Success,
        );
        let updated = ses
            .update_destination(
                name,
                "gate",
                json!({
                    "Enabled": true,
                    "MatchingEventTypes": ["SEND", "DELIVERY"],
                    "SnsDestination": sns("ses-events"),
                }),
            )
            .await;
        assert_modelled(
            &format!("UpdateConfigurationSetEventDestination {name:?}"),
            &updated,
            &Expected::Success,
        );
        assert!(
            updated.is_empty_body(),
            "case {name:?}: UpdateConfigurationSetEventDestinationResponse declares no members, \
             so the body must be empty, got {:?}",
            updated.member("EventDestination")
        );
    }

    // A second create of a name the account holds is the modelled duplicate refusal,
    // not a 501 and not a success.
    let duplicate = ses
        .create_set(json!({"ConfigurationSetName": set_names[0]}))
        .await;
    assert_modelled(
        "CreateConfigurationSet of a name the account already holds",
        &duplicate,
        &Expected::Refusal(400, "AlreadyExistsException"),
    );

    // The account report needs nothing configured: it answers for the one emulated
    // account, and a repeated read answers the same thing.
    let account = ses.get_account().await;
    assert_modelled("GetAccount", &account, &Expected::Success);
    for required in ["SendingEnabled", "DedicatedIpAutoWarmupEnabled", "ProductionAccessEnabled"] {
        assert!(
            account.member(required).is_boolean(),
            "GetAccount: {required} is modelled required with a boolean default, got {:?}",
            account.member(required)
        );
    }
    for reported in ["SendQuota", "SuppressionAttributes", "VdmAttributes", "EnforcementStatus"] {
        assert!(
            !account.member(reported).is_null(),
            "GetAccount: expected the report to carry {reported}, got {:?}",
            account.member(reported)
        );
    }
    let again = ses.get_account().await;
    for reported in [
        "SendingEnabled",
        "ProductionAccessEnabled",
        "SendQuota",
        "SuppressionAttributes",
        "EnforcementStatus",
    ] {
        assert_eq!(
            again.member(reported),
            account.member(reported),
            "GetAccount: a read must not change what a later read sees; {reported} was {:?}, \
             then {:?}",
            account.member(reported),
            again.member(reported)
        );
    }
}

/// One row of the update table: the definition the destination is created with, the
/// definition an update carries, and the configuration a read must report afterwards.
struct UpdateCase {
    case: &'static str,
    initial: Value,
    update: Value,
    enabled: bool,
    /// The event types a read must report: the ones the definition named, in the order
    /// SES reports them, which is ascending by name rather than the order they were sent
    /// in.
    matching_event_types: Vec<String>,
    /// The destination members a read must report, as `(member, value)`.
    present: Vec<(&'static str, Value)>,
    /// The destination members a read must NOT report.
    absent: Vec<&'static str>,
}

/// The rows of the update table: each changes a different part of the destination's
/// configuration, and one changes nothing at all.
fn update_cases() -> Vec<UpdateCase> {
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
        UpdateCase {
            case: "a disabled destination is enabled",
            initial: json!({
                "Enabled": false,
                "MatchingEventTypes": ["SEND", "DELIVERY"],
                "SnsDestination": sns("events"),
            }),
            update: json!({
                "Enabled": true,
                "MatchingEventTypes": ["SEND", "DELIVERY"],
                "SnsDestination": sns("events"),
            }),
            enabled: true,
            matching_event_types: vec!["DELIVERY".to_string(), "SEND".to_string()],
            present: vec![("SnsDestination", sns("events"))],
            absent: vec!["KinesisFirehoseDestination", "CloudWatchDestination"],
        },
        UpdateCase {
            case: "an enabled destination is disabled",
            initial: json!({
                "Enabled": true,
                "MatchingEventTypes": ["BOUNCE"],
                "SnsDestination": sns("events"),
            }),
            // `Enabled` is modelled with the default `false`, so a definition that
            // does not name it describes a destination that is off.
            update: json!({
                "MatchingEventTypes": ["BOUNCE"],
                "SnsDestination": sns("events"),
            }),
            enabled: false,
            matching_event_types: vec!["BOUNCE".to_string()],
            present: vec![("SnsDestination", sns("events"))],
            absent: vec!["KinesisFirehoseDestination"],
        },
        UpdateCase {
            case: "the matched event types widen to every modelled value",
            initial: json!({
                "Enabled": true,
                "MatchingEventTypes": ["SEND"],
                "SnsDestination": sns("events"),
            }),
            update: json!({
                "Enabled": true,
                "MatchingEventTypes": EVENT_TYPES,
                "SnsDestination": sns("events"),
            }),
            enabled: true,
            matching_event_types: every_event_type,
            present: vec![("SnsDestination", sns("events"))],
            absent: vec!["PinpointDestination"],
        },
        UpdateCase {
            case: "the matched event types narrow to none",
            initial: json!({
                "Enabled": true,
                "MatchingEventTypes": ["SEND", "BOUNCE", "COMPLAINT"],
                "SnsDestination": sns("events"),
            }),
            // `MatchingEventTypes` is optional on a definition, so a definition that
            // names none describes a destination that matches nothing.
            update: json!({"Enabled": true, "SnsDestination": sns("events")}),
            enabled: true,
            matching_event_types: Vec::new(),
            present: vec![("SnsDestination", sns("events"))],
            absent: vec!["CloudWatchDestination"],
        },
        UpdateCase {
            case: "the destination publishes to another topic of the same service",
            initial: json!({
                "Enabled": true,
                "MatchingEventTypes": ["SEND"],
                "SnsDestination": sns("first-topic"),
            }),
            update: json!({
                "Enabled": true,
                "MatchingEventTypes": ["SEND"],
                "SnsDestination": sns("second-topic"),
            }),
            enabled: true,
            matching_event_types: vec!["SEND".to_string()],
            present: vec![("SnsDestination", sns("second-topic"))],
            absent: vec!["KinesisFirehoseDestination"],
        },
        UpdateCase {
            case: "the destination publishes to another service entirely",
            initial: json!({
                "Enabled": true,
                "MatchingEventTypes": ["SEND"],
                "SnsDestination": sns("events"),
            }),
            update: json!({
                "Enabled": true,
                "MatchingEventTypes": ["SEND", "REJECT"],
                "KinesisFirehoseDestination": firehose("ses-events"),
            }),
            enabled: true,
            matching_event_types: vec!["REJECT".to_string(), "SEND".to_string()],
            present: vec![("KinesisFirehoseDestination", firehose("ses-events"))],
            // The definition the update carried does not describe an SNS destination,
            // so the destination no longer publishes to one.
            absent: vec!["SnsDestination"],
        },
        UpdateCase {
            case: "the destination publishes CloudWatch dimensions",
            initial: json!({
                "Enabled": false,
                "MatchingEventTypes": ["OPEN", "CLICK"],
                "PinpointDestination": {
                    "ApplicationArn": "arn:aws:mobiletargeting:us-east-1:123456789012:apps/1",
                },
            }),
            update: json!({
                "Enabled": true,
                "MatchingEventTypes": ["OPEN", "CLICK"],
                "CloudWatchDestination": {"DimensionConfigurations": [{
                    "DimensionName": "campaign",
                    "DimensionValueSource": "LINK_TAG",
                    "DefaultDimensionValue": "none",
                }]},
            }),
            enabled: true,
            matching_event_types: vec!["CLICK".to_string(), "OPEN".to_string()],
            present: vec![(
                "CloudWatchDestination",
                json!({"DimensionConfigurations": [{
                    "DimensionName": "campaign",
                    "DimensionValueSource": "LINK_TAG",
                    "DefaultDimensionValue": "none",
                }]}),
            )],
            absent: vec!["PinpointDestination"],
        },
        UpdateCase {
            case: "the update carries the definition the destination already has",
            initial: json!({
                "Enabled": true,
                "MatchingEventTypes": ["DELIVERY", "DELIVERY_DELAY"],
                "EventBridgeDestination": {
                    "EventBusArn": "arn:aws:events:us-east-1:123456789012:event-bus/default",
                },
            }),
            update: json!({
                "Enabled": true,
                "MatchingEventTypes": ["DELIVERY", "DELIVERY_DELAY"],
                "EventBridgeDestination": {
                    "EventBusArn": "arn:aws:events:us-east-1:123456789012:event-bus/default",
                },
            }),
            enabled: true,
            matching_event_types: vec!["DELIVERY".to_string(), "DELIVERY_DELAY".to_string()],
            present: vec![(
                "EventBridgeDestination",
                json!({"EventBusArn": "arn:aws:events:us-east-1:123456789012:event-bus/default"}),
            )],
            absent: vec!["SnsDestination"],
        },
    ]
}

#[tokio::test]
async fn an_update_makes_the_definition_the_destinations_configuration() {
    for case in update_cases() {
        let ses = Ses::new();
        let set = "updated-set";
        assert_eq!(
            ses.create_set(json!({"ConfigurationSetName": set}))
                .await
                .status,
            200,
            "case {:?}: creating the configuration set must answer 200",
            case.case
        );
        // A second destination under the same set, so an update is shown to address
        // only the destination its URI names.
        let untouched_definition = json!({
            "Enabled": true,
            "MatchingEventTypes": ["COMPLAINT"],
            "SnsDestination": sns("untouched"),
        });
        for (name, definition) in [
            ("under-test", case.initial.clone()),
            ("untouched", untouched_definition.clone()),
        ] {
            let created = ses.create_destination(set, name, definition).await;
            assert_eq!(
                created.status,
                200,
                "case {:?}: attaching {name:?}: expected 200, got {} {} ({})",
                case.case,
                created.status,
                created.error_code(),
                created.message()
            );
        }

        let updated = ses
            .update_destination(set, "under-test", case.update.clone())
            .await;
        assert_modelled(case.case, &updated, &Expected::Success);

        let reported = destination(&ses, set, "under-test").await;
        assert_eq!(
            member(&reported, "Name"),
            json!("under-test"),
            "case {:?}: the destination keeps the name it is addressed by, got {reported:?}",
            case.case
        );
        assert_eq!(
            flag(&reported, "Enabled"),
            case.enabled,
            "case {:?}: expected Enabled {}, got {reported:?}",
            case.case,
            case.enabled
        );
        assert_eq!(
            list(&reported, "MatchingEventTypes"),
            case.matching_event_types,
            "case {:?}: expected MatchingEventTypes {:?}, got {reported:?}",
            case.case,
            case.matching_event_types
        );
        for (name, expected) in &case.present {
            assert_eq!(
                member(&reported, name),
                *expected,
                "case {:?}: expected {name} {expected}, got {reported:?}",
                case.case
            );
        }
        for name in &case.absent {
            assert_eq!(
                member(&reported, name),
                Value::Null,
                "case {:?}: the definition the update carried describes no {name}, so the read \
                 must report none, got {reported:?}",
                case.case
            );
        }

        // A read reports, it does not change: a second read answers the same thing.
        let read_again = destination(&ses, set, "under-test").await;
        assert_eq!(
            read_again, reported,
            "case {:?}: a second read reported {read_again:?}, not the first read's {reported:?}",
            case.case
        );

        // The sibling destination of the same set is untouched.
        let sibling = destination(&ses, set, "untouched").await;
        assert_eq!(
            member(&sibling, "SnsDestination"),
            sns("untouched"),
            "case {:?}: an update must change only the destination it names, got {sibling:?}",
            case.case
        );
        assert_eq!(
            list(&sibling, "MatchingEventTypes"),
            vec!["COMPLAINT".to_string()],
            "case {:?}: an update must change only the destination it names, got {sibling:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_update_of_a_resource_the_account_does_not_hold_is_refused_as_not_found() {
    let ses = Ses::new();
    let set = "held-set";
    assert_eq!(
        ses.create_set(json!({"ConfigurationSetName": set}))
            .await
            .status,
        200,
        "creating the configuration set must answer 200"
    );
    let definition = json!({
        "Enabled": true,
        "MatchingEventTypes": ["SEND"],
        "SnsDestination": sns("events"),
    });
    assert_eq!(
        ses.create_destination(set, "held", definition.clone())
            .await
            .status,
        200,
        "attaching the destination must answer 200"
    );
    let before = destination(&ses, set, "held").await;

    // A set the account does not hold, and a destination the set does not hold: both
    // name a resource that does not exist, whatever the other half of the pair is. Each
    // refusal names the resource SES states as missing — the set the request addressed,
    // whether it is the set itself that is not there or the destination within it.
    for (case, set_name, destination_name, named) in [
        ("a set the account does not hold", "absent-set", "held", "absent-set"),
        ("another set the account does not hold", "Absent_Set-2", "absent", "Absent_Set-2"),
        ("a destination the set does not hold", set, "absent-destination", set),
        ("a destination name differing only in case", set, "HELD", set),
    ] {
        let reply = ses
            .update_destination(set_name, destination_name, definition.clone())
            .await;
        assert_modelled(case, &reply, &Expected::Refusal(404, "NotFoundException"));
        assert!(
            reply.message().contains(named),
            "case {case:?}: the refusal must name the resource {named:?} it did not find, got {:?}",
            reply.message()
        );
    }

    // `ConfigurationSetName`: "The name can contain up to 64 alphanumeric characters,
    // including letters, numbers, hyphens (-) and underscores (_) only" — so a name
    // carrying another character, or one character past the bound, is invalid input,
    // while the bound itself is merely a set the account does not hold.
    let past_the_bound = "x".repeat(65);
    let at_the_bound = "x".repeat(64);
    for (case, set_name, expected) in [
        (
            "a set name carrying a character outside the rule",
            "has.a.dot".to_string(),
            Expected::Refusal(400, "BadRequestException"),
        ),
        (
            "a set name one character past the bound",
            past_the_bound,
            Expected::Refusal(400, "BadRequestException"),
        ),
        (
            "a set name at the bound",
            at_the_bound,
            Expected::Refusal(404, "NotFoundException"),
        ),
    ] {
        let reply = ses
            .update_destination(&set_name, "held", definition.clone())
            .await;
        assert_modelled(case, &reply, &expected);
    }

    // An enum-valued member outside its own modelled value set is invalid input, on
    // the update exactly as on the create.
    for (case, bad_definition) in [
        (
            "an event type that is not a modelled value",
            json!({"MatchingEventTypes": ["SENT"], "SnsDestination": sns("events")}),
        ),
        (
            "a modelled event type in the wrong case",
            json!({"MatchingEventTypes": ["send"], "SnsDestination": sns("events")}),
        ),
        (
            "one bad event type beside good ones",
            json!({
                "MatchingEventTypes": ["SEND", "DELIVERED", "BOUNCE"],
                "SnsDestination": sns("events"),
            }),
        ),
        (
            "a dimension value source that is not a modelled value",
            json!({
                "MatchingEventTypes": ["SEND"],
                "CloudWatchDestination": {"DimensionConfigurations": [{
                    "DimensionName": "campaign",
                    "DimensionValueSource": "MESSAGE-TAG",
                    "DefaultDimensionValue": "none",
                }]},
            }),
        ),
    ] {
        let reply = ses.update_destination(set, "held", bad_definition).await;
        assert_modelled(case, &reply, &Expected::Refusal(400, "BadRequestException"));
    }

    // No refused update changed the destination it addressed.
    let after = destination(&ses, set, "held").await;
    assert_eq!(
        after, before,
        "a refused update must change nothing: the destination was {before:?}, then {after:?}"
    );
}
