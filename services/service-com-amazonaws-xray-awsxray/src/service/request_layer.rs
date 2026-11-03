// SPDX-License-Identifier: CC0-1.0

//! This service's request-DESERIALIZATION layer: how a body it cannot read into
//! the modelled input shape is refused.
//!
//! Two refusals happen before a handler runs, and they are not the same error
//! class:
//!
//! * a value the codec cannot read into the modelled type of the member it is
//!   bound to — a JSON string where the model declares an `integer`, an object
//!   where it declares a `list` — never becomes a value at all. AWS answers that
//!   at the deserialization layer with the protocol-level
//!   [`SERIALIZATION_EXCEPTION`] (HTTP 400), whose message states the two facts
//!   the caller has to reconcile: the CLASS of JSON value it sent and the
//!   modelled type that value could not become ("STRING_VALUE can not be
//!   converted to an Integer");
//! * a value that WAS read and then failed a rule the model declares (an absent
//!   `@required` member, a `@length`/`@range`/`@pattern`/enum violation) is a
//!   constraint refusal, which AWS answers with `ValidationException`.
//!
//! The framework renders BOTH as `ValidationException` (see
//! `vendor/smithy-server-runtime/src/pre_dispatch.rs`, where every
//! `ProtocolError::Serde` takes that code), so a client branching on the error
//! code to decide whether to fix its serializer or fix a value takes the wrong
//! branch. The vendored tree is read-only from a service crate, so the
//! separation is made here, at the one seam a service has before dispatch
//! ([`crate::service::EmulatedService::override_operation`]), exactly as
//! `services/service-com-amazonaws-directoryservicedata-directoryservicedata`
//! does for its own request layer.
//!
//! Nothing about the model is written down here. WHETHER a body can be read is
//! decided by running the framework's OWN deserializer over it
//! ([`refusal_for`]): the same protocol, the same input schema and the same
//! generated `DeserializableStruct` the dispatch sequence uses, so this layer
//! cannot disagree with the walk that follows it. [`Probe`] wraps that
//! deserializer only to learn WHICH read failed — the failing read names the
//! modelled target type through the `Schema` it is handed — and leaves the error
//! itself untouched.
//!
//! # The value a constraint refusal echoes
//!
//! The second class is answered here too, for one reason: an AWS constraint
//! message QUOTES the value the caller submitted (`Value '<value>' at
//! '<member path>' failed to satisfy constraint: …`), and the framework's walk
//! has no rendering of a LIST or MAP member beyond its element count, so a
//! clause about a collection reaches `Service::override_refusal` carrying
//! `<n> items` rather than the value. That seam is handed the violations ALONE,
//! never the request they came from, so the value cannot be recovered there.
//! This layer already has the built input in its hand ([`probe`]), so the echo
//! is restored here — through `service_common`'s own restoring walk — and the
//! refusal is then worded by the SAME `service_common` entry point
//! `override_refusal` delegates to, so the two cannot drift: only which value a
//! clause quotes differs, and the code, the status, the envelope, the member
//! paths and every requirement stay the framework's.

use std::cell::RefCell;
use std::marker::PhantomData;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::serde::{SerdeError, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_schema_server::constraints::{self, Violation};
use aws_smithy_schema_server::ProtocolError;
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::{BigDecimal, BigInteger, Blob, DateTime, Document};
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    ServerHttpProtocol, Service,
};

/// The error code a request body that cannot be deserialized is refused with.
///
/// It is the PROTOCOL's error, not one of this service's modelled shapes: the
/// request never became an input, so no operation ran to answer with its own
/// error. The framework already answers this code for a body it cannot process
/// at all (`pre_dispatch.rs`'s catch-all arm), which is the same refusal one
/// unreadable member earns.
pub const SERIALIZATION_EXCEPTION: &str = "SerializationException";

/// The HTTP status a protocol-level refusal of the request carries: the caller
/// sent something that cannot be read, so it is a client error.
const SERIALIZATION_EXCEPTION_STATUS: u16 = 400;

