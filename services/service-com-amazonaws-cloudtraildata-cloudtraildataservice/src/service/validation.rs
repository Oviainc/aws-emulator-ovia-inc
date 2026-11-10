// SPDX-License-Identifier: CC0-1.0

//! How this service states a request it refused BEFORE any handler ran: the operation
//! wrapper that re-reads the request so a clause about a COLLECTION member can quote the
//! collection the caller submitted.
//!
//! Why a wrapper at all: [`smithy_server_runtime::Service::override_refusal`] is handed
//! the violations alone and never the request they were found in, so the one clause whose
//! value the framework's validating walk could not render — a list has no scalar
//! rendering, and the walk stores its own `<n> items` summary of one, which no caller ever
//! sent — cannot be completed there. Here the input can be read again.
//!
//! Nothing is re-implemented: the violations and the value echoes come from the tree's
//! shared collecting walk (`service_common::validation`, which runs the protocol's own
//! deserializer and the framework's own constraint check and owns every bound on what one
//! clause may quote), each clause's restatement from `service_common::validation_message`,
//! the envelope and every other clause from the framework, and the status and the error
//! body from the framework's refusal channel. A request this wrapper does not refuse is
//! handed to the generated operation, which owns deserialize -> validate -> handle ->
//! serialize and every modelled error.

use super::refuses_channel_arn;
use crate::ops::put_audit_events::{INVALID_CHANNEL_ARN_CODE, INVALID_CHANNEL_ARN_MESSAGE};
use service_common::validation::SubmittedEcho;
use service_common::validation_message::quoting_the_submitted_collection;
use smithy_server_runtime::{ErasedOperation, Operation, Refused, Service, Violation};

/// The error code the framework's own default names a constraint refusal with on this
/// service's JSON protocol, restated here because [`Refused`] replaces the code and the
/// message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The member positions one element of the batch spends: one per member `AuditEvent`
/// declares — id, eventData, eventDataChecksum
/// (`smithy_query(query="shape", shape="AuditEvent")`) — and one for the structure itself.
const ECHO_POSITIONS_PER_EVENT: usize = 4;

/// The members `PutAuditEventsRequest` declares, each a position of its own: the batch, the
/// channel reference and the external id
/// (`smithy_query(query="operation", operation="PutAuditEvents")`).
const REQUEST_MEMBERS: usize = 3;

/// The largest batch the shared echo-collecting walk renders at all: it renders a composite
/// from at most 256 children and keeps no rendering for one holding more, so a batch past
/// this is stated with the framework's own count summary whatever this service allows. The
/// other bound on the same rendering is its SIZE, which this service states separately
/// ([`MAX_QUOTED_BATCH`]), so a batch of this many very large events is the case that bound
/// answers.
const LARGEST_RENDERED_BATCH: usize = 256;

/// The member POSITIONS the echo-collecting walk may visit for one request of this
/// service, stated from this model rather than taken from that walk's own default.
///
/// A collection is quoted in a clause only when every one of its elements was walked, and
/// the clause this service states about the `auditEvents` member quotes the batch the
/// request carried — at EVERY size that breaks the member's `@length` maximum (100:
/// `smithy_query(query="shape", shape="AuditEvents")`), not only at the first size past it,
/// since a caller chooses how far past the bound its batch runs. So the ceiling covers
/// every batch that can be rendered at all: [`LARGEST_RENDERED_BATCH`] elements at
/// [`ECHO_POSITIONS_PER_EVENT`] positions each, plus the request's own members. Past this
/// the framework's own count summary stands, which is the fail-safe direction: a batch
/// larger than any the walk renders is not quoted at all rather than quoted short.
const MAX_ECHO_POSITIONS: usize =
    ECHO_POSITIONS_PER_EVENT * LARGEST_RENDERED_BATCH + REQUEST_MEMBERS;

