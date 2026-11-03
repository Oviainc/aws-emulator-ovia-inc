// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The framework walks the modelled input constraints and hands [`Refusal`] to
//! `service.rs` STRUCTURALLY — one [`Violation`] per offending value, carrying the path
//! it sits at, what it failed and what it held — and leaves the words to the service,
//! because no model records them. This module is that wording for every constrained
//! member of every operation of this service: ONE formatter, so no operation can state a
//! refusal its own way.
//!
//! The dialect it implements:
//!
//! * the envelope is AWS's — `N validation error(s) detected: ` over one clause per
//!   violation — and a clause names the offending MEMBER PATH and the CONSTRAINT it
//!   broke, both the framework's own;
//! * a member the model marks `@sensitive` is reported WITHOUT the value that broke the
//!   constraint ([`SENSITIVE_MEMBERS`]): the clause reads `Value at '<path>' …`. The
//!   member and the constraint are what a caller acts on, and echoing the value would
//!   put a value the model says not to disclose into every client log that records an
//!   error body;
//! * any other member's value is quoted back as the framework quotes it
//!   (`Value '<value>' at '<path>' …`), which is the only place the server reports the
//!   offending input to the caller;
//! * an ABSENT `@required` member is reported as the `null` it was — a fact of the
//!   request's shape rather than a value the caller sent — `@sensitive` or not, since
//!   there is nothing to disclose;
//! * a `@pattern` requirement advertises the expression this service PUBLISHES for the
//!   member ([`advertised_pattern`]), which is the model's own pattern except where the
//!   two differ: a caller that lifts the expression out of the message gets the one this
//!   service publishes.
//!
//! Nothing here is keyed to an operation or to a submitted value: the rule is applied to
//! whatever [`Violation`] the walk produces, so it holds for every constrained member of
//! every operation — a `Name`'s `@pattern` and `@length`, a `TargetAddress`'s `@length`
//! wherever it is nested, a `Resource`'s or a `ClientRequestToken`'s `@pattern` alike.
//!
//! A violation class this module has no wording for declines the WHOLE refusal (`None`),
//! which keeps the framework's own sentence rather than inventing one: [`ViolationKind`]
//! is `#[non_exhaustive]`, so a class can be added upstream.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// The error code this service answers an input-constraint violation with: the modelled
/// `ValidationException` (`@httpError 400`), which this service declares for every
/// operation. Restated here because `override_refusal` replaces the code and the message
/// TOGETHER — a client branches on the code — and it is the code the framework's own
/// default would have answered with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The members this service marks `@sensitive`, as a validation path spells them.
///
/// `smithy_query(query="shape",shape="NotificationRuleName,TargetAddress,Target")`: the
/// `NotificationRuleName` and `TargetAddress` string shapes both carry `sensitive: true`
/// and are the only sensitive shapes in this model. They are reached as the `Name` member
/// of the create and update requests, as `TargetAddress` on the unsubscribe request, and
/// as `Target$TargetAddress` nested inside the create and subscribe requests; a path
/// segment is lowerCamel, so a violation on one of them is reported at `name` /
/// `targetAddress` — the LAST segment of its path, wherever it is nested.
const SENSITIVE_MEMBERS: &[&str] = &["name", "targetAddress"];

/// Whether `path` names a member this service marks `@sensitive`.
///
/// The LAST segment decides: the member a violation is reported on is the one whose value
/// would be echoed, whatever structure or collection it sits in.
fn names_a_sensitive_member(path: &str) -> bool {
    SENSITIVE_MEMBERS.contains(&path.split('.').next_back().unwrap_or(path))
}

/// The expressions this service PUBLISHES where they differ from the public model's
/// spelling of the same `@pattern`, keyed by that spelling.
///
/// A clause advertises the expression the service publishes for the member, which is the
/// model's pattern verbatim for every member but one. That is OBSERVED, not chosen: in
/// the recorded error paths of this service (`codestar-notifications-create-rule-error-paths`),
/// the refusal of a `NotificationRuleName` outside its pattern quotes the expression
/// without the leading whole-value anchor, while the refusal of a
/// `NotificationRuleResource` in the very next step of the same fixture quotes its own
/// anchored expression in full — so the anchoring is a property of the published
/// expression and not a rendering applied to all of them. Keyed on the EXPRESSION rather
/// than on a member name, so it holds for every member that targets the shape carrying it
/// (`CreateNotificationRule$Name` and `UpdateNotificationRule$Name` alike) wherever it is
/// nested.
const PUBLISHED_PATTERNS: &[(&str, &str)] = &[(r"^[A-Za-z0-9\-_ ]+$", r"[A-Za-z0-9\-_ ]+$")];

/// The expression a `@pattern` clause advertises: the one this service publishes for the
/// member, which is the model's own unless [`PUBLISHED_PATTERNS`] states otherwise.
///
/// The CHECK is the framework's, run against the model's pattern as written — only the
/// rendering is chosen here.
fn advertised_pattern(pattern: &str) -> &str {
    PUBLISHED_PATTERNS
        .iter()
        .find_map(|(modelled, published)| (*modelled == pattern).then_some(*published))
        .unwrap_or(pattern)
}

