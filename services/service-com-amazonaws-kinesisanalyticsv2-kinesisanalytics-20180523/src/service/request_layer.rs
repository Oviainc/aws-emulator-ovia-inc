// SPDX-License-Identifier: CC0-1.0

//! This service's request-DESERIALIZATION layer: how a value whose JSON class a modelled
//! TIMESTAMP member cannot be read from is refused.
//!
//! An `awsJson1_1` timestamp that declares no `@timestampFormat` travels in the protocol's
//! default form for a body-bound timestamp — `epoch-seconds`, a JSON NUMBER — so a value of
//! any other JSON class names no instant at all. AWS refuses such a request at the
//! deserialization layer, before the operation runs: HTTP 400 with the protocol's own
//! [`SERIALIZATION_EXCEPTION`], whose message states the CLASS of value the caller sent and
//! the form it could not be read into. Nothing is read, so nothing is mutated — which
//! matters most for the operations that take a timestamp as the caller's PROOF of which
//! resource it is acting on (`DeleteApplication$CreateTimestamp`,
//! `DeleteApplicationSnapshot$SnapshotCreationTimestamp`): a request AWS never reads must
//! not delete anything.
//!
//! The framework's codec is more lenient than the protocol: it reads a QUOTED number into
//! such a member, so the value becomes a timestamp and the handler acts on it. The vendored
//! tree is read-only from a service crate, so the refusal is made here, at the one seam a
//! service has before dispatch ([`crate::service::EmulatedService::override_operation`]),
//! exactly as
//! `services/service-com-amazonaws-directoryservicedata-directoryservicedata` and
//! `services/service-com-amazonaws-xray-awsxray` do for their own request layers.
//!
//! Nothing about the model is written down here beyond the one table of modelled input
//! schemas ([`input_schema`]): WHICH members are timestamps, and under which wire name each
//! arrives, is read off those schemas through `service_common::json_wire_values`, which is
//! this tree's one naming of a JSON value's class, of the member a key names, and of the
//! sentence refusing a value that could not be read — shared with the two other services
//! that put such a layer in front of their application code.
//!
//! SCOPE: the TOP-LEVEL members of an operation's input, which is where every timestamp
//! this model binds in a request arrives (`DeleteApplication$CreateTimestamp`,
//! `DeleteApplicationSnapshot$SnapshotCreationTimestamp`). A member `Schema` in this
//! framework carries its target's KIND but not its target's schema, so a timestamp nested
//! inside a structure, a list or a map is not reachable from here and is left exactly as the
//! codec reads it — the same boundary
//! `services/service-com-amazonaws-directoryservicedata-directoryservicedata` states for its
//! own body walk. A model that nested one would want this layer to grow a walk, not a
//! special case.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::Schema;
use aws_smithy_schema::ShapeType;
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use service_common::json_wire_values::{
    json_value_class, member_by_wire_name, unreadable_message,
};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The error code a request body that cannot be deserialized is refused with.
///
/// It is the PROTOCOL's error, not one of this service's modelled shapes: the request never
/// became an input, so no operation ran to answer with its own error.
pub const SERIALIZATION_EXCEPTION: &str = "SerializationException";

/// The HTTP status that refusal carries: the caller sent something that cannot be read, so
/// it is a client error.
const SERIALIZATION_EXCEPTION_STATUS: u16 = 400;

/// The media type every response of this `awsJson1_1` service carries.
const JSON_CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// The member of the error envelope this protocol-level refusal states its reason in.
const MESSAGE_MEMBER: &str = "Message";

/// The sentence refusing `body`, when one of the values it binds to a modelled timestamp
/// member of `operation` is of a JSON class that member cannot be read from; `None` when
/// there is nothing here to refuse.
///
/// `None` deliberately covers every other outcome: a routing key naming no modelled
/// operation, an operation whose input binds no timestamp at all, a body that is not JSON,
/// and a body whose every timestamp is a number — each of which is left exactly as the
/// framework answers it today.
///
/// The body is parsed ONLY for an operation that binds a timestamp this layer governs, which
/// is read off the input schema (a `&'static Schema` baked at codegen), so a request to any
/// other operation pays a member scan and no parse.
pub fn unreadable_timestamp(operation: &str, body: &[u8]) -> Option<String> {
    let schema = input_schema(operation)?;
    if !binds_governed_timestamp(schema) {
        return None;
    }
    let Ok(Value::Object(members)) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    members
        .iter()
        // A key naming no member is ignored, exactly as the codec ignores it; which member
        // a key names is the tree's one statement of that lookup.
        .filter_map(|(key, held)| Some((member_by_wire_name(schema, key)?, held)))
        .filter(|(member, _)| governs(member))
        .find_map(|(_, held)| unreadable_value_message(held))
}