/// The largest batch rendering one clause of this service quotes.
///
/// A constraint clause of this service quotes the value the request carried, and for the
/// `auditEvents` member that value is the batch itself — so a ceiling able to quote the
/// refusal of the largest batch this operation documents has to sit above that batch's own
/// size. `PutAuditEvents` documents "You can add up to 100 of these events (or up to 1 MB)
/// per PutAuditEvents request" (`smithy_query(query="doc", shape="PutAuditEvents")`), so
/// the batch a clause has to quote is bounded by that 1 MB plus the punctuation and member
/// names the rendering writes around it. Twice the documented maximum, so a batch a little
/// past it is still quoted whole while one clause can never carry request-sized content: a
/// MODEL constant rather than a request-sized one. Past it the framework's own count
/// summary stands, which is the fail-safe direction.
const MAX_QUOTED_BATCH: usize = 2 * 1024 * 1024;

/// One modelled operation, served so that a refusal found before its handler runs is
/// stated the way this service states one.
pub(super) struct StatingSubmittedValues<Op> {
    operation: Op,
}

impl<Op> StatingSubmittedValues<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub(super) fn new(operation: Op) -> Self {
        Self { operation }
    }
}

impl<Op> ErasedOperation for StatingSubmittedValues<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> smithy_server_runtime::BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let collected: Vec<(Violation, SubmittedEcho)> =
                ::service_common::validation::refusing_violations_with_declared_echoes_rendering_up_to::<Op>(
                    svc.definition().protocol(),
                    req,
                    MAX_ECHO_POSITIONS,
                    MAX_QUOTED_BATCH,
                );
            if let Some(refused) = refusal_of(&collected) {
                let violations: Vec<Violation> =
                    collected.into_iter().map(|(violation, _)| violation).collect();
                return svc.definition().refusal_response(
                    aws_smithy_schema_server::ProtocolError::Invalid(violations),
                    Some(refused),
                    req,
                );
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

/// How this service answers the violations `collected` from one request, or `None` when
/// there are none and the request is the operation's to serve.
///
/// A channel reference comes FIRST, whatever else the request carried: this service
/// resolves every call against the channel the request names before it reads the payload,
/// so a value that is not a channel reference is answered under the error its model
/// declares for that fault rather than as one constraint violation among several (see
/// [`super::refuses_channel_arn`]).
///
/// Everything else is the constraint envelope AWS states on this service's JSON protocol,
/// with the two renderings it differs from the framework in — a `@pattern` requirement
/// quoting the expression without the whole-value anchors the model wraps it in, and a
/// collection's `@length` clause quoting the collection the request carried.
fn refusal_of(collected: &[(Violation, SubmittedEcho)]) -> Option<Refused> {
    if collected.is_empty() {
        return None;
    }
    if collected
        .iter()
        .any(|(violation, _)| refuses_channel_arn(violation))
    {
        return Some(Refused {
            code: INVALID_CHANNEL_ARN_CODE.to_string(),
            message: INVALID_CHANNEL_ARN_MESSAGE.to_string(),
        });
    }
    let stated: Vec<Violation> = collected
        .iter()
        .map(|(violation, echo)| stated_clause(violation.clone(), echo.clone()))
        .collect();
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: aws_smithy_schema_server::constraints::envelope(&stated)?,
    })
}

/// One violation as this service states it: a `@pattern` requirement quoting the
/// expression unanchored, and a COLLECTION member's `@length` clause quoting the
/// collection the request carried (`validation_message::quoting_the_submitted_collection`,
/// the tree's one home for that restoring rule). Every other clause is the framework's,
/// untouched.
fn stated_clause(violation: Violation, echo: SubmittedEcho) -> Violation {
    let quoted = quoting_the_submitted_collection(violation, echo);
    match &quoted.kind {
        ::smithy_server_runtime::ViolationKind::Pattern(pattern) => Violation {
            kind: ::smithy_server_runtime::ViolationKind::Pattern(
                ::service_common::validation::unanchored(pattern),
            ),
            ..quoted
        },
        _ => quoted,
    }
}
