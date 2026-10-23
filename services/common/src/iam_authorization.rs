// SPDX-License-Identifier: CC0-1.0

//! The ONE evaluation of IAM policy documents against a request, for every service that
//! decides server-side whether the caller may perform an action on a resource.
//!
//! An emulated service that enforces authorization has to answer the same question IAM
//! answers: given the documents in force — the calling principal's identity-based
//! policies, and the resource's own resource-based policy — may this principal perform
//! this action on this resource? That is ONE rule, so it lives here once rather than per
//! service: two services asked about the same document must not answer differently. The
//! documents themselves are collected by the service (its own store holds the resource
//! policy, the IAM store holds the principal's), which is why nothing here reaches a
//! store or names a service.
//!
//! The rule, in IAM's own evaluation order ("Determining whether a request is allowed or
//! denied within an account"):
//!
//! * a statement is ABOUT the request only when its action element matches the requested
//!   action, its resource element matches the requested resource, and — for a
//!   resource-based policy — its principal element names the calling principal;
//! * an explicit `Deny` among those statements WINS, whatever any `Allow` says, and
//!   whichever order they are written in;
//! * otherwise the request is allowed when at least one of them is an `Allow`
//!   ([`Decision::Allowed`]); a document that is about the request but says neither is
//!   [`Decision::NotStated`], which within one account means "no grant from here" rather
//!   than "refused" — the identity-based and resource-based decisions are combined by
//!   [`is_allowed`], where either grant suffices and a single deny refuses.
//!
//! `Action`/`NotAction`, `Resource`/`NotResource` and `Principal`/`NotPrincipal` are each
//! either one string or a list of them, and the `Not…` spelling matches everything its
//! values do NOT. IAM compares action names case-insensitively and resource ARNs
//! case-sensitively, with `*` (any run) and `?` (one character) wildcards in both
//! ([`wildcard_match`]); a principal is compared literally, because the IAM grammar gives
//! `Principal` no partial wildcards — only the whole-element `"*"`.
//!
//! ## Why this is not the only pattern matcher in the tree
//!
//! Two services carry their own IAM-shaped matcher, and neither is a caller this module
//! can take over:
//!
//! * IAM Access Analyzer's `check_no_new_access` owns a POLICY COMPARISON engine (does
//!   policy B grant access policy A does not?), which enumerates request contexts under a
//!   step and time budget and reports WHY it cannot decide, in that operation's own
//!   vocabulary. Its matcher is charged against that budget; this one answers a single
//!   access request, where the documents are already bounded by the operations that
//!   accepted them, so it needs no budget and reports no reasons.
//! * Amazon DynamoDB's resource-policy reader answers the same shape of question this
//!   module does and SHOULD move onto it, which is a change to that service's handlers
//!   (and to its tests) rather than to this one; it is left for the issue that owns it.
//!
//! ## Conditions
//!
//! A statement's `Condition` is evaluated only for the keys the caller puts in the
//! [`Request::context`] it passes (`rum:alias`, say, which CloudWatch RUM documents for
//! `PutRumEvents`) and only for the string operators below. A clause this CAN decide is
//! part of whether its statement applies at all, whichever effect the statement states: a
//! `Deny` whose condition demonstrably does not hold refuses nothing. A clause this CANNOT
//! decide — an operator it does not implement, or a key the request context does not carry
//! — makes the statement UNDECIDED, and an undecided statement never grants (an `Allow`
//! whose condition may not hold is not a grant) while it still refuses (a `Deny` whose
//! condition may hold is never weakened into permission). That is the fail-closed reading
//! in both directions; nothing here guesses a condition value.

/// The string condition operators this evaluator decides. Everything else — the numeric,
/// date, IP, ARN and `Null` operators, and the `ForAnyValue:`/`ForAllValues:` set
/// qualifiers — leaves the statement undecided (see the module docs).
const STRING_OPERATORS: &[&str] = &[
    "StringEquals",
    "StringNotEquals",
    "StringEqualsIgnoreCase",
    "StringNotEqualsIgnoreCase",
    "StringLike",
    "StringNotLike",
];

