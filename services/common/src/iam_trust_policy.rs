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
//!   AND its `Action` names the action asked about — [`ASSUME_ROLE_ACTION`] for a service
//!   that only assumes the role, every action it performs on the role for a service that
//!   also tags the session or sets a source identity ([`admits_service_principal_actions`])
//!   — or a wildcard covering it. `Action` and `Principal.Service` are each either one
//!   string or a list of them, and IAM compares both case-insensitively;
//! * an explicit `Deny` among those statements WINS, whatever any `Allow` says;
//! * otherwise the role admits the principal exactly when one of them is an `Allow`.
//!
//! A service that AUTHORIZES one assume, rather than validating a trust relationship, asks
//! [`grants_service_principal`] instead: the same rule plus the statement's `Condition`,
//! decided against the keys that request carries. A trust policy is a resource-based policy,
//! so that reading is [`super::iam_authorization`]'s one evaluation of a policy document
//! against a request rather than a second one written here.
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
    admits_service_principal_actions(document, principal, &[ASSUME_ROLE_ACTION])
}

/// Whether `document` lets the service principal `principal` perform EVERY action in
/// `actions` on the role it is attached to.
///
/// [`admits_service_principal`] is this asked about [`ASSUME_ROLE_ACTION`] alone, which is
/// all a service that just assumes the role needs. A service that assumes it WITH session
/// tags or a source identity performs further STS actions on it in the same call
/// (`sts:TagSession`, `sts:SetSourceIdentity`), and a trust policy that withholds any one
/// of them is a role that service cannot act as — so each action it performs is asked about
/// here, and the role admits the principal only when the document grants them all. The rule
/// per action, and what each answer means, is [`admits_service_principal`]'s.
///
/// An empty `actions` asks nothing of the document, so every policy admits it.
pub fn admits_service_principal_actions(
    document: &str,
    principal: &str,
    actions: &[&str],
) -> Option<bool> {
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
    Some(
        actions
            .iter()
            .all(|action| admits_action(&statements, principal, action)),
    )
}

/// Whether the statements of one document let `principal` perform `action` on the role:
/// IAM's order over the statements that are ABOUT that call.
fn admits_action(statements: &[&::serde_json::Value], principal: &str, action: &str) -> bool {
    let mut admitted = false;
    for entry in statements {
        if !statement_covers(entry, principal, action) {
            continue;
        }
        let effect = entry["Effect"].as_str().unwrap_or_default();
        if effect.eq_ignore_ascii_case("Deny") {
            // An explicit deny is final: no `Allow`, before it or after it, puts the
            // grant back.
            return false;
        }
        if effect.eq_ignore_ascii_case("Allow") {
            admitted = true;
        }
    }
    admitted
}

