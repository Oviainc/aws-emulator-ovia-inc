// SPDX-License-Identifier: CC0-1.0

//! How many event destinations one configuration set holds, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: `CreateConfigurationSetEventDestination` is served from the
//! store — it persists the definition the request carried and answers the empty 200 its
//! output shape declares — and a request it cannot serve is refused with the modelled
//! error for its reason, never with a status or an error code this service's model does
//! not declare. The leg the sibling suites leave untested is the ceiling on how many
//! destinations ONE set holds: because the count is taken from what that set actually
//! holds, a create is refused `LimitExceededException` only PAST the ceiling, the ceiling
//! is per configuration set rather than per account, a refused create stores nothing, and
//! a delete frees a slot the next create takes.
//!
//! The rows span what the rule generalises over: which destination object each definition
//! carries (each of the five the model declares), whether it is enabled, how many event
//! types it matches, where the create sits relative to the ceiling (one below it, at it,
//! one past it), which set the create addresses, and whether a slot was freed first. The
//! ceiling itself is the store's own [`MAX_EVENT_DESTINATIONS_PER_SET`], so the cases
//! follow the bound rather than a literal.

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::MAX_EVENT_DESTINATIONS_PER_SET;

use crate::configuration_set_event_destinations::{create_set, reported_names, EventDestinations};
use crate::configuration_sets::Ses;

/// The destination object a definition carries, as one member name and its value.
///
/// Cycles the five objects `EventDestinationDefinition` declares and the members each one
/// requires (`smithy_query(query="shape",shape="EventDestinationDefinition,SnsDestination,
/// EventBridgeDestination,CloudWatchDestination,CloudWatchDimensionConfiguration,
/// KinesisFirehoseDestination,PinpointDestination")`), so a set filled to its ceiling
/// holds every kind of destination rather than one kind repeated.
fn destination_object(index: usize) -> (&'static str, Value) {
    match index % 5 {
        0 => (
            "SnsDestination",
            json!({"TopicArn": format!("arn:aws:sns:us-east-1:111122223333:ses-events-{index}")}),
        ),
        1 => (
            "EventBridgeDestination",
            json!({"EventBusArn": "arn:aws:events:us-east-1:111122223333:event-bus/default"}),
        ),
        2 => (
            "CloudWatchDestination",
            json!({"DimensionConfigurations": [{
                "DimensionName": "ses-configuration-set",
                "DimensionValueSource": "MESSAGE_TAG",
                "DefaultDimensionValue": format!("tag-{index}"),
            }]}),
        ),
        3 => (
            "KinesisFirehoseDestination",
            json!({
                "IamRoleArn": "arn:aws:iam::111122223333:role/ses-events",
                "DeliveryStreamArn": format!(
                    "arn:aws:firehose:us-east-1:111122223333:deliverystream/ses-events-{index}"
                ),
            }),
        ),
        _ => (
            "PinpointDestination",
            json!({"ApplicationArn": "arn:aws:mobiletargeting:us-east-1:111122223333:apps/abc"}),
        ),
    }
}

/// One `EventDestinationDefinition` per index: a destination object, an enabled flag and a
/// matching-event-type list that differ from row to row, so no two definitions in a filled
/// set are the same request twice.
fn definition(index: usize) -> Value {
    // Values of the `EventType` enum
    // (`smithy_query(query="shape",shape="EventType")`), in lists of three different
    // lengths — none, one, several — because the count a definition matches is one of the
    // things the ceiling rule must not depend on.
    let matching_event_types: Vec<&str> = match index % 3 {
        0 => Vec::new(),
        1 => vec!["SEND"],
        _ => vec!["SEND", "DELIVERY", "RENDERING_FAILURE"],
    };
    let mut members = serde_json::Map::new();
    members.insert("Enabled".to_string(), json!(index.is_multiple_of(2)));
    members.insert("MatchingEventTypes".to_string(), json!(matching_event_types));
    let (member, object) = destination_object(index);
    members.insert(member.to_string(), object);
    Value::Object(members)
}

/// The name of the filler destination at `index`.
fn filler_name(index: usize) -> String {
    format!("destination-{index}")
}

/// Attach `count` destinations to `set`, which must already exist, and assert every one of
/// them was accepted: the ceiling is a property of what the set HOLDS, so a case about it
/// has to put that many there through the wire path.
async fn fill(ses: &Ses, set: &str, count: usize) {
    for index in 0..count {
        let name = filler_name(index);
        let reply = ses.create_destination(set, &name, definition(index)).await;
        assert_eq!(
            reply.status,
            200,
            "case filling {set:?} to {count}: destination {name:?} is number {} of the \
             ceiling of {MAX_EVENT_DESTINATIONS_PER_SET}, so it must be accepted; got {} \
             {:?} ({})",
            index + 1,
            reply.status,
            reply.error_code(),
            reply.message()
        );
    }
}

