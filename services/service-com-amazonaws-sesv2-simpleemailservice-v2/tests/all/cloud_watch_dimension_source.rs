// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a CloudWatch dimension source it does not recognise, end to
//! end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in [`crate::configuration_sets`], so
//! routing, the constraint walk, the handler and serialization are all exercised.
//!
//! The rule these cases pin: a `CloudWatchDimensionConfiguration$DimensionValueSource`
//! outside its modelled value set is refused `BadRequestException` (HTTP 400) with the
//! service's OWN sentence about that member — fixed text naming neither the offending
//! value nor a member path — rather than with a report of the constraint that failed, and
//! nothing is stored. The member's value set is `[MESSAGE_TAG, EMAIL_HEADER, LINK_TAG]`
//! (`smithy_query(query="shape",shape="DimensionValueSource")`); every value in it is
//! accepted.
//!
//! The tables below span what the rule generalises over: the offending value (the
//! wire-document spelling of each modelled value, a value no enum names, the empty value),
//! where in `DimensionConfigurations` the offending entry sits (alone, first of several,
//! last of several), how many entries carry one, and which operation carried the member
//! (the create of an event destination and the update of one).

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};

/// The sentence this service answers for a dimension source it does not recognise, as
/// AWS words it: fixed text with no variable part.
const INVALID_DIMENSION_SOURCE: &str = "invalid CloudWatch dimension source";

/// The configuration set every case attaches its destinations to.
const SET: &str = "events";

/// Every value `DimensionValueSource` binds
/// (`smithy_query(query="shape",shape="DimensionValueSource")`).
const DIMENSION_VALUE_SOURCES: [&str; 3] = ["MESSAGE_TAG", "EMAIL_HEADER", "LINK_TAG"];

/// The event-destination calls these cases drive, over the shared harness.
trait DimensionSourceSurface {
    async fn create_destination(&self, set: &str, name: &str, definition: Value) -> Reply;
    async fn update_destination(&self, set: &str, name: &str, definition: Value) -> Reply;
    async fn destinations(&self, set: &str) -> Vec<Value>;
}

impl DimensionSourceSurface for Ses {
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

    async fn destinations(&self, set: &str) -> Vec<Value> {
        self.call(
            "GET",
            &format!("/v2/email/configuration-sets/{set}/event-destinations"),
            None,
        )
        .await
        .member("EventDestinations")
        .as_array()
        .cloned()
        .unwrap_or_default()
    }
}

/// The configuration set every case attaches its destinations to, created through the
/// shared harness's own create.
async fn configuration_set(ses: &Ses) {
    let created = ses.create_set(json!({"ConfigurationSetName": SET})).await;
    assert_eq!(
        created.status, 200,
        "the configuration set must be created first, got {} {:?}",
        created.status,
        created.message()
    );
}

/// One `CloudWatchDimensionConfiguration`, with the dimension source the case supplies.
fn dimension(name: &str, source: &str) -> Value {
    json!({
        "DimensionName": name,
        "DimensionValueSource": source,
        "DefaultDimensionValue": "none",
    })
}

/// An `EventDestinationDefinition` publishing to CloudWatch with `dimensions`.
fn cloud_watch_definition(dimensions: Vec<Value>) -> Value {
    json!({
        "Enabled": true,
        "MatchingEventTypes": ["SEND"],
        "CloudWatchDestination": {"DimensionConfigurations": dimensions},
    })
}

