// SPDX-License-Identifier: CC0-1.0

//! The ONE grammar of an IAM JSON **resource-based** policy document, for every service
//! whose API takes one as a string and has to decide whether to accept it.
//!
//! A member documented as "a policy document, formatted in JSON" is not satisfied by any
//! JSON value: it carries a document in the IAM JSON policy language, and a service that
//! only asks `serde_json` whether the text parses accepts `5`, `"hello"`, `[]` and `{}` as
//! policies and then reports them back as the resource's policy. The grammar below is what
//! separates a policy from any other JSON, stated once so two services handed the same
//! document cannot disagree about whether it is one.
//!
//! The grammar, in the IAM JSON policy reference's own order:
//!
//! * the document is a JSON **object** — the policy's top-level elements;
//! * a `Version` it declares is one of [`POLICY_VERSIONS`]; a document may omit it;
//! * `Statement` is **required** and holds either one statement object or a non-empty array
//!   of them;
//! * every statement carries an `Effect` of [`POLICY_EFFECTS`], at least one action
//!   (`Action` or its complement `NotAction`) and — because a RESOURCE-based policy is the
//!   one that states WHO the permission is granted to — at least one principal (`Principal`
//!   or its complement `NotPrincipal`).
//!
//! `Resource` is deliberately NOT required: in a resource-based policy the resource the
//! document is attached to is the implicit one, which is why IAM's resource-policy examples
//! omit it.
//!
//! This judges only the SHAPE of a document. What a policy then SAYS — whether it allows a
//! request — is [`super::iam_authorization`], and the two read the same elements in the
//! same forms (a string, a list of them, or the `{"AWS": …}` object a `Principal` is
//! written as), so a document this accepts is one that module can evaluate.
//!
//! Each caller words its own modelled fault: [`PolicyDefect`] says WHAT the document failed
//! to be and nothing about any one service's error shape, so a service that reports a
//! single opaque "invalid policy" phrase can ignore it and one that names the member can
//! render it.

/// The policy-language versions an IAM JSON policy document may declare. The IAM JSON
/// policy reference documents exactly two `Version` values: `2012-10-17`, the current one,
/// and `2008-10-17`, the legacy one that no new policy should use.
pub const POLICY_VERSIONS: [&str; 2] = ["2012-10-17", "2008-10-17"];

/// The values an IAM statement's `Effect` may take — "the `Effect` element is required and
/// specifies whether the statement results in an allow or an explicit deny".
pub const POLICY_EFFECTS: [&str; 2] = ["Allow", "Deny"];

/// What a document that is not a resource-based policy failed to be.
///
/// One variant per clause of the grammar, so a caller can word a fault that says which
/// clause the document broke — or ignore it and state its own opaque phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDefect {
    /// The text is not JSON at all.
    NotJson,
    /// The text is JSON, but not an object: a scalar or an array is not a policy.
    NotAnObject,
    /// `Version` is declared as something other than one of [`POLICY_VERSIONS`].
    UnsupportedVersion,
    /// There is no `Statement`, or it holds no statement at all.
    NoStatement,
    /// A `Statement` entry is not an object.
    StatementNotAnObject,
    /// A statement declares no `Effect` of [`POLICY_EFFECTS`].
    MissingEffect,
    /// A statement names no action, by either `Action` or `NotAction`.
    MissingAction,
    /// A statement names no principal, by either `Principal` or `NotPrincipal`.
    MissingPrincipal,
}

impl PolicyDefect {
    /// The clause of the grammar this document broke, as one phrase a fault message can be
    /// built around. It describes the DOCUMENT, naming no member of any service's API.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotJson => "the document is not JSON",
            Self::NotAnObject => "the document is not a JSON object",
            Self::UnsupportedVersion => "the document declares a policy version that does not exist",
            Self::NoStatement => "the document declares no statement",
            Self::StatementNotAnObject => "a statement is not a JSON object",
            Self::MissingEffect => "a statement declares no Effect of Allow or Deny",
            Self::MissingAction => "a statement names no Action or NotAction",
            Self::MissingPrincipal => "a statement names no Principal or NotPrincipal",
        }
    }
}

