// SPDX-License-Identifier: CC0-1.0

//! The modelled refusals several of this service's operations share, built from the
//! request so one fault is reported the same way whichever operation carried it.

use super::*;

/// The modelled `ValidationException` this service refuses a request with: the
/// reason (one of `ValidationExceptionReason`), the member at fault, and the text.
///
/// It lives in this module rather than one of its own because `src/ops/mod.rs` is
/// GENERATED, so a helper several operations share has no other home in this crate.
pub fn validation_exception(
    reason: &str,
    field: &str,
    message: impl Into<String>,
) -> crate::ops::types::ValidationException {
    let message = message.into();
    crate::ops::types::ValidationException {
        message: message.clone(),
        reason: Some(reason.to_string()),
        fields: Some(vec![crate::ops::types::ValidationExceptionField {
            name: field.to_string(),
            message,
        }]),
    }
}

/// The modelled refusal a request earns when one of the account ids it names is
/// already associated with a DIFFERENT billing group.
///
/// An account belongs to at most one billing group, so an account list naming one
/// another group already owns is a fault in the INPUT, not an inconsistent state of
/// the group addressed: `ValidationExceptionReason` enumerates
/// `ACCOUNTS_ALREADY_ASSOCIATED` for exactly this case, while no
/// `ConflictExceptionReason` value describes it
/// (`smithy_query(query="shape",shape="ValidationExceptionReason,ConflictExceptionReason")`).
/// So the answer is the 400 `ValidationException` with that reason, built here so
/// every operation that takes account ids answers it the same way; `field` names the
/// input member that carried the id (`AccountIds`, `AccountGrouping.LinkedAccountIds`,
/// …), which is what goes in `Fields`.
pub fn accounts_already_associated(
    field: &str,
    account_id: &str,
    owner_group_arn: &str,
) -> crate::ops::types::ValidationException {
    validation_exception(
        "ACCOUNTS_ALREADY_ASSOCIATED",
        field,
        format!(
            "account {account_id} is already associated with billing group {owner_group_arn}"
        ),
    )
}

/// The modelled refusal a request earns when one of the account ids it names is not a
/// linked account of the payer's consolidated billing family.
///
/// A billing group groups the accounts of ONE consolidated billing family: the
/// operation that connects accounts to a group takes "an array of account IDs in a
/// consolidated billing family" and "the account IDs must be a part of the
/// consolidated billing family during the current month", and
/// `AccountGrouping.LinkedAccountIds` repeats it ("Account IDs must be a part of the
/// consolidated billing family, and not associated with another billing group"). An
/// account list naming an account outside the family is therefore a fault in the
/// INPUT: `ValidationExceptionReason` enumerates `ILLEGAL_ACCOUNTS` for an account
/// list this service may not take
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`), so the answer is
/// the 400 `ValidationException` with that reason — built here so every operation that
/// takes account ids answers it the same way, beside the
/// [`accounts_already_associated`] refusal the other precondition earns; `field` names
/// the input member that carried the id (`AccountIds`,
/// `AccountGrouping.LinkedAccountIds`, …), which is what goes in `Fields`.
pub fn account_not_in_billing_family(
    field: &str,
    account_id: &str,
) -> crate::ops::types::ValidationException {
    validation_exception(
        "ILLEGAL_ACCOUNTS",
        field,
        format!("account {account_id} is not a part of the consolidated billing family"),
    )
}

/// The modelled refusal a request earns when the billing group name it claims is
/// already occupied in the CURRENT billing period by a group that was deleted during
/// it.
///
/// A billing group bills a consolidated family for a billing period, so the period it
/// billed in keeps the record of it and of the name it carried there: until the next
/// billing period that name names that group. `ValidationExceptionReason` enumerates
/// `BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD` for exactly this
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`), so the answer is
/// the 400 `ValidationException` with that reason — built here so every operation that
/// claims a billing group name answers it the same way; `field` names the input member
/// that carried the name.
///
/// The message states the rule and the period, never the NAME: a billing group's name
/// is `@sensitive` (`smithy_query(query="shape",shape="BillingGroupName")`), so its
/// value belongs in no message and no log — the member named in `Fields` is what tells
/// the caller which value of its own request was at fault.
pub fn billing_group_already_exists_in_current_billing_period(
    field: &str,
    billing_period: &str,
) -> crate::ops::types::ValidationException {
    validation_exception(
        "BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD",
        field,
        format!(
            "a billing group with this name already exists in the current billing period \
             ({billing_period})"
        ),
    )
}

/// The modelled refusal a request earns when enabling automatic account association
/// would take the consolidated billing family past
/// [`MAX_AUTO_ASSOCIATE_BILLING_GROUPS`] groups that have it enabled.
///
/// Built here so every operation that can turn `AccountGrouping.AutoAssociate` on
/// answers it the same way; `field` names the input member that carried the flag, and
/// `holders` the groups that already have it, which is what the message is built from.
pub fn too_many_auto_associate_billing_groups(
    field: &str,
    holders: &[String],
) -> crate::ops::types::ValidationException {
    validation_exception(
        "TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS",
        field,
        format!(
            "at most {MAX_AUTO_ASSOCIATE_BILLING_GROUPS} billing group of a consolidated \
             billing family may automatically associate newly added accounts; it is already \
             enabled on {}",
            holders.join(", ")
        ),
    )
}

/// The modelled refusal a request earns when it asks to delete a billing group that
/// still has automatic account association enabled.
///
/// A group with `AccountGrouping.AutoAssociate` on is the one the consolidated billing
/// family hands its newly added accounts to, so it is not deletable while it holds that
/// election: the caller turns the flag off (`UpdateBillingGroup`) and then deletes.
/// `ValidationExceptionReason` enumerates
/// `CANNOT_DELETE_AUTO_ASSOCIATE_BILLING_GROUP` for exactly this
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`), so the answer is
/// the 400 `ValidationException` with that reason. `field` names the input member that
/// carried the group's ARN.
pub fn cannot_delete_auto_associate_billing_group(
    field: &str,
    group_arn: &str,
) -> crate::ops::types::ValidationException {
    validation_exception(
        "CANNOT_DELETE_AUTO_ASSOCIATE_BILLING_GROUP",
        field,
        format!(
            "the billing group {group_arn} automatically associates newly added accounts and \
             cannot be deleted; turn AccountGrouping.AutoAssociate off before deleting it"
        ),
    )
}
