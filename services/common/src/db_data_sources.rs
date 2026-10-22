// SPDX-License-Identifier: CC0-1.0

//! The DATA-SOURCE DIRECTORY: which database service minted an immutable DB resource
//! id, and what engine the resource it names runs.
//!
//! A monitoring service addresses a database resource by the IMMUTABLE resource id its
//! OWNING service minted (`DbiResourceId` for a DB instance, `DbClusterResourceId` for
//! a cluster), and several services mint ids of the same FORM: Amazon RDS and Amazon
//! DocumentDB both mint `db-<TOKEN>` for an instance and `cluster-<TOKEN>` for a
//! cluster. So the form of an id cannot say which service owns it, and a monitoring
//! service that needs to know — Performance Insights has to decide whether a request's
//! `ServiceType` is the service that owns the resource its `Identifier` names — can
//! only know because the minting service RECORDED it.
//!
//! This is that record: ONE directory in the shared engine, written by the database
//! services when they mint a resource id (and cleared when the resource is deleted),
//! read by the services that monitor those resources. Nothing here names a wire shape:
//! the directory holds the resource id, the monitoring `ServiceType` that serves it and
//! the engine it runs, and a reader projects those onto the shapes ITS model declares.
//!
//! The directory is a record of what this emulator MINTED, so it is partial by
//! construction: an id it holds no record of may still be a resource id of the right
//! form (one minted before the process restarted, say), and a reader must treat
//! "unknown here" as "undecided" rather than as "does not exist". It is also BOUNDED —
//! see [`MAX_REGISTERED_DATA_SOURCES`] — so a caller looping on create cannot grow it
//! without end; the oldest registration is dropped first, which keeps the resources a
//! live workflow just created resolvable.

use std::collections::{HashMap, VecDeque};

/// The monitoring `ServiceType` that serves an Amazon RDS data source — the value of
/// the Performance Insights `ServiceType` enum for RDS.
pub const SERVICE_TYPE_RDS: &str = "RDS";

/// The monitoring `ServiceType` that serves an Amazon DocumentDB data source.
pub const SERVICE_TYPE_DOCDB: &str = "DOCDB";

/// How many data sources the directory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this directory lives in memory for the
/// lifetime of the process, so without a bound a caller creating resources in a loop
/// would grow it without end. It is set far above any ordinary workflow, and past it
/// the OLDEST registration is dropped — never a newer one — so a resource a running
/// workflow just created is always still resolvable.
pub const MAX_REGISTERED_DATA_SOURCES: usize = 10_000;

/// One database resource a database service minted, as the directory records it.
#[derive(Debug, Clone)]
pub struct DataSourceRecord {
    /// The immutable resource id the owning service minted (`db-…` / `cluster-…`),
    /// which is also this record's key in the directory.
    pub resource_id: String,
    /// The monitoring `ServiceType` that serves this resource: [`SERVICE_TYPE_RDS`] or
    /// [`SERVICE_TYPE_DOCDB`].
    pub service_type: String,
    /// The database engine the resource runs (`mysql`, `postgres`, `docdb`, …), as the
    /// owning service persisted it.
    pub engine: String,
}

/// The directory itself: resource id -> the service that owns it.
///
/// Registered in the shared engine under THIS type, so the service that mints a
/// resource id and the service that monitors it read one directory instead of each
/// keeping its own copy.
#[derive(Debug, Default)]
pub struct DataSourceDirectory {
    /// The records, by resource id.
    sources: HashMap<String, DataSourceRecord>,
    /// Registration order, so the ceiling drops the oldest entry first.
    order: VecDeque<String>,
}

impl DataSourceDirectory {
    /// Record that `resource_id` is a data source of `service_type` running `engine`.
    ///
    /// Re-registering an id REPLACES its record (an id is minted once, so this is the
    /// owning service correcting what it told the directory) and does not move its
    /// place in the eviction order.
    pub fn register(&mut self, resource_id: &str, service_type: &str, engine: &str) {
        let record = DataSourceRecord {
            resource_id: resource_id.to_string(),
            service_type: service_type.to_string(),
            engine: engine.to_string(),
        };
        if self
            .sources
            .insert(resource_id.to_string(), record)
            .is_none()
        {
            self.order.push_back(resource_id.to_string());
            while self.order.len() > MAX_REGISTERED_DATA_SOURCES {
                if let Some(oldest) = self.order.pop_front() {
                    self.sources.remove(&oldest);
                }
            }
        }
    }

    /// Drop the record of `resource_id`: the resource it named is gone, so the
    /// directory no longer claims to know which service owns that id.
    pub fn forget(&mut self, resource_id: &str) {
        if self.sources.remove(resource_id).is_some() {
            self.order.retain(|id| id != resource_id);
        }
    }

    /// The record of `resource_id`, or `None` when the directory holds none.
    pub fn get(&self, resource_id: &str) -> Option<&DataSourceRecord> {
        self.sources.get(resource_id)
    }

    /// The record of `resource_id` when the directory holds it under a service OTHER
    /// than `service_type` — the one case in which "this pair names no data source" is
    /// a fact and not a guess.
    ///
    /// `None` both when the id is registered under `service_type` itself and when the
    /// directory holds no record of it at all: an unknown id is UNDECIDED here, and the
    /// caller decides it on whatever else it knows.
    pub fn registered_elsewhere(
        &self,
        resource_id: &str,
        service_type: &str,
    ) -> Option<&DataSourceRecord> {
        self.sources
            .get(resource_id)
            .filter(|record| record.service_type != service_type)
    }