#[tokio::test]
async fn a_create_is_refused_only_past_the_sets_ceiling() {
    let ses = Ses::new();
    let set = "at-the-ceiling";
    create_set(&ses, set).await;

    // One below the ceiling: every create so far has a slot of its own.
    fill(&ses, set, MAX_EVENT_DESTINATIONS_PER_SET - 1).await;

    // At the ceiling: the last slot is still a slot, so this create is served.
    let at_the_bound = "destination-at-the-bound";
    let accepted = ses
        .create_destination(set, at_the_bound, definition(MAX_EVENT_DESTINATIONS_PER_SET - 1))
        .await;
    assert_eq!(
        accepted.status,
        200,
        "case at the ceiling: the {MAX_EVENT_DESTINATIONS_PER_SET}th destination of a set \
         is inside the bound, so it must be accepted; got {} {:?} ({})",
        accepted.status,
        accepted.error_code(),
        accepted.message()
    );
    assert!(
        accepted.is_empty_body(),
        "case at the ceiling: CreateConfigurationSetEventDestinationResponse declares no \
         members, so the body must be empty; got {:?}",
        accepted.member("EventDestinations")
    );

    // One past the ceiling: the modelled refusal for too many of a resource, on its
    // modelled status.
    let past_the_bound = "destination-past-the-bound";
    let refused = ses
        .create_destination(set, past_the_bound, definition(0))
        .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "LimitExceededException"),
        "case one past the ceiling: expected 400 LimitExceededException at \
         {MAX_EVENT_DESTINATIONS_PER_SET} destinations, got {} {:?} ({})",
        refused.status,
        refused.error_code(),
        refused.message()
    );
    let message = refused.message();
    assert!(
        message.contains(set) && message.contains(&MAX_EVENT_DESTINATIONS_PER_SET.to_string()),
        "case one past the ceiling: the refusal must name the set it is about and the bound \
         it met, both taken from the request and the store; got {message:?}"
    );

    // The refused create stored nothing, and the accepted ones are all still there.
    let names = reported_names(&ses, set).await;
    assert_eq!(
        names.len(),
        MAX_EVENT_DESTINATIONS_PER_SET,
        "case one past the ceiling: the set must still report exactly \
         {MAX_EVENT_DESTINATIONS_PER_SET} destinations, got {}",
        names.len()
    );
    assert!(
        !names.contains(&past_the_bound.to_string()),
        "case one past the ceiling: a refused create must store nothing, but the set reports \
         {past_the_bound:?}"
    );
    assert!(
        names.contains(&at_the_bound.to_string()),
        "case at the ceiling: the accepted create must be reported, but the set reports no \
         {at_the_bound:?}"
    );
}

#[tokio::test]
async fn the_ceiling_is_per_set_and_a_delete_frees_a_slot() {
    let ses = Ses::new();
    let full = "holds-its-ceiling";
    let other = "holds-none";
    create_set(&ses, full).await;
    create_set(&ses, other).await;
    fill(&ses, full, MAX_EVENT_DESTINATIONS_PER_SET).await;

    // The same name and the same definition, judged against two different sets: what is
    // refused on the full one is served on the one holding nothing, because the count comes
    // from the set the request addresses.
    let name = "one-more-destination";
    let refused = ses.create_destination(full, name, definition(1)).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "LimitExceededException"),
        "case the full set: expected 400 LimitExceededException, got {} {:?} ({})",
        refused.status,
        refused.error_code(),
        refused.message()
    );
    let accepted = ses.create_destination(other, name, definition(1)).await;
    assert_eq!(
        accepted.status,
        200,
        "case another set: a set holding no destination must accept the definition the full \
         set refused, got {} {:?} ({})",
        accepted.status,
        accepted.error_code(),
        accepted.message()
    );
    assert_eq!(
        reported_names(&ses, other).await,
        vec![name.to_string()],
        "case another set: {other:?} must report exactly the one destination it took"
    );

    // Freeing a slot is what lets the next create take one: the ceiling is measured
    // against what the set holds now, not against how many creates it has seen.
    let freed = filler_name(0);
    let deleted = ses.delete_destination(full, &freed).await;
    assert_eq!(
        deleted.status,
        200,
        "case freeing a slot: the delete of {freed:?} must be served, got {} {:?} ({})",
        deleted.status,
        deleted.error_code(),
        deleted.message()
    );
    let after_delete = ses.create_destination(full, name, definition(1)).await;
    assert_eq!(
        after_delete.status,
        200,
        "case freeing a slot: the create refused a moment ago must be served once the set \
         holds one destination fewer, got {} {:?} ({})",
        after_delete.status,
        after_delete.error_code(),
        after_delete.message()
    );

    // And the set is at its ceiling again, so the next create meets the same refusal.
    let refused_again = ses
        .create_destination(full, "yet-another-destination", definition(2))
        .await;
    assert_eq!(
        (refused_again.status, refused_again.error_code().as_str()),
        (400, "LimitExceededException"),
        "case full again: expected 400 LimitExceededException, got {} {:?} ({})",
        refused_again.status,
        refused_again.error_code(),
        refused_again.message()
    );
    let names = reported_names(&ses, full).await;
    assert_eq!(
        names.len(),
        MAX_EVENT_DESTINATIONS_PER_SET,
        "case full again: the set must report exactly {MAX_EVENT_DESTINATIONS_PER_SET} \
         destinations, got {}",
        names.len()
    );
    assert!(
        !names.contains(&freed),
        "case freeing a slot: the deleted destination {freed:?} must be gone, but the set \
         still reports it"
    );
    assert!(
        names.contains(&name.to_string()),
        "case freeing a slot: the destination that took the freed slot must be reported, but \
         the set reports no {name:?}"
    );
}
