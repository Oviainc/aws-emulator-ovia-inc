// SPDX-License-Identifier: CC0-1.0

//! Where an enabled Apache Airflow log type's logs are PUBLISHED: the CloudWatch Logs log
//! group Amazon MWAA creates for that log type in the caller's account.
//!
//! Enabling a log type asks MWAA to publish that type's Apache Airflow logs, and MWAA
//! publishes them to a log group of its own naming
//! ([`crate::state::log_group_name`]) whose ARN it then reports as
//! `LoggingConfiguration.<Module>.CloudWatchLogGroupArn` — "The Amazon Resource Name (ARN)
//! for the CloudWatch Logs group where the Apache Airflow log type … is published". So a
//! client that enables a log type and then asks CloudWatch Logs for that group
//! (`DescribeLogGroups`, `DescribeLogStreams`, `GetLogEvents`) has to FIND it: a reported
//! ARN that names nothing is a resource no reader can reach, and reporting one is the same
//! defect as advertising an endpoint nothing serves.
//!
//! The group is created in the CloudWatch Logs service's OWN store in the SAME shared
//! engine ([`logs_state`]), through that service's own record constructor
//! ([`::service_com_amazonaws_cloudwatchlogs_logs_20140328::state::LogGroupRecord::new`])
//! and under its own per-Region-account ceiling, so what MWAA created is what
//! CloudWatch Logs' own readers report — exactly the way a published metric goes into the
//! store CloudWatch's readers read ([`super::metrics`]). It is written there directly
//! rather than sent through `logs:CreateLogGroup`, because a create or an update resolves
//! every refusal it owes under THIS service's store lock and answers in one step: going
//! out over the in-process API would mean awaiting with that guard held, and would make a
//! group that already exists (a client may hold a group of that name, and a second call
//! that enables the same log type adopts the one already there) a refusal to work around
//! rather than the state this step owes the caller.
//!
//! What this does NOT do:
//!
//! * nothing is created for a DISABLED log type — it publishes nothing, which is the same
//!   condition under which no `CloudWatchLogGroupArn` is reported for it;
//! * turning a log type OFF leaves its group where it is, and so does deleting the
//!   environment: the Apache Airflow logs already in the group belong to the account, and
//!   nothing documents MWAA deleting a customer's logs;
//! * nothing is written to a group here. These are the groups the environment's own
//!   Apache Airflow components publish into, and no component of this emulator runs
//!   Apache Airflow, so a log event nothing produced is not invented for them.
//!
//! What a group is NOT keyed by in the destination is the REGION: that store holds one log
//! group per NAME for the whole engine, so two environments of one name in two Regions
//! address one group, and the second of them adopts the group the first was given (whose
//! own ARN names the Region it was created in). That is the destination's own identity for
//! a log group — the same place a Region would have to enter for a published metric series
//! ([`super::metrics`]) — and not a claim about the modelled `CloudWatchLogGroupArn`, which
//! this service reports in the Region the environment lives in.

use ::std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

/// The CloudWatch Logs store the log groups of this service's environments live in.
///
/// It is the CloudWatch Logs service's OWN state struct, reached through the SAME shared
/// engine this service's store lives in, so a group created here is one that service's own
/// readers report.
pub type LogsState =
    ::service_com_amazonaws_cloudwatchlogs_logs_20140328::ComAmazonawsCloudwatchlogsLogs20140328State;

/// The CloudWatch Logs store in the SAME shared engine. `None` means the engine carries no
/// such store — a deployment with no CloudWatch Logs for a log group to be created in.
///
/// LOCK ORDER: a caller may hold this service's own store guard while taking the
/// destination's ([`create_missing`]), so that the groups and the record that advertises
/// them are decided together; never the other way round.
pub fn logs_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<LogsState>> {
    logs::service_state(engine)
}

/// The ARN of the log group `name`, as the OWNING service formats it: the form WITH the
/// trailing `:*`, which is the one a log group's own `arn` member carries and the one the
/// `ModuleLoggingConfiguration$CloudWatchLogGroupArn` doc's own example ends in
/// (`…:log-group:airflow-MyMWAAEnvironment-MwaaEnvironment-DAGProcessing:*`).
///
/// Reported ARNs are built through here ([`crate::state::log_group_arn`]) rather than
/// formatted again locally, so what MWAA reports for a log type is what CloudWatch Logs
/// reports for the group created for it.
#[must_use]
pub fn destination_log_group_arn(region: &str, name: &str) -> String {
    logs::log_group_arn(region, name, true)
}

/// Why the log groups an enabled log type publishes to were not created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogGroupRejection {
    /// The destination store holds as many log groups as it may, so there is no room for
    /// the groups this environment's log types would publish to.
    DestinationAtCapacity,
}

/// `at` in milliseconds since the Unix epoch, which is what every CloudWatch Logs timestamp
/// is expressed in (`LogGroup$creationTime`). An instant before the epoch, which no clock
/// reports, reads as the epoch itself.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Create every group of `names` the destination does not hold yet, in `region`, as of
/// `at` — or none of them.
///
/// All or nothing: the room the destination has is judged against the groups this call
/// would OPEN counted together, before any of them is inserted, so a configuration whose
/// groups do not fit leaves none of them behind for the caller to have to clean up.
///
/// A name the store ALREADY holds is left exactly as it is: the group under that name is
/// what this step owes the caller, whether an earlier call of this service created it or
/// the account created it itself, and overwriting it would discard the Apache Airflow logs
/// and the settings (retention, tags, filters) already on it.
///
/// Call it with this service's own store guard HELD, so the groups and the environment
/// record that advertises them are decided together — that is the lock order this pair of
/// stores is taken in ([`logs_state`]), and nothing in the destination reaches back into
/// this service's store.
pub fn create_missing(
    state: &::engine::ServiceState<LogsState>,
    region: &str,
    names: &[String],
    at: SystemTime,
) -> Result<(), LogGroupRejection> {
    if names.is_empty() {
        return Ok(());
    }
    let created_at = epoch_millis(at);
    let mut store = state.lock();
    let opening = names
        .iter()
        .filter(|name| !store.log_groups.contains_key(*name))
        .count();
    if store.log_groups.len() + opening > logs::MAX_LOG_GROUPS {
        return Err(LogGroupRejection::DestinationAtCapacity);
    }
    for name in names {
        store
            .log_groups
            .entry(name.clone())
            .or_insert_with(|| logs::LogGroupRecord::new(region, name, created_at));
    }
    Ok(())
}
