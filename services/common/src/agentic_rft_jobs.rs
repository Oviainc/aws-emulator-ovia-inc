// SPDX-License-Identifier: CC0-1.0

//! The JOB DIRECTORY: which SageMaker jobs the control plane has created, so the job
//! runtime data plane can resolve the job ARN its callers carry.
//!
//! A SageMaker job of a reinforcement-fine-tuning category is CREATED by the SageMaker
//! control plane (`CreateJob`: its name, its category, the configuration document and
//! the execution role) and USED by the AgenticRFT job runtime data plane (`Sample`,
//! `SampleWithResponseStream`), whose own API carries none of that definition: a runtime
//! request names a job by ARN in a header and nothing else. So the data plane can only
//! know whether that ARN denotes a job at all because the control plane RECORDED it.
//!
//! This is that record: ONE directory in the shared engine, written by the control plane
//! when it creates a job (and cleared when the job is deleted), read by the data plane on
//! every sampling call. Nothing here names a wire shape: the directory holds the job's
//! name, its category and the ARN the control plane minted, and a reader projects those
//! onto the shapes ITS model declares.
//!
//! Like [`crate::feature_store_groups`], an ARN this directory does not hold is a FACT
//! and not a guess: a job exists only because a create minted it, so the data plane
//! refuses an ARN with no registration rather than treating it as undecided. The
//! directory is BOUNDED all the same — see [`MAX_REGISTERED_JOBS`] — so a caller looping
//! on create cannot grow it without end; the oldest registration is dropped first, which
//! keeps the jobs a live workflow just created resolvable.

use std::collections::{HashMap, VecDeque};

/// How many jobs the directory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this directory lives in memory for the lifetime
/// of the process, so it needs a maximum count. It is an order of magnitude above the
/// count the SageMaker control plane's own create admits, so while that bound stands no
/// job the control plane still holds can be evicted from here. Were it ever to admit
/// more, eviction drops the OLDEST registration first — never a newer one — so the jobs a
/// live workflow just created stay resolvable and only the longest-standing ones could
/// become unresolvable to the data plane.
pub const MAX_REGISTERED_JOBS: usize = 10_000;

/// The `<type>` component of a job's ARN, from the control plane's own `JobArn` pattern
/// (`^arn:(aws[a-z\-]*):sagemaker:[a-z0-9\-]+:[0-9]{12}:job/[a-zA-Z0-9]+/…`) and from the
/// job runtime's (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/…`).
const JOB_RESOURCE_TYPE: &str = "job";

/// One job the control plane created, as the directory records it.
#[derive(Debug, Clone)]
pub struct JobRecord {
    /// The `JobName` the job was created with.
    pub name: String,
    /// The `JobCategory` it was created under, which is the segment the job's ARN carries
    /// between the resource type and the name.
    pub category: String,
    /// The `JobArn` the control plane minted for it, which is also what this record's key
    /// is derived from.
    pub arn: String,
}

impl JobRecord {
    /// The `<category>/<name>` pair this record is keyed by: the resource path of the
    /// job's ARN, which is the only part of an ARN that identifies a job rather than the
    /// partition, Region or account it was addressed through.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}/{}", self.category, self.name)
    }
}

/// The directory itself: `<category>/<name>` -> the job the control plane created.
///
/// Registered in the shared engine under THIS type, so the control plane that creates a
/// job and the data plane that serves its rollouts read one directory instead of each
/// keeping its own copy.
#[derive(Debug, Default)]
pub struct JobDirectory {
    /// The records, by the `<category>/<name>` pair [`JobRecord::key`] builds.
    jobs: HashMap<String, JobRecord>,
    /// Registration order, so the ceiling drops the oldest entry first.
    order: VecDeque<String>,
}

impl JobDirectory {
    /// Record `job` under its own `<category>/<name>` pair.
    ///
    /// Re-registering a pair REPLACES its record (the control plane correcting what it
    /// told the directory) and does not move its place in the eviction order.
    pub fn register(&mut self, job: JobRecord) {
        let key = job.key();
        if self.jobs.insert(key.clone(), job).is_none() {
            self.order.push_back(key);
            while self.order.len() > MAX_REGISTERED_JOBS {
                if let Some(oldest) = self.order.pop_front() {
                    self.jobs.remove(&oldest);
                }
            }
        }
    }

    /// Drop the record of the job `category` / `name` names: the job is gone, so the
    /// directory no longer claims it exists.
    pub fn forget(&mut self, category: &str, name: &str) {
        let key = format!("{category}/{name}");
        if self.jobs.remove(&key).is_some() {
            self.order.retain(|held| held != &key);
        }
    }

    /// The job the ARN `job_arn` names, or `None` when no job of that category and name
    /// was ever created.
    ///
    /// Only a well-formed ARN naming a `job` resolves (read through
    /// [`crate::arn::resource_id_of_type`], this crate's ONE parse of a caller-supplied
    /// resource ARN), so a value carrying slashes for some other reason cannot be
    /// mistaken for an ARN's tail. Two ARNs that differ only in partition, Region or
    /// account collapse onto one record, which is correct for an emulator serving a
    /// single account and Region.
    #[must_use]
    pub fn get_by_arn(&self, job_arn: &str) -> Option<&JobRecord> {
        self.jobs
            .get(crate::arn::resource_id_of_type(job_arn, JOB_RESOURCE_TYPE)?)
    }

    /// The job `category` / `name` names, as the control plane addresses one.
    #[must_use]
    pub fn get(&self, category: &str, name: &str) -> Option<&JobRecord> {
        self.jobs.get(&format!("{category}/{name}"))
    }