/// Whether any top-level member of `schema` is one THIS layer decides the JSON class of —
/// the question that says whether the body needs reading at all.
fn binds_governed_timestamp(schema: &Schema) -> bool {
    schema.members().iter().any(|member| governs(member))
}

/// Whether this layer decides `member`'s JSON class: a timestamp that declares no
/// `@timestampFormat`, and so is read in the protocol's default form for a body-bound
/// timestamp. A member that declares its own format is read in THAT format, so which
/// classes it admits is not this layer's rule.
fn governs(member: &Schema) -> bool {
    matches!(member.shape_type(), ShapeType::Timestamp) && member.timestamp_format().is_none()
}

/// The sentence refusing `value` as an epoch timestamp, or `None` for a value that IS one.
///
/// A number is the form the protocol carries, so it is readable. JSON `null` is absence
/// rather than a class — an absent `@required` member is the validation layer's refusal, not
/// the codec's — so it is left alone. Everything else is named by its class and refused,
/// with the target stated as the form the protocol reads a timestamp in rather than as the
/// modelled shape type.
fn unreadable_value_message(value: &Value) -> Option<String> {
    if matches!(value, Value::Number(_) | Value::Null) {
        return None;
    }
    Some(unreadable_message(json_value_class(value), EPOCH_FORM))
}

/// What a value of the wrong class could not be read into, as AWS words it for a timestamp:
/// the `epoch-seconds` form, stated as the unit the instant is counted in.
const EPOCH_FORM: &str = "milliseconds since epoch";

/// The INPUT schema of the operation an `X-Amz-Target` names: the one place this module
/// enumerates the service's surface.
///
/// Every modelled operation is listed, so an operation whose input gains a timestamp member
/// is covered without a change here, and a key naming no modelled operation answers `None`
/// and is left to the routing table's own refusal.
fn input_schema(operation: &str) -> Option<&'static Schema> {
    use crate::ops::types as t;
    Some(match operation {
        "AddApplicationCloudWatchLoggingOption" => {
            t::ADD_APPLICATION_CLOUD_WATCH_LOGGING_OPTION_INPUT_SCHEMA
        }
        "AddApplicationInput" => t::ADD_APPLICATION_INPUT_INPUT_SCHEMA,
        "AddApplicationInputProcessingConfiguration" => {
            t::ADD_APPLICATION_INPUT_PROCESSING_CONFIGURATION_INPUT_SCHEMA
        }
        "AddApplicationOutput" => t::ADD_APPLICATION_OUTPUT_INPUT_SCHEMA,
        "AddApplicationReferenceDataSource" => {
            t::ADD_APPLICATION_REFERENCE_DATA_SOURCE_INPUT_SCHEMA
        }
        "AddApplicationVpcConfiguration" => t::ADD_APPLICATION_VPC_CONFIGURATION_INPUT_SCHEMA,
        "CreateApplication" => t::CREATE_APPLICATION_INPUT_SCHEMA,
        "CreateApplicationPresignedUrl" => t::CREATE_APPLICATION_PRESIGNED_URL_INPUT_SCHEMA,
        "CreateApplicationSnapshot" => t::CREATE_APPLICATION_SNAPSHOT_INPUT_SCHEMA,
        "DeleteApplication" => t::DELETE_APPLICATION_INPUT_SCHEMA,
        "DeleteApplicationCloudWatchLoggingOption" => {
            t::DELETE_APPLICATION_CLOUD_WATCH_LOGGING_OPTION_INPUT_SCHEMA
        }
        "DeleteApplicationInputProcessingConfiguration" => {
            t::DELETE_APPLICATION_INPUT_PROCESSING_CONFIGURATION_INPUT_SCHEMA
        }
        "DeleteApplicationOutput" => t::DELETE_APPLICATION_OUTPUT_INPUT_SCHEMA,
        "DeleteApplicationReferenceDataSource" => {
            t::DELETE_APPLICATION_REFERENCE_DATA_SOURCE_INPUT_SCHEMA
        }
        "DeleteApplicationSnapshot" => t::DELETE_APPLICATION_SNAPSHOT_INPUT_SCHEMA,
        "DeleteApplicationVpcConfiguration" => {
            t::DELETE_APPLICATION_VPC_CONFIGURATION_INPUT_SCHEMA
        }
        "DescribeApplication" => t::DESCRIBE_APPLICATION_INPUT_SCHEMA,
        "DescribeApplicationOperation" => t::DESCRIBE_APPLICATION_OPERATION_INPUT_SCHEMA,
        "DescribeApplicationSnapshot" => t::DESCRIBE_APPLICATION_SNAPSHOT_INPUT_SCHEMA,
        "DescribeApplicationVersion" => t::DESCRIBE_APPLICATION_VERSION_INPUT_SCHEMA,
        "DiscoverInputSchema" => t::DISCOVER_INPUT_SCHEMA_INPUT_SCHEMA,
        "ListApplicationOperations" => t::LIST_APPLICATION_OPERATIONS_INPUT_SCHEMA,
        "ListApplicationSnapshots" => t::LIST_APPLICATION_SNAPSHOTS_INPUT_SCHEMA,
        "ListApplicationVersions" => t::LIST_APPLICATION_VERSIONS_INPUT_SCHEMA,
        "ListApplications" => t::LIST_APPLICATIONS_INPUT_SCHEMA,
        "ListTagsForResource" => t::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
        "RollbackApplication" => t::ROLLBACK_APPLICATION_INPUT_SCHEMA,
        "StartApplication" => t::START_APPLICATION_INPUT_SCHEMA,
        "StopApplication" => t::STOP_APPLICATION_INPUT_SCHEMA,
        "TagResource" => t::TAG_RESOURCE_INPUT_SCHEMA,
        "UntagResource" => t::UNTAG_RESOURCE_INPUT_SCHEMA,
        "UpdateApplication" => t::UPDATE_APPLICATION_INPUT_SCHEMA,
        "UpdateApplicationMaintenanceConfiguration" => {
            t::UPDATE_APPLICATION_MAINTENANCE_CONFIGURATION_INPUT_SCHEMA
        }
        _ => return None,
    })
}

