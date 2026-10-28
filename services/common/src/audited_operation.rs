// SPDX-License-Identifier: CC0-1.0

//! Serving an operation AUDITED: one wrapper that records each call a service serves
//! through it as a management event, so CloudTrail's `LookupEvents` reports it.
//!
//! # Why a wrapper and not per-handler code
//!
//! Every control-plane API call of an account is a management event, which makes the
//! recording a property of SERVING a call rather than of any one operation: an operation
//! that records its own call records the same six facts as its neighbour, and an operation
//! nobody remembered to edit records nothing at all. So the recording lives here, once,
//! and a service serves its modelled operations through it
//! ([`AuditedTable`], handed back from `Service::override_operation`).
//!
//! The wrapper sits at the TYPED seam ([`Operation`]), not at the wire: it is handed the
//! deserialized input before the handler runs and the handler's own `Result` after, which
//! is what lets one rule hold for every operation —
//!
//! * `eventName` is the operation's modelled name ([`Operation::NAME`]);
//! * `requestParameters` / `responseElements` are the built input and output rendered
//!   against their own schemas ([`crate::management_events::recorded_shape`]), so a
//!   sensitive member is replaced by the redaction literal rather than stored;
//! * `errorCode` / `errorMessage` come from the error channel the caller was answered
//!   through ([`crate::management_events::recorded_failure`]), so a failed call records
//!   the fault SHAPE's name.
//!
//! Nothing else about the request changes: the wrapper runs the framework's own
//! deserialize -> validate -> handle -> serialize sequence for the operation it wraps
//! ([`smithy_server_runtime::run_operation_http`]), forwards the operation's modelled auth
//! schemes, and leaves every status, code and message to the framework.
//!
//! # What is NOT recorded
//!
//! A call the scaffold refused as not implemented ([`NOT_IMPLEMENTED_SHAPE_NAME`]) was
//! never served, so no event is recorded for it: event history is the record of the calls
//! an account's APIs actually served, and an emulator gap is not one of them.
//!
//! Neither is a request the FRAMEWORK refused before any handler ran — a body that does
//! not deserialize, a member outside a modelled constraint, a pre-dispatch
//! authentication refusal. Those are answered without the typed call this wrapper hangs
//! off ever happening, so the input and the error channel a record is built from do not
//! exist; a reader should expect event history here to hold the calls that REACHED an
//! operation. Recording them too needs a seam that can name the operation and render its
//! fault from the framework's own pre-dispatch failure, which this wrapper is not.
//!
//! Recording is BEST EFFORT, as every other recorder in this tree: a service whose log is
//! unreachable (a unit test over a bare engine) still serves the call, because failing a
//! caller because the audit log is unreachable would be a worse answer than serving it
//! unrecorded.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, OperationError,
    OperationRoutingTable, PreDispatchProtocol, Service,
};

use crate::management_events::{recorded_failure, recorded_shape, ServedApiCall};

/// The wire error code of the scaffold's "not implemented" signal, which every generated
/// service's error channel renders a stub with (`crate::scaffolded` in each service
/// crate). A call it refused reached no implementation, so it is not an event.
pub const NOT_IMPLEMENTED_SHAPE_NAME: &str = "NotImplemented";

