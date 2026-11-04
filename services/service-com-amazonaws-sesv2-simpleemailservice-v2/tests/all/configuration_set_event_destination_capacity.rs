// SPDX-License-Identifier: CC0-1.0

//! What ONE event destination of a configuration set carries, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised, and reads the stored destination back through
//! `GetConfigurationSetEventDestinations`.
//!
//! The rule these cases pin: a list member of the `EventDestinationDefinition` a create
//! carries is persisted EXACTLY as the request spelled it — the same values in the same
//! order, however many there are — because the model declares those members as plain
//! lists, with no uniqueness, ordering or length constraint for the service to normalise
//! against; and repeated reads report the same thing, since a read reports and does not
//! change. The sibling suites pin one destination object per definition with a canonical
//! one-element list; the capacity of the lists INSIDE a definition is the leg they leave
//! untested.
//!
//! The rows span what the rule generalises over: how many event types a definition matches
//! (none, one, several, every modelled value), the ORDER they arrive in (the model's own
//! and its reverse), how many CloudWatch dimension configurations one destination carries
//! (one, three, four, ten), and which `DimensionValueSource` each of them names (every
//! value of the enum, not just the one the sibling table uses).

use serde_json::{json, Value};

use crate::configuration_set_event_destinations::{
    create_set, reported_destination, EventDestinations, EVENT_TYPES,
};
use crate::configuration_sets::Ses;

/// Every value of the `DimensionValueSource` enum, in the order the model lists them
/// (`smithy_query(query="shape",shape="DimensionValueSource")`): the locations SES finds a
/// published dimension's value in.
const DIMENSION_VALUE_SOURCES: &[&str] = &["MESSAGE_TAG", "EMAIL_HEADER", "LINK_TAG"];

/// One dimension configuration, with `source` as its `DimensionValueSource`.
///
/// `CloudWatchDimensionConfiguration` requires all three members
/// (`smithy_query(query="shape",shape="CloudWatchDimensionConfiguration")`), so each row
/// carries all three.
fn dimension(index: usize, source: &str) -> Value {
    json!({
        "DimensionName": format!("dimension-{index}"),
        "DimensionValueSource": source,
        "DefaultDimensionValue": format!("default-{index}"),
    })
}

/// One row of the table: the event types the definition matches and the dimension
/// configurations its CloudWatch destination carries. The dimension configurations are
/// reported unchanged; the event types are reported in the order SES reports a
/// destination's types, ascending by name, whatever order the definition listed them in.
struct ListCase {
    case: &'static str,
    matching_event_types: Vec<String>,
    /// The event types a read must report: the ones sent, ascending by name. Written out
    /// per row rather than derived from the row's own list, so the expectation does not
    /// come from the behaviour under test.
    reported_event_types: Vec<String>,
    dimensions: Vec<Value>,
}

/// `event_types` as a list of owned strings, for a row that writes its order out.
fn types(event_types: &[&str]) -> Vec<String> {
    event_types.iter().map(|value| value.to_string()).collect()
}

/// The rows of the table: the counts and orders a definition's lists can arrive in.
fn list_cases() -> Vec<ListCase> {
    let every_type: Vec<String> = EVENT_TYPES.iter().map(|value| value.to_string()).collect();
    let mut reversed = every_type.clone();
    reversed.reverse();
    // One dimension configuration per modelled value source, so every value of the enum is
    // stored and reported, and in an order that is not the model's own.
    let one_per_source: Vec<Value> = DIMENSION_VALUE_SOURCES
        .iter()
        .rev()
        .enumerate()
        .map(|(index, source)| dimension(index, source))
        .collect();
    vec![
        ListCase {
            case: "no event type and one dimension",
            matching_event_types: Vec::new(),
            reported_event_types: Vec::new(),
            dimensions: vec![dimension(0, DIMENSION_VALUE_SOURCES[0])],
        },
        ListCase {
            case: "one event type and one dimension per modelled value source",
            matching_event_types: types(&["SEND"]),
            reported_event_types: types(&["SEND"]),
            dimensions: one_per_source,
        },
        ListCase {
            case: "several event types out of model order, several dimensions",
            matching_event_types: types(&["CLICK", "BOUNCE", "DELIVERY_DELAY"]),
            reported_event_types: types(&["BOUNCE", "CLICK", "DELIVERY_DELAY"]),
            dimensions: (0..4)
                .map(|index| {
                    dimension(
                        index,
                        DIMENSION_VALUE_SOURCES[index % DIMENSION_VALUE_SOURCES.len()],
                    )
                })
                .collect(),
        },
        ListCase {
            // The sibling suite's table already pins every event type in the model's own
            // order, so this row carries them reversed: what the rule is about is that
            // the order a read reports is SES's own, whatever order was sent.
            case: "every modelled event type in reverse order",
            matching_event_types: reversed,
            reported_event_types: types(&[
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
            ]),
            dimensions: (0..10)
                .map(|index| {
                    dimension(
                        index,
                        DIMENSION_VALUE_SOURCES[index % DIMENSION_VALUE_SOURCES.len()],
                    )
                })
                .collect(),
        },
    ]
}

#[tokio::test]
async fn a_definitions_lists_are_stored_whole_and_reported_as_ses_reports_them() {
    for case in list_cases() {
        let ses = Ses::new();
        let set = "lists-under-test";
        create_set(&ses, set).await;
        let name = "every-list";

        let created = ses
            .create_destination(
                set,
                name,
                json!({
                    "Enabled": true,
                    "MatchingEventTypes": case.matching_event_types,
                    "CloudWatchDestination": {"DimensionConfigurations": case.dimensions},
                }),
            )
            .await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: expected 200, got {} {:?} ({})",
            case.case,
            created.status,
            created.error_code(),
            created.message()
        );

        let stored = reported_destination(&ses, set, name)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the created destination {name:?} is not reported at all",
                    case.case
                )
            });
        // `EventDestinationDefinition$MatchingEventTypes` is optional, so a serializer that
        // omits an empty list and one that writes it out agree: absent reads as empty, and
        // what the non-empty rows pin is the values and their order.
        let reported_event_types = stored
            .get("MatchingEventTypes")
            .cloned()
            .unwrap_or_else(|| json!([]));
        assert_eq!(
            reported_event_types,
            json!(case.reported_event_types),
            "case {:?}: expected MatchingEventTypes {:?}, got {stored:?}",
            case.case,
            case.reported_event_types
        );
        assert_eq!(
            stored
                .pointer("/CloudWatchDestination/DimensionConfigurations")
                .cloned(),
            Some(Value::Array(case.dimensions.clone())),
            "case {:?}: expected the {} dimension configurations sent, in that order, got \
             {stored:?}",
            case.case,
            case.dimensions.len()
        );

        // A read reports, it does not change: the second read answers the first one.
        let again = reported_destination(&ses, set, name).await;
        assert_eq!(
            again.as_ref(),
            Some(&stored),
            "case {:?}: a second read reported {again:?}, not the first read's {stored:?}",
            case.case
        );
    }
}
