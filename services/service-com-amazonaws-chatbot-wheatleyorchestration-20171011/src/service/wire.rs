// SPDX-License-Identifier: CC0-1.0

//! How this service's `restJson1` ERROR document carries a modelled exception's `Message`
//! member.
//!
//! RULE: every error document AWS Chatbot answers CARRIES the error shape's `Message`
//! member — the sentence the error class words, or JSON `null` where that class words none —
//! so a client can tell "this error class states no message" from "the member was not
//! reported at all".
//!
//! Every exception this service's model declares carries exactly one optional member,
//! `Message` (target `ErrorMessage`, a plain string with no HTTP binding:
//! `smithy_query(query="shape",shape="ResourceNotFoundException,InvalidRequestException",
//! model_path="services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/smithy-model/com-amazonaws-chatbot-wheatleyorchestration-20171011.json")`),
//! and some of this service's refusals state no message at all — a custom action's delete
//! addressed to an ARN holding none, and a tagging request refused for the resource's tag
//! total. The framework's generic serializer writes an absent optional member by writing
//! NOTHING, which is right for a success body but drops the key out of the error document:
//! an SDK then reports that error's message as the empty string it substitutes for an absent
//! key, where the service's own answer reads as "none".
//!
//! [`MessageAlwaysPresentRestJson`] is the protocol `service.rs` hands the dispatch sequence
//! through [`smithy_server_runtime::Service::override_protocol`], so EVERY modelled failure
//! of EVERY operation goes out with the member present, and no handler, shape declaration or
//! error constructor mentions the rendering at all. Everything else — routing, request
//! deserialization, success bodies, the status, `__type`, `x-amzn-errortype` and an error
//! whose message IS stated — is the framework's own restJson1 protocol, delegated to
//! unchanged.
//!
//! A framework-level refusal rendered before any handler ran (a value outside a modelled
//! bound, an authentication failure) does not come through this seam, which is correct: that
//! message is built from the request and is never absent (see `override_refusal` in the
//! parent module).

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::RestJsonServerProtocol;
use aws_smithy_types::body::SdkBody;
use serde_json::Value;

/// The member names an error document's message can occupy: the spelling this service's
/// model declares (`Message`), and the lowercase one the JSON error documents of other AWS
/// services use for the same member, accepted so a shape that ever declares it is covered by
/// the same rule.
const MESSAGE_MEMBER_NAMES: &[&str] = &["Message", "message"];

/// The error shape's message member, or `None` for a shape that declares none.
///
/// Only a plain string member of the BODY qualifies: a member bound to a header, to the
/// query or to the payload is not part of the JSON object this rule is about, and writing
/// `null` for it would mean something else entirely.
fn message_member_name(error_schema: &Schema) -> Option<&'static str> {
    error_schema.members().iter().copied().find_map(|member| {
        let name = MESSAGE_MEMBER_NAMES
            .iter()
            .copied()
            .find(|name| member.member_name() == Some(name))?;
        (matches!(member.shape_type(), ShapeType::String)
            && member.http_header().is_none()
            && member.http_payload().is_none()
            && member.http_query().is_none())
        .then_some(name)
    })
}

/// `body` with `member` added as JSON `null`, or `None` when there is nothing to add: the
/// body is not a JSON object, or it already carries the member (whatever value the error
/// stated, including an empty string, which is a STATED message and not this case).
fn with_null_message(body: &[u8], member: &str) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    if object.contains_key(member) {
        return None;
    }
    object.insert(member.to_string(), Value::Null);
    serde_json::to_vec(&document).ok()
}

/// This service's wire protocol: `restJson1` exactly as the model declares it, with an error
/// document that always carries the error shape's `Message` member.
#[derive(Debug)]
pub struct MessageAlwaysPresentRestJson {
    inner: RestJsonServerProtocol,
}

impl MessageAlwaysPresentRestJson {
    /// Build the protocol once, at service construction: `override_protocol` hands back a
    /// borrow of it and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's operation
    /// through the generated `ServiceDefinition`'s own protocol (whose table is the model's);
    /// an author-overridden protocol drives only the modelled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error` — none of which consults
    /// routes.
    pub fn new() -> Self {
        Self {
            inner: RestJsonServerProtocol::new(&[]),
        }
    }
}

impl Default for MessageAlwaysPresentRestJson {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerProtocol for MessageAlwaysPresentRestJson {
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
        let mut resp = self
            .inner
            .serialize_error(error, error_schema, error_code, http_status)?;
        // A shape with no message member has nothing to keep present, and a body the
        // renderer did not hold in memory is left exactly as it wrote it.
        let Some(member) = message_member_name(error_schema) else {
            return Ok(resp);
        };
        let Some(rewritten) = resp.body().bytes().and_then(|bytes| with_null_message(bytes, member))
        else {
            return Ok(resp);
        };
        *resp.body_mut() = SdkBody::from(rewritten);
        Ok(resp)
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
    use super::*;

    /// The rule over the bodies the renderer can hand this seam: a stated message survives
    /// verbatim (the empty string included — a stated empty message is NOT "none"), and a
    /// body that carries no member at all gains it as an explicit null.
    #[test]
    fn a_stated_message_survives_and_an_absent_one_becomes_an_explicit_null() {
        let table: [(&str, &str, Option<Value>); 5] = [
            (
                "no member at all",
                r#"{"__type":"ResourceNotFoundException"}"#,
                Some(Value::Null),
            ),
            (
                "a stated message",
                r#"{"__type":"ResourceNotFoundException","Message":"sdk action [a] not found"}"#,
                None,
            ),
            (
                "a stated empty message",
                r#"{"__type":"InvalidRequestException","Message":""}"#,
                None,
            ),
            (
                "the member already null",
                r#"{"__type":"InvalidRequestException","Message":null}"#,
                None,
            ),
            ("not a JSON object", r#"["not","an","object"]"#, None),
        ];

        for (case, body, expected) in table {
            let rewritten = with_null_message(body.as_bytes(), "Message");
            match expected {
                None => assert!(
                    rewritten.is_none(),
                    "case {case:?}: expected no rewrite, got {:?}",
                    rewritten.map(String::from_utf8)
                ),
                Some(value) => {
                    let bytes =
                        rewritten.unwrap_or_else(|| panic!("case {case:?}: expected a rewrite"));
                    let document: Value = serde_json::from_slice(&bytes)
                        .unwrap_or_else(|e| panic!("case {case:?}: not JSON ({e})"));
                    assert_eq!(
                        document.get("Message"),
                        Some(&value),
                        "case {case:?}: expected the member to be {value:?}, got {document}"
                    );
                }
            }
        }
    }
}
