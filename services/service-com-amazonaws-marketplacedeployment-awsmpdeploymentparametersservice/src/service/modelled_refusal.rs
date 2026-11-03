// SPDX-License-Identifier: CC0-1.0

//! The BODY this service answers a request that was refused before any handler ran.
//!
//! # The rule
//!
//! A failure answered as a shape the model declares carries every member that shape
//! marks `@required`, so a client deserializing the typed error finds each of them. The
//! client-input error of this service is `ValidationException`, and the model marks BOTH
//! its members required — `message` and `fieldName`, "the field name associated with the
//! error" (`smithy_query(query="shape",shape="ValidationException")`, `httpError 400`) —
//! so a refusal stated as that shape names the member it is about, whichever layer of the
//! service refused the request.
//!
//! The handler-side refusals already do ([`crate::state::unsupported_catalog`],
//! [`crate::state::reserved_tag_key`], [`crate::state::merge_tags`]). The framework's
//! pre-dispatch renderer cannot: its only seam, [`smithy_server_runtime::Service::override_refusal`],
//! carries a code and a sentence and nothing else ([`smithy_server_runtime::Refused`]), so
//! the body it writes is `message` alone — leaving one logical failure with two different
//! bodies depending on which layer answered.
//!
//! So that refusal is stated HERE instead, and serialized through the model's own
//! `ValidationException` schema. Nothing about it is re-implemented:
//!
//! * the violations come from the framework's own machinery — the protocol's deserializer
//!   and its constraint walk — through the collecting half every service that words its
//!   own pre-handler refusal shares
//!   ([`service_common::validation::refusing_violations`]), so this cannot find a
//!   different set from the walk that runs behind it;
//! * the sentence is [`super::constraint_envelope`]'s, the one this service already states
//!   through `override_refusal`, so the two paths cannot word one refusal differently;
//! * a request this declines is handed on to the operation the routing table names, which
//!   still owns deserialize -> validate -> handle -> serialize and every modelled error.
//!
//! The cost is that a request is read twice — once here and once in the operation's own
//! sequence — because the framework hands no already-built input to an operation. It is
//! the same cost [`super::EmulatedService::front_door_refuses`] already pays, bounded by
//! the body the request already carries.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use service_common::validation::refusing_violations;
use smithy_server_runtime::{Operation, ServerHttpProtocol, Violation};

use crate::ops::types::{ValidationException, SCHEMA_VALIDATION_EXCEPTION};

/// The HTTP status the modelled shape declares (`@httpError 400`) — the status the
/// framework's own refusal path answers a constraint violation with, restated here
/// because this module renders the response itself.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The response this service answers `req` with when the modelled walk refuses it before
/// a handler runs, or `None` when nothing is refused here.
///
/// `None` leaves the framework's own refusal path to answer — which it still words through
/// [`super::EmulatedService::override_refusal`] from the same envelope — rather than
/// answering a 400 with a member this service cannot fill: that happens for a violation
/// class this service has no wording for, for a violation naming no member, for a request
/// the protocol cannot bind at all, and when the modelled error cannot be serialized.
pub(super) fn refusal_response<Eng: Send + Sync + 'static>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<RtResponse> {
    let refused = refusal_for_request::<Eng>(protocol, req)?;
    protocol
        .serialize_error(
            &refused,
            &SCHEMA_VALIDATION_EXCEPTION,
            super::VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        .ok()
}

/// The modelled `ValidationException` that refuses `req` before the handler of the
/// operation its routing key names, or `None` when none does.
///
/// One arm per operation this model binds (`smithy_query(query="digest")`: four), because
/// `ValidationException` is a service-COMMON error — every operation declares it, so each
/// one owes the same body, and an operation left out would answer the framework's
/// `message`-only envelope instead. The input is read through the operation's OWN
/// model-derived input schema, so the value judged is the value dispatch would hand that
/// operation's handler.
fn refusal_for_request<Eng: Send + Sync + 'static>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<ValidationException> {
    use crate::ops::list_tags_for_resource::ListTagsForResource;
    use crate::ops::put_deployment_parameter::PutDeploymentParameter;
    use crate::ops::tag_resource::TagResource;
    use crate::ops::untag_resource::UntagResource;

    let key = protocol.routing_key(req).ok()?;
    if key == <PutDeploymentParameter<Eng> as Operation>::NAME {
        return refusal_for::<PutDeploymentParameter<Eng>>(protocol, req);
    }
    if key == <TagResource<Eng> as Operation>::NAME {
        return refusal_for::<TagResource<Eng>>(protocol, req);
    }
    if key == <UntagResource<Eng> as Operation>::NAME {
        return refusal_for::<UntagResource<Eng>>(protocol, req);
    }
    if key == <ListTagsForResource<Eng> as Operation>::NAME {
        return refusal_for::<ListTagsForResource<Eng>>(protocol, req);
    }
    // A key this model does not name is not refused here: the router's own
    // routing-error path answers it, exactly as it does without this wrapper.
    None
}