    /// How many jobs the directory holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    /// Whether the directory holds no job at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One registration, named.
    fn job(category: &str, name: &str) -> JobRecord {
        JobRecord {
            name: name.to_owned(),
            category: category.to_owned(),
            arn: format!("arn:aws:sagemaker:us-east-1:000000000000:job/{category}/{name}"),
        }
    }

    #[test]
    fn a_registered_job_resolves_by_arn_whatever_partition_region_or_account_addressed_it() {
        let mut directory = JobDirectory::default();
        directory.register(job("AgentRFT", "rollout-1"));

        // (case, the ARN a data-plane request carries, whether it resolves)
        let rows = [
            (
                "the ARN the control plane minted",
                "arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/rollout-1".to_owned(),
                true,
            ),
            (
                "another Region and account",
                "arn:aws:sagemaker:eu-west-1:123456789012:job/AgentRFT/rollout-1".to_owned(),
                true,
            ),
            (
                "another partition",
                "arn:aws-cn:sagemaker:cn-north-1:123456789012:job/AgentRFT/rollout-1".to_owned(),
                true,
            ),
            (
                "a name never created",
                "arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/rollout-2".to_owned(),
                false,
            ),
            (
                "the created name under another category",
                "arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFTEvaluation/rollout-1"
                    .to_owned(),
                false,
            ),
            (
                "an ARN of another resource type",
                "arn:aws:sagemaker:us-east-1:000000000000:endpoint/AgentRFT/rollout-1".to_owned(),
                false,
            ),
            (
                "a value that merely carries slashes",
                "job/AgentRFT/rollout-1".to_owned(),
                false,
            ),
        ];
        for (case, spelling, expected) in rows {
            let actual = directory.get_by_arn(&spelling).is_some();
            assert_eq!(
                actual, expected,
                "case {case:?} ({spelling}): expected resolves={expected}, got {actual}"
            );
        }
        let held = directory
            .get("AgentRFT", "rollout-1")
            .expect("the registered job");
        assert_eq!(
            (held.category.as_str(), held.name.as_str()),
            ("AgentRFT", "rollout-1"),
            "case the registered job: expected the record it was registered with, got {held:?}"
        );
    }

    #[test]
    fn re_registering_a_job_replaces_its_record_without_growing_the_directory() {
        let mut directory = JobDirectory::default();
        directory.register(job("AgentRFT", "rollout-1"));
        let mut updated = job("AgentRFT", "rollout-1");
        updated.arn = "arn:aws:sagemaker:eu-west-1:000000000000:job/AgentRFT/rollout-1".to_owned();
        directory.register(updated);

        assert_eq!(
            directory.len(),
            1,
            "case a re-registration: expected one record, got {}",
            directory.len()
        );
        assert_eq!(
            directory
                .get("AgentRFT", "rollout-1")
                .map(|held| held.arn.as_str()),
            Some("arn:aws:sagemaker:eu-west-1:000000000000:job/AgentRFT/rollout-1"),
            "case a re-registration: expected the newest record to stand"
        );
    }

    #[test]
    fn forgetting_a_job_drops_it_and_leaves_the_others() {
        let mut directory = JobDirectory::default();
        directory.register(job("AgentRFT", "kept"));
        directory.register(job("AgentRFT", "dropped"));
        directory.register(job("AgentRFTEvaluation", "kept"));

        directory.forget("AgentRFT", "dropped");
        assert!(
            directory.get("AgentRFT", "dropped").is_none()
                && directory.get("AgentRFT", "kept").is_some()
                && directory.get("AgentRFTEvaluation", "kept").is_some(),
            "case one job forgotten: expected only it to go, got {} record(s)",
            directory.len()
        );

        // Forgetting a job the directory does not hold changes nothing, so a repeated
        // delete is harmless.
        directory.forget("AgentRFT", "dropped");
        directory.forget("AgentRFT", "never-created");
        assert_eq!(
            directory.len(),
            2,
            "case forgetting an absent job: expected the remaining records untouched, got {}",
            directory.len()
        );
    }

    #[test]
    fn the_ceiling_drops_the_oldest_registration_first() {
        let mut directory = JobDirectory::default();
        for index in 0..MAX_REGISTERED_JOBS {
            directory.register(job("AgentRFT", &format!("run{index}")));
        }
        assert_eq!(
            directory.len(),
            MAX_REGISTERED_JOBS,
            "case a directory filled to the ceiling: expected {MAX_REGISTERED_JOBS} records, got {}",
            directory.len()
        );

        // A re-registration is not a new entry, so it moves nothing out.
        directory.register(job("AgentRFT", "run0"));
        assert!(
            directory.get("AgentRFT", "run0").is_some() && directory.len() == MAX_REGISTERED_JOBS,
            "case a re-registration at the ceiling: expected the record kept, got {} record(s)",
            directory.len()
        );

        // One more NEW job evicts the oldest and only the oldest.
        directory.register(job("AgentRFT", &format!("run{MAX_REGISTERED_JOBS}")));
        assert_eq!(
            directory.len(),
            MAX_REGISTERED_JOBS,
            "case one past the ceiling: expected the ceiling held, got {}",
            directory.len()
        );
        assert!(
            directory.get("AgentRFT", "run0").is_none(),
            "case one past the ceiling: expected the oldest registration dropped"
        );
        assert!(
            directory.get("AgentRFT", "run1").is_some()
                && directory
                    .get("AgentRFT", &format!("run{MAX_REGISTERED_JOBS}"))
                    .is_some(),
            "case one past the ceiling: expected the newest registrations kept"
        );
    }
}
