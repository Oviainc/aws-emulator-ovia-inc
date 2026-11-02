// SPDX-License-Identifier: CC0-1.0

//! The SQL Server High Availability registrations the account holds: for each instance
//! enabled for SQL Server High Availability standby detection monitoring, the Secrets
//! Manager secret the enablement named and the determination monitoring owes that node.
//!
//! A submodule of [`crate::state`] rather than more lines in `state.rs`, for the reason
//! `classic_link.rs` gives: one subject — the registration, its projection and the one seam
//! it is written through — in one file, and out of the end of `state.rs`, where every
//! concurrently landing EC2 fix inserts. Everything here is re-exported by `crate::state`,
//! so a handler reaches it as `crate::state::<item>`.
//!
//! The store itself is a field of [`ComAmazonawsEc2Amazonec2State`]
//! (`sql_ha_registrations`), because the state struct is one struct; only the code that
//! reads and writes it lives here.
//!
//! ## What is stored, and what is derived
//!
//! A registration is a FACT ABOUT an instance the account already holds, so the store keeps
//! only what the registration itself adds, keyed by the `InstanceId` it is a registration
//! of: the secret the enablement named and the role monitoring determined for the node,
//! with the instant that determination falls due. Everything else `RegisteredInstance`
//! reports is derived when the registration is projected
//! ([`ComAmazonawsEc2Amazonec2State::sql_ha_instance_view`]):
//!
//! * `HaStatus` is `processing` until the clock reaches the determination's due instant —
//!   "`processing` - The SQL Server High Availability status for the SQL Server High
//!   Availability instance is being updated" — and the determined status from then on, so
//!   the move off `processing` happens whether or not anybody called in the meantime and a
//!   read never has to write it back.
//! * `SqlServerLicenseUsage` follows from that status: standby nodes are the ones the
//!   waiver applies to ("If the instances are determined to be standby failover nodes,
//!   Amazon Web Services automatically applies SQL Server licensing fee waiver for those
//!   instances", `EnableInstanceSqlHaStandbyDetections`), so a `standby` node is `waived`
//!   and every other status is `full`.
//! * `LastUpdatedTime` is "the date and time when the instance's SQL Server High
//!   Availability status was last updated", which is the instant the reported status was
//!   reached: the determination's due instant once it has passed, and otherwise the instant
//!   the instance was registered.
//! * `Tags` are the instance's own, read off the instance record, so a tag `CreateTags`
//!   writes after the registration is reported by the next describe and no second copy can
//!   drift.
//!
//! The store starts EMPTY and stays empty until something registers an instance.
//! [`ComAmazonawsEc2Amazonec2State::register_instance_sql_ha`] is the one way a registration
//! is stored — the seam `EnableInstanceSqlHaStandbyDetections`, still a scaffold, has to
//! write through — and it refuses a registration for an instance this account does not
//! hold, so a reported registration always names an instance that exists.

use super::*;

/// `HaStatus`'s `processing` member: "The SQL Server High Availability status for the SQL
/// Server High Availability instance is being updated" (`RegisteredInstance$HaStatus`). The
/// full catalog is the generated `crate::ops::types::HA_STATUS_VALUES`; these constants name
/// the members this slice reads and writes, so one value is spelled one way everywhere.
pub const HA_STATUS_PROCESSING: &str = "processing";

/// `HaStatus`'s `active` member: "The SQL Server High Availability instance is an active node
/// in an SQL Server High Availability cluster" (see [`HA_STATUS_PROCESSING`]).
pub const HA_STATUS_ACTIVE: &str = "active";

/// `HaStatus`'s `standby` member: "The SQL Server High Availability instance is a standby
/// failover node in an SQL Server High Availability cluster" (see [`HA_STATUS_PROCESSING`]).
pub const HA_STATUS_STANDBY: &str = "standby";

/// `HaStatus`'s `invalid` member: "An error occurred due to misconfigured permissions, or
/// unable to dertemine SQL Server High Availability status" (see [`HA_STATUS_PROCESSING`]).
pub const HA_STATUS_INVALID: &str = "invalid";

/// `SqlServerLicenseUsage`'s `full` member: "The SQL Server High Availability instance is
/// using a full SQL Server license" (`RegisteredInstance$SqlServerLicenseUsage`). The full
/// catalog is the generated `crate::ops::types::SQL_SERVER_LICENSE_USAGE_VALUES`.
pub const SQL_SERVER_LICENSE_USAGE_FULL: &str = "full";

