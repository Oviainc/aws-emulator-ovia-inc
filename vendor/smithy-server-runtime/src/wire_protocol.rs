// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Per-request wire-protocol selection.
//!
//! A Smithy service can declare several protocol traits side by side, for
//! example `awsJson1_0`, `rpcv2Cbor` and `awsQuery`, and a client may speak any
//! of them. The router selects such a service by its signing name, which is the
//! same for every protocol, so the service has to answer each request in the
//! protocol that request arrived in.
//!
//! The generated `ServiceDefinition` holds one [`DeclaredProtocol`] for every
//! protocol the model declares, in a [`DeclaredProtocols`] list. No entry is the
//! default. [`DeclaredProtocols::select`] picks the entry for one request:
//!
//! - A list of one serves every request in that protocol. REST services are
//!   always this case: a REST request carries no protocol marker.
//! - A list of several serves the protocol [`WireProtocolKind::detect`] reads from
//!   the request's headers and path.
//! - A request that names none of the service's protocols is refused with a
//!   `404` that carries no `content-type` and
//!   [`UNRECOGNISED_PROTOCOL_BODY`](crate::UNRECOGNISED_PROTOCOL_BODY) as its
//!   body. The router refuses it before it reads the body or looks for an
//!   operation. The service's
//!   [`override_routing_error`](crate::Service::override_routing_error) may word
//!   that refusal; its [`override_operation`](crate::Service::override_operation)
//!   is not asked, because the request is never served.
//!
//! The returned [`WireProtocol`] is what
//! [`ErasedServiceDefinition::wire_protocol`](crate::ErasedServiceDefinition::wire_protocol)
//! hands back, and every step of the request uses it: operation lookup,
//! authentication, decode, encode, and every error response.

use crate::{PreDispatchProtocol, Refused, ServerHttpProtocol};
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};

/// A wire-protocol family a request can be recognised as, from its headers and
/// path alone.
///
/// A family is one request marker. The members of a family (`awsJson1_0` and
/// `awsJson1_1`; `awsQuery` and `ec2Query`) send the same marker, so a service
/// cannot declare two of them. The REST protocols (`restJson1`, `restXml`) are
/// not listed: they route by a static method + URI table and carry no protocol
/// marker, so a request cannot be recognised as one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireProtocolKind {
    /// `awsJson1_0` or `awsJson1_1`.
    AwsJson,
    /// Smithy `rpcv2Cbor`.
    RpcV2Cbor,
    /// `awsQuery` or `ec2Query`.
    Query,
}

impl WireProtocolKind {
    /// The protocol family the request names, or `None` when it names none. The
    /// rules, first match wins:
    ///
    /// 1. a `smithy-protocol: rpc-v2-cbor` header, or a
    ///    `/service/{Service}/operation/{Operation}` path, is
    ///    [`RpcV2Cbor`](Self::RpcV2Cbor), because the rpcv2Cbor specification
    ///    requires the header and routes by the path;
    /// 2. an `x-amz-target` header, or a `content-type` of `application/x-amz-json-*`,
    ///    is [`AwsJson`](Self::AwsJson);
    /// 3. a `content-type` of `application/x-www-form-urlencoded` is
    ///    [`Query`](Self::Query).
    ///
    /// Only the request line and headers are read, never the body, so detection
    /// costs nothing for a large request and cannot disagree with a later body
    /// read.
    pub fn detect(request: &RtRequest) -> Option<Self> {
        let headers = request.headers();
        if headers
            .get("smithy-protocol")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("rpc-v2-cbor"))
            || is_rpcv2_cbor_path(request.uri())
        {
            return Some(Self::RpcV2Cbor);
        }
        let content_type = headers
            .get("content-type")
            .map(|v| v.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if headers.get("x-amz-target").is_some()
            || content_type.starts_with("application/x-amz-json")
        {
            return Some(Self::AwsJson);
        }
        if content_type.starts_with("application/x-www-form-urlencoded") {
            return Some(Self::Query);
        }
        None
    }
}

