// SPDX-License-Identifier: CC0-1.0

//! The reading of a caller-supplied IAM ROLE ARN that services are migrating onto: which
//! role of this account it names, if any.
//!
//! Several emulated services take a role ARN in a request and have to resolve it against
//! the account's IAM records before they write anything (EventBridge Scheduler for a
//! schedule's `Target.RoleArn`, FIS for an experiment template's `roleArn`, Firehose for
//! every `RoleARN` a Firehose stream's configuration carries). Each of them needs the same
//! two facts out of the ARN — does it name an IAM ROLE of THIS account, and under what name
//! does IAM hold it — so the reading is stated here for them to share.
//!
//! The trust-policy half of the question, once the role is found, is
//! [`crate::iam_trust_policy`].
//!
//! Today Firehose reads it here; the copies that predate this module — `iam_role_name` in
//! `service-com-amazonaws-scheduler-awschronosservice/src/state.rs`, `is_iam_role_arn` in
//! `service-com-amazonaws-fis-faultinjectionsimulator`, and the readings in
//! `service-com-amazonaws-eks-awswesleyfrontend` and
//! `service-com-amazonaws-rolesanywhere-rolesanywhere` — are to be MIGRATED onto it, each
//! in the change that owns its service, rather than copied again. Nothing new should
//! inline the parse, and this module does not inline the ARN grammar either: the six
//! fields come from [`crate::arn`], the crate's one ARN parse.

/// The role name an IAM role ARN carries, when the ARN names a role of THIS account:
/// everything after the last `/`, so a role under a path (`:role/service-role/<name>`)
/// resolves to the name IAM holds it under.
///
/// `None` when the value does not name an IAM role of this account:
///
/// * it is not an ARN of the six-field form, or its vendor field is not `iam`;
/// * an IAM ARN carries no Region, so a value with one is not one;
/// * it names another account, whose roles a service acting for this account cannot
///   assume;
/// * its resource is not a `role/…`, or the role name is empty.
///
/// A malformed role ARN is usually refused by the modelled `@pattern` on the member before
/// a handler runs; this reading is what says whether a WELL-FORMED one is this account's.
#[must_use]
pub fn role_name(role_arn: &str) -> Option<&str> {
    // The six fields come from the crate's ONE ARN parse (`crate::arn`), which also
    // settles that the value is an ARN of the `iam` vendor at all.
    let arn = crate::arn::parse_of(role_arn, "iam")?;
    // An IAM ARN is partition-global: it carries no Region, so a value with one in that
    // field names no IAM resource.
    if !arn.region.is_empty() {
        return None;
    }
    if arn.account != ::emulator_server::emulator_account_id() {
        return None;
    }
    let name = arn.resource.strip_prefix("role/")?.rsplit('/').next()?;
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::role_name;

    /// The name is read from the resource field of an IAM role ARN of this account,
    /// whatever path the role lives under — and from nothing else.
    #[test]
    fn a_role_name_is_read_only_from_this_accounts_role_arn() {
        let account = ::emulator_server::emulator_account_id();
        // (case, value, the name the parse must report)
        let rows: Vec<(&str, String, Option<&str>)> = vec![
            (
                "a role under the default path",
                format!("arn:aws:iam::{account}:role/delivery"),
                Some("delivery"),
            ),
            (
                "a role under a path",
                format!("arn:aws:iam::{account}:role/service-role/delivery"),
                Some("delivery"),
            ),
            (
                "a role under a nested path",
                format!("arn:aws:iam::{account}:role/a/b/c/delivery"),
                Some("delivery"),
            ),
            (
                "another partition",
                format!("arn:aws-cn:iam::{account}:role/delivery"),
                Some("delivery"),
            ),
            (
                "a role name holding the characters IAM admits in one",
                format!("arn:aws:iam::{account}:role/a_b-1+2=3,4.5@6"),
                Some("a_b-1+2=3,4.5@6"),
            ),
            (
                "another account",
                "arn:aws:iam::123456789012:role/delivery".to_string(),
                None,
            ),
            (
                "another IAM entity type",
                format!("arn:aws:iam::{account}:user/delivery"),
                None,
            ),
            (
                "an empty role name",
                format!("arn:aws:iam::{account}:role/"),
                None,
            ),
            (
                "a Region in the IAM field, which an IAM ARN does not carry",
                format!("arn:aws:iam:us-east-1:{account}:role/delivery"),
                None,
            ),
            (
                "another vendor",
                format!("arn:aws:sts::{account}:role/delivery"),
                None,
            ),
            ("not an ARN at all", "delivery".to_string(), None),
            ("the empty string", String::new(), None),
        ];

        for (case, value, expected) in rows {
            let read = role_name(&value);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