/// The subject the framework renders for a violation carrying no value, and the subject a
/// clause whose value is WITHHELD carries instead: a sensitive member's value is not
/// absent, so it is not reported as `null`.
const NULL_SUBJECT: &str = "Value null at '";
const WITHHELD_SUBJECT: &str = "Value at '";

/// One violation as a clause of the envelope.
///
/// The requirement sentence and the member path are the framework's own
/// ([`Violation::message`]), so this service cannot drift from them; what this chooses is
/// the expression a `@pattern` advertises and whether the value is quoted.
fn clause(violation: &Violation) -> String {
    let withhold = (violation.value.is_some() || violation.sensitive)
        && names_a_sensitive_member(&violation.path);
    let restated = Violation {
        path: violation.path.clone(),
        kind: match &violation.kind {
            ViolationKind::Pattern(pattern) => {
                ViolationKind::Pattern(advertised_pattern(pattern).to_string())
            }
            other => other.clone(),
        },
        value: if withhold { None } else { violation.value.clone() },
        sensitive: violation.sensitive,
    };
    let stated = restated.message();
    if withhold {
        // Rendered as the framework renders a violation with no value, then stripped of
        // the `null` that stands for one: the value exists and is withheld.
        return stated.replacen(NULL_SUBJECT, WITHHELD_SUBJECT, 1);
    }
    stated
}