/// Whether `uri` has the rpcv2Cbor shape `/service/{Service}/operation/{Operation}`.
///
/// The shape is matched at the end of the path, not at its root, so a path with a
/// prefix before `/service/` matches. That is the same reading the codec's
/// `aws_smithy_schema_server::rpc::routing_key_from_path` applies, so every path
/// detected as rpcv2Cbor is one the codec can route.
fn is_rpcv2_cbor_path(uri: &str) -> bool {
    let path = uri.split('?').next().unwrap_or(uri);
    let Some((service_path, operation)) = path.rsplit_once("/operation/") else {
        return false;
    };
    let operation = operation.trim_end_matches('/');
    let names_one_service = service_path
        .rsplit_once("/service/")
        .is_some_and(|(_, service)| !service.is_empty() && !service.contains('/'));
    names_one_service && !operation.is_empty() && !operation.contains('/')
}

/// One protocol a service declares: the codec that decodes and encodes its
/// requests, the error envelope its framework-level failures render in, and the
/// marker a request names it by.
pub struct DeclaredProtocol {
    marker: Option<WireProtocolKind>,
    protocol: Box<dyn ServerHttpProtocol>,
    envelope: PreDispatchProtocol<'static>,
}

impl DeclaredProtocol {
    /// `marker` is `None` for a protocol whose requests carry none (REST), which
    /// can therefore only be a service's only protocol.
    pub fn new(
        marker: Option<WireProtocolKind>,
        protocol: Box<dyn ServerHttpProtocol>,
        envelope: PreDispatchProtocol<'static>,
    ) -> Self {
        Self {
            marker,
            protocol,
            envelope,
        }
    }

    fn serving(&self) -> WireProtocol<'_> {
        WireProtocol::new(self.protocol.as_ref(), self.envelope)
    }
}

/// Every protocol a service declares. See the [module docs](self) for how one is
/// selected per request.
pub struct DeclaredProtocols {
    declared: Vec<DeclaredProtocol>,
}

impl DeclaredProtocols {
    /// The service's protocols. The generated code builds the list from the
    /// model's protocol traits and refuses, at compile time, a model whose
    /// protocols share a marker or that mixes a REST protocol with another.
    pub fn new(declared: Vec<DeclaredProtocol>) -> Self {
        Self { declared }
    }

    /// The codec of the first protocol in the list, whatever the request. It backs
    /// the legacy
    /// [`ErasedServiceDefinition::protocol`](crate::ErasedServiceDefinition::protocol)
    /// accessor and nothing else; [`select`](Self::select) is the protocol a request
    /// is served in.
    ///
    /// An empty list recognises no request, so its first codec is the one
    /// [`WireProtocol::unrecognised`] carries, which refuses every call.
    pub fn first_declared(&self) -> &dyn ServerHttpProtocol {
        match self.declared.first() {
            Some(first) => first.protocol.as_ref(),
            None => WireProtocol::unrecognised().protocol(),
        }
    }

    /// The protocol `request` is served in, or the refusal for a request that names
    /// none of the declared protocols.
    pub fn select(&self, request: &RtRequest) -> WireProtocol<'_> {
        if let [only] = self.declared.as_slice() {
            return only.serving();
        }
        let Some(named) = WireProtocolKind::detect(request) else {
            return WireProtocol::unrecognised();
        };
        match self
            .declared
            .iter()
            .find(|declared| declared.marker == Some(named))
        {
            Some(declared) => declared.serving(),
            None => WireProtocol::unrecognised(),
        }
    }
}

/// The protocol one request is served in: the codec that decodes and encodes it,
/// paired with the error envelope every framework-level failure renders in.
///
/// The two always travel together, so a response's body and its error envelope
/// can never come from different protocols.
///
/// For a request that names none of the service's protocols
/// ([`is_unrecognised`](Self::is_unrecognised)) the codec refuses every call and
/// every renderer answers the fixed refusal.
///
/// Its three renderers are what the generated service definition's own
/// renderers call. The router never calls them directly: it calls the
/// definition's, so a definition that wraps the generated entry to reword an
/// error keeps that wording whichever protocol the request arrived in.
#[derive(Clone, Copy)]
pub struct WireProtocol<'a> {
    protocol: &'a dyn ServerHttpProtocol,
    envelope: PreDispatchProtocol<'a>,
}