/// The bytes of a request's body, empty for a request that carries none.
pub fn request_body(req: &RtRequest) -> &[u8] {
    req.body().bytes().unwrap_or_default()
}

/// The operation this service serves for a request whose body it cannot read into the
/// modelled input shape.
///
/// An [`ErasedOperation`] rather than a response built in
/// [`Service::override_operation`], because that seam hands the router a BORROWED operation.
/// It holds no state: the refusal is re-derived from the same request the router passes back
/// into [`ErasedOperation::run`], so one value serves every such request.
pub struct UnreadableBody;

impl ErasedOperation for UnreadableBody {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The same decision `override_operation` made, re-derived from the same request:
        // this operation is reached only when it answered `Some`, so the refusal is found
        // again. The fallback states only what IS known on that path — that the body could
        // not be read — and never names a class the request may not have carried.
        let message = service_common::rpc_target::requested_operation(
            req,
            svc.definition().identity().shape_id(),
        )
        .and_then(|operation| unreadable_timestamp(operation, request_body(req)))
        .unwrap_or_else(|| UNREADABLE_BODY.to_string());
        Box::pin(std::future::ready(serialization_exception(&message)))
    }
}

/// What this layer states when the request that reached it no longer shows WHICH value
/// could not be read: the fact the refusal rests on, with no class claimed.
const UNREADABLE_BODY: &str = "The request body can not be deserialized.";

/// The wire answer to a value this service cannot read: `400 SerializationException`, with
/// the reason under the capital-`Message` member AWS states a protocol-level refusal in, in
/// the `__type`-discriminated envelope every `awsJson1_1` error uses.
fn serialization_exception(message: &str) -> RtResponse {
    let body = serde_json::json!({
        "__type": SERIALIZATION_EXCEPTION,
        MESSAGE_MEMBER: message,
    });
    let mut response = RtResponse::new(
        StatusCode::try_from(SERIALIZATION_EXCEPTION_STATUS)
            .expect("the status is a constant of this module"),
        SdkBody::from(body.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    response
}