/// `SqlServerLicenseUsage`'s `waived` member: "The SQL Server High Availability instance is
/// waived from the SQL Server license" (see [`SQL_SERVER_LICENSE_USAGE_FULL`]).
pub const SQL_SERVER_LICENSE_USAGE_WAIVED: &str = "waived";

/// What monitoring determined about one registered node, and when that determination falls
/// due.
///
/// A registration does not report its node's role immediately: "Once enabled, Amazon Web
/// Services monitors the metadata for the instances to determine whether they are active or
/// standby nodes" (`EnableInstanceSqlHaStandbyDetections`), and until that has happened the
/// instance's `HaStatus` is `processing`. So the determination is held with the instant it
/// becomes the instance's status, and the projection compares that instant with the clock
/// rather than a reader writing the move back.
#[derive(Debug, Clone)]
pub struct SqlHaDetermination {
    /// When this determination becomes the instance's reported `HaStatus`. Before it, the
    /// instance reads [`HA_STATUS_PROCESSING`].
    pub due_at: SystemTime,
    /// The status monitoring determined: one of [`HA_STATUS_ACTIVE`], [`HA_STATUS_STANDBY`]
    /// or [`HA_STATUS_INVALID`] — the three settled members of `HaStatus`.
    pub ha_status: String,
    /// "A brief description of the SQL Server High Availability status. If the instance is
    /// in the `invalid` High Availability status, this parameter includes the error message"
    /// (`RegisteredInstance$ProcessingStatus`). Absent when monitoring reported none.
    pub processing_status: Option<String>,
}

/// One instance's SQL Server High Availability registration: what enabling it added, beside
/// the instance the account already holds.
///
/// Not the modelled `RegisteredInstance` shape, unlike most stores of this module, because
/// four of that shape's seven members are not facts of the REGISTRATION: the status, the
/// license usage and the instant the status was reached follow from the determination below,
/// and the tags belong to the instance.
#[derive(Debug, Clone)]
pub struct SqlHaRegistrationRecord {
    /// When the instance was registered for monitoring: the instant its first status — and
    /// so its `LastUpdatedTime` while no determination has fallen due — dates from.
    pub registered_at: SystemTime,
    /// "The ARN of the Secrets Manager secret containing the SQL Server access credentials"
    /// the enablement named (`EnableInstanceSqlHaStandbyDetectionsRequest$SqlServerCredentials`),
    /// reported as `RegisteredInstance$SqlServerCredentials`. Absent when the request named
    /// none, in which case "deafult local user credentials will be used".
    pub sql_server_credentials: Option<String>,
    /// What monitoring determined for this node, if it has reached a determination at all.
    /// `None` leaves the instance `processing` indefinitely, which is what an instance
    /// nothing has determined anything about reports.
    pub determination: Option<SqlHaDetermination>,
}

