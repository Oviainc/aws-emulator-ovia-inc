// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The framework walks the modelled input constraints and hands [`Refusal`] to
//! `service.rs` STRUCTURALLY — one [`Violation`] per offending value, carrying the path
//! it sits at, what it failed and what it held — and leaves the words to the service,
//! because no model records them. This module is that wording for every constrained
//! member of every operation of this service: ONE formatter, so no operation can state a
//! refusal its own way, and nothing here is keyed to an operation or to a submitted
//! value.
//!
//! The dialect it implements:
//!
//! * the envelope is AWS's — `N validation error(s) detected: ` over one clause per
//!   violated constraint — and a clause names the offending MEMBER PATH and the
//!   CONSTRAINT it broke (the pattern, the length bound, the numeric bound, the enum's
//!   value set), both taken from the framework's own structural description of the
//!   violation, so the message always reports WHICH constraint failed;
//! * the submitted value is NEVER echoed back: the clause reads
//!   `Value at '<path>' …`. Every input of this service may carry a patient's clinical
//!   text or an identifier derived from it, and the member plus the constraint are what
//!   a caller acts on. An ABSENT member is still reported as the `null` it was — a fact
//!   of the request's shape rather than a value the caller sent;
//! * a `@pattern` requirement quotes the model's expression verbatim, anchors included:
//!   a caller that lifts the expression out of the message gets the one the model
//!   declares;
//! * the error CODE is chosen per violation class, not per operation: text past its
//!   modelled length maximum is this service's own `TextSizeLimitExceededException`,
//!   which every text-inference operation declares for exactly that, and everything else
//!   is the `InvalidRequestException` every operation declares for a bad value;
//! * one member this service validates ITSELF rather than at the request boundary: a
//!   data-access role ARN that was provided and does not fit the modelled IAM role ARN
//!   shape is refused with the service's own reason tag
//!   ([`DATA_ACCESS_ROLE_ARN_INVALID`]). The same code covers a well-formed ARN naming a
//!   role the service cannot assume, so the tag — not the code — is what tells a caller
//!   to correct the string rather than the role.

use smithy_server_runtime::{Refusal, Refused, Violation, ViolationKind};

/// The error code this service answers a bad input value with: the modelled
/// `InvalidRequestException` (`@httpError 400`), which every operation of this model
/// declares. Restated here because `override_refusal` replaces the code and the message
/// TOGETHER — a client branches on the code.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The error code this service answers text past its modelled length maximum with: the
/// modelled `TextSizeLimitExceededException` (`@httpError 400`), declared by the
/// operations that take clinical text.
const TEXT_SIZE_LIMIT_EXCEEDED_EXCEPTION: &str = "TextSizeLimitExceededException";

/// The member carrying the clinical text of a synchronous inference call, as a validation
/// path spells it: `smithy_query(query="operation",operation="DetectEntitiesV2,InferRxNorm")`
/// shows `Text` targeting `BoundedLengthString` (`@length 1..20000`) and
/// `OntologyLinkingBoundedLengthString` (`@length 1..10000`). A path segment is
/// lowerCamel, so a violation on one of them is reported at `text`.
const TEXT_MEMBER: &str = "text";

/// The member naming the IAM role an asynchronous job reads and writes Amazon S3 with, as
/// a validation path spells it: every `Start*Job` operation of the model declares
/// `DataAccessRoleArn` targeting `IamRoleArn` (`@length 20..2048`,
/// `@pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN_MEMBER: &str = "dataAccessRoleArn";

/// What this service reports for a data-access role ARN that does not fit the modelled
/// shape: its own reason tag, then the sentence. The tag is the discriminator between the
/// two failures that share `InvalidRequestException` — a syntactically wrong ARN, fixed by
/// correcting the string, and a well-formed one naming a role the service cannot assume,
/// fixed by creating or re-trusting the role.
const DATA_ACCESS_ROLE_ARN_INVALID: &str =
    "DATA_ACCESS_ROLE_ARN_INVALID: The provided dataAccessRoleArn is invalid.";

/// The subject the framework renders for a violation carrying no value, and the subject a
/// clause whose value is WITHHELD carries instead: a value this service declines to echo
/// is not an absent one, so it is not reported as `null`.
const NULL_SUBJECT: &str = "Value null at '";
const WITHHELD_SUBJECT: &str = "Value at '";

/// The member a violation is reported on: the LAST segment of its path, which is the one
/// whose value broke the constraint whatever structure or collection it sits in.
fn member_of(path: &str) -> &str {
    path.split('.').next_back().unwrap_or(path)
}

/// Whether `violation` is a value the caller SENT (rather than a member it left out)
/// outside `member`'s constraints.
fn submitted_value_of(violation: &Violation, member: &str) -> bool {
    // A value the walk withheld (a `@sensitive` member's) was still SENT: the violation
    // carries no copy of it, which is what `sensitive` says, not that the member was
    // absent.
    (violation.value.is_some() || violation.sensitive)
        && violation.kind != ViolationKind::MissingRequiredMember
        && member_of(&violation.path).eq_ignore_ascii_case(member)
}