#[tokio::test]
async fn a_dimension_source_outside_the_value_set_is_refused_with_the_services_own_sentence() {
    // The rows span the offending value: the wire-document spelling of each modelled
    // value ("to use the message tags ... choose messageTag ... choose emailHeader ...
    // choose linkTags" — `smithy_query(query="shape",shape="DimensionValueSource")`), a
    // value no enum names, a modelled value with its separator changed, and the empty
    // value.
    let values = [
        "messageTag",
        "emailHeader",
        "linkTags",
        "MESSAGE-TAG",
        "message_tag",
        "NOT_A_SOURCE",
        "",
    ];

    for value in values {
        // ... and the rows below span where the offending entry sits and how many
        // entries there are, so the refusal depends on neither.
        let placements: [(&str, Vec<Value>); 4] = [
            ("the only dimension", vec![dimension("campaign", value)]),
            (
                "the first of several dimensions",
                vec![
                    dimension("campaign", value),
                    dimension("cohort", "MESSAGE_TAG"),
                ],
            ),
            (
                "the last of several dimensions",
                vec![
                    dimension("campaign", "LINK_TAG"),
                    dimension("cohort", "EMAIL_HEADER"),
                    dimension("segment", value),
                ],
            ),
            (
                "every dimension",
                vec![dimension("campaign", value), dimension("cohort", value)],
            ),
        ];

        for (placement, dimensions) in placements {
            let case = format!("{value:?} as {placement}");
            let ses = Ses::new();
            configuration_set(&ses).await;
            // A destination the update can address, carrying a legal dimension source.
            assert_eq!(
                ses.create_destination(
                    SET,
                    "held",
                    cloud_watch_definition(vec![dimension("campaign", "MESSAGE_TAG")]),
                )
                .await
                .status,
                200,
                "case {case:?}: the addressed destination must be created first"
            );
            let before = ses.destinations(SET).await;

            for (operation, refused) in [
                (
                    "CreateConfigurationSetEventDestination",
                    ses.create_destination(
                        SET,
                        "new",
                        cloud_watch_definition(dimensions.clone()),
                    )
                    .await,
                ),
                (
                    "UpdateConfigurationSetEventDestination",
                    ses.update_destination(SET, "held", cloud_watch_definition(dimensions.clone()))
                        .await,
                ),
            ] {
                assert_eq!(
                    refused.status, 400,
                    "case {case:?} on {operation}: expected HTTP 400, got {} {:?}",
                    refused.status,
                    refused.message()
                );
                assert_eq!(
                    refused.error_code(),
                    "BadRequestException",
                    "case {case:?} on {operation}: expected BadRequestException, got {:?}",
                    refused.error_code()
                );
                assert_eq!(
                    refused.message(),
                    INVALID_DIMENSION_SOURCE,
                    "case {case:?} on {operation}: expected the service's own sentence, got \
                     {:?}",
                    refused.message()
                );
                // The sentence has no variable part: it echoes neither the value nor a
                // member path.
                assert!(
                    !refused.message().contains("dimensionConfigurations"),
                    "case {case:?} on {operation}: the sentence must not name a member path, \
                     got {:?}",
                    refused.message()
                );
            }

            // Neither refused call stored or changed a destination.
            assert_eq!(
                ses.destinations(SET).await,
                before,
                "case {case:?}: a refused call must change nothing, got {:?}",
                ses.destinations(SET).await
            );
        }
    }
}

#[tokio::test]
async fn every_modelled_dimension_source_is_accepted_and_read_back() {
    // The other side of the rule: each value the member's enum binds is stored and
    // reported, so the refusal is about the value set and not about the member.
    let ses = Ses::new();
    configuration_set(&ses).await;

    for source in DIMENSION_VALUE_SOURCES {
        let created = ses
            .create_destination(
                SET,
                source,
                cloud_watch_definition(vec![dimension("campaign", source)]),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {source:?}: expected HTTP 200 from the create, got {} {:?}",
            created.status,
            created.message()
        );

        let stored = ses
            .destinations(SET)
            .await
            .into_iter()
            .find(|destination| {
                destination.pointer("/Name").and_then(Value::as_str) == Some(source)
            })
            .unwrap_or(Value::Null);
        assert_eq!(
            stored
                .pointer("/CloudWatchDestination/DimensionConfigurations/0/DimensionValueSource")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            source,
            "case {source:?}: expected the stored destination to report the source it was \
             created with, got {stored:?}"
        );
    }
}