/// Why this layer refuses a request before any handler runs.
///
/// The two arms are the two error CLASSES the module header separates, and they
/// are answered by two different operations: a value the codec could not read at
/// all is the protocol's own failure ([`UnreadableBody`]), while a value that was
/// read and then broke a rule the model declares is a constraint refusal
/// ([`ConstraintRefusal`]).
#[derive(Debug)]
pub enum PreDispatchRefusal {
    /// A value whose JSON type the modelled member it is bound to cannot be read
    /// from, described by [`coercion_message`].
    Unreadable(String),
    /// Every value of the request the model's own constraints refuse, each with
    /// its clause's value echo restored to the bytes the caller submitted.
    Invalid(Vec<Violation>),
}

/// The media type every response of this restJson1 service carries.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The operation this service serves for a request whose body it cannot read
/// into the modelled input shape.
///
/// An [`ErasedOperation`] rather than a response built in
/// [`Service::override_operation`], because that seam hands the router a
/// BORROWED operation. It holds no state: the refusal is re-derived from the
/// same request the router passes back into [`ErasedOperation::run`], so one
/// value serves every such request.
///
/// `Eng` is the engine the service is generic over. It is never touched — the
/// refusal reads nothing and writes nothing — but naming it is what lets this
/// value sit on `EmulatedService<Eng>` and reach the per-operation input shapes,
/// which are generic over the same engine.
pub struct UnreadableBody<Eng> {
    engine: PhantomData<Eng>,
}

impl<Eng> UnreadableBody<Eng> {
    /// The one refusal value a service holds.
    pub const fn new() -> Self {
        Self {
            engine: PhantomData,
        }
    }
}

impl<Eng> Default for UnreadableBody<Eng> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for UnreadableBody<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn
            // ErasedServiceDefinition` is not `Sync` and this future is. The
            // protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            // The same decision `override_operation` made, re-derived from the
            // same request: this operation is reached only when it answered
            // `Some`, so the refusal is found again. The fallback keeps the
            // error CLASS right even if it somehow is not — the request reached
            // here because its body could not be read.
            let message = match protocol
                .routing_key(req)
                .ok()
                .and_then(|key| refusal_for::<Eng>(key, protocol, req))
            {
                Some(PreDispatchRefusal::Unreadable(message)) => message,
                _ => "the request body can not be deserialized".to_string(),
            };
            serialization_exception(&message)
        })
    }
}

