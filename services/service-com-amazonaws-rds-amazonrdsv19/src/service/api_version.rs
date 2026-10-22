// SPDX-License-Identifier: CC0-1.0

//! The API version this Query front door serves, and the refusal for a request whose
//! envelope names another one.
//!
//! An `awsQuery` request carries TWO routing fields in its form body: `Action`, the
//! operation, and `Version`, the API version that operation is asked for. The framework's
//! routing key reads `Action` alone (`aws_smithy_schema_server::extract_action`), so a
//! request for an action this service models is served whatever version it asks for —
//! while AWS resolves the PAIR and refuses a pair it cannot resolve with `400`
//! `InvalidAction`, naming the action and the version it could not find it for.
//!
//! [`API_VERSION`] is this service's one version, from
//! `smithy_query(query="service_info",
//! model_path="services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json")`
//! (`version: 2014-10-31`) — the same version every SDK built from this model sends, and
//! the only one an emulator built from it can serve.
//!
//! Only the version half of the pair lives here: an `Action` that names no modelled
//! operation is the framework's own routing failure, rendered by the generated
//! definition, and every served request goes to the operation the model's routing table
//! names.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The one API version this service's model declares, and so the only value a request's
/// `Version` field may name. From `smithy_query(query="service_info")` on this crate's
/// model: `version: 2014-10-31`.
pub const API_VERSION: &str = "2014-10-31";

/// The Query envelope's `Version` field, or `None` when the body carries none.
fn requested_version(req: &RtRequest) -> Option<String> {
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
/// [`UnsupportedApiVersionOperation`].
pub fn serves_requested_version(req: &RtRequest) -> bool {
    requested_version(req).is_none_or(|version| version == API_VERSION)
}

/// Refuse a Query request whose `Version` names an API version this service does not
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
/// service does not serve.
///
/// The message is built from the request — the action and the version its envelope named
/// — because that is the only thing a caller can act on; the status and the code are what
/// a client branches on, and they are AWS's for this refusal. The envelope itself is the
/// framework's own `awsQuery` error document
/// (`aws_smithy_schema_server::query_error_body`), so a routing refusal raised here and
/// one raised by the generated definition cannot answer in different shapes.
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
