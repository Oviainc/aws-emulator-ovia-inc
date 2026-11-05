// SPDX-License-Identifier: CC0-1.0

//! The judgement every WAFv2 operation makes about the REQUEST ITSELF, before it
//! resolves anything the request names or touches the store: the `Scope` the request
//! names, against the endpoint it was signed for.
//!
//! The ONLY thing it reads out of the request is the SigV4 credential scope's
//! region, through the framework's own reader ([`request_region`]); it reads no
//! environment variable, no host and no file, and it compares strings — nothing here
//! indexes, slices or unwraps a value a caller controls, so no request can reach a
//! panicking construct through it.
//!
//! It lives here, beside the file that decides which requests this service claims
//! and how it words what the framework refuses, rather than in [`crate::state`],
//! which is the store: it neither reads nor writes it. Every handler calls it by
//! this path, so what a handler reaches through `crate::state` is state and nothing
//! else.

use crate::ops::types::WAFInvalidParameterException;
use crate::state::{invalid_scope_exception, is_known_scope, DEFAULT_REGION, SCOPE_CLOUDFRONT};

/// The AWS region the request was signed for, or [`DEFAULT_REGION`] when it
/// carries no SigV4 credential scope to read one from.
///
/// The credential scope is parsed by the framework's own reader, which answers
/// `None` for a request carrying no usable `Authorization` header, so this reads
/// nothing else out of the request and cannot fail on a malformed one.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ONE region endpoint that serves the CLOUDFRONT scope.
///
/// "To work with CloudFront, you must also specify the Region US East (N.
/// Virginia) as follows: CLI - Specify the Region when you use the CloudFront
/// scope: --scope=CLOUDFRONT --region=us-east-1. API and SDKs - For all calls, use
/// the Region endpoint us-east-1" (`Scope`'s own doc): the global namespace lives
/// in that one region, so no other endpoint can be asked about it.
pub const CLOUDFRONT_SCOPE_REGION: &str = "us-east-1";

/// `Ok(())` when a request signed for this endpoint may name `scope`, else the
/// refusal WAF answers.
///
/// Two conditions, one refusal: a value the `Scope` enum does not declare is no
/// scope at all, and the CLOUDFRONT scope is served ONLY at
/// [`CLOUDFRONT_SCOPE_REGION`] (see that constant), so a request signed for any
/// other region names a scope this endpoint cannot act in. Both are a bad value of
/// the `Scope` member, judged from the request alone — so every operation that
/// takes a `Scope` makes this judgement FIRST, before it resolves anything the
/// request names: a caller at the wrong endpoint is told its scope is wrong rather
/// than that its resource does not exist (in the global namespace the regional
/// endpoint cannot see, nothing does).
pub fn validate_scope(
    scope: &str,
    parts: &::http::request::Parts,
) -> Result<(), WAFInvalidParameterException> {
    let served_here =
        scope != SCOPE_CLOUDFRONT || request_region(parts) == CLOUDFRONT_SCOPE_REGION;
    if !is_known_scope(scope) || !served_here {
        return Err(invalid_scope_exception(scope));
    }
    Ok(())
}