/// The HTTP status that signal renders: a 501, deliberately not a retryable 5xx. Both it
/// and the name above must match before a failure is read as the scaffold's, so a modelled
/// error a service's own model happens to name `NotImplemented` — answered with that
/// model's own status by a handler that DID serve the call — is still recorded.
pub const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// What the serving SERVICE contributes to each record: the facts that are the same for
/// every call it serves, plus the per-call ones it alone can resolve.
///
/// Implemented once per service (on a value holding that service's engine handle), so the
/// wrapper names no engine type and no service state.
pub trait AuditedService: Send + Sync + 'static {
    /// Record one served call in the account's management-event log, or drop it when this
    /// engine carries no log.
    fn record(&self, call: ServedApiCall);

    /// The `eventSource` every record of this service carries: its endpoint prefix in
    /// CloudTrail's `<prefix>.amazonaws.com` form.
    fn event_source(&self) -> &'static str;

    /// The Region the call was served in, read off the request the way this service reads
    /// it everywhere else.
    fn region(&self, ctx: &OperationContext<'_>) -> String;

    /// The account the call was served for.
    fn account_id(&self) -> String;

    /// The resources the call acted on, from the parameters it carried — the pairs
    /// CloudTrail reports in an event's `Resources`, which a lookup by `ResourceName` or
    /// `ResourceType` filters on. The default is none: a service that can name the
    /// resource a call addressed overrides this.
    fn resources(
        &self,
        request_parameters: &serde_json::Value,
        region: &str,
    ) -> Vec<crate::management_events::AuditedResource> {
        let _ = (request_parameters, region);
        Vec::new()
    }

    /// The members this service's model declares `@sensitive`. A value held at one of
    /// them is replaced by the redaction literal and never stored.
    fn sensitive_members(&self) -> &'static [&'static str];
}

/// Whether a call only REPORTED state, as CloudTrail stamps an event's `readOnly` — the
/// one fact about a call that the operation's own semantics decide, so it is stated per
/// operation where the table is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// The call reports state and changes none of it; `readOnly` is true and, as
    /// CloudTrail does for a read, the record carries no `responseElements`.
    Reporting,
    /// The call changes the account's resources; `readOnly` is false and the record
    /// carries what the response answered with.
    Mutating,
}

impl CallKind {
    /// The event's `readOnly`.
    fn read_only(self) -> bool {
        matches!(self, Self::Reporting)
    }
}

/// One modelled operation served audited.
///
/// Built by [`AuditedTable::serve`]; there is no reason to name this type elsewhere.
pub struct AuditedOperation<O, S> {
    /// The operation that serves the call.
    operation: O,
    /// The service that records it.
    service: Arc<S>,
    /// What this operation's own semantics make of the call.
    kind: CallKind,
    /// The protocol envelope the framework renders a pre-dispatch failure of this
    /// operation in — the same value the generated shim passes.
    pre_dispatch: PreDispatchProtocol<'static>,
}

impl<O: Operation, S: AuditedService> Operation for AuditedOperation<O, S> {
    type Input = O::Input;
    type Output = O::Output;
    type Error = O::Error;

    const NAME: &'static str = O::NAME;
    const INPUT_SCHEMA: &'static aws_smithy_schema::Schema = O::INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static aws_smithy_schema::Schema = O::OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = O::SUCCESS_STATUS;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is rendered BEFORE the handler runs, because the handler consumes it;
        // the document is what the request carried, which is what a record reports.
        let request_parameters = recorded_shape(
            &input,
            O::INPUT_SCHEMA,
            self.service.sensitive_members(),
        );
        let outcome = self.operation.handle(input, ctx).await;
        self.record(&request_parameters, ctx, &outcome);
        outcome
    }
}

