// SPDX-License-Identifier: CC0-1.0

//! The sentences this service refuses a request with.
//!
//! An error's HTTP status and code come from the model; its MESSAGE does not, so each
//! sentence AppFlow words is stated ONCE here and built by whichever operation raises it.
//! That is what keeps two operations refusing the same thing from wording it differently
//! — the flow a read cannot find and the flow a run cannot find are the same absence, and
//! say so with the same sentence.
//!
//! Every sentence below is FIXED text plus the values the request and the store supply:
//! the flow name a request named, never anything about one particular call.

/// How AppFlow opens the message of a request it refuses as a conflict with the state the
/// named resource is in.
///
/// Shared by every [`crate::ops::types::ConflictException`] whose message states WHICH
/// conflict it is, so the opening cannot drift between them. The one conflict that is not
/// about a resource's state — two runs of one flow racing
/// ([`CONCURRENT_RUN_CONFLICT`]) — states the race itself and carries no resource at all.
const CONFLICT_OPENING: &str = "Conflict executing request: ";

/// The message of the `ResourceNotFoundException` every operation that ADDRESSES a flow by
/// name answers when the account holds no such flow.
///
/// One sentence for the whole class: a flow that was never created, and one a `DeleteFlow`
/// has taken away, are the same absence to the caller, so `DescribeFlow`, `StartFlow`,
/// `StopFlow`, `UpdateFlow`, `DeleteFlow`, `CancelFlowExecutions` and
/// `DescribeFlowExecutionRecords` all state it this way. The flow name is the only
/// variable part and is taken verbatim from the request, at the END of the sentence, which
/// is where a client parsing it out looks.
pub fn no_such_flow(flow_name: &str) -> String {
    format!("There is no flow with the name {flow_name}")
}

/// The message of the `ResourceNotFoundException` a request that names a connector profile
/// the account does not hold is refused with.
///
/// The profile name is the only variable part and comes from the request. Same shape as
/// [`no_such_flow`]: the resource kind, then the name it was addressed by.
pub fn no_such_connector_profile(connector_profile_name: &str) -> String {
    format!("Connector Profile with name {connector_profile_name} does not exist.")
}

/// The message of the `ConflictException` a `CreateFlow` is refused with when the account
/// already holds a flow of that name — "a flow with the given name already exists within
/// the account" (`smithy_query(query="shape", shape="ConflictException")`).
///
/// The flow name is the only variable part and is taken verbatim from the request.
pub fn flow_name_already_exists(flow_name: &str) -> String {
    format!("{CONFLICT_OPENING}Flow with name {flow_name} already exists.")
}

/// The message of the `ConflictException` an unforced `DeleteFlow` is refused with while
/// the flow is active, naming the two ways the caller can proceed: ask for the delete to
/// be forced, or deactivate the flow first.
pub fn flow_is_active(flow_name: &str) -> String {
    format!(
        "{CONFLICT_OPENING}Flow is in active state, please set forceDelete to true or \
         deactivate the flow: {flow_name}"
    )
}

/// The message of the `ConflictException` a `StopFlow` is refused with when there is no
/// armed trigger left to deactivate — a flow that runs only on demand, or one whose
/// trigger is already disarmed.
pub fn flow_already_deactivated(flow_name: &str) -> String {
    format!("{CONFLICT_OPENING}Flow is already deactivated: {flow_name}")
}

/// The message of the `ConflictException` a `StartFlow` is refused with when the flow's
/// trigger is already armed, so there is nothing left for the call to activate.
pub fn flow_already_activated(flow_name: &str) -> String {
    format!("{CONFLICT_OPENING}Flow is already activated: {flow_name}")
}

/// The message of the `ConflictException` the LOSER of two concurrent `StartFlow` calls on
/// one flow is refused with: the conflict is between the two calls rather than with the
/// state of a resource, so the sentence names neither the flow nor the run that won — it
/// has no variable part at all.
pub const CONCURRENT_RUN_CONFLICT: &str = "Conflicts detected while executing request";
