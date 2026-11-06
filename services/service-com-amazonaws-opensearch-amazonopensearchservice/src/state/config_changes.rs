// SPDX-License-Identifier: CC0-1.0

//! Configuration changes of a domain, and the dry runs that validate one.
//!
//! A [`ConfigChange`] is the one record of an accepted `UpdateDomainConfig`:
//! `DescribeDomainChangeProgress` reports its progress, `CancelDomainConfigChange`
//! withdraws it and the domain's own `ChangeProgressDetails` is derived from it. A
//! [`DryRun`] is the same for a change asked about rather than applied, which
//! `DescribeDryRunProgress` reports.

use super::*;

/// One configuration change on a domain: what `DescribeDomainChangeProgress`
/// reports in full and what `DomainStatus`/`DomainConfig` summarise as a
/// `ChangeProgressDetails`.
///
/// A change is minted by the call that starts it (`CreateDomain`, or an
/// `UpdateDomainConfig` that changed at least one option) and completes when the
/// provisioning span the clock owes it has passed — the same instant the domain
/// itself settles, so the change's status and the domain's `Processing` flag can
/// never disagree.
#[derive(Debug, Clone)]
pub struct ConfigChange {
    /// `ChangeProgressStatusDetails$ChangeId`: the GUID this change is addressed by.
    pub change_id: String,
    /// The `DomainConfig` member names this change sets, as the `OPT_*` constants
    /// spell them. Reported as `PendingProperties` while the change is in flight and
    /// as `CompletedProperties` once it has landed.
    pub properties: Vec<String>,
    /// `StartTime`: the instant of the call that started the change.
    pub started_at: SystemTime,
    /// `LastUpdatedTime`: the instant this change's status last moved.
    pub last_updated_at: SystemTime,
    /// When the clock owes this change completion; `None` once it has completed.
    pub completes_at: Option<SystemTime>,
    /// One of `InitiatedBy`'s values; see [`INITIATED_BY_CUSTOMER`].
    pub initiated_by: String,
    /// The option values the domain held BEFORE this change, for a change a cancel
    /// can put back — recorded by [`Domain::record_revert`] and `None` for a change
    /// there is no going back from (the one a create starts, an engine version
    /// upgrade), which is therefore not cancellable.
    pub revert_to: Option<DomainOptions>,
    /// Whether this change was cancelled before it landed.
    pub cancelled: bool,
}