/// Whether `document` grants the service principal `principal` the assume of the role
/// `role_arn` it is attached to FOR ONE PARTICULAR assume — the request whose condition
/// keys `context` states, as `(key, value)` pairs.
///
/// [`admits_service_principal`] answers the WEAKER question: does the role trust this
/// principal at all, whatever a statement's `Condition` narrows that trust to. That is what
/// a service asks when it validates a trust RELATIONSHIP it is about to store. A service
/// that AUTHORIZES one assume — a caller handing it a role to act under for a named request
/// — asks this instead, because a statement's `Condition` is part of its grant: a grant the
/// request cannot be shown to satisfy is no grant for THAT request. The two therefore read
/// one document under one rule and differ only in how much of the request they are told
/// about.
///
/// The evaluation is the tree's one evaluation of a policy document against a request
/// ([`super::iam_authorization`]): a trust policy IS a resource-based policy, whose
/// statements name the principals they grant to and whose missing `Resource` element covers
/// the role the document is attached to. So IAM's order — an explicit `Deny` wins, an
/// `Allow` whose conditions this cannot decide is not a grant — is that module's, not a
/// second reading of it here.
///
/// `Some(true)` when the grant covers the request, `Some(false)` when the document is a
/// policy that does not grant it, and `None` when `document` is not a policy this can be
/// read out of — the same case [`admits_service_principal`] leaves to its caller, for the
/// reason the module docs give.
///
/// Two further differences follow from going through that evaluator, both of them IAM's own
/// reading rather than this module's: the principal is compared LITERALLY (the context-free
/// reading folds case, which the `Principal` element's own grammar does not ask for), and it
/// is compared against every principal KIND a statement names, so a document naming this
/// value under `AWS` rather than under `Service` grants to it as well.
pub fn grants_service_principal(
    document: &str,
    principal: &str,
    role_arn: &str,
    context: &[(String, String)],
) -> Option<bool> {
    // Whether this is a policy AT ALL is the one thing the evaluator does not report back
    // (it holds a document it cannot read as stating nothing), so it is asked here — and the
    // document itself is handed on AS IT STANDS, in whichever of the two forms a store
    // returned it, because the evaluator reads it through this same parse.
    parse_policy(document)?;
    let principals = [principal.to_string()];
    let request =
        super::iam_authorization::Request::new(ASSUME_ROLE_ACTION, role_arn, &principals)
            .with_context(context);
    Some(
        super::iam_authorization::resource_decision(document, &request)
            == super::iam_authorization::Decision::Allowed,
    )
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

/// Whether one statement is ABOUT `principal` performing `action` on the role — whatever
/// effect it then states.
fn statement_covers(statement: &::serde_json::Value, principal: &str, action: &str) -> bool {
    let names_principal = json_strings(&statement["Principal"]["Service"])
        .iter()
        .any(|service| service.eq_ignore_ascii_case(principal));
    let names_action = json_strings(&statement["Action"])
        .iter()
        .any(|named| action_covers(named, action));
    names_principal && names_action
}

/// Whether one `Action` entry covers `action`: the action itself, or a wildcard IAM admits
/// in its place (`*`, or a prefix wildcard such as `sts:*` or `sts:Assume*`).
fn action_covers(named: &str, action: &str) -> bool {
    if named == "*" || named.eq_ignore_ascii_case(action) {
        return true;
    }
    match named.strip_suffix('*') {
        Some(prefix) => {
            action.len() >= prefix.len() && action[..prefix.len()].eq_ignore_ascii_case(prefix)
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

    /// RULE: a service that performs SEVERAL STS actions on the role it assumes is admitted
    /// only by a document that grants the principal EVERY one of them; a document that
    /// withholds any single action withholds the whole assume.
    #[test]
    fn every_action_a_service_performs_has_to_be_granted() {
        // The session actions a service that assumes a role WITH session tags and a source
        // identity performs on it, beside the assume itself.
        let actions = [ASSUME_ROLE_ACTION, "sts:SetSourceIdentity", "sts:TagSession"];
        // The rows differ along what the rule generalises over: how many of the actions the
        // document names, how they are spread over statements, the wildcards IAM admits in
        // their place, the casing, and the effect.
        let rows: [(&str, String, Option<bool>); 8] = [
            (
                "every action in one statement",
                policy(
                    "Allow",
                    "[\"sts:AssumeRole\",\"sts:SetSourceIdentity\",\"sts:TagSession\"]",
                    "\"fis.amazonaws.com\"",
                ),
                Some(true),
            ),
            (
                "every action, one per statement",
                format!(
                    "{{\"Statement\":[{}]}}",
                    actions
                        .iter()
                        .map(|action| format!(
                            "{{\"Effect\":\"Allow\",\
                             \"Principal\":{{\"Service\":\"fis.amazonaws.com\"}},\
                             \"Action\":\"{action}\"}}"
                        ))
                        .collect::<Vec<String>>()
                        .join(",")
                ),
                Some(true),
            ),
            (
                "a service wildcard covering them all",
                policy("Allow", "\"sts:*\"", "\"fis.amazonaws.com\""),
                Some(true),
            ),
            (
                "every action, named in another casing",
                policy(
                    "Allow",
                    "[\"STS:ASSUMEROLE\",\"sts:setsourceidentity\",\"Sts:TagSession\"]",
                    "\"fis.amazonaws.com\"",
                ),
                Some(true),
            ),
            (
                "the assume alone, without the session actions",
                policy("Allow", "\"sts:AssumeRole\"", "\"fis.amazonaws.com\""),
                Some(false),
            ),
            (
                "all but one of them",
                policy(
                    "Allow",
                    "[\"sts:AssumeRole\",\"sts:TagSession\"]",
                    "\"fis.amazonaws.com\"",
                ),
                Some(false),
            ),
            (
                "every action, explicitly denied",
                policy(
                    "Deny",
                    "[\"sts:AssumeRole\",\"sts:SetSourceIdentity\",\"sts:TagSession\"]",
                    "\"fis.amazonaws.com\"",
                ),
                Some(false),
            ),
            ("not a policy at all", "this is not json".to_string(), None),
        ];

        for (case, document, expected) in rows {
            let admitted =
                admits_service_principal_actions(&document, "fis.amazonaws.com", &actions);
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

    /// The ROLE a trust document is read for, as the context-aware reading is handed one.
    const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/delivery";

    /// A trust policy with one statement, written as a client would send it, carrying
    /// `condition` as its `Condition` element (`""` for a statement with none).
    fn conditioned(condition: &str) -> String {
        let element = if condition.is_empty() {
            String::new()
        } else {
            format!(",\"Condition\":{condition}")
        };
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"Service\":\"bedrock.amazonaws.com\"}},\
             \"Action\":\"sts:AssumeRole\"{element}}}]}}"
        )
    }

    /// The reading a service AUTHORIZING one assume asks: the grant has to cover the
    /// request, so the statement's `Condition` is decided against the keys that request
    /// carries and a clause this cannot decide is no grant.
    #[test]
    fn a_grant_covers_an_assume_only_where_its_conditions_are_satisfied() {
        let context = [(
            "aws:SourceAccount".to_string(),
            "000000000000".to_string(),
        )];
        // The rows differ along what the rule generalises over: whether the grant carries a
        // condition at all, which operator it is written under, whether the request carries
        // a value for the key, and whether the value it carries satisfies the clause.
        let rows: [(&str, String, Option<bool>); 7] = [
            ("an unconditioned grant", conditioned(""), Some(true)),
            (
                "a clause over a key the request carries, which holds",
                conditioned("{\"StringEquals\":{\"aws:SourceAccount\":\"000000000000\"}}"),
                Some(true),
            ),
            (
                "the same clause over another value",
                conditioned("{\"StringEquals\":{\"aws:SourceAccount\":\"210987654321\"}}"),
                Some(false),
            ),
            (
                "a clause over a key the request carries no value for",
                conditioned(
                    "{\"ArnEquals\":{\"aws:SourceArn\":\
                     \"arn:aws:bedrock:us-east-1:000000000000:model-customization-job/*\"}}",
                ),
                Some(false),
            ),
            (
                // Every clause of a statement has to hold, so one the request cannot
                // decide withholds the grant whatever the others say.
                "a clause that holds beside one the request cannot decide",
                conditioned(
                    "{\"StringEquals\":{\"aws:SourceAccount\":\"000000000000\"},\
                     \"ArnEquals\":{\"aws:SourceArn\":\"arn:aws:bedrock:us-east-1:*\"}}",
                ),
                Some(false),
            ),
            (
                "a statement that grants another principal only",
                "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                 \"Principal\":{\"Service\":\"lambda.amazonaws.com\"},\
                 \"Action\":\"sts:AssumeRole\"}]}"
                    .to_string(),
                Some(false),
            ),
            ("not a policy at all", "this is not json".to_string(), None),
        ];

        for (case, document, expected) in rows {
            let granted =
                grants_service_principal(&document, "bedrock.amazonaws.com", ROLE_ARN, &context);
            assert_eq!(
                granted, expected,
                "case {case:?}: expected {expected:?}, got {granted:?} for {document}"
            );
        }
    }

    /// An explicit `Deny` wins here too, and the URL-encoded form a store hands a document
    /// back in reads as the policy it encodes — the two properties the context-free reading
    /// has, held by the reading that decides conditions as well.
    #[test]
    fn the_context_aware_reading_keeps_denies_and_either_document_form() {
        let context: [(String, String); 0] = [];
        let denied = "{\"Version\":\"2012-10-17\",\"Statement\":[\
                      {\"Effect\":\"Allow\",\"Principal\":{\"Service\":\"bedrock.amazonaws.com\"},\
                      \"Action\":\"sts:AssumeRole\"},\
                      {\"Effect\":\"Deny\",\"Principal\":{\"Service\":\"bedrock.amazonaws.com\"},\
                      \"Action\":\"sts:AssumeRole\"}]}";
        let granted = grants_service_principal(denied, "bedrock.amazonaws.com", ROLE_ARN, &context);
        assert_eq!(
            granted,
            Some(false),
            "case an explicit deny beside the grant: expected no grant, got {granted:?}"
        );

        let plain = conditioned("");
        let encoded = plain
            .replace('{', "%7B")
            .replace('}', "%7D")
            .replace('"', "%22");
        for (case, document) in [("plain", plain.clone()), ("url-encoded", encoded)] {
            let granted =
                grants_service_principal(&document, "bedrock.amazonaws.com", ROLE_ARN, &context);
            assert_eq!(
                granted,
                Some(true),
                "case {case:?}: expected the grant to be read, got {granted:?}"
            );
        }
    }
}
