// SPDX-License-Identifier: CC0-1.0

//! Which API version this Query front door serves a request as, and out of WHICH of the
//! two Elastic Load Balancing models.
//!
//! # Why this service claims a name it shares
//!
//! Elastic Load Balancing publishes ONE endpoint for two APIs. Both models declare
//! `endpointPrefix` / `arnNamespace` / `aws.auth#sigv4` name `elasticloadbalancing`
//! (`smithy_query(query="service_info")` on either model), differing only in `sdkId`
//! (`Elastic Load Balancing` vs `Elastic Load Balancing v2`) and in the API version they
//! declare: [`CLASSIC_API_VERSION`] here and [`V2_API_VERSION`] in
//! `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10`.
//! Nothing in a request's headers or URI tells the two apart, and the router requires
//! EXACTLY ONE claimant, answering `500 AmbiguousService` — a code no AWS model carries,
//! in a JSON body an awsQuery client cannot parse — when two services claim one key. So
//! the shared key resolves to the ONE service whose own identity is that name: this one.
//!
//! # The version half of the routing key
//!
//! An `awsQuery` request carries TWO routing fields in its form body: `Action`, the
//! operation, and `Version`, the API version that operation is asked for. The framework's
//! routing key reads `Action` alone (`aws_smithy_schema_server::extract_action`), so a
//! request for an action a model declares would be served whatever version it asks for —
//! while AWS resolves the PAIR, serving it out of the model that declares that version
//! and refusing a pair it cannot resolve with `400` `InvalidAction`, naming the action and
//! the version it could not find it for. That matters here more than anywhere else: TEN
//! action names are declared by BOTH models (`CreateLoadBalancer`,
//! `DescribeLoadBalancers`, `DeleteLoadBalancer`, `DescribeLoadBalancerAttributes`,
//! `ModifyLoadBalancerAttributes`, `AddTags`, `RemoveTags`, `DescribeTags`,
//! `DescribeAccountLimits`, `SetSecurityGroups`) with different inputs and outputs, so
//! serving a `2015-12-01` envelope out of this model would answer the wrong API's
//! document to a client that asked for the other one.
//!
//! [`serving_model`] is therefore the whole routing rule of this shared endpoint, and it
//! keys on the version the envelope names rather than on the action:
//!
//! * [`CLASSIC_API_VERSION`], or no `Version` field at all, is served out of THIS model,
//!   through the generated routing table. A request that names no version has stated
//!   nothing to disagree with, and the front door's own API is what it then serves — the
//!   action alone deciding, exactly as the framework's routing key does.
//! * [`V2_API_VERSION`] is served out of the ELBv2 model, through [`V2FrontDoor`], which
//!   is that model's own emulated service over this emulator's one shared engine. That is
//!   how AWS resolves the two APIs on their shared endpoint too: an AWS SDK ELBv2 client
//!   signs `elasticloadbalancing` and names `Version=2015-12-01`, so this is the ONLY way
//!   its calls reach the ELBv2 operations at all.
//! * Any other version is a pair neither model can resolve, refused by
//!   [`UnsupportedApiVersionOperation`].
//!
//! An `Action` the serving model does not declare is a routing failure of THAT model —
//! `400` `InvalidAction` naming the pair — whichever of the two is serving, so a Classic
//! action asked for at the ELBv2 version and an ELBv2 action asked for at the Classic
//! version are refused alike. [`unresolved_pair_response`] is that ONE answer, reached
//! from all three paths (the ELBv2 front door, the unknown-version refusal, and this
//! service's own `override_routing_error`), so no two of them word the same refusal
//! differently.
//!
//! # What stays with the claiming service
//!
//! A request served out of the ELBv2 model is handed that model's own service as the
//! serving one, so its protocol, its envelope and its `Service::override_refusal` wording
//! are the ELBv2 model's. Two seams are reached BEFORE an operation is named and so stay
//! with the service that claimed the request, this one: `can_handle` (which is why the
//! shared key resolves here at all) and `override_framework_error`, the transport-level
//! failure the router detects up front. Both models leave that second seam at the
//! framework default, so the two cannot answer such a failure differently today; a service
//! that reworded it would have to reword it on this side.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10 as elbv2;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The API version THIS model declares, from `smithy_query(query="service_info")` on this
/// crate's model: `version: 2012-06-01`.
pub const CLASSIC_API_VERSION: &str = "2012-06-01";

