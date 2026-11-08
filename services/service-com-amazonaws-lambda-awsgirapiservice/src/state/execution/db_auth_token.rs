// SPDX-License-Identifier: CC0-1.0

//! The AUTHENTICATION TOKEN an SDK mints for a handler that connects to a database with IAM
//! database authentication.
//!
//! `generate_db_auth_token` is not an API call: the SDKs build the token in the CLIENT, by
//! SigV4-presigning a request to `connect` as one database user on one endpoint — "the
//! authentication token consists of … a signature", and "the token has a lifetime of 15
//! minutes" (Amazon RDS User Guide, IAM database authentication). So there is no service to
//! dispatch to, and the token has to be minted where the CREDENTIAL is: on the host.
//!
//! On AWS the signing credential is the function's own execution-role credential, which its
//! runtime puts in the environment. This emulator gives a run no credential at all — the
//! guest never holds one ([`super::aws_api`]) — so the token is signed with the credential
//! the framework hands this emulator's handlers, which is the identity every other
//! in-process call of this emulator is made under. What the guest reads back is the token:
//! the request it states, the credential's access key id and the signature over it, and no
//! secret.
//!
//! The signature itself is produced by `aws-sigv4`, the SDK's own signer, in the presigned
//! form (the signing values go in the query string), so the token is the one that signer
//! computes rather than a format written out here.

use std::time::{Duration, SystemTime};

use ::aws_sigv4::http_request::{
    sign, SignableBody, SignableRequest, SignatureLocation, SigningSettings,
};
use ::smithy_server_runtime::EmbeddedEmulator;

/// The signing service name an RDS authentication token is signed for, which is the service
/// its credential scope names.
const SIGNING_NAME: &str = "rds-db";

/// The action a token is minted for: `connect`, the only one it is generated for.
const TOKEN_ACTION: &str = "connect";

/// How long a token is valid for: "the token has a lifetime of 15 minutes" (Amazon RDS User
/// Guide, IAM database authentication), which is the `X-Amz-Expires` the presigned request
/// carries.
const TOKEN_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// The provider name the signing identity carries; it names the emulator rather than any
/// AWS credential source, and never travels in the token.
const PROVIDER_NAME: &str = "aws-emulator";

/// The characters a value keeps when it is written into the token's query string: a database
/// user's name is a caller-supplied value, and the signer signs the URI as it is given, so
/// what goes in is percent-encoded first.
const QUERY_VALUE: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Mint the token a login as `user` on `host`:`port` presents, signed for `region`.
///
/// `Err` carries what went wrong as text, for a guest that is told the token could not be
/// minted rather than handed something that authenticates nothing.
pub async fn db_auth_token(
    callback: &EmbeddedEmulator,
    host: &str,
    port: u16,
    user: &str,
    region: &str,
) -> Result<String, String> {
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
    // The presigned form: the signing values travel in the query string, which is what makes
    // the token a string a client can hand over as a password.
    let mut settings = SigningSettings::default();
    settings.signature_location = SignatureLocation::QueryParams;
    settings.expires_in = Some(TOKEN_LIFETIME);
    let params = ::aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(SIGNING_NAME)
        .settings(settings)
        .time(SystemTime::now())
        .build()
        .map_err(|error| error.to_string())?;
    let authority = format!("{host}:{port}");
    let encoded_user = ::percent_encoding::utf8_percent_encode(user, QUERY_VALUE).to_string();
    let uri = format!("https://{authority}/?Action={TOKEN_ACTION}&DBUser={encoded_user}");
    let signable = SignableRequest::new(
        "GET",
        &uri,
        std::iter::once(("host", authority.as_str())),
        SignableBody::Bytes(&[]),
    )
    .map_err(|error| error.to_string())?;
    let (instructions, _signature) = sign(signable, &params.into())
        .map_err(|error| error.to_string())?
        .into_parts();
    let mut request = ::http::Request::builder()
        .method("GET")
        .uri(&uri)
        .header("host", authority.clone())
        .body(Vec::<u8>::new())
        .map_err(|error| error.to_string())?;
    instructions.apply_to_request_http1x(&mut request);
    // The token is the signed request's address without its scheme, which is the form
    // `generate_db_auth_token` answers with and the form a database client takes as a
    // password.
    let signed = request.uri().to_string();
    Ok(signed
        .strip_prefix("https://")
        .unwrap_or(&signed)
        .to_string())
}
