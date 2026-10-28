// SPDX-License-Identifier: CC0-1.0

//! The event destinations a configuration set publishes its sending events to,
//! end to end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them.
//!
//! What is asserted is the RULE the create follows over a SET of inputs:
//!
//! * a create records the ONE destination the request provides, under the name it
//!   gives it, and the next `DescribeConfigurationSet` reports exactly that
//!   destination — for every destination class, every form of the members each
//!   class carries, and every event type the model names;
//! * the name is the destination's key within the set, so a second create of one
//!   name is a conflict and the stored destination stands;
//! * a create naming a configuration set the account does not hold is refused,
//!   whatever the name;
//! * a destination that describes nothing Amazon SES could publish to is refused
//!   with that destination class's own error — an ARN naming no resource of this
//!   account and Region, a dimension outside its documented form — before the set
//!   is looked up at all, and changes nothing;
//! * a request must provide one, and only one, destination, and a name outside
//!   the documented form is refused;
//! * a set takes no more destinations than Amazon SES allows one.

use crate::harness::{
    account, call, create_event_destination, described_event_destinations, element, elements,
    emulator, field, topic_arn, xml_text, Destination, REGION,
};
use smithy_server_runtime::MultiServiceRouter;

/// `CreateConfigurationSet`.
async fn create_configuration_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await
}

/// A set the account holds, so a case about the DESTINATION starts from one.
async fn given_configuration_set(router: &MultiServiceRouter, name: &str) {
    let (status, body) = create_configuration_set(router, name).await;
    assert_eq!(
        status, 200,
        "case create set {name}: expected 200, got {status} with {body}"
    );
}

/// The `(Code, Message)` of a refusal, with the XML entities the codec escapes a
/// message's text with read back as that text.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code").map(|code| xml_text(&code)),
        element(body, "Message").map(|message| xml_text(&message)),
    )
}

/// The error code alone, for a case whose message carries the value it is about.
fn code(body: &str) -> Option<String> {
    element(body, "Code").map(|text| xml_text(&text))
}

