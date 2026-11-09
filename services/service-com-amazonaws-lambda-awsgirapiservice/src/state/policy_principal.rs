// SPDX-License-Identifier: CC0-1.0

//! WHICH identities a function's resource-based policy may be granted TO, and how a value
//! that names none of them is refused.
//!
//! `AddPermission$Principal` is documented as "The Amazon Web Services service, Amazon Web
//! Services account, IAM user, or IAM role that invokes the function", where "For Amazon
//! Web Services services, the principal is a domain-style identifier that the service
//! defines, such as s3.amazonaws.com or sns.amazonaws.com"
//! (`smithy_query(query="doc",shape="AddPermissionRequest",member="Principal")`), and the
//! policy language adds the wildcard that names every principal. So there are exactly four
//! FORMS a grant can name, and a value outside all of them denotes nobody at all:
//!
//! * `*`, every principal;
//! * an account, written as its twelve-digit id ([`is_account_id`]);
//! * an IAM identity, written as its ARN (`service_common::arn::is_iam_principal_arn`,
//!   which is where the identity ARN grammar is stated once for every service that resolves
//!   a policy principal);
//! * a service, written as the domain-style identifier that service defines
//!   ([`is_service_principal`]).
//!
//! The member's own `@pattern` (`^[^\s]+$`) admits far more than that — it only forbids
//! whitespace — so the form is read HERE, where the grant is made, and a value in none of
//! the four is refused with [`INVALID_PRINCIPAL_MESSAGE`] rather than stored as a grant
//! that no caller can ever match. The abbreviation that makes this worth checking is a
//! service's SHORT name: it is a well-formed string, it looks like the service it names,
//! and it is not the identifier that service defines, so a grant to it is a grant to
//! nobody.
//!
//! The same forms decide how a stored statement's `Principal` element is RENDERED
//! ([`super::principal_json`]), which calls the predicates here rather than restating them,
//! so what is admitted and what is written cannot disagree.

/// The refusal a `Principal` in none of the four documented forms gets.
///
/// Fixed text, naming nothing of the request: what is wrong is that the value denotes no
/// identity at all, which is the same sentence whatever was submitted, so the caller is
/// told to check the principal it sent rather than being read its own bytes back.
pub const INVALID_PRINCIPAL_MESSAGE: &str =
    "The provided principal was invalid. Please check the principal and try again.";

/// The principal that names EVERY principal, as the policy language writes it.
pub const EVERY_PRINCIPAL: &str = "*";

/// The digits an AWS account id carries: every account id is exactly twelve of them.
const ACCOUNT_ID_DIGITS: usize = 12;

/// Whether `value` is an AWS account id — the form an account is named by, which the policy
/// language writes as that account's root ARN.
#[must_use]
pub fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `value` is the domain-style identifier of an AWS service.
///
/// The form is the identifier's SHAPE rather than any one suffix: a DNS name of at least
/// two labels, each a non-empty run of the characters a DNS label carries. That admits
/// every service principal whatever domain it sits under — the `amazonaws.com` ones, a
/// partition's own (`s3.amazonaws.com.cn`), a regionalised one
/// (`logs.<region>.amazonaws.com`) and the ones AWS defines elsewhere
/// (`alexa-appkit.amazon.com`) — while a bare token with no domain at all, which is what a
/// caller writes when it abbreviates a service to its short name, is not a domain-style
/// identifier and names no service.
#[must_use]
pub fn is_service_principal(value: &str) -> bool {
    value.contains('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// Whether `principal` names an identity a resource-based policy can be granted to: one of
/// the four forms this module's header lists.
#[must_use]
pub fn is_grantable_principal(principal: &str) -> bool {
    principal == EVERY_PRINCIPAL
        || is_account_id(principal)
        || ::service_common::arn::is_iam_principal_arn(principal)
        || is_service_principal(principal)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a grant names an identity in one of the four documented forms, and a value in
    /// none of them names nobody.
    ///
    /// The rows span what the rule generalises over: each of the four forms, the domains a
    /// service identifier sits under, the identity kinds an ARN names, and the near-misses
    /// a caller writes — a service's short name, an id that is not twelve digits, an ARN of
    /// something that is not an identity.
    #[test]
    fn only_the_documented_principal_forms_name_an_identity() {
        let rows: &[(&str, bool)] = &[
            // Every principal.
            ("*", true),
            // An account, by id.
            ("123456789012", true),
            ("000000000000", true),
            ("12345678901", false),
            ("1234567890123", false),
            ("12345678901a", false),
            // An IAM identity, by ARN.
            ("arn:aws:iam::123456789012:root", true),
            ("arn:aws:iam::123456789012:user/someone", true),
            ("arn:aws:iam::123456789012:role/service-role/invoker", true),
            ("arn:aws:sts::123456789012:assumed-role/invoker/session", true),
            ("arn:aws-cn:iam::123456789012:role/invoker", true),
            ("arn:aws:s3:::uploads", false),
            ("arn:aws:iam::123456789012:policy/invoker", false),
            // A service, by the domain-style identifier it defines.
            ("s3.amazonaws.com", true),
            ("sns.amazonaws.com", true),
            ("s3.amazonaws.com.cn", true),
            ("logs.us-west-2.amazonaws.com", true),
            ("alexa-appkit.amazon.com", true),
            // And the abbreviations that name nobody.
            ("apigateway", false),
            ("s3", false),
            ("s3.", false),
            (".amazonaws.com", false),
            ("s3..amazonaws.com", false),
            ("s3_bucket.amazonaws.com", false),
            ("", false),
        ];

        for (principal, grantable) in rows {
            let decided = is_grantable_principal(principal);
            assert_eq!(
                decided, *grantable,
                "case {principal:?}: expected grantable {grantable}, got {decided}"
            );
        }
    }
}
