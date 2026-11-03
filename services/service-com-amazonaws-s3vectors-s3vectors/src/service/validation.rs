// SPDX-License-Identifier: CC0-1.0

//! How this service states a request it refused for a MODELLED input constraint,
//! before any handler ran: a value outside a declared `@range` or `@length`, outside a
//! `@pattern` or an enum's value set, or a `@required` member the request left out.
//!
//! # The rule
//!
//! Every such refusal is HTTP 400 with this service's OWN modelled
//! `ValidationException` — the error the model binds to every operation
//! (`smithy_query(query="digest")` lists it under `common_errors`) — so the body carries
//! that shape's members: the `@required` `message`, and `fieldList` naming each input
//! that failed with the sentence for it
//! (`smithy_query(query="shape", shape="ValidationException,ValidationExceptionField")`).
//!
//! The parts of that body:
//!
//! * one CLAUSE per violated member, `Value at '<json pointer>' failed to satisfy
//!   constraint: <requirement>` — the member named by a JSON POINTER into the request
//!   (`/dimension`, leading slash) and the requirement naming the modelled constraint,
//!   with BOTH declared bounds where the member declares two. The rejected value is not
//!   quoted, so no caller input is echoed back;
//! * `message`, the counted envelope `<n> validation error(s) detected. ` over the
//!   clauses;
//! * `fieldList`, one entry per violated member: the same pointer as `path` and that
//!   member's own clause as `message`, so the two halves of the body cannot word one
//!   violation two ways.
//!
//! # Why this cannot live in `Service::override_refusal`
//!
//! The framework's own pre-dispatch renderer gets the status (400) and the wire code
//! (`ValidationException`) right and the BODY wrong: its only seam,
//! `Service::override_refusal`, chooses a code and a sentence and nothing else, so the
//! body it writes is `message` alone and `fieldList` — the machine-readable half a
//! client reads the failing member from — is absent. [`modelled_refusal`] therefore
//! states the refusal itself, from the framework's own deserializer and constraint
//! walk, and serializes it through the model's `ValidationException` schema. `super`'s
//! [`ModelledValidation`](super::ModelledValidation) is what puts it in front of every
//! operation, and `super`'s `override_refusal` words anything the framework still
//! renders from these same helpers, so the two paths cannot disagree.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::aws_smithy_schema::{Schema, TraitMap};
use ::aws_smithy_schema_server::constraints::{path, LengthTrait, RangeTrait};
use smithy_server_runtime::{Operation, Refused, ServerHttpProtocol, Violation, ViolationKind};

use crate::ops::types::{
    ValidationException, ValidationExceptionField, SCHEMA_VALIDATION_EXCEPTION,
};

/// The wire error code of `com.amazonaws.s3vectors#ValidationException` — the shape
/// name, which is what an SDK matches a restJson1 error on.
pub(super) const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares
/// (`smithy_query(query="shape", shape="ValidationException")`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The greatest number of violations one refusal states. The framework's constraint
/// walk stops collecting at this many, so this is the same number it used rather than a
/// literal of this crate's own.
const MAX_VIOLATIONS: usize = service_common::validation::MAX_VIOLATIONS;

/// The response this service answers a request it refuses before `O`'s handler runs, or
/// `None` when the request reaches that handler.
///
/// `None` also when the modelled error cannot be serialized: that leaves the
/// framework's own refusal path to answer, worded by `super`'s `override_refusal` from
/// these same helpers, rather than answering a 400 with nothing in it.
pub(super) fn modelled_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<RtResponse> {
    // The framework's OWN machinery — the protocol's deserializer and
    // `constraints::validate` over the built input — so this cannot find a different
    // violation set from the walk that runs behind it.
    let violations = service_common::validation::refusing_violations::<O>(protocol, req);
    let refused = refusal_for(&violations, Some(O::INPUT_SCHEMA))?;
    protocol
        .serialize_error(
            &refused,
            &SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_CODE,
            VALIDATION_EXCEPTION_STATUS,
        )
        .ok()
}

/// `violations` as the modelled `ValidationException`, or `None` when there is nothing
/// to state (an empty set, or a constraint class this service has no wording for).
fn refusal_for(
    violations: &[Violation],
    input_schema: Option<&Schema>,
) -> Option<ValidationException> {
    let violations = &violations[..violations.len().min(MAX_VIOLATIONS)];
    let clauses = clauses_for(violations, input_schema)?;
    Some(ValidationException {
        message: envelope(&clauses),
        field_list: Some(
            violations
                .iter()
                .zip(&clauses)
                .map(|(violation, clause)| ValidationExceptionField {
                    path: pointer(&violation.path),
                    message: clause.clone(),
                })
                .collect(),
        ),
    })
}

/// How this service states a refusal the framework renders itself: the same code and the
/// same envelope, which is all [`Refused`] can carry.
///
/// `None` declines the refusal, keeping the framework's own wording — see
/// [`requirement_for`]. The input schema is not at hand here (the seam is given the
/// violations alone), so a clause states the bound the violation carries.
pub(super) fn refused_for(violations: &[Violation]) -> Option<Refused> {
    let violations = &violations[..violations.len().min(MAX_VIOLATIONS)];
    let clauses = clauses_for(violations, None)?;
    Some(Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message: envelope(&clauses),
    })
}

