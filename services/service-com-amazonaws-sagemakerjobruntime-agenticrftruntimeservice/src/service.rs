// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice` service: how it claims requests and how it words the
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
//! - `can_handle` — which requests this service claims. EDITED HERE: this service's
//!   SigV4 signing name (`sagemaker`) is shared with every other SageMaker-family
//!   service, so the claim also requires the request to address this service's own
//!   modeled URI path space.
//! - `override_operation` — which operation serves a request. EDITED HERE: the key and
//!   the handler stay the generated table's, but the operation is served through
//!   [`InModelledEnvelope`], which refuses a request outside a modelled constraint with
//!   this service's own `ValidationException` — whose prose member the model calls
//!   `Message` — instead of the framework's pre-dispatch envelope, which names only a
//!   lowercase `message`.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE: a value outside a declared constraint is
//!   `ValidationException`, worded from the violations the framework collected, and a
//!   value that is not an ARN at all is reported as a malformed ARN.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! Beside those seams, this file owns one submodule: [`authorization`], the IAM decision
//! every operation of this service takes before it resolves what a request names. It sits
//! here because it is about the REQUEST and the credentials it was signed with — the same
//! subject as `can_handle` and the refusal seams — rather than about the trajectories
//! `crate::state` holds.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

/// WHO may reach this service's operations: the IAM authorization step every one of its
/// handlers runs before it resolves the job or the trajectory a request names.
///
/// It is a submodule of this file rather than of `state`, because the decision is about
/// the REQUEST and the caller's credentials — not about the trajectories this service
/// stores, which `state` owns.
pub mod authorization;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The routing keys that name THIS service rather than its signing name: the
/// model's endpoint prefix (`job-runtime.sagemaker`) and that prefix's first
/// label, which is what `emulator_server::service_from_host` derives from this
/// service's own endpoint host (`job-runtime.sagemaker.<region>.amazonaws.com`).
///
/// The signing name (`sagemaker`) is shared with every other SageMaker-family
/// service, so it cannot identify this one on its own; the endpoint prefix can.
const ENDPOINT_KEYS: [&str; 2] = ["job-runtime.sagemaker", "job-runtime"];

/// The dotted member path of this service's ARN-valued member, as the validating walk
/// spells it: `JobArn` in lowerCamel. Every operation of this service binds it, so the
/// path is matched by its tail rather than by whole-string equality.
const ARN_MEMBER_PATH: &str = "jobArn";

/// The error code a refused request answers with, unchanged from the one the framework
/// would have chosen for a constraint violation: the shape
/// `com.amazonaws.sagemakerjobruntime#ValidationException`, which every operation of
/// this service lists and whose one prose member the model calls `Message`
/// (`smithy_query(query="shape",shape="ValidationException")`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The greatest number of bytes of prose THIS SERVICE's errors can carry, read off the
/// model: every error shape it declares (`AccessDeniedException`, `ConflictException`,
/// `InternalServiceError`, `ResourceNotFoundException`, `ServiceQuotaExceededException`,
/// `ThrottlingException`, `ValidationException`) has exactly one member, `Message`,
/// targeting `FailureReason`, whose `@length` is min 0 **max 1024**
/// (`smithy_query(query="shape",shape="FailureReason,ValidationException")`).
///
/// It is the member that has to HOLD a refusal, so it is the bound on what a refusal may
/// say — not a property of any member a request carries.
const LARGEST_MESSAGE_BYTES: usize = 1024;

/// What ONE clause of a refusal of this service states besides the value it quotes.
///
/// The longest such text this model can produce is the envelope
/// (`1 validation error detected: Value '` … `' at '<member path>' failed to satisfy
/// constraint: `, 85 bytes over this model's longest member path, `trajectoryId`) plus the
/// longest requirement the framework words for a member of this model — the job-ARN
/// `@pattern` as a regular expression, 150 bytes — which is 235 together; every other
/// requirement (an `@length` bound, the `CompletionStatus` value set, an absent `@required`
/// member) is shorter, and so is this service's own malformed-ARN wording.
///
/// It is the allowance for ONE clause, which is what [`LARGEST_QUOTED_VALUE_BYTES`] needs: a
/// sentence of SEVERAL clauses carries that much prose per clause, so what its clauses may
/// quote together is bounded by the sentence rather than by this
/// ([`held_within_the_message_maximum`], which reads the prose off the rendered sentence
/// instead of counting it). `a_refusal_of_any_clause_count_fits_the_carrying_member` pins
/// both, so a wording that outgrew the allowance fails here rather than overrunning the
/// member.
const LARGEST_CLAUSE_PROSE_BYTES: usize = 256;

