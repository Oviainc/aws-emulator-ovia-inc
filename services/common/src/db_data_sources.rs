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
//! the directory holds the resource id, the monitoring `ServiceType` that serves it, the
//! engine it runs and the MONITORING CONFIGURATION the owning service persisted for it
//! ([`MonitoringConfiguration`] — whether Performance Insights is turned on, and which
//! database-insights mode the resource runs under), and a reader projects those onto the
//! shapes ITS model declares.
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

/// The `DatabaseInsightsMode` value under which a resource runs the ADVANCED feature
/// set, as the database control planes model it (`DatabaseInsightsMode`: `advanced` |
/// `standard`).
pub const DATABASE_INSIGHTS_MODE_ADVANCED: &str = "advanced";

/// The monitoring configuration a database control plane persisted for one resource.
///
/// A monitoring service cannot measure whether a feature is turned on for a DB
/// resource — the resource's OWNER is what the setting was given to, so the owner is
/// what records it here, and the monitoring service projects it onto its own shapes.
/// Nothing in this struct names a wire member of any monitoring model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonitoringConfiguration {
    /// Whether Performance Insights is turned on for the resource
    /// (`EnablePerformanceInsights` on the create/modify request).
    pub performance_insights_enabled: bool,
    /// The `DatabaseInsightsMode` the resource runs under (`standard` | `advanced`), or
    /// `None` when the owning service's model has no such setting at all — Amazon
    /// DocumentDB, say, which persists only whether Performance Insights is on.
    pub database_insights_mode: Option<String>,
}

impl MonitoringConfiguration {
    /// Whether the resource runs the ADVANCED database-insights mode, the mode whose
    /// feature set is wider than the standard one.
    ///
    /// Compared without regard to case: the mode is a modelled enum value, and the
    /// control planes store it as the caller wrote it.
    pub fn runs_advanced_insights_mode(&self) -> bool {
        self.database_insights_mode
            .as_deref()
            .is_some_and(|mode| mode.eq_ignore_ascii_case(DATABASE_INSIGHTS_MODE_ADVANCED))
    }
}

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
    /// What the owning service persisted about this resource's monitoring
    /// configuration, or `None` when it recorded none — which a reader must treat as
    /// UNDECIDED (the configuration is unknown here), never as "everything off".
    pub monitoring: Option<MonitoringConfiguration>,
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
    /// place in the eviction order. The replacement carries no monitoring
    /// configuration, so an owner that records one calls [`Self::record_monitoring`]
    /// after registering.
    pub fn register(&mut self, resource_id: &str, service_type: &str, engine: &str) {
        let record = DataSourceRecord {
            resource_id: resource_id.to_string(),
            service_type: service_type.to_string(),
            engine: engine.to_string(),
            monitoring: None,
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

    /// Record what the owning service persisted about `resource_id`'s monitoring
    /// configuration — on the create that minted it, and again on every modification
    /// that changes one of those settings, so a monitoring service reads the
    /// configuration the resource is running under now rather than the one it was
    /// created with.
    ///
    /// A no-op for an id the directory holds no record of: the owner registers the id
    /// first, and nothing here invents a record for a resource the directory never saw
    /// minted.
    pub fn record_monitoring(&mut self, resource_id: &str, monitoring: MonitoringConfiguration) {
        if let Some(record) = self.sources.get_mut(resource_id) {
            record.monitoring = Some(monitoring);
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
    fn a_monitoring_configuration_is_recorded_against_the_registered_id_only() {
        let mut directory = DataSourceDirectory::default();
        directory.register("db-MONITORED00000001", SERVICE_TYPE_RDS, "mysql");

        // A registered id with nothing recorded yet is UNDECIDED, not "everything off".
        assert_eq!(
            directory
                .get("db-MONITORED00000001")
                .map(|record| record.monitoring.clone()),
            Some(None),
            "case a fresh registration: expected no monitoring configuration on record"
        );

        // Each row is a configuration an owner persists, including the re-record a
        // modification makes.
        let rows: &[(&str, bool, Option<&str>, bool)] = &[
            ("performance insights off", false, Some("standard"), false),
            ("standard insights mode", true, Some("standard"), false),
            ("advanced insights mode", true, Some("advanced"), true),
            ("the mode as the caller wrote it", true, Some("ADVANCED"), true),
            ("an owner with no mode at all", true, None, false),
        ];
        for (case, enabled, mode, advanced) in rows {
            directory.record_monitoring(
                "db-MONITORED00000001",
                MonitoringConfiguration {
                    performance_insights_enabled: *enabled,
                    database_insights_mode: mode.map(str::to_string),
                },
            );
            let recorded = directory
                .get("db-MONITORED00000001")
                .and_then(|record| record.monitoring.clone())
                .unwrap_or_else(|| panic!("case {case}: expected a recorded configuration"));
            assert_eq!(
                (
                    recorded.performance_insights_enabled,
                    recorded.runs_advanced_insights_mode()
                ),
                (*enabled, *advanced),
                "case {case}: expected ({enabled}, {advanced}), got {recorded:?}"
            );
        }

        // An id the directory never saw minted gets no record invented for it.
        directory.record_monitoring(
            "db-NEVERREGISTERED1",
            MonitoringConfiguration {
                performance_insights_enabled: true,
                database_insights_mode: Some("advanced".to_string()),
            },
        );
        assert!(
            directory.get("db-NEVERREGISTERED1").is_none() && directory.len() == 1,
            "case an unregistered id: expected no record invented, got {} record(s)",
            directory.len()
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
