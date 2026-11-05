// SPDX-License-Identifier: CC0-1.0

//! This service's request-DESERIALIZATION layer: how a body it cannot read into the
//! modelled input shape is refused.
//!
//! Two refusals happen before a handler runs, and they are not the same error class:
//!
//! * a value whose JSON type the modelled type of the member it is bound to cannot be
//!   read from — a JSON number where the model declares a string, a string where it
//!   declares an integer, an object where it declares a list — never becomes a value at
//!   all. AWS answers that at the deserialization layer with the protocol-level
//!   [`SERIALIZATION_EXCEPTION`] (HTTP 400), whose message states the two facts the
//!   caller has to reconcile: the CLASS of JSON value it sent and the modelled type that
//!   value could not become ("NUMBER_VALUE can not be converted to a String"). The
//!   refusal names the class, never the value, so a long value cannot be amplified into a
//!   long response — and the boolean literals are two CLASSES rather than one
//!   ([`submitted_class`]);
//! * a value that WAS read and then failed a rule the model declares (an absent
//!   `@required` member, a `@length`/`@range`/`@pattern`/enum violation) is a constraint
//!   refusal, which this service answers through
//!   [`crate::service::EmulatedService::override_refusal`].
//!
//! The framework renders BOTH as `ValidationException` (see
//! `vendor/smithy-server-runtime/src/pre_dispatch.rs`, where every `ProtocolError::Serde`
//! takes that code), so a client branching on the error code to decide whether to fix its
//! serializer or fix a value takes the wrong branch — and the codec's own prose, which
//! that path renders, is not the sentence AWS states. The vendored tree is read-only from
//! a service crate, so the separation is made here, inside the one scope this service
//! already serves every request through
//! (`crate::service::OperationScopedRefusal::run`), exactly as
//! `services/service-com-amazonaws-xray-awsxray` does for its own request layer.
//!
//! Nothing about the model is written down here. WHETHER a body can be read is decided by
//! running the framework's OWN deserializer over it ([`coercion_refusal`]): the same
//! protocol, the same input schema and the same generated `DeserializableStruct` the
//! dispatch sequence uses, so this layer cannot disagree with the walk that follows it.
//! [`Probe`] wraps that deserializer only to learn WHICH read failed — the failing read
//! names the modelled target type through the `Schema` it is handed — and leaves the error
//! itself untouched.

use std::cell::RefCell;