/// The wire answer to a body this service cannot read: `400
/// SerializationException`, with the reason in the envelope the framework
/// renders every other pre-dispatch failure of this restJson1 service in — the
/// `__type` plus lowercase `message` body, and the `x-amzn-errortype` header
/// restJson1 resolves an error shape from.
fn serialization_exception(message: &str) -> RtResponse {
    let body = serde_json::json!({ "__type": SERIALIZATION_EXCEPTION, "message": message });
    let mut response = RtResponse::new(
        StatusCode::try_from(SERIALIZATION_EXCEPTION_STATUS)
            .expect("the status is a constant of this module"),
        SdkBody::from(body.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    response
        .headers_mut()
        .insert("x-amzn-errortype", SERIALIZATION_EXCEPTION);
    response
}

/// Dispatch [`probe`] on the routing key, one line per modelled operation.
///
/// The arm's key is the operation type's own name, because a REST protocol's
/// routing key IS the operation shape name: there is no second spelling of the
/// surface to drift from the first.
macro_rules! probe_by_key {
    ($key:expr, $protocol:expr, $req:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        match $key {
            $(
                stringify!($operation) => {
                    probe::<crate::ops::$module::$operation<Eng>>($protocol, $req)
                }
            )+
            _ => None,
        }
    };
}

/// Whether the body of `req` can be read into the input shape of the operation
/// `key` names, and the refusal's message when it cannot.
///
/// `key` is the routing key the protocol resolved — for a REST protocol that is
/// the operation's shape name, which is also the name of the generated operation
/// type — so the match below is the ONE place this file names the service's
/// surface, and a key naming no modelled operation is left to the router's own
/// routing failure.
pub fn refusal_for<Eng: Send + Sync + 'static>(
    key: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<PreDispatchRefusal> {
    probe_by_key!(
        key,
        protocol,
        req,
        batch_get_traces::BatchGetTraces,
        cancel_trace_retrieval::CancelTraceRetrieval,
        create_group::CreateGroup,
        create_sampling_rule::CreateSamplingRule,
        delete_group::DeleteGroup,
        delete_resource_policy::DeleteResourcePolicy,
        delete_sampling_rule::DeleteSamplingRule,
        get_encryption_config::GetEncryptionConfig,
        get_group::GetGroup,
        get_groups::GetGroups,
        get_indexing_rules::GetIndexingRules,
        get_insight::GetInsight,
        get_insight_events::GetInsightEvents,
        get_insight_impact_graph::GetInsightImpactGraph,
        get_insight_summaries::GetInsightSummaries,
        get_retrieved_traces_graph::GetRetrievedTracesGraph,
        get_sampling_rules::GetSamplingRules,
        get_sampling_statistic_summaries::GetSamplingStatisticSummaries,
        get_sampling_targets::GetSamplingTargets,
        get_service_graph::GetServiceGraph,
        get_time_series_service_statistics::GetTimeSeriesServiceStatistics,
        get_trace_graph::GetTraceGraph,
        get_trace_segment_destination::GetTraceSegmentDestination,
        get_trace_summaries::GetTraceSummaries,
        list_resource_policies::ListResourcePolicies,
        list_retrieved_traces::ListRetrievedTraces,
        list_tags_for_resource::ListTagsForResource,
        put_encryption_config::PutEncryptionConfig,
        put_resource_policy::PutResourcePolicy,
        put_telemetry_records::PutTelemetryRecords,
        put_trace_segments::PutTraceSegments,
        start_trace_retrieval::StartTraceRetrieval,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_group::UpdateGroup,
        update_indexing_rule::UpdateIndexingRule,
        update_sampling_rule::UpdateSamplingRule,
        update_trace_segment_destination::UpdateTraceSegmentDestination,
    )
}

/// Run the framework's own deserializer for `O` over the request, and report
/// what refuses it: the first value whose JSON type could not be read into the
/// modelled type of the member it is bound to, or — for a body that READ — every
/// constraint the model declares that the values broke, with each clause's value
/// echo restored to the bytes the caller submitted.
///
/// `None` is "nothing for THIS layer to refuse", which covers three different
/// outcomes deliberately: the body read cleanly and satisfies every modelled
/// constraint; the body could not be reached at all (there is no value to
/// describe, so the framework's own rendering stands); or the read failed for a
/// reason that is NOT a coercion — an absent `@required` member, a timestamp
/// whose text does not parse — which the framework answers in its own envelope,
/// unchanged by this layer.
///
/// The constraint walk is the framework's own (`constraints::validate` over the
/// built input, the same call the dispatch sequence makes), so this layer can
/// never find a different set of violations from the walk that follows it; only
/// the value each clause quotes is restored, by `service_common`'s own walk over
/// the SAME built input.
fn probe<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<PreDispatchRefusal> {
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let found: RefCell<Option<String>> = RefCell::new(None);
    let read = {
        let mut probe = Probe {
            inner: &mut *deserializer,
            found: &found,
        };
        <O::Input as DeserializableStruct>::deserialize(&mut probe)
    };
    let Ok(input) = read else {
        return found.into_inner().map(PreDispatchRefusal::Unreadable);
    };
    let violations = constraints::validate(&input, O::INPUT_SCHEMA);
    if violations.is_empty() {
        return None;
    }
    Some(PreDispatchRefusal::Invalid(
        service_common::validation::with_submitted_values(violations, &input, O::INPUT_SCHEMA),
    ))
}

/// The operation this service serves for a request whose values the model's own
/// constraints refuse.
///
/// It words that refusal through the SAME `service_common` entry point
/// [`crate::service::EmulatedService::override_refusal`] delegates to, over
/// violations whose value echo has been restored to the caller's own bytes — so
/// the only difference from the framework's own walk is which value a clause
/// quotes. Holds no state, for the same reason [`UnreadableBody`] holds none:
/// the refusal is re-derived from the request the router passes back in.
pub struct ConstraintRefusal<Eng> {
    engine: PhantomData<Eng>,
}

impl<Eng> ConstraintRefusal<Eng> {
    /// The one refusal value a service holds.
    pub const fn new() -> Self {
        Self {
            engine: PhantomData,
        }
    }
}

impl<Eng> Default for ConstraintRefusal<Eng> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ConstraintRefusal<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The same decision `override_operation` made, re-derived from the
            // same request: this operation is reached only when it answered
            // `Some` with this arm, so the violations are found again. An empty
            // set leaves the framework to render its own default, which is the
            // fail-safe direction.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            let violations = match protocol
                .routing_key(req)
                .ok()
                .and_then(|key| refusal_for::<Eng>(key, protocol, req))
            {
                Some(PreDispatchRefusal::Invalid(violations)) => violations,
                _ => Vec::new(),
            };
            // Worded by the same `service_common` entry point
            // `Service::override_refusal` delegates to, over the violations in the
            // ORDER this service states an enumerated value set in
            // (`crate::service::in_stated_value_order`) — the one owner of that
            // order, so the two seams cannot word one set two ways.
            let chosen = service_common::aws_validation_refusal(
                &crate::service::in_stated_value_order(&Refusal::Invalid(violations.clone())),
            );
            svc.definition()
                .refusal_response(ProtocolError::Invalid(violations), chosen, req)
        })
    }
}

