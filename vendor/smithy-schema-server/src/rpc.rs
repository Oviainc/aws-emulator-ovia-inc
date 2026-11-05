// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! HTTP RPC server protocol — the inverse of
//! [`aws_smithy_schema::http_protocol::HttpRpcProtocol`].
//!
//! Body-only RPC protocols (`awsJson1_0`, `awsJson1_1`, `rpcv2Cbor`) all
//! share the same shape on the wire: every operation is a `POST /` with the
//! whole input encoded in the body. They differ only by codec and routing
//! header.
//!
//! [`HttpRpcServerProtocol`] captures that pattern: pick a [`Codec`], a
//! content-type, and a routing-key extractor, and you have a working server
//! protocol. [`AwsJson10`] / [`AwsJson11`] are convenience constructors for
//! the awsJson family.

use std::io::Read;

use crate::cbor::CborCodec;
use crate::json::JsonCodec;
use crate::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_types::body::SdkBody;
use bytes::Bytes;
use std::sync::Arc;

/// The most bytes a `content-encoding: gzip` request body may inflate to.
///
/// A cap on the wire body bounds the COMPRESSED bytes only, and a run of
/// repeated bytes compresses to a tiny fraction of its size, so a request small
/// enough to pass any transport limit could otherwise allocate gigabytes here
/// and take the whole emulator process down. This equals the production serve
/// path's cap on an uncompressed body, so compressing a request never lets it
/// carry more than sending it plain would. Raise the two together.
const MAX_DECOMPRESSED_BODY_BYTES: usize = 16 * 1024 * 1024;

/// A function that extracts the operation name from a request.
///
/// Boxed so that protocols can pick one at runtime — `awsJson1_0` reads
/// `x-amz-target`, REST-JSON would read the URI path, etc.
pub type RoutingKeyFn =
    Arc<dyn for<'a> Fn(&'a Request) -> Result<&'a str, ProtocolError> + Send + Sync>;

/// Read the operation name from the `x-amz-target` header.
///
/// AWS RPC protocols use the header `x-amz-target: ServiceName.OperationName`.
/// This function returns the substring after the dot.
pub fn routing_key_from_x_amz_target(request: &Request) -> Result<&str, ProtocolError> {
    let header = request
        .headers()
        .get("x-amz-target")
        .ok_or(ProtocolError::MissingHeader("x-amz-target"))?;
    header
        .split_once('.')
        .map(|(_, op)| op)
        .ok_or_else(|| ProtocolError::InvalidHeader {
            name: "x-amz-target",
            message: format!("expected `Service.Operation`, got `{header}`"),
        })
}

/// Read the operation name from the request URI path.
///
/// The Smithy RPCv2 CBOR protocol routes by path rather than by a header:
/// every request is `POST /service/{ServiceName}/operation/{OperationName}`.
/// This returns the `{OperationName}` segment — the substring after the last
/// `/operation/`.
pub fn routing_key_from_path(request: &Request) -> Result<&str, ProtocolError> {
    let path = request.uri();
    // Strip any query string.
    let path = path.split('?').next().unwrap_or(path);
    let op = path
        .rsplit_once("/operation/")
        .map(|(_, op)| op.trim_end_matches('/'))
        .filter(|op| !op.is_empty() && !op.contains('/'));
    op.ok_or_else(|| ProtocolError::InvalidHeader {
        name: ":path",
        message: format!("expected `/service/{{Service}}/operation/{{Operation}}`, got `{path}`"),
    })
}

/// HTTP RPC server protocol parameterised over a [`Codec`].
///
/// This is the server-side analog of
/// [`aws_smithy_schema::http_protocol::HttpRpcProtocol`]. It is generic over
/// `Codec`, so the same body of code services awsJson1_0, awsJson1_1,
/// rpcv2Cbor, and any future RPC protocol that fits the same shape.
pub struct HttpRpcServerProtocol<C> {
    protocol_id: ShapeId,
    // `Arc` so the codec can be shared into an `OwningDeserializer` (the gzip
    // path) without requiring the codec itself to be `Copy`/`Clone` — upstream
    // `aws_smithy_cbor::codec::CborCodec` is neither.
    codec: Arc<C>,
    content_type: &'static str,
    /// Byte sequence substituted for an empty request body before handing it to
    /// the codec. awsJson uses `b"{}"`; rpcv2Cbor uses an empty CBOR map.
    empty_body: &'static [u8],
    /// Value for the `smithy-protocol` response header, if the protocol requires
    /// one (rpcv2Cbor sets `rpc-v2-cbor`; awsJson sets none).
    smithy_protocol: Option<&'static str>,
    routing_key: RoutingKeyFn,
}

