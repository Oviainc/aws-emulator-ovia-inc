// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! REST+JSON server protocol implementation (`aws.protocols#restJson1`).
//!
//! This module implements [`ServerProtocol`] for the `restJson1` Smithy
//! protocol. It is the JSON analog of [`crate::rest_xml::RestXmlServerProtocol`]:
//! routing is HTTP method + URI-pattern based (the two REST protocols share the
//! same route table and matcher — see [`crate::rest_xml::route_lookup`]); only
//! the body codec differs. Bodies are serialized/deserialized as JSON via
//! [`JsonCodec::rest_json`], which honours `@jsonName`.
//!
//! HTTP binding (`@httpLabel`, `@httpQuery`, `@httpHeader`,
//! `@httpPrefixHeaders`, `@httpQueryParams`, `@httpResponseCode`) is handled by
//! [`HttpBindingDeserializer`] / [`HttpBindingSerializer`] from
//! [`crate::http_binding`] — the same protocol-agnostic layer restXml uses.
//! Modeled errors are rendered as a JSON envelope carrying a `__type`
//! discriminator, matching the awsJson family.

use crate::http_binding::{HttpBindingDeserializer, HttpBindingSerializer};
use crate::json::JsonCodec;
use crate::protocol::{
    insert_error_response_headers, insert_response_headers, ProtocolError, ServerProtocol,
};
use crate::rest_xml::{
    collect_headers, request_path, request_query, route_lookup, EmptyDeserializer, Route,
};
use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{
    SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer,
};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};
use std::cell::RefCell;

/// REST+JSON server protocol.
///
/// Constructed with a static route table. Implements [`ServerProtocol`] for
/// HTTP method + path routing, JSON body ser/deser (honouring `@jsonName`),
/// and JSON `__type` error envelopes.
#[derive(Debug)]
pub struct RestJsonServerProtocol {
    routes: Vec<Route>,
}

impl RestJsonServerProtocol {
    pub fn new(routes: &[Route]) -> Self {
        Self {
            routes: routes.to_vec(),
        }
    }
}

/// Schema for the `__type` discriminator emitted by JSON error envelopes.
///
/// restJson1 (like the awsJson family) carries an extra string member `__type`
/// whose value is the shape name of the concrete error.
static __TYPE_MEMBER_SCHEMA: Schema = Schema::new_member(
    ShapeId::from_static("smithy.api#__type", "smithy.api", "__type"),
    ShapeType::String,
    "__type",
    0,
)
.with_json_name("__type");

/// Forwards every [`ShapeSerializer`] call to a borrowed
/// `&mut dyn ShapeSerializer`.
///
/// This lets [`HttpBindingSerializer`] wrap the JSON codec's member-key-emitting
/// adapter — which is only reachable *inside*
/// `JsonShapeSerializer::write_struct` — so REST-JSON body members are written
/// with the correct `{ "name": value }` object framing and commas, while
/// `@http*`-bound members are still diverted to headers / status.
struct SerializerRef<'a>(&'a mut dyn ShapeSerializer);

impl<'a> ShapeSerializer for SerializerRef<'a> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.0.write_struct(schema, value)
    }
    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.0.write_list(schema, write_elements)
    }
    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.0.write_map(schema, write_entries)
    }
    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.0.write_boolean(schema, value)
    }
    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.0.write_byte(schema, value)
    }
    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.0.write_short(schema, value)
    }
    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.0.write_integer(schema, value)
    }
    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.0.write_long(schema, value)
    }
    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.0.write_float(schema, value)
    }
    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.0.write_double(schema, value)
    }
    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.0.write_big_integer(schema, value)
    }
    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.0.write_big_decimal(schema, value)
    }
    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.0.write_string(schema, value)
    }
    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.0.write_blob(schema, value)
    }
    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.0.write_timestamp(schema, value)
    }
    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.0.write_document(schema, value)
    }
    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.0.write_null(schema)
    }
}