#[tokio::test]
async fn a_create_records_the_destination_every_accepted_form_describes() {
    let router = emulator();
    let topic = topic_arn("events-topic");
    let account = account();
    let stream = format!("arn:aws:firehose:{REGION}:{account}:deliverystream/ses-events");
    let role = format!("arn:aws:iam::{account}:role/ses-events");
    let role_under_a_path = format!("arn:aws:iam::{account}:role/service-role/ses-events");
    // The longest name the rule admits, bound so the table below can borrow it.
    let longest_name = "d".repeat(64);

    // One row per form the rule admits: each destination class, the members each
    // class carries in every documented form, the `Enabled` switch both ways,
    // and the event types from one to every one the model names.
    let cases: Vec<(&str, bool, Vec<&str>, Destination)> = vec![
        (
            "sns-dest",
            true,
            vec!["send"],
            Destination::Sns(topic.clone()),
        ),
        (
            "sns_dest_2",
            false,
            vec![
                "send",
                "reject",
                "bounce",
                "complaint",
                "delivery",
                "open",
                "click",
                "renderingFailure",
            ],
            Destination::Sns(topic.clone()),
        ),
        (
            "firehose-dest",
            true,
            vec!["bounce", "complaint"],
            Destination::Firehose(role.clone(), stream.clone()),
        ),
        (
            "firehose-dest-pathed-role",
            false,
            vec!["delivery"],
            Destination::Firehose(role_under_a_path.clone(), stream.clone()),
        ),
        (
            "cw-dest",
            true,
            vec!["open", "click"],
            // The three sources `DimensionValueSource` names, a dimension name
            // carrying the colon its rule admits, and a default value carrying
            // the at sign and the period its own rule admits.
            Destination::CloudWatch(vec![
                (
                    "campaign".to_string(),
                    "messageTag".to_string(),
                    "none".to_string(),
                ),
                (
                    "ses:configuration-set".to_string(),
                    "emailHeader".to_string(),
                    "ops@example.com".to_string(),
                ),
                (
                    "link_tag-9".to_string(),
                    "linkTag".to_string(),
                    "v1.0".to_string(),
                ),
            ]),
        ),
        (
            &longest_name,
            true,
            vec!["send"],
            Destination::CloudWatch(vec![(
                "a".repeat(256),
                "messageTag".to_string(),
                "b".repeat(256),
            )]),
        ),
    ];

    for (index, (name, enabled, event_types, destination)) in cases.iter().enumerate() {
        // One set per row, so each row is read back on its own.
        let set = format!("set-{index}");
        given_configuration_set(&router, &set).await;
        let (status, body) =
            create_event_destination(&router, &set, name, *enabled, event_types, destination).await;
        assert_eq!(
            status, 200,
            "case {name} on {set}: expected 200, got {status} with {body}"
        );

        let reported = described_event_destinations(&router, &set).await;
        assert_eq!(
            element(&reported, "Name").as_deref(),
            Some(*name),
            "case {name} on {set}: expected the stored name, got {reported}"
        );
        assert_eq!(
            element(&reported, "Enabled").as_deref() == Some("true"),
            *enabled,
            "case {name} on {set}: expected Enabled {enabled}, got {reported}"
        );
        let reported_types = element(&reported, "MatchingEventTypes")
            .map(|types| elements(&types, "member"))
            .unwrap_or_default();
        assert_eq!(
            reported_types, *event_types,
            "case {name} on {set}: expected the matched event types {event_types:?}, \
             got {reported_types:?}"
        );
        match destination {
            Destination::Sns(topic_arn) => assert_eq!(
                element(&reported, "TopicARN").as_deref(),
                Some(topic_arn.as_str()),
                "case {name} on {set}: expected the stored topic, got {reported}"
            ),
            Destination::Firehose(role_arn, stream_arn) => assert_eq!(
                (
                    element(&reported, "IAMRoleARN"),
                    element(&reported, "DeliveryStreamARN")
                ),
                (Some(role_arn.clone()), Some(stream_arn.clone())),
                "case {name} on {set}: expected the stored stream and role, got {reported}"
            ),
            Destination::CloudWatch(dimensions) => {
                let reported_dimensions: Vec<(String, String, String)> =
                    elements(&reported, "DimensionName")
                        .into_iter()
                        .zip(elements(&reported, "DimensionValueSource"))
                        .zip(elements(&reported, "DefaultDimensionValue"))
                        .map(|((name, source), default)| (name, source, default))
                        .collect();
                assert_eq!(
                    reported_dimensions,
                    dimensions
                        .iter()
                        .map(|(name, source, default)| (
                            name.clone(),
                            source.clone(),
                            xml_text(default)
                        ))
                        .collect::<Vec<_>>(),
                    "case {name} on {set}: expected the stored dimensions, got {reported}"
                );
            }
            other => panic!("case {name}: {other:?} is not an accepted destination"),
        }
    }

    // An ARN names its resource by its trimmed value, which is what the create
    // reads and asks Amazon SNS about — so that is also what the set stores and
    // a reader reports, for every ARN member of either class.
    given_configuration_set(&router, "set-padded").await;
    let (status, body) = create_event_destination(
        &router,
        "set-padded",
        "padded-sns",
        true,
        &["send"],
        &Destination::Sns(format!("  {topic}  ")),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a padded topic ARN: expected 200, got {status} with {body}"
    );
    let (status, body) = create_event_destination(
        &router,
        "set-padded",
        "padded-firehose",
        true,
        &["send"],
        &Destination::Firehose(format!(" {role} "), format!("\t{stream}\n")),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a padded stream and role ARN: expected 200, got {status} with {body}"
    );
    let reported = described_event_destinations(&router, "set-padded").await;
    assert_eq!(
        (
            element(&reported, "TopicARN"),
            element(&reported, "IAMRoleARN"),
            element(&reported, "DeliveryStreamARN")
        ),
        (
            Some(topic.clone()),
            Some(role.clone()),
            Some(stream.clone())
        ),
        "case padded ARNs: expected each ARN stored as the resource it names, got {reported}"
    );
}