impl<C: std::fmt::Debug> std::fmt::Debug for HttpRpcServerProtocol<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRpcServerProtocol")
            .field("protocol_id", &self.protocol_id)
            .field("codec", &self.codec)
            .field("content_type", &self.content_type)
            .finish_non_exhaustive()
    }
}

impl<C: Codec> HttpRpcServerProtocol<C> {
    /// Construct a server protocol from its constituent parts.
    pub fn new(
        protocol_id: ShapeId,
        codec: C,
        content_type: &'static str,
        routing_key: RoutingKeyFn,
    ) -> Self {
        Self {
            protocol_id,
            codec: Arc::new(codec),
            content_type,
            empty_body: b"{}",
            smithy_protocol: None,
            routing_key,
        }
    }

    /// Override the byte sequence substituted for an empty request body.
    ///
    /// Defaults to `b"{}"` (the JSON empty object). rpcv2Cbor uses an empty
    /// CBOR map (`&[0xa0]`).
    pub fn with_empty_body(mut self, empty_body: &'static [u8]) -> Self {
        self.empty_body = empty_body;
        self
    }

    /// Set the value emitted for the `smithy-protocol` response header.
    pub fn with_smithy_protocol(mut self, value: &'static str) -> Self {
        self.smithy_protocol = Some(value);
        self
    }
}

/// A [`ShapeDeserializer`] that owns its backing byte buffer.
///
/// When the request body is compressed, the decompressed bytes are not part of
/// the original request and therefore cannot borrow from it. This wrapper owns
/// the decompressed bytes as a [`Bytes`] buffer and creates deserializers
/// on-demand that borrow from it.
struct OwningDeserializer<C> {
    data: Bytes,
    codec: Arc<C>,
}

impl<C: Codec> OwningDeserializer<C> {
    fn new(data: Vec<u8>, codec: Arc<C>) -> Self {
        Self {
            data: Bytes::from(data),
            codec,
        }
    }

    fn with_deser<R>(&self, f: impl FnOnce(&mut C::Deserializer<'_>) -> R) -> R {
        let mut deser = self.codec.create_deserializer(&self.data);
        f(&mut deser)
    }
}

impl<C: Codec> ShapeDeserializer for OwningDeserializer<C>
where
    for<'a> C::Deserializer<'a>: ShapeDeserializer,
{
    fn read_struct(
        &mut self,
        schema: &Schema,
        state: &mut dyn FnMut(
            &Schema,
            &mut dyn ShapeDeserializer,
        ) -> Result<(), aws_smithy_schema::serde::SerdeError>,
    ) -> Result<(), aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_struct(schema, state))
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        state: &mut dyn FnMut(
            &mut dyn ShapeDeserializer,
        ) -> Result<(), aws_smithy_schema::serde::SerdeError>,
    ) -> Result<(), aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_list(schema, state))
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        state: &mut dyn FnMut(
            String,
            &mut dyn ShapeDeserializer,
        ) -> Result<(), aws_smithy_schema::serde::SerdeError>,
    ) -> Result<(), aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_map(schema, state))
    }

    fn read_boolean(
        &mut self,
        schema: &Schema,
    ) -> Result<bool, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_boolean(schema))
    }

    fn read_byte(&mut self, schema: &Schema) -> Result<i8, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_byte(schema))
    }

    fn read_short(&mut self, schema: &Schema) -> Result<i16, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_short(schema))
    }

    fn read_integer(
        &mut self,
        schema: &Schema,
    ) -> Result<i32, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_integer(schema))
    }

    fn read_long(&mut self, schema: &Schema) -> Result<i64, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_long(schema))
    }

    fn read_float(&mut self, schema: &Schema) -> Result<f32, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_float(schema))
    }

    fn read_double(
        &mut self,
        schema: &Schema,
    ) -> Result<f64, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_double(schema))
    }

    fn read_big_integer(
        &mut self,
        schema: &Schema,
    ) -> Result<aws_smithy_types::BigInteger, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_big_integer(schema))
    }

    fn read_big_decimal(
        &mut self,
        schema: &Schema,
    ) -> Result<aws_smithy_types::BigDecimal, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_big_decimal(schema))
    }

    fn read_string(
        &mut self,
        schema: &Schema,
    ) -> Result<String, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_string(schema))
    }

    fn read_blob(
        &mut self,
        schema: &Schema,
    ) -> Result<aws_smithy_types::Blob, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_blob(schema))
    }

    fn read_timestamp(
        &mut self,
        schema: &Schema,
    ) -> Result<aws_smithy_types::DateTime, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_timestamp(schema))
    }

    fn read_document(
        &mut self,
        schema: &Schema,
    ) -> Result<aws_smithy_types::Document, aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_document(schema))
    }

    fn is_null(&self) -> bool {
        self.with_deser(|d| d.is_null())
    }

    fn read_null(&mut self) -> Result<(), aws_smithy_schema::serde::SerdeError> {
        self.with_deser(|d| d.read_null())
    }

    fn container_size(&self) -> Option<usize> {
        self.with_deser(|d| d.container_size())
    }
}

