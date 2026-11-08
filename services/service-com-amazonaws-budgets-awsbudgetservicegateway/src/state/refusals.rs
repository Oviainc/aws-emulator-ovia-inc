// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusals several operations share.
//!
//! A refusal is part of the wire contract: a client branches on the error code, and —
//! because this service answers every missing resource with the one `NotFoundException`
//! code ("We can't locate the resource that you specified") — on the MESSAGE as well,
//! which is the only thing that tells a missing budget from a missing budget action.
//! So each sentence below belongs to the CONDITION being refused, not to the operation
//! that noticed it: every handler that resolves a budget by name words its not-found the
//! same way, every handler that resolves a budget action by id words its not-found the
//! same way, and every method refused because of the state an action is in words that
//! the same way too.
//!
//! The variable parts come from the request and from this service's own state (the
//! budget name, the account id, the action id, the status the action currently carries);
//! the rest of each sentence is fixed text.

use super::MAX_NOTIFICATIONS_PER_BUDGET;
use super::TaggedResource;
use crate::ops::types::ACTION_STATUS_VALUES;

/// The sentence refusing an operation that named a budget this account does not hold.
///
/// ONE sentence for every operation: the budget is addressed by name
/// (`BudgetName`, or `NewBudget$BudgetName` on `UpdateBudget`), so the name is the one
/// variable part, taken verbatim from the request. The verb does not follow the calling
/// operation — a client cannot be asked to know a different sentence per operation when
/// the error code is the same for all of them.
pub fn budget_not_found(budget_name: &str) -> String {
    format!("Unable to get budget: {budget_name} - the budget doesn't exist.")
}

/// The sentence refusing an operation that named a budget action no action of this
/// account carries.
///
/// An action is identified by the account it belongs to plus its system-generated
/// `ActionId`, and both are named back: a missing action is reported in its own wording,
/// not in the budget's, so a client that distinguishes the two conditions by message
/// text can.
pub fn action_not_found(account_id: &str, action_id: &str) -> String {
    format!("Action for [AccountId: {account_id}, ActionId: {action_id}] is not found!")
}

/// The sentence refusing a tagging operation whose `ResourceARN` resolves to nothing.
///
/// An ARN addresses the same two resources the named operations do, so the refusal is
/// the same one: a budget the account does not hold is reported by
/// [`budget_not_found`] and a budget action it does not hold by [`action_not_found`],
/// with the names read out of the ARN the request carried.
pub fn tagged_resource_not_found(target: &TaggedResource) -> String {
    match target {
        TaggedResource::Budget { budget_name, .. } => budget_not_found(budget_name),
        TaggedResource::Action {
            account_id,
            action_id,
            ..
        } => action_not_found(account_id, action_id),
    }
}

/// The sentence refusing a method the state of the budget action it names does not
/// allow: a second execution of work that is still running, an execution type the
/// action's current status leaves nothing to do, an update or a delete of an action
/// mid-flight.
///
/// The refusal is about the RESOURCE's state rather than about any value in the request,
/// which is what `ResourceLockedException` reports ("the server rejected that particular
/// method for the requested resource"), so the sentence names no method and no execution
/// type — only the status the action carries at that moment
/// ([`action_status_rendered`]).
pub fn method_not_allowed_during(status: &str) -> String {
    format!(
        "This method is not allowed during [ActionStatus: {}]",
        action_status_rendered(status)
    )
}

/// An `ActionStatus` as this sentence renders it: the modelled value, whose wire form is
/// upper snake case (`STANDBY`, `EXECUTION_IN_PROGRESS`), written in the upper camel case
/// the status-conflict sentence carries it in (`Standby`, `ExecutionInProgress`).
///
/// The conversion is over the value's own shape, so a status the model gains later needs
/// no entry here; a value the model does not declare (`ACTION_STATUS_VALUES`, generated
/// from it) is left exactly as it came, since nothing is known about its spelling.
pub fn action_status_rendered(status: &str) -> String {
    if !ACTION_STATUS_VALUES.contains(&status) {
        return status.to_string();
    }
    status
        .split('_')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(characters.flat_map(char::to_lowercase))
                    .collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

/// The sentence refusing a notification a budget has no room left for — the capacity
/// [`MAX_NOTIFICATIONS_PER_BUDGET`] states, which belongs to the budget rather than to
/// the call that would have attached the notification.
///
/// Nothing from the request is interpolated: the quota is the same for every budget, so
/// the sentence names the limit and not the resource that reached it.
pub fn notification_quota_exceeded() -> String {
    format!(
        "Unable to create notification - one budget can only have \
         {MAX_NOTIFICATIONS_PER_BUDGET} notifications"
    )
}

/// The sentence refusing a budget whose `TimePeriod` is the wrong way round — "The start
/// date must come before the end date" (`Budget$TimePeriod`). Wholly fixed text: the
/// two instants are in the request the caller is holding.
pub const BUDGET_TIME_PERIOD_ORDER_REFUSAL: &str =
    "Unable to create/update budget - end time has to be after start time.";

/// The sentence refusing a budget that determines its amount from more than one of the
/// three members that can determine it (`BudgetLimit`, `PlannedBudgetLimits`,
/// `AutoAdjustData`). Fixed text that names all three, whichever pair of them the
/// request supplied, so the whole rule is readable from the refusal.
pub const BUDGET_AMOUNT_EXCLUSIVITY_REFUSAL: &str =
    "Unable to create/update budget - please provide only one of the followings: \
     Budget Limit/ Planned Budget Limit/ Auto Adjust Data";