/// The most bytes one policy document this evaluator reads may carry.
///
/// Every document that reaches here came through an operation that bounds it (IAM bounds a
/// managed policy at 6,144 characters and an inline role policy at 10,240; CloudWatch RUM
/// bounds an app monitor's resource policy at 4 KB), so this is a backstop an accepted
/// document never meets: it keeps a document that somehow got past those from making the
/// wildcard matcher — whose cost is the product of the pattern and value lengths — do
/// unbounded work. A longer document is read as stating nothing.
pub const MAX_POLICY_BYTES: usize = 64 * 1024;

/// What one set of policy documents says about a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// At least one statement about the request is an `Allow`, and none is a `Deny`.
    Allowed,
    /// A statement about the request is an explicit `Deny`.
    Denied,
    /// No statement is about the request at all (or the only ones that are cannot be
    /// decided): these documents neither grant nor refuse.
    NotStated,
}

/// The access request a decision is made about.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// The IAM action, service prefix included: `rum:PutRumEvents`.
    pub action: &'a str,
    /// The ARN of the resource the action is taken on.
    pub resource: &'a str,
    /// Every spelling of the calling principal a resource-based policy may name — the
    /// assumed-role session ARN and the role's own ARN, say. Unused when the documents are
    /// identity-based, where statements name no principal.
    pub principals: &'a [String],
    /// The condition keys whose values this request knows, as `(key, value)` pairs. A
    /// clause over any other key leaves its statement undecided.
    pub context: &'a [(String, String)],
}

impl<'a> Request<'a> {
    /// A request with no condition context: the common case, where the operation
    /// publishes no condition keys of its own.
    pub fn new(action: &'a str, resource: &'a str, principals: &'a [String]) -> Self {
        Self {
            action,
            resource,
            principals,
            context: &[],
        }
    }

    /// The same request, carrying the condition keys `context` names.
    pub fn with_context(mut self, context: &'a [(String, String)]) -> Self {
        self.context = context;
        self
    }
}

/// Which grammar a document follows, which is what decides how its statements are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flavor {
    /// Attached to a principal, so a statement names no `Principal` and must name the
    /// resources it covers.
    Identity,
    /// Attached to the resource, so a statement names the principals it grants to, and a
    /// statement that names no `Resource` covers the resource the policy is attached to.
    Resource,
}

/// What the principal's identity-based policies say about the request.
///
/// `documents` are the policies in force on the caller — its inline policies and the
/// documents of the managed policies attached to it — in any order: the result does not
/// depend on it, because a `Deny` in any of them wins.
///
/// Each document is READ here, so a caller asking about SEVERAL requests over the same
/// documents (one action on each of the resources it reaches, say) reads them once with
/// [`prepare_documents`] and asks through [`identity_decision_prepared`] instead, which
/// keeps the cost of n requests n statement walks rather than n parses.
pub fn identity_decision(documents: &[String], request: &Request<'_>) -> Decision {
    identity_decision_prepared(&prepare_documents(documents), request)
}

/// One policy document READ ONCE, for a caller that decides several requests against the
/// same documents.
///
/// A document this cannot read as a policy — one past [`MAX_POLICY_BYTES`], or one that is
/// not a policy at all — is held as "states nothing", which is exactly what deciding it
/// would have answered; the operation that accepted the document is what refuses a
/// malformed one.
#[derive(Debug, Clone)]
pub struct PreparedDocument {
    policy: Option<::serde_json::Value>,
}

impl PreparedDocument {
    /// Read `document` once.
    #[must_use]
    pub fn read(document: &str) -> Self {
        let policy = (document.len() <= MAX_POLICY_BYTES)
            .then(|| super::iam_trust_policy::parse_policy(document))
            .flatten();
        Self { policy }
    }
}

/// Every document of `documents`, read once each. See [`PreparedDocument`].
#[must_use]
pub fn prepare_documents(documents: &[String]) -> Vec<PreparedDocument> {
    documents
        .iter()
        .map(|document| PreparedDocument::read(document))
        .collect()
}

