// SPDX-License-Identifier: CC0-1.0

//! The API version this Query front door serves, and the refusal for a request whose
//! envelope names another one.
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
//! request for an action this model declares is served whatever version it asks for —
//! while AWS resolves the PAIR and refuses a pair it cannot resolve with `400`
//! `InvalidAction`, naming the action and the version it could not find it for. That
//! matters here more than anywhere else: TEN action names are declared by BOTH models
//! (`CreateLoadBalancer`, `DescribeLoadBalancers`, `DeleteLoadBalancer`,
//! `DescribeLoadBalancerAttributes`, `ModifyLoadBalancerAttributes`, `AddTags`,
//! `RemoveTags`, `DescribeTags`, `DescribeAccountLimits`, `SetSecurityGroups`) with
//! different inputs and outputs, so serving a `2015-12-01` envelope out of this model
//! would answer the wrong API's document to a client that asked for the other one.
//!
//! [`UnsupportedApiVersionOperation`] therefore refuses an envelope naming any version
//! this model does not declare, and the ELBv2 model is reached under its OWN routing key
//! (`elasticloadbalancingv2`, see that crate's `src/service.rs`). Serving BOTH versions
//! through this one front door — the way AWS does — needs the two models co-located, and
//! belongs to the ELBv2 service's own issue rather than to this one.
//!
//! Only the version half of the pair lives here: an `Action` that names no modelled
//! operation is the framework's own routing failure, rendered by the generated
//! definition, and every served request goes to the operation the model's routing table
//! names.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The API version THIS model declares, from `smithy_query(query="service_info")` on this
/// crate's model: `version: 2012-06-01`. The only one a request's `Version` field may
/// name here.
pub const CLASSIC_API_VERSION: &str = "2012-06-01";

/// The API version the ELBv2 model declares, from the same query on
/// `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10`:
/// `version: 2015-12-01`. Named here only to document what this front door does NOT
/// serve; a request for it is refused by [`UnsupportedApiVersionOperation`] like any
/// other version this model does not declare.
pub const V2_API_VERSION: &str = "2015-12-01";

/// The Query envelope's `Version` field, or `None` when the body carries none.
pub fn requested_version(req: &RtRequest) -> Option<String> {
    let body = req.body().bytes()?;
    aws_smithy_schema_server::parse_form_urlencoded(body)
        .into_iter()
        .find(|(name, _)| name == "Version")
        .map(|(_, value)| value)
}

/// Whether this service serves the version the request's envelope names.
///
/// A request that names NO version is served: the version is the client's statement of
/// which API it compiled against, and with none stated there is nothing to disagree with
/// — the action alone then decides, exactly as the framework's routing key does. A
/// request that names THIS version is served. Anything else is refused by
/// [`UnsupportedApiVersionOperation`], including the ELBv2 version
/// ([`V2_API_VERSION`]), whose actions this model's 2012-06-01 shapes cannot answer.
pub fn serves_requested_version(req: &RtRequest) -> bool {
    requested_version(req).is_none_or(|version| version == CLASSIC_API_VERSION)
}

/// Refuse a Query request whose `Version` names an API version this front door does not
/// serve, with the `400` `InvalidAction` envelope AWS answers an action/version pair it
/// cannot resolve with.
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
        Box::pin(async move { unsupported_version_response(req) })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` defers to the modelled operation the request's routing key names, so
        // this refusal is reached under exactly the auth the model declares for the
        // action asked for, and cannot weaken it.
        None
    }
}

/// The `400` `InvalidAction` `<ErrorResponse>` for an action asked for at a version this
/// front door does not serve.
///
/// The message is built from the request — the action and the version its envelope named
/// — because that is the only thing a caller can act on; the status and the code are
/// AWS's for this refusal. The envelope itself is the framework's own `awsQuery` error
/// document (`aws_smithy_schema_server::query_error_body`), so a routing refusal raised
/// here and one raised by the generated definition cannot answer in different shapes.
fn unsupported_version_response(req: &RtRequest) -> RtResponse {
    let action = req
        .body()
        .bytes()
        .and_then(aws_smithy_schema_server::extract_action)
        .unwrap_or_default();
    let version = requested_version(req).unwrap_or_default();
    let body = aws_smithy_schema_server::query_error_body(
        false,
        "InvalidAction",
        &format!("Could not find operation {action} for version {version}"),
    );
    let mut resp = RtResponse::new(
        StatusCode::try_from(400).expect("400 is a valid status"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", "text/xml");
    resp
}
