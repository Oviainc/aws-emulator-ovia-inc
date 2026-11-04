// SPDX-License-Identifier: CC0-1.0

//! Serving a request that OMITS a member this service's model requires.
//!
//! A REST route is more than a method and a URI template: it also carries the `@required`
//! header and query members of the operation bound to it, and the generated routing table
//! DECLINES a request that is missing one. That is how two operations sharing a method
//! and a URI template are told apart — S3's object writes are the case it exists for.
//!
//! This service's model has no such pair: each of its six operations binds a distinct
//! (method, URI). So a request that matches one of them names that operation whatever
//! headers it carries, and a member the model requires and the request omitted is that
//! operation's own INPUT error — HTTP 400 with this service's modelled
//! `ValidationException`, carrying the `Message` and the `Reason` a client reads. Left to
//! the table, such a request resolves to no operation at all and is answered as an
//! operation this emulator does not serve (HTTP 501 `NotImplemented`), which tells a
//! caller that forgot a header that the emulator is broken rather than that its request
//! is, and makes anything branching on 4xx-versus-5xx take the retry path for a request
//! that can never succeed.
//!
//! So `crate::service` routes each operation that has such members through
//! [`RequiresMembers`], which answers the absent member itself and otherwise hands the
//! request straight to the operation. It runs BEFORE deserialization, which is the only
//! place it can run: the framework's own report for an absent required member is the
//! synthetic envelope, which carries neither of the two members a client reads here.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use crate::ops::types::ValidationException;
use crate::state::absent_required_member_message;

/// The `ValidationExceptionReason` an absent input member is reported under: the request
/// carried no value for a parameter that must have one.
const ABSENT_MEMBER_REASON: &str = "INVALID_PARAMETER_VALUE";

/// The wire error code of this service's modelled `ValidationException`, and the status
/// its `@httpError` declares
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#ValidationException")`).
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// Where in the request a member travels, which is where its absence is detected.
#[derive(Debug, Clone, Copy)]
pub enum Binding {
    /// The member's `@httpHeader` name.
    Header(&'static str),
    /// The member's `@httpQuery` name.
    Query(&'static str),
}

/// One member an operation's model requires, and how this service words its absence.
#[derive(Debug, Clone, Copy)]
pub struct RequiredMember {
    /// The Smithy member name, which is what the refusal names.
    pub member: &'static str,
    /// Where the request carries it.
    pub binding: Binding,
    /// The sentence this service answers with when the member is absent, where it has one
    /// of its own; `None` takes the shared template for an absent required member
    /// ([`absent_required_member_message`]).
    pub stated: Option<&'static str>,
}

impl RequiredMember {
    /// Whether `req` carries this member at all. Only presence is judged here: a value
    /// that IS present is judged by the boundary and by the operation, against the
    /// constraints the model declares for it.
    fn present(&self, req: &RtRequest) -> bool {
        match self.binding {
            Binding::Header(name) => req.headers().get(name).is_some(),
            Binding::Query(name) => query_keys(req.uri()).any(|key| key == name),
        }
    }

    /// The `Message` the refusal carries.
    fn message(&self) -> String {
        self.stated.map_or_else(
            || absent_required_member_message(self.member),
            str::to_string,
        )
    }
}

/// The keys of a request URI's query string, in the order they appear.
fn query_keys(uri: &str) -> impl Iterator<Item = &str> {
    uri.split_once('?')
        .map_or("", |(_, query)| query)
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').map_or(pair, |(key, _)| key))
}

/// An operation served only once the request carries the members its model requires.
///
/// Holds the operation itself, so the request it admits runs the very same handler the
/// generated routing table would have run.
pub struct RequiresMembers<Op> {
    operation: Op,
    required: &'static [RequiredMember],
}

impl<Op> RequiresMembers<Op> {
    /// Guard `operation` on `required`, in the order the members are listed: the FIRST
    /// absent member is the one reported, so an operation whose members are not
    /// independent (a payload's checksum before the algorithm that computed it) lists
    /// them in the order it judges them.
    pub const fn new(operation: Op, required: &'static [RequiredMember]) -> Self {
        Self {
            operation,
            required,
        }
    }
}

impl<Op: ErasedOperation> ErasedOperation for RequiresMembers<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        match self.required.iter().find(|member| !member.present(req)) {
            Some(absent) => Box::pin(async move { absent_member_response(svc, req, absent) }),
            None => self.operation.run(svc, req, ctx),
        }
    }

    /// The guarded operation's own schemes: guarding changes which requests reach the
    /// handler, never how a request is authenticated.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The refusal for a request that omitted `absent`: this service's modelled
/// `ValidationException`, rendered through the protocol the request is served in, so the
/// body and the `x-amzn-errortype` header are the ones every other refusal of this
/// service carries.
fn absent_member_response(
    svc: &dyn Service,
    req: &RtRequest,
    absent: &RequiredMember,
) -> RtResponse {
    let error = ValidationException {
        message: Some(absent.message()),
        reason: Some(ABSENT_MEMBER_REASON.to_string()),
    };
    let definition = svc.definition();
    definition
        .wire_protocol(req)
        .protocol()
        .serialize_error(
            &error,
            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_CODE,
            VALIDATION_EXCEPTION_STATUS,
        )
        // The refusal could not be rendered in this request's protocol, which is a
        // framework-level failure and is reported as one rather than as a snapshot error.
        .unwrap_or_else(|failure| definition.framework_error_response(failure, req))
}