use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_schema::serde::{SerdeError, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_types::{BigDecimal, BigInteger, Blob, DateTime, Document};
use smithy_server_runtime::{DeserializableStruct, Operation, ServerHttpProtocol};

/// The error code a request body that cannot be deserialized is refused with.
///
/// It is the PROTOCOL's error, not one of this service's modelled shapes: the request
/// never became an input, so no operation ran to answer with its own error. The framework
/// already answers this code for a body it cannot process at all (`pre_dispatch.rs`'s
/// catch-all arm), which is the same refusal one unreadable member earns.
pub const SERIALIZATION_EXCEPTION: &str = "SerializationException";

/// Dispatch [`probe`] on the routing key, one line per modelled operation.
///
/// The arm's key is the operation type's own name, because this service's one protocol
/// (`smithy_query(query="service_info")`: awsJson1_1) resolves its routing key to the
/// operation shape name, which is also the name of the generated operation type: there is
/// no second spelling of the surface to drift from the first.
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

/// The sentence refusing `req`'s body, when a value it carries cannot be read into the
/// modelled type of the member it is bound to, for the operation `key` names.
///
/// `None` is "nothing for THIS layer to refuse", which covers three different outcomes
/// deliberately: the body read cleanly; the body could not be reached at all (there is no
/// value to describe, so the framework's own rendering stands); or the read failed for a
/// reason that is NOT a coercion — an absent `@required` member, text that is not a
/// timestamp — which the framework and this service's own refusal wording answer,
/// unchanged by this layer. A key naming no modelled operation is likewise left to the
/// router's own routing failure.
pub fn coercion_refusal<Eng: Send + Sync + 'static>(
    key: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<String> {
    probe_by_key!(
        key,
        protocol,
        req,
        add_instance_fleet::AddInstanceFleet,
        add_instance_groups::AddInstanceGroups,
        add_job_flow_steps::AddJobFlowSteps,
        add_tags::AddTags,
        cancel_steps::CancelSteps,
        create_persistent_app_ui::CreatePersistentAppUI,
        create_security_configuration::CreateSecurityConfiguration,
        create_studio::CreateStudio,
        create_studio_session_mapping::CreateStudioSessionMapping,
        delete_security_configuration::DeleteSecurityConfiguration,
        delete_studio::DeleteStudio,
        delete_studio_session_mapping::DeleteStudioSessionMapping,
        describe_cluster::DescribeCluster,
        describe_job_flows::DescribeJobFlows,
        describe_notebook_execution::DescribeNotebookExecution,
        describe_persistent_app_ui::DescribePersistentAppUI,
        describe_release_label::DescribeReleaseLabel,
        describe_security_configuration::DescribeSecurityConfiguration,
        describe_step::DescribeStep,
        describe_studio::DescribeStudio,
        get_auto_termination_policy::GetAutoTerminationPolicy,
        get_block_public_access_configuration::GetBlockPublicAccessConfiguration,
        get_cluster_session_credentials::GetClusterSessionCredentials,
        get_managed_scaling_policy::GetManagedScalingPolicy,
        get_on_cluster_app_ui_presigned_url::GetOnClusterAppUIPresignedURL,
        get_persistent_app_ui_presigned_url::GetPersistentAppUIPresignedURL,
        get_session::GetSession,
        get_session_endpoint::GetSessionEndpoint,
        get_studio_session_mapping::GetStudioSessionMapping,
        list_bootstrap_actions::ListBootstrapActions,
        list_clusters::ListClusters,
        list_instance_fleets::ListInstanceFleets,
        list_instance_groups::ListInstanceGroups,
        list_instances::ListInstances,
        list_notebook_executions::ListNotebookExecutions,
        list_release_labels::ListReleaseLabels,
        list_security_configurations::ListSecurityConfigurations,
        list_sessions::ListSessions,
        list_steps::ListSteps,
        list_studio_session_mappings::ListStudioSessionMappings,
        list_studios::ListStudios,
        list_supported_instance_types::ListSupportedInstanceTypes,
        modify_cluster::ModifyCluster,
        modify_instance_fleet::ModifyInstanceFleet,
        modify_instance_groups::ModifyInstanceGroups,
        put_auto_scaling_policy::PutAutoScalingPolicy,
        put_auto_termination_policy::PutAutoTerminationPolicy,
        put_block_public_access_configuration::PutBlockPublicAccessConfiguration,
        put_managed_scaling_policy::PutManagedScalingPolicy,
        remove_auto_scaling_policy::RemoveAutoScalingPolicy,
        remove_auto_termination_policy::RemoveAutoTerminationPolicy,
        remove_managed_scaling_policy::RemoveManagedScalingPolicy,
        remove_tags::RemoveTags,
        run_job_flow::RunJobFlow,
        set_keep_job_flow_alive_when_no_steps::SetKeepJobFlowAliveWhenNoSteps,
        set_termination_protection::SetTerminationProtection,
        set_unhealthy_node_replacement::SetUnhealthyNodeReplacement,
        set_visible_to_all_users::SetVisibleToAllUsers,
        start_notebook_execution::StartNotebookExecution,
        start_session::StartSession,
        stop_notebook_execution::StopNotebookExecution,
        terminate_job_flows::TerminateJobFlows,
        terminate_session::TerminateSession,
        update_studio::UpdateStudio,
        update_studio_session_mapping::UpdateStudioSessionMapping,
    )
}

/// Run the framework's own deserializer for `O` over the request, and report the first
/// value whose JSON type could not be read into the modelled type of the member it is
/// bound to.
///
/// A body that READS is this layer's business no further: whatever the model's own
/// constraints then say about its values is a constraint refusal, which the walk that
/// follows answers.
///
/// The CLASS the sentence names is read off the submitted VALUE, which the probe reaches
/// through the path it recorded, because the codec's own complaint names a JSON TOKEN KIND
/// and two classes share one kind. That kind is kept as the fallback for a path the body
/// cannot be walked by, so the sentence is never worse than the codec's own.
fn probe<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Option<String> {
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let found: RefCell<Option<Unreadable>> = RefCell::new(None);
    let path: RefCell<Vec<Step>> = RefCell::new(Vec::new());
    let read = {
        let mut probe = Probe {
            inner: &mut *deserializer,
            found: &found,
            path: &path,
        };
        <O::Input as DeserializableStruct>::deserialize(&mut probe)
    };
    if read.is_ok() {
        return None;
    }
    sentence(found.into_inner(), req.body().bytes())
}

/// The sentence one unreadable value is refused with: its CLASS, read off the submitted
/// body at the path the probe recorded, and the type the failing read was for.
///
/// The codec's own token kind stands in for the class when the body cannot be walked by
/// that path, so the sentence is never worse than the codec's own.
fn sentence(unreadable: Option<Unreadable>, body: Option<&[u8]>) -> Option<String> {
    let unreadable = unreadable?;
    let submitted = body
        .and_then(|body| ::serde_json::from_slice::<::serde_json::Value>(body).ok())
        .and_then(|value| value_at(&value, &unreadable.path).map(submitted_class));
    Some(coercion_message(
        submitted.unwrap_or(unreadable.class),
        unreadable.target,
    ))
}

/// One step of the path from the request body to a value inside it.
#[derive(Debug, Clone)]
enum Step {
    /// A member of a structure, by the name the wire spells — which on this service's one
    /// protocol (awsJson1_1) is the Smithy member name.
    Member(String),
    /// An element of a list, by position.
    Index(usize),
    /// An entry of a map, by key.
    Key(String),
}

/// The value the submitted `body` carries at `path`, or `None` when the body carries
/// nothing there.
fn value_at<'b>(body: &'b ::serde_json::Value, path: &[Step]) -> Option<&'b ::serde_json::Value> {
    let mut cursor = body;
    for step in path {
        cursor = match step {
            Step::Member(name) | Step::Key(name) => cursor.get(name)?,
            Step::Index(at) => cursor.get(*at)?,
        };
    }
    Some(cursor)
}

