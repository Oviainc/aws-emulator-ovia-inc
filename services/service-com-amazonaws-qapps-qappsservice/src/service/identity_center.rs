// SPDX-License-Identifier: CC0-1.0

//! This service's FRONT DOOR: the IAM Identity Center identity a request has to
//! carry before anything else about it is looked at.
//!
//! # The rule
//!
//! Amazon Q Apps serves a Q Business web-experience user, so a request is
//! authorized by the IAM Identity Center identity its credentials carry, not by the
//! access key alone. That identity is evaluated AHEAD of everything else — the
//! existence of the resource the request names, the shape of its input, and whether
//! the operation is implemented at all — and a caller presenting no such identity is
//! refused `UnauthorizedException` (HTTP 401, `com.amazonaws.qapps#UnauthorizedException`,
//! with the required `message`). Nothing behind the door runs for it: a refused
//! request reads no state, validates no member and reaches no handler, so it cannot
//! be told whether a resource exists, which constraint its payload broke, or whether
//! the operation it named is implemented.
//!
//! Which operations the door covers is the MODEL's answer, not a list chosen here:
//! every operation whose errors include `UnauthorizedException` is behind it, which
//! is every operation of this service except the three of the AWS-standard
//! resource-tagging surface ([`IAM_AUTHORIZED_OPERATIONS`]). Those three declare no
//! 401 — they authorize through the caller's IAM policy against the ARN they name,
//! which is [`super::authorization`]'s decision and is taken in their handlers,
//! where the ARN the request carries has been read.
//!
//! # Who carries such an identity
//!
//! The emulator's own Identity Center portal issues role credentials to a signed-in
//! user ([`::service_com_amazonaws_sso_swbportalservice`]), and that record — the
//! credential, and the sign-in session it was issued to — is the one place an
//! Identity Center identity exists in this emulator. So a request is federated
//! exactly when the access key id it presents is such a credential, still usable at
//! the instant it is asked: a long-term IAM access key, a session another service
//! minted and a credential past its own duration all carry no Identity Center
//! identity and are refused. A credential whose sign-in was logged out still carries
//! the identity it was issued to, because such a credential goes on working for the
//! rest of its own duration.
//!
//! The lookup is read-only and goes through the portal's own accessor
//! (`signed_in_subject`), so this service never second-guesses what a sign-in means;
//! and it is asked per request, so a credential that lapses stops being served
//! immediately.
//!
//! # Why the door is here and not in each handler
//!
//! The framework refuses a modelled constraint and dispatches a scaffolded operation
//! BEFORE any handler body runs, so a gate written per handler could not be ahead of
//! either. [`FrontDoor`] is an [`ErasedOperation`] the service serves every modelled
//! operation through (its `override_operation` seam, the same seam
//! [`::service_common::front_door`] uses for the body-validation door other services
//! put in front of their application code): it decides first, and on a decision to
//! serve it hands the request to the operation the model's own routing table names,
//! which owns deserialize -> validate -> handle -> serialize exactly as before.

use std::sync::Arc;
use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::service_com_amazonaws_sso_swbportalservice::state as identity_center_portal;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationError, Refused,
    Service,
};

use crate::ops::types::UnauthorizedException;

/// The message this service refuses an unfederated caller with.
///
/// One gate, one refusal: it is the same sentence whatever operation was addressed and
/// whatever the request carried, so nothing about the request leaks into it. The
/// model types `UnauthorizedException$message` as a required `String` without stating
/// a text; this is the one Q Apps is recorded answering a caller with no Identity
/// Center identity.
pub const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The operations that are NOT behind this door: the AWS-standard resource-tagging
/// surface, whose three operations are the only ones of this service whose model
/// declares no `UnauthorizedException`
/// (`smithy_query(query="digest")` — `ListTagsForResource`, `TagResource` and
/// `UntagResource` list `ResourceNotFoundException`, `ConflictException` and the
/// service-common errors, which include `AccessDeniedException`, and no 401).
///
/// They authorize through plain IAM against the ARN they name instead
/// ([`super::authorization`]). A name here that the model does not declare, or a
/// 401-less operation missing from here, would put a request through the wrong door —
/// which `tests/all/identity_center_front_door.rs` pins against the model's own error
/// lists.
pub const IAM_AUTHORIZED_OPERATIONS: &[&str] =
    &["ListTagsForResource", "TagResource", "UntagResource"];

/// The refusal this door answers with, as the modelled error.
///
/// Returned by value so a handler that needs the same refusal states it the same way.
pub fn unauthorized() -> UnauthorizedException {
    UnauthorizedException {
        message: UNAUTHORIZED_MESSAGE.to_string(),
    }
}