/// The framework's deserializer, watched.
///
/// Every method forwards to the real deserializer and returns its answer
/// unchanged — this is not a codec, and it reimplements no part of one. The ONE
/// thing it adds is that a read which fails on a JSON type mismatch is recorded
/// with the modelled target type the failing read was handed, which the error
/// alone does not carry. The first such read wins: it is the one the caller has
/// to fix first, and the reads after it are reading a body the parser has
/// already left.
struct Probe<'d, 'f> {
    inner: &'d mut (dyn ShapeDeserializer + 'd),
    found: &'f RefCell<Option<String>>,
}

/// Record `error` as a coercion failure of `schema`'s modelled type, when that
/// is what it is, and return it untouched.
///
/// A free function rather than a method, so a failing read can hand it the
/// recorder while the deserializer it is reading through is still borrowed. The
/// first record wins, which is why a nested read's own failure is never
/// overwritten by the container it propagates through.
fn note_in(found: &RefCell<Option<String>>, schema: &Schema, error: SerdeError) -> SerdeError {
    let Some(class) = value_class(&error) else {
        return error;
    };
    let mut held = found.borrow_mut();
    if held.is_none() {
        *held = Some(coercion_message(class, schema.shape_type()));
    }
    error
}

/// The CLASS of JSON value a codec refusal is about, or `None` when the refusal
/// is not about a value's type.
///
/// The codec states a type mismatch as "expected <the type it wanted>, got <the
/// JSON type it found>" and a number outside the modelled integral width as
/// "number … does not fit in <width>" (`vendor/smithy-schema-server/src/json.rs`
/// — `TokenKind`'s `Display` is the vocabulary of the tail, chosen there
/// precisely so it names a JSON type a caller can act on). Those are the two
/// refusals a value's own type earns; every other codec failure — an absent
/// member, text that is not a timestamp, a truncated body — says nothing about a
/// JSON type and is left to the framework.
///
/// JSON `null` is deliberately NOT a class: a null is absence, not a type error,
/// and an absent member is the validation layer's refusal.
///
/// The wordings this reads are PINNED by [`tests::the_codec_wordings_this_layer_reads`]
/// below, so a change to them in the vendored codec fails loudly here instead of
/// silently sending every coercion failure back to `ValidationException`. TO BE
/// REPLACED when `::aws_smithy_schema::serde::SerdeError` gains a structured
/// kind for a type mismatch (today every one of them is
/// `SerdeError::custom(String)`, built in
/// `vendor/smithy-schema-server/src/json.rs`): this layer should read that kind
/// instead of the prose.
fn value_class(error: &SerdeError) -> Option<&'static str> {
    use service_common::json_wire_values as classes;
    let stated = error.to_string();
    if stated.contains("does not fit in") {
        return Some(classes::NUMBER_VALUE);
    }
    let found = stated.strip_prefix("expected ")?.split(", got ").nth(1)?;
    match found {
        "string" => Some(classes::STRING_VALUE),
        "number" => Some(classes::NUMBER_VALUE),
        "boolean" => Some(classes::BOOLEAN_VALUE),
        "array start" => Some(classes::ARRAY_VALUE),
        "object start" => Some(classes::OBJECT_VALUE),
        // "null", and the framing tokens ("object key", "array end", …), which
        // describe where the parser is rather than a value a caller sent.
        _ => None,
    }
}

