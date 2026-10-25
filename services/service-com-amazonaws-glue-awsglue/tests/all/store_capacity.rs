// SPDX-License-Identifier: CC0-1.0

//! The store's CAPACITY contract: the bounds that keep this service's resident memory
//! finite whatever a caller asks for.
//!
//! Every ceiling in `state.rs` is one of two kinds, and this suite covers the kind the
//! per-container ceilings cannot express: how many CONTAINERS the store holds
//! ([`MAX_CATALOGS`]) and how many records it holds ACROSS them
//! ([`MAX_STORED_DATABASES`], [`MAX_STORED_CONNECTIONS`]). A per-catalog ceiling alone
//! bounds one catalog; multiplied by an unbounded number of catalogs it bounds nothing.
//!
//! The bounds are asserted against the store directly rather than over the wire: reaching
//! a ten-thousand-record ceiling through requests would exercise the transport, and the
//! container ceiling is only reachable at all through the store, since a request naming a
//! catalog this account does not own is already refused before any ceiling is consulted.

use std::collections::HashMap;
use std::time::SystemTime;

use service_com_amazonaws_glue_awsglue::ops::types::{ConnectionInput, DatabaseInput};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, ConnectionEntry, DatabaseEntry, MAX_CATALOGS,
    MAX_STORED_CONNECTIONS, MAX_STORED_DATABASES,
};

/// A stored database with nothing but its name set — enough to occupy one slot.
fn database(name: &str) -> DatabaseEntry {
    let input = DatabaseInput {
        name: name.to_string(),
        ..DatabaseInput::default()
    };
    DatabaseEntry::from_input(&input, "000000000000", None, SystemTime::UNIX_EPOCH)
}

/// A stored connection definition with nothing but its name and type set.
fn connection(name: &str) -> ConnectionEntry {
    let input = ConnectionInput {
        name: name.to_string(),
        connection_type: "JDBC".to_string(),
        connection_properties: HashMap::new(),
        ..ConnectionInput::default()
    };
    ConnectionEntry::from_input(&input, None, None, SystemTime::UNIX_EPOCH)
}

/// Fill `store` with `catalogs` catalogs holding `per_catalog` databases each, through
/// the one admission point, and report how many catalogs were admitted.
fn seed_catalogs(
    store: &mut ComAmazonawsGlueAwsglueState,
    catalogs: usize,
    per_catalog: usize,
) -> usize {
    let mut admitted = 0;
    for catalog in 0..catalogs {
        let catalog_id = format!("catalog-{catalog}");
        let Some(entry) = store.catalog_mut_for_create(&catalog_id) else {
            break;
        };
        admitted += 1;
        for index in 0..per_catalog {
            let name = format!("database-{catalog}-{index}");
            entry.databases.insert(name.clone(), database(&name));
        }
    }
    admitted
}

/// The number of catalog containers is bounded: the store admits catalogs up to
/// [`MAX_CATALOGS`] and refuses a FRESH one past it, while a catalog it already holds
/// stays reachable — so the bound caps how many containers exist, not which record a
/// create may write.
#[test]
fn the_catalog_container_count_is_bounded() {
    let mut store = ComAmazonawsGlueAwsglueState::default();

    let admitted = seed_catalogs(&mut store, MAX_CATALOGS, 0);
    assert_eq!(
        admitted, MAX_CATALOGS,
        "case up to the ceiling: expected {MAX_CATALOGS} catalogs admitted, got {admitted}"
    );

    for fresh in ["one-too-many", "another-account", ""] {
        assert!(
            store.catalog_mut_for_create(fresh).is_none(),
            "case fresh catalog {fresh:?}: a catalog past the ceiling of {MAX_CATALOGS} must be \
             refused, got one"
        );
    }

    // A catalog the store already holds is still reachable at the ceiling: the bound is
    // on new containers, so a create into an existing catalog is unaffected.
    for held in ["catalog-0", "catalog-1"] {
        assert!(
            store.catalog_mut_for_create(held).is_some(),
            "case held catalog {held}: an existing catalog must stay reachable at the ceiling"
        );
    }

    // Free one container and a fresh catalog is admitted again: the ceiling bounds what
    // is stored rather than latching on how many were ever created.
    store.catalogs.remove("catalog-0");
    assert!(
        store.catalog_mut_for_create("after-a-removal").is_some(),
        "case a freed slot: a fresh catalog must be admitted once one is removed"
    );
}

/// Databases are bounded ACROSS catalogs, not only within one: once the store holds
/// [`MAX_STORED_DATABASES`] it can hold no more, whichever catalog the next one would go
/// in, and freeing one slot lets it hold another.
#[test]
fn the_stored_database_count_is_bounded_across_catalogs() {
    let mut store = ComAmazonawsGlueAwsglueState::default();

    // Spread the records over several containers: the point of the bound is that a
    // per-catalog ceiling multiplied by a container count is not itself a bound.
    let per_catalog = MAX_STORED_DATABASES / 4;
    seed_catalogs(&mut store, 4, per_catalog);
    assert_eq!(
        store.stored_databases(),
        MAX_STORED_DATABASES,
        "case at the ceiling: expected {MAX_STORED_DATABASES} stored databases, got {}",
        store.stored_databases()
    );
    assert!(
        !store.can_store_database(),
        "case at the ceiling: the store must hold no further database, got room for one"
    );

    store
        .catalogs
        .get_mut("catalog-0")
        .expect("the seeded catalog")
        .databases
        .remove("database-0-0");
    assert!(
        store.can_store_database(),
        "case a freed slot: the store must hold another database once one is removed"
    );
}

/// The same bound for connection definitions: counted across every catalog, not per
/// catalog.
#[test]
fn the_stored_connection_count_is_bounded_across_catalogs() {
    let mut store = ComAmazonawsGlueAwsglueState::default();

    let per_catalog = MAX_STORED_CONNECTIONS / 4;
    for catalog in 0..4 {
        let catalog_id = format!("catalog-{catalog}");
        let entry = store
            .catalog_mut_for_create(&catalog_id)
            .expect("a catalog below the container ceiling");
        for index in 0..per_catalog {
            let name = format!("connection-{catalog}-{index}");
            entry.connections.insert(name.clone(), connection(&name));
        }
    }
    assert_eq!(
        store.stored_connections(),
        MAX_STORED_CONNECTIONS,
        "case at the ceiling: expected {MAX_STORED_CONNECTIONS} stored connections, got {}",
        store.stored_connections()
    );
    assert!(
        !store.can_store_connection(),
        "case at the ceiling: the store must hold no further connection, got room for one"
    );

    store
        .catalogs
        .get_mut("catalog-0")
        .expect("the seeded catalog")
        .connections
        .remove("connection-0-0");
    assert!(
        store.can_store_connection(),
        "case a freed slot: the store must hold another connection once one is removed"
    );
}
