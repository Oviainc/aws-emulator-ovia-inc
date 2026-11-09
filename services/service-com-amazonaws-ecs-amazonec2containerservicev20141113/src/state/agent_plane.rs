// SPDX-License-Identifier: CC0-1.0

//! The AGENT plane: what a `Submit*StateChange` report may say, and whose report it can be.
//!
//! Those operations are the container agent's — "This action is only used by the Amazon ECS
//! agent, and it is not intended for use outside of the agent" — and Amazon ECS holds a
//! report to two rules before it records anything:
//!
//! * the VALUES it carries are the ones the plane accepts. `SubmitTaskStateChange$status` and
//!   `AttachmentStateChange$status` are plain strings in the model, so the accepted set is
//!   recorded behavior: a task is reported at one of the stages of the ECS task lifecycle
//!   ([`AGENT_REPORT_TASK_STATUSES`], and a value outside it is refused naming the value
//!   itself) and an attachment is reported ATTACHED and nothing else
//!   ([`AGENT_REPORT_ATTACHMENT_STATUSES`], refused with the sentence this service words
//!   every out-of-set value with, [`super::value_set_message`]) — the one transition the
//!   agent makes, from the `ATTACHING` Amazon ECS leaves the interface at;
//! * the CALLER is the agent that hosts the task ([`agent_plane_denies_report`]).
//!
//! The values are checked FIRST, so a report that cannot be read at all is refused for what
//! it says rather than for who sent it.

use super::*;

/// The stages a task may be REPORTED at by its agent: the ECS task lifecycle, in the order a
//  task passes through it.
///
/// `SubmitTaskStateChange$status` is "The status of the state change request" with no
/// modelled value set, so this is the lifecycle the Amazon ECS developer guide publishes
/// ("Task lifecycle"): the stages this service also stores and ranks a retry against
/// ([`TASK_LIFECYCLE`]), plus the ones only the agent observes between them.
pub const AGENT_REPORT_TASK_STATUSES: [&str; 8] = [
    STATUS_PROVISIONING,
    STATUS_PENDING,
    "ACTIVATING",
    STATUS_RUNNING,
    STATUS_DEACTIVATING,
    "STOPPING",
    "DEPROVISIONING",
    STATUS_STOPPED,
];

/// The states an ATTACHMENT may be reported in: `ATTACHED` alone.
///
/// Amazon ECS creates the interface and takes it as far as `ATTACHING` itself; the agent is
/// what reports the attach it completed, so the one state change it sends is to
/// [`ATTACHMENT_STATUS_ATTACHED`]. Every other `Attachment$status` value is a state Amazon
/// ECS itself puts the attachment in, not one a report may name.
pub const AGENT_REPORT_ATTACHMENT_STATUSES: [&str; 1] = [ATTACHMENT_STATUS_ATTACHED];

/// The member `AttachmentStateChange$status` is named by in the refusal of a value outside
/// [`AGENT_REPORT_ATTACHMENT_STATUSES`].
pub const AGENT_REPORT_ATTACHMENT_STATUS_MEMBER: &str = "attachmentStatus";

/// `InvalidParameterException$message` for a task status a report names that is not one of
/// [`AGENT_REPORT_TASK_STATUSES`].
///
/// Amazon ECS states the value it could not read rather than the set it accepts, so the
/// variable part is the status the REQUEST carried and the rest is fixed text.
pub fn invalid_task_status_message(status: &str) -> String {
    format!("The status {status} is not a valid status.")
}

/// The refusal a task status a report NAMES earns, or `None` when it names one of
/// [`AGENT_REPORT_TASK_STATUSES`] (or none at all, which reports no stage).
pub fn agent_report_task_status_violation(status: Option<&str>) -> Option<String> {
    let reported = status?;
    (!AGENT_REPORT_TASK_STATUSES.contains(&reported))
        .then(|| invalid_task_status_message(reported))
}

/// The refusal the attachment states a report NAMES earn, or `None` when every entry names
/// one of [`AGENT_REPORT_ATTACHMENT_STATUSES`].
pub fn agent_report_attachment_status_violation<'a>(
    statuses: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    statuses
        .into_iter()
        .any(|status| !AGENT_REPORT_ATTACHMENT_STATUSES.contains(&status))
        .then(|| {
            value_set_message(
                AGENT_REPORT_ATTACHMENT_STATUS_MEMBER,
                &AGENT_REPORT_ATTACHMENT_STATUSES,
            )
        })
}

/// `AccessDeniedException$message` for a state-change report about a task whose hosting
/// agent cannot be the caller.
///
/// Amazon ECS states what the report is missing: the agent reporting a FARGATE task's state
/// has to be an agent whose container instance Amazon ECS verified, and no caller of this
/// API is one. Fixed text, naming nothing the request carried.
pub const AGENT_INSTANCE_NOT_VERIFIED_MESSAGE: &str =
    "Fargate task state changes from the agent must have the instance verified";

/// Whether the agent plane refuses a state-change report about `task` to the caller that
/// sent it.
///
/// The agent that reports a task's state is the one running on the capacity that hosts it. A
/// task on Fargate runs on infrastructure Amazon Web Services manages, so there is no
/// container instance of this account whose identity could be verified for the report: it
/// cannot have come from that task's agent, and is refused as the unauthorized call it is.
///
/// A task on capacity registered to the cluster IS hosted by a container instance the
/// account registered, whose agent is the caller of this plane — so its reports are taken,
/// which is what the agent plane exists for.
pub fn agent_plane_denies_report(task: &TaskRecord) -> bool {
    task.runs_on_fargate()
}