/// The largest single value ONE clause of this service's refusal quotes back.
///
/// An AWS constraint message quotes the value it refused, and the shared restorer's own
/// default ceiling (`service_common::validation`, 16 KiB) is sized for the members most
/// models declare — 64x the 256 maximum a typical member carries. This model bounds the
/// member a refusal is CARRIED in instead ([`LARGEST_MESSAGE_BYTES`]), so quoting to that
/// default answers a `Message` up to sixteen times the maximum its own shape declares. The
/// ceiling is therefore stated from the model: the declared maximum of the carrying member,
/// less what one clause states around the value ([`LARGEST_CLAUSE_PROSE_BYTES`]), so no
/// single value is restored larger than the member could hold even alone. It is a MODEL
/// constant, not a request-sized one; past it the framework's own shortened rendering stands
/// — a character-boundary-safe prefix plus the marker naming the submitted byte count —
/// which is the one divergence `services/common/src/validation.rs` records as the core seam
/// that whole re-read exists until.
///
/// This is the ceiling on ONE value. The bound on the whole sentence is
/// [`refusal_message`]'s, and it holds whatever the clause count, on both refusal paths of
/// this service: the violations collected here, and the clause
/// `crate::ops::complete_rollout` words itself for a value its handler is the one to reach.
pub const LARGEST_QUOTED_VALUE_BYTES: usize = LARGEST_MESSAGE_BYTES - LARGEST_CLAUSE_PROSE_BYTES;

/// How this service reports a value that is not an ARN at all.
///
/// Built from the value the request sent, which is the only thing the caller can act on —
/// held, like every clause, to what the carrying member leaves once this sentence's own
/// prose is in it ([`refusal_message`]).
#[must_use]
fn malformed_arn_message(value: &str) -> String {
    format!("Input arn {value} is malformed")
}

/// What [`malformed_arn_message`] states besides the value, so the value's room in it is
/// read off the sentence rather than counted by hand.
fn malformed_arn_prose_bytes() -> usize {
    malformed_arn_message("").len()
}

/// Whether `violation` is an ARN-valued member holding something that is not an ARN.
///
/// The member is named by the LAST segment of the dotted path, so a member whose name
/// merely ends in those letters (`notAJobArn`) is not mistaken for it.
fn is_malformed_arn(violation: &Violation) -> bool {
    violation.path.rsplit('.').next() == Some(ARN_MEMBER_PATH)
        && matches!(violation.kind, ViolationKind::Pattern(_))
}