/// The sentence a failed coercion states: the class of value the request sent
/// and the modelled type it could not be read as — the two facts a caller has to
/// reconcile, and the two the refusal is built from.
///
/// Both the class vocabulary above and this sentence are
/// `service_common::json_wire_values`, the tree's one statement of them, shared
/// with the request layers of
/// `services/service-com-amazonaws-directoryservicedata-directoryservicedata`
/// (which walks the body against its input schemas rather than running the
/// framework's deserializer) and of
/// `services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523`.
/// Only the sentence is shared ground: each layer detects an unreadable body its
/// own way and answers in its own envelope.
fn coercion_message(class: &str, target: ShapeType) -> String {
    service_common::json_wire_values::coercion_message(class, target)
}

/// Forward one scalar read to the real deserializer, recording a JSON type
/// mismatch against the modelled type that read was handed.
///
/// `found` is copied out of `self` BEFORE the read starts, so the recorder is
/// still reachable while the deserializer underneath is mutably borrowed.
macro_rules! watched_reads {
    ($($method:ident -> $value:ty),+ $(,)?) => {
        $(
            fn $method(&mut self, schema: &Schema) -> Result<$value, SerdeError> {
                let found = self.found;
                self.inner
                    .$method(schema)
                    .map_err(|error| note_in(found, schema, error))
            }
        )+
    };
}

