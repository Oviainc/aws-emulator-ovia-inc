// SPDX-License-Identifier: CC0-1.0

//! The network-migration JOB concern of this service's state: the jobs a `Start*`
//! operation of the network-migration pipeline records against an execution, and the
//! refusals those operations word.
//!
//! A job is the unit of work one pipeline stage runs for one execution. It is recorded
//! when the stage is started and carries the instant the work it runs is due to have
//! finished, so its status is READ from the stored instants rather than from a timer:
//! two reads of the same job at the same instant answer the same status, and nothing
//! has to run to move it on.

use std::time::{Duration, SystemTime};

use super::*;

/// The most network migration mapping jobs this store holds at a time — per account AND
/// in total.
///
/// The emulator's own bound on the mapping-job growth site, not a claim about any AWS
/// quota: the same reasoning as [`APPLICATION_QUOTA`]. Whatever records a job checks
/// [`ComAmazonawsMgnApplicationmigrationserviceState::network_migration_mapping_job_store_is_full`]
/// before minting anything and refuses past it with the modelled
/// `ServiceQuotaExceededException`, so the record count cannot be grown past the bound
/// by varying the request's account.
pub const NETWORK_MIGRATION_MAPPING_JOB_QUOTA: usize = 1_000;

/// How long the mapping work a job runs takes before the clock owes its completion.
///
/// The emulator's own span for a transition it owes rather than a figure AWS documents:
/// a real mapping run takes as long as the source export takes. It is never zero, so a
/// caller that reads the job straight after starting it still sees the work running.
pub const MAPPING_JOB_DURATION: Duration = Duration::from_secs(3);

/// The `NetworkMigrationJobStatus` a job carries while the work it runs is still due,
/// taken from the model's own generated value list.
pub const NETWORK_MIGRATION_JOB_STATUS_STARTED: &str = network_migration_job_status("STARTED");

/// The `NetworkMigrationJobStatus` a job carries once the work it runs is due to have
/// finished, likewise.
pub const NETWORK_MIGRATION_JOB_STATUS_SUCCEEDED: &str = network_migration_job_status("SUCCEEDED");

/// The `NetworkMigrationJobStatus` value the model spells `wanted`, resolved at compile
/// time against `ops::types::NETWORK_MIGRATION_JOB_STATUS_VALUES` (generated from the
/// Smithy model), exactly as [`super::VALIDATION_FIELD_FAILED`] is: a name the model does
/// not declare is a compile error, not a wire value no client would recognise.
const fn network_migration_job_status(wanted: &str) -> &str {
    let declared = crate::ops::types::NETWORK_MIGRATION_JOB_STATUS_VALUES;
    let mut index = 0;
    while index < declared.len() {
        if same_ascii(declared[index], wanted) {
            return declared[index];
        }
        index += 1;
    }
    panic!("the model declares no such NetworkMigrationJobStatus")
}

/// One stored network migration mapping job: the mapping run `StartNetworkMigrationMapping`
/// recorded for one execution of one definition.
///
/// The status is NOT stored: it follows from [`Self::completes_at`], the instant the
/// clock owes this job's completion at, so no read has to move it on and two reads at
/// the same instant answer the same status.
#[derive(Debug, Clone)]
pub struct NetworkMigrationMappingJob {
    pub job_id: String,
    /// The execution this job runs a stage of.
    pub network_migration_execution_id: String,
    /// The definition that execution belongs to.
    pub network_migration_definition_id: String,
    /// The account the job lives in, so one account's jobs are never another's to read.
    pub account_id: String,
    /// The request's `securityGroupMappingStrategy`: one of the model's
    /// `SecurityGroupMappingStrategy` values, or absent when the request named none.
    pub security_group_mapping_strategy: Option<String>,
    pub created_at: SystemTime,
    /// The instant this job's work is due to have finished: until then the job is
    /// running, from then on it has succeeded.
    pub completes_at: SystemTime,
}

impl NetworkMigrationMappingJob {
    /// The job a mapping start records: minted for `execution_id` under `definition_id`
    /// in `account_id`, running from `at` and due to have finished one
    /// [`MAPPING_JOB_DURATION`] later.
    pub fn started(
        job_id: String,
        definition_id: &str,
        execution_id: &str,
        account_id: &str,
        security_group_mapping_strategy: Option<String>,
        at: SystemTime,
    ) -> Self {
        Self {
            job_id,
            network_migration_execution_id: execution_id.to_string(),
            network_migration_definition_id: definition_id.to_string(),
            account_id: account_id.to_string(),
            security_group_mapping_strategy,
            created_at: at,
            completes_at: at + MAPPING_JOB_DURATION,
        }
    }

    /// Whether this job's work is still running at `now`: the clock has not reached the
    /// instant its completion is owed at.
    pub fn is_running(&self, now: SystemTime) -> bool {
        now < self.completes_at
    }

