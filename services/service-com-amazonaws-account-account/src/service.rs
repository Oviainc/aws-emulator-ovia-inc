// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-account-account` service: how it claims requests and how it words the
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
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code Account answers an input-constraint violation with: the modeled
/// `ValidationException`, which this service declares as a common error with
/// `@error("client")` and `@httpError(400)` (the framework's own default code for a
/// refusal, restated here because `override_refusal` replaces code and message
/// together).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The framework's own ceiling on the value a refusal echoes
/// (`MAX_ECHOED_VALUE` in `aws_smithy_schema_server::constraints`, which is
/// private): a longer value reaches this seam ALREADY elided to
/// `<prefix>... [<n> bytes]`, so the echoed string is neither the input string
/// nor a value whose length can be stated. A clause that would have to state
/// one of those declines (see [`length_clause`]), and an elided value is
/// exactly the echo that is strictly longer than the cap.
const ECHOED_VALUE_CAP: usize = 256;

/// One `@pattern` violation as Account states it: the violated ECMA-262 expression
/// and the input string that did not match it, and nothing else — no member name,
/// no path, no count. Account validates a request against the JSON-Schema form of
/// its model, so a refusal reports the regex and the subject, not the Coral-style
/// `Value '<v>' at '<member>' failed to satisfy constraint: ...` clause the
/// framework words by default.
///
/// The expression is the `@pattern` verbatim, anchors included, because that is the
/// expression the value was tested against. `None` for anything that is not a
/// pattern violation, which lets [`refusal_clause`] try the other clauses this
/// service words.
fn pattern_clause(violation: &Violation) -> Option<String> {
    let ViolationKind::Pattern(regex) = &violation.kind else {
        return None;
    };
    // A pattern violation always carries the value it tested; a violation without
    // one (an absent member) is not a pattern violation at all.
    let value = violation.value.as_deref()?;
    Some(format!(
        "ECMA 262 regex \"{regex}\" does not match input string \"{value}\""
    ))
}

/// The whole `ValidationException` message for the violations a HANDLER's own
/// constraint walk found, worded exactly as [`refusal_message`] words the
/// boundary's.
///
/// A handler needs this where it applies a constraint the model declares on a
/// SIBLING member (an email-address expression, say, which this model declares
/// on one of its two address members): the refusal it words then reads like the
/// boundary's refusal of that member, because it IS the same sentence about the
/// same expression, down to how the subject is echoed. `None` means there is
/// nothing to refuse — no violation — or that the violation is of a class this
/// service has no wording for, which is the same answer [`override_refusal`]
/// gives for it.
pub(crate) fn constraint_refusal_message(violations: &[Violation]) -> Option<String> {
    refusal_message(violations)
}

/// One `@length` MAXIMUM violation as Account states it: the subject, its own
/// length and the maximum the constraint admits — `string "<value>" is too long
/// (length: <len>, maximum allowed: <max>)` — in the same JSON-Schema voice as
/// [`pattern_clause`], naming neither the member nor its path. Every element of
/// it is derived from the request and the modelled bound, so the clause is a
/// rule and not a recorded constant.
///
/// `length` is the length the `@length` trait itself counts, which for a string
/// is its CHARACTER count, not its byte count.
///
/// The clause says `string` because every `@length`-constrained INPUT member
/// this service models targets a string shape (`AccountName`, `Name`, `Title`,
/// `EmailAddress`, `PhoneNumber`, `PrimaryEmailAddress`, `NextToken` and the
/// `ContactInformation` members, per
/// `smithy_query(query="operation", operation="…")` over this service's
/// operations; its one input list, `RegionOptStatusContains`, carries no
/// `@length`). Should a non-string bound ever appear, it reaches this seam
/// indistinguishable from a string one, so the wording is kept to what the model
/// actually holds.
///
/// `None` for anything that is not a `@length` maximum violation — a `@length`
/// MINIMUM included: this service has no recorded phrasing for being too short,
/// and the framework's wording is better than an invented sentence. Also `None`
/// when the echoed value was elided past [`ECHOED_VALUE_CAP`], because then
/// neither the subject nor its length is the caller's.
fn length_clause(violation: &Violation) -> Option<String> {
    let ViolationKind::MaxLength(max) = &violation.kind else {
        return None;
    };
    let value = violation.value.as_deref()?;
    if value.len() > ECHOED_VALUE_CAP {
        return None;
    }
    let length = value.chars().count();
    Some(format!(
        "string \"{value}\" is too long (length: {length}, maximum allowed: {max})"
    ))
}

/// The reason Account states for ONE violation, or `None` for a violation class
/// this service has no wording for.
fn refusal_clause(violation: &Violation) -> Option<String> {
    pattern_clause(violation).or_else(|| length_clause(violation))
}