/// The sentence this service refuses `violations` with, or `None` when there is nothing
/// to refuse.
///
/// Every clause comes from the framework's own constraint renderer
/// (`constraints::envelope`), so a constraint class this service has never seen cannot
/// arrive with a sentence of its own. ONE class is worded differently: a value that is
/// not an ARN at all. Every member of this service whose value is an ARN (`JobArn`, on
/// all four operations) carries the `@pattern`
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/…$`, and the service reports a
/// value that fails it as a malformed ARN naming the value it was given, rather than as
/// a regular-expression clause: a caller cannot act on the pattern source, and the ARN
/// is the thing it got wrong.
///
/// It is also where the MEMBER that has to hold this sentence bounds it
/// ([`LARGEST_MESSAGE_BYTES`]): the ONE renderer both refusal paths of this service reach,
/// so the bound cannot hold on one path and not the other, and the only part of a sentence
/// that is the caller's to size — the values its clauses quote — is held to what the prose
/// leaves ([`held_within_the_message_maximum`]). A sentence already inside the maximum is
/// returned exactly as the framework worded it.
pub(crate) fn refusal_message(violations: &[Violation]) -> Option<String> {
    // Only when the value is actually known: a pattern violation that captured none
    // would otherwise be reported as "Input arn  is malformed", which tells the caller
    // nothing, so the framework's own clause is kept for it instead.
    let malformed_arn = violations
        .iter()
        .filter(|violation| is_malformed_arn(violation))
        .find_map(|violation| violation.value.as_deref());
    if let Some(value) = malformed_arn {
        // One clause, one value: its room is the whole member less this wording's prose.
        let room = LARGEST_MESSAGE_BYTES.saturating_sub(malformed_arn_prose_bytes());
        return Some(malformed_arn_message(
            &service_common::validation::echo_held_to(value, room),
        ));
    }
    let rendered = constraints::envelope(violations)?;
    if rendered.len() <= LARGEST_MESSAGE_BYTES {
        return Some(rendered);
    }
    held_within_the_message_maximum(violations, &rendered).or(Some(rendered))
}

/// `violations` worded again with every quoted value held to its share of what the carrying
/// member leaves after the clauses' own prose, for a sentence that did not fit it.
///
/// Why a share rather than a per-clause ceiling: the clauses of one refusal are joined into
/// ONE `Message`, so what they may quote is bounded TOGETHER — a ceiling that fits one
/// clause fits two of them twice over. The prose is read off the sentence the framework just
/// worded (its length less the echoes interpolated into it), so no count of clause text is
/// kept by hand, and what is left is handed out shortest echo first: a clause that needs
/// less than an equal share leaves the rest to the clauses that quote more, so the sentence
/// states as much of each value as the member admits.
///
/// Holding an echo keeps the byte count it names
/// ([`service_common::validation::echo_held_to`]), so every clause still reports how much
/// was sent. Where the clauses' own prose alone fills the member — which takes more clauses
/// than this model's four input members can break at once — there is nothing caller-sized
/// left to hold, and the framework's own wording stands rather than a sentence with clauses
/// missing from it.
fn held_within_the_message_maximum(
    violations: &[Violation],
    rendered: &str,
) -> Option<String> {
    // The clauses that quote a value, shortest echo first, so an unspent share reaches the
    // clauses that quote more.
    let mut quoting: Vec<usize> = (0..violations.len())
        .filter(|at| violations[*at].value.is_some())
        .collect();
    if quoting.is_empty() {
        return None;
    }
    quoting.sort_by_key(|at| violations[*at].value.as_deref().unwrap_or_default().len());

    let echoed: usize = violations
        .iter()
        .filter_map(|violation| violation.value.as_deref())
        .map(str::len)
        .sum();
    let prose = rendered.len().saturating_sub(echoed);
    let mut budget = LARGEST_MESSAGE_BYTES.saturating_sub(prose);
    let mut room = vec![0_usize; violations.len()];
    let mut left = quoting.len();
    for at in quoting {
        let share = budget / left;
        let echo = violations[at].value.as_deref().unwrap_or_default();
        let spent = service_common::validation::echo_held_to(echo, share).len();
        budget -= spent.min(budget);
        room[at] = share;
        left -= 1;
    }

    let held: Vec<Violation> = violations
        .iter()
        .enumerate()
        .map(|(at, violation)| Violation {
            path: violation.path.clone(),
            kind: violation.kind.clone(),
            value: violation
                .value
                .as_deref()
                .map(|echo| service_common::validation::echo_held_to(echo, room[at]).into_owned()),
            sensitive: violation.sensitive,
        })
        .collect();
    constraints::envelope(&held)
}

/// The names that identify THIS service to the router: its SigV4 signing name — shared
/// with every other SageMaker-family service, so it cannot identify this one on its own
/// — and its own [`ENDPOINT_KEYS`].
fn claiming_names(signing_name: &str) -> impl Iterator<Item = &str> {
    std::iter::once(signing_name).chain(ENDPOINT_KEYS)
}

/// The `com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`InModelledEnvelope`] so a request refused before the handler runs is answered
    /// in this service's own `ValidationException`. Keyed by the routing key the
    /// protocol resolves — the key the generated table uses — and built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
    in_modelled_envelope: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`InModelledEnvelope`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(InModelledEnvelope::new(
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
        let mut in_modelled_envelope = OperationRoutingTable::new();
        wrap_operations!(
            in_modelled_envelope,
            engine,
            complete_rollout::CompleteRollout,
            sample::Sample,
            sample_with_response_stream::SampleWithResponseStream,
            update_reward::UpdateReward,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_modelled_envelope,
        }
    }
}