/// What one unreadable value is refused with: the type the failing read was FOR, the path
/// the submitted value stands at, and the class the codec's own complaint named it under.
#[derive(Debug)]
struct Unreadable {
    target: ShapeType,
    path: Vec<Step>,
    class: &'static str,
}

/// The CLASS a refusal names a submitted value under.
///
/// The vocabulary is the tree's shared one
/// ([`service_common::json_wire_values::json_value_class`]) with ONE difference, which is
/// recorded behavior for this service: a boolean is named by its LITERAL, because its two
/// literals are two classes rather than one. Every other JSON type is one class, so the
/// shared naming stands for it.
fn submitted_class(value: &::serde_json::Value) -> &'static str {
    match value {
        ::serde_json::Value::Bool(true) => TRUE_VALUE,
        ::serde_json::Value::Bool(false) => FALSE_VALUE,
        other => service_common::json_wire_values::json_value_class(other),
    }
}

/// See [`submitted_class`].
const TRUE_VALUE: &str = "TRUE_VALUE";

/// See [`submitted_class`].
const FALSE_VALUE: &str = "FALSE_VALUE";

/// The framework's deserializer, watched.
///
/// Every method forwards to the real deserializer and returns its answer unchanged — this
/// is not a codec, and it reimplements no part of one. The ONE thing it adds is that a
/// read which fails on a JSON type mismatch is recorded with the modelled type that read
/// was for and the path it stood at, neither of which the error carries. The first such
/// read wins: it is the one the caller has to fix first, and the reads after it are
/// reading a body the parser has already left.
struct Probe<'d, 'f> {
    inner: &'d mut (dyn ShapeDeserializer + 'd),
    found: &'f RefCell<Option<Unreadable>>,
    /// Where the read currently is in the request body, so the submitted value can be
    /// reached when a read of it fails.
    path: &'f RefCell<Vec<Step>>,
}

