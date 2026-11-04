// SPDX-License-Identifier: CC0-1.0

//! The ORDER event destinations are deleted in, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in [`crate::configuration_sets`] and
//! the shared event-destination calls, create-and-read table and readers in
//! [`crate::configuration_set_event_destinations`], so routing, deserialization, the
//! handler and serialization are all exercised.
//!
//! The rule the sibling suite pins for one delete — it removes exactly the destination
//! the URI names from exactly the set the URI names, answers an empty 200, and refuses a
//! name the set does not hold with `NotFoundException` — holds for EVERY delete of a
//! sequence, whatever order the sequence comes in. That sequencing is the one axis this
//! suite adds: the table spans how many destinations the set holds (one, two, three, all
//! five of the modelled kinds) and the order the deletes address them in (creation order,
//! reverse order, the middle one first, middle out, every other one first), and the whole
//! remainder is checked after EVERY step rather than after a single delete.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::configuration_set_event_destinations::{
    create_set, destination_cases, reported_destinations, reported_names, DestinationCase,
    EventDestinations,
};
use crate::configuration_sets::Ses;

/// One row of the deletion table: the order the deletes address the attached
/// destinations in, as positions into the shared create-and-read table, whose length is
/// also how many the set holds.
struct DeletionCase {
    case: &'static str,
    order: &'static [usize],
}

/// The rows of the deletion table.
const DELETION_CASES: &[DeletionCase] = &[
    DeletionCase {
        case: "the only destination of the set",
        order: &[0],
    },
    DeletionCase {
        case: "two, in creation order",
        order: &[0, 1],
    },
    DeletionCase {
        case: "two, the newest first",
        order: &[1, 0],
    },
    DeletionCase {
        case: "three, the middle one first",
        order: &[1, 0, 2],
    },
    DeletionCase {
        case: "three, in reverse creation order",
        order: &[2, 1, 0],
    },
    DeletionCase {
        case: "five, in creation order",
        order: &[0, 1, 2, 3, 4],
    },
    DeletionCase {
        case: "five, in reverse creation order",
        order: &[4, 3, 2, 1, 0],
    },
    DeletionCase {
        case: "five, middle out",
        order: &[2, 1, 3, 0, 4],
    },
    DeletionCase {
        case: "five, every other one first",
        order: &[1, 3, 0, 2, 4],
    },
];

/// Row `position` of the shared create-and-read table, so this suite attaches the same
/// modelled destination kinds the sibling suite reads back.
fn row(position: usize) -> DestinationCase {
    destination_cases()
        .into_iter()
        .nth(position)
        .unwrap_or_else(|| panic!("the create-and-read table has no row {position}"))
}

/// Attach the first `count` rows of that table to `set`, which must already exist.
async fn attach_first(ses: &Ses, set: &str, count: usize, case: &str) {
    for position in 0..count {
        let attached = row(position);
        let created = ses
            .create_destination(set, attached.name, attached.definition)
            .await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: attaching {:?} to {set:?}: expected 200, got {} {}",
            attached.name,
            created.status,
            created.message()
        );
    }
}

/// What `set` reports, keyed by destination name, so a case compares a destination's
/// whole stored definition and not only its name.
async fn reported_by_name(ses: &Ses, set: &str) -> BTreeMap<String, Value> {
    reported_destinations(ses, set)
        .await
        .into_iter()
        .map(|destination| {
            let name = destination
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            (name, destination)
        })
        .collect()
}

#[tokio::test]
async fn deleting_destinations_in_any_order_removes_exactly_the_named_ones() {
    for DeletionCase { case, order } in DELETION_CASES {
        let ses = Ses::new();
        let set = "deleted-from";
        // A second set holds the SAME destinations, so a delete that reached past the set
        // the URI names shows up here.
        let untouched = "untouched-set";
        create_set(&ses, set).await;
        create_set(&ses, untouched).await;
        attach_first(&ses, set, order.len(), case).await;
        attach_first(&ses, untouched, order.len(), case).await;

        let attached = reported_by_name(&ses, set).await;
        let elsewhere = reported_by_name(&ses, untouched).await;
        let mut expected: BTreeSet<String> = (0..order.len())
            .map(|position| row(position).name.to_string())
            .collect();
        assert_eq!(
            attached.keys().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {case:?}: before any delete the set must report every attached name, got {:?}",
            attached.keys().collect::<Vec<_>>()
        );

        for position in *order {
            let name = row(*position).name.to_string();

            let deleted = ses.delete_destination(set, &name).await;
            assert_eq!(
                deleted.status,
                200,
                "case {case:?}: deleting {name:?}: expected 200, got {} {} ({})",
                deleted.status,
                deleted.error_code(),
                deleted.message()
            );
            assert!(
                deleted.is_empty_body(),
                "case {case:?}: deleting {name:?}: the response shape declares no members, so \
                 the body must be empty, got {:?}",
                deleted.member("EventDestinations")
            );

            expected.remove(&name);
            let remaining = reported_names(&ses, set).await;
            let want: Vec<String> = expected.iter().cloned().collect();
            assert_eq!(
                remaining, want,
                "case {case:?}: after deleting {name:?} the set must report {want:?}, got \
                 {remaining:?}"
            );

            // Every survivor is still stored exactly as its own create described it.
            let survivors = reported_by_name(&ses, set).await;
            for survivor in &expected {
                assert_eq!(
                    survivors.get(survivor),
                    attached.get(survivor),
                    "case {case:?}: deleting {name:?} must leave {survivor:?} unchanged, got {:?}",
                    survivors.get(survivor)
                );
            }

            // The name is gone, so deleting it again addresses a destination the set does
            // not hold.
            let again = ses.delete_destination(set, &name).await;
            assert_eq!(
                again.status,
                404,
                "case {case:?}: a second delete of {name:?}: expected 404, got {} {}",
                again.status,
                again.message()
            );
            assert_eq!(
                again.error_code(),
                "NotFoundException",
                "case {case:?}: a second delete of {name:?}: expected NotFoundException, got {:?} \
                 ({})",
                again.error_code(),
                again.message()
            );
            assert_eq!(
                again.message(),
                format!("No EventDestination found for {set}"),
                "case {case:?}: a second delete of {name:?} must answer the service's fixed \
                 sentence naming the set, got {:?}",
                again.message()
            );

            // And nothing changed in the set the URI never named.
            assert_eq!(
                reported_by_name(&ses, untouched).await,
                elsewhere,
                "case {case:?}: after deleting {name:?} the other set must report exactly what it \
                 did before"
            );
        }

        // The set outlives its destinations: it reports none and stays addressable.
        let empty = reported_names(&ses, set).await;
        assert!(
            empty.is_empty(),
            "case {case:?}: after every delete the set must report no destination, got {empty:?}"
        );
        let read_set = ses.get_set(set).await;
        assert_eq!(
            read_set.status,
            200,
            "case {case:?}: the set itself must survive losing its destinations: expected 200, \
             got {} {}",
            read_set.status,
            read_set.message()
        );
        assert_eq!(
            read_set.member("ConfigurationSetName"),
            Value::String(set.to_string()),
            "case {case:?}: the surviving set must report its own name, got {:?}",
            read_set.member("ConfigurationSetName")
        );
    }
}
