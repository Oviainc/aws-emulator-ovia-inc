// SPDX-License-Identifier: CC0-1.0

//! The sentence this service refuses a change that needs an ACTIVE user group with.
//!
//! `InvalidUserGroupStateFault` is "the user group is not in an active state", and four
//! handlers answer it — the change to a user group itself, and the three changes that
//! ATTACH one to another resource — so the sentence is stated here once rather than in
//! each of them. Its words are the whole difference a client sees: the message is the only
//! place the service reports WHICH status blocked the call, which is what a caller racing
//! two changes, or waiting to retry, reads.
//!
//! The service states such a gate as two sentences: the group's id as the request named it
//! and the status the group carries now as a describe reports it, then the status that
//! class of call admits. The sibling gate for a DELETE states the same shape over the two
//! statuses a delete admits ([`crate::state::user_group_delete_state_refusal`]). Neither
//! template carries anything from one call's values.

use crate::state::{UserGroupRecord, USER_STATUS_ACTIVE};

/// Why `group` cannot take part in a change at this instant, or `None` when it can.
///
/// A group still being created, already carrying a change the clock has not settled, or on
/// its way out takes no further change; the refusal reports the group's own id and its
/// current status, both read off the record the store holds.
pub fn user_group_change_state_refusal(group: &UserGroupRecord) -> Option<String> {
    (group.status != USER_STATUS_ACTIVE).then(|| {
        format!(
            "User group {} has status {}. Only {} user groups can be modified.",
            group.user_group_id, group.status, USER_STATUS_ACTIVE
        )
    })
}