/// The `ValidationException` message for a refusal every one of whose violations
/// Account words itself: the bracketed list of the reasons, one per violated
/// member, in the order the validating walk found them.
///
/// The brackets are the list itself — a single violation reads
/// `[ECMA 262 regex "..." does not match input string "..."]` — so a request that
/// violates two constraints states both reasons inside the one pair of brackets.
///
/// `None` declines the refusal: either it holds no violation at all, or at least
/// one of them is of a class this service has no wording for (see
/// [`refusal_clause`]), and a message half in one service's voice and half in
/// another's is worse than the framework's consistent default.
fn refusal_message(violations: &[Violation]) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    let clauses: Vec<String> = violations
        .iter()
        .map(refusal_clause)
        .collect::<Option<Vec<_>>>()?;
    Some(format!("[{}]", clauses.join(", ")))
}

/// The `com-amazonaws-account-account` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedPerOperation`] so a refused request is worded the way this service words
    /// it — including the input string a clause quotes, which only a reading of the
    /// REQUEST can state. Keyed by the routing key the protocol resolves (the key the
    /// generated table uses) and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    stated: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(StatedPerOperation::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut stated = OperationRoutingTable::new();
        // EVERY modelled operation (one per file in `src/ops/`), because the rule is the
        // SERVICE's and not one operation's: an operation whose input declares no
        // constraint simply never has a violation to state, which the wrapper reads off
        // that operation's own input schema.
        wrap_operations!(
            stated,
            engine,
            accept_primary_email_update::AcceptPrimaryEmailUpdate,
            delete_alternate_contact::DeleteAlternateContact,
            disable_region::DisableRegion,
            enable_region::EnableRegion,
            get_account_information::GetAccountInformation,
            get_alternate_contact::GetAlternateContact,
            get_contact_information::GetContactInformation,
            get_gov_cloud_account_information::GetGovCloudAccountInformation,
            get_primary_email::GetPrimaryEmail,
            get_primary_email_update_status::GetPrimaryEmailUpdateStatus,
            get_region_opt_status::GetRegionOptStatus,
            list_regions::ListRegions,
            put_account_name::PutAccountName,
            put_alternate_contact::PutAlternateContact,
            put_contact_information::PutContactInformation,
            send_phone_number_verification::SendPhoneNumberVerification,
            start_primary_email_update::StartPrimaryEmailUpdate,
            verify_phone_number::VerifyPhoneNumber,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            stated,
        }
    }
}

/// One modelled operation, served so that a refused request is stated the way Account
/// states it — the violated expression or bound and THE INPUT STRING that broke it.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery, so this cannot refuse a request the framework's own walk would accept,
/// nor accept one it would refuse.
///
/// Why the wrapper exists at all, rather than `override_refusal` alone: Account's clause
/// QUOTES the value that broke the constraint, and the validating walk drops the value of
/// a member the model marks `@sensitive` (an email address is one) as it records the
/// violation. `override_refusal` is handed the violations alone, so by then the input
/// string is gone; the shared collecting walk here re-reads the request and restores it
/// (`service_common::validation_message::quoting_the_whole_submitted_value`). A refusal
/// that still reaches `override_refusal` — one raised for an operation this table does not
/// hold — is worded by the SAME two clause builders, so the two paths cannot differ.
struct StatedPerOperation<O> {
    operation: O,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

/// Account's wording for every modelled constraint `req` breaks, for the operation `O`, or
/// `None` when it breaks none — or when one of them is of a class this service has no
/// wording for, which keeps the framework's whole message exactly as
/// [`refusal_message`] decides for the other path.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    let violations: Vec<Violation> =
        service_common::validation::refusing_violations_with_echoes::<O>(protocol, req)
            .into_iter()
            .map(|(violation, echo)| {
                service_common::validation_message::quoting_the_whole_submitted_value(
                    violation, &echo,
                )
            })
            .collect();
    let message = refusal_message(&violations)?;
    Some((
        violations,
        Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message,
        },
    ))
}

impl<O> ErasedOperation for StatedPerOperation<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The definition is read, used and dropped inside each statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                return svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    Some(stated),
                    req,
                );
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
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
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in [`StatedPerOperation`]
        // so a refused request is worded the way this service words it, with the input
        // string its clause quotes. A key the model does not name resolves to `None` here
        // exactly as it does in the generated table, so the router's routing-error path is
        // unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Account states a violation in the JSON-Schema voice its validator
        // speaks, for every constrained member of every operation and at any
        // depth in the input — the message names no member and no path:
        //
        // - a `@pattern` violation as the violated ECMA-262 expression and the
        //   input string that failed it;
        // - a `@length` MAXIMUM violation as the subject, its own length and the
        //   maximum admitted.
        //
        // The code stays the modeled `ValidationException` (HTTP 400), which is
        // what the framework would have answered; only the wording changes.
        //
        // Every other refusal — an absent required member, a `@length` minimum, a
        // `@range` bound, an enum value, an authentication failure — keeps the
        // framework's wording: nothing records how this service phrases those.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: refusal_message(violations)?,
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