/// One modelled operation, served with the refusals it owes stated in this service's own
/// `ValidationException`.
///
/// The framework's pre-dispatch renderer gets the status (400) and the error code
/// (`ValidationException`) right, but writes the prose into a framework-chosen lowercase
/// `message` key, the same for every restJson1 service. This service models the prose
/// member of each of its error shapes as `Message` (`FailureReason`, `@required`,
/// `@length` min 0 max 1024 — the bound [`LARGEST_QUOTED_VALUE_BYTES`] is stated from), so
/// that envelope carries no `Message` at all and an SDK surfaces a constraint refusal
/// with an empty reason.
///
/// So this wrapper collects the refusal itself — the very two classes the framework's own
/// walk collects, from the framework's own deserializer and constraint walk — and renders
/// it through the model's `ValidationException` schema. The envelope is built from the
/// structured [`Violation`]s, never from a rendered response. Nothing else is
/// re-implemented: a request this service does not refuse is handed to the generated
/// `ErasedOperation::run`, which owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error.
struct InModelledEnvelope<O> {
    operation: O,
}

impl<O> InModelledEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InModelledEnvelope<O>
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
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition` is
            // not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            let violations = refused_by::<O>(protocol, req);
            if let Some(refusal) = validation_exception(protocol, &violations) {
                return refusal;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every violation that refuses `req` before `O`'s handler runs: a `@required` member
/// absent, or a value outside a declared constraint. Empty when the request is valid,
/// and empty for a failure that is NOT a refusal — a body the codec cannot read is a
/// serialization failure, which the framework's own walk answers in its own envelope.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), so this cannot find a different set of
/// violations from the walk that follows it.
///
/// What the violations are then put through is NOT another check: the shared restorer
/// re-reads the SAME built input to restore each violation's value echo to the bytes the
/// caller sent, which the framework's walk stores elided (a prefix plus `... [<n> bytes]`).
/// An AWS constraint-validation message is built from the request, so the echoed value is
/// part of the rule a caller reads the rejected input back out of. The collecting step is
/// the shared crate's: it is the same for every service that words its own pre-handler
/// refusal, and only the ENVELOPE below is this service's.
///
/// How LONG a value it will quote is this service's own, and is stated from this model
/// ([`LARGEST_QUOTED_VALUE_BYTES`]) rather than left to the shared default, because this
/// model bounds the member a refusal is carried in.
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    service_common::validation::refusing_violations_quoting_up_to::<O>(
        protocol,
        req,
        LARGEST_QUOTED_VALUE_BYTES,
    )
}

/// This service's answer to `violations`: HTTP 400 `ValidationException` with the prose
/// in the `Message` member the model declares, serialized through that shape's own
/// schema. `None` when there is nothing to refuse — and also when the error cannot be
/// serialized, which leaves the framework's walk to refuse the request in its own
/// envelope rather than answering a 400 with no reason at all.
fn validation_exception(
    protocol: &dyn ServerHttpProtocol,
    violations: &[Violation],
) -> Option<RtResponse> {
    let refused = crate::ops::types::shared_types::ValidationException {
        message: refusal_message(violations)?,
    };
    protocol
        .serialize_error(
            &refused,
            &crate::ops::types::shared_types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        .ok()
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `sagemaker` — a signing name it SHARES with every
        // other SageMaker-family service (the control plane, the endpoint runtimes,
        // A2I, Edge, FeatureStore, Metrics, the training-session runtime), while its
        // own endpoint is `job-runtime.sagemaker`. The router resolves the routing key
        // from the SigV4 credential scope first, so all of them see the key
        // `sagemaker`: a service that claims on the signing name ALONE claims every
        // sibling's traffic too, and the router — which refuses rather than guesses —
        // answers `500 AmbiguousService` for ALL of it.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1`: its surface is the URI path space its
        // operations bind (`/sample`, `/sample-with-response-stream`,
        // `/complete-rollout`, `/update-reward`), which is what `claims_request` matches
        // against the generated `ROUTE_PATH_SEGMENTS` when a service declares no RPC
        // protocol. The rule is applied to every name that identifies this service, so a
        // request addressed at its own endpoint host rather than signed reaches it too.
        claiming_names(self.entry.identity().signing_name()).any(|name| {
            service_common::claims_request(parts, name, crate::ROUTE_PATH_SEGMENTS, None)
        })
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `InModelledEnvelope` so a request refused before the handler runs is refused
        // in this service's `ValidationException`, whose prose member the model calls
        // `Message`, rather than in the framework's pre-dispatch envelope, which names
        // only a lowercase `message`. A key the model does not name resolves to `None`
        // here exactly as it does in the generated table, so the router's routing-error
        // path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_modelled_envelope.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `InModelledEnvelope` refuses a constraint violation itself, in the modelled
        // envelope, so this seam is what words any refusal the FRAMEWORK still renders.
        // Wording it here from the same violations keeps the two paths from disagreeing:
        // a value outside a declared bound is this service's `ValidationException`
        // either way, with the same sentence (see `refusal_message`).
        //
        // One thing this seam cannot do is restore an elided value echo: the framework
        // hands over the violations alone, not the request they were collected from, and
        // the value a violation stores was already shortened when it was built. The
        // request that reaches here has therefore already been refused by
        // `InModelledEnvelope` (which re-reads the input and does restore it), so this is
        // the fallback for the one case that path declines: an error this service's own
        // schema could not serialize.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: refusal_message(violations)?,
            }),
            // Authentication is the framework's to word: this service models
            // `AccessDeniedException` for a caller it refuses, and the request never
            // reached an operation, so there is nothing of this service's own to say.
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