/// The modelled `ValidationException` that refuses `req` before `O`'s handler runs.
fn refusal_for<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<ValidationException> {
    modelled_refusal(&refusing_violations::<O>(protocol, req))
}

/// `violations` as the modelled `ValidationException`: the sentence this service states
/// for them, and the member it blames.
///
/// `None` when either required member cannot be filled — see [`refusal_response`].
pub(super) fn modelled_refusal(violations: &[Violation]) -> Option<ValidationException> {
    Some(ValidationException {
        message: super::constraint_envelope(violations)?,
        field_name: blamed_member(violations)?.to_string(),
    })
}

/// The member a refusal is blamed on: the one the FIRST clause of the message names.
///
/// `fieldName` is a single member path, so a request breaking several constraints is
/// blamed on the first violation the walk reports — which is the first clause of the
/// sentence, so the two halves of the body agree. The path is the one the validation
/// report spells (lowerCamel, dotted through a nested shape), which is how the modelled
/// member names a member and how the handler-side refusals of this service fill it.
///
/// `None` when no violation names a member at all, which `fieldName` has no value for.
fn blamed_member(violations: &[Violation]) -> Option<&str> {
    violations
        .iter()
        .map(|violation| violation.path.as_str())
        .find(|path| !path.is_empty())
}

#[cfg(test)]
mod tests {
    //! Which members the modelled body fills, and from which violation. What reaches
    //! the wire, for which request, is pinned over the router in
    //! `tests/all/boundary_validation_field_name.rs`.

    use super::*;
    use smithy_server_runtime::ViolationKind;

    /// A violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// Every constraint class this service words fills both required members, and the
    /// member named is the one the clause is about.
    #[test]
    fn every_worded_class_fills_both_required_members() {
        let classes = [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(32),
            ViolationKind::MaxLength(64),
            ViolationKind::MinValue("1".to_string()),
            ViolationKind::MaxValue("50".to_string()),
            ViolationKind::Pattern("^[a-zA-Z_-]+$".to_string()),
            ViolationKind::EnumValue(vec!["AWSMarketplace".to_string()]),
            ViolationKind::UniqueItems,
        ];
        for kind in classes {
            let refused =
                modelled_refusal(&[violation("deploymentParameter.name", kind.clone(), Some("x"))])
                    .unwrap_or_else(|| panic!("case {kind:?}: must be refused"));
            assert_eq!(
                refused.field_name, "deploymentParameter.name",
                "case {kind:?}: expected the member the clause is about, got {:?}",
                refused.field_name
            );
            assert!(
                refused.message.contains("deploymentParameter.name"),
                "case {kind:?}: expected the clause to name the same member, got {:?}",
                refused.message
            );
        }
    }

    /// Several violations are blamed on the member the first clause names.
    #[test]
    fn several_violations_are_blamed_on_the_first_clause() {
        let refused = modelled_refusal(&[
            violation(
                "catalog",
                ViolationKind::Pattern("^[a-zA-Z_-]+$".to_string()),
                Some("AWSMarketplace1"),
            ),
            violation("clientToken", ViolationKind::MinLength(32), Some("short")),
        ])
        .expect("two violations are a refusal");
        assert!(
            refused.message.starts_with("2 validation errors detected: "),
            "expected the counted envelope, got {:?}",
            refused.message
        );
        assert_eq!(
            refused.field_name, "catalog",
            "expected the member the first clause names, got {:?}",
            refused.field_name
        );
    }

    /// Nothing this service cannot state as both members is answered from here.
    #[test]
    fn a_refusal_this_body_cannot_state_is_declined() {
        assert!(
            modelled_refusal(&[]).is_none(),
            "expected no violations to state nothing"
        );
        assert!(
            modelled_refusal(&[violation("", ViolationKind::MinLength(1), Some("x"))]).is_none(),
            "expected a violation naming no member to be declined"
        );
    }
}