/// [`identity_decision`] over documents already read: the SAME rule, and the same answer
/// for the same documents — the reading is all that is hoisted out.
#[must_use]
pub fn identity_decision_prepared(
    documents: &[PreparedDocument],
    request: &Request<'_>,
) -> Decision {
    combine(
        documents
            .iter()
            .map(|document| decide(document, request, Flavor::Identity)),
    )
}

/// What the resource's own resource-based policy says about the request.
pub fn resource_decision(document: &str, request: &Request<'_>) -> Decision {
    decide(&PreparedDocument::read(document), request, Flavor::Resource)
}

/// Whether the request is allowed, given the two decisions.
///
/// IAM's order: an explicit `Deny` from either side refuses; otherwise, within one
/// account, an `Allow` from EITHER the identity-based policies or the resource-based
/// policy is enough ("the resource-based policy and the identity-based policy are
/// evaluated together, and access is granted if either allows it"). With neither — the
/// implicit deny — the request is refused.
pub fn is_allowed(identity: Decision, resource: Decision) -> bool {
    if identity == Decision::Denied || resource == Decision::Denied {
        return false;
    }
    identity == Decision::Allowed || resource == Decision::Allowed
}

/// Whether `pattern` matches `value`, with IAM's `*` (any run of characters, including
/// none) and `?` (exactly one character) wildcards.
///
/// `case_sensitive` is false for an action name (IAM compares those case-insensitively)
/// and true for a resource ARN. The walk is the classic single-pass wildcard match with
/// one backtrack point per `*`, so its cost is bounded by the product of the two lengths
/// and it never recurses.
pub fn wildcard_match(pattern: &str, value: &str, case_sensitive: bool) -> bool {
    let fold = |text: &str| -> Vec<char> {
        if case_sensitive {
            text.chars().collect()
        } else {
            text.to_lowercase().chars().collect()
        }
    };
    let pattern = fold(pattern);
    let value = fold(value);

    let mut pattern_at = 0;
    let mut value_at = 0;
    // The last `*` seen, and how much of `value` it had consumed when it was seen: the one
    // place the walk returns to when a literal comparison fails after it.
    let mut star_at: Option<usize> = None;
    let mut star_consumed = 0;
    while value_at < value.len() {
        match pattern.get(pattern_at) {
            Some('*') => {
                star_at = Some(pattern_at);
                star_consumed = value_at;
                pattern_at += 1;
            }
            Some('?') => {
                pattern_at += 1;
                value_at += 1;
            }
            Some(literal) if *literal == value[value_at] => {
                pattern_at += 1;
                value_at += 1;
            }
            // Either the pattern ran out or it disagrees here: give the last `*` one more
            // character and try again from just after it.
            _ => match star_at {
                Some(star) => {
                    pattern_at = star + 1;
                    star_consumed += 1;
                    value_at = star_consumed;
                }
                None => return false,
            },
        }
    }
    // Trailing `*`s match the empty remainder.
    pattern[pattern_at..].iter().all(|glyph| *glyph == '*')
}

/// The decision ONE document reaches about the request.
fn decide(document: &PreparedDocument, request: &Request<'_>, flavor: Flavor) -> Decision {
    let Some(policy) = document.policy.as_ref() else {
        // Not a policy this can be read out of: it states nothing.
        return Decision::NotStated;
    };
    let statement = &policy["Statement"];
    let statements: Vec<&::serde_json::Value> = match statement {
        ::serde_json::Value::Array(entries) => entries.iter().collect(),
        ::serde_json::Value::Object(_) => vec![statement],
        _ => return Decision::NotStated,
    };

    let mut allowed = false;
    for entry in statements {
        let effect = entry["Effect"].as_str().unwrap_or_default();
        let denies = effect.eq_ignore_ascii_case("Deny");
        let allows = effect.eq_ignore_ascii_case("Allow");
        if !denies && !allows {
            // Neither of the two effects the grammar admits: the statement says nothing.
            continue;
        }
        if !covers(entry, request, flavor) {
            continue;
        }
        // The statement's condition, where this evaluator can decide it, is part of
        // whether the statement applies at all — to a `Deny` as much as to an `Allow`. A
        // clause it CANNOT decide is read fail-closed in both directions: it withholds a
        // grant and keeps a refusal (see the module docs).
        match conditions_hold(entry, request) {
            // Decidably false: the statement does not apply, whichever effect it states.
            Some(false) => continue,
            // Undecidable: no grant, but a refusal stands.
            None if allows => continue,
            _ => {}
        }
        if denies {
            return Decision::Denied;
        }
        allowed = true;
    }
    if allowed {
        Decision::Allowed
    } else {
        Decision::NotStated
    }
}