#[tokio::test]
async fn a_second_create_of_one_name_is_a_conflict() {
    let router = emulator();
    let topic = topic_arn("events-topic");
    // The names span the rule: short, mixed case with both separators, and the
    // longest a name may be.
    for name in ["dest", "Dest_2-x", &"n".repeat(64)] {
        let set = format!("set-for-{}", name.len());
        given_configuration_set(&router, &set).await;
        let (status, body) = create_event_destination(
            &router,
            &set,
            name,
            true,
            &["send"],
            &Destination::Sns(topic.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case first create {name}: expected 200, got {status} with {body}"
        );

        // A second create of the same name conflicts, whatever it would store.
        let (status, body) = create_event_destination(
            &router,
            &set,
            name,
            false,
            &["bounce"],
            &Destination::CloudWatch(vec![(
                "campaign".to_string(),
                "messageTag".to_string(),
                "none".to_string(),
            )]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case second create {name}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("EventDestinationAlreadyExists".to_string()),
                Some(format!(
                    "An event destination with name <{name}> already exists for configuration \
                     set <{set}>."
                ))
            ),
            "case second create {name}: expected the already-exists refusal, got {body}"
        );

        // The destination the first create stored stands, and there is still
        // only the one.
        let reported = described_event_destinations(&router, &set).await;
        assert_eq!(
            (
                elements(&reported, "Name"),
                element(&reported, "Enabled").as_deref() == Some("true"),
                element(&reported, "TopicARN")
            ),
            (vec![name.to_string()], true, Some(topic.clone())),
            "case second create {name}: the first destination stands, got {reported}"
        );
    }
}

#[tokio::test]
async fn a_create_naming_a_set_the_account_does_not_hold_is_refused() {
    let router = emulator();
    let topic = topic_arn("events-topic");
    // The names again span everything the set-naming rule admits, and none of
    // these sets was ever created.
    let cases = [
        "absent-set".to_string(),
        "Absent_Set_2".to_string(),
        "z".to_string(),
        "a".repeat(64),
    ];
    for set in cases {
        let (status, body) = create_event_destination(
            &router,
            &set,
            "dest",
            true,
            &["send"],
            &Destination::Sns(topic.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case create on absent {set:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            refusal(&body),
            (
                Some("ConfigurationSetDoesNotExist".to_string()),
                Some(format!("Configuration set <{set}> does not exist."))
            ),
            "case create on absent {set:?}: expected the not-found refusal, got {body}"
        );
    }
}

#[tokio::test]
async fn a_destination_ses_could_not_publish_to_is_refused() {
    let router = emulator();
    let topic = topic_arn("events-topic");
    given_configuration_set(&router, "set").await;
    let account = account();
    let other_account = "210987654321";
    let stream = format!("arn:aws:firehose:{REGION}:{account}:deliverystream/ses-events");
    let role = format!("arn:aws:iam::{account}:role/ses-events");

    // (what is wrong with it, the destination, the error code it is refused with)
    let cases: Vec<(&str, Destination, &str)> = vec![
        // A topic ARN that names no topic this caller could publish to: another
        // account's, another Region's, another partition's, another vendor's
        // resource, a subscription rather than a topic, a topic name outside the
        // form Amazon SNS mints, and text that is no ARN at all.
        (
            "a topic of another account",
            Destination::Sns(format!("arn:aws:sns:{REGION}:{other_account}:events-topic")),
            "InvalidSNSDestination",
        ),
        (
            "a topic of another Region",
            Destination::Sns(format!("arn:aws:sns:eu-west-1:{account}:events-topic")),
            "InvalidSNSDestination",
        ),
        (
            "a topic of another partition",
            Destination::Sns(format!("arn:aws-cn:sns:{REGION}:{account}:events-topic")),
            "InvalidSNSDestination",
        ),
        (
            "another vendor's resource",
            Destination::Sns(format!("arn:aws:sqs:{REGION}:{account}:events-topic")),
            "InvalidSNSDestination",
        ),
        (
            "a subscription rather than a topic",
            Destination::Sns(format!(
                "arn:aws:sns:{REGION}:{account}:events-topic:8c2f-4f1b"
            )),
            "InvalidSNSDestination",
        ),
        (
            "a topic name outside the form Amazon SNS mints",
            Destination::Sns(format!("arn:aws:sns:{REGION}:{account}:events topic")),
            "InvalidSNSDestination",
        ),
        (
            "no ARN at all",
            Destination::Sns("events-topic".to_string()),
            "InvalidSNSDestination",
        ),
        (
            "an empty topic ARN",
            Destination::Sns(String::new()),
            "InvalidSNSDestination",
        ),
        // A Firehose destination naming a stream or a role this caller could not
        // publish through.
        (
            "a delivery stream of another account",
            Destination::Firehose(
                role.clone(),
                format!("arn:aws:firehose:{REGION}:{other_account}:deliverystream/ses-events"),
            ),
            "InvalidFirehoseDestination",
        ),
        (
            "a delivery stream of another Region",
            Destination::Firehose(
                role.clone(),
                format!("arn:aws:firehose:eu-west-1:{account}:deliverystream/ses-events"),
            ),
            "InvalidFirehoseDestination",
        ),
        (
            "another resource of Firehose",
            Destination::Firehose(
                role.clone(),
                format!("arn:aws:firehose:{REGION}:{account}:deliverystream-alias/ses-events"),
            ),
            "InvalidFirehoseDestination",
        ),
        (
            "a role of another account",
            Destination::Firehose(
                format!("arn:aws:iam::{other_account}:role/ses-events"),
                stream.clone(),
            ),
            "InvalidFirehoseDestination",
        ),
        (
            "an IAM user rather than a role",
            Destination::Firehose(format!("arn:aws:iam::{account}:user/ops"), stream.clone()),
            "InvalidFirehoseDestination",
        ),
        (
            "no role ARN at all",
            Destination::Firehose("ses-events".to_string(), stream.clone()),
            "InvalidFirehoseDestination",
        ),
        // A CloudWatch destination that categorizes nothing, or names a
        // dimension outside its documented form. On the awsQuery wire an empty
        // dimension list carries no field at all, so such a request provides no
        // destination — which the one-and-only-one rule is what refuses.
        (
            "no dimension configuration at all",
            Destination::CloudWatch(Vec::new()),
            "InvalidParameterValue",
        ),
        (
            "a dimension name carrying a character its rule excludes",
            Destination::CloudWatch(vec![(
                "camp aign".to_string(),
                "messageTag".to_string(),
                "none".to_string(),
            )]),
            "InvalidCloudWatchDestination",
        ),
        (
            "a dimension name past its documented length",
            Destination::CloudWatch(vec![(
                "a".repeat(257),
                "messageTag".to_string(),
                "none".to_string(),
            )]),
            "InvalidCloudWatchDestination",
        ),
        (
            "an empty dimension name",
            Destination::CloudWatch(vec![(
                String::new(),
                "messageTag".to_string(),
                "none".to_string(),
            )]),
            "InvalidCloudWatchDestination",
        ),
        (
            "a default dimension value carrying a character its rule excludes",
            Destination::CloudWatch(vec![(
                "campaign".to_string(),
                "messageTag".to_string(),
                "no ne".to_string(),
            )]),
            "InvalidCloudWatchDestination",
        ),
        (
            "a default dimension value past its documented length",
            Destination::CloudWatch(vec![(
                "campaign".to_string(),
                "messageTag".to_string(),
                "b".repeat(257),
            )]),
            "InvalidCloudWatchDestination",
        ),
        (
            "a second dimension outside the form, after one inside it",
            Destination::CloudWatch(vec![
                (
                    "campaign".to_string(),
                    "messageTag".to_string(),
                    "none".to_string(),
                ),
                (
                    "campaign!".to_string(),
                    "messageTag".to_string(),
                    "none".to_string(),
                ),
            ]),
            "InvalidCloudWatchDestination",
        ),
    ];

    for (case, destination, expected) in cases {
        let (status, body) =
            create_event_destination(&router, "set", "dest", true, &["send"], &destination).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            code(&body).as_deref(),
            Some(expected),
            "case {case}: expected the {expected} refusal, got {body}"
        );
        // The refusal is read off the request, so it is the same for a set the
        // account does not hold — the destination is refused before the set is
        // looked up at all.
        let (status, absent_body) = create_event_destination(
            &router,
            "absent-set",
            "dest",
            true,
            &["send"],
            &destination,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case} on an absent set: expected 400, got {status} with {absent_body}"
        );
        assert_eq!(
            code(&absent_body).as_deref(),
            Some(expected),
            "case {case} on an absent set: expected the {expected} refusal, got {absent_body}"
        );
    }

    // Nothing any of those refusals describes was stored, and a destination that
    // IS one Amazon SES could publish to is still admitted afterwards.
    let reported = described_event_destinations(&router, "set").await;
    assert_eq!(
        elements(&reported, "Name"),
        Vec::<String>::new(),
        "a refused create stores nothing, got {reported}"
    );
    let (status, body) = create_event_destination(
        &router,
        "set",
        "dest",
        true,
        &["send"],
        &Destination::Sns(topic.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create after the refusals: expected 200, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_topic_arn_is_admitted_exactly_when_it_names_a_topic_of_this_account() {
    let router = emulator();
    given_configuration_set(&router, "set").await;
    // The ARN's resource field is the topic NAME alone, and Amazon SNS documents
    // the names it mints: "made up of only uppercase and lowercase ASCII
    // letters, numbers, underscores, and hyphens", "between 1 and 256 characters
    // long" — so 256 characters is a topic and 257 is not.
    let admitted = [
        "events-topic".to_string(),
        "Events_Topic_2".to_string(),
        "t".to_string(),
        "t".repeat(256),
    ];
    for (index, name) in admitted.iter().enumerate() {
        let destination_name = format!("dest-{index}");
        let (status, body) = create_event_destination(
            &router,
            "set",
            &destination_name,
            true,
            &["send"],
            &Destination::Sns(topic_arn(name)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case topic name {}: a topic of this account is admitted, got {status} with {body}",
            name.len()
        );
    }
    let reported = described_event_destinations(&router, "set").await;
    assert_eq!(
        elements(&reported, "TopicARN"),
        admitted
            .iter()
            .map(|name| topic_arn(name))
            .collect::<Vec<_>>(),
        "every admitted topic is stored as the ARN it was given, got {reported}"
    );

    // And a resource field that is no topic name names no topic, however
    // well-formed the rest of the ARN is.
    let refused = [
        "t".repeat(257),
        "events.topic".to_string(),
        "events topic".to_string(),
        String::new(),
    ];
    for name in refused {
        let (status, body) = create_event_destination(
            &router,
            "set",
            "refused-dest",
            true,
            &["send"],
            &Destination::Sns(topic_arn(&name)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case topic name {name:?}: a value that is no topic name is refused, got {status} \
             with {body}"
        );
        assert_eq!(
            code(&body).as_deref(),
            Some("InvalidSNSDestination"),
            "case topic name {name:?}: expected the invalid-SNS-destination refusal, got {body}"
        );
    }
    let reported = described_event_destinations(&router, "set").await;
    assert_eq!(
        elements(&reported, "Name").len(),
        admitted.len(),
        "a refused topic stores nothing, got {reported}"
    );
}

#[tokio::test]
async fn a_request_must_name_one_destination_and_a_name_of_the_documented_form() {
    // No Amazon SNS mounted: nothing here is about a topic's existence, and a
    // request refused on its own members never asks about one.
    let router = emulator();
    given_configuration_set(&router, "set").await;
    let account = account();
    let topic_arn = format!("arn:aws:sns:{REGION}:{account}:events-topic");

    // "You must provide one, and only one, destination": none and more than one
    // are both refused, with the parameter violation neither modelled
    // destination error covers.
    for (case, destination) in [
        ("no destination at all", Destination::None),
        (
            "two destinations",
            Destination::SnsAndCloudWatch(topic_arn.clone()),
        ),
    ] {
        let (status, body) =
            create_event_destination(&router, "set", "dest", true, &["send"], &destination).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            code(&body).as_deref(),
            Some("InvalidParameterValue"),
            "case {case}: expected the parameter refusal, got {body}"
        );
    }

    // `EventDestination$Name`: "Contain only ASCII letters (a-z, A-Z), numbers
    // (0-9), underscores (_), or dashes (-)" and "Contain 64 characters or
    // fewer" — so 64 characters is accepted and 65 is not.
    let accepted = ["d", "Dest_1-2", "9", &"a".repeat(64)];
    for name in accepted {
        let (status, body) = create_event_destination(
            &router,
            "set",
            name,
            true,
            &["send"],
            &Destination::Sns(topic_arn.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case create {name:?}: a name meeting the rule is accepted, got {status} with {body}"
        );
    }
    let reported = described_event_destinations(&router, "set").await;
    assert_eq!(
        elements(&reported, "Name"),
        accepted
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>(),
        "every accepted name is stored, in the order it was added, got {reported}"
    );

    let refused = ["has space", "dot.name", "name!", &"a".repeat(65), ""];
    for name in refused {
        let (status, body) = create_event_destination(
            &router,
            "set",
            name,
            true,
            &["send"],
            &Destination::Sns(topic_arn.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case create {name:?}: a name outside the rule is refused, got {status} with {body}"
        );
        assert_eq!(
            code(&body).as_deref(),
            Some("InvalidParameterValue"),
            "case create {name:?}: expected the parameter refusal, got {body}"
        );
    }
    // And none of the refused names was stored.
    let reported = described_event_destinations(&router, "set").await;
    assert_eq!(
        elements(&reported, "Name").len(),
        accepted.len(),
        "a refused name stores nothing, got {reported}"
    );
}

#[tokio::test]
async fn a_set_takes_no_more_destinations_than_amazon_ses_allows_one() {
    let router = emulator();
    given_configuration_set(&router, "set").await;
    // Up to the ceiling the store applies, each destination added under its own
    // name.
    let ceiling = service_com_amazonaws_ses_simpleemailservice::state::MAX_EVENT_DESTINATIONS;
    for index in 0..ceiling {
        let name = format!("dest-{index}");
        let (status, body) = create_event_destination(
            &router,
            "set",
            &name,
            true,
            &["send"],
            &Destination::CloudWatch(vec![(
                "campaign".to_string(),
                "messageTag".to_string(),
                "none".to_string(),
            )]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case create {name}: expected 200, got {status} with {body}"
        );
    }
    let reported = described_event_destinations(&router, "set").await;
    assert_eq!(
        elements(&reported, "Name").len(),
        ceiling,
        "case at the ceiling: expected {ceiling} stored destinations, got {reported}"
    );

    // One past it is refused, and the set keeps exactly what it held.
    let (status, body) = create_event_destination(
        &router,
        "set",
        "one-too-many",
        true,
        &["send"],
        &Destination::CloudWatch(vec![(
            "campaign".to_string(),
            "messageTag".to_string(),
            "none".to_string(),
        )]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the ceiling: expected 400, got {status} with {body}"
    );
    assert_eq!(
        refusal(&body),
        (
            Some("LimitExceeded".to_string()),
            Some(format!(
                "Configuration set <set> may not have more than {ceiling} event destinations."
            ))
        ),
        "case one past the ceiling: expected the limit refusal, got {body}"
    );
    let reported = described_event_destinations(&router, "set").await;
    assert_eq!(
        elements(&reported, "Name").len(),
        ceiling,
        "case one past the ceiling: the stored destinations are unchanged, got {reported}"
    );

    // A set of its own is unaffected by another set's fullness.
    given_configuration_set(&router, "other-set").await;
    let (status, body) = create_event_destination(
        &router,
        "other-set",
        "dest",
        true,
        &["send"],
        &Destination::CloudWatch(vec![(
            "campaign".to_string(),
            "messageTag".to_string(),
            "none".to_string(),
        )]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create on another set: expected 200, got {status} with {body}"
    );
}