impl ShapeDeserializer for Probe<'_, '_> {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let found = self.found;
        let mut watched = |member: &Schema, inner: &mut dyn ShapeDeserializer| {
            let mut probe = Probe { inner, found };
            consumer(member, &mut probe)
        };
        self.inner
            .read_struct(schema, &mut watched)
            .map_err(|error| note_in(found, schema, error))
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let found = self.found;
        let mut watched = |inner: &mut dyn ShapeDeserializer| {
            let mut probe = Probe { inner, found };
            consumer(&mut probe)
        };
        self.inner
            .read_list(schema, &mut watched)
            .map_err(|error| note_in(found, schema, error))
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let found = self.found;
        let mut watched = |key: String, inner: &mut dyn ShapeDeserializer| {
            let mut probe = Probe { inner, found };
            consumer(key, &mut probe)
        };
        self.inner
            .read_map(schema, &mut watched)
            .map_err(|error| note_in(found, schema, error))
    }

    // Every value read, forwarded and watched. The list is the trait's own
    // scalar surface; the modelled target type is the one each read is handed.
    watched_reads! {
        read_boolean -> bool,
        read_byte -> i8,
        read_short -> i16,
        read_integer -> i32,
        read_long -> i64,
        read_float -> f32,
        read_double -> f64,
        read_big_integer -> BigInteger,
        read_big_decimal -> BigDecimal,
        read_string -> String,
        read_blob -> Blob,
        read_timestamp -> DateTime,
        read_document -> Document,
    }

    fn read_null(&mut self) -> Result<(), SerdeError> {
        self.inner.read_null()
    }

    fn is_null(&self) -> bool {
        self.inner.is_null()
    }

    fn container_size(&self) -> Option<usize> {
        self.inner.container_size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::aws_smithy_schema::codec::Codec;
    use ::aws_smithy_schema_server::json::JsonCodec;

    /// Read `body` into `T` through the same watching probe the request layer
    /// drives, and report what it recorded — the JSON codec alone, so the case
    /// is about the codec's refusal and nothing else.
    fn recorded<T: DeserializableStruct>(body: &str) -> Option<String> {
        let codec = JsonCodec::rest_json();
        let mut deserializer = codec.create_deserializer(body.as_bytes());
        let found: RefCell<Option<String>> = RefCell::new(None);
        {
            let mut probe = Probe {
                inner: &mut deserializer,
                found: &found,
            };
            if T::deserialize(&mut probe).is_ok() {
                return None;
            }
        }
        found.into_inner()
    }

    /// The codec wordings [`value_class`] reads, pinned.
    ///
    /// This layer learns the CLASS of value a caller sent from the text of the
    /// vendored codec's own refusal ("expected <type>, got <json type>", "number
    /// … does not fit in <width>"), which is the only place it is stated. The
    /// tree it comes from is read-only from here, so this case is what makes a
    /// change to those wordings fail loudly instead of quietly sending every
    /// coercion failure back to the framework's `ValidationException`.
    #[test]
    fn the_codec_wordings_this_layer_reads() {
        use crate::ops::types::{BatchGetTracesInput, PutTelemetryRecordsInput};

        // A string, a boolean and an object where the model declares an
        // `integer`; a number past that integer's width; a boolean where it
        // declares a `timestamp`; a string and an object where it declares a
        // list.
        let integer_rows = [
            (r#""17""#, "STRING_VALUE can not be converted to an Integer"),
            (r#"true"#, "BOOLEAN_VALUE can not be converted to an Integer"),
            (
                r#"{"Count":3}"#,
                "OBJECT_VALUE can not be converted to an Integer",
            ),
            (
                r#"2147483648"#,
                "NUMBER_VALUE can not be converted to an Integer",
            ),
        ];
        for (sent, expected) in integer_rows {
            let body = format!(
                r#"{{"TelemetryRecords":[{{"Timestamp":1.0,"SegmentsReceivedCount":{sent}}}]}}"#
            );
            let recorded = recorded::<PutTelemetryRecordsInput>(&body);
            assert_eq!(
                recorded.as_deref(),
                Some(expected),
                "case {sent:?}: expected the refusal {expected:?}, got {recorded:?}"
            );
        }

        let timestamp = recorded::<PutTelemetryRecordsInput>(r#"{"TelemetryRecords":[{"Timestamp":true}]}"#);
        assert_eq!(
            timestamp.as_deref(),
            Some("BOOLEAN_VALUE can not be converted to a Timestamp"),
            "a boolean where a timestamp is modelled: got {timestamp:?}"
        );

        let list_rows = [
            (r#""one-id""#, "STRING_VALUE can not be converted to a List"),
            (
                r#"{"0":"one-id"}"#,
                "OBJECT_VALUE can not be converted to a List",
            ),
        ];
        for (sent, expected) in list_rows {
            let body = format!(r#"{{"TraceIds":{sent}}}"#);
            let recorded = recorded::<BatchGetTracesInput>(&body);
            assert_eq!(
                recorded.as_deref(),
                Some(expected),
                "case {sent:?}: expected the refusal {expected:?}, got {recorded:?}"
            );
        }

        // And the refusals that are NOT about a value's JSON type: an absent
        // `@required` member, an explicit null, and text that is the modelled
        // type but not a timestamp. None is this layer's to answer.
        let not_ours = [
            r#"{"TelemetryRecords":[{}]}"#,
            r#"{"TelemetryRecords":[{"Timestamp":null}]}"#,
            r#"{"TelemetryRecords":[{"Timestamp":"half past four"}]}"#,
        ];
        for body in not_ours {
            let recorded = recorded::<PutTelemetryRecordsInput>(body);
            assert_eq!(
                recorded, None,
                "case {body:?}: a refusal that is not about a value's JSON type must record \
                 nothing here, got {recorded:?}"
            );
        }
    }
}