/// Record `error` as a coercion failure into `target` at the current path, when that is
/// what it is, and return it untouched.
///
/// A free function rather than a method, so a failing read can hand it the recorder while
/// the deserializer it is reading through is still borrowed. The first record wins, which
/// is why a nested read's own failure is never overwritten by the container it propagates
/// through.
///
/// `target` is the type the FAILING READ was for, not the shape type of the `Schema` that
/// read was handed: the two agree for a member of a structure, and only the read knows the
/// element type of a collection, because the JSON codec hands a collection's elements the
/// collection member's own schema.
fn note_in(
    found: &RefCell<Option<Unreadable>>,
    path: &RefCell<Vec<Step>>,
    target: ShapeType,
    error: SerdeError,
) -> SerdeError {
    let Some(class) = value_class(&error) else {
        return error;
    };
    let mut held = found.borrow_mut();
    if held.is_none() {
        *held = Some(Unreadable {
            target,
            path: path.borrow().clone(),
            class,
        });
    }
    error
}

/// The CLASS of JSON value a codec refusal is about, or `None` when the refusal is not
/// about a value's type.
///
/// The codec states a type mismatch as "expected <the type it wanted>, got <the JSON type
/// it found>" and a number outside the modelled integral width as "number … does not fit
/// in <width>" (`vendor/smithy-schema-server/src/json.rs` — `TokenKind`'s `Display` is the
/// vocabulary of the tail, chosen there precisely so it names a JSON type a caller can act
/// on). Those are the two refusals a value's own type earns; every other codec failure —
/// an absent member, text that is not a timestamp, a truncated body — says nothing about a
/// JSON type and is left to the framework.
///
/// JSON `null` is deliberately NOT a class: a null is absence, not a type error, and an
/// absent member is the validation layer's refusal.
///
/// The wordings this reads are PINNED by [`tests::the_codec_wordings_this_layer_reads`]
/// below, so a change to them in the vendored codec fails loudly here instead of silently
/// sending every coercion failure back to `ValidationException`.
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
        // "null", and the framing tokens ("object key", "array end", …), which describe
        // where the parser is rather than a value a caller sent.
        _ => None,
    }
}

/// The sentence a failed coercion states: the class of value the request sent and the
/// modelled type it could not be read as — the two facts a caller has to reconcile.
///
/// Both the class vocabulary above and this sentence are
/// `service_common::json_wire_values`, the tree's one statement of them, shared with the
/// request layers of the other services that separate these two error classes. Only the
/// sentence is shared ground: each layer detects an unreadable body its own way and
/// answers in its own envelope.
fn coercion_message(class: &str, target: ShapeType) -> String {
    service_common::json_wire_values::coercion_message(class, target)
}

