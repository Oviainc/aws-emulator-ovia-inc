// SPDX-License-Identifier: CC0-1.0

//! The ONE question every merge that takes a `conflictResolution` asks before it
//! looks at a single entry of that list: can the request's conflict resolution
//! strategy honour a resolution list at all?
//!
//! A `conflictResolution` is "if AUTOMERGE is the conflict resolution strategy, a
//! list of inputs to use when resolving conflicts during a merge": under any other
//! strategy the version each conflicting path takes is already decided, so a list
//! supplied with one cannot be honoured and the COMBINATION is what fails. The
//! service therefore judges the combination first and never examines an entry of a
//! list it will not read — a request that supplies a resolution list under `NONE` is
//! refused with `InvalidConflictResolutionException` whether or not its entries
//! would have been usable under `AUTOMERGE`.
//!
//! Each merge handler asks that question itself
//! ([`crate::state::merge::resolution_cannot_be_honoured`], the one reading of it
//! this crate has) — but it cannot ask it FIRST, because the framework validates
//! the request against the model before any handler runs, and
//! `ReplaceContentEntry::replacementType` is a modelled enum. A value outside that
//! enum, in a list the strategy rules out, would therefore be refused as an invalid
//! replacement type: a different error class, a different SDK error type, and a
//! sentence describing an entry the service never reads. This layer asks the
//! question at the one seam a service has before dispatch
//! ([`crate::service::EmulatedService::override_operation`]), exactly as
//! `services/service-com-amazonaws-xray-awsxray` does for its own request layer,
//! and only for a request the model's own constraints would otherwise refuse first.
//!
//! Nothing about the model is written down here. WHICH values the model refuses is
//! decided by running the framework's OWN walk over the built input
//! ([`constraints::validate`] against the operation's `INPUT_SCHEMA`, the same call
//! the dispatch sequence makes), and the input is built by the framework's own
//! deserializer for the same operation — so this layer cannot disagree with the walk
//! that follows it. A request whose values the model accepts is left alone entirely:
//! its handler then refuses the combination itself, in the same words.
//!
//! The cost is deliberate, and it is paid only by the five operations that carry a
//! `conflictResolution`: such a request's body is read twice before dispatch (once
//! where the decision is made, in `Service::override_operation`, and again when the
//! refusal re-derives it from the request the router passes back — that seam hands
//! back a BORROWED operation, so there is nowhere to carry the first answer), and a
//! third time by the dispatch that serves it when this layer leaves it alone. Every
//! other operation of the service pays nothing: the routing key decides that before
//! a byte of the body is read.

use std::marker::PhantomData;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{self, Violation};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refused,
    ServerHttpProtocol, Service,
};

use crate::ops::types::ConflictResolution;
use crate::state::merge;

/// The error code CodeCommit refuses a resolution list its strategy cannot honour
/// with: the shape the model declares for exactly that ("The specified conflict
/// resolution list is not valid", client, 400), which every operation taking a
/// `conflictResolution` models.
const INVALID_CONFLICT_RESOLUTION: &str = "InvalidConflictResolutionException";

/// The member of a merge request this layer reads, over the input shapes of the
/// five operations that carry one.
///
/// Each operation declares its own input shape, so the two members are reached
/// through this trait rather than by naming one shape; the bodies are identical,
/// including the strategy a request that names none is evaluated with.
trait ResolutionRequest {
    /// The strategy this merge is evaluated with: the one the request asked for,
    /// or the documented default where it asked for none.
    fn strategy(&self) -> &str;
    /// The resolution list the request supplied, if any.
    fn resolution(&self) -> Option<&ConflictResolution>;
}

/// Implement [`ResolutionRequest`] for each input shape that carries the two
/// members, which the model spells identically on every one of them.
macro_rules! resolution_requests {
    ($($input:ident),+ $(,)?) => {
        $(
            impl ResolutionRequest for crate::ops::types::$input {
                fn strategy(&self) -> &str {
                    self.conflict_resolution_strategy
                        .as_deref()
                        .unwrap_or(merge::CONFLICT_RESOLUTION_NONE)
                }

                fn resolution(&self) -> Option<&ConflictResolution> {
                    self.conflict_resolution.as_ref()
                }
            }
        )+
    };
}

resolution_requests!(
    CreateUnreferencedMergeCommitInput,
    MergeBranchesBySquashInput,
    MergeBranchesByThreeWayInput,
    MergePullRequestBySquashInput,
    MergePullRequestByThreeWayInput,
);

/// Dispatch [`probe`] on the routing key, one line per operation that takes a
/// `conflictResolution`. The arm's key is the operation's shape name, which is
/// also the name of the generated operation type.
macro_rules! probe_by_key {
    ($key:expr, $protocol:expr, $req:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        match $key {
            $(
                stringify!($operation) => {
                    probe::<crate::ops::$module::$operation<Eng>>($protocol, $req)
                }
            )+
            _ => None,
        }
    };
}