impl<C> ServerProtocol for HttpRpcServerProtocol<C>
where
    C: Codec + Send + Sync + std::fmt::Debug + 'static,
    for<'a> C::Deserializer<'a>: ShapeDeserializer,
{
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        &self.protocol_id
    }

    fn routing_key<'a>(&self, request: &'a Request) -> Result<&'a str, ProtocolError> {
        (self.routing_key)(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Request,
        _input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        let body = request
            .body()
            .bytes()
            .ok_or_else(|| ProtocolError::BodyRead("body is not available as bytes".into()))?;

        let is_gzip = request
            .headers()
            .get("content-encoding")
            .map(|v| v.contains("gzip"))
            .unwrap_or(false);

        if is_gzip {
            // Read one byte past the limit: a body that reaches it is refused
            // without ever being inflated further.
            let limit = MAX_DECOMPRESSED_BODY_BYTES as u64 + 1;
            let mut decoder = flate2::read::GzDecoder::new(body).take(limit);
            let mut decompressed = Vec::new();
            decoder
                .read_to_end(&mut decompressed)
                .map_err(|e| ProtocolError::BodyRead(format!("gzip decompression failed: {e}")))?;
            if decompressed.len() > MAX_DECOMPRESSED_BODY_BYTES {
                return Err(ProtocolError::BodyRead(format!(
                    "gzip body inflates past the limit of {MAX_DECOMPRESSED_BODY_BYTES} bytes"
                )));
            }
            if decompressed.is_empty() {
                decompressed = self.empty_body.to_vec();
            }
            Ok(Box::new(OwningDeserializer::new(
                decompressed,
                Arc::clone(&self.codec),
            )))
        } else {
            // Empty bodies are valid for inputs with all optional members; treat
            // them as the codec's empty-container sentinel.
            let bytes: &'a [u8] = if body.is_empty() {
                self.empty_body
            } else {
                body
            };
            Ok(Box::new(self.codec.create_deserializer(bytes)))
        }
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        _operation_name: &str,
        success_status: u16,
    ) -> Result<Response, ProtocolError> {
        let mut serializer = self.codec.create_serializer();
        serializer.write_struct(output_schema, output)?;
        let body = serializer.finish();

        // The awsJson family carries no transport status binding, so the
        // operation's success status is the response status directly.
        let status = aws_smithy_runtime_api::http::StatusCode::try_from(success_status)
            .unwrap_or_else(|_| {
                aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200 is valid")
            });
        let mut resp = Response::new(status, SdkBody::from(body));
        resp.headers_mut().insert("Content-Type", self.content_type);
        if let Some(sp) = self.smithy_protocol {
            resp.headers_mut().insert("smithy-protocol", sp);
        }
        Ok(resp)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Response, ProtocolError> {
        // For the awsJson family the error body is just the error struct
        // serialised as JSON, with a `__type` member added at the top level.
        // Rather than mutate the Schema (which is `&'static`), we wrap the
        // user-supplied error struct in a small `SerializableStruct` adaptor
        // that emits `__type` first and then delegates to the original
        // struct's `serialize_members`.
        struct WithType<'a> {
            error_code: &'a str,
            inner: &'a dyn SerializableStruct,
        }

        impl<'a> SerializableStruct for WithType<'a> {
            fn serialize_members(
                &self,
                serializer: &mut dyn ShapeSerializer,
            ) -> Result<(), aws_smithy_schema::serde::SerdeError> {
                serializer.write_string(&__TYPE_MEMBER_SCHEMA, self.error_code)?;
                self.inner.serialize_members(serializer)
            }
        }

        let mut serializer = self.codec.create_serializer();
        serializer.write_struct(
            error_schema,
            &WithType {
                error_code,
                inner: error,
            },
        )?;
        let body = serializer.finish();

        let status = aws_smithy_runtime_api::http::StatusCode::try_from(http_status)
            .unwrap_or(aws_smithy_runtime_api::http::StatusCode::try_from(400u16).expect("400"));
        let mut resp = Response::new(status, SdkBody::from(body));
        resp.headers_mut().insert("Content-Type", self.content_type);
        if let Some(sp) = self.smithy_protocol {
            resp.headers_mut().insert("smithy-protocol", sp);
        }
        Ok(resp)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Response, ProtocolError> {
        use aws_smithy_eventstream::frame::write_message_to;
        use aws_smithy_types::event_stream::{Header, HeaderValue, Message};

        let mut serializer = self.codec.create_serializer();
        serializer.write_struct(event_schema, event)?;
        let payload = serializer.finish();

        // Frame headers match the AWS event-stream contract the SDK decodes:
        // a data event carries :event-type, an in-band error :exception-type.
        let msg = if is_exception {
            Message::new(payload)
                .add_header(Header::new(
                    ":message-type",
                    HeaderValue::String("exception".into()),
                ))
                .add_header(Header::new(
                    ":exception-type",
                    HeaderValue::String(event_type.to_string().into()),
                ))
        } else {
            Message::new(payload)
                .add_header(Header::new(
                    ":message-type",
                    HeaderValue::String("event".into()),
                ))
                .add_header(Header::new(
                    ":event-type",
                    HeaderValue::String(event_type.to_string().into()),
                ))
                .add_header(Header::new(
                    ":content-type",
                    HeaderValue::String("application/json".into()),
                ))
        };

        let mut frame = Vec::new();
        write_message_to(&msg, &mut frame).map_err(|e| {
            ProtocolError::Serde(aws_smithy_schema::serde::SerdeError::custom(format!(
                "event-stream frame encode failed: {e}"
            )))
        })?;

        let mut resp = Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200 is valid"),
            SdkBody::from(Bytes::from(frame)),
        );
        resp.headers_mut()
            .insert("Content-Type", "application/vnd.amazon.eventstream");
        Ok(resp)
    }
}