impl<O: Operation, S: AuditedService> AuditedOperation<O, S> {
    /// Record the call just served, unless it was refused as not implemented (so nothing
    /// was served at all).
    fn record(
        &self,
        request_parameters: &serde_json::Value,
        ctx: &OperationContext<'_>,
        outcome: &Result<O::Output, O::Error>,
    ) {
        if outcome.as_ref().err().is_some_and(|error| {
            error.shape_name() == NOT_IMPLEMENTED_SHAPE_NAME
                && error.http_status() == NOT_IMPLEMENTED_STATUS
        }) {
            return;
        }
        let region = self.service.region(ctx);
        // "responseElements" is what the call ANSWERED with, which a reporting call has
        // nothing of: CloudTrail records a read with a null `responseElements`.
        let response_elements = match (self.kind, outcome.as_ref().ok()) {
            (CallKind::Mutating, Some(output)) => Some(recorded_shape(
                output,
                O::OUTPUT_SCHEMA,
                self.service.sensitive_members(),
            )),
            _ => None,
        };
        let (error_code, error_message) = recorded_failure(outcome);
        let caller = crate::management_events::recorded_caller(ctx.parts());
        self.service.record(ServedApiCall {
            event_name: O::NAME.to_string(),
            event_source: self.service.event_source().to_string(),
            read_only: self.kind.read_only(),
            access_key_id: caller.access_key_id,
            user_name: caller.user_name,
            principal_type: caller.principal_type.to_string(),
            // The unique id of the IAM entity a credential speaks for is IAM's to mint,
            // and a serving service resolves none, so the record leaves it out rather
            // than naming a principal nothing resolves.
            principal_id: None,
            // A call signed with an AWS credential was authenticated by this account, not
            // by an external provider.
            identity_provider: None,
            account_id: self.service.account_id(),
            user_agent: crate::management_events::recorded_user_agent(ctx.parts()),
            resources: self.service.resources(request_parameters, &region),
            aws_region: region,
            request_parameters: request_parameters.clone(),
            response_elements,
            error_code,
            error_message,
        });
    }
}

impl<O, S> ErasedOperation for AuditedOperation<O, S>
where
    O: Operation + ErasedOperation,
    S: AuditedService,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The framework's own per-request sequence, driven with THIS wrapper as the
            // operation: the handler it calls is the wrapped one, so nothing about the
            // wire changes and the recording happens inside the typed call.
            smithy_server_runtime::run_operation_http(self, svc, req, ctx, self.pre_dispatch)
                .await
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapped operation's modelled schemes: wrapping must not change which
        // credentials sign a call.
        self.operation.auth_schemes()
    }
}

/// The routing table a service serves its operations audited through: every operation
/// wrapped in [`AuditedOperation`], keyed by the routing key the protocol resolves (the
/// operation's own modelled name).
///
/// A service builds one in its `EmulatedService::new` and answers
/// `Service::override_operation` from it; [`serve`](Self::serve) takes one
/// already-constructed operation and files it under [`Operation::NAME`], so a wrapped
/// operation cannot land under a key the protocol never resolves.
pub struct AuditedTable<S> {
    table: OperationRoutingTable,
    service: Arc<S>,
    pre_dispatch: PreDispatchProtocol<'static>,
}

impl<S: AuditedService> AuditedTable<S> {
    /// An empty table recording into `service`, whose operations render a pre-dispatch
    /// failure in `pre_dispatch` — the service's own protocol envelope.
    pub fn new(service: Arc<S>, pre_dispatch: PreDispatchProtocol<'static>) -> Self {
        Self {
            table: OperationRoutingTable::new(),
            service,
            pre_dispatch,
        }
    }

    /// Serve `operation` audited, as a call of `kind`.
    pub fn serve<O: Operation + ErasedOperation>(
        &mut self,
        operation: O,
        kind: CallKind,
    ) -> &mut Self {
        self.table.add_operation(
            O::NAME,
            Box::new(AuditedOperation {
                operation,
                service: Arc::clone(&self.service),
                kind,
                pre_dispatch: self.pre_dispatch,
            }),
        );
        self
    }

    /// Serve `operation` audited as a REPORTING call ([`CallKind::Reporting`]).
    pub fn reports<O: Operation + ErasedOperation>(&mut self, operation: O) -> &mut Self {
        self.serve(operation, CallKind::Reporting)
    }

    /// Serve `operation` audited as a MUTATING call ([`CallKind::Mutating`]).
    pub fn mutates<O: Operation + ErasedOperation>(&mut self, operation: O) -> &mut Self {
        self.serve(operation, CallKind::Mutating)
    }

    /// The table, for the service to hold and route from.
    #[must_use]
    pub fn finish(self) -> OperationRoutingTable {
        self.table
    }
}