/// Fold the per-document decisions of one document SET: a `Deny` anywhere wins, otherwise
/// an `Allow` anywhere grants.
fn combine(decisions: impl IntoIterator<Item = Decision>) -> Decision {
    let mut allowed = false;
    for decision in decisions {
        match decision {
            Decision::Denied => return Decision::Denied,
            Decision::Allowed => allowed = true,
            Decision::NotStated => {}
        }
    }
    if allowed {
        Decision::Allowed
    } else {
        Decision::NotStated
    }
}

/// Whether one statement is ABOUT the request — whatever effect it then states.
fn covers(statement: &::serde_json::Value, request: &Request<'_>, flavor: Flavor) -> bool {
    names_action(statement, request.action)
        && names_resource(statement, request.resource, flavor)
        && names_principal(statement, request.principals, flavor)
}

/// Whether the statement's `Action`/`NotAction` element covers the requested action.
fn names_action(statement: &::serde_json::Value, action: &str) -> bool {
    match element(statement, "Action", "NotAction") {
        Some((values, negated)) => {
            let hit = values
                .iter()
                .any(|pattern| wildcard_match(pattern, action, false));
            hit != negated
        }
        // A statement that names no action at all governs nothing.
        None => false,
    }
}

/// Whether the statement's `Resource`/`NotResource` element covers the requested resource.
fn names_resource(statement: &::serde_json::Value, resource: &str, flavor: Flavor) -> bool {
    match element(statement, "Resource", "NotResource") {
        Some((values, negated)) => {
            let hit = values
                .iter()
                .any(|pattern| wildcard_match(pattern, resource, true));
            hit != negated
        }
        // An identity-based statement must name the resources it covers. A
        // resource-based one is attached to the resource, so leaving the element out
        // covers exactly that resource.
        None => flavor == Flavor::Resource,
    }
}

/// Whether the statement's `Principal`/`NotPrincipal` element names the caller.
fn names_principal(
    statement: &::serde_json::Value,
    principals: &[String],
    flavor: Flavor,
) -> bool {
    let named = principal_values(&statement["Principal"]);
    let not_named = principal_values(&statement["NotPrincipal"]);
    match flavor {
        // An identity-based policy's statements name no principal: the policy is attached
        // to one. A statement that DOES name one is a resource-policy statement, and is
        // not read as a grant to its own holder.
        Flavor::Identity => named.is_none() && not_named.is_none(),
        Flavor::Resource => {
            let (values, negated) = match (named, not_named) {
                (Some(values), _) => (values, false),
                (None, Some(values)) => (values, true),
                // A resource-based statement that names no principal grants to nobody.
                (None, None) => return false,
            };
            let hit = values.iter().any(|value| {
                *value == "*" || principals.iter().any(|principal| principal == value)
            });
            hit != negated
        }
    }
}

/// The values of a policy element, and whether they came from its negated spelling.
///
/// `None` when the statement carries neither spelling. The positive spelling wins when a
/// statement carries both, which is not a document IAM accepts at all.
fn element<'a>(
    statement: &'a ::serde_json::Value,
    positive: &str,
    negative: &str,
) -> Option<(Vec<&'a str>, bool)> {
    let named = super::iam_trust_policy::json_strings(&statement[positive]);
    if !named.is_empty() {
        return Some((named, false));
    }
    let not_named = super::iam_trust_policy::json_strings(&statement[negative]);
    if !not_named.is_empty() {
        return Some((not_named, true));
    }
    None
}

