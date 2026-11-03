// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-managedblockchainquery-tietonchainqueryservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. THIS SERVICE USES IT: every modelled operation is served behind
//!   [`ModelledValidation`], which answers a request refused BEFORE the handler
//!   runs — an absent `@required` member — in the service's own modelled
//!   `ValidationException` instead of the framework's two-field refusal body, and
//!   hands every other request to the generated operation unchanged.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. THIS SERVICE REWORDS IT: a constraint violation is
//!   stated in AWS's validation-framework form, with a `@pattern` quoted
//!   unanchored. See the inline note on that method.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

/// The error code this service answers a refused request with: the modelled
/// `ValidationException` (`@httpError 400`), which every operation of this service
/// declares. Restated here because `override_refusal` replaces the code and the message
/// together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `@pattern` expression without the anchors the model spells it with.
///
/// The requirement a message states is the expression a value must satisfy, not the
/// anchoring the model uses to make the match total, so a client that re-validates
/// locally with the pattern it read out of the message uses the one AWS reports. A `$`
/// escaped by a backslash is a literal dollar sign inside the expression, not the end
/// anchor, so it stays.
///
/// NOTE, stated as `services/common/src/validation.rs` asks: this rule has LANDED in that
/// shared module as `service_common::validation::unanchored`, which is the staged home for
/// it; ~26 service crates — this one included — still carry the inlined copy below, and
/// "nothing new should inline the rule again". This copy PRE-DATES this change and is not a
/// new one: deleting it, together with the other crates', in favour of the shared function
/// is that migration's own change, and the shared rendering is the stricter of the two (it
/// drops a `^`/`$` pair only when the pair wraps the WHOLE expression), so swapping it in
/// is a behavioural change to re-pin against this service's recordings rather than a
/// no-op. The two rules this file also inlines — the requirement half in [`requirement`]
/// and the subject half in [`constraint_clause`] — are the parts that module lists as
/// still to come, so they stay here until it owns them. What is NOT service-independent,
/// and so stays in this file either way, is the envelope dialect, which values an enum's
/// value set lists and in which order, and this service's choice to quote a submitted
/// scalar value — each pinned by this service's recordings and tests.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The constraint half of a clause: what the value failed, as AWS's validation framework
/// words it.
///
/// `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's wording
/// rather than invent a sentence for it ([`ViolationKind`] is `#[non_exhaustive]`, so a
/// class can be added upstream).
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    })
}

/// Whether this violation's value is the framework's stand-in for a COLLECTION's size
/// rather than something the caller sent.
///
/// A `@length` failure on a list or a map carries `"<n> items"` / `"<n> entries"` as its
/// value, because the collection itself is not rendered. That is not a value any caller
/// submitted, so a clause states the member without it rather than quoting a phrase the
/// request never contained. Only a `@length` failure can carry it, so no other
/// violation's value is second-guessed.
fn is_collection_size(violation: &Violation) -> bool {
    if !matches!(
        violation.kind,
        ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
    ) {
        return false;
    }
    let Some(value) = violation.value.as_deref() else {
        return false;
    };
    let Some((count, unit)) = value.split_once(' ') else {
        return false;
    };
    matches!(unit, "items" | "entries")
        && !count.is_empty()
        && count.bytes().all(|byte| byte.is_ascii_digit())
}

/// Render one constraint violation the way AWS's validation framework states it: the
/// submitted value, the member's dotted path, and the constraint it failed.
///
/// `pub(crate)` because the two refusal paths share it: `crate::state::validation` words
/// BOTH of them with it — the modelled constraints the handlers check themselves, and the
/// one refusal no handler can make (an absent `@required` member, which
/// [`ModelledValidation`] below collects from the framework's own deserializer) — so the
/// answer always carries the modelled `reason` and `fieldList`. This file's
/// [`Service::override_refusal`] uses it for whatever is left: a refusal the framework
/// still renders itself, which that hook can reword but not reshape.
pub(crate) fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = requirement(&violation.kind)?;
    let path = &violation.path;
    if is_collection_size(violation) {
        return Some(format!(
            "Value at '{path}' failed to satisfy constraint: {requirement}"
        ));
    }
    Some(match &violation.value {
        // An absent member has no submitted value at all, so it is reported as the null
        // it was.
        None => format!("Value null at '{path}' failed to satisfy constraint: {requirement}"),
        Some(value) => {
            format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
        }
    })
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation.
///
/// `None` declines the whole refusal — see [`constraint_clause`], which also says why
/// this is `pub(crate)`.
pub(crate) fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-managedblockchainquery-tietonchainqueryservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One [`ModelledValidation`] per modelled operation, keyed by the routing key the
    /// model names that operation by ([`Operation::NAME`], the same key the generated
    /// routing table uses). Built once here, never per request; each entry is zero-sized,
    /// so this is a table of NAMES rather than a second set of operations — the operation
    /// that serves a request is still the one the generated table built from the shared
    /// engine.
    refusals: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            refusals: modelled_validation_table::<Eng>(),
        }
    }
}

