// SPDX-License-Identifier: CC0-1.0

//! Whether the IAM role a request binds to an ACME external account binding is a role this
//! service can ACT AS.
//!
//! An external account binding does not act as whoever created it: the ACME enrollment
//! surface issues certificates for a registered account by assuming the binding's own
//! `RoleArn`, as its own service principal ([`SERVICE_PRINCIPAL`]), with the session tags
//! and the source identity that assume carries. A role whose trust policy does not let
//! that principal perform those STS actions is a role this service could never assume, so
//! there is nothing a binding carrying it could do — and the binding is refused at
//! creation time rather than stored and reported ready.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **which role** — the `RoleArn` the request names, resolved against the IAM store of
//!    the SAME shared engine this service's own store lives in, so the trust document read
//!    is the one `iam:CreateRole`/`iam:UpdateAssumeRolePolicy` wrote;
//! 2. **the reading of its trust document** —
//!    [`::service_common::iam_trust_policy::admits_service_principal_actions`], the tree's
//!    ONE reading of a trust policy, so this service and every other one handed the same
//!    document answer alike: an explicit `Deny` wins, a document that names neither this
//!    principal nor one of the actions grants nothing, and the wildcards IAM admits in an
//!    action's place cover it;
//! 3. **the refusal** — [`unable_to_assume_message`], the one sentence this service states
//!    that failure in, whose only variable part is the role ARN the request carried.
//!
//! Nothing is refused on evidence this emulator does not have — the same position every
//! other service in this tree that resolves a caller-supplied execution role takes
//! (`service-com-amazonaws-pipes-pipes/src/state/execution_role.rs`). Each of these shows
//! nothing about the role and lets the create go ahead: an engine carrying no IAM store, a
//! `RoleArn` the store holds no role for (it may belong to another account, or to one the
//! emulator was never told about), a role carrying no trust document, and a document that
//! cannot be read as a policy. What a document DOES state is decided in IAM's own order.

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_trust_policy::{self, ASSUME_ROLE_ACTION};

/// The service principal the ACME enrollment surface assumes a bound role as, as a trust
/// document names one.
pub const SERVICE_PRINCIPAL: &str = "acm-acme.amazonaws.com";

/// Every STS action that assume performs on the role: the assume itself, the source
/// identity it sets for the session, and the session tags it attaches. A trust policy that
/// withholds any one of them is a role this service cannot act as, which is why the refusal
/// names all three.
pub const SESSION_ACTIONS: &[&str] = &[
    ASSUME_ROLE_ACTION,
    "sts:SetSourceIdentity",
    "sts:TagSession",
];

/// Why this service cannot assume the role `role_arn`, or `None` when nothing of the kind
/// can be shown (see the module docs for the cases that show nothing).
///
/// Read BEFORE the store this service writes is locked, so a create that is about to be
/// refused never holds both stores at once, and answered as the sentence the caller is
/// refused with.
pub fn assume_refusal<Eng: Send + Sync + 'static>(engine: &Eng, role_arn: &str) -> Option<String> {
    let iam = iam_store::service_state(engine)?;
    let store = iam.lock();
    // A role this account does not hold shows nothing: the ARN may name another account's.
    // The stored ARN carries the role's PATH, so comparing ARNs resolves a role under one.
    let role = store.roles.values().find(|role| role.arn == role_arn)?;
    let trust = role.assume_role_policy_document.as_deref()?;
    let admitted = iam_trust_policy::admits_service_principal_actions(
        trust,
        SERVICE_PRINCIPAL,
        SESSION_ACTIONS,
    );
    match admitted {
        Some(false) => Some(unable_to_assume_message(role_arn)),
        _ => None,
    }
}

/// The sentence this service refuses a role it cannot assume with: WHICH role (the one
/// variable part, echoed from the request) and what its trust policy has to grant — the
/// principal this service assumes as and every STS action it performs on the role, listed
/// from [`SESSION_ACTIONS`] so the sentence cannot drift from what is checked.
#[must_use]
pub fn unable_to_assume_message(role_arn: &str) -> String {
    format!(
        "Unable to assume role for {role_arn}. Verify that the role exists and its trust \
         policy grants {SERVICE_PRINCIPAL} access to {}.",
        action_list(SESSION_ACTIONS)
    )
}

/// `actions` as the sentence above lists them: comma-separated, with `and` before the last.
fn action_list(actions: &[&str]) -> String {
    match actions.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, leading)) => format!("{}, and {last}", leading.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the refusal is fixed text apart from the role ARN it echoes, and it names the
    /// principal this service assumes as together with every action it checks for.
    #[test]
    fn the_refusal_names_the_role_the_principal_and_every_action_checked() {
        for role_arn in [
            "arn:aws:iam::000000000000:role/AcmeIssuer",
            "arn:aws:iam::000000000000:role/service-role/nested/Acme-Issuer_1",
        ] {
            let message = unable_to_assume_message(role_arn);
            assert_eq!(
                message,
                format!(
                    "Unable to assume role for {role_arn}. Verify that the role exists and its \
                     trust policy grants acm-acme.amazonaws.com access to sts:AssumeRole, \
                     sts:SetSourceIdentity, and sts:TagSession."
                ),
                "case {role_arn:?}: expected ACM's wording, got {message:?}"
            );
        }
    }

    /// RULE: the list reads as English for any number of actions, so the sentence stays
    /// well-formed if the set this service performs ever changes.
    #[test]
    fn the_action_list_reads_as_a_sentence_at_every_length() {
        let rows: &[(&str, &[&str], &str)] = &[
            ("no action at all", &[], ""),
            ("one action", &["sts:AssumeRole"], "sts:AssumeRole"),
            (
                "two actions",
                &["sts:AssumeRole", "sts:TagSession"],
                "sts:AssumeRole, and sts:TagSession",
            ),
            (
                "the three this service performs",
                SESSION_ACTIONS,
                "sts:AssumeRole, sts:SetSourceIdentity, and sts:TagSession",
            ),
        ];
        for (case, actions, expected) in rows {
            let rendered = action_list(actions);
            assert_eq!(
                rendered, *expected,
                "case {case:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}
