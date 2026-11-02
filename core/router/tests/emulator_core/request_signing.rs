// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Signs a request with SigV4 as an SDK does: in the `Authorization` header or a presigned query.

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest};
use aws_sigv4::http_request::{SignatureLocation, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use std::time::{Duration, SystemTime};

/// The region the published `config` names, which an SDK signs for.
pub const SIGNING_REGION: &str = "us-east-1";

/// Who signs, for which service, when, and for a presigned URL how long it is valid.
#[derive(Clone)]
pub struct Signer {
    pub key: String,
    pub secret: String,
    pub signing_name: String,
    pub session_token: Option<String>,
    pub time: SystemTime,
    pub presign_for: Option<Duration>,
}

impl Signer {
    /// Signs now, in the `Authorization` header, with a key pair and no session token.
    pub fn new(key: String, secret: String, signing_name: &str) -> Self {
        Self {
            key,
            secret,
            signing_name: signing_name.to_string(),
            session_token: None,
            time: SystemTime::now(),
            presign_for: None,
        }
    }
}

/// `request` signed by `signer`; a declared `x-amz-content-sha256` is signed as written.
pub fn signed(mut request: http::Request<Vec<u8>>, signer: &Signer) -> http::Request<Vec<u8>> {
    let token = signer.session_token.clone();
    let identity = Credentials::new(&signer.key, &signer.secret, token, None, "signer").into();
    let params = SigningParams::builder().identity(&identity);
    let params = params.region(SIGNING_REGION).name(&signer.signing_name);
    let params = params.time(signer.time);
    let params = params.settings(settings(signer)).build().unwrap().into();
    let (method, uri) = (request.method().to_string(), request.uri().to_string());
    let headers = request.headers().iter();
    let headers = headers.map(|(name, value)| (name.as_str(), value.to_str().unwrap()));
    let declared = request.headers().get("x-amz-content-sha256");
    let declared = declared.map(|payload| payload.to_str().unwrap().to_string());
    let bytes = SignableBody::Bytes(request.body());
    let body = declared.map_or(bytes, SignableBody::Precomputed);
    let signable = SignableRequest::new(&method, &uri, headers, body).unwrap();
    let (instructions, _) = sign(signable, &params).unwrap().into_parts();
    instructions.apply_to_request_http1x(&mut request);
    request
}

fn settings(signer: &Signer) -> SigningSettings {
    let mut settings = SigningSettings::default();
    if let Some(valid_for) = signer.presign_for {
        settings.signature_location = SignatureLocation::QueryParams;
        settings.expires_in = Some(valid_for);
    }
    settings
}