/// The principals a `Principal`/`NotPrincipal` element names, flattened.
///
/// The element is either the string `"*"`, or a map keyed by principal kind (`AWS`,
/// `Service`, `Federated`, `CanonicalUser`) whose values are each one string or a list of
/// them. `None` when the statement does not carry the element.
fn principal_values(value: &::serde_json::Value) -> Option<Vec<&str>> {
    match value {
        ::serde_json::Value::String(one) => Some(vec![one.as_str()]),
        ::serde_json::Value::Array(many) => Some(
            many.iter()
                .filter_map(::serde_json::Value::as_str)
                .collect(),
        ),
        ::serde_json::Value::Object(kinds) => Some(
            kinds
                .values()
                .flat_map(super::iam_trust_policy::json_strings)
                .collect(),
        ),
        _ => None,
    }
}

/// Whether every condition clause this statement carries holds for the request.
///
/// `Some(true)` when the statement carries no `Condition` or every clause holds,
/// `Some(false)` when a clause this evaluator decides does NOT hold (so the statement does
/// not apply, whichever effect it states), and `None` when a clause cannot be decided at
/// all — an operator outside [`STRING_OPERATORS`], or a key the request context does not
/// carry — which the caller reads fail-closed in both directions.
fn conditions_hold(statement: &::serde_json::Value, request: &Request<'_>) -> Option<bool> {
    let Some(clauses) = statement["Condition"].as_object() else {
        // No condition at all holds trivially; an element that is not a map is not a
        // condition this reads, so it cannot be decided.
        return statement["Condition"].is_null().then_some(true);
    };
    let mut held = true;
    for (operator, keys) in clauses {
        let keys = keys.as_object()?;
        if !STRING_OPERATORS.contains(&operator.as_str()) {
            return None;
        }
        for (key, expected) in keys {
            let actual = request
                .context
                .iter()
                .find(|(known, _)| known.eq_ignore_ascii_case(key))
                .map(|(_, value)| value.as_str())?;
            let expected = super::iam_trust_policy::json_strings(expected);
            held = held && clause_holds(operator, &expected, actual);
        }
    }
    Some(held)
}