/// The refusal this layer answers `req` with, or `None` to leave the request to
/// the framework and the handler it names.
///
/// `Some` carries the violations the model's own walk found — so the fail-safe
/// direction is the framework's own refusal of them — beside the words CodeCommit
/// refuses the combination with.
pub fn refusal_for<Eng: Send + Sync + 'static>(
    key: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    probe_by_key!(
        key,
        protocol,
        req,
        create_unreferenced_merge_commit::CreateUnreferencedMergeCommit,
        merge_branches_by_squash::MergeBranchesBySquash,
        merge_branches_by_three_way::MergeBranchesByThreeWay,
        merge_pull_request_by_squash::MergePullRequestBySquash,
        merge_pull_request_by_three_way::MergePullRequestByThreeWay,
    )
}

/// The member path every value of a `conflictResolution` block stands under.
const RESOLUTION_MEMBER: &str = "conflictResolution";

/// Does `path` name the `conflictResolution` block or a value INSIDE it?
///
/// A constraint path is its member names dot-joined
/// (`vendor/smithy-schema-server/src/constraints.rs`), so the step into the block
/// has to be matched as a step: the sibling member `conflictResolutionStrategy`
/// starts with the same letters, and a value of THAT member is refused for the
/// member it is (`crate::service`'s own
/// `InvalidConflictResolutionStrategyException`), not for the combination.
fn stands_in_the_resolution(path: &str) -> bool {
    path == RESOLUTION_MEMBER
        || path
            .strip_prefix(RESOLUTION_MEMBER)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// Whether `req` supplies a resolution list its strategy cannot honour AND
/// carries, inside that list, a value the model refuses — the one case where the
/// framework's walk would answer before the handler does.
///
/// `None` in every other case, deliberately: a request whose values the model
/// accepts is refused by its handler in the same words, and a request that breaks
/// a constraint OUTSIDE the resolution list is refused for that value, which is
/// not about the list at all.
fn probe<O>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Option<(Vec<Violation>, Refused)>
where
    O: Operation,
    O::Input: ResolutionRequest,
{
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let input = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer).ok()?;
    drop(deserializer);
    let strategy = input.strategy();
    if !merge::resolution_cannot_be_honoured(strategy, input.resolution()) {
        return None;
    }
    let violations = constraints::validate(&input, O::INPUT_SCHEMA);
    if violations.is_empty()
        || !violations
            .iter()
            .all(|violation| stands_in_the_resolution(&violation.path))
    {
        return None;
    }
    Some((
        violations,
        Refused {
            code: INVALID_CONFLICT_RESOLUTION.to_string(),
            message: merge::invalid_conflict_resolution_message(strategy),
        },
    ))
}

/// The operation this service serves a merge request whose `conflictResolution`
/// its strategy cannot honour, where the model's own constraints would otherwise
/// refuse one of that list's entries first.
///
/// An [`ErasedOperation`] rather than a response built in
/// [`Service::override_operation`], because that seam hands the router a BORROWED
/// operation. It holds no state: the refusal is re-derived from the same request
/// the router passes back into [`ErasedOperation::run`], so one value serves every
/// such request.
///
/// `Eng` is the engine the service is generic over. It is never touched — the
/// refusal reads nothing and writes nothing — but naming it is what lets this value
/// sit on `EmulatedService<Eng>` and reach the per-operation input shapes, which
/// are generic over the same engine.
pub struct UnhonourableConflictResolution<Eng> {
    engine: PhantomData<Eng>,
}

impl<Eng> UnhonourableConflictResolution<Eng> {
    /// The one refusal value a service holds.
    pub const fn new() -> Self {
        Self {
            engine: PhantomData,
        }
    }
}

impl<Eng> Default for UnhonourableConflictResolution<Eng> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for UnhonourableConflictResolution<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn
            // ErasedServiceDefinition` is not `Sync` and this future is. The
            // protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            // The same decision `override_operation` made, re-derived from the
            // same request: this operation is reached only when it answered
            // `Some`, so the refusal is found again. If it somehow is not, the
            // framework renders its own refusal of whatever the walk found, which
            // is the fail-safe direction.
            let (violations, chosen) = match protocol
                .routing_key(req)
                .ok()
                .and_then(|key| refusal_for::<Eng>(key, protocol, req))
            {
                Some((violations, chosen)) => (violations, Some(chosen)),
                None => (Vec::new(), None),
            };
            svc.definition()
                .refusal_response(ProtocolError::Invalid(violations), chosen, req)
        })
    }
}
