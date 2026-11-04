// SPDX-License-Identifier: CC0-1.0

//! Resolving the KMS key a `StartSnapshot` names, through KMS.
//!
//! A snapshot encrypted under a caller-named key is a snapshot that depends on another
//! service's resource, and `StartSnapshotRequest$KmsKeyArn` is a reference to it rather
//! than a value this service can judge on its own: the ARN's form is all the model
//! constrains, and an ARN of the right form can still name a key that does not exist.
//! So the reference is RESOLVED — through the KMS endpoint of this same emulator, the way
//! every emulated service reaches another, over the in-process transport the operation
//! callback hands out ([`smithy_server_runtime::OperationContext::callback`]). No socket
//! is opened, no name is resolved, and nothing outside this process is reachable: the
//! endpoint URL below is ADDRESSING for that transport, which routes the call back into
//! this emulator's own KMS service. Its only variable part is the Region the request was
//! signed for, which names which regional endpoint of this emulator is addressed; no
//! caller-supplied value becomes a destination, and the caller's `Authorization` header is
//! never forwarded — the credentials are the emulator's own.
//!
//! A key KMS does not hold is a DEPENDENT SERVICE refusing the request, and that is what
//! the caller is told: the refusal relays KMS's own answer — the exception class, its
//! message, the status its class is answered with and the identifier of that answer —
//! inside this service's sentence for a dependency it could not use.
//!
//! An emulator assembled WITHOUT a KMS cannot resolve anything, so the reference is
//! accepted as given: the transport's own refusal says nothing about the key, and
//! refusing every encrypted snapshot would make this service unusable without KMS
//! mounted beside it.

use ::aws_sdk_kms::error::{ProvideErrorMetadata, SdkError};
use ::aws_smithy_runtime_api::client::orchestrator::HttpResponse;

/// The service name a relayed KMS refusal names itself by, which is KMS's own in the
/// wrapped failure this service relays.
const KMS_SERVICE_NAME: &str = "AWSKMS";

/// The package the relayed exception class is named under, which is KMS's own: the
/// wrapped failure is reported as `com.amazonaws.services.kms.model.<ErrorCode>`.
const KMS_EXCEPTION_PACKAGE: &str = "com.amazonaws.services.kms.model";

/// The status a relayed KMS refusal reports for a request KMS rejected, and the one for a
/// fault on its side: KMS answers a client-side refusal with 400 and a fault of its own
/// with 500, whichever exception shape carries it, so the status the relay states is the
/// one for the refusal's CLASS rather than the status line of one answer.
const KMS_CLIENT_STATUS: u16 = 400;
const KMS_FAULT_STATUS: u16 = 500;

/// The response headers a request identifier is read from, so the relay reports the
/// identifier of the answer it wraps whenever that answer carries one.
const REQUEST_ID_HEADERS: [&str; 2] = ["x-amzn-requestid", "x-amz-request-id"];

/// How the relayed clause states a field the wrapped answer did not carry.
const ABSENT_FIELD: &str = "null";

/// How this service introduces a dependent service's refusal: the fixed opening of the
/// `Message` it answers with, followed by the answer it relays.
const DEPENDENCY_RELAY_PREFIX: &str = "Invalid parameters for a dependent service:";

/// The outcome of resolving a `KmsKeyArn`.
#[derive(Debug)]
pub enum KeyReference {
    /// The key resolved, or there was no KMS to resolve it against (see the module
    /// docs): the request may create its snapshot.
    Usable,
    /// KMS refused the reference. The string is the `Message` this service answers with:
    /// its own sentence for a dependency it could not use, wrapping KMS's answer.
    Refused(String),
}

/// The AWS Region a request was signed for, which is the Region the key it names lives
/// in: a KMS key is a regional resource, so the same key identifier in another Region is
/// a different (absent) key. An unsigned request names no Region, so it is attributed to
/// the Region every emulated endpoint answers on by default.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The Region an unsigned request is attributed to, which is the one every emulated
/// endpoint answers on when a request's credential scope names none.
const DEFAULT_REGION: &str = "us-east-1";