impl ::std::fmt::Display for PolicyDefect {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The clause of the resource-policy grammar `text` breaks, or `None` when `text` is a
/// resource-based policy document.
///
/// Parsing is `serde_json`'s; only the grammar on top of it is stated here. Nothing about a
/// document's SIZE is judged: a length bound belongs to the member that carries it, which
/// the framework's boundary validator already enforces from the model.
pub fn resource_policy_defect(text: &str) -> Option<PolicyDefect> {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(text) else {
        return Some(PolicyDefect::NotJson);
    };
    resource_policy_defect_of(&document)
}

/// [`resource_policy_defect`] for a document that is already parsed — for a caller that
/// parsed it to read something else out of it and must not parse it twice.
pub fn resource_policy_defect_of(document: &::serde_json::Value) -> Option<PolicyDefect> {
    let Some(document) = document.as_object() else {
        return Some(PolicyDefect::NotAnObject);
    };

    if let Some(version) = document.get("Version") {
        let declared = version.as_str().unwrap_or_default();
        if !POLICY_VERSIONS.contains(&declared) {
            return Some(PolicyDefect::UnsupportedVersion);
        }
    }

    let statements = match document.get("Statement") {
        // One statement may be written as the object itself rather than as a list.
        Some(::serde_json::Value::Array(entries)) if !entries.is_empty() => {
            entries.iter().collect::<Vec<&::serde_json::Value>>()
        }
        Some(::serde_json::Value::Object(_)) => {
            vec![&document["Statement"]]
        }
        _ => return Some(PolicyDefect::NoStatement),
    };

    statements.into_iter().find_map(statement_defect)
}

/// The clause one statement breaks, or `None` when it is a resource-policy statement.
fn statement_defect(statement: &::serde_json::Value) -> Option<PolicyDefect> {
    let Some(statement) = statement.as_object() else {
        return Some(PolicyDefect::StatementNotAnObject);
    };
    let effect = statement
        .get("Effect")
        .and_then(::serde_json::Value::as_str)
        .unwrap_or_default();
    if !POLICY_EFFECTS.contains(&effect) {
        return Some(PolicyDefect::MissingEffect);
    }
    let names = |keys: [&str; 2]| {
        keys.iter()
            .any(|key| statement.get(*key).is_some_and(names_values))
    };
    if !names(["Action", "NotAction"]) {
        return Some(PolicyDefect::MissingAction);
    }
    if !names(["Principal", "NotPrincipal"]) {
        return Some(PolicyDefect::MissingPrincipal);
    }
    None
}

/// Whether a policy element names at least one value: a non-empty string, a non-empty array
/// of them, or — as `Principal` is written — a non-empty object mapping a principal type to
/// either of those.
fn names_values(element: &::serde_json::Value) -> bool {
    match element {
        ::serde_json::Value::String(value) => !value.is_empty(),
        ::serde_json::Value::Array(entries) => {
            !entries.is_empty() && entries.iter().all(names_values)
        }
        ::serde_json::Value::Object(members) => {
            !members.is_empty() && members.values().all(names_values)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resource-based policy with one statement, written as a client sends it.
    fn policy(statement: &str) -> String {
        format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statement}]}}")
    }

    /// The statement every accepting row starts from: an `Effect`, an `Action` and a
    /// `Principal`.
    const STATEMENT: &str = "{\"Effect\":\"Allow\",\"Action\":\"svc:DoThing\",\
                             \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"}}";

    #[test]
    fn the_grammar_separates_a_resource_policy_from_any_other_json() {
        // The rows differ along what the rule generalises over: the JSON type of the
        // document, the form `Statement` takes (one object or a list), the number of
        // statements, which element is missing, and the form each element is written in.
        let rows: [(&str, String, Option<PolicyDefect>); 20] = [
            ("one statement in a list", policy(STATEMENT), None),
            (
                "two statements",
                policy(&format!("{STATEMENT},{STATEMENT}")),
                None,
            ),
            (
                "one statement written as the object itself",
                format!("{{\"Version\":\"2012-10-17\",\"Statement\":{STATEMENT}}}"),
                None,
            ),
            (
                "no Version at all, which a policy may omit",
                format!("{{\"Statement\":[{STATEMENT}]}}"),
                None,
            ),
            (
                "the legacy policy version",
                format!("{{\"Version\":\"2008-10-17\",\"Statement\":[{STATEMENT}]}}"),
                None,
            ),
            (
                "an Action list and a Principal string",
                policy(
                    "{\"Effect\":\"Deny\",\"Action\":[\"svc:A\",\"svc:B\"],\
                     \"Principal\":\"*\"}",
                ),
                None,
            ),
            (
                "the complements of Action and Principal",
                policy(
                    "{\"Effect\":\"Deny\",\"NotAction\":\"svc:A\",\
                     \"NotPrincipal\":{\"AWS\":[\"arn:aws:iam::000000000000:root\"]}}",
                ),
                None,
            ),
            (
                "a statement carrying a Sid, a Resource and a Condition too",
                policy(
                    "{\"Sid\":\"one\",\"Effect\":\"Allow\",\"Action\":\"svc:A\",\
                     \"Principal\":\"*\",\"Resource\":\"*\",\
                     \"Condition\":{\"StringEquals\":{\"aws:PrincipalOrgID\":\"o-1\"}}}",
                ),
                None,
            ),
            ("a bare number", "5".to_string(), Some(PolicyDefect::NotAnObject)),
            (
                "a bare string",
                "\"hello\"".to_string(),
                Some(PolicyDefect::NotAnObject),
            ),
            ("an empty array", "[]".to_string(), Some(PolicyDefect::NotAnObject)),
            (
                "an array of statements with no document around them",
                format!("[{STATEMENT}]"),
                Some(PolicyDefect::NotAnObject),
            ),
            ("null", "null".to_string(), Some(PolicyDefect::NotAnObject)),
            (
                "an empty object",
                "{}".to_string(),
                Some(PolicyDefect::NoStatement),
            ),
            (
                "a Statement that is an empty list",
                "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
                Some(PolicyDefect::NoStatement),
            ),
            (
                "a Statement that is a string",
                "{\"Statement\":\"let me in\"}".to_string(),
                Some(PolicyDefect::NoStatement),
            ),
            (
                "a version no policy language has",
                format!("{{\"Version\":\"2024-01-01\",\"Statement\":[{STATEMENT}]}}"),
                Some(PolicyDefect::UnsupportedVersion),
            ),
            (
                "a statement with no Effect",
                policy("{\"Action\":\"svc:A\",\"Principal\":\"*\"}"),
                Some(PolicyDefect::MissingEffect),
            ),
            (
                "a statement with no Action",
                policy("{\"Effect\":\"Allow\",\"Principal\":\"*\"}"),
                Some(PolicyDefect::MissingAction),
            ),
            (
                "a statement with no Principal",
                policy("{\"Effect\":\"Allow\",\"Action\":\"svc:A\"}"),
                Some(PolicyDefect::MissingPrincipal),
            ),
        ];

        for (case, document, expected) in rows {
            let defect = resource_policy_defect(&document);
            assert_eq!(
                defect, expected,
                "case {case:?}: expected {expected:?}, got {defect:?} for {document}"
            );
        }
    }

    #[test]
    fn a_defect_is_reported_for_a_statement_wherever_it_sits_in_the_list() {
        // The grammar holds of EVERY statement, not just the first, so the same broken
        // statement is caught at each position of the list.
        let broken = "{\"Effect\":\"Allow\",\"Action\":\"svc:A\"}";
        for (case, document) in [
            ("broken first", policy(&format!("{broken},{STATEMENT}"))),
            ("broken last", policy(&format!("{STATEMENT},{broken}"))),
            (
                "broken in the middle",
                policy(&format!("{STATEMENT},{broken},{STATEMENT}")),
            ),
        ] {
            let defect = resource_policy_defect(&document);
            assert_eq!(
                defect,
                Some(PolicyDefect::MissingPrincipal),
                "case {case:?}: every statement is judged, got {defect:?}"
            );
        }
    }

    #[test]
    fn text_that_is_not_json_is_reported_as_not_json() {
        for case in ["not a policy", "{", "[1,2", "\"unterminated", "", "{\"Statement\":}"] {
            let defect = resource_policy_defect(case);
            assert_eq!(
                defect,
                Some(PolicyDefect::NotJson),
                "case {case:?}: text that is not JSON is not a policy, got {defect:?}"
            );
        }
    }

    #[test]
    fn an_element_written_as_an_empty_container_names_nothing() {
        for (case, statement, expected) in [
            (
                "an empty Action list",
                "{\"Effect\":\"Allow\",\"Action\":[],\"Principal\":\"*\"}",
                PolicyDefect::MissingAction,
            ),
            (
                "an empty Action string",
                "{\"Effect\":\"Allow\",\"Action\":\"\",\"Principal\":\"*\"}",
                PolicyDefect::MissingAction,
            ),
            (
                "an empty Principal object",
                "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":{}}",
                PolicyDefect::MissingPrincipal,
            ),
            (
                "a Principal whose type names nothing",
                "{\"Effect\":\"Allow\",\"Action\":\"svc:A\",\"Principal\":{\"AWS\":[]}}",
                PolicyDefect::MissingPrincipal,
            ),
            (
                "an Effect that is not Allow or Deny",
                "{\"Effect\":\"Maybe\",\"Action\":\"svc:A\",\"Principal\":\"*\"}",
                PolicyDefect::MissingEffect,
            ),
        ] {
            let defect = resource_policy_defect(&policy(statement));
            assert_eq!(
                defect,
                Some(expected),
                "case {case:?}: expected {expected:?}, got {defect:?}"
            );
        }
    }
}