/// The Identity Center identity the request `ctx` describes carries: the subject of
/// the sign-in session the portal issued its credential to, or `None` when it carries
/// none.
///
/// The access key id is read through the ONE reader for it
/// ([`::service_common::request_access_key_id`]), so a presigned call is attributed to
/// the credential that signed the URL rather than to nobody.
pub fn federated_subject<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
) -> Option<String> {
    let access_key_id = ::service_common::request_access_key_id(ctx.parts());
    let portal = identity_center_portal::service_state(engine)?;
    // Bound rather than returned directly, so the store's guard is dropped before the
    // store it borrows from is.
    let subject = portal
        .lock()
        .signed_in_subject(&access_key_id, SystemTime::now())
        .map(str::to_string);
    subject
}

/// This service's front door, as the one [`ErasedOperation`] every modelled
/// operation is served through.
///
/// Owns the shared engine exactly as a generated operation does, because the identity
/// it resolves lives in the SAME engine the handlers read: a sign-in recorded in the
/// portal's store is in force on the next request, and one that ended stops being.
pub struct FrontDoor<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> FrontDoor<Eng> {
    /// Build the door from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// What the door decided about one request.
enum Decision<'r> {
    /// Serve it, through the operation the model's routing table names.
    Serve(&'r dyn ErasedOperation),
    /// Refuse it, with this already-rendered response. BOXED because a rendered
    /// response is much the larger of the two variants, and a `Decision` is built on
    /// every request.
    Refuse(Box<RtResponse>),
}

impl<Eng: Send + Sync + 'static> ErasedOperation for FrontDoor<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The decision is taken in one call that reads, uses and drops the service
            // definition: `&dyn ErasedServiceDefinition` is not `Sync`, so holding it
            // across the await below would make this future non-`Send`.
            match self.decide(svc, req, ctx) {
                Decision::Refuse(response) => *response,
                Decision::Serve(operation) => operation.run(svc, req, ctx).await,
            }
        })
    }

    /// The door signs nothing of its own: the schemes are the modelled operation's,
    /// which the router reads from the routing table for the request's own routing key
    /// (`None` defers to exactly that), so serving an operation behind this door
    /// cannot change which credentials sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

impl<Eng: Send + Sync + 'static> FrontDoor<Eng> {
    /// Whether to serve this request, and through what.
    fn decide<'r>(
        &self,
        svc: &'r dyn Service,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
    ) -> Decision<'r> {
        let definition = svc.definition();
        let named = definition
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| definition.routing_table().get(key).map(|op| (key, op)));
        // The request named no modelled operation. UNREACHABLE: the service serves a
        // request through this door only once its routing key names one. The
        // protocol-correct routing answer is given rather than a panic, so the door
        // stays total.
        let Some((operation_name, operation)) = named else {
            return Decision::Refuse(Box::new(definition.routing_error_response(req)));
        };
        // The tagging surface has its own front door (see the module docs): plain IAM,
        // decided in its handlers against the ARN the request names.
        if IAM_AUTHORIZED_OPERATIONS.contains(&operation_name) {
            return Decision::Serve(operation);
        }
        if federated_subject(&*self.engine, ctx).is_some() {
            return Decision::Serve(operation);
        }
        Decision::Refuse(Box::new(unauthorized_response(definition, req)))
    }
}

/// The door's refusal, rendered in this service's own protocol envelope.
///
/// The status (401) and the wire code are read off the MODELLED error rather than
/// written here, which is why the refusal is constructed through one operation's error
/// enum: `UnauthorizedException` is ONE shape with one `@httpError`, declared by every
/// operation behind this door, so any of them renders the same bytes — and the status
/// and code then cannot drift from the model. (The same construction as the IAM
/// Identity Center portal's own gate, which renders its refusal through
/// `ListAccounts`'s enum for exactly this reason.)
fn unauthorized_response(definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> RtResponse {
    let refusal =
        crate::ops::get_q_app::GetQAppErrorModeled::UnauthorizedException(unauthorized());
    let rendered = definition.protocol().serialize_error(
        refusal.as_serializable(),
        refusal.variant_schema(),
        refusal.shape_name(),
        refusal.http_status(),
    );
    match rendered {
        Ok(response) => response,
        // UNREACHABLE for a shape of this service's own model. A refusal that cannot be
        // rendered in the modelled envelope is still stated as a refusal, in the
        // framework's, rather than served.
        Err(error) => definition.refusal_response(
            error,
            Some(Refused {
                code: refusal.shape_name().to_string(),
                message: UNAUTHORIZED_MESSAGE.to_string(),
            }),
            req,
        ),
    }
}
