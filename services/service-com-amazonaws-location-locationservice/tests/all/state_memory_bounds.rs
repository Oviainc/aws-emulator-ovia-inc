// SPDX-License-Identifier: CC0-1.0

//! The emulator memory bounds every request-growable container of this store carries.
//!
//! RULE: a container a caller can grow one entry per request is bounded, so repeated
//! writes cannot grow the in-memory store without limit — the position history and the
//! device map per tracker, the geofences per collection, and the consumer associations
//! per tracker. Each bound is the emulator's own (not an AWS quota) and each is applied
//! by the state helper that does the insert, which is the contract a write uses: a
//! handler reaches these containers through that helper rather than inserting itself.
//!
//! The device-position half is driven over HTTP by the other suites, and so is the
//! geofence write path (`tests/all/geofences.rs` drives `BatchPutGeofence`); the
//! BOUNDS themselves are asserted against the state helpers directly, because
//! reaching one over the wire would take a thousand requests per bound. The consumer
//! half has no implemented writer yet (`AssociateTrackerConsumer` is a scaffolded
//! 501 owning its own stub issue), so it is asserted the same way.

use service_com_amazonaws_location_locationservice::state::{
    Geofence, GeofenceCollection, Tracker, MAX_CONSUMERS_PER_TRACKER,
    MAX_GEOFENCES_PER_COLLECTION,
};

/// An empty collection to put geofences into.
fn collection() -> GeofenceCollection {
    GeofenceCollection {
        name: "fences".to_string(),
        arn: "arn:aws:geo:us-west-2:000000000000:geofence-collection/fences".to_string(),
        description: String::new(),
        kms_key_id: None,
        tags: Default::default(),
        create_time: 0,
        update_time: 0,
        geofences: Default::default(),
    }
}

/// One geofence whose `UpdateTime` is `update_time`, so eviction order is observable.
fn geofence(id: &str, update_time: i64) -> Geofence {
    Geofence {
        geofence_id: id.to_string(),
        geometry: Default::default(),
        status: "ACTIVE".to_string(),
        create_time: update_time,
        update_time,
        properties: Default::default(),
    }
}

fn tracker() -> Tracker {
    Tracker {
        name: "fleet".to_string(),
        arn: "arn:aws:geo:us-west-2:000000000000:tracker/fleet".to_string(),
        description: String::new(),
        position_filtering: "TimeBased".to_string(),
        kms_key_id: None,
        event_bridge_enabled: false,
        kms_key_enable_geospatial_queries: false,
        tags: Default::default(),
        create_time: 0,
        update_time: 0,
        consumer_arns: Default::default(),
        devices: Default::default(),
    }
}

#[test]
fn a_collection_holds_at_most_the_bound_and_evicts_the_least_recently_updated() {
    // Rows differ along how far past the bound the writes go: at it, one past it, and
    // well past it.
    for overshoot in [0usize, 1, 25] {
        let case = format!("{overshoot} puts past the bound");
        let mut collection = collection();
        let total = MAX_GEOFENCES_PER_COLLECTION + overshoot;
        for index in 0..total {
            // `UpdateTime` ascends with the put order, so the oldest put is the one
            // eviction must drop.
            collection.record_geofence(geofence(&format!("fence-{index}"), index as i64));
        }

        assert_eq!(
            collection.geofences.len(),
            MAX_GEOFENCES_PER_COLLECTION,
            "case {case:?}: a collection must hold at most {MAX_GEOFENCES_PER_COLLECTION} \
             geofences, got {}",
            collection.geofences.len()
        );
        // The newest put is always held; the `overshoot` oldest ones were evicted.
        let newest = format!("fence-{}", total - 1);
        assert!(
            collection.geofences.contains_key(&newest),
            "case {case:?}: the geofence just put ({newest}) must be held, got {:?}",
            collection.geofences.keys().collect::<Vec<_>>()
        );
        for index in 0..overshoot {
            let evicted = format!("fence-{index}");
            assert!(
                !collection.geofences.contains_key(&evicted),
                "case {case:?}: {evicted} is the least recently updated and must have been \
                 evicted, got it still held"
            );
        }
    }
}

#[test]
fn re_putting_a_geofence_replaces_it_rather_than_growing_the_collection() {
    let mut collection = collection();
    for round in 0..3i64 {
        collection.record_geofence(geofence("fence-1", round));
        assert_eq!(
            collection.geofences.len(),
            1,
            "case round {round}: a put for a held GeofenceId must replace it, got {:?}",
            collection.geofences.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            collection
                .geofences
                .get("fence-1")
                .map(|held| held.update_time),
            Some(round),
            "case round {round}: the held geofence must be the one just put, got {:?}",
            collection.geofences.get("fence-1").map(|held| held.update_time)
        );
    }
}

#[test]
fn a_tracker_takes_at_most_the_bound_of_consumer_associations() {
    let mut tracker = tracker();
    for index in 0..MAX_CONSUMERS_PER_TRACKER {
        let arn = format!("arn:aws:geo:us-west-2:000000000000:geofence-collection/fences-{index}");
        assert!(
            tracker.associate_consumer(arn.clone()),
            "case {index}: an association inside the bound must be stored, got no room"
        );
        // Re-associating one already held stays held and is never refused.
        assert!(
            tracker.associate_consumer(arn),
            "case {index}: re-associating a held consumer must stay held"
        );
    }
    assert_eq!(
        tracker.consumer_arns.len(),
        MAX_CONSUMERS_PER_TRACKER,
        "the tracker must hold each distinct consumer once, got {}",
        tracker.consumer_arns.len()
    );

    // Past the bound nothing is stored, whatever the ARN.
    for suffix in ["overflow", "another", "yet-another"] {
        let arn = format!("arn:aws:geo:us-west-2:000000000000:geofence-collection/{suffix}");
        assert!(
            !tracker.associate_consumer(arn.clone()),
            "case {suffix:?}: an association past the bound must report no room"
        );
        assert!(
            !tracker.consumer_arns.contains(&arn),
            "case {suffix:?}: a refused association must store nothing, got it held"
        );
    }
    assert_eq!(
        tracker.consumer_arns.len(),
        MAX_CONSUMERS_PER_TRACKER,
        "the bound must hold however many associations arrive, got {}",
        tracker.consumer_arns.len()
    );
}
