// SPDX-License-Identifier: CC0-1.0

//! Whether a function's EXECUTION ROLE may make the SET of calls one thing this service does
//! on the customer's behalf takes.
//!
//! Lambda does several things as the function's own execution role rather than as the caller,
//! and each takes more than one call: attaching the function to a VPC takes three network
//! interface calls ([`super::vpc_attachment_authorization`]), and delivering an invocation's
//! log takes three CloudWatch Logs calls ([`super::execution::log_delivery`]). Asking "which
//! of these may the role not make" is therefore the same question in both places, so it is
//! here once.
//!
//! The DECISION itself is not here: it is
//! [`super::event_source_authorization::withholds_call`], the one reading of the documents in
//! force on a role this crate states, so every gate that asks what an execution role may do
//! answers alike. This module only asks it per call of a set, and keeps the rule that nothing
//! is withheld on evidence this emulator does not have: a role the IAM store holds no record
//! for, and a store that cannot be read at all, show nothing and withhold nothing.

use super::event_source_authorization::{withholds_call, IamState};

/// The FIRST of `actions` the role `role_arn` may not perform on `resource`, or `None` when
/// it may perform all of them — or when nothing of the kind can be shown.
///
/// Each entry is the IAM action string the call is named by, paired with whatever the CALLER
/// calls that action: a refusal sentence names the call in the service's own words, which is
/// not always the action string.
pub(super) fn withheld_action(
    iam: &IamState,
    role_arn: &str,
    resource: &str,
    actions: &[(String, &'static str)],
) -> Option<&'static str> {
    actions
        .iter()
        .find(|(action, _)| withholds_call(iam, role_arn, action, resource).unwrap_or(false))
        .map(|(_, named)| *named)
}