/// The API version the ELBv2 model declares, from the same query on
/// `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10`:
/// `version: 2015-12-01`. An envelope naming it is served out of that model through
/// [`V2FrontDoor`].
pub const V2_API_VERSION: &str = "2015-12-01";

/// The awsQuery error code AWS answers an action/version pair it cannot resolve with.
/// Reserved for exactly that: an action the service does not have at the version asked
/// for, never a well-formed request to an action it does.
const INVALID_ACTION: &str = "InvalidAction";

/// Which of the two Elastic Load Balancing models serves a request on this shared
/// endpoint, decided by the API version its awsQuery envelope names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServingModel {
    /// This crate's `2012-06-01` model, through the generated routing table.
    Classic,
    /// The `2015-12-01` ELBv2 model, through [`V2FrontDoor`].
    V2,
    /// Neither: the envelope names a version no Elastic Load Balancing model declares.
    Unresolvable,
}

/// The Query envelope's `Version` field, or `None` when the body carries none.
///
/// The router buffers a request's body before it reaches any seam that reads one
/// (`MultiServiceRouter::dispatch` builds the runtime-API request from the buffered bytes
/// up front, which is why `Service::override_operation` is handed the whole request), so
/// `bytes()` answers the envelope here and an absent `Version` field is the only reason
/// this is `None`.
pub fn requested_version(req: &RtRequest) -> Option<String> {
    let body = req.body().bytes()?;
    aws_smithy_schema_server::parse_form_urlencoded(body)
        .into_iter()
        .find(|(name, _)| name == "Version")
        .map(|(_, value)| value)
}

/// The model that serves `req`, from the API version its envelope names. The whole rule
/// is this module's header; nothing about the ACTION is read here, because the version
/// alone names the API, and the serving model's own routing table then answers whether
/// that API has the action.
pub fn serving_model(req: &RtRequest) -> ServingModel {
    match requested_version(req).as_deref() {
        None | Some(CLASSIC_API_VERSION) => ServingModel::Classic,
        Some(V2_API_VERSION) => ServingModel::V2,
        Some(_) => ServingModel::Unresolvable,
    }
}

/// The ELBv2 half of this shared endpoint: the ELBv2 model's own emulated service, served
/// for an envelope naming [`V2_API_VERSION`].
///
/// Held as a field on the Classic service and handed back from
/// `Service::override_operation`, which is the one seam that sees the request body before
/// the model's routing table resolves it — and the `Version` field it decides on lives in
/// that body.
///
/// It is built over the SAME shared engine, so the ELBv2 operations reached through here
/// and the ones reached under the ELBv2 model's own routing key read and write the ONE
/// ELBv2 store: a target group created through this front door is read back by either.
pub struct V2FrontDoor<Eng: Send + Sync + 'static> {
    v2: elbv2::EmulatedService<Eng>,
}