/// Forward one scalar read to the real deserializer, recording a JSON type mismatch
/// against the type that read is FOR — which is the read's own target type, named beside
/// it here, rather than the shape type of the schema it is handed.
///
/// `found` is copied out of `self` BEFORE the read starts, so the recorder is still
/// reachable while the deserializer underneath is mutably borrowed.
macro_rules! watched_reads {
    ($($method:ident -> $value:ty as $target:ident),+ $(,)?) => {
        $(
            fn $method(&mut self, schema: &Schema) -> Result<$value, SerdeError> {
                let (found, path) = (self.found, self.path);
                self.inner
                    .$method(schema)
                    .map_err(|error| note_in(found, path, ShapeType::$target, error))
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
        let (found, path) = (self.found, self.path);
        let mut watched = |member: &Schema, inner: &mut dyn ShapeDeserializer| {
            let mut probe = Probe { inner, found, path };
            // The step is on the path for the WHOLE read of this member, including every
            // read nested inside it, and comes off again however that read ended.
            let descended = member.member_name().is_some_and(|name| {
                path.borrow_mut().push(Step::Member(name.to_string()));
                true
            });
            let read = consumer(member, &mut probe);
            if descended {
                path.borrow_mut().pop();
            }
            read
        };
        self.inner
            .read_struct(schema, &mut watched)
            .map_err(|error| note_in(found, path, schema.shape_type(), error))
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let (found, path) = (self.found, self.path);
        // The consumer is called once per element, in the body's own order, which is what
        // names each element's position.
        let mut at = 0usize;
        let mut watched = |inner: &mut dyn ShapeDeserializer| {
            let mut probe = Probe { inner, found, path };
            path.borrow_mut().push(Step::Index(at));
            at = at.saturating_add(1);
            let read = consumer(&mut probe);
            path.borrow_mut().pop();
            read
        };
        self.inner
            .read_list(schema, &mut watched)
            .map_err(|error| note_in(found, path, schema.shape_type(), error))
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let (found, path) = (self.found, self.path);
        let mut watched = |key: String, inner: &mut dyn ShapeDeserializer| {
            let mut probe = Probe { inner, found, path };
            path.borrow_mut().push(Step::Key(key.clone()));
            let read = consumer(key, &mut probe);
            path.borrow_mut().pop();
            read
        };
        self.inner
            .read_map(schema, &mut watched)
            .map_err(|error| note_in(found, path, schema.shape_type(), error))
    }

    // Every value read, forwarded and watched. The list is the trait's own scalar
    // surface, each read beside the modelled type it is for.
    watched_reads! {
        read_boolean -> bool as Boolean,
        read_byte -> i8 as Byte,
        read_short -> i16 as Short,
        read_integer -> i32 as Integer,
        read_long -> i64 as Long,
        read_float -> f32 as Float,
        read_double -> f64 as Double,
        read_big_integer -> BigInteger as BigInteger,
        read_big_decimal -> BigDecimal as BigDecimal,
        read_string -> String as String,
        read_blob -> Blob as Blob,
        read_document -> Document as Document,
        read_timestamp -> DateTime as Timestamp,
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

    /// Read `body` into `T` through the same watching probe the request layer drives, and
    /// report the sentence it would be refused with — the JSON codec alone, so the case is
    /// about the codec's refusal and the class of the submitted value and nothing else.
    fn recorded<T: DeserializableStruct>(body: &str) -> Option<String> {
        let codec = JsonCodec::new();
        let mut deserializer = codec.create_deserializer(body.as_bytes());
        let found: RefCell<Option<Unreadable>> = RefCell::new(None);
        let path: RefCell<Vec<Step>> = RefCell::new(Vec::new());
        {
            let mut probe = Probe {
                inner: &mut deserializer,
                found: &found,
                path: &path,
            };
            if T::deserialize(&mut probe).is_ok() {
                return None;
            }
        }
        sentence(found.into_inner(), Some(body.as_bytes()))
    }

    /// The codec wordings [`value_class`] reads, and the path [`Probe`] records, pinned.
    ///
    /// This layer learns WHETHER a read failed on a value's JSON type from the text of the
    /// vendored codec's own refusal ("expected <type>, got <json type>", "number … does
    /// not fit in <width>"), which is the only place it is stated. The tree it comes from
    /// is read-only from here, so this case is what makes a change to those wordings fail
    /// loudly instead of quietly sending every coercion failure back to the framework's
    /// `ValidationException`. It pins the two halves of the sentence with it: the CLASS,
    /// read off the submitted value at the recorded path, and the modelled TYPE the
    /// failing read was for.
    #[test]
    fn the_codec_wordings_this_layer_reads() {
        use crate::ops::types::{ListClustersInput, RunJobFlowInput, TerminateJobFlowsInput};

        // A number, each boolean literal, an array and an object where the model declares
        // a string (`RunJobFlow$Name` targets `XmlStringMaxLen256`).
        let string_rows = [
            ("17", "NUMBER_VALUE can not be converted to a String"),
            ("17.5", "NUMBER_VALUE can not be converted to a String"),
            ("true", "TRUE_VALUE can not be converted to a String"),
            ("false", "FALSE_VALUE can not be converted to a String"),
            (r#"["a"]"#, "ARRAY_VALUE can not be converted to a String"),
            (
                r#"{"Name":"a"}"#,
                "OBJECT_VALUE can not be converted to a String",
            ),
        ];
        for (sent, expected) in string_rows {
            let body = format!(r#"{{"Name":{sent},"Instances":{{}}}}"#);
            let recorded = recorded::<RunJobFlowInput>(&body);
            assert_eq!(
                recorded.as_deref(),
                Some(expected),
                "case {sent:?}: expected the refusal {expected:?}, got {recorded:?}"
            );
        }

        // A string and an object where the model declares a list
        // (`ListClusters$ClusterStates`), and each boolean where it declares a timestamp.
        let other_rows = [
            (
                r#"{"ClusterStates":"RUNNING"}"#,
                "STRING_VALUE can not be converted to a List",
            ),
            (
                r#"{"ClusterStates":{"0":"RUNNING"}}"#,
                "OBJECT_VALUE can not be converted to a List",
            ),
            (
                r#"{"CreatedAfter":true}"#,
                "TRUE_VALUE can not be converted to a Timestamp",
            ),
            (
                r#"{"CreatedBefore":false}"#,
                "FALSE_VALUE can not be converted to a Timestamp",
            ),
        ];
        for (body, expected) in other_rows {
            let recorded = recorded::<ListClustersInput>(body);
            assert_eq!(
                recorded.as_deref(),
                Some(expected),
                "case {body:?}: expected the refusal {expected:?}, got {recorded:?}"
            );
        }

        // A number where the model declares an integer, past that integer's width:
        // `Instances.InstanceCount` targets `Integer`.
        let wide =
            recorded::<RunJobFlowInput>(r#"{"Name":"a","Instances":{"InstanceCount":2147483648}}"#);
        assert_eq!(
            wide.as_deref(),
            Some("NUMBER_VALUE can not be converted to an Integer"),
            "a number past the modelled integer width: got {wide:?}"
        );

        // Values inside a collection: the path reaches them, so the class is the
        // ELEMENT's own and the type is the one the failing read was for — an element of
        // a list of strings, and a value of a map of string to string.
        let element = recorded::<TerminateJobFlowsInput>(r#"{"JobFlowIds":["j-A",false]}"#);
        assert_eq!(
            element.as_deref(),
            Some("FALSE_VALUE can not be converted to a String"),
            "a boolean element of a list of strings: got {element:?}"
        );
        let entry = recorded::<RunJobFlowInput>(
            r#"{"Name":"a","Instances":{},"Configurations":[{"Properties":{"k":7}}]}"#,
        );
        assert_eq!(
            entry.as_deref(),
            Some("NUMBER_VALUE can not be converted to a String"),
            "a number value of a map of strings: got {entry:?}"
        );

        // And the refusals that are NOT about a value's JSON type: an absent `@required`
        // member and an explicit null. Neither is this layer's to answer.
        let not_ours = [r#"{"Instances":{}}"#, r#"{"Name":null,"Instances":{}}"#];
        for body in not_ours {
            let recorded = recorded::<RunJobFlowInput>(body);
            assert_eq!(
                recorded, None,
                "case {body:?}: a refusal that is not about a value's JSON type must \
                 record nothing here, got {recorded:?}"
            );
        }
    }
}
