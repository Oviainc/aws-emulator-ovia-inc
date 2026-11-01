// SPDX-License-Identifier: CC0-1.0

//! How the two add-on operations that take `resolveConflicts` refuse a value
//! outside the modelled enum, in this service's own wording.
//!
//! `CreateAddon$resolveConflicts` and `UpdateAddon$resolveConflicts` target the
//! enum `ResolveConflicts`
//! (`smithy_query(query="shape",shape="ResolveConflicts",
//! model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")`),
//! so the framework's validating walk refuses an off-enum value BEFORE any handler
//! runs and words it as one more constrained leaf — `Value '<value>' at
//! 'resolveConflicts' failed to satisfy constraint: …`. This service refuses that
//! value in ITS own sentence instead, which names the value and the operation it
//! was sent to, because the message is part of the contract a client matches on.
//!
//! The wording is per OPERATION, and `crate::service::EmulatedService::override_refusal`
//! is per SERVICE: it is reached with only the violations in hand and no operation
//! named (`vendor/smithy-server-runtime/src/pre_dispatch.rs`), so it cannot name the
//! operation the sentence has to carry. The one seam a service has BEFORE dispatch
//! (`smithy_server_runtime::Service::override_operation`) is therefore where this is
//! decided — the same place the tag contract beside this module is decided — and the
//! refusal is answered as the `InvalidParameterException` both operations declare,
//! carrying the cluster and the add-on the request named, exactly as the refusals the
//! handlers themselves word do.
//!
//! Only the enum membership of that one member is judged here. Every other rule of
//! those two requests is the handler's, and a request whose `resolveConflicts` is
//! absent or inside the enum is never claimed by this module at all.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::validate;
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, ErasedServiceDefinition, OperationContext,
    OperationError, Refused, ServerHttpProtocol, Service,
};

use crate::ops::create_addon::CreateAddonErrorModeled;
use crate::ops::types::{
    CreateAddonInput, InvalidParameterException, UpdateAddonInput, RESOLVE_CONFLICTS_VALUES,
    CREATE_ADDON_INPUT_SCHEMA, UPDATE_ADDON_INPUT_SCHEMA,
};
use crate::ops::update_addon::UpdateAddonErrorModeled;

/// The routing key of the operation that INSTALLS an add-on. On this service's
/// restJson1 protocol a routing key is the operation's own shape name
/// (`smithy_query(query="digest",…)` reports `restJson1`), so these two constants are
/// the only place this module names the operations it answers for.
const CREATE_ADDON: &str = "CreateAddon";
/// The routing key of the operation that UPDATES one; see [`CREATE_ADDON`].
const UPDATE_ADDON: &str = "UpdateAddon";

/// The member this contract judges, as a violation path spells it — which is how the
/// framework's own walk names it too.
const RESOLVE_CONFLICTS_MEMBER: &str = "resolveConflicts";

/// The 400 both operations declare for a member of the body that is wrong.
const INVALID_PARAMETER_EXCEPTION: &str = "InvalidParameterException";

/// The value as the sentence may echo it: elided at the cap every refusal this
/// service words shares ([`crate::state::elided_value`]), because the member targets
/// an enum and so carries no modelled `length` bound at all.
///
/// Rendering only — the enum membership below is decided on the ORIGINAL value, so
/// eliding can never turn a declared value into a refused one.
fn echoed(value: &str) -> String {
    crate::state::elided_value(value, crate::state::MAX_ECHOED_VALUE)
}

/// The sentence Amazon EKS refuses an off-enum `resolveConflicts` with.
///
/// Fixed text around the two variable parts: the value the request sent, and the
/// operation it sent it to — which is why this cannot be worded by a per-service
/// refusal seam.
fn refusal_message(value: &str, operation: &str) -> String {
    format!(
        "Resolve conflicts option {} is not supported for operation {operation}",
        echoed(value)
    )
}

/// One refused add-on request: the operation it was sent to, and the exception this
/// service answers it with.
///
/// The exception is built once, where the input is in hand, and carried to the
/// response: the operation decides which error channel it is serialized in, and both
/// channels declare this same `InvalidParameterException` (HTTP 400).
#[derive(Debug)]
pub struct ResolveConflictsRefusal {
    operation: &'static str,
    exception: InvalidParameterException,
}