impl std::fmt::Debug for WireProtocol<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireProtocol")
            .field("protocol", &self.protocol.protocol_id())
            .field("envelope", &self.envelope)
            .finish()
    }
}

impl<'a> WireProtocol<'a> {
    /// Pair a codec with the error envelope of the same protocol.
    pub fn new(protocol: &'a dyn ServerHttpProtocol, envelope: PreDispatchProtocol<'a>) -> Self {
        Self { protocol, envelope }
    }

    /// The request names none of the service's protocols.
    pub fn unrecognised() -> WireProtocol<'static> {
        static NO_PROTOCOL: NoProtocol = NoProtocol;
        WireProtocol::new(&NO_PROTOCOL, PreDispatchProtocol::Unrecognised)
    }

    /// Whether the request names none of the service's protocols, so the only
    /// answer is the refusal.
    pub fn is_unrecognised(&self) -> bool {
        matches!(self.envelope, PreDispatchProtocol::Unrecognised)
    }

    /// The codec that decodes the request and encodes its response.
    pub fn protocol(&self) -> &'a dyn ServerHttpProtocol {
        self.protocol
    }

    /// The error envelope framework-level failures render in.
    pub fn envelope(&self) -> PreDispatchProtocol<'a> {
        self.envelope
    }

    /// The routing failure for `request` in this protocol's envelope: the routing key
    /// it names matched no modeled operation, or the request named none.
    pub fn routing_error_response(&self, request: &RtRequest) -> RtResponse {
        let error = match self.protocol.routing_key(request) {
            Ok(op) => ProtocolError::UnknownOperation(op.to_string()),
            Err(e) => e,
        };
        crate::protocol_error_response(self.envelope, error)
    }

    /// An arbitrary framework-level failure in this protocol's envelope.
    pub fn framework_error_response(&self, error: ProtocolError) -> RtResponse {
        crate::protocol_error_response(self.envelope, error)
    }

    /// A refusal in this protocol's envelope, worded by the service's
    /// `override_refusal` choice when it made one.
    pub fn refusal_response(&self, error: ProtocolError, chosen: Option<Refused>) -> RtResponse {
        crate::protocol_error_response_with(self.envelope, error, chosen)
    }
}

/// The codec of [`WireProtocol::unrecognised`]. The request names no protocol, so
/// there is nothing to decode or encode it with: every call refuses.
#[derive(Debug)]
struct NoProtocol;

impl NoProtocol {
    fn refusal() -> ProtocolError {
        ProtocolError::Malformed("the request names none of the service's protocols".to_string())
    }
}

impl ServerProtocol for NoProtocol {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &ShapeId {
        static ID: ShapeId = ShapeId::from_static("emulator#NoProtocol", "emulator", "NoProtocol");
        &ID
    }

    fn routing_key<'a>(&self, _request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        Err(Self::refusal())
    }

    fn deserialize_input<'a>(
        &self,
        _request: &'a RtRequest,
        _input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        Err(Self::refusal())
    }

    fn serialize_output(
        &self,
        _output: &dyn SerializableStruct,
        _output_schema: &Schema,
        _operation_name: &str,
        _success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        Err(Self::refusal())
    }

    fn serialize_error(
        &self,
        _error: &dyn SerializableStruct,
        _error_schema: &Schema,
        _error_code: &str,
        _http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        Err(Self::refusal())
    }
}

#[cfg(test)]
mod tests {
    use super::{DeclaredProtocol, DeclaredProtocols, WireProtocol, WireProtocolKind};
    use crate::pre_dispatch::UNRECOGNISED_PROTOCOL_BODY;
    use crate::PreDispatchProtocol;
    use aws_smithy_runtime_api::http::Request as RtRequest;
    use aws_smithy_schema_server::rpc::{AwsJson10, Rpcv2Cbor};
    use aws_smithy_schema_server::QueryServerProtocol;
    use aws_smithy_types::body::SdkBody;