/// The modelled-refusal wrapper for EVERY operation this service serves.
///
/// `ValidationException` is a common error of this service — the model declares it on all
/// nine operations (`smithy_query(query="digest")`: `common_errors`) — so every operation
/// states a request refused before its handler ran the same way, and there is no operation
/// whose boundary refusal may answer in the framework's envelope instead. An operation
/// added to the model later is wrapped by adding one line here.
fn modelled_validation_table<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut refusals: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    register::<crate::ops::batch_get_token_balance::BatchGetTokenBalance<Eng>>(&mut refusals);
    register::<crate::ops::get_asset_contract::GetAssetContract<Eng>>(&mut refusals);
    register::<crate::ops::get_token_balance::GetTokenBalance<Eng>>(&mut refusals);
    register::<crate::ops::get_transaction::GetTransaction<Eng>>(&mut refusals);
    register::<crate::ops::list_asset_contracts::ListAssetContracts<Eng>>(&mut refusals);
    register::<crate::ops::list_filtered_transaction_events::ListFilteredTransactionEvents<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::list_token_balances::ListTokenBalances<Eng>>(&mut refusals);
    register::<crate::ops::list_transaction_events::ListTransactionEvents<Eng>>(&mut refusals);
    register::<crate::ops::list_transactions::ListTransactions<Eng>>(&mut refusals);
    refusals
}

/// Register one operation's modelled-refusal wrapper under the key the model names that
/// operation by, read off the operation itself so the table cannot be keyed by a
/// hand-typed name that drifts from the model.
fn register<Op: Operation>(refusals: &mut HashMap<&'static str, Box<dyn ErasedOperation>>) {
    refusals.insert(Op::NAME, Box::new(ModelledValidation::<Op>::new()));
}

/// One modelled operation, served so that a request refused BEFORE its handler runs is
/// answered in this service's modelled `ValidationException` — with the model-required
/// `reason` and a `fieldList` — rather than in the framework's refusal body, which carries
/// an error code and a `message` and nothing else
/// (`smithy_server_runtime::Refused`).
///
/// The refusal this closes is the one no handler can make: an ABSENT `@required` member is
/// found while the request is being READ, so no input value ever reaches a handler to
/// check (the model makes such a member non-`Option` on the generated input struct). Every
/// other refusal of this service is already made inside the handler, through
/// `crate::state::validation`, and is untouched here.
///
/// Zero-sized: the wrapper holds no engine and no operation instance. It carries the
/// operation TYPE, which is all it needs — the input shape to read the request into and
/// the input `Schema` to read it against — and hands a request the model accepts to the
/// one operation instance the generated routing table built from the shared engine. There
/// is therefore no second instance of any operation and no second store.
///
/// A request this wrapper does not refuse is READ TWICE, once here and once in the
/// operation's own sequence, because the framework hands no already-built input to an
/// operation. That cost is deliberate and bounded by the body limits the framework already
/// enforces; it is what buys the refusal a shape only the handler side could otherwise
/// have.
struct ModelledValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModelledValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation> ErasedOperation for ModelledValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            // The protocol is read, used and dropped inside this one block:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = {
                let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
                let violations = pre_handler_violations::<Op>(protocol, req);
                match crate::state::validation::boundary_exception(&violations) {
                    Some(modelled) => match protocol.serialize_error(
                        &modelled,
                        &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                        VALIDATION_EXCEPTION,
                        VALIDATION_EXCEPTION_STATUS,
                    ) {
                        Ok(response) => Some(response),
                        // A refusal that will not serialize leaves the request to the
                        // framework, which refuses it in its own envelope — the very
                        // divergence this wrapper removes, so it is logged rather than
                        // passed over silently.
                        Err(error) => {
                            tracing::warn!(
                                operation = Op::NAME,
                                %error,
                                "this service's modelled refusal would not serialize; it \
                                 falls back to the framework's own envelope"
                            );
                            None
                        }
                    },
                    None => None,
                }
            };
            if let Some(response) = refused {
                // The row the framework sequence would have recorded for this request, so
                // a refused one is not a hole in the telemetry series.
                record_refusal::<Op>(ctx, started.elapsed());
                return response;
            }
            // Nothing to state: the request is served by the generated operation, which
            // runs the whole framework sequence (deserialize, validate, handle, serialize)
            // plus its telemetry over the same bytes. Reached through the routing table
            // rather than through `override_operation`, which would route straight back
            // here.
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    /// `None` — the framework reads the schemes off the MODELLED operation the request's
    /// routing key names (the generated table's instance), which is what the model bakes
    /// them on, so wrapping an operation cannot change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// Every violation that refuses `req` before `Op`'s handler runs, in the order the