impl ServerProtocol for RestJsonServerProtocol {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        static ID: ShapeId =
            ShapeId::from_static("aws.protocols#restJson1", "aws.protocols", "restJson1");
        &ID
    }

    fn routing_key<'a>(&self, request: &'a Request) -> Result<&'a str, ProtocolError> {
        let uri = request.uri();
        let path = request_path(uri);
        let query = request_query(uri);
        let method = request.method();
        route_lookup(&self.routes, method, path, query, &|name| {
            request.headers().get(name).is_some()
        })
        .ok_or_else(|| ProtocolError::UnknownOperation(format!("{} {}", method, path)))
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        let body_bytes = request
            .body()
            .bytes()
            .ok_or_else(|| ProtocolError::BodyRead("body is not available as bytes".into()))?;

        let path = request_path(request.uri()).to_string();
        let query = request_query(request.uri()).to_string();
        let headers = collect_headers(request);
        let uri_pattern = input_schema.http().map(|h| h.uri().to_string());

        // `@httpPayload`: the raw body IS one member's value, so the JSON codec
        // must not sweep it as a member object. Wire BOTH sources so
        // `HttpBindingDeserializer::read_struct` can pick per the payload
        // member's target shape: the raw bytes for a scalar (`blob`/`string`)
        // payload, and the real JSON body codec for a structured
        // (`structure`/`union`/`list`/`map`/`document`) payload (parsed as the
        // whole body).
        let has_payload = input_schema
            .members()
            .iter()
            .any(|m| m.http_payload().is_some());
        if has_payload {
            let codec = JsonCodec::rest_json();
            let body = codec.create_deserializer(body_bytes);
            let d =
                HttpBindingDeserializer::new(uri_pattern.as_deref(), &path, &query, headers, body)
                    .with_payload_bytes(body_bytes.to_vec());
            return Ok(Box::new(d));
        }

        // Empty body: still need an HttpBindingDeserializer so HTTP-bound
        // members are read; wrap a no-op body deserializer.
        if body_bytes.is_empty() {
            let body = EmptyDeserializer;
            let d: HttpBindingDeserializer<EmptyDeserializer> =
                HttpBindingDeserializer::new(uri_pattern.as_deref(), &path, &query, headers, body);
            return Ok(Box::new(d));
        }

        let codec = JsonCodec::rest_json();
        let body = codec.create_deserializer(body_bytes);
        let d = HttpBindingDeserializer::new(uri_pattern.as_deref(), &path, &query, headers, body);
        Ok(Box::new(d))
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        _operation_name: &str,
        success_status: u16,
    ) -> Result<Response, ProtocolError> {
        // Classify the output body shape up front:
        //  * a `smithy.api#Unit` output has NO body — emit an empty body, never
        //    a bogus `{}` object (and honor any `@httpResponseCode`);
        //  * a structured `@httpPayload` member (`structure`/`union`/`list`/
        //    `map`/`document`) IS the whole body — serialize just that member's
        //    value as the top-level JSON, with no outer object wrapping it;
        //  * everything else uses the normal JSON object, with a scalar
        //    (`blob`/`string`) `@httpPayload` still diverted to `body_override`.
        let is_unit = crate::http_binding::is_unit_shape(output_schema);
        let structured_payload = output_schema.members().iter().any(|m| {
            m.http_payload().is_some()
                && matches!(
                    m.shape_type(),
                    ShapeType::Structure
                        | ShapeType::Union
                        | ShapeType::List
                        | ShapeType::Map
                        | ShapeType::Document
                )
        });

        if is_unit || structured_payload {
            // Serialize members straight into a fresh JSON serializer through the
            // binder. For Unit no body member exists (empty body). For a
            // structured payload the payload member writes its value as the
            // top-level JSON (`write_struct`/`write_list`/… at root emit the
            // bare value, no key); all other members are HTTP-bound and captured
            // as headers / status.
            let ser = JsonCodec::rest_json().create_serializer();
            let mut binder = HttpBindingSerializer::new(ser);
            output.serialize_members(&mut binder)?;
            let (collected, ser) = binder.finish();
            let json_body = ser.finish();
            let status_code = collected.status_code.unwrap_or(success_status);

            let (body_bytes, content_type) = if is_unit {
                (Vec::new(), "application/json")
            } else {
                // A scalar payload would have set `body_override`; a structured
                // payload's serialized bytes ARE the whole JSON body.
                match collected.body_override {
                    Some((raw, ct)) => (raw, ct),
                    None => (json_body, "application/json"),
                }
            };

            let mut resp = Response::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(status_code).unwrap_or(
                    aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
                ),
                SdkBody::from(body_bytes),
            );
            resp.headers_mut().insert("content-type", content_type);
            insert_response_headers(&mut resp, collected.headers)?;
            return Ok(resp);
        }

        // The JSON codec drives object framing + `@jsonName` key emission via
        // `write_struct`'s internal member adapter. We wrap that adapter with
        // an `HttpBindingSerializer` (through `SerializerRef`) so `@http*`-bound
        // members are diverted to headers / status while body members flow into
        // the JSON object with correct keys and commas. Collected headers /
        // status are surfaced out of the `write_struct` closure via `RefCell`.
        let headers: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let status: RefCell<Option<u16>> = RefCell::new(None);
        // `@httpPayload`: when set, these raw bytes are the entire response body
        // (with the content-type its target type maps to).
        let body_override: RefCell<Option<(Vec<u8>, &'static str)>> = RefCell::new(None);

        struct BodyView<'a> {
            inner: &'a dyn SerializableStruct,
            headers: &'a RefCell<Vec<(String, String)>>,
            status: &'a RefCell<Option<u16>>,
            body_override: &'a RefCell<Option<(Vec<u8>, &'static str)>>,
        }
        impl<'a> SerializableStruct for BodyView<'a> {
            fn serialize_members(
                &self,
                serializer: &mut dyn ShapeSerializer,
            ) -> Result<(), SerdeError> {
                let mut binder = HttpBindingSerializer::new(SerializerRef(serializer));
                self.inner.serialize_members(&mut binder)?;
                let (collected, _sr) = binder.finish();
                *self.headers.borrow_mut() = collected.headers;
                *self.status.borrow_mut() = collected.status_code;
                *self.body_override.borrow_mut() = collected.body_override;
                Ok(())
            }
        }

        let mut ser = JsonCodec::rest_json().create_serializer();
        ser.write_struct(
            output_schema,
            &BodyView {
                inner: output,
                headers: &headers,
                status: &status,
                body_override: &body_override,
            },
        )?;
        let json_body = ser.finish();

        let status_code = status.into_inner().unwrap_or(success_status);

        // A payload member overrides the whole body with the content-type its
        // target type maps to (blob → octet-stream, string → text/plain);
        // otherwise emit the JSON object.
        let (body_bytes, content_type) = match body_override.into_inner() {
            Some((raw, ct)) => (raw, ct),
            None => (json_body, "application/json"),
        };

        let mut resp = Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status_code).unwrap_or(
                aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            ),
            SdkBody::from(body_bytes),
        );
        resp.headers_mut().insert("content-type", content_type);
        insert_response_headers(&mut resp, headers.into_inner())?;
        Ok(resp)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Response, ProtocolError> {
        // JSON error envelope: `{"__type":"<Code>", ...body members}`. The
        // `__type` discriminator is emitted first; the remaining members flow
        // through the same header/body split as a normal output so any
        // `@httpHeader`-bound error members are routed to response headers.
        let headers: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());

        struct ErrorBody<'a> {
            error_code: &'a str,
            inner: &'a dyn SerializableStruct,
            headers: &'a RefCell<Vec<(String, String)>>,
        }
        impl<'a> SerializableStruct for ErrorBody<'a> {
            fn serialize_members(
                &self,
                serializer: &mut dyn ShapeSerializer,
            ) -> Result<(), SerdeError> {
                serializer.write_string(&__TYPE_MEMBER_SCHEMA, self.error_code)?;
                let mut binder = HttpBindingSerializer::new(SerializerRef(serializer));
                self.inner.serialize_members(&mut binder)?;
                let (collected, _sr) = binder.finish();
                *self.headers.borrow_mut() = collected.headers;
                Ok(())
            }
        }

        let mut ser = JsonCodec::rest_json().create_serializer();
        ser.write_struct(
            error_schema,
            &ErrorBody {
                error_code,
                inner: error,
                headers: &headers,
            },
        )?;
        let body_bytes = ser.finish();

        let status = aws_smithy_runtime_api::http::StatusCode::try_from(http_status)
            .unwrap_or(aws_smithy_runtime_api::http::StatusCode::try_from(400u16).expect("400"));
        let mut resp = Response::new(status, SdkBody::from(body_bytes));
        resp.headers_mut()
            .insert("content-type", "application/json");
        // restJson1 resolves an error SHAPE from this header rather than from the
        // body. Set it here as well as on the pre-dispatch path, or one service
        // answers its two error paths with two different shape-resolution
        // contracts: a MODELED error carrying only `__type` and a FRAMEWORK error
        // carrying the header too. Most real 4xx responses carry it. Inserted BEFORE
        // the `@httpHeader`-bound
        // members below, so a model that binds a member to this header still wins.
        //
        // Set FALLIBLY. `error_code` is a `&str` parameter of a public trait method, and
        // a header value rejects control bytes that a JSON string accepts, so an
        // unchecked insert would panic on the ERROR path — where a panic becomes a 500
        // an SDK retries. On a rejected value the header is omitted and the body's
        // `__type` still carries the shape. `aws_json_error_response` guards the same
        // header for the same reason.
        if http::HeaderValue::from_str(error_code).is_ok() {
            resp.headers_mut()
                .insert("x-amzn-errortype", error_code.to_string());
        }
        insert_error_response_headers(&mut resp, headers.into_inner());
        Ok(resp)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Response, ProtocolError> {
        // REST+JSON event streams frame each `@streaming` union variant as a
        // `vnd.amazon.eventstream` message whose payload is the JSON-encoded
        // event, mirroring the awsJson framing in `rpc.rs` (same header
        // contract the SDK decoder expects) but with the restJson1 JSON codec.
        use aws_smithy_eventstream::frame::write_message_to;
        use aws_smithy_types::event_stream::{Header, HeaderValue, Message};

        let mut serializer = JsonCodec::rest_json().create_serializer();
        serializer.write_struct(event_schema, event)?;
        let payload = serializer.finish();

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
                .add_header(Header::new(
                    ":content-type",
                    HeaderValue::String("application/json".into()),
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
            ProtocolError::Serde(SerdeError::custom(format!(
                "event-stream frame encode failed: {e}"
            )))
        })?;

        let mut resp = Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200 is valid"),
            SdkBody::from(frame),
        );
        resp.headers_mut()
            .insert("content-type", "application/vnd.amazon.eventstream");
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::Request;
    use aws_smithy_types::base64;
    use aws_smithy_types::Blob;

    // ---------------------------------------------------------------------
    // A minimal restJson1 "CreateFunction" model, hand-built as schemas.
    //
    // Input : POST /2015-03-31/functions/{FunctionName}
    //   FunctionName  String  @httpLabel
    //   Description   String  body  @jsonName("Description")
    //   ZipFile       Blob    body  @jsonName("ZipFile")   (base64 in JSON)
    // Output:
    //   FunctionName  String  body  @jsonName("FunctionName")
    //   Runtime       String  @httpHeader("X-Runtime")
    //   CodeSize      Integer body  @jsonName("CodeSize")
    // Error : ResourceNotFoundException  (@httpError 404)
    //   Message       String  body  @jsonName("Message")
    // ---------------------------------------------------------------------

    static IN_FUNCTION_NAME: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CreateFunctionInput$FunctionName",
            "test#CreateFunctionInput",
            "FunctionName",
        ),
        ShapeType::String,
        "FunctionName",
        0,
    )
    .with_http_label();
    static IN_DESCRIPTION: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CreateFunctionInput$Description",
            "test#CreateFunctionInput",
            "Description",
        ),
        ShapeType::String,
        "Description",
        1,
    )
    .with_json_name("Description");
    static IN_ZIP_FILE: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CreateFunctionInput$ZipFile",
            "test#CreateFunctionInput",
            "ZipFile",
        ),
        ShapeType::Blob,
        "ZipFile",
        2,
    )
    .with_json_name("ZipFile");
    static IN_MEMBERS: &[&Schema] = &[&IN_FUNCTION_NAME, &IN_DESCRIPTION, &IN_ZIP_FILE];
    static INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#CreateFunctionInput", "test", "CreateFunctionInput"),
        ShapeType::Structure,
        IN_MEMBERS,
    )
    .with_http(aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/2015-03-31/functions/{FunctionName}",
        None,
    ));

    static OUT_FUNCTION_NAME: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CreateFunctionOutput$FunctionName",
            "test#CreateFunctionOutput",
            "FunctionName",
        ),
        ShapeType::String,
        "FunctionName",
        0,
    )
    .with_json_name("FunctionName");
    static OUT_RUNTIME: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CreateFunctionOutput$Runtime",
            "test#CreateFunctionOutput",
            "Runtime",
        ),
        ShapeType::String,
        "Runtime",
        1,
    )
    .with_http_header("X-Runtime");
    static OUT_CODE_SIZE: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CreateFunctionOutput$CodeSize",
            "test#CreateFunctionOutput",
            "CodeSize",
        ),
        ShapeType::Integer,
        "CodeSize",
        2,
    )
    .with_json_name("CodeSize");
    static OUT_MEMBERS: &[&Schema] = &[&OUT_FUNCTION_NAME, &OUT_RUNTIME, &OUT_CODE_SIZE];
    static OUTPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#CreateFunctionOutput", "test", "CreateFunctionOutput"),
        ShapeType::Structure,
        OUT_MEMBERS,
    );

    static ERR_MESSAGE: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#ResourceNotFoundException$Message",
            "test#ResourceNotFoundException",
            "Message",
        ),
        ShapeType::String,
        "Message",
        0,
    )
    .with_json_name("Message");
    static ERR_MEMBERS: &[&Schema] = &[&ERR_MESSAGE];
    static ERROR_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "test#ResourceNotFoundException",
            "test",
            "ResourceNotFoundException",
        ),
        ShapeType::Structure,
        ERR_MEMBERS,
    );

    fn protocol() -> RestJsonServerProtocol {
        RestJsonServerProtocol::new(&[
            Route::new(
                "POST",
                "/2015-03-31/functions/{FunctionName}",
                "CreateFunction",
            ),
            Route::new("GET", "/2015-03-31/functions/{FunctionName}", "GetFunction"),
        ])
    }

    #[test]
    fn routing_key_matches_method_and_uri_pattern() {
        let proto = protocol();
        let mut req = Request::new(SdkBody::empty());
        req.set_uri("/2015-03-31/functions/my-func").unwrap();
        req.set_method("POST").unwrap();
        assert_eq!(proto.routing_key(&req).unwrap(), "CreateFunction");

        let mut get = Request::new(SdkBody::empty());
        get.set_uri("/2015-03-31/functions/my-func").unwrap();
        get.set_method("GET").unwrap();
        assert_eq!(proto.routing_key(&get).unwrap(), "GetFunction");
    }

    #[test]
    fn routing_key_unknown_route_errors() {
        let proto = protocol();
        let mut req = Request::new(SdkBody::empty());
        req.set_uri("/nope").unwrap();
        req.set_method("POST").unwrap();
        assert!(proto.routing_key(&req).is_err());
    }

    #[test]
    fn deserialize_input_reads_http_label_and_json_body_with_base64_blob() {
        let proto = protocol();
        let zip_bytes: &[u8] = &[0x50, 0x4b, 0x03, 0x04, 0x00, 0xff, 0x7f];
        let body = format!(
            r#"{{"Description":"my function","ZipFile":"{}"}}"#,
            base64::encode(zip_bytes)
        );
        let mut req = Request::new(SdkBody::from(body));
        req.set_uri("/2015-03-31/functions/my-func").unwrap();
        req.set_method("POST").unwrap();

        let mut deser = proto
            .deserialize_input(&req, &INPUT_SCHEMA)
            .expect("deserialize_input");

        let mut function_name = None;
        let mut description = None;
        let mut zip_file: Option<Blob> = None;
        deser
            .read_struct(&INPUT_SCHEMA, &mut |member, field| {
                match member.member_name() {
                    Some("FunctionName") => function_name = Some(field.read_string(member)?),
                    Some("Description") => description = Some(field.read_string(member)?),
                    Some("ZipFile") => zip_file = Some(field.read_blob(member)?),
                    _ => {}
                }
                Ok(())
            })
            .expect("read_struct");

        assert_eq!(function_name.as_deref(), Some("my-func"));
        assert_eq!(description.as_deref(), Some("my function"));
        assert_eq!(zip_file.expect("zip file").as_ref(), zip_bytes);
    }

    struct CreateFunctionOutput {
        function_name: String,
        runtime: String,
        code_size: i32,
    }
    impl SerializableStruct for CreateFunctionOutput {
        fn serialize_members(
            &self,
            serializer: &mut dyn ShapeSerializer,
        ) -> Result<(), SerdeError> {
            serializer.write_string(&OUT_FUNCTION_NAME, &self.function_name)?;
            serializer.write_string(&OUT_RUNTIME, &self.runtime)?;
            serializer.write_integer(&OUT_CODE_SIZE, self.code_size)?;
            Ok(())
        }
    }

    #[test]
    fn serialize_output_writes_json_body_and_routes_http_header() {
        let proto = protocol();
        let out = CreateFunctionOutput {
            function_name: "my-func".to_string(),
            runtime: "python3.12".to_string(),
            code_size: 42,
        };
        let resp = proto
            .serialize_output(&out, &OUTPUT_SCHEMA, "TestOp", 200)
            .expect("serialize_output");

        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(resp.headers().get("content-type"), Some("application/json"));
        // @httpHeader member routed to a real header, not the body.
        assert_eq!(resp.headers().get("X-Runtime"), Some("python3.12"));

        let body = std::str::from_utf8(resp.body().bytes().unwrap()).unwrap();
        assert_eq!(body, r#"{"FunctionName":"my-func","CodeSize":42}"#);
        assert!(
            !body.contains("python3.12"),
            "header leaked into body: {body}"
        );
        assert!(
            !body.contains("Runtime"),
            "header key leaked into body: {body}"
        );
    }

    struct ResourceNotFound {
        message: String,
    }
    impl SerializableStruct for ResourceNotFound {
        fn serialize_members(
            &self,
            serializer: &mut dyn ShapeSerializer,
        ) -> Result<(), SerdeError> {
            serializer.write_string(&ERR_MESSAGE, &self.message)?;
            Ok(())
        }
    }

    #[test]
    fn serialize_error_emits_json_type_envelope() {
        let proto = protocol();
        let err = ResourceNotFound {
            message: "no such function".to_string(),
        };
        let resp = proto
            .serialize_error(&err, &ERROR_SCHEMA, "ResourceNotFoundException", 404)
            .expect("serialize_error");

        assert_eq!(resp.status().as_u16(), 404);
        assert_eq!(resp.headers().get("content-type"), Some("application/json"));
        // restJson1 resolves the error SHAPE from this header. A MODELED error must
        // carry it for the same reason a framework error does, and with the SAME
        // value as the body's `__type` — otherwise one service answers its two error
        // paths with two different shape-resolution contracts.
        assert_eq!(
            resp.headers().get("x-amzn-errortype"),
            Some("ResourceNotFoundException")
        );

        let body = std::str::from_utf8(resp.body().bytes().unwrap()).unwrap();
        assert_eq!(
            body,
            r#"{"__type":"ResourceNotFoundException","Message":"no such function"}"#
        );
    }

    /// An unrepresentable error code must OMIT the header, never panic.
    ///
    /// `error_code` is a `&str` on a public trait method, and a header value rejects
    /// control bytes a JSON string accepts. A panic here lands on the error path, where
    /// it becomes a 500 an SDK retries — strictly worse than the 4xx it replaces. The
    /// body's `__type` still carries the shape.
    #[test]
    fn an_unrepresentable_error_code_omits_the_header_instead_of_panicking() {
        let proto = protocol();
        let err = ResourceNotFound {
            message: "no such function".to_string(),
        };
        for hostile in ["Bad\u{0}Code", "Bad\nCode", "Bad\u{7f}Code"] {
            let resp = proto
                .serialize_error(&err, &ERROR_SCHEMA, hostile, 404)
                .expect("serialize_error must not fail");
            assert_eq!(resp.status().as_u16(), 404);
            assert_eq!(
                resp.headers().get("x-amzn-errortype"),
                None,
                "{hostile:?} is not a legal header value and must be omitted"
            );
            let body = std::str::from_utf8(resp.body().bytes().unwrap()).unwrap();
            assert!(
                body.contains("__type"),
                "the shape must still be resolvable from the body: {body}"
            );
        }
    }

    #[test]
    fn protocol_id_is_rest_json1() {
        assert_eq!(
            protocol().protocol_id().to_string(),
            "aws.protocols#restJson1"
        );
    }

    // ---------------------------------------------------------------------
    // @httpPayload: the raw body IS one blob member (no JSON wrapping).
    // ---------------------------------------------------------------------

    static PAYLOAD_FN_NAME: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#InvokeInput$FunctionName",
            "test#InvokeInput",
            "FunctionName",
        ),
        ShapeType::String,
        "FunctionName",
        0,
    )
    .with_http_label();
    static PAYLOAD_MEMBER: Schema = Schema::new_member(
        ShapeId::from_static("test#InvokeInput$Payload", "test#InvokeInput", "Payload"),
        ShapeType::Blob,
        "Payload",
        1,
    )
    .with_http_payload();
    static PAYLOAD_IN_MEMBERS: &[&Schema] = &[&PAYLOAD_FN_NAME, &PAYLOAD_MEMBER];
    static PAYLOAD_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#InvokeInput", "test", "InvokeInput"),
        ShapeType::Structure,
        PAYLOAD_IN_MEMBERS,
    )
    .with_http(aws_smithy_schema::traits::HttpTrait::new(
        "POST",
        "/2015-03-31/functions/{FunctionName}/invocations",
        None,
    ));

    static PAYLOAD_OUT_MEMBER: Schema = Schema::new_member(
        ShapeId::from_static("test#InvokeOutput$Payload", "test#InvokeOutput", "Payload"),
        ShapeType::Blob,
        "Payload",
        0,
    )
    .with_http_payload();
    static PAYLOAD_OUT_MEMBERS: &[&Schema] = &[&PAYLOAD_OUT_MEMBER];
    static PAYLOAD_OUTPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#InvokeOutput", "test", "InvokeOutput"),
        ShapeType::Structure,
        PAYLOAD_OUT_MEMBERS,
    );

    #[test]
    fn deserialize_input_binds_raw_http_payload_body() {
        let proto = RestJsonServerProtocol::new(&[Route::new(
            "POST",
            "/2015-03-31/functions/{FunctionName}/invocations",
            "Invoke",
        )]);

        // Raw, non-JSON body — must be bound verbatim to the Payload blob.
        let raw = b"\x00\x01\x02 raw not json {".to_vec();
        let mut req = Request::new(SdkBody::from(raw.clone()));
        req.set_uri("/2015-03-31/functions/my-func/invocations")
            .unwrap();
        req.set_method("POST").unwrap();

        let mut de = proto
            .deserialize_input(&req, &PAYLOAD_INPUT_SCHEMA)
            .expect("deserialize_input");

        let mut function_name: Option<String> = None;
        let mut payload: Option<Blob> = None;
        de.read_struct(&PAYLOAD_INPUT_SCHEMA, &mut |member, d| {
            match member.member_name() {
                Some("FunctionName") => function_name = Some(d.read_string(member)?),
                Some("Payload") => payload = Some(d.read_blob(member)?),
                _ => {}
            }
            Ok(())
        })
        .expect("read_struct");

        assert_eq!(function_name.as_deref(), Some("my-func"));
        assert_eq!(
            payload.expect("payload").as_ref(),
            raw.as_slice(),
            "raw @httpPayload body must bind verbatim (no base64, no JSON parse)"
        );
    }

    struct InvokeOutput {
        payload: Vec<u8>,
    }
    impl SerializableStruct for InvokeOutput {
        fn serialize_members(
            &self,
            serializer: &mut dyn ShapeSerializer,
        ) -> Result<(), SerdeError> {
            serializer.write_blob(&PAYLOAD_OUT_MEMBER, &self.payload)?;
            Ok(())
        }
    }

    #[test]
    fn serialize_output_emits_raw_http_payload_as_whole_body() {
        let proto = protocol();
        let raw = b"\xDE\xAD\xBE\xEF raw response body".to_vec();
        let out = InvokeOutput {
            payload: raw.clone(),
        };
        let resp = proto
            .serialize_output(&out, &PAYLOAD_OUTPUT_SCHEMA, "TestOp", 200)
            .expect("serialize_output");

        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            resp.headers().get("content-type"),
            Some("application/octet-stream"),
            "raw @httpPayload output must not be application/json"
        );
        assert_eq!(
            resp.body().bytes().unwrap(),
            raw.as_slice(),
            "response body must be the raw payload bytes, not a JSON object"
        );
    }

    static STR_PAYLOAD_OUT_MEMBER: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#StrInvokeOutput$Payload",
            "test#StrInvokeOutput",
            "Payload",
        ),
        ShapeType::String,
        "Payload",
        0,
    )
    .with_http_payload();
    static STR_PAYLOAD_OUT_MEMBERS: &[&Schema] = &[&STR_PAYLOAD_OUT_MEMBER];
    static STR_PAYLOAD_OUTPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#StrInvokeOutput", "test", "StrInvokeOutput"),
        ShapeType::Structure,
        STR_PAYLOAD_OUT_MEMBERS,
    );

    struct StrInvokeOutput {
        payload: String,
    }
    impl SerializableStruct for StrInvokeOutput {
        fn serialize_members(
            &self,
            serializer: &mut dyn ShapeSerializer,
        ) -> Result<(), SerdeError> {
            serializer.write_string(&STR_PAYLOAD_OUT_MEMBER, &self.payload)?;
            Ok(())
        }
    }

    #[test]
    fn serialize_output_string_http_payload_is_text_plain() {
        let proto = protocol();
        let out = StrInvokeOutput {
            payload: "just some plain text, not json".to_string(),
        };
        let resp = proto
            .serialize_output(&out, &STR_PAYLOAD_OUTPUT_SCHEMA, "TestOp", 200)
            .expect("serialize_output");

        // Per the Smithy spec a string `@httpPayload` maps to `text/plain`, not
        // `application/octet-stream` (blob) or `application/json` (object).
        assert_eq!(
            resp.headers().get("content-type"),
            Some("text/plain"),
            "string @httpPayload must be text/plain"
        );
        assert_eq!(
            resp.body().bytes().unwrap(),
            b"just some plain text, not json".as_slice(),
            "string payload body must be the raw UTF-8 bytes, not JSON-quoted"
        );
    }

    // ---------------------------------------------------------------------
    // @streaming: each event variant frames as a vnd.amazon.eventstream message.
    // ---------------------------------------------------------------------

    static EVENT_DATA: Schema = Schema::new_member(
        ShapeId::from_static("test#Chunk$Data", "test#Chunk", "Data"),
        ShapeType::String,
        "Data",
        0,
    )
    .with_json_name("Data");
    static EVENT_MEMBERS: &[&Schema] = &[&EVENT_DATA];
    static EVENT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#Chunk", "test", "Chunk"),
        ShapeType::Structure,
        EVENT_MEMBERS,
    );

    struct Chunk {
        data: String,
    }
    impl SerializableStruct for Chunk {
        fn serialize_members(
            &self,
            serializer: &mut dyn ShapeSerializer,
        ) -> Result<(), SerdeError> {
            serializer.write_string(&EVENT_DATA, &self.data)?;
            Ok(())
        }
    }

    #[test]
    fn serialize_event_stream_message_frames_json_event() {
        use aws_smithy_eventstream::frame::read_message_from;
        use aws_smithy_types::event_stream::HeaderValue;

        let proto = protocol();
        let event = Chunk {
            data: "hello".to_string(),
        };
        let resp = proto
            .serialize_event_stream_message("Chunk", false, &event, &EVENT_SCHEMA)
            .expect("serialize_event_stream_message");

        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            resp.headers().get("content-type"),
            Some("application/vnd.amazon.eventstream")
        );

        // Decode the single frame and verify the AWS event-stream headers +
        // JSON payload the SDK decoder expects.
        let frame_bytes = resp.body().bytes().unwrap().to_vec();
        let msg = read_message_from(&mut bytes::Bytes::from(frame_bytes))
            .expect("frame must decode as a valid event-stream message");

        let header = |name: &str| {
            msg.headers()
                .iter()
                .find(|h| h.name().as_str() == name)
                .map(|h| match h.value() {
                    HeaderValue::String(s) => s.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        assert_eq!(header(":message-type").as_deref(), Some("event"));
        assert_eq!(header(":event-type").as_deref(), Some("Chunk"));
        assert_eq!(header(":content-type").as_deref(), Some("application/json"));

        let payload = std::str::from_utf8(msg.payload()).unwrap();
        assert_eq!(payload, r#"{"Data":"hello"}"#);
    }

    #[test]
    fn serialize_event_stream_message_frames_exception() {
        use aws_smithy_eventstream::frame::read_message_from;
        use aws_smithy_types::event_stream::HeaderValue;

        let proto = protocol();
        let event = Chunk {
            data: "boom".to_string(),
        };
        let resp = proto
            .serialize_event_stream_message("ThrottlingError", true, &event, &EVENT_SCHEMA)
            .expect("serialize_event_stream_message");

        let frame_bytes = resp.body().bytes().unwrap().to_vec();
        let msg = read_message_from(&mut bytes::Bytes::from(frame_bytes)).expect("decode frame");
        let header = |name: &str| {
            msg.headers()
                .iter()
                .find(|h| h.name().as_str() == name)
                .map(|h| match h.value() {
                    HeaderValue::String(s) => s.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        assert_eq!(header(":message-type").as_deref(), Some("exception"));
        assert_eq!(
            header(":exception-type").as_deref(),
            Some("ThrottlingError")
        );
    }

    // =====================================================================
    // struct / Unit `@httpPayload` (this change)
    //
    // The REST-JSON side of the fix: a `structure` (and union/list/map/
    // document) `@httpPayload` body is the WHOLE JSON body (parsed on the
    // request, emitted as the top-level JSON on the response — no wrapping
    // object), and a `smithy.api#Unit` output carries no body.
    // =====================================================================

    // --- struct `@httpPayload` INPUT: PUT /things/{Name} with a JSON config body ---
    static CFG_STATUS: Schema = Schema::new_member(
        ShapeId::from_static("test#ThingConfig$Status", "test#ThingConfig", "Status"),
        ShapeType::String,
        "Status",
        0,
    )
    .with_json_name("Status");
    static CFG_MEMBERS: &[&Schema] = &[&CFG_STATUS];
    static CFG_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#ThingConfig", "test", "ThingConfig"),
        ShapeType::Structure,
        CFG_MEMBERS,
    );
    static PT_NAME: Schema = Schema::new_member(
        ShapeId::from_static("test#PutThingInput$Name", "test#PutThingInput", "Name"),
        ShapeType::String,
        "Name",
        0,
    )
    .with_http_label();
    static PT_CONFIG: Schema = Schema::new_member(
        ShapeId::from_static("test#PutThingInput$Config", "test#PutThingInput", "Config"),
        ShapeType::Structure,
        "Config",
        1,
    )
    .with_http_payload();
    static PT_IN_MEMBERS: &[&Schema] = &[&PT_NAME, &PT_CONFIG];
    static PT_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#PutThingInput", "test", "PutThingInput"),
        ShapeType::Structure,
        PT_IN_MEMBERS,
    )
    .with_http(aws_smithy_schema::traits::HttpTrait::new(
        "PUT",
        "/things/{Name}",
        None,
    ));

    #[test]
    fn deserialize_input_parses_struct_http_payload_from_json_body() {
        let proto = RestJsonServerProtocol::new(&[Route::new("PUT", "/things/{Name}", "PutThing")]);
        let mut req = Request::new(SdkBody::from(r#"{"Status":"Enabled"}"#));
        req.set_uri("/things/widget").unwrap();
        req.set_method("PUT").unwrap();

        let mut de = proto
            .deserialize_input(&req, &PT_INPUT_SCHEMA)
            .expect("deserialize_input");

        let mut name: Option<String> = None;
        let mut status: Option<String> = None;
        de.read_struct(&PT_INPUT_SCHEMA, &mut |member, field| {
            match member.member_name() {
                Some("Name") => name = Some(field.read_string(member)?),
                Some("Config") => {
                    field.read_struct(&CFG_SCHEMA, &mut |m, d| {
                        if m.member_name() == Some("Status") {
                            status = Some(d.read_string(m)?);
                        }
                        Ok(())
                    })?;
                }
                _ => {}
            }
            Ok(())
        })
        .expect("read_struct must succeed for a struct @httpPayload");

        assert_eq!(name.as_deref(), Some("widget"));
        assert_eq!(
            status.as_deref(),
            Some("Enabled"),
            "struct @httpPayload JSON body must parse into the target struct"
        );
    }

    // --- struct `@httpPayload` OUTPUT: the payload struct IS the whole JSON body ---
    static OUT_CFG_STATUS: Schema = Schema::new_member(
        ShapeId::from_static("test#ThingResult$Status", "test#ThingResult", "Status"),
        ShapeType::String,
        "Status",
        0,
    )
    .with_json_name("Status");
    static OUT_CFG_MEMBERS: &[&Schema] = &[&OUT_CFG_STATUS];
    static OUT_CFG_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#ThingResult", "test", "ThingResult"),
        ShapeType::Structure,
        OUT_CFG_MEMBERS,
    );
    static PT_OUT_RESULT: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#PutThingOutput$Result",
            "test#PutThingOutput",
            "Result",
        ),
        ShapeType::Structure,
        "Result",
        0,
    )
    .with_http_payload();
    static PT_OUT_MEMBERS: &[&Schema] = &[&PT_OUT_RESULT];
    static PT_OUTPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#PutThingOutput", "test", "PutThingOutput"),
        ShapeType::Structure,
        PT_OUT_MEMBERS,
    );

    struct ThingResult {
        status: String,
    }
    impl SerializableStruct for ThingResult {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_string(&OUT_CFG_STATUS, &self.status)?;
            let _ = &OUT_CFG_SCHEMA;
            Ok(())
        }
    }
    struct PutThingOutput {
        result: ThingResult,
    }
    impl SerializableStruct for PutThingOutput {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_struct(&PT_OUT_RESULT, &self.result)
        }
    }

    #[test]
    fn serialize_output_emits_struct_http_payload_as_whole_json_body() {
        let proto = protocol();
        let out = PutThingOutput {
            result: ThingResult {
                status: "Enabled".to_string(),
            },
        };
        let resp = proto
            .serialize_output(&out, &PT_OUTPUT_SCHEMA, "TestOp", 200)
            .expect("serialize_output");

        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(resp.headers().get("content-type"), Some("application/json"));
        let body = std::str::from_utf8(resp.body().bytes().unwrap()).unwrap();
        assert_eq!(
            body, r#"{"Status":"Enabled"}"#,
            "struct @httpPayload output must be the payload struct's own JSON at top level, \
             NOT wrapped in an outer {{\"Result\":…}} object"
        );
    }

    // --- Unit output: empty body ---
    static JSON_UNIT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ShapeType::Structure,
        &[],
    );

    struct JsonUnit;
    impl SerializableStruct for JsonUnit {
        fn serialize_members(&self, _s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            Ok(())
        }
    }

    #[test]
    fn serialize_output_unit_emits_empty_body_and_honors_success_status() {
        let proto = protocol();
        let resp = proto
            .serialize_output(&JsonUnit, &JSON_UNIT_SCHEMA, "TestOp", 204)
            .expect("serialize_output");
        assert!(
            resp.body().bytes().unwrap().is_empty(),
            "a smithy.api#Unit output must have an EMPTY body, not a bogus {{}} object"
        );
        assert_eq!(
            resp.status().as_u16(),
            204,
            "the operation's success status must be the response status"
        );
    }

    // --- regression: blob `@httpPayload` INPUT still binds raw bytes ---
    #[test]
    fn deserialize_input_blob_http_payload_still_binds_raw_bytes_json() {
        let proto = RestJsonServerProtocol::new(&[Route::new(
            "POST",
            "/2015-03-31/functions/{FunctionName}/invocations",
            "Invoke",
        )]);
        let raw = b"\x00\x01 not json {".to_vec();
        let mut req = Request::new(SdkBody::from(raw.clone()));
        req.set_uri("/2015-03-31/functions/f/invocations").unwrap();
        req.set_method("POST").unwrap();

        let mut de = proto
            .deserialize_input(&req, &PAYLOAD_INPUT_SCHEMA)
            .expect("deserialize_input");
        let mut payload: Option<Blob> = None;
        de.read_struct(&PAYLOAD_INPUT_SCHEMA, &mut |member, d| {
            if member.member_name() == Some("Payload") {
                payload = Some(d.read_blob(member)?);
            }
            Ok(())
        })
        .expect("read_struct");
        assert_eq!(
            payload.expect("payload").as_ref(),
            raw.as_slice(),
            "blob @httpPayload must still bind the raw body verbatim (no regression)"
        );
    }
}