impl ResolveConflictsRefusal {
    /// This refusal as a response in the service's own protocol envelope, serialized
    /// through the modelled error channel of the operation it belongs to — the same
    /// call the framework's own error tail makes, so a refusal worded here and one
    /// worded by the handler are indistinguishable on the wire.
    ///
    /// FAIL CLOSED: the handlers do not judge this member (an off-enum value never
    /// reaches them), so a request this contract refused must never fall through to
    /// one. Where the modelled envelope cannot be rendered at all, the same sentence
    /// is answered through the service's framework-refusal envelope instead, which is
    /// the same 400 and the same code.
    fn response(self, definition: &dyn ErasedServiceDefinition) -> RtResponse {
        let Self {
            operation,
            exception,
        } = self;
        let stated = exception.message.clone().unwrap_or_default();
        let rendered = if operation == UPDATE_ADDON {
            serialize(
                definition.protocol(),
                &UpdateAddonErrorModeled::InvalidParameterException(exception),
            )
        } else {
            serialize(
                definition.protocol(),
                &CreateAddonErrorModeled::InvalidParameterException(exception),
            )
        };
        rendered.unwrap_or_else(|| {
            definition.refusal_response(
                ProtocolError::Invalid(Vec::new()),
                Some(Refused {
                    code: INVALID_PARAMETER_EXCEPTION.to_string(),
                    message: stated,
                }),
            )
        })
    }
}

/// One modelled error as a response in this service's protocol envelope.
fn serialize(protocol: &dyn ServerHttpProtocol, error: &dyn OperationError) -> Option<RtResponse> {
    protocol
        .serialize_error(
            error.as_serializable(),
            error.variant_schema(),
            error.shape_name(),
            error.http_status(),
        )
        .ok()
}

/// Read the request into `T` through the protocol and schema the dispatch sequence
/// uses, so this layer reads exactly the value the handler would be given, or `None`
/// for a body it cannot read at all — which is the framework's refusal, not this
/// one's.
fn read<T: DeserializableStruct>(
    protocol: &dyn ServerHttpProtocol,
    schema: &'static ::aws_smithy_schema::Schema,
    req: &RtRequest,
) -> Option<T> {
    let mut deserializer = protocol.deserialize_input(req, schema).ok()?;
    T::deserialize(&mut *deserializer).ok()
}

/// The value of `resolveConflicts` when the request carries one the enum does not
/// declare, or `None` when it carries none or carries a declared one.
///
/// The declared set is the generated `RESOLVE_CONFLICTS_VALUES` and the membership
/// test is the service-wide one ([`super::off_enum_member_contract::off_enum`]), so
/// this layer, the layer that answers the other members judged this way, and the
/// framework's own walk all read the SAME enum the model declares.
fn off_enum(value: Option<&str>) -> Option<&str> {
    super::off_enum_member_contract::off_enum(RESOLVE_CONFLICTS_VALUES, value)
}

/// Whether the framework's own validating walk finds anything wrong with this input
/// BESIDES the member this contract words.
///
/// A body that also breaks a modelled constraint keeps the framework's whole
/// violation report, which one sentence about one member cannot carry: this contract
/// rewords a refusal, it does not outrank the walk. The walk is run through the same
/// `validate` and the same generated input schema the dispatch sequence runs, as the
/// tag contract beside this module does, so this layer cannot disagree with the walk
/// that would otherwise follow it.
fn breaks_another_rule(input: &dyn SerializableStruct, schema: &'static Schema) -> bool {
    validate(input, schema)
        .iter()
        .any(|violation| violation.path != RESOLVE_CONFLICTS_MEMBER)
}