impl ConfigChange {
    /// Materialise the completion the clock owes this change, if `now` is past its
    /// due instant.
    ///
    /// The completion is dated at the DUE instant rather than at `now`, so the
    /// `LastUpdatedTime` a caller reads does not move with the moment it happened to
    /// look.
    pub(super) fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.completes_at else {
            return;
        };
        if now < due {
            return;
        }
        self.completes_at = None;
        self.last_updated_at = due;
    }

    /// Whether this change is still being applied.
    pub fn in_progress(&self) -> bool {
        self.completes_at.is_some()
    }

    /// Whether cancelling this change is something the service can do: it is still
    /// being applied AND the configuration it started from was recorded, so
    /// cancelling it puts that configuration back rather than leaving the change
    /// applied under another name.
    pub fn cancellable(&self) -> bool {
        self.in_progress() && self.revert_to.is_some()
    }

    /// `ChangeProgressStatusDetails$Status`: one of `OverallChangeStatus`'s values.
    pub fn status(&self) -> &'static str {
        if self.in_progress() {
            CHANGE_STATUS_PROCESSING
        } else if self.cancelled {
            CHANGE_STATUS_FAILED
        } else {
            CHANGE_STATUS_COMPLETED
        }
    }

    /// `ConfigChangeStatus` for this change: the emulator applies a change in one
    /// step, so it is `ApplyingChanges` until the change falls due and `Completed`
    /// after — or `Cancelled` where a cancel stopped it before it landed.
    pub fn config_change_status(&self) -> &'static str {
        if self.in_progress() {
            CONFIG_CHANGE_APPLYING_CHANGES
        } else if self.cancelled {
            CONFIG_CHANGE_CANCELLED
        } else {
            CONFIG_CHANGE_COMPLETED
        }
    }

    /// This change as `ChangeProgressStatusDetails` — what
    /// `DescribeDomainChangeProgress` answers with.
    pub fn to_progress_status(&self) -> ChangeProgressStatusDetails {
        // A property of the change in flight is pending; one of a change that has
        // landed is completed. The two lists partition `properties`, so no property
        // is ever reported as both — and a cancelled change landed none of them, so
        // neither list holds any.
        let (pending, completed) = match (self.in_progress(), self.cancelled) {
            (true, _) => (self.properties.clone(), Vec::new()),
            (false, true) => (Vec::new(), Vec::new()),
            (false, false) => (Vec::new(), self.properties.clone()),
        };
        ChangeProgressStatusDetails {
            change_id: Some(self.change_id.clone()),
            start_time: Some(datetime(self.started_at)),
            status: Some(self.status().to_string()),
            pending_properties: Some(pending),
            completed_properties: Some(completed),
            // OpenSearch Service's blue/green deployment stages are not published,
            // and this emulator applies a configuration change in ONE step rather
            // than walking a sequence of stages — so no stage list is invented and
            // the total stays at the member's own modelled default.
            total_number_of_stages: 0,
            change_progress_stages: None,
            last_updated_time: Some(datetime(self.last_updated_at)),
            config_change_status: Some(self.config_change_status().to_string()),
            initiated_by: Some(self.initiated_by.clone()),
            // Validation failures come from the pre-update validation check a verbose
            // dry run runs, which reports them on the dry run's OWN record
            // ([`DryRun::to_progress_status`], read by `DescribeDryRunProgress`)
            // rather than on the change a later call applied.
            validation_failures: None,
            // Advisory warnings are accepted through the same verbose dry run.
            accepted_warnings: None,
        }
    }

    /// This change as the `ChangeProgressDetails` every full descriptor of the domain
    /// (`DomainStatus`, `DomainConfig`) carries, so the change a descriptor names is
    /// the same change `DescribeDomainChangeProgress` reports.
    pub fn to_details(&self) -> ChangeProgressDetails {
        ChangeProgressDetails {
            change_id: Some(self.change_id.clone()),
            // `Message` is "a message corresponding to the status of the
            // configuration change"; the service's own wording for one is not
            // published, and the status itself is reported in
            // `ConfigChangeStatus`, so none is invented.
            message: None,
            config_change_status: Some(self.config_change_status().to_string()),
            initiated_by: Some(self.initiated_by.clone()),
            start_time: Some(datetime(self.started_at)),
            last_updated_time: Some(datetime(self.last_updated_at)),
        }
    }
}

/// How many verbose dry runs one domain keeps, the same bound and for the same
/// reason as [`CHANGE_HISTORY`]: a caller may ask for one before every update it is
/// considering, so the record set a domain holds has to be bounded.
pub const DRY_RUN_HISTORY: usize = 50;

/// `DryRunMode`'s `Verbose` value (one of [`DRY_RUN_MODE_VALUES`]): the mode that
/// "runs an additional check to validate the changes you're making", which is the
/// analysis `DescribeDryRunProgress` reports the progress of. `Basic` "only returns
/// the type of deployment (blue/green or dynamic) that the update will cause", which
/// the update call answers from its own analysis, so it starts no run and leaves
/// nothing to report on.
pub const DRY_RUN_MODE_VERBOSE: &str = "Verbose";

/// `DryRunProgressStatus$DryRunStatus` while the validation check the dry run runs
/// has not finished — the status the call that started it answers with, so a caller
/// polling straight after it still observes the analysis in flight.
pub const DRY_RUN_STATUS_PENDING: &str = "PENDING";

/// `DryRunProgressStatus$DryRunStatus` once that check has finished.
pub const DRY_RUN_STATUS_COMPLETED: &str = "COMPLETED";