/// One clause per violation, or `None` when any of them is a class this service has no
/// wording for.
fn clauses_for(violations: &[Violation], input_schema: Option<&Schema>) -> Option<Vec<String>> {
    if violations.is_empty() {
        return None;
    }
    violations
        .iter()
        .map(|violation| clause_for(violation, input_schema))
        .collect()
}

/// The counted envelope over the clauses of one refusal: `<n> validation error(s)
/// detected. ` and then the clauses, joined in the order the walk found them.
fn envelope(clauses: &[String]) -> String {
    match clauses {
        [clause] => format!("1 validation error detected. {clause}"),
        many => format!(
            "{} validation errors detected. {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// One violation as this service states it: the member named by its JSON pointer and
/// the requirement it failed, with no echo of the value the caller sent.
///
/// `None` for a constraint class this service has no wording for — [`ViolationKind`] is
/// `#[non_exhaustive]`, so a class added upstream keeps the framework's own wording
/// rather than being dropped from the body.
fn clause_for(violation: &Violation, input_schema: Option<&Schema>) -> Option<String> {
    let declared = input_schema.and_then(|schema| member_traits(schema, &violation.path));
    let requirement = requirement_for(violation, declared)?;
    Some(format!(
        "Value at '{}' failed to satisfy constraint: {requirement}",
        pointer(&violation.path)
    ))
}

/// The requirement `violation` failed, as this service words it — the half of a clause
/// that says what the member had to satisfy, and the sentence its `fieldList` entry
/// carries.
///
/// A bounded member states BOTH its declared bounds (`Member must be between 1 and
/// 4096, inclusive`), which is what the recorded refusal of a value above a `@range`
/// maximum carries; a violation alone carries only the bound it broke, so the pair is
/// read off `declared`, the violated member's own schema traits. A member that declares
/// one bound states that bound alone, there being no pair to state.
///
/// `None` for a class this service has no wording for.
fn requirement_for(violation: &Violation, declared: Option<&TraitMap>) -> Option<String> {
    Some(match &violation.kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinValue(min) => match declared.and_then(range_bounds) {
            Some((low, high)) => format!("Member must be between {low} and {high}, inclusive"),
            None => format!("Member must be greater than or equal to {min}"),
        },
        ViolationKind::MaxValue(max) => match declared.and_then(range_bounds) {
            Some((low, high)) => format!("Member must be between {low} and {high}, inclusive"),
            None => format!("Member must be less than or equal to {max}"),
        },
        ViolationKind::MinLength(min) => match declared.and_then(length_bounds) {
            Some((low, high)) => {
                format!("Member must have length between {low} and {high}, inclusive")
            }
            None => format!("Member must have length greater than or equal to {min}"),
        },
        ViolationKind::MaxLength(max) => match declared.and_then(length_bounds) {
            Some((low, high)) => {
                format!("Member must have length between {low} and {high}, inclusive")
            }
            None => format!("Member must have length less than or equal to {max}"),
        },
        // The expression as the model spells it: a client that recompiles it gets the
        // regex the member is held to, anchors and all.
        ViolationKind::Pattern(pattern) => {
            format!("Member must satisfy regular expression pattern: {pattern}")
        }
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// The member path spelled as the JSON POINTER into the request this service names a
/// failing input by (`/dimension`, `/vectors/0/data`), which is
/// [`service_common::json_pointer`]'s one translation of the dotted path the framework's
/// constraint walk reports.
fn pointer(path: &str) -> String {
    service_common::json_pointer::json_pointer(path)
}

/// The traits of the top-level member `path` names, or `None` when `path` names none of
/// `input_schema`'s members or reaches INSIDE one.
///
/// Only a top-level member's schema is reachable: a member schema carries its own name
/// and its own constraints, and nothing of the shape it targets, so there is no nested
/// member to look up. A nested violation therefore states the bound it carries.
fn member_traits<'schema>(input_schema: &'schema Schema, path: &str) -> Option<&'schema TraitMap> {
    input_schema
        .members()
        .iter()
        .copied()
        .find(|member| {
            member
                .member_name()
                .is_some_and(|name| path::member(name) == path)
        })
        .and_then(Schema::traits)
}

/// Both bounds the member's `@range` declares, or `None` unless it declares two.
fn range_bounds(traits: &TraitMap) -> Option<(String, String)> {
    let range = traits
        .get(&RangeTrait::TRAIT_ID)?
        .as_any()
        .downcast_ref::<RangeTrait>()?;
    Some((bound(range.min?), bound(range.max?)))
}

/// Both bounds the member's `@length` declares, or `None` unless it declares two.
fn length_bounds(traits: &TraitMap) -> Option<(String, String)> {
    let length = traits
        .get(&LengthTrait::TRAIT_ID)?
        .as_any()
        .downcast_ref::<LengthTrait>()?;
    Some((length.min?.to_string(), length.max?.to_string()))
}

/// A `@range` bound as a requirement prints it: an integral bound without a trailing
/// `.0`, a fractional one with its decimals — the rendering the framework's own clause
/// uses for the same bound, so the two cannot print one model constant two ways.
fn bound(value: f64) -> String {
    if value.fract() == 0.0 && value.is_finite() {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    //! The wording of a refusal and the members it fills. What reaches the wire, for
    //! which request, is pinned over the router in
    //! `tests/all/modelled_input_validation.rs`.

    use super::*;

    /// A violation of `kind` on `path`, with no value echoed.
    fn violation(path: &str, kind: ViolationKind) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: None,
            sensitive: false,
        }
    }

    /// Every constraint class states a clause and names its member in `fieldList`, and
    /// the two halves of the body word the violation the same way.
    #[test]
    fn every_constraint_class_names_its_member_and_its_requirement() {
        let classes = [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(3),
            ViolationKind::MaxLength(63),
            ViolationKind::MinValue("1".to_string()),
            ViolationKind::MaxValue("4096".to_string()),
            ViolationKind::Pattern("^[a-z0-9]+$".to_string()),
            ViolationKind::EnumValue(vec!["float32".to_string()]),
            ViolationKind::UniqueItems,
        ];
        for kind in classes {
            let refused = refusal_for(&[violation("dimension", kind.clone())], None)
                .unwrap_or_else(|| panic!("case {kind:?}: must be refused"));
            let fields = refused.field_list.clone().unwrap_or_default();
            assert_eq!(
                fields.len(),
                1,
                "case {kind:?}: expected one field, got {fields:?}"
            );
            assert_eq!(
                fields[0].path, "/dimension",
                "case {kind:?}: expected the member's JSON pointer, got {:?}",
                fields[0].path
            );
            assert_eq!(
                refused.message,
                format!("1 validation error detected. {}", fields[0].message),
                "case {kind:?}: the envelope must state the field's own clause, got {:?}",
                refused.message
            );
        }
    }

    /// A member whose schema declares both bounds states both of them, inclusive;
    /// one that declares a single bound states that bound alone.
    #[test]
    fn a_requirement_states_both_declared_bounds() {
        // (case, kind, the member's declared traits, the requirement)
        let both_range: TraitMap = {
            let mut map = TraitMap::new();
            map.insert(Box::new(RangeTrait::new(Some(1.0), Some(4096.0))));
            map
        };
        let min_only: TraitMap = {
            let mut map = TraitMap::new();
            map.insert(Box::new(RangeTrait::new(Some(1.0), None)));
            map
        };
        let both_length: TraitMap = {
            let mut map = TraitMap::new();
            map.insert(Box::new(LengthTrait::new(Some(3), Some(63))));
            map
        };
        let rows: [(&str, ViolationKind, Option<&TraitMap>, &str); 5] = [
            (
                "a value above a two-sided range",
                ViolationKind::MaxValue("4096".to_string()),
                Some(&both_range),
                "Member must be between 1 and 4096, inclusive",
            ),
            (
                "a value below a two-sided range",
                ViolationKind::MinValue("1".to_string()),
                Some(&both_range),
                "Member must be between 1 and 4096, inclusive",
            ),
            (
                "a value below a one-sided range",
                ViolationKind::MinValue("1".to_string()),
                Some(&min_only),
                "Member must be greater than or equal to 1",
            ),
            (
                "a value whose member the schema does not carry",
                ViolationKind::MaxValue("500".to_string()),
                None,
                "Member must be less than or equal to 500",
            ),
            (
                "a string outside a two-sided length",
                ViolationKind::MaxLength(63),
                Some(&both_length),
                "Member must have length between 3 and 63, inclusive",
            ),
        ];

        for (case, kind, declared, expected) in rows {
            let actual = requirement_for(&violation("member", kind), declared);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Several violations are counted in one envelope and each names its own member.
    #[test]
    fn several_violations_are_counted_and_stated_together() {
        let refused = refusal_for(
            &[
                violation("dimension", ViolationKind::MaxValue("4096".to_string())),
                violation("indexName", ViolationKind::MissingRequiredMember),
            ],
            None,
        )
        .expect("two violations are a refusal");
        assert!(
            refused.message.starts_with("2 validation errors detected. "),
            "expected the counted envelope, got {:?}",
            refused.message
        );
        let paths: Vec<String> = refused
            .field_list
            .unwrap_or_default()
            .into_iter()
            .map(|field| field.path)
            .collect();
        assert_eq!(
            paths,
            vec!["/dimension".to_string(), "/indexName".to_string()],
            "every violated member must be named, got {paths:?}"
        );
    }

    /// An empty violation set is not a refusal: a request the walk found nothing wrong
    /// with is served.
    #[test]
    fn no_violation_is_no_refusal() {
        assert!(
            refusal_for(&[], None).is_none(),
            "an empty violation set must not be a refusal"
        );
        assert!(
            refused_for(&[]).is_none(),
            "an empty violation set must not be worded as a refusal"
        );
    }
}