impl<Eng: Send + Sync + 'static> V2FrontDoor<Eng> {
    /// Build the ELBv2 service this front door serves, from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            v2: elbv2::EmulatedService::new(engine),
        }
    }

    /// The ELBv2 operation the request's `Action` names, or `None` when the ELBv2 model
    /// declares no such action.
    ///
    /// The framework's own lookup, in the order and with the data the router uses
    /// (`MultiServiceRouter::dispatch` STEP 2): the ELBv2 service's own
    /// `override_operation` seam first, then its wire protocol reads the routing key off
    /// the request and its model-derived routing table must hold an operation under it. So
    /// the action table consulted here is the ELBv2 model's own, never a list restated
    /// beside it, and a routing decision that model makes for itself is made here too.
    fn v2_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        if let Some(op) = self.v2.override_operation(req) {
            return Some(op);
        }
        let definition = self.v2.definition();
        let key = definition.protocol().routing_key(req).ok()?;
        definition.routing_table().get(key)
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for V2FrontDoor<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match self.v2_operation(req) {
                // The ELBv2 service — not the one that claimed the request — is handed
                // back as the serving service, which is what the framework's centralized
                // sequence reads the wire protocol and the refusal wording off. So the
                // answer carries the ELBv2 model's own awsQuery envelope, down to the
                // `@xmlNamespace` of its `2015-12-01` document, exactly as it does under
                // that model's own routing key.
                Some(op) => op.run(&self.v2, req, ctx).await,
                // The version resolved to the ELBv2 API and that API has no such action:
                // the same unresolvable action/version pair, worded the same way.
                None => unresolved_pair_response(req, V2_API_VERSION),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is the author-written operation's answer: the router asks the CLAIMING
        // service's modelled routing table first (`MultiServiceRouter::authenticate`), so
        // for an action both models declare the schemes are the Classic operation's, and
        // for an ELBv2-only action no modelled schemes are found and the request is
        // verified by whichever registered authenticator recognizes it. Neither outcome
        // can serve an ELBv2 operation unsigned, and neither widens what signs it: the
        // ELBv2 model binds no `@auth` or `@optionalAuth` override on any operation, so
        // every one of them declares exactly the service's `aws.auth#sigv4` — the same
        // scheme the Classic model declares — and a request no authenticator recognizes
        // is still refused. Returning a list from here could only restate that one
        // scheme, and would restate it beside the model instead of reading it.
        None
    }
}

/// Refuse a Query request whose `Version` names an API version NEITHER Elastic Load
/// Balancing model declares, with the `400` `InvalidAction` envelope AWS answers an
/// action/version pair it cannot resolve with.
///
/// Held as a field on the service and handed back from `Service::override_operation`,
/// which is the one seam that sees the request body before the model's routing table
/// resolves it.
pub struct UnsupportedApiVersionOperation;

impl ErasedOperation for UnsupportedApiVersionOperation {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The envelope named a version, or this operation would not have been
            // reached, so the fallback below is never the one reported here.
            unresolved_pair_response(req, CLASSIC_API_VERSION)
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` defers to the modelled operation the request's routing key names, so
        // this refusal is reached under exactly the auth the model declares for the
        // action asked for, and cannot weaken it.
        None
    }
}

/// The `400` `InvalidAction` `<ErrorResponse>` for an action/version pair no Elastic Load
/// Balancing model resolves — an action at a version neither model declares, or an action
/// the model that declares the named version does not have.
///
/// The message is built from the request — the action and the version its envelope named
/// — because that is the only thing a caller can act on; the status and the code are
/// AWS's for this refusal. `served_as` is the version named when the envelope carries
/// none: a request that states no version is served as the model's own API version (see
/// [`serving_model`]), so that is the version it was not found for. The envelope itself is
/// the framework's own `awsQuery` error document
/// (`aws_smithy_schema_server::query_error_body`), so a routing refusal raised here and
/// one raised by either generated definition cannot answer in different shapes.
pub fn unresolved_pair_response(req: &RtRequest, served_as: &str) -> RtResponse {
    let action = req
        .body()
        .bytes()
        .and_then(aws_smithy_schema_server::extract_action)
        .unwrap_or_default();
    let version = requested_version(req).unwrap_or_else(|| served_as.to_string());
    let body = aws_smithy_schema_server::query_error_body(
        false,
        INVALID_ACTION,
        &format!("Could not find operation {action} for version {version}"),
    );
    let mut resp = RtResponse::new(
        StatusCode::try_from(400).expect("400 is a valid status"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", "text/xml");
    resp
}