/// The caller-chosen bytes one stored registration holds: the secret ARN the enablement
/// named and the status words a determination carries. The key is an instance id this
/// service minted, so it is fixed-size.
///
/// The ONE definition of that charge, the way every other store of this module has one:
/// [`ComAmazonawsEc2Amazonec2State::register_instance_sql_ha`] charges exactly it, and a
/// path that takes a registration back out must give back the same number.
pub fn sql_ha_registration_payload_bytes(record: &SqlHaRegistrationRecord) -> usize {
    let credentials = record
        .sql_server_credentials
        .as_ref()
        .map_or(0, String::len);
    let determined = record.determination.as_ref().map_or(0, |determination| {
        determination.ha_status.len()
            + determination
                .processing_status
                .as_ref()
                .map_or(0, String::len)
    });
    credentials.saturating_add(determined)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Register the instance `instance_id` for SQL Server High Availability standby
    /// detection monitoring with `record`, and answer whether the store took it.
    ///
    /// `false` means NOTHING was written and the caller must refuse the request rather than
    /// report a registration that does not exist: the account holds no such instance (the
    /// store never holds a registration of an instance that is not there, so a describe
    /// cannot report one), or the registration's caller-chosen bytes do not fit the store's
    /// byte budget ([`Self::would_exceed_stored_bytes_after_refund`]).
    ///
    /// An instance is registered once, so registering one again REPLACES its registration —
    /// which is how re-enabling an instance with different credentials reads — and the bytes
    /// the first was charged are given back.
    ///
    /// Its only caller today is this crate's tests, because
    /// `EnableInstanceSqlHaStandbyDetections` is still a scaffold: this is the seam that
    /// operation must write through, not dead code.
    pub fn register_instance_sql_ha(
        &mut self,
        instance_id: &str,
        record: SqlHaRegistrationRecord,
    ) -> bool {
        if !self.instances.contains_key(instance_id) {
            return false;
        }
        let charge = sql_ha_registration_payload_bytes(&record);
        let refund = self
            .sql_ha_registrations
            .get(instance_id)
            .map_or(0, sql_ha_registration_payload_bytes);
        if self.would_exceed_stored_bytes_after_refund(charge, refund) {
            return false;
        }
        self.release_stored_bytes(refund);
        self.add_stored_bytes(charge);
        self.sql_ha_registrations
            .insert(instance_id.to_string(), record);
        true
    }

    /// The modelled `RegisteredInstance` for `instance_id` as of `now`, or `None` when that
    /// instance is not registered for SQL Server High Availability monitoring — or is no
    /// longer an instance this account holds.
    ///
    /// The one projection of a registration, so every operation that reports one reports it
    /// identically, and a READ: the status, the license usage and the instant the status was
    /// reached are all DERIVED from the stored determination and the clock (see the module
    /// docs), so two calls a moment apart report the same thing and nothing is written back.
    pub fn sql_ha_instance_view(
        &self,
        instance_id: &str,
        now: SystemTime,
    ) -> Option<crate::ops::types::RegisteredInstance> {
        let record = self.sql_ha_registrations.get(instance_id)?;
        let instance = self.instances.get(instance_id)?;
        // A determination the clock has not reached yet has not happened: the instance is
        // still having its status updated.
        let settled = record
            .determination
            .as_ref()
            .filter(|determination| now >= determination.due_at);
        let ha_status = settled.map_or(HA_STATUS_PROCESSING, |determination| {
            determination.ha_status.as_str()
        });
        Some(crate::ops::types::RegisteredInstance {
            instance_id: Some(instance.instance_id.clone()),
            sql_server_license_usage: Some(sql_server_license_usage_of(ha_status).to_string()),
            ha_status: Some(ha_status.to_string()),
            processing_status: settled
                .and_then(|determination| determination.processing_status.clone()),
            last_updated_time: Some(system_time_to_date_time(
                settled.map_or(record.registered_at, |determination| determination.due_at),
            )),
            sql_server_credentials: record.sql_server_credentials.clone(),
            tags: Some(tag_list(&instance.tags)),
        })
    }
}

/// The `SqlServerLicenseUsage` an instance in the SQL Server High Availability status
/// `ha_status` is on.
///
/// The waiver is what the monitoring is FOR: "If the instances are determined to be standby
/// failover nodes, Amazon Web Services automatically applies SQL Server licensing fee waiver
/// for those instances" (`EnableInstanceSqlHaStandbyDetections`). So a standby node is
/// [`SQL_SERVER_LICENSE_USAGE_WAIVED`] and every other status — an active node, one still
/// being determined, one whose status could not be determined at all — is on the
/// [`SQL_SERVER_LICENSE_USAGE_FULL`] license it was launched with.
pub fn sql_server_license_usage_of(ha_status: &str) -> &'static str {
    if ha_status == HA_STATUS_STANDBY {
        SQL_SERVER_LICENSE_USAGE_WAIVED
    } else {
        SQL_SERVER_LICENSE_USAGE_FULL
    }
}

/// Refuse the `InstanceIds` entries that name no instance registered for SQL Server High
/// Availability monitoring: HTTP 400 `InvalidInstanceID.NotFound`, in the wording the SQL
/// Server High Availability describes answer.
///
/// ONE refusal for the whole request rather than one per id, and its own wording rather than
/// [`crate::scaffolded::Scaffolded::invalid_instance_id_not_found`]'s: this surface's
/// validator does not distinguish an id the account holds no instance for from one whose
/// instance is not enabled for monitoring — both are "not found in account or SQL HA standby
/// detection not enabled" — so it names every id it could not resolve and says so once. A
/// second named constructor rather than a literal at a call site for the reason
/// [`crate::scaffolded::Scaffolded::instance_id_is_malformed`] is one: EC2's per-operation
/// validators do not word an unresolvable instance id the same way, and keeping each wording
/// in one place is what stops a third call site from inventing a third.
///
/// `unresolved` is in the order the request named them, so the sentence lists them the way
/// the caller sent them.
pub fn sql_ha_instance_ids_not_found<E>(unresolved: &[&str]) -> Scaffolded<E> {
    Scaffolded::service_error(
        // The same wire code `crate::scaffolded::Scaffolded::invalid_instance_id_not_found`
        // answers; only the sentence differs.
        "InvalidInstanceID.NotFound",
        400,
        format!(
            "Requested instance ID(s) [{}] not found in account or SQL HA standby detection \
             not enabled.",
            unresolved.join(", ")
        ),
    )
}