/// Schema for the `__type` discriminator emitted by awsJson protocol errors.
///
/// awsJson errors carry an extra string member `__type` whose value is the
/// shape ID (or shape name) of the concrete error. The codec needs a
/// [`Schema`] reference to write a string, even though the JSON name is not
/// actually used for anything in our serializer (the `WithType` adaptor
/// emits the literal `"__type"` key directly via the member-name lookup).
static __TYPE_MEMBER_SCHEMA: Schema = Schema::new_member(
    aws_smithy_schema::ShapeId::from_static("smithy.api#__type", "smithy.api", "__type"),
    aws_smithy_schema::ShapeType::String,
    "__type",
    0,
)
.with_json_name("__type");

/// `awsJson1_0` server protocol: [`JsonCodec`] over `application/x-amz-json-1.0`.
#[allow(non_snake_case)]
pub fn AwsJson10() -> HttpRpcServerProtocol<JsonCodec> {
    HttpRpcServerProtocol::new(
        ShapeId::from_static("aws.protocols#awsJson1_0", "aws.protocols", "awsJson1_0"),
        JsonCodec::new(),
        "application/x-amz-json-1.0",
        Arc::new(routing_key_from_x_amz_target),
    )
}

/// `awsJson1_1` server protocol: [`JsonCodec`] over `application/x-amz-json-1.1`.
#[allow(non_snake_case)]
pub fn AwsJson11() -> HttpRpcServerProtocol<JsonCodec> {
    HttpRpcServerProtocol::new(
        ShapeId::from_static("aws.protocols#awsJson1_1", "aws.protocols", "awsJson1_1"),
        JsonCodec::new(),
        "application/x-amz-json-1.1",
        Arc::new(routing_key_from_x_amz_target),
    )
}

