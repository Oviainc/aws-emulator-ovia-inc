// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of an IAM trust policy, for every service that resolves a
//! caller-supplied execution role before it writes.
//!
//! Several emulated services take a `roleArn` the caller names, act as that role, and so
//! check up front that the role's trust policy lets their own service principal assume it
//! (EventBridge Scheduler does it for `Target.RoleArn`, FIS for an experiment template's
//! `roleArn`). That check is ONE IAM rule, so it is stated here once rather than per
//! service: two services asked about the same document must not answer differently.
//!
//! The rule, in IAM's own order:
//!
//! * the document is an IAM policy — a `Statement` that is one object or a list of them.
//!   IAM reports a trust document URL-encoded, so a document that is not JSON as it
//!   stands is percent-decoded and parsed again ([`parse_policy`]);
//! * a statement is ABOUT this call only when its `Principal.Service` names the principal
//!   AND its `Action` names [`ASSUME_ROLE_ACTION`] (or a wildcard covering it). `Action`
//!   and `Principal.Service` are each either one string or a list of them, and IAM
//!   compares both case-insensitively;
//! * an explicit `Deny` among those statements WINS, whatever any `Allow` says;
//! * otherwise the role admits the principal exactly when one of them is an `Allow`.
//!
//! The two readers this rule is built on — [`parse_policy`], which accepts a document in
//! either of the forms a store hands one back in, and [`json_strings`], which reads an
//! element that is one string or a list of them — are the SAME readers
//! [`super::iam_authorization`] evaluates a full policy with, so the two modules cannot
//! disagree about what a document says.
//!
//! What a service does when the document cannot be PARSED at all is its own policy, not
//! IAM's, which is why [`admits_service_principal`] reports that case as `None` instead of
//! choosing: EventBridge Scheduler refuses (it resolved the role and found nothing it could
//! act on), while FIS treats it as no evidence either way. A document that parses but
//! carries no statement is not that case: it is a policy that grants nothing.

/// The action a trust-policy statement must name for the principal it names to be able to
/// assume the role. A statement that names some other action governs something else.
pub const ASSUME_ROLE_ACTION: &str = "sts:AssumeRole";

/// Whether `document` lets the service principal `principal` (`scheduler.amazonaws.com`,
/// `fis.amazonaws.com`, …) assume the role it is attached to.
///
/// `Some(true)` when it does, `Some(false)` when the document is a policy that withholds
/// the grant (it says nothing about this principal, or it explicitly denies it), and `None`
/// when `document` is not a policy this can be read out of — see the module docs for why
/// that case is the caller's to decide.
pub fn admits_service_principal(document: &str, principal: &str) -> Option<bool> {
    let policy = parse_policy(document)?;
    let statement = &policy["Statement"];
    // A document that PARSED but carries no statement this can read is a policy that
    // grants nothing, not an unreadable one: it withholds the grant rather than leaving
    // the caller without evidence.
    let statements: Vec<&::serde_json::Value> = match statement {
        ::serde_json::Value::Array(entries) => entries.iter().collect(),
        ::serde_json::Value::Object(_) => vec![statement],
        _ => return Some(false),
    };
    let mut admitted = false;
    for entry in statements {
        if !statement_covers(entry, principal) {
            continue;
        }
        let effect = entry["Effect"].as_str().unwrap_or_default();
        if effect.eq_ignore_ascii_case("Deny") {
            // An explicit deny is final: no `Allow`, before it or after it, puts the
            // grant back.
            return Some(false);
        }
        if effect.eq_ignore_ascii_case("Allow") {
            admitted = true;
        }
    }
    Some(admitted)
}

/// The policy `document` carries, in either of the two forms a store hands one back in.
///
/// A document that is JSON as it stands is taken AS IT STANDS: the percent-decoder is a
/// URI decoder, so running it over text that needs no decoding would rewrite a literal `+`
/// inside the policy (a condition value, a base64-looking string) to a space and corrupt
/// it. Only a document that does not parse is percent-decoded — the URL-encoded form IAM
/// reports a trust policy in — and parsed again.
pub(crate) fn parse_policy(document: &str) -> Option<::serde_json::Value> {
    if let Ok(policy) = ::serde_json::from_str::<::serde_json::Value>(document) {
        return Some(policy);
    }
    let decoded = ::smithy_server_runtime::percent_decode(document);
    ::serde_json::from_str(&decoded).ok()
}

/// Whether one statement is ABOUT `principal` assuming the role — whatever effect it then
/// states.
fn statement_covers(statement: &::serde_json::Value, principal: &str) -> bool {
    let names_principal = json_strings(&statement["Principal"]["Service"])
        .iter()
        .any(|service| service.eq_ignore_ascii_case(principal));
    let names_assume_role = json_strings(&statement["Action"])
        .iter()
        .any(|action| action_covers_assume_role(action));
    names_principal && names_assume_role
}

/// Whether one `Action` entry covers [`ASSUME_ROLE_ACTION`]: the action itself, or a
/// wildcard IAM admits in its place (`*`, or a prefix wildcard such as `sts:*` or
/// `sts:Assume*`).
fn action_covers_assume_role(action: &str) -> bool {
    if action == "*" || action.eq_ignore_ascii_case(ASSUME_ROLE_ACTION) {
        return true;
    }
    match action.strip_suffix('*') {
        Some(prefix) => {
            ASSUME_ROLE_ACTION.len() >= prefix.len()
                && ASSUME_ROLE_ACTION[..prefix.len()].eq_ignore_ascii_case(prefix)
        }
        None => false,
    }
}