/// The code the whole refusal travels with: text past its modelled length maximum is the
/// text-size error, every other bad value the generic client error.
fn code_for(violations: &[Violation]) -> &'static str {
    let text_too_long = violations.iter().any(|violation| {
        member_of(&violation.path).eq_ignore_ascii_case(TEXT_MEMBER)
            && matches!(violation.kind, ViolationKind::MaxLength(_))
    });
    if text_too_long {
        TEXT_SIZE_LIMIT_EXCEEDED_EXCEPTION
    } else {
        INVALID_REQUEST_EXCEPTION
    }
}

/// One violation as a clause of the envelope.
///
/// The requirement sentence and the member path are the framework's own
/// ([`Violation::message`]), so this service cannot drift from them; what this chooses is
/// that the submitted value is withheld.
fn clause(violation: &Violation) -> String {
    let withhold = violation.value.is_some() || violation.sensitive;
    let restated = Violation {
        path: violation.path.clone(),
        kind: violation.kind.clone(),
        value: None,
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

/// AWS's envelope over one clause per violation, in the order the walk found them.
///
/// Restated here rather than taken from [`aws_smithy_schema_server::envelope`], which
/// renders it from the violations themselves: every clause of this dialect withholds a
/// value the framework always renders, so the clauses are built first and joined after.
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
/// The data-access role ARN is asked FIRST: that member is checked by the service itself,
/// which answers its own tagged sentence rather than a boundary clause, so a request
/// carrying a malformed one is stated as that failure. Only an input-constraint refusal is
/// reworded at all — an authentication refusal is a different failure with a different
/// code and status, and nothing about it is this service's to word.
pub(crate) fn refused(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    if violations
        .iter()
        .any(|violation| submitted_value_of(violation, DATA_ACCESS_ROLE_ARN_MEMBER))
    {
        return Some(Refused {
            code: INVALID_REQUEST_EXCEPTION.to_string(),
            message: DATA_ACCESS_ROLE_ARN_INVALID.to_string(),
        });
    }
    Some(Refused {
        code: code_for(violations).to_string(),
        message: envelope(violations)?,
    })
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

    /// The member constants are the framework's own spelling of the model's member names,
    /// not literals that can drift from them.
    #[test]
    fn the_named_members_are_spelled_as_the_framework_spells_them() {
        for (member, named) in [("Text", TEXT_MEMBER), ("DataAccessRoleArn", DATA_ACCESS_ROLE_ARN_MEMBER)] {
            let spelled = ::aws_smithy_schema_server::constraints::path::member(member);
            assert_eq!(
                spelled, named,
                "case {member:?}: expected the framework's spelling {spelled:?}, got {named:?}"
            );
        }
    }

    /// RULE: a clause names the member path and the constraint that failed, and withholds
    /// the submitted value; an absent member is reported as the `null` it was.
    #[test]
    fn a_clause_states_the_constraint_and_withholds_the_value() {
        // (case, path, kind, submitted value, the clause expected)
        let rows: [(&str, &str, ViolationKind, Option<&str>, &str); 7] = [
            (
                "a value outside its pattern",
                "jobId",
                ViolationKind::Pattern(r"^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$".to_string()),
                Some("not#a#valid#job#id!"),
                r"Value at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$",
            ),
            (
                "a nested value outside its pattern",
                "filter.jobName",
                ViolationKind::Pattern(r"^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$".to_string()),
                Some("bad#name#!"),
                r"Value at 'filter.jobName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$",
            ),
            (
                "a number above its range maximum",
                "maxResults",
                ViolationKind::MaxValue("500".to_string()),
                Some("501"),
                "Value at 'maxResults' failed to satisfy constraint: Member must have value less \
                 than or equal to 500",
            ),
            (
                "a number below its range minimum",
                "maxResults",
                ViolationKind::MinValue("1".to_string()),
                Some("0"),
                "Value at 'maxResults' failed to satisfy constraint: Member must have value \
                 greater than or equal to 1",
            ),
            (
                "text above its length maximum",
                "text",
                ViolationKind::MaxLength(20_000),
                Some("aaaa"),
                "Value at 'text' failed to satisfy constraint: Member must have length less than \
                 or equal to 20000",
            ),
            (
                "a value below its length minimum",
                "text",
                ViolationKind::MinLength(1),
                Some(""),
                "Value at 'text' failed to satisfy constraint: Member must have length greater \
                 than or equal to 1",
            ),
            (
                "a member that was absent",
                "jobId",
                ViolationKind::MissingRequiredMember,
                None,
                "Value null at 'jobId' failed to satisfy constraint: Member must not be null",
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
    fn the_envelope_states_one_clause_per_violated_constraint() {
        let one = Refusal::Invalid(vec![violation(
            "jobId",
            ViolationKind::MaxLength(32),
            Some("over"),
        )]);
        let stated = refused(&one).expect("a constraint refusal is stated");
        assert_eq!(
            stated.code, INVALID_REQUEST_EXCEPTION,
            "expected the modelled client-error code, got {:?}",
            stated.code
        );
        assert_eq!(
            stated.message,
            "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member \
             must have length less than or equal to 32",
            "expected one clause, got {:?}",
            stated.message
        );

        let two = Refusal::Invalid(vec![
            violation("filter.jobName", ViolationKind::MaxLength(256), Some("over")),
            violation("maxResults", ViolationKind::MaxValue("500".to_string()), Some("501")),
        ]);
        let stated = refused(&two).expect("a constraint refusal is stated");
        assert_eq!(
            stated.message,
            "2 validation errors detected: Value at 'filter.jobName' failed to satisfy \
             constraint: Member must have length less than or equal to 256; Value at \
             'maxResults' failed to satisfy constraint: Member must have value less than or \
             equal to 500",
            "expected two clauses in the order the walk found them, got {:?}",
            stated.message
        );
    }

    /// RULE: text past its length MAXIMUM is the text-size error; every other bad value,
    /// including text under its minimum, is the generic client error.
    #[test]
    fn only_text_past_its_length_maximum_is_the_text_size_error() {
        // (case, path, kind, the code expected)
        let rows: [(&str, &str, ViolationKind, &str); 4] = [
            (
                "text past the entity-detection limit",
                "text",
                ViolationKind::MaxLength(20_000),
                TEXT_SIZE_LIMIT_EXCEEDED_EXCEPTION,
            ),
            (
                "text past the ontology-linking limit",
                "text",
                ViolationKind::MaxLength(10_000),
                TEXT_SIZE_LIMIT_EXCEEDED_EXCEPTION,
            ),
            (
                "text under its minimum",
                "text",
                ViolationKind::MinLength(1),
                INVALID_REQUEST_EXCEPTION,
            ),
            (
                "another member past ITS length maximum",
                "jobId",
                ViolationKind::MaxLength(32),
                INVALID_REQUEST_EXCEPTION,
            ),
        ];
        for (case, path, kind, expected) in rows {
            let refusal = Refusal::Invalid(vec![violation(path, kind, Some("x"))]);
            let stated = refused(&refusal).expect("a constraint refusal is stated");
            assert_eq!(
                stated.code, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.code
            );
        }
    }

    /// RULE: a data-access role ARN the caller PROVIDED outside its modelled shape is the
    /// tagged sentence, whichever of the member's constraints it broke and wherever the
    /// member sits; an absent one is the boundary clause for an absent member.
    #[test]
    fn a_provided_data_access_role_arn_outside_its_shape_is_tagged() {
        // (case, path, kind, submitted value, the message expected)
        let rows: [(&str, &str, ViolationKind, Option<&str>, &str); 4] = [
            (
                "an ARN outside the pattern",
                "dataAccessRoleArn",
                ViolationKind::Pattern(r"^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$".to_string()),
                Some("not-a-valid-arn"),
                DATA_ACCESS_ROLE_ARN_INVALID,
            ),
            (
                "an ARN under the length minimum",
                "dataAccessRoleArn",
                ViolationKind::MinLength(20),
                Some("short"),
                DATA_ACCESS_ROLE_ARN_INVALID,
            ),
            (
                "an ARN past the length maximum",
                "dataAccessRoleArn",
                ViolationKind::MaxLength(2048),
                Some("long"),
                DATA_ACCESS_ROLE_ARN_INVALID,
            ),
            (
                "an absent ARN",
                "dataAccessRoleArn",
                ViolationKind::MissingRequiredMember,
                None,
                "1 validation error detected: Value null at 'dataAccessRoleArn' failed to satisfy \
                 constraint: Member must not be null",
            ),
        ];
        for (case, path, kind, value, expected) in rows {
            let refusal = Refusal::Invalid(vec![violation(path, kind, value)]);
            let stated = refused(&refusal).expect("a constraint refusal is stated");
            assert_eq!(
                stated.code, INVALID_REQUEST_EXCEPTION,
                "case {case:?}: expected the modelled client-error code, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// RULE: the tagged sentence is the whole message — a malformed ARN reported once per
    /// constraint it broke still states one sentence, naming the member once.
    #[test]
    fn a_malformed_role_arn_states_one_sentence_however_many_constraints_it_broke() {
        let refusal = Refusal::Invalid(vec![
            violation("dataAccessRoleArn", ViolationKind::MinLength(20), Some("not-a-valid-arn")),
            violation(
                "dataAccessRoleArn",
                ViolationKind::Pattern(r"^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$".to_string()),
                Some("not-a-valid-arn"),
            ),
        ]);
        let stated = refused(&refusal).expect("a constraint refusal is stated");
        assert_eq!(
            stated.message, DATA_ACCESS_ROLE_ARN_INVALID,
            "expected the tagged sentence once, got {:?}",
            stated.message
        );
        assert_eq!(
            stated.message.matches(DATA_ACCESS_ROLE_ARN_MEMBER).count(),
            1,
            "expected the member named once, got {:?}",
            stated.message
        );
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