#[cfg(test)]
mod tests {
    use super::{
        refusal_message, Violation, ViolationKind, ARN_MEMBER_PATH, LARGEST_CLAUSE_PROSE_BYTES,
        LARGEST_MESSAGE_BYTES, LARGEST_QUOTED_VALUE_BYTES,
    };

    /// The `@pattern` every ARN-valued member of this model declares, as the framework
    /// quotes it in a requirement
    /// (`smithy_query(query="shape",shape="JobArn")`) — the longest requirement text a
    /// clause of this service can state.
    const JOB_ARN_PATTERN: &str = r"^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$";

    /// The longest member path this model binds, which every row states its clause over so
    /// the prose measured is the longest this service can produce
    /// (`smithy_query(query="operation",operation="CompleteRollout")`: `TrajectoryId`, the
    /// validating walk's lowerCamel spelling).
    const LONGEST_MEMBER_PATH: &str = "trajectoryId";

    /// One violation per constraint class this service's refusals can state, each over
    /// [`LONGEST_MEMBER_PATH`] and quoting `value`.
    fn every_requirement(value: &str) -> Vec<(&'static str, Violation)> {
        let clause = |kind: ViolationKind, value: Option<String>| Violation {
            path: LONGEST_MEMBER_PATH.to_string(),
            kind,
            value,
            sensitive: false,
        };
        vec![
            (
                "an absent required member",
                clause(ViolationKind::MissingRequiredMember, None),
            ),
            (
                "a length minimum",
                clause(ViolationKind::MinLength(1), Some(value.to_string())),
            ),
            (
                "a length maximum",
                clause(ViolationKind::MaxLength(256), Some(value.to_string())),
            ),
            (
                "the job-ARN regular expression",
                clause(
                    ViolationKind::Pattern(JOB_ARN_PATTERN.to_string()),
                    Some(value.to_string()),
                ),
            ),
            (
                "the completion-status value set",
                clause(
                    ViolationKind::EnumValue(vec!["ready".to_string(), "failed".to_string()]),
                    Some(value.to_string()),
                ),
            ),
        ]
    }