    /// This job's `status` at `now`: one of the model's `NetworkMigrationJobStatus`
    /// values, read from the stored instants.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.is_running(now) {
            NETWORK_MIGRATION_JOB_STATUS_STARTED
        } else {
            NETWORK_MIGRATION_JOB_STATUS_SUCCEEDED
        }
    }

    /// The instant this job ended, or `None` while its work is still running.
    pub fn ended_at(&self, now: SystemTime) -> Option<SystemTime> {
        if self.is_running(now) {
            None
        } else {
            Some(self.completes_at)
        }
    }
}

impl ComAmazonawsMgnApplicationmigrationserviceState {
    /// Every mapping job recorded for the execution `execution_id` names under
    /// `definition_id` within `account_id`, in creation order. A job of another account,
    /// of another definition or of another execution is not part of this selection.
    pub fn network_migration_mapping_jobs_of(
        &self,
        account_id: &str,
        definition_id: &str,
        execution_id: &str,
    ) -> Vec<&NetworkMigrationMappingJob> {
        self.network_migration_mapping_job_order
            .iter()
            .filter_map(|id| self.network_migration_mapping_jobs.get(id))
            .filter(|job| {
                job.account_id == account_id
                    && job.network_migration_definition_id == definition_id
                    && job.network_migration_execution_id == execution_id
            })
            .collect()
    }

    /// The mapping job still running for that execution at `now`, or `None` when none
    /// is: the job a retry of the call that started it answers, and the one a start of
    /// conflicting mapping work conflicts with.
    pub fn running_network_migration_mapping_job(
        &self,
        account_id: &str,
        definition_id: &str,
        execution_id: &str,
        now: SystemTime,
    ) -> Option<&NetworkMigrationMappingJob> {
        self.network_migration_mapping_jobs_of(account_id, definition_id, execution_id)
            .into_iter()
            .rev()
            .find(|job| job.is_running(now))
    }

    /// A fresh `NetworkMigrationJobID` no stored job holds: 32 `[0-9a-f]` characters
    /// grouped 8-4-4-4-12, as the model's `NetworkMigrationJobID` pattern requires.
    pub fn mint_network_migration_job_id(&self) -> String {
        loop {
            let candidate = mint_uuid_shaped_id();
            if !self.network_migration_mapping_jobs.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Whether the mapping-job store is full: either the account already holds
    /// [`NETWORK_MIGRATION_MAPPING_JOB_QUOTA`] jobs, or the whole store does. Checked
    /// before a start mints anything, so neither the per-account count nor the total can
    /// be grown past the bound.
    pub fn network_migration_mapping_job_store_is_full(&self, account_id: &str) -> bool {
        self.network_migration_mapping_jobs.len() >= NETWORK_MIGRATION_MAPPING_JOB_QUOTA
            || self
                .network_migration_mapping_jobs
                .values()
                .filter(|job| job.account_id == account_id)
                .count()
                >= NETWORK_MIGRATION_MAPPING_JOB_QUOTA
    }

    /// Store a network migration mapping job, remembering its place in creation order.
    ///
    /// Answers whether it was stored, exactly as [`Self::insert_application`] does: a
    /// store already at [`NETWORK_MIGRATION_MAPPING_JOB_QUOTA`] refuses rather than
    /// growing past the bound, and re-storing a known id keeps its place in the creation
    /// order rather than duplicating it.
    pub fn insert_network_migration_mapping_job(
        &mut self,
        job: NetworkMigrationMappingJob,
    ) -> bool {
        let known = self.network_migration_mapping_jobs.contains_key(&job.job_id);
        if !known
            && self.network_migration_mapping_jobs.len() >= NETWORK_MIGRATION_MAPPING_JOB_QUOTA
        {
            return false;
        }
        if !known {
            self.network_migration_mapping_job_order
                .push(job.job_id.clone());
        }
        self.network_migration_mapping_jobs
            .insert(job.job_id.clone(), job);
        true
    }
}

/// The modelled refusal for a start that would run mapping work on an execution a
/// mapping job is already running for, under a different `securityGroupMappingStrategy`
/// than that job runs: the request conflicts with the execution's current state.
pub fn network_migration_mapping_job_in_progress(
    execution_id: &str,
    job_id: &str,
) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Network migration execution {execution_id} is already running mapping job {job_id}."
        )),
        code: None,
        resource_id: Some(execution_id.to_string()),
        resource_type: Some(NETWORK_MIGRATION_EXECUTION_RESOURCE_TYPE.to_string()),
        errors: None,
    }
}

/// The modelled refusal for an account already holding
/// [`NETWORK_MIGRATION_MAPPING_JOB_QUOTA`] network migration mapping jobs, so no
/// further mapping run can be recorded.
pub fn network_migration_mapping_job_quota_exceeded(
    account_id: &str,
) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "Account {account_id} already holds the maximum of \
             {NETWORK_MIGRATION_MAPPING_JOB_QUOTA} network migration mapping jobs."
        )),
        code: None,
        resource_id: None,
        resource_type: Some(NETWORK_MIGRATION_EXECUTION_RESOURCE_TYPE.to_string()),
        service_code: Some(ARN_NAMESPACE.to_string()),
        quota_code: None,
        quota_value: i32::try_from(NETWORK_MIGRATION_MAPPING_JOB_QUOTA).ok(),
    }
}
