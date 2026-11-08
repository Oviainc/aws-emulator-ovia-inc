// SPDX-License-Identifier: CC0-1.0

//! This service's request-DESERIALIZATION layer: how a body it cannot read into
//! the operation's modelled input shape is refused.
//!
//! Two refusals happen before a handler runs, and they are not the same class:
//!
//! * a value that WAS read and then broke a rule the model declares — an absent
//!   `@required` member, a value outside a declared `@length`, `@pattern`,
//!   `@range` or enumerated set — is a constraint refusal, which the service
//!   words through [`crate::service::EmulatedService::override_refusal`];
//! * a value whose JSON type the member it is bound to cannot be read from at
//!   all — a string where the model declares a list, an object where it declares
//!   a string — never becomes a value, so no constraint was ever checked and no
//!   handler ran. This service answers that class with its own invalid-input
//!   sentence ([`crate::state::INVALID_INPUT_MESSAGE`]) under
//!   `BadRequestException`, the ONE client-input error its model declares.
//!
//! The framework renders the second class from the codec's own words under an
//! error code this model does not carry (`vendor/smithy-server-runtime/src/pre_dispatch.rs`
//! names it for every `ProtocolError::Serde` it cannot describe structurally),
//! so a generated client finds an unmodelled error type where its
//! `BadRequestException` branch belongs, and the message describes this
//! emulator's deserializer instead of the request. The vendored tree is
//! read-only from a service crate, so the separation is made here, at the one
//! seam a service has in front of dispatch
//! ([`smithy_server_runtime::Service::override_operation`]) — the same place
//! `services/service-com-amazonaws-xray-awsxray` makes it for its own request
//! layer.
//!
//! Nothing about the model is written down here. WHETHER a body can be read is
//! decided by running the framework's OWN deserializer over it
//! ([`body_is_unreadable`]): the same protocol, the same input schema and the
//! same generated input type the dispatch sequence uses, so this layer cannot
//! disagree with the walk that follows it. The cost is that a body is read
//! twice — once here and once by that walk — which an emulator pays to answer
//! the right error class.
//!
//! Only the operations whose model binds members to the BODY are probed: a
//! request addressed entirely by path labels and query members carries no body
//! for this layer to fault, and a request that carries no body at all is left to
//! the framework, which is what states an absent `@required` member.

use std::marker::PhantomData;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext,
    ServerHttpProtocol, Service,
};

use crate::ops::types::{BadRequestException, SCHEMA_BAD_REQUEST_EXCEPTION};

/// The HTTP status the model gives `BadRequestException` (`@httpError(400)`) —
/// the status the framework's own refusal renderer answers this class with too,
/// so restating the refusal changes the error code and the sentence and nothing
/// else.
const BAD_REQUEST_STATUS: u16 = 400;

/// Whether the body of `req` is one this service cannot read into the input of
/// the operation `O`.
///
/// `false` covers three outcomes deliberately: a body that reads cleanly
/// (whatever its values then fail), a request that carries no body at all, and a
/// read that failed on an absent `@required` member — which the framework
/// describes per member and this service already words through
/// `override_refusal`, so it is not this layer's refusal to make.
fn body_is_unreadable_as<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> bool {
    // An absent or empty body is not an unreadable one: there is nothing the
    // caller sent for this layer to fault.
    if req.body().bytes().is_none_or(<[u8]>::is_empty) {
        return false;
    }
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        // The body could not be reached at all, which is a transport-level
        // failure the framework owns rather than a value this service refuses.
        return false;
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(_) => false,
        Err(SerdeError::MissingMember { .. }) => false,
        Err(_) => true,
    }
}

/// Dispatch [`body_is_unreadable_as`] on the routing key, one line per modelled
/// operation that takes a request body.
///
/// The arm's key is the operation type's own name, because a REST protocol's
/// routing key IS the operation shape name: there is no second spelling of the
/// surface to drift from the first.
macro_rules! unreadable_by_key {
    ($key:expr, $protocol:expr, $req:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        match $key {
            $(
                stringify!($operation) => {
                    body_is_unreadable_as::<crate::ops::$module::$operation<Eng>>($protocol, $req)
                }
            )+
            _ => false,
        }
    };
}

/// Whether `req` carries a body this service cannot read into the input of the
/// operation `key` names.
///
/// `key` is the routing key the protocol resolved — for this restJson1 service
/// that is the operation's shape name, which is also the name of the generated
/// operation type — so the match below is the ONE place this file names the
/// service's surface. Every operation the model binds a body member on is here;
/// a key naming any other operation reads nothing from the body, so there is
/// nothing to fault.
pub fn body_is_unreadable<Eng: Send + Sync + 'static>(
    key: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> bool {
    unreadable_by_key!(
        key,
        protocol,
        req,
        create_namespace::CreateNamespace,
        create_table::CreateTable,
        create_table_bucket::CreateTableBucket,
        put_table_bucket_encryption::PutTableBucketEncryption,
        put_table_bucket_maintenance_configuration::PutTableBucketMaintenanceConfiguration,
        put_table_bucket_metrics_configuration::PutTableBucketMetricsConfiguration,
        put_table_bucket_policy::PutTableBucketPolicy,
        put_table_bucket_replication::PutTableBucketReplication,
        put_table_bucket_storage_class::PutTableBucketStorageClass,
        put_table_maintenance_configuration::PutTableMaintenanceConfiguration,
        put_table_policy::PutTablePolicy,
        put_table_record_expiration_configuration::PutTableRecordExpirationConfiguration,
        put_table_replication::PutTableReplication,
        rename_table::RenameTable,
        tag_resource::TagResource,
        update_table_metadata_location::UpdateTableMetadataLocation,
    )
}

/// The operation this service serves for a request whose body it cannot read
/// into the modelled input shape.
///
/// An [`ErasedOperation`] rather than a response built in
/// [`Service::override_operation`], because that seam hands the router a
/// BORROWED operation. It holds no state: the refusal is one fixed sentence, so
/// one value serves every such request.
///
/// `Eng` is the engine the service is generic over. It is never touched — the
/// refusal reads nothing and writes nothing — but naming it is what lets this
/// value sit on `EmulatedService<Eng>` beside the operations, which are generic
/// over the same engine.
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
        // Rendered before the future, so no borrow of the service's definition
        // is held across an await: the refusal is one fixed sentence and needs
        // nothing from the request.
        let protocol = svc
            .override_protocol()
            .unwrap_or_else(|| svc.definition().protocol());
        let refusal = BadRequestException {
            message: Some(crate::state::INVALID_INPUT_MESSAGE.to_string()),
        };
        let response = protocol
            .serialize_error(
                &refusal,
                &SCHEMA_BAD_REQUEST_EXCEPTION,
                super::BAD_REQUEST_CODE,
                BAD_REQUEST_STATUS,
            )
            .unwrap_or_else(|e| svc.definition().framework_error_response(e, req));
        Box::pin(async move { response })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", so
        // this layer cannot weaken the schemes the model declares for the
        // operation the request addressed.
        None
    }
}