    /// RULE: a refusal of this service fits the maximum the carrying member declares,
    /// whatever its clauses quote and however many clauses it states — the allowance one
    /// clause's prose is given covers every requirement this model can word, and what
    /// SEVERAL clauses quote is held to what the sentence leaves them.
    ///
    /// Rows are the constraint classes a refusal of this service can state, each over the
    /// longest member path this model binds, at the clause counts a request to this service
    /// can reach (its four input members break at most four constraints at once) and well
    /// past them; the malformed-ARN wording this service puts in place of the ARN pattern
    /// clause is measured beside them.
    #[test]
    fn a_refusal_of_any_clause_count_fits_the_carrying_member() {
        // ONE clause: the prose allowance the per-value ceiling is stated from.
        let at_ceiling = "v".repeat(LARGEST_QUOTED_VALUE_BYTES);
        for (case, violation) in every_requirement(&at_ceiling) {
            let quoted = violation.value.as_deref().unwrap_or_default().len();
            let message = refusal_message(std::slice::from_ref(&violation))
                .unwrap_or_else(|| panic!("case {case:?}: expected a refusal sentence"));
            let prose = message.len() - quoted;
            assert!(
                prose <= LARGEST_CLAUSE_PROSE_BYTES,
                "case {case:?}: expected at most {LARGEST_CLAUSE_PROSE_BYTES} bytes of clause \
                 prose, got {prose}"
            );
            assert!(
                message.len() <= LARGEST_MESSAGE_BYTES,
                "case {case:?}: expected a refusal within the {LARGEST_MESSAGE_BYTES} byte \
                 maximum the carrying member declares, got {} bytes",
                message.len()
            );
        }

        // SEVERAL clauses, every one of them quoting a value: rows differ along the clause
        // count — two, the three a request to this service can reach, and four, the most its
        // models' constrained members could ever break at once (`JobArn` length and pattern,
        // `TrajectoryId` length, `Status` value set; `ClientToken`, `Rewards` and the
        // inference `Body` declare no constraint to break) — and along the size of the values
        // quoted, from under the per-value ceiling to a megabyte, which the walk hands over
        // already shortened.
        let every_class: Vec<Violation> = every_requirement(&at_ceiling)
            .into_iter()
            .filter(|(_, violation)| violation.value.is_some())
            .map(|(_, violation)| violation)
            .collect();
        for clauses in [2_usize, 3, 4] {
            for (sized, value) in [
                ("a quarter of the per-value ceiling", {
                    "v".repeat(LARGEST_QUOTED_VALUE_BYTES / 4)
                }),
                ("the per-value ceiling", at_ceiling.clone()),
                (
                    "a value the walk already shortened",
                    service_common::validation::quoted_echo_up_to(
                        &"v".repeat(1024 * 1024),
                        LARGEST_QUOTED_VALUE_BYTES,
                    )
                    .into_owned(),
                ),
            ] {
                let case = &format!("{clauses} clauses, each quoting {sized}");
                let violations: Vec<Violation> = (0..clauses)
                    .map(|at| {
                        let mut violation = every_class[at % every_class.len()].clone();
                        violation.value = Some(value.clone());
                        violation
                    })
                    .collect();
                let message = refusal_message(&violations)
                    .unwrap_or_else(|| panic!("case {case:?}: expected a refusal sentence"));
                assert!(
                    message.starts_with(&format!("{clauses} validation errors detected: ")),
                    "case {case:?}: expected all {clauses} clauses in one envelope, got {:?}",
                    message.chars().take(120).collect::<String>()
                );
                assert!(
                    message.len() <= LARGEST_MESSAGE_BYTES,
                    "case {case:?}: expected a refusal within the {LARGEST_MESSAGE_BYTES} byte \
                     maximum the carrying member declares, got {} bytes",
                    message.len()
                );
            }
        }

        let malformed = refusal_message(&[Violation {
            path: ARN_MEMBER_PATH.to_string(),
            kind: ViolationKind::Pattern(JOB_ARN_PATTERN.to_string()),
            value: Some("z".repeat(1024 * 1024)),
            sensitive: false,
        }])
        .expect("the malformed-ARN sentence");
        assert!(
            malformed.len() <= LARGEST_MESSAGE_BYTES,
            "case \"the malformed-ARN wording\": expected a refusal within the \
             {LARGEST_MESSAGE_BYTES} byte maximum the carrying member declares, got {} bytes",
            malformed.len()
        );
        assert!(
            malformed.starts_with("Input arn ") && malformed.ends_with(" is malformed"),
            "case \"the malformed-ARN wording\": expected this service's own wording, got \
             {malformed:?}"
        );
    }
}