    /// How many data sources the directory holds.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Whether the directory holds no data source at all.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource id of the form a database service mints, numbered.
    fn resource_id(index: usize) -> String {
        format!("db-REGISTERED{index:013}")
    }

    #[test]
    fn a_registration_answers_only_for_another_service_type() {
        let mut directory = DataSourceDirectory::default();
        directory.register("db-OWNEDBYRDS000001", SERVICE_TYPE_RDS, "mysql");

        let record = directory
            .get("db-OWNEDBYRDS000001")
            .expect("case a registered id: the directory holds it");
        assert_eq!(
            (record.service_type.as_str(), record.engine.as_str()),
            (SERVICE_TYPE_RDS, "mysql"),
            "case a registered id: expected the owner and engine it was registered with, got \
             {record:?}"
        );
        assert!(
            directory
                .registered_elsewhere("db-OWNEDBYRDS000001", SERVICE_TYPE_RDS)
                .is_none(),
            "case the owning service type: a record under it is not registered elsewhere"
        );
        assert!(
            directory
                .registered_elsewhere("db-OWNEDBYRDS000001", SERVICE_TYPE_DOCDB)
                .is_some(),
            "case the other service type: the record is registered elsewhere"
        );
        assert!(
            directory
                .registered_elsewhere("db-NEVERREGISTERED1", SERVICE_TYPE_DOCDB)
                .is_none(),
            "case an id the directory never saw: undecided, so not registered elsewhere"
        );
    }

    #[test]
    fn re_registering_an_id_replaces_its_record_without_growing_the_directory() {
        let mut directory = DataSourceDirectory::default();
        directory.register("db-REREGISTERED0001", SERVICE_TYPE_RDS, "mysql");
        directory.register("db-REREGISTERED0001", SERVICE_TYPE_RDS, "postgres");

        assert_eq!(
            directory.len(),
            1,
            "case a re-registration: expected one record, got {}",
            directory.len()
        );
        assert_eq!(
            directory
                .get("db-REREGISTERED0001")
                .map(|record| record.engine.clone()),
            Some("postgres".to_string()),
            "case a re-registration: expected the newest record to stand"
        );
    }

    #[test]
    fn forgetting_an_id_drops_it_and_leaves_the_others() {
        let mut directory = DataSourceDirectory::default();
        directory.register("db-KEPT000000000001", SERVICE_TYPE_RDS, "mysql");
        directory.register("cluster-DROPPED0001", SERVICE_TYPE_DOCDB, "docdb");

        directory.forget("cluster-DROPPED0001");
        assert!(
            directory.get("cluster-DROPPED0001").is_none()
                && directory.get("db-KEPT000000000001").is_some(),
            "case one id forgotten: expected only it to go, got {} record(s)",
            directory.len()
        );

        // Forgetting an id the directory does not hold changes nothing, so a repeated
        // delete is harmless.
        directory.forget("cluster-DROPPED0001");
        directory.forget("db-NEVERREGISTERED1");
        assert_eq!(
            directory.len(),
            1,
            "case forgetting an absent id: expected the remaining record untouched, got {}",
            directory.len()
        );

        directory.forget("db-KEPT000000000001");
        assert!(
            directory.is_empty(),
            "case every id forgotten: expected an empty directory, got {} record(s)",
            directory.len()
        );
    }

    #[test]
    fn the_ceiling_drops_the_oldest_registration_first() {
        let mut directory = DataSourceDirectory::default();
        for index in 0..MAX_REGISTERED_DATA_SOURCES {
            directory.register(&resource_id(index), SERVICE_TYPE_RDS, "mysql");
        }
        assert_eq!(
            directory.len(),
            MAX_REGISTERED_DATA_SOURCES,
            "case a directory filled to the ceiling: expected {MAX_REGISTERED_DATA_SOURCES} \
             records, got {}",
            directory.len()
        );

        // A re-registration is not a new entry, so it moves nothing out.
        directory.register(&resource_id(0), SERVICE_TYPE_RDS, "postgres");
        assert!(
            directory.get(&resource_id(0)).is_some()
                && directory.len() == MAX_REGISTERED_DATA_SOURCES,
            "case a re-registration at the ceiling: expected the record kept, got {} record(s)",
            directory.len()
        );

        // One more NEW id evicts the oldest and only the oldest.
        directory.register(
            &resource_id(MAX_REGISTERED_DATA_SOURCES),
            SERVICE_TYPE_RDS,
            "mysql",
        );
        assert_eq!(
            directory.len(),
            MAX_REGISTERED_DATA_SOURCES,
            "case one past the ceiling: expected the ceiling held, got {}",
            directory.len()
        );
        assert!(
            directory.get(&resource_id(0)).is_none(),
            "case one past the ceiling: expected the oldest registration dropped"
        );
        assert!(
            directory.get(&resource_id(1)).is_some()
                && directory
                    .get(&resource_id(MAX_REGISTERED_DATA_SOURCES))
                    .is_some(),
            "case one past the ceiling: expected the newest registrations kept"
        );
    }
}