/// The `ValidationException` message for a refused request: AWS's envelope over one
/// clause per violation, in the order the walk found them.
///
/// The envelope is restated here rather than taken from
/// [`aws_smithy_schema_server::envelope`], which renders it from the violations
/// themselves: one clause of this dialect withholds a value the framework always
/// renders, so the clauses are built first and joined after.
fn envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations.iter().map(clause).collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// How this service states `refusal`, or `None` to keep the framework's wording.
///
/// Only an input-constraint refusal is reworded. An authentication refusal is a different
/// failure with a different code and status, and nothing about it is this service's to
/// word.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: envelope(violations)?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One violation of `kind` at `path`, holding `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// RULE: a violation the WALK itself withheld — which it hands over with no value and
    /// says so — is stated as a withheld clause and not as an absent member, whatever
    /// this service's own reading of the member path says.
    ///
    /// The walk is what produces this shape (`vendor/smithy-schema-server`'s constraint
    /// walk drops a `@sensitive` member's value as it records the violation), and `Value
    /// null at '<member>'` — the clause an ABSENT member earns — is what the same
    /// violation reads as when the withheld mark is not read.
    #[test]
    fn a_violation_the_walk_withheld_is_not_stated_as_an_absent_member() {
        // (case, the member path, the class, the requirement the clause states)
        let rows = [
            (
                "a sensitive member outside its pattern",
                "name",
                ViolationKind::Pattern(r"^[A-Za-z0-9\-_ ]+$".to_string()),
                r"Member must satisfy regular expression pattern: [A-Za-z0-9\-_ ]+$",
            ),
            (
                "a sensitive member past its length maximum",
                "targetAddress",
                ViolationKind::MaxLength(320),
                "Member must have length less than or equal to 320",
            ),
        ];
        for (case, path, kind, requirement) in rows {
            let withheld = Violation {
                path: path.to_string(),
                kind,
                // What the walk hands over for a member whose value it withheld.
                value: None,
                sensitive: true,
            };
            let stated = clause(&withheld);
            let expected =
                format!("Value at '{path}' failed to satisfy constraint: {requirement}");
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }

        // The same member ABSENT: no value was sent, so the clause states the null it was.
        let absent = violation("name", ViolationKind::MissingRequiredMember, None);
        let stated = clause(&absent);
        assert_eq!(
            stated,
            "Value null at 'name' failed to satisfy constraint: Member must not be null",
            "case \"an absent member\": expected the null clause, got {stated:?}"
        );
    }

    /// The sensitive member paths are the framework's own spelling of the model's member
    /// names, not literals that can drift from them.
    #[test]
    fn the_sensitive_members_are_spelled_as_the_framework_spells_them() {
        for member in ["Name", "TargetAddress"] {
            let spelled = ::aws_smithy_schema_server::constraints::path::member(member);
            assert!(
                SENSITIVE_MEMBERS.contains(&spelled.as_str()),
                "case {member:?}: expected the framework's spelling {spelled:?} among \
                 {SENSITIVE_MEMBERS:?}"
            );
        }
    }

    /// RULE: a `@sensitive` member's clause names the member and the constraint and
    /// withholds the value; every other member's clause quotes it; an absent member is
    /// `null` either way.
    #[test]
    fn a_sensitive_members_value_is_withheld_and_nothing_else_is() {
        // (case, path, kind, submitted value, the clause expected)
        let rows: [(&str, &str, ViolationKind, Option<&str>, &str); 6] = [
            (
                "a sensitive member outside its pattern",
                "name",
                ViolationKind::Pattern(r"^[A-Za-z0-9\-_ ]+$".to_string()),
                Some("bad name!"),
                r"Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9\-_ ]+$",
            ),
            (
                "a sensitive member over its length maximum",
                "targetAddress",
                ViolationKind::MaxLength(320),
                Some("sns"),
                "Value at 'targetAddress' failed to satisfy constraint: Member must have length \
                 less than or equal to 320",
            ),
            (
                "a sensitive member nested in a structure",
                "target.targetAddress",
                ViolationKind::MinLength(1),
                Some(""),
                "Value at 'target.targetAddress' failed to satisfy constraint: Member must have \
                 length greater than or equal to 1",
            ),
            (
                "a sensitive member that was absent",
                "name",
                ViolationKind::MissingRequiredMember,
                None,
                "Value null at 'name' failed to satisfy constraint: Member must not be null",
            ),
            (
                "a member the model does not mark sensitive",
                "resource",
                ViolationKind::Pattern(r"^arn:aws[^:\s]*:[^:\s]+$".to_string()),
                Some("not-an-arn"),
                r"Value 'not-an-arn' at 'resource' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[^:\s]*:[^:\s]+$",
            ),
            (
                "a member whose name merely ends in a sensitive one's",
                "ruleName",
                ViolationKind::MaxLength(64),
                Some("x"),
                "Value 'x' at 'ruleName' failed to satisfy constraint: Member must have length \
                 less than or equal to 64",
            ),
        ];

        for (case, path, kind, value, expected) in rows {
            let stated = clause(&violation(path, kind, value));
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: the envelope counts the clauses, and the code travels with the message.
    #[test]
    fn the_envelope_states_one_clause_per_violation() {
        let one = Refusal::Invalid(vec![violation(
            "name",
            ViolationKind::MaxLength(64),
            Some("over"),
        )]);
        let stated = refused(&one).expect("a constraint refusal is stated");
        assert_eq!(
            stated.code, VALIDATION_EXCEPTION,
            "expected the modelled refusal code, got {:?}",
            stated.code
        );
        assert_eq!(
            stated.message,
            "1 validation error detected: Value at 'name' failed to satisfy constraint: Member \
             must have length less than or equal to 64",
            "expected one clause, got {:?}",
            stated.message
        );

        let two = Refusal::Invalid(vec![
            violation("name", ViolationKind::MaxLength(64), Some("over")),
            violation(
                "resource",
                ViolationKind::Pattern("^arn:.+$".to_string()),
                Some("nope"),
            ),
        ]);
        let stated = refused(&two).expect("a constraint refusal is stated");
        assert_eq!(
            stated.message,
            "2 validation errors detected: Value at 'name' failed to satisfy constraint: Member \
             must have length less than or equal to 64; Value 'nope' at 'resource' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: ^arn:.+$",
            "expected two clauses in the order the walk found them, got {:?}",
            stated.message
        );
    }

    /// RULE: a clause advertises the expression this service publishes for the member —
    /// the model's own pattern, unless [`PUBLISHED_PATTERNS`] states a different one for
    /// that expression.
    ///
    /// The rows are the model's own expressions
    /// (`smithy_query(query="operation",operation="CreateNotificationRule")`), one
    /// published differently and the rest verbatim, plus an expression no member carries
    /// to show that nothing is rewritten by position.
    #[test]
    fn a_pattern_clause_advertises_the_expression_this_service_publishes() {
        // (case, the model's pattern, the expression a clause must state)
        let rows: [(&str, &str, &str); 5] = [
            (
                "the name expression, which this service publishes unanchored at the start",
                r"^[A-Za-z0-9\-_ ]+$",
                r"[A-Za-z0-9\-_ ]+$",
            ),
            (
                "the resource expression, published as the model spells it",
                r"^arn:aws[^:\s]*:[^:\s]*:[^:\s]*:[0-9]{12}:[^\s]+$",
                r"^arn:aws[^:\s]*:[^:\s]*:[^:\s]*:[0-9]{12}:[^\s]+$",
            ),
            (
                "the client-token expression",
                r"^[\w:/-]+$",
                r"^[\w:/-]+$",
            ),
            (
                "the rule-ARN expression",
                r"^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$",
                r"^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$",
            ),
            (
                "an expression no member of this model carries",
                r"^[a-z]{3}$",
                r"^[a-z]{3}$",
            ),
        ];
        for (case, modelled, expected) in rows {
            let stated = advertised_pattern(modelled);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: only a constraint refusal is reworded, and a refusal with nothing to state
    /// keeps the framework's wording.
    #[test]
    fn nothing_but_a_constraint_refusal_is_reworded() {
        assert!(
            refused(&Refusal::Unauthenticated).is_none(),
            "expected an unauthenticated caller to keep the framework's wording"
        );
        assert!(
            refused(&Refusal::Invalid(Vec::new())).is_none(),
            "expected no violations to state nothing"
        );
    }
}
