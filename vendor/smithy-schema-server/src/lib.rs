// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Server-side protocol traits for `aws-smithy-schema`.
//!
//! `aws-smithy-schema` ships [`ClientProtocolInner`][cpi], which translates an
//! operation _input_ into a transport request and a transport response into an
//! operation _output_. A server needs the inverse mapping: take an inbound
//! request, route it to an operation, deserialize the request body into an
//! operation input, and serialize the corresponding output (or error) back to
//! a response.
//!
//! [cpi]: https://docs.rs/aws-smithy-schema/0.1.0/aws_smithy_schema/protocol/trait.ClientProtocolInner.html
//!
//! This crate provides:
//!
//! - [`ServerProtocol`]: a server-side protocol abstraction. Implementors
//!   write this trait once per wire protocol (awsJson1_0, awsJson1_1, RPCv2
//!   CBOR, REST-JSON, …).
//! - [`HttpRpcServerProtocol`]: a generic body-only RPC server protocol
//!   parameterised over a [`Codec`]. This handles awsJson1_0 / awsJson1_1 /
//!   rpcv2Cbor uniformly — pick a codec, pick a content-type, pick a routing
//!   key extractor, and you have a working server protocol.
//! - [`json::JsonCodec`]: an [`aws_smithy_schema::codec::Codec`]
//!   implementation built on `aws-smithy-json`. This is the codec the awsJson
//!   family of protocols use.
//! - [`AwsJson10`] and [`AwsJson11`]: convenience constructors that pick
//!   the right codec, content-type, and `x-amz-target` routing strategy for
//!   the awsJson1_0 / awsJson1_1 server protocols.

pub mod aws_chunked;
pub mod cbor;
pub mod constraints;
pub mod http_binding;
pub mod json;
pub mod protocol;
pub mod query;
pub mod rest_json;
pub mod rest_xml;
pub mod rpc;
pub mod xml;

pub use cbor::CborCodec;
pub use constraints::{
    envelope, missing_member_message, validate, validate_to_message, Constrained, LengthTrait,
    ValidatingSerializer, Violation, ViolationKind,
};
pub use http_binding::{HttpBindingDeserializer, HttpBindingOutput, HttpBindingSerializer};
pub use protocol::{
    insert_error_response_headers, insert_response_headers, ProtocolError, ServerProtocol,
};
pub use query::{
    extract_action, parse_form_urlencoded, query_error_body, QueryServerProtocol,
    QueryShapeDeserializer,
};
pub use rest_json::RestJsonServerProtocol;
pub use rest_xml::{RequiredBinding, RestXmlServerProtocol, Route};
pub use rpc::{
    routing_key_from_path, routing_key_from_x_amz_target, AwsJson10, AwsJson11,
    HttpRpcServerProtocol, Rpcv2Cbor,
};
pub use xml::xml_escape;

use aws_smithy_schema::serde::{SerdeError, ShapeDeserializer};

/// Trait for shapes that can be constructed from a [`ShapeDeserializer`].
///
/// This is the inverse of
/// [`aws_smithy_schema::serde::SerializableStruct`]: it lets a server
/// reconstruct a structure value from an arbitrary serial format. A
/// `DeserializableStruct` impl knows how to walk over the members produced
/// by [`ShapeDeserializer::read_struct`] and assemble its own state.
///
/// Future versions of `aws-smithy-schema` may add this trait directly; in
/// the meantime it lives here so server code has a single name to refer to.
pub trait DeserializableStruct: Sized {
    /// Reads a value from the deserializer.
    fn deserialize(d: &mut dyn ShapeDeserializer) -> Result<Self, SerdeError>;
}

/// The error for a required member absent from the request, naming it by its
/// Smithy member name.
///
/// Deliberately carries the name rather than a rendered sentence: a
/// deserializer knows WHICH member is missing but not how the service it
/// belongs to words that refusal, so the wording is chosen at the protocol
/// boundary instead. See [`constraints::missing_member_message`].
pub fn missing_member(member_name: &str) -> SerdeError {
    SerdeError::MissingMember {
        member_name: constraints::path::member(member_name),
    }
}

/// Re-root a [`missing_member`] error one level out, so a member absent from a
/// nested shape reports its trail from the operation input rather than just its
/// own name. Any other error passes through untouched.
///
/// The error travels outwards as the deserializer unwinds, and each enclosing
/// member prepends itself — which is the only point at which the enclosing name
/// is known.
pub fn within_member(error: SerdeError, enclosing_member: &str) -> SerdeError {
    within_segment(error, &constraints::path::member(enclosing_member))
}

/// Re-root a [`missing_member`] error inside the list element being read, which
/// a service names by its 1-based position followed by `member`.
pub fn within_list_element(error: SerdeError, position: usize) -> SerdeError {
    within_segment(error, &constraints::path::element(position))
}

/// Re-root a [`missing_member`] error inside the map entry being read, which a
/// service names by the entry's key followed by `member`.
///
/// The key is a value the CLIENT chose, so it is echoed exactly as sent — never
/// re-cased the way a model name is.
pub fn within_map_entry(error: SerdeError, key: &str) -> SerdeError {
    within_segment(error, &constraints::path::entry(key))
}

/// Prepend an already-final path segment. Any error other than a missing member
/// has no path to extend and passes through untouched.
fn within_segment(error: SerdeError, segment: &str) -> SerdeError {
    match error {
        SerdeError::MissingMember { member_name } => SerdeError::MissingMember {
            member_name: format!("{segment}.{member_name}"),
        },
        other => other,
    }
}