/// Whether one decided string clause holds: `expected` are the clause's values, `actual`
/// the value the request carries for its key.
fn clause_holds(operator: &str, expected: &[&str], actual: &str) -> bool {
    let any_equals = |case_sensitive: bool| {
        expected.iter().any(|value| {
            if case_sensitive {
                *value == actual
            } else {
                value.eq_ignore_ascii_case(actual)
            }
        })
    };
    let any_like = || {
        expected
            .iter()
            .any(|pattern| wildcard_match(pattern, actual, true))
    };
    match operator {
        "StringEquals" => any_equals(true),
        "StringNotEquals" => !any_equals(true),
        "StringEqualsIgnoreCase" => any_equals(false),
        "StringNotEqualsIgnoreCase" => !any_equals(false),
        "StringLike" => any_like(),
        "StringNotLike" => !any_like(),
        // Unreachable: the caller checks the operator against `STRING_OPERATORS` first.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The principal the rows below call with, in the two spellings a resource policy may
    /// name it by.
    fn principals() -> Vec<String> {
        vec![
            "arn:aws:sts::000000000000:assumed-role/writer/session".to_string(),
            "arn:aws:iam::000000000000:role/writer".to_string(),
        ]
    }

    /// The resource every row below asks about.
    const RESOURCE: &str = "arn:aws:rum:us-east-1:000000000000:appmonitor/shop";

    /// One identity-based document with the statement text given.
    fn identity(statement: &str) -> String {
        format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statement}]}}")
    }

    #[test]
    fn an_identity_policy_grants_exactly_the_action_and_resource_it_names() {
        let rows: [(&str, &str, Decision); 10] = [
            (
                "the action and the resource, exactly",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"arn:aws:rum:us-east-1:000000000000:appmonitor/shop\"}",
                Decision::Allowed,
            ),
            (
                "a service wildcard and every resource",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:*\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "a prefix wildcard in the action",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:Put*\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "a single-character wildcard in the resource",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"arn:aws:rum:us-east-1:000000000000:appmonitor/sho?\"}",
                Decision::Allowed,
            ),
            (
                "the action in another casing, which IAM folds",
                "{\"Effect\":\"Allow\",\"Action\":\"RUM:PUTRUMEVENTS\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "another action",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:GetAppMonitor\",\"Resource\":\"*\"}",
                Decision::NotStated,
            ),
            (
                "another resource",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"arn:aws:rum:us-east-1:000000000000:appmonitor/other\"}",
                Decision::NotStated,
            ),
            (
                "an explicit deny",
                "{\"Effect\":\"Deny\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::Denied,
            ),
            (
                "a NotAction that excludes the action",
                "{\"Effect\":\"Allow\",\"NotAction\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::NotStated,
            ),
            (
                "a statement naming no resource, which an identity policy must",
                "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\"}",
                Decision::NotStated,
            ),
        ];

        let held = principals();
        for (case, statement, expected) in rows {
            let request = Request::new("rum:PutRumEvents", RESOURCE, &held);
            let reached = identity_decision(&[identity(statement)], &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    #[test]
    fn a_resource_policy_grants_only_the_principals_it_names() {
        let rows: [(&str, &str, Decision); 7] = [
            (
                "the session principal",
                "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:sts::000000000000:assumed-role/writer/session\"},\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "the role principal",
                "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:role/writer\"},\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "every principal",
                "{\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::Allowed,
            ),
            (
                "a statement naming no resource, which the attached resource answers",
                "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:role/writer\"},\"Action\":\"rum:PutRumEvents\"}",
                Decision::Allowed,
            ),
            (
                "another principal",
                "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:role/reader\"},\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::NotStated,
            ),
            (
                "the account root, which delegates to the identity policies",
                "{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"},\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::NotStated,
            ),
            (
                "a deny naming the principal",
                "{\"Effect\":\"Deny\",\"Principal\":\"*\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}",
                Decision::Denied,
            ),
        ];

        let held = principals();
        for (case, statement, expected) in rows {
            let request = Request::new("rum:PutRumEvents", RESOURCE, &held);
            let reached = resource_decision(&identity(statement), &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    #[test]
    fn an_explicit_deny_wins_whichever_document_and_order_it_is_in() {
        let allow =
            "{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}".to_string();
        let deny =
            "{\"Effect\":\"Deny\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\"}".to_string();
        let held = principals();
        let request = Request::new("rum:PutRumEvents", RESOURCE, &held);
        for (case, documents) in [
            (
                "allow then deny, in one document",
                vec![format!("{{\"Statement\":[{allow},{deny}]}}")],
            ),
            (
                "deny then allow, in one document",
                vec![format!("{{\"Statement\":[{deny},{allow}]}}")],
            ),
            (
                "allow in one document, deny in another",
                vec![identity(&allow), identity(&deny)],
            ),
        ] {
            let reached = identity_decision(&documents, &request);
            assert_eq!(
                reached,
                Decision::Denied,
                "case {case:?}: an explicit deny wins, got {reached:?}"
            );
        }
        // And the combination rule: a deny on either side refuses, an allow on either
        // side suffices.
        let rows: [(&str, Decision, Decision, bool); 6] = [
            ("identity allows", Decision::Allowed, Decision::NotStated, true),
            ("the resource allows", Decision::NotStated, Decision::Allowed, true),
            ("both allow", Decision::Allowed, Decision::Allowed, true),
            ("neither states anything", Decision::NotStated, Decision::NotStated, false),
            ("identity denies what the resource allows", Decision::Denied, Decision::Allowed, false),
            ("the resource denies what identity allows", Decision::Allowed, Decision::Denied, false),
        ];
        for (case, identity_said, resource_said, expected) in rows {
            let allowed = is_allowed(identity_said, resource_said);
            assert_eq!(
                allowed, expected,
                "case {case:?}: expected allowed={expected}, got {allowed}"
            );
        }
    }

    #[test]
    fn a_condition_is_honoured_only_where_the_request_carries_its_key() {
        let statement = |condition: &str| {
            identity(&format!(
                "{{\"Effect\":\"Allow\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
                 \"Condition\":{condition}}}"
            ))
        };
        let held = principals();
        let context = [("rum:alias".to_string(), "shop-web".to_string())];
        let rows: [(&str, &str, Decision); 6] = [
            (
                "StringEquals on a key the request carries, matching",
                "{\"StringEquals\":{\"rum:alias\":\"shop-web\"}}",
                Decision::Allowed,
            ),
            (
                "StringEquals on a key the request carries, not matching",
                "{\"StringEquals\":{\"rum:alias\":\"other\"}}",
                Decision::NotStated,
            ),
            (
                "StringLike with a wildcard",
                "{\"StringLike\":{\"rum:alias\":\"shop-*\"}}",
                Decision::Allowed,
            ),
            (
                "StringNotEquals on another value",
                "{\"StringNotEquals\":{\"rum:alias\":\"other\"}}",
                Decision::Allowed,
            ),
            (
                "a key the request does not carry",
                "{\"StringEquals\":{\"aws:SourceVpc\":\"vpc-1\"}}",
                Decision::NotStated,
            ),
            (
                "an operator this evaluator does not decide",
                "{\"DateLessThan\":{\"aws:CurrentTime\":\"2020-01-01T00:00:00Z\"}}",
                Decision::NotStated,
            ),
        ];
        for (case, condition, expected) in rows {
            let request =
                Request::new("rum:PutRumEvents", RESOURCE, &held).with_context(&context);
            let reached = identity_decision(&[statement(condition)], &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
        // A conditional DENY is never weakened by a condition this cannot decide — and a
        // conditional DENY this CAN decide refuses only where its condition holds.
        let request = Request::new("rum:PutRumEvents", RESOURCE, &held);
        let denial = identity(
            "{\"Effect\":\"Deny\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
             \"Condition\":{\"DateLessThan\":{\"aws:CurrentTime\":\"2020-01-01T00:00:00Z\"}}}",
        );
        let reached = identity_decision(&[denial], &request);
        assert_eq!(
            reached,
            Decision::Denied,
            "an undecidable condition must not turn a Deny into permission, got {reached:?}"
        );

        let deny_rows: [(&str, &str, Decision); 2] = [
            (
                "a decidable Deny condition that holds",
                "{\"StringEquals\":{\"rum:alias\":\"shop-web\"}}",
                Decision::Denied,
            ),
            (
                "a decidable Deny condition that does not hold",
                "{\"StringEquals\":{\"rum:alias\":\"other\"}}",
                Decision::NotStated,
            ),
        ];
        for (case, condition, expected) in deny_rows {
            let document = identity(&format!(
                "{{\"Effect\":\"Deny\",\"Action\":\"rum:PutRumEvents\",\"Resource\":\"*\",\
                 \"Condition\":{condition}}}"
            ));
            let request =
                Request::new("rum:PutRumEvents", RESOURCE, &held).with_context(&context);
            let reached = identity_decision(&[document], &request);
            assert_eq!(
                reached, expected,
                "case {case:?}: expected {expected:?}, got {reached:?}"
            );
        }
    }

    #[test]
    fn the_wildcard_matcher_follows_iam_s_two_wildcards() {
        let rows: [(&str, &str, &str, bool, bool); 11] = [
            ("an exact match", "s3:GetObject", "s3:GetObject", true, true),
            ("a trailing star", "s3:*", "s3:GetObject", true, true),
            ("a star in the middle", "s3:Get*ect", "s3:GetObject", true, true),
            ("two stars", "*Get*", "s3:GetObject", true, true),
            ("a question mark", "s3:GetObjec?", "s3:GetObject", true, true),
            (
                "a question mark that must match one character",
                "s3:GetObject?",
                "s3:GetObject",
                true,
                false,
            ),
            ("a star matching nothing", "s3:GetObject*", "s3:GetObject", true, true),
            ("a non-match", "s3:Put*", "s3:GetObject", true, false),
            ("case folding off", "S3:GETOBJECT", "s3:GetObject", true, false),
            ("case folding on", "S3:GETOBJECT", "s3:GetObject", false, true),
            ("every value", "*", "", true, true),
        ];
        for (case, pattern, value, case_sensitive, expected) in rows {
            let matched = wildcard_match(pattern, value, case_sensitive);
            assert_eq!(
                matched, expected,
                "case {case:?}: {pattern:?} against {value:?} (case_sensitive={case_sensitive}) \
                 expected {expected}, got {matched}"
            );
        }
    }
}
