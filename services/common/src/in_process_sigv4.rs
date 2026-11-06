// SPDX-License-Identifier: CC0-1.0

//! The ONE SigV4 signature an emulated service puts on an IN-PROCESS call it makes to
//! another emulated service.
//!
//! A service that calls another one over the callback transport
//! (`OperationContext::callback`) is a CALLER like any other, so the request it sends has to
//! carry the signature the emulator's own verifier expects — otherwise the call is served
//! only while request authentication happens to be off. Producing that signature takes two
//! things, and both are the same for every caller: the identity (the credential the
//! framework hands every handler, which is the only identity an in-process request here can
//! be signed as) and the SigV4 settings the emulator's own verifier re-signs with
//! (`vendor/server/src/request_authentication.rs`) — the payload-checksum kind it computes
//! over the body and the header set it does not exclude. Written once here, so a signature
//! one service produces is the signature that verifier computes.
//!
//! Nothing here reaches the network, the clock's state or a store: it is a pure
//! transformation of the request it is handed, and the only value it reads from outside is
//! the emulator's own credential, which it puts in the request's `Authorization` and nowhere
//! else — never in a response, a log or a file.
//!
//! Services still carrying their own copy of this recipe, written before this module
//! existed: `service-com-amazonaws-cloudformation-cloudformation`
//! (`state::provisioning::sign_callback_request`),
//! `service-com-amazonaws-ecr-amazonec2containerregistry-v20150921`,
//! `service-com-amazonaws-sfn-awsstepfunctions` and
//! `service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository`.
//! They are to be MIGRATED onto this module rather than copied again; nothing new writes the
//! recipe out — call this module.

use ::aws_sigv4::http_request::{
    sign, PayloadChecksumKind, SignableBody, SignableRequest, SigningSettings,
};
use ::smithy_server_runtime::OperationContext;

/// The provider name the signing identity carries: it names the emulator rather than any
/// AWS credential source, and never travels on the wire.
const PROVIDER_NAME: &str = "aws-emulator";

/// SigV4-sign `request` in place, for the service `signing_name` in `region`, with the
/// credential the framework hands this handler.
///
/// The settings are the ones the emulator's own verifier re-signs with, so the signature
/// this produces is the one that verifier computes over the same request — which is what
/// lets an in-process call be served with request authentication ON.
///
/// `Err` carries what went wrong as text, for a caller that reports a call it could not
/// make; nothing is signed on that path, so the request is left exactly as it was.
pub async fn sign_in_process_request(
    request: &mut ::http::Request<Vec<u8>>,
    signing_name: &str,
    region: &str,
    ctx: &OperationContext<'_>,
) -> Result<(), String> {
    sign_callback_request(request, signing_name, region, ctx.callback()).await
}

/// The same signature, for a caller that holds the CALLBACK rather than the operation
/// context it came from.
///
/// A call made outside the handler's own stack frame — from a task an operation spawned,
/// or from a host function a running engine calls — has the callback
/// ([`::smithy_server_runtime::EmbeddedEmulator`], which is `Clone` and `'static`) but no
/// borrowed [`OperationContext`]. It is the same identity and the same settings either
/// way, so there is one implementation and [`sign_in_process_request`] is this with the
/// context's callback handed in.
pub async fn sign_callback_request(
    request: &mut ::http::Request<Vec<u8>>,
    signing_name: &str,
    region: &str,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) -> Result<(), String> {
    use ::aws_credential_types::provider::ProvideCredentials;

    let credentials = callback
        .aws_credentials_provider()
        .provide_credentials()
        .await
        .map_err(|error| error.to_string())?;
    let identity: ::aws_smithy_runtime_api::client::identity::Identity =
        ::aws_credential_types::Credentials::builder()
            .provider_name(PROVIDER_NAME)
            .access_key_id(credentials.access_key_id())
            .secret_access_key(credentials.secret_access_key())
            .build()
            .into();
    let mut settings = SigningSettings::default();
    settings.payload_checksum_kind = PayloadChecksumKind::NoHeader;
    settings.excluded_headers = None;
    let params = ::aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(signing_name)
        .settings(settings)
        .time(::std::time::SystemTime::now())
        .build()
        .map_err(|error| error.to_string())?;
    let body = request.body().clone();
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        request.method().as_str(),
        request.uri().to_string(),
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(&body),
    )
    .map_err(|error| error.to_string())?;
    let (instructions, _signature) = sign(signable, &params.into())
        .map_err(|error| error.to_string())?
        .into_parts();
    instructions.apply_to_request_http1x(request);
    Ok(())
}