/// The strings a policy member holds, whether it is one string or a list of them.
/// Anything else contributes none.
pub(crate) fn json_strings(value: &::serde_json::Value) -> Vec<&str> {
    match value {
        ::serde_json::Value::String(one) => vec![one.as_str()],
        ::serde_json::Value::Array(many) => many.iter().filter_map(|item| item.as_str()).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trust policy with one statement, written as a client would send it.
    fn policy(effect: &str, action: &str, service: &str) -> String {
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
             \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
        )
    }

    #[test]
    fn the_rule_holds_across_the_forms_a_trust_policy_takes() {
        // The rows differ along what the rule generalises over: the form of `Action` and
        // of `Principal.Service` (one string or a list), the wildcards IAM admits in the
        // action's place, the effect, which principal is named, and the casing.
        let rows: [(&str, String, Option<bool>); 13] = [
            (
                "the action and the principal, as strings",
                policy("Allow", "\"sts:AssumeRole\"", "\"fis.amazonaws.com\""),
                Some(true),
            ),
            (
                "both as lists",
                policy(
                    "Allow",
                    "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                    "[\"ec2.amazonaws.com\",\"fis.amazonaws.com\"]",
                ),
                Some(true),
            ),
            (
                "a service wildcard in the action",
                policy("Allow", "\"sts:*\"", "\"fis.amazonaws.com\""),
                Some(true),
            ),
            (
                "a prefix wildcard in the action",
                policy("Allow", "\"sts:Assume*\"", "\"fis.amazonaws.com\""),
                Some(true),
            ),
            (
                "every action",
                policy("Allow", "\"*\"", "\"fis.amazonaws.com\""),
                Some(true),
            ),
            (
                "the action named in another casing",
                policy("Allow", "\"STS:ASSUMEROLE\"", "\"FIS.amazonaws.com\""),
                Some(true),
            ),
            (
                "another service's principal",
                policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
                Some(false),
            ),
            (
                "an action that is not assuming the role",
                policy("Allow", "\"s3:GetObject\"", "\"fis.amazonaws.com\""),
                Some(false),
            ),
            (
                "an explicit deny",
                policy("Deny", "\"sts:AssumeRole\"", "\"fis.amazonaws.com\""),
                Some(false),
            ),
            (
                "a statement carrying no action at all",
                "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                 \"Principal\":{\"Service\":\"fis.amazonaws.com\"}}]}"
                    .to_string(),
                Some(false),
            ),
            (
                "one statement as an object rather than a list",
                "{\"Version\":\"2012-10-17\",\"Statement\":{\"Effect\":\"Allow\",\
                 \"Principal\":{\"Service\":\"fis.amazonaws.com\"},\
                 \"Action\":\"sts:AssumeRole\"}}"
                    .to_string(),
                Some(true),
            ),
            (
                "not a policy at all",
                "this is not json".to_string(),
                None,
            ),
            (
                "a document that parses but carries no statement",
                "{\"Version\":\"2012-10-17\"}".to_string(),
                Some(false),
            ),
        ];

        for (case, document, expected) in rows {
            let admitted = admits_service_principal(&document, "fis.amazonaws.com");
            assert_eq!(
                admitted, expected,
                "case {case:?}: expected {expected:?}, got {admitted:?} for {document}"
            );
        }
    }

    #[test]
    fn an_explicit_deny_wins_whichever_order_the_statements_are_written_in() {
        let allow = "{\"Effect\":\"Allow\",\"Principal\":{\"Service\":\"fis.amazonaws.com\"},\
                     \"Action\":\"sts:AssumeRole\"}";
        let deny = "{\"Effect\":\"Deny\",\"Principal\":{\"Service\":\"fis.amazonaws.com\"},\
                    \"Action\":\"sts:AssumeRole\"}";
        for (case, document) in [
            (
                "allow then deny",
                format!("{{\"Statement\":[{allow},{deny}]}}"),
            ),
            (
                "deny then allow",
                format!("{{\"Statement\":[{deny},{allow}]}}"),
            ),
        ] {
            let admitted = admits_service_principal(&document, "fis.amazonaws.com");
            assert_eq!(
                admitted,
                Some(false),
                "case {case:?}: an explicit deny wins, got {admitted:?}"
            );
        }
    }

    #[test]
    fn a_literal_plus_inside_the_document_survives() {
        // The percent-decoder is a URI decoder, which reads '+' as a space, so a document
        // that is already JSON must not be run through it.
        let document = "{\"Statement\":[{\"Effect\":\"Allow\",\
                        \"Principal\":{\"Service\":\"fis.amazonaws.com\"},\
                        \"Action\":\"sts:AssumeRole\",\
                        \"Condition\":{\"StringEquals\":{\"sts:ExternalId\":\"a+b/c=\"}}}]}";
        let admitted = admits_service_principal(document, "fis.amazonaws.com");
        assert_eq!(
            admitted,
            Some(true),
            "a policy carrying a literal '+' is read as it stands, got {admitted:?}"
        );
    }

    #[test]
    fn a_url_encoded_document_is_read_as_the_policy_it_encodes() {
        // IAM reports the trust document URL-encoded, so the same policy must read the
        // same way through either form.
        let plain = policy("Allow", "\"sts:AssumeRole\"", "\"fis.amazonaws.com\"");
        let encoded = plain
            .replace('{', "%7B")
            .replace('}', "%7D")
            .replace('"', "%22");
        for (case, document) in [("plain", plain.clone()), ("url-encoded", encoded)] {
            let admitted = admits_service_principal(&document, "fis.amazonaws.com");
            assert_eq!(
                admitted,
                Some(true),
                "case {case:?}: expected the grant to be read, got {admitted:?}"
            );
        }
    }
}