/// How this service refuses the request `key` names, or `None` when this contract
/// has nothing to say about it.
pub fn refusal_for(
    key: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<ResolveConflictsRefusal> {
    match key {
        CREATE_ADDON => {
            let input: CreateAddonInput = read(protocol, CREATE_ADDON_INPUT_SCHEMA, req)?;
            let value = off_enum(input.resolve_conflicts.as_deref())?;
            if breaks_another_rule(&input, CREATE_ADDON_INPUT_SCHEMA) {
                return None;
            }
            Some(ResolveConflictsRefusal {
                operation: CREATE_ADDON,
                exception: InvalidParameterException {
                    message: Some(refusal_message(value, CREATE_ADDON)),
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    ..Default::default()
                },
            })
        }
        UPDATE_ADDON => {
            let input: UpdateAddonInput = read(protocol, UPDATE_ADDON_INPUT_SCHEMA, req)?;
            let value = off_enum(input.resolve_conflicts.as_deref())?;
            if breaks_another_rule(&input, UPDATE_ADDON_INPUT_SCHEMA) {
                return None;
            }
            Some(ResolveConflictsRefusal {
                operation: UPDATE_ADDON,
                exception: InvalidParameterException {
                    message: Some(refusal_message(value, UPDATE_ADDON)),
                    cluster_name: Some(input.cluster_name.clone()),
                    addon_name: Some(input.addon_name.clone()),
                    ..Default::default()
                },
            })
        }
        _ => None,
    }
}

/// The operation this service serves for an add-on request whose `resolveConflicts`
/// this contract refuses.
///
/// An [`ErasedOperation`] rather than a response built in
/// [`Service::override_operation`], because that seam hands the router a BORROWED
/// operation. It holds no state: the refusal is re-derived from the same request the
/// router passes back into [`ErasedOperation::run`], so one value serves every such
/// request.
#[derive(Debug, Default)]
pub struct RefusedAddonRequest;

impl RefusedAddonRequest {
    /// The one refusal value a service holds.
    pub const fn new() -> Self {
        Self
    }
}

impl ErasedOperation for RefusedAddonRequest {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The service's model-derived half is read inside this block and not held
            // past it: `&dyn ErasedServiceDefinition` is not `Sync`, and the future
            // returned here is `Send`. Only the modelled operation — which is
            // `Send + Sync` — crosses the await below.
            let answer = {
                let definition = svc.definition();
                let protocol = definition.protocol();
                let key = protocol.routing_key(req).ok();
                // The same decision `override_operation` made, re-derived from the same
                // request: this operation is reached only when it answered `Some`, so
                // the refusal is found again, and answering it is FAIL CLOSED (see
                // [`ResolveConflictsRefusal::response`]). If it somehow is not found,
                // the modelled operation serves the request as it would have anyway.
                match key.and_then(|key| refusal_for(key, protocol, req)) {
                    Some(refusal) => Ok(refusal.response(definition)),
                    None => match key.and_then(|key| definition.routing_table().get(key)) {
                        Some(operation) => Err(operation),
                        None => Ok(definition.routing_error_response(req)),
                    },
                }
            };
            match answer {
                Ok(response) => response,
                Err(operation) => operation.run(svc, req, ctx).await,
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", so this
        // refusal cannot weaken the schemes the model declares for it.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The enum's own value set decides what this contract refuses, and the sentence
    /// it refuses with names the value and the operation.
    ///
    /// Source for the accepted rows: the `ResolveConflicts` enum values the model
    /// declares, read through the generated `RESOLVE_CONFLICTS_VALUES`.
    #[test]
    fn the_modelled_enum_decides_which_values_this_contract_refuses() {
        for value in RESOLVE_CONFLICTS_VALUES {
            assert!(
                off_enum(Some(value)).is_none(),
                "case {value:?}: a declared enum value is not this contract's refusal"
            );
        }
        assert!(
            off_enum(None).is_none(),
            "case absent: a request that names no value is not refused"
        );
        // Rows differ along HOW the value misses the enum: unknown, the right word in
        // the wrong case, a declared value with padding, and the empty string.
        for value in ["BANANA", "overwrite", " NONE", ""] {
            assert_eq!(
                off_enum(Some(value)),
                Some(value),
                "case {value:?}: a value outside the enum must be refused"
            );
        }
    }

    #[test]
    fn the_refusal_sentence_names_the_value_and_the_operation() {
        for (operation, value, expected) in [
            (
                CREATE_ADDON,
                "BANANA",
                "Resolve conflicts option BANANA is not supported for operation CreateAddon",
            ),
            (
                UPDATE_ADDON,
                "overwrite",
                "Resolve conflicts option overwrite is not supported for operation UpdateAddon",
            ),
        ] {
            let message = refusal_message(value, operation);
            assert_eq!(
                message, expected,
                "case {operation}/{value:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// A value the caller chose the length of is echoed only up to the cap, whatever
    /// its bytes are: the rows differ along the length and along whether the cut
    /// would land inside a multi-byte character.
    #[test]
    fn a_caller_sized_value_is_echoed_only_up_to_the_cap() {
        let cap = crate::state::MAX_ECHOED_VALUE;
        for (case, value) in [
            ("at the cap", "x".repeat(cap)),
            ("one character past the cap", "x".repeat(cap + 1)),
            ("far past the cap", "x".repeat(cap * 40)),
            (
                "multi-byte characters across the cap",
                "\u{00e9}".repeat(cap * 2),
            ),
        ] {
            // The ORIGINAL value decides the refusal, however long it is.
            assert_eq!(
                off_enum(Some(&value)),
                Some(value.as_str()),
                "case {case}: the value itself is what the enum is checked against"
            );
            let message = refusal_message(&value, CREATE_ADDON);
            assert!(
                message.chars().count() < cap * 2,
                "case {case}: the sentence must not grow with the value, got {} characters",
                message.chars().count()
            );
        }
    }
}