/// Resolve `kms_key_arn` through the KMS endpoint of this emulator, in the Region the
/// request was signed for.
///
/// `DescribeKey` is the read that resolves a key reference and nothing else: it reports
/// the key's metadata, so a key KMS holds answers it and a key it does not hold is
/// refused with the not-found class this relays.
pub async fn resolve_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kms_key_arn: &str,
) -> KeyReference {
    match kms_client(ctx, region)
        .describe_key()
        .key_id(kms_key_arn)
        .send()
        .await
    {
        Ok(_) => KeyReference::Usable,
        Err(error) => classify(&error),
    }
}

/// Which [`KeyReference`] an SDK error from the resolving call is.
///
/// A refusal KMS stated is relayed as it was answered: the exception class it stated it
/// under, the message it stated, the status its class is answered with, and the
/// identifier of that answer — a field the answer did not carry is stated as absent
/// rather than invented. An error that reached no KMS at all, and a fault that reached no
/// answer, say nothing about the key, so the reference is left as given.
fn classify<E: ProvideErrorMetadata + ::std::error::Error + 'static>(
    error: &SdkError<E, HttpResponse>,
) -> KeyReference {
    let SdkError::ServiceError(context) = error else {
        ::tracing::debug!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
            "KMS did not answer the key reference of this snapshot"
        );
        return KeyReference::Usable;
    };
    let refused = context.err();
    let Some(code) = refused.code() else {
        // An answer with no error code names no class to relay.
        return KeyReference::Usable;
    };
    if TRANSPORT_REFUSALS.contains(&code) {
        ::tracing::debug!(
            code,
            "no KMS answered the key reference of this snapshot"
        );
        return KeyReference::Usable;
    }
    let answered = context.raw();
    let status = if answered.status().as_u16() >= KMS_FAULT_STATUS {
        KMS_FAULT_STATUS
    } else {
        KMS_CLIENT_STATUS
    };
    let request_id = REQUEST_ID_HEADERS
        .into_iter()
        .find_map(|header| answered.headers().get(header))
        .map(str::to_string)
        // A request identifier belongs to the ANSWER, and the refusal is relayed with the
        // one that answer carried. An answer that carries none is still an answer to one
        // request, so the relay states an identifier of that form for it rather than
        // leaving the field out of a clause that states it.
        .unwrap_or_else(|| ::uuid::Uuid::new_v4().to_string());
    let clause = format!(
        "(Service: {KMS_SERVICE_NAME}; Status Code: {status}; Error Code: {code}; \
         Request ID: {request_id}; Proxy: {ABSENT_FIELD})"
    );
    let relayed = match refused.message() {
        Some(message) => format!("{KMS_EXCEPTION_PACKAGE}.{code}: {message} {clause}"),
        None => format!("{KMS_EXCEPTION_PACKAGE}.{code}: {clause}"),
    };
    KeyReference::Refused(format!("{DEPENDENCY_RELAY_PREFIX} {relayed}"))
}

/// The codes the TRANSPORT answers when this emulator was assembled without a KMS, or
/// when more than one of its services claims a KMS request. Either way the call reached
/// no KMS, so it says nothing about the key — the same reading every service that calls
/// another in process makes of these two codes.
const TRANSPORT_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// A KMS client over the in-process callback into this same emulator.
///
/// `region` is the request's own signing Region, so the key is resolved where the
/// snapshot that names it is created. The transport is in-process, so there is nothing to
/// retry, time out or watch for a stalled stream — each would need a sleep implementation
/// this client is not given — and the credentials the callback hands over are used
/// directly rather than cached. Those credentials are the emulator's own: the caller's
/// `Authorization` header is never forwarded.
fn kms_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}
