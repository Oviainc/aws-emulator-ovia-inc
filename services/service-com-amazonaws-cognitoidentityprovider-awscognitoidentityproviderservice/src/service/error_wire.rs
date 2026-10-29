// SPDX-License-Identifier: CC0-1.0

//! How this service answers an ERROR on the wire: the HTTP status a refusal of the caller
//! carries, and the header that names the error shape it refused with.
//!
//! Two facts, both of this service's recorded wire rather than of its model:
//!
//! * **The status.** A refusal of the CALLER is HTTP 400, whatever `@httpError` the error
//!   shape declares. The model annotates the shapes with the statuses an HTTP-bound
//!   service would answer (`ResourceNotFoundException` 404, `NotAuthorizedException` and
//!   `FeatureUnavailableInTierException` 403, `TooManyRequestsException` 429), and this
//!   endpoint applies none of them: every recorded refusal of this API is a 400 whose
//!   error class travels in the body's `__type`. That matters past cosmetics on an
//!   RPC endpoint, where a 404 reads as "no such route" and a 403 as "your credentials
//!   were rejected" to every proxy, gateway and SDK retry classifier in front of it, so a
//!   modelled refusal the caller branches on arrives as a transport failure instead.
//!   A SERVER fault keeps its own status: a 5xx is not a refusal of the caller, and the
//!   scaffold's `NotImplemented` 501 must stay a 501 so an unimplemented operation is not
//!   read as a bad request.
//!
//! * **The header.** Every error response names its error shape in `x-amzn-errortype`, as
//!   the shape's simple name followed by a colon. An AWS SDK resolves the concrete
//!   exception type from that header first and falls back to the body's `__type` only
//!   when it is absent, so a client that reads the header alone cannot tell two refusals
//!   apart without it. The framework's awsJson renderers deliberately omit it
//!   (`vendor/smithy-server-runtime/src/pre_dispatch.rs`), which is why it is added here.
//!
//! Nothing else about a response changes. The body, the message, the media type and every
//! other header stay with the framework renderer that owns them, and a SUCCESS is passed
//! through untouched: [`ErrorWireProtocol`] delegates routing, request deserialization and
//! the success body to the framework's own `awsJson1_1` — the protocol this service's model
//! declares, built here exactly as the generated entry builds it.
//!
//! Which seams this is applied at is [`crate::service`]'s business: the protocol wrapper
//! covers every error a handler raises, and [`naming_the_error_shape`] is called directly
//! on the refusal the centralized walk renders for a request that breaks a modelled rule,
//! which no protocol reaches.

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::json::JsonCodec;
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::rpc::HttpRpcServerProtocol;

/// The response header an AWS client resolves the concrete error shape from.
const ERRORTYPE_HEADER: &str = "x-amzn-errortype";

/// The status this endpoint answers every refusal of the caller with.
const CLIENT_REFUSAL_STATUS: u16 = 400;

/// `resp` as this service sends it for a refusal whose error class is `error_code`: the
/// error shape named in `x-amzn-errortype`, as the shape's simple name and a colon.
///
/// The colon is fixed text; the name is the class the refusal was raised with. Inserted
/// rather than appended, so a response that already names its shape keeps one value.
pub fn naming_the_error_shape(mut resp: Response, error_code: &str) -> Response {
    resp.headers_mut()
        .insert(ERRORTYPE_HEADER, format!("{error_code}:"));
    resp
}

/// The status this endpoint answers an error with, given the `modelled` status the error
/// shape's `@httpError` declares: 400 for every refusal of the caller, the modelled status
/// for a server fault (and for the scaffold's `NotImplemented`).
fn served_status(modelled: u16) -> u16 {
    if (400..500).contains(&modelled) {
        CLIENT_REFUSAL_STATUS
    } else {
        modelled
    }
}

/// This service's wire protocol: `awsJson1_1` exactly as the model declares it, with the
/// error status and the error-shape header this endpoint is recorded answering (see the
/// module docs).
#[derive(Debug)]
pub struct ErrorWireProtocol {
    inner: HttpRpcServerProtocol<JsonCodec>,
}

impl ErrorWireProtocol {
    /// Build the protocol once, at service construction: `override_protocol` borrows it
    /// from `&self` and must never build one per request.
    pub fn new() -> Self {
        Self {
            inner: aws_smithy_schema_server::rpc::AwsJson11(),
        }
    }
}

impl Default for ErrorWireProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for ErrorWireProtocol {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Self::Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_output(output, output_schema, operation_name, success_status)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        let resp = self.inner.serialize_error(
            error,
            error_schema,
            error_code,
            served_status(http_status),
        )?;
        Ok(naming_the_error_shape(resp, error_code))
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}

#[cfg(test)]
mod tests {
    use super::{naming_the_error_shape, served_status, ERRORTYPE_HEADER};
    use aws_smithy_types::body::SdkBody;

    /// A response carrying `status` and nothing else.
    fn response(status: u16) -> aws_smithy_runtime_api::http::Response {
        aws_smithy_runtime_api::http::Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(&b"{}"[..]),
        )
    }

    /// [`served_status`] over the statuses this service's error shapes declare, and over
    /// the bounds of the client-error range either side.
    ///
    /// The modelled statuses are this model's own
    /// (`smithy_query(query="shape",shape="ResourceNotFoundException,NotAuthorizedException,TooManyRequestsException,InternalErrorException")`):
    /// 400 `InvalidParameterException`, 403 `NotAuthorizedException` /
    /// `FeatureUnavailableInTierException` / `ForbiddenException`, 404
    /// `ResourceNotFoundException` / `UserNotFoundException`, 429
    /// `TooManyRequestsException`, 500 `InternalErrorException` — plus the scaffold's 501.
    #[test]
    fn every_client_refusal_is_served_as_a_400_and_a_server_fault_keeps_its_status() {
        // (case, modelled status, served status)
        let rows: &[(&str, u16, u16)] = &[
            ("a bad parameter, modelled at 400 already", 400, 400),
            ("the lowest client status there is", 400, 400),
            ("an unauthorized caller, modelled at 403", 403, 400),
            ("a resource that does not exist, modelled at 404", 404, 400),
            ("a throttled caller, modelled at 429", 429, 400),
            ("the highest client status there is", 499, 400),
            ("an internal fault, modelled at 500", 500, 500),
            ("the scaffold's not-implemented signal", 501, 501),
            ("a gateway fault one past the client range", 502, 502),
        ];

        for (case, modelled, expected) in rows {
            let actual = served_status(*modelled);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }

    /// [`naming_the_error_shape`] names the shape it was raised with, once, whatever the
    /// response already carried.
    #[test]
    fn an_error_response_names_its_shape_in_the_errortype_header() {
        for case in [
            "ResourceNotFoundException",
            "InvalidParameterException",
            "NotAuthorizedException",
        ] {
            let resp = naming_the_error_shape(response(400), case);
            let actual = resp.headers().get(ERRORTYPE_HEADER);
            assert_eq!(
                actual,
                Some(format!("{case}:").as_str()),
                "case {case:?}: expected the shape name and a colon, got {actual:?}"
            );

            // Naming it twice leaves ONE value: a client reads one header.
            let resp = naming_the_error_shape(resp, case);
            let named: Vec<&str> = resp.headers().get_all(ERRORTYPE_HEADER).collect();
            assert_eq!(
                named,
                vec![format!("{case}:").as_str()],
                "case {case:?}: expected one named shape, got {named:?}"
            );
        }
    }
}
