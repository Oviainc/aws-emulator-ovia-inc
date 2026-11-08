// SPDX-License-Identifier: CC0-1.0

//! The cross-member preconditions a user-pool write has to satisfy before the
//! pool is stored.
//!
//! Each rule below holds over the configuration a request would LEAVE the pool
//! with, not over one member in isolation, so the operations that write a pool —
//! `CreateUserPool` and `UpdateUserPool` — ask the same question of the same
//! values and answer it with the same sentence.

use super::*;

/// The message a user-pool write is refused with when it selects `phone_number`
/// for auto-verification while the pool would have no SMS delivery path
/// ([`sms_configuration_missing_for_auto_verification`]).
///
/// Fixed text: it names neither the pool nor the attribute list the request
/// carried, since the member and the setting it depends on are the same two for
/// every such request.
pub const SMS_CONFIGURATION_REQUIRED_MESSAGE: &str =
    "SMS configuration is required when phone_number is selected for auto verification";

/// The `VerifiedAttributeType` value whose verification message is delivered over
/// SMS (the other member of that enumeration, `email`, is delivered by email —
/// `smithy_query(query="shape",shape="VerifiedAttributeType")`).
pub const PHONE_NUMBER_VERIFIED_ATTRIBUTE: &str = "phone_number";

/// Whether the configuration a write would leave the pool with asks Amazon
/// Cognito to verify a phone number with no way to send the message.
///
/// A phone number is verified by a code delivered over SMS, and SMS is sent
/// through the Amazon SNS caller a pool's `SmsConfiguration` names ("the SMS
/// configuration with the settings for your Amazon Cognito user pool to send SMS
/// message with Amazon Simple Notification Service" — `SmsConfigurationType` in
/// this service's model), so auto-verifying `phone_number` without one asks for
/// a verification nothing could deliver. Both arguments are the EFFECTIVE values
/// of the request — what the pool would hold once it is written — so the pair is
/// judged as the caller left it, whichever operation wrote it.
#[must_use]
pub fn sms_configuration_missing_for_auto_verification(
    auto_verified_attributes: &[String],
    sms_configuration: Option<&SmsConfigurationType>,
) -> bool {
    auto_verified_attributes
        .iter()
        .any(|attribute| attribute == PHONE_NUMBER_VERIFIED_ATTRIBUTE)
        && sms_configuration.is_none()
}

/// The message a user-pool write is refused with when its
/// `AccountRecoverySetting` repeats a priority or a mechanism
/// ([`duplicate_recovery_mechanisms`]). Fixed text, naming neither the
/// mechanisms nor the priorities the request carried.
pub const DUPLICATE_RECOVERY_MECHANISMS_MESSAGE: &str =
    "Invalid account recovery setting parameter. Account Recovery Setting cannot have \
     duplicate priorities or recovery mechanisms.";

/// Whether `setting`'s `RecoveryMechanisms` repeat a `Priority` value or a
/// mechanism `Name`.
///
/// A recovery option's priority is its RANK among the pool's mechanisms — "if
/// `verified_email` has a priority of 1 and `verified_phone_number` has a
/// priority of 2, your user pool sends account-recovery messages to a verified
/// email address but falls back to an SMS message" (`RecoveryOptionType` in this
/// service's model) — so two mechanisms at one priority leave the fallback order
/// undefined, and one mechanism listed twice gives that mechanism two ranks.
/// Neither is a configuration a pool can apply, so a request carrying one is
/// refused rather than stored.
#[must_use]
pub fn duplicate_recovery_mechanisms(setting: &AccountRecoverySettingType) -> bool {
    let Some(mechanisms) = setting.recovery_mechanisms.as_deref() else {
        return false;
    };
    let mut priorities: BTreeSet<i32> = BTreeSet::new();
    let mut names: BTreeSet<&str> = BTreeSet::new();
    mechanisms.iter().any(|mechanism| {
        !priorities.insert(mechanism.priority) || !names.insert(mechanism.name.as_str())
    })
}

/// The service principal a user pool sends SMS messages as: Amazon Cognito
/// assumes the role an `SmsConfiguration` names, so that role's trust policy has
/// to admit this principal.
const SMS_CALLER_SERVICE_PRINCIPAL: &str = "cognito-idp.amazonaws.com";

/// The message a user-pool write is refused with when the role its
/// `SmsConfiguration` names does not let Amazon Cognito assume it
/// ([`sms_caller_role_is_unassumable`]). Fixed text, naming neither the role nor
/// the pool.
pub const SMS_ROLE_TRUST_RELATIONSHIP_MESSAGE: &str =
    "Role does not have a trust relationship allowing Cognito to assume the role";

/// Whether IAM answered that the role `sms_configuration` names is one Amazon
/// Cognito cannot assume.
///
/// "To send SMS messages with Amazon SNS in the Amazon Web Services Region that
/// you want, the Amazon Cognito user pool uses an Identity and Access Management
/// (IAM) role in your Amazon Web Services account" (`SmsConfigurationType` in
/// this service's model) — so the pool only sends SMS at all if it can assume
/// that role, and a configuration naming a role whose trust policy withholds
/// this principal configures a delivery path that cannot work.
///
/// The reading is the tree's one reading of a caller-supplied role
/// (`service_common::iam_role::held_role_is_unassumable`), the variant that
/// refuses only what a STORED trust document withholds: an SMS caller role is
/// routinely provisioned out of band, so a role this emulator's IAM holds
/// nothing under is not evidence about the role the caller named. Await it with
/// no state guard held.
pub async fn sms_caller_role_is_unassumable(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    sms_configuration: Option<&SmsConfigurationType>,
) -> bool {
    let Some(role_arn) = sms_configuration.and_then(|sms| sms.sns_caller_arn.as_deref()) else {
        return false;
    };
    ::service_common::iam_role::held_role_is_unassumable(
        ctx,
        region,
        role_arn,
        SMS_CALLER_SERVICE_PRINCIPAL,
    )
    .await
}