/// One verbose dry run on a domain: the analysis an `UpdateDomainConfig` carrying
/// `DryRun` with `DryRunMode` [`DRY_RUN_MODE_VERBOSE`] started, which
/// `DescribeDryRunProgress` reports the progress, the configuration and the results
/// of.
///
/// A dry run applies NOTHING: it is "a pre-update dry run analysis", so the record
/// holds the configuration the change WOULD have left the domain in — computed by
/// the same merge the real update performs, on a copy of the domain — beside the
/// deployment kind that change would need. It is [`DRY_RUN_STATUS_PENDING`] from the
/// call that started it until the span the clock owes the analysis has passed
/// ([`PROVISIONING`]), and
/// [`DRY_RUN_STATUS_COMPLETED`] after — materialised by whichever later call reads
/// the record ([`Self::settle`]), so nothing sleeps.
#[derive(Debug, Clone)]
pub struct DryRun {
    /// `DryRunProgressStatus$DryRunId`: the GUID this run is addressed by.
    pub dry_run_id: String,
    /// `CreationDate`: the instant of the call that started the run.
    pub started_at: SystemTime,
    /// `UpdateDate`: the instant this run's status last moved.
    pub last_updated_at: SystemTime,
    /// When the clock owes this run its conclusion; `None` once it has concluded.
    pub completes_at: Option<SystemTime>,
    /// The `DomainStatus` of the configuration the change would leave the domain in
    /// — "details about the changes you're planning to make on the domain", which
    /// `DescribeDryRunProgress` answers as `DryRunConfig`.
    ///
    /// A projection rather than a domain record, because it describes a
    /// configuration that was never applied and never will be by this run: there is
    /// no live domain for it to be re-derived from, so it is held exactly as the
    /// analysis computed it.
    pub config: DomainStatus,
    /// `DryRunResults$DeploymentType`: which of the four documented deployment kinds
    /// applying the change would be, as the update path's own analysis decided.
    pub deployment_type: String,
    /// `AcceptedWarnings`: "the list of advisory warning codes that were accepted
    /// for the configuration change", as the request that started the run carried
    /// them.
    pub accepted_warnings: Vec<String>,
}

impl DryRun {
    /// Materialise the conclusion the clock owes this run, if `now` is past its due
    /// instant.
    ///
    /// Dated at the DUE instant rather than at `now`, for the same reason a
    /// configuration change's completion is ([`ConfigChange::settle`]): the
    /// `UpdateDate` a caller reads does not move with the moment it happened to look.
    pub(super) fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.completes_at else {
            return;
        };
        if now < due {
            return;
        }
        self.completes_at = None;
        self.last_updated_at = due;
    }

    /// Whether the analysis is still running.
    pub fn in_progress(&self) -> bool {
        self.completes_at.is_some()
    }

    /// `DryRunProgressStatus$DryRunStatus` for this run.
    pub fn status(&self) -> &'static str {
        if self.in_progress() {
            DRY_RUN_STATUS_PENDING
        } else {
            DRY_RUN_STATUS_COMPLETED
        }
    }

    /// This run as `DryRunProgressStatus` — what the call that starts it and
    /// `DescribeDryRunProgress` both answer with, so the two can never report one run
    /// differently.
    pub fn to_progress_status(&self) -> DryRunProgressStatus {
        DryRunProgressStatus {
            dry_run_id: self.dry_run_id.clone(),
            dry_run_status: self.status().to_string(),
            creation_date: datetime_text(self.started_at),
            update_date: datetime_text(self.last_updated_at),
            // "The validation failures that occurred as a result of the dry run": a
            // configuration this service cannot apply is refused by the update path
            // itself, under the modelled error class for the rule it broke, and only
            // a configuration that passed every one of those checks reaches a dry
            // run — so a run that concluded found no failure, and one still running
            // has found none yet.
            validation_failures: Some(Vec::new()),
            accepted_warnings: Some(self.accepted_warnings.clone()),
        }
    }

    /// This run as `DryRunResults` — the same answer the call that started it gave,
    /// from the same stored analysis.
    pub fn to_results(&self) -> DryRunResults {
        DryRunResults {
            deployment_type: Some(self.deployment_type.clone()),
            // "A message corresponding to the deployment type": this service's own
            // wording for one is not published, and the deployment kind is already
            // reported in `DeploymentType`, so none is invented.
            message: None,
        }
    }
}