    fn request(headers: &[(&str, &str)]) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        for (name, value) in headers {
            req.headers_mut()
                .append(name.to_string(), value.to_string());
        }
        req
    }

    fn request_to(uri: &str, headers: &[(&str, &str)]) -> RtRequest {
        let mut req = request(headers);
        req.set_uri(uri).expect("valid uri");
        req
    }

    #[test]
    fn the_smithy_protocol_header_selects_cbor() {
        let req = request(&[
            ("smithy-protocol", "rpc-v2-cbor"),
            ("content-type", "application/cbor"),
        ]);
        assert_eq!(
            WireProtocolKind::detect(&req),
            Some(WireProtocolKind::RpcV2Cbor)
        );
    }

    #[test]
    fn the_rpcv2_path_selects_cbor_without_the_header() {
        let req = request_to("/service/Granite/operation/ListMetrics?x=1", &[]);
        assert_eq!(
            WireProtocolKind::detect(&req),
            Some(WireProtocolKind::RpcV2Cbor)
        );
    }

    #[test]
    fn a_path_that_is_not_the_rpcv2_shape_selects_nothing() {
        for uri in [
            "/",
            "/operation/ListMetrics",
            "/service/operation/ListMetrics",
            "/service/a/b/operation/ListMetrics",
            "/service/Granite/operation/",
            "/service/Granite/operation/a/b",
        ] {
            let req = request_to(uri, &[]);
            assert_eq!(WireProtocolKind::detect(&req), None, "{uri}");
        }
    }

    #[test]
    fn x_amz_target_selects_aws_json() {
        let req = request(&[("x-amz-target", "GraniteServiceVersion20100801.ListMetrics")]);
        assert_eq!(
            WireProtocolKind::detect(&req),
            Some(WireProtocolKind::AwsJson)
        );
    }

    #[test]
    fn an_aws_json_content_type_selects_aws_json() {
        let req = request(&[("content-type", "application/x-amz-json-1.0")]);
        assert_eq!(
            WireProtocolKind::detect(&req),
            Some(WireProtocolKind::AwsJson)
        );
    }

    #[test]
    fn a_form_content_type_selects_query() {
        let req = request(&[(
            "content-type",
            "application/x-www-form-urlencoded; charset=utf-8",
        )]);
        assert_eq!(
            WireProtocolKind::detect(&req),
            Some(WireProtocolKind::Query)
        );
    }

    #[test]
    fn a_request_with_no_marker_names_no_protocol() {
        let req = request(&[("content-type", "application/json")]);
        assert_eq!(WireProtocolKind::detect(&req), None);
    }

    fn json() -> DeclaredProtocol {
        DeclaredProtocol::new(
            Some(WireProtocolKind::AwsJson),
            Box::new(AwsJson10()),
            PreDispatchProtocol::Json {
                content_type: "application/x-amz-json-1.0",
                errortype_header: false,
            },
        )
    }

    fn cbor() -> DeclaredProtocol {
        DeclaredProtocol::new(
            Some(WireProtocolKind::RpcV2Cbor),
            Box::new(Rpcv2Cbor()),
            PreDispatchProtocol::RpcV2Cbor,
        )
    }

    fn query() -> DeclaredProtocol {
        DeclaredProtocol::new(
            Some(WireProtocolKind::Query),
            Box::new(QueryServerProtocol::aws_query()),
            PreDispatchProtocol::Query { ec2: false },
        )
    }

    fn rest_json() -> DeclaredProtocol {
        DeclaredProtocol::new(
            None,
            Box::new(aws_smithy_schema_server::RestJsonServerProtocol::new(&[])),
            PreDispatchProtocol::Json {
                content_type: "application/json",
                errortype_header: true,
            },
        )
    }

    fn served_by(declared: &DeclaredProtocols, req: &RtRequest) -> String {
        let wire = declared.select(req);
        assert!(!wire.is_unrecognised(), "{wire:?}");
        wire.protocol().protocol_id().to_string()
    }

    #[test]
    fn a_list_of_one_serves_every_request_in_its_protocol() {
        let declared = DeclaredProtocols::new(vec![json()]);
        for req in [
            request(&[]),
            request(&[("x-amz-target", "Svc.Op")]),
            request(&[("smithy-protocol", "rpc-v2-cbor")]),
            request(&[("content-type", "application/x-www-form-urlencoded")]),
        ] {
            assert_eq!(served_by(&declared, &req), "aws.protocols#awsJson1_0");
        }
    }

    #[test]
    fn the_first_declared_codec_is_the_head_of_the_list() {
        let first_id =
            |declared: DeclaredProtocols| declared.first_declared().protocol_id().to_string();
        assert_eq!(
            first_id(DeclaredProtocols::new(vec![json()])),
            "aws.protocols#awsJson1_0"
        );
        assert_eq!(
            first_id(DeclaredProtocols::new(vec![json(), cbor(), query()])),
            "aws.protocols#awsJson1_0"
        );
        assert_eq!(
            first_id(DeclaredProtocols::new(vec![cbor(), query()])),
            "smithy.protocols#rpcv2Cbor"
        );
    }

    #[test]
    fn an_empty_list_has_the_refusing_codec_first() {
        let declared = DeclaredProtocols::new(vec![]);
        assert_eq!(
            declared.first_declared().protocol_id(),
            WireProtocol::unrecognised().protocol().protocol_id()
        );
    }

    #[test]
    fn a_rest_service_serves_a_request_that_carries_no_marker() {
        let declared = DeclaredProtocols::new(vec![rest_json()]);
        let req = request_to("/things/1", &[("content-type", "application/json")]);
        assert_eq!(served_by(&declared, &req), "aws.protocols#restJson1");
    }

    #[test]
    fn a_list_of_several_serves_the_protocol_the_request_names() {
        let declared = DeclaredProtocols::new(vec![json(), cbor(), query()]);
        let as_json = request(&[("x-amz-target", "Svc.Op")]);
        let as_cbor = request(&[("smithy-protocol", "rpc-v2-cbor")]);
        let as_query = request(&[("content-type", "application/x-www-form-urlencoded")]);
        assert_eq!(served_by(&declared, &as_json), "aws.protocols#awsJson1_0");
        assert_eq!(served_by(&declared, &as_cbor), "smithy.protocols#rpcv2Cbor");
        assert_eq!(served_by(&declared, &as_query), "aws.protocols#awsQuery");
    }

    #[test]
    fn the_order_of_the_list_does_not_change_the_selection() {
        let declared = DeclaredProtocols::new(vec![query(), cbor(), json()]);
        let as_json = request(&[("x-amz-target", "Svc.Op")]);
        assert_eq!(served_by(&declared, &as_json), "aws.protocols#awsJson1_0");
    }

    #[test]
    fn a_request_with_no_marker_is_unrecognised_by_a_list_of_several() {
        let declared = DeclaredProtocols::new(vec![json(), cbor()]);
        let wire = declared.select(&request(&[("content-type", "application/json")]));
        assert!(wire.is_unrecognised(), "{wire:?}");
    }

    #[test]
    fn a_request_naming_an_undeclared_protocol_is_unrecognised() {
        let declared = DeclaredProtocols::new(vec![json(), cbor()]);
        let as_query = request(&[("content-type", "application/x-www-form-urlencoded")]);
        assert!(declared.select(&as_query).is_unrecognised());
    }

    #[test]
    fn the_unrecognised_protocol_refuses_with_a_fixed_404() {
        let req = request(&[]);
        let wire = WireProtocol::unrecognised();
        for response in [
            wire.routing_error_response(&req),
            wire.framework_error_response(aws_smithy_schema_server::ProtocolError::Malformed(
                "ignored".to_string(),
            )),
            wire.refusal_response(
                aws_smithy_schema_server::ProtocolError::Malformed("ignored".to_string()),
                Some(crate::Refused {
                    code: "ignored".to_string(),
                    message: "ignored".to_string(),
                }),
            ),
        ] {
            assert_eq!(response.status().as_u16(), 404);
            assert_eq!(response.headers().get("content-type"), None);
            assert_eq!(
                response.body().bytes(),
                Some(UNRECOGNISED_PROTOCOL_BODY.as_bytes())
            );
        }
    }
}