/// framework finds them.
///
/// This is the framework's OWN boundary walk, in the framework's order and with the
/// framework's own code — read the request into the operation's input shape
/// ([`smithy_server_runtime::DeserializableStruct`]), then run the constraint-trait walk
/// over what was read ([`::aws_smithy_schema_server::constraints::validate`]) — so it
/// cannot find a different violation set from the sequence that follows it. Only the
/// WORDING of the answer is this service's, and that lives in
/// [`crate::state::validation`].
///
/// For this service the set is the ABSENCE the deserializer found: every constrained
/// member of every input shape carries `skip_validation` in `src/ops/types/`, so no
/// constraint trait is left at the boundary for the second step to fire on — but the step
/// stays, so a member added to the model without `skip_validation` is answered in the
/// modelled shape too rather than in the framework's.
///
/// EMPTY for a request the model accepts, and empty for a failure that is NOT a refusal (a
/// body the codec cannot read at all, a member of the wrong type): that is not a violation
/// of a modelled rule, so it keeps the framework's own envelope. `member_name` arrives
/// already spelled as a violation path (`aws_smithy_schema_server::missing_member` applies
/// `constraints::path::member` and each enclosing member prepends its own segment), so it
/// places exactly as a constraint violation's path does.
///
/// The same two steps are collected by `service_common::validation::refusing_violations`,
/// which additionally restores each echoed value to the bytes the caller sent. This service
/// echoes no value on THIS path — an absent member has none — and the handler side already
/// owns the echo for the values it refuses, so the two framework calls are named directly
/// here, as the sibling crates that validate at this seam without that dependency do
/// (`wellarchitected`'s `modelled_refusal`). Migrating this service onto the shared walk is
/// that module's own change; nothing here re-implements a check.
fn pre_handler_violations<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, Op::INPUT_SCHEMA) else {
        return Vec::new();
    };
    let read = <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer);
    drop(deserializer);
    match read {
        Ok(input) => ::aws_smithy_schema_server::constraints::validate(&input, Op::INPUT_SCHEMA),
        Err(::aws_smithy_schema::serde::SerdeError::MissingMember { member_name }) => {
            vec![Violation {
                path: member_name,
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            }]
        }
        Err(_) => Vec::new(),
    }
}

/// `ValidationException`'s modelled `@httpError`
/// (`smithy_query(query="shape",shape="ValidationException")`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// Record the refusal answered above as the operation row the framework sequence would
/// have recorded, so a request refused by the wrapper is not a hole in the telemetry
/// series: the same classification (from the refusal's status and error shape), the same
/// folded handle (this service's signing name and the operation's own name), and the same
/// already-sanitized request metadata. Everything is gated on a client collecting, exactly
/// as the framework gates it.
fn record_refusal<Op: Operation>(ctx: &OperationContext<'_>, latency: std::time::Duration) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), Op::NAME);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            VALIDATION_EXCEPTION_STATUS,
            VALIDATION_EXCEPTION,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Serve every modelled operation through its [`ModelledValidation`] wrapper, so a
        // request this service refuses before the handler runs is stated in the service's
        // OWN modelled `ValidationException` — with the model-required `reason` and a
        // `fieldList` — instead of the framework's two-field refusal body. The wrapper
        // hands every request it does not refuse to the generated operation in the routing
        // table, which remains the only thing that serves one.
        //
        // Keyed by the protocol's own routing key, so a request the model names no
        // operation for resolves to nothing here and keeps the framework's routing
        // failure.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.refusals.get(key).map(|wrapper| &**wrapper)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // Every operation of this service models `ValidationException` (HTTP 400) for a
        // request it refuses, and that is the code the framework would have answered
        // too; what changes here is only the wording, which this service states in the
        // AWS validation-framework form — one clause per violation, naming the submitted
        // value, the member path and the constraint, with a `@pattern` quoted as the
        // expression itself rather than as the model's anchored spelling.
        //
        // This hook can reword a refusal but not RESHAPE it: `Refused` carries a code and
        // a message, so nothing reached through here can carry the `reason` the model
        // marks `@required` on `ValidationException`. Every refusal of this service is
        // therefore made where the modelled shape can be returned — in a handler, or in
        // [`ModelledValidation`] for the absence a handler never sees — and this stays as
        // the wording of last resort for a refusal that still renders here (a codec
        // failure the wrapper declines, say).
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
            }),
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