/// Smithy `rpcv2Cbor` server protocol: [`CborCodec`] over `application/cbor`.
///
/// Unlike the awsJson family, rpcv2Cbor routes by URI path
/// (`POST /service/{Service}/operation/{Operation}`) rather than the
/// `x-amz-target` header, substitutes an empty CBOR map for empty request
/// bodies, and tags every response with the `smithy-protocol: rpc-v2-cbor`
/// header.
#[allow(non_snake_case)]
pub fn Rpcv2Cbor() -> HttpRpcServerProtocol<CborCodec> {
    HttpRpcServerProtocol::new(
        ShapeId::from_static(
            "smithy.protocols#rpcv2Cbor",
            "smithy.protocols",
            "rpcv2Cbor",
        ),
        CborCodec::default(),
        "application/cbor",
        Arc::new(routing_key_from_path),
    )
    // Empty CBOR map (definite length 0): the rpcv2Cbor equivalent of `{}`.
    .with_empty_body(&[0xa0])
    .with_smithy_protocol("rpc-v2-cbor")
}

/// Encode a minimal `rpcv2Cbor` error envelope: the definite-length CBOR map
/// `{"__type": <error_code>, "message": <message>}`.
///
/// This mirrors the awsJson `{"__type", "message"}` synthetic-error shape but
/// on the CBOR wire, so the request dispatcher can render framework-level
/// failures (constraint validation, unhandled protocol errors) that occur
/// before a modeled error struct is available — cases where the codec-driven
/// [`ServerProtocol::serialize_error`] path cannot be used because there is no
/// `SerializableStruct` to serialize.
pub fn cbor_synthetic_error_body(error_code: &str, message: &str) -> Vec<u8> {
    let mut encoder = aws_smithy_cbor::Encoder::new(Vec::new());
    encoder
        .map(2)
        .str("__type")
        .str(error_code)
        .str("message")
        .str(message);
    encoder.into_writer()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;
    use aws_smithy_runtime_api::http::Request;
    use aws_smithy_types::body::SdkBody;
    use std::io::Write;

    fn req_with_uri(uri: &str) -> Request {
        let mut req = Request::new(SdkBody::empty());
        req.set_uri(uri).expect("uri parses");
        req
    }

    #[test]
    fn routing_key_from_path_extracts_operation() {
        let req = req_with_uri("/service/RpcV2Protocol/operation/EmptyInputOutput");
        check!(routing_key_from_path(&req).unwrap() == "EmptyInputOutput");
    }

    #[test]
    fn routing_key_from_path_strips_query_string() {
        let req = req_with_uri("/service/RpcV2Protocol/operation/Op?foo=bar");
        check!(routing_key_from_path(&req).unwrap() == "Op");
    }

    #[test]
    fn routing_key_from_path_rejects_non_rpcv2_path() {
        let req = req_with_uri("/not/an/rpcv2/path");
        check!(routing_key_from_path(&req).is_err());
    }

    #[test]
    fn rpcv2cbor_protocol_id_and_headers() {
        let proto = Rpcv2Cbor();
        check!(proto.protocol_id().to_string() == "smithy.protocols#rpcv2Cbor");

        // An empty output struct serializes with the smithy-protocol header set.
        struct Empty;
        impl SerializableStruct for Empty {
            fn serialize_members(
                &self,
                _: &mut dyn ShapeSerializer,
            ) -> Result<(), aws_smithy_schema::serde::SerdeError> {
                Ok(())
            }
        }
        static OUTPUT_SCHEMA: Schema = Schema::new_struct(
            ShapeId::from_static("test#Out", "test", "Out"),
            aws_smithy_schema::ShapeType::Structure,
            &[],
        );
        let resp = proto
            .serialize_output(&Empty, &OUTPUT_SCHEMA, "TestOp", 200)
            .expect("serialize_output");
        check!(resp.headers().get("smithy-protocol") == Some("rpc-v2-cbor"));
        check!(resp.headers().get("Content-Type") == Some("application/cbor"));
    }

    #[test]
    fn cbor_synthetic_error_body_roundtrips() {
        let body = cbor_synthetic_error_body("ValidationException", "bad input: 3 > 2");

        // Decode the CBOR map back and assert both members survive the wire.
        let mut decoder = aws_smithy_cbor::Decoder::new(&body);
        let len = decoder
            .map()
            .expect("cbor map header")
            .expect("definite length");
        check!(len == 2);

        let mut error_code = None;
        let mut message = None;
        for _ in 0..len {
            let key = decoder.str().expect("map key is a string").into_owned();
            let value = decoder.str().expect("map value is a string").into_owned();
            match key.as_str() {
                "__type" => error_code = Some(value),
                "message" => message = Some(value),
                other => panic!("unexpected key {other}"),
            }
        }
        check!(error_code.as_deref() == Some("ValidationException"));
        check!(message.as_deref() == Some("bad input: 3 > 2"));
    }

    #[test]
    fn test_gzip_decompression() {
        // Create gzip-compressed JSON body {"value": 42}
        let json = br#"{"value":42}"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(json).expect("write succeeded");
        let compressed = encoder.finish().expect("gzip encoding succeeded");

        // Build request with content-encoding: gzip
        let mut req = Request::new(SdkBody::from(compressed));
        req.headers_mut().insert("content-encoding", "gzip");
        req.headers_mut()
            .insert("x-amz-target", "Service.Operation");

        // Create a simple test schema
        static VALUE_MEMBER: Schema = Schema::new_member(
            ShapeId::from_static("test#TestInput$value", "test#TestInput", "value"),
            aws_smithy_schema::ShapeType::Integer,
            "value",
            0,
        );
        static TEST_INPUT_MEMBERS: &[&Schema] = &[&VALUE_MEMBER];
        static TEST_INPUT_SCHEMA: Schema = Schema::new_struct(
            ShapeId::from_static("test#TestInput", "test", "TestInput"),
            aws_smithy_schema::ShapeType::Structure,
            TEST_INPUT_MEMBERS,
        );

        // Deserialize via AwsJson10 protocol
        let protocol = AwsJson10();
        let mut deser = protocol
            .deserialize_input(&req, &TEST_INPUT_SCHEMA)
            .expect("deserialize_input succeeded");

        // Read the integer value to exercise the forwarding methods
        let mut value_read = None;
        deser
            .read_struct(&TEST_INPUT_SCHEMA, &mut |member_schema, field_deser| {
                if member_schema.member_name() == Some("value") {
                    value_read = Some(field_deser.read_integer(member_schema)?);
                }
                Ok(())
            })
            .expect("read_struct succeeded");

        check!(value_read == Some(42));
    }

    fn gzip_request(plain: &[u8]) -> Request {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(plain).expect("write succeeded");
        let compressed = encoder.finish().expect("gzip encoding succeeded");
        let mut req = Request::new(SdkBody::from(compressed));
        req.headers_mut().insert("content-encoding", "gzip");
        req.headers_mut()
            .insert("x-amz-target", "Service.Operation");
        req
    }

    static EMPTY_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#EmptyInput", "test", "EmptyInput"),
        aws_smithy_schema::ShapeType::Structure,
        &[],
    );

    #[test]
    fn a_gzip_body_that_inflates_past_the_limit_is_refused() {
        // A few KiB on the wire: the compressed size is no defence, only the
        // inflated size is.
        let req = gzip_request(&vec![0u8; MAX_DECOMPRESSED_BODY_BYTES + 1]);
        check!(req.body().bytes().map_or(0, <[u8]>::len) < 64 * 1024);

        let refused = AwsJson10().deserialize_input(&req, &EMPTY_INPUT_SCHEMA);

        let Err(ProtocolError::BodyRead(message)) = refused else {
            panic!("expected a BodyRead refusal");
        };
        check!(message.contains(&MAX_DECOMPRESSED_BODY_BYTES.to_string()));
    }

    #[test]
    fn a_gzip_body_that_inflates_to_exactly_the_limit_is_read() {
        let req = gzip_request(&vec![b' '; MAX_DECOMPRESSED_BODY_BYTES]);

        check!(AwsJson10()
            .deserialize_input(&req, &EMPTY_INPUT_SCHEMA)
            .is_ok());
    }
}
