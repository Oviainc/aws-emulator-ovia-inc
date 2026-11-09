// SPDX-License-Identifier: CC0-1.0

//! The protocol this service serves its modelled answers in: the model's own `restJson1`,
//! with the one framing correction an answer that carries NO CONTENT needs.
//!
//! An operation whose output structure declares no member leaves its caller nothing to
//! parse, and the surface that serves it reports such a success with the status alone
//! ([`crate::scaffolded::TAGGING_NO_CONTENT_STATUS`]). The status is stated on the
//! operation (`Operation::SUCCESS_STATUS`); the BODY cannot be, because the protocol's
//! serializer renders a members-less output structure as the empty JSON object (it is a
//! `structure`, not `smithy.api#Unit`, which is the one shape the renderers treat as
//! bodyless) — so an answer that declares it has no content would still carry some.
//!
//! The correction itself is `service_common::no_content_responses::without_content`, the
//! shared reading of that framing — no body, and the length such an answer states for it —
//! and all this module does is take it at the one seam every response a modelled operation
//! produces passes through
//! ([`smithy_server_runtime::Service::override_protocol`], wired in [`crate::service`]).
//! Nothing else about a response is touched: the status, the headers and every body byte of
//! an answer that DOES carry content — every reader, every create, every modelled
//! failure — are its renderer's. The framing header itself is the HTTP writer's last word
//! rather than this crate's: that writer leaves it off a no-content answer on the wire,
//! which the suite's wire probe pins.
//!
//! It is implemented on [`EmulatedService`] itself rather than on a protocol type of its
//! own because the model-derived protocol it delegates to is owned by the generated
//! definition and reached by reference (`self.entry.protocol()`), and that seam hands back
//! a borrow of `&self`: one protocol instance, the generated one, and no second routing
//! table to drift from the model.

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use service_common::no_content_responses::without_content;

use super::EmulatedService;

impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = WireRequest;
    type Response = WireResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a WireRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a WireRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        self.entry
            .protocol()
            .deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        let response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // Only an answer at the no-content status is emptied of the document rendered for
        // it; every other success keeps the body, the status and the headers the protocol
        // wrote.
        Ok(without_content(response))
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<WireResponse, ProtocolError> {
        let response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every refusal of this service carries its error document at a 4xx or 5xx, so
        // nothing here is emptied; the same call is made on both paths rather than leaving
        // one of them to frame an answer differently.
        Ok(without_content(response))
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<WireResponse, ProtocolError> {
        self.entry.protocol().serialize_event_stream_message(
            event_type,
            is_exception,
            event,
            event_schema,
        )
    }
}

/// `ServerProtocol` requires `Debug`; the generated entry is not `Debug`, so this names the
/// protocol being served rather than the service's internals.
impl<Eng: Send + Sync + 'static> std::fmt::Debug for EmulatedService<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatedService")
            .field("protocol", &self.entry.protocol().protocol_id())
            .finish()
    }
}
