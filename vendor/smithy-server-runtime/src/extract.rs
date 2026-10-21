// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
/// Extraction from HTTP request parts (headers, URI, etc.) without consuming the body.
///
/// This trait is inspired by axum's `FromRequestParts` but simplified for Smithy services.
/// Extractors can read headers, extensions, and other request metadata to provide
/// per-request context to handlers.
pub trait FromSmithyParts: Sized {
    type Rejection: std::error::Error + Send + Sync + 'static;

    fn from_smithy_parts(parts: &http::request::Parts) -> Result<Self, Self::Rejection>;
}

/// Error type for extractor failures
#[derive(Debug)]
pub struct ExtractorError(pub String);

impl std::fmt::Display for ExtractorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Extractor error: {}", self.0)
    }
}

impl std::error::Error for ExtractorError {}

/// AWS Access Key ID extracted from the Authorization header.
///
/// Parses the SigV4 `Credential=AKID/...` component. Falls back to `"default"` if missing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Akid(pub String);

impl FromSmithyParts for Akid {
    type Rejection = std::convert::Infallible;

    fn from_smithy_parts(parts: &http::request::Parts) -> Result<Self, Self::Rejection> {
        Ok(Akid(emulator_server::extract_akid(&parts.headers)))
    }
}

/// Telemetry dimensions derived from one request: SDK, language, agent client
/// and the SigV4 credential-scope region. Only the parsed, closed values are
/// kept; the raw `User-Agent` never leaves [`RequestTelemetry::from_headers`].
///
/// Empty whenever telemetry is not recording, so a disabled emulator never
/// parses request headers for telemetry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestTelemetry(Option<emulator_telemetry::SanitizedRequestMetadata>);

impl RequestTelemetry {
    /// Derive the telemetry dimensions from request headers. Usable by
    /// handlers that receive a raw `http::Request` instead of extractors.
    pub fn from_headers(headers: &http::HeaderMap) -> Self {
        if !emulator_telemetry::recording_active() {
            return Self(None);
        }
        Self(Some(Self::sanitize(headers)))
    }

    fn sanitize(headers: &http::HeaderMap) -> emulator_telemetry::SanitizedRequestMetadata {
        let region = emulator_server::sigv4_credential_scope(headers)
            .map(|scope| emulator_telemetry::Region::fold(scope.region));
        emulator_telemetry::sanitize_request_metadata(headers, region)
    }

    /// The metadata to hand to the telemetry recording API.
    pub fn into_metadata(self) -> Option<emulator_telemetry::SanitizedRequestMetadata> {
        self.0
    }
}

impl FromSmithyParts for RequestTelemetry {
    type Rejection = std::convert::Infallible;

    fn from_smithy_parts(parts: &http::request::Parts) -> Result<Self, Self::Rejection> {
        Ok(Self::from_headers(&parts.headers))
    }
}

// Tuple impls for 0-4 extractors

impl FromSmithyParts for () {
    type Rejection = std::convert::Infallible;

    fn from_smithy_parts(_: &http::request::Parts) -> Result<Self, Self::Rejection> {
        Ok(())
    }
}

impl<T1: FromSmithyParts> FromSmithyParts for (T1,) {
    type Rejection = T1::Rejection;

    fn from_smithy_parts(parts: &http::request::Parts) -> Result<Self, Self::Rejection> {
        Ok((T1::from_smithy_parts(parts)?,))
    }
}

impl<T1: FromSmithyParts, T2: FromSmithyParts> FromSmithyParts for (T1, T2) {
    type Rejection = ExtractorError;

    fn from_smithy_parts(parts: &http::request::Parts) -> Result<Self, Self::Rejection> {
        Ok((
            T1::from_smithy_parts(parts).map_err(|e| ExtractorError(e.to_string()))?,
            T2::from_smithy_parts(parts).map_err(|e| ExtractorError(e.to_string()))?,
        ))
    }
}

impl<T1: FromSmithyParts, T2: FromSmithyParts, T3: FromSmithyParts> FromSmithyParts
    for (T1, T2, T3)
{
    type Rejection = ExtractorError;

    fn from_smithy_parts(parts: &http::request::Parts) -> Result<Self, Self::Rejection> {
        Ok((
            T1::from_smithy_parts(parts).map_err(|e| ExtractorError(e.to_string()))?,
            T2::from_smithy_parts(parts).map_err(|e| ExtractorError(e.to_string()))?,
            T3::from_smithy_parts(parts).map_err(|e| ExtractorError(e.to_string()))?,
        ))
    }
}

impl<T1: FromSmithyParts, T2: FromSmithyParts, T3: FromSmithyParts, T4: FromSmithyParts>
    FromSmithyParts for (T1, T2, T3, T4)
{
    type Rejection = ExtractorError;

    fn from_smithy_parts(parts: &http::request::Parts) -> Result<Self, Self::Rejection> {
        Ok((
            T1::from_smithy_parts(parts).map_err(|e| ExtractorError(e.to_string()))?,
            T2::from_smithy_parts(parts).map_err(|e| ExtractorError(e.to_string()))?,
            T3::from_smithy_parts(parts).map_err(|e| ExtractorError(e.to_string()))?,
            T4::from_smithy_parts(parts).map_err(|e| ExtractorError(e.to_string()))?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emulator_telemetry::{sanitize_request_metadata, Region};

    const USER_AGENT: &str =
        "aws-sdk-rust/1.82.0 os/linux lang/rust/1.85.1 md/claude-code customer@example.com";

    fn headers(authorization: Option<&str>) -> http::HeaderMap {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::USER_AGENT, USER_AGENT.parse().unwrap());
        if let Some(authorization) = authorization {
            headers.insert(http::header::AUTHORIZATION, authorization.parse().unwrap());
        }
        headers
    }

    fn signed_for(region: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260925/{region}/sqs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef"
        )
    }

    #[test]
    fn region_comes_from_the_sigv4_credential_scope() {
        let headers = headers(Some(&signed_for("eu-west-1")));
        assert_eq!(
            RequestTelemetry::sanitize(&headers),
            sanitize_request_metadata(&headers, Region::new("eu-west-1").ok()),
        );
    }

    #[test]
    fn an_unrecognized_scope_region_folds_to_unknown() {
        let headers = headers(Some(&signed_for("customer-secret-9")));
        assert_eq!(
            RequestTelemetry::sanitize(&headers),
            sanitize_request_metadata(&headers, Some(Region::unknown())),
        );
    }

    #[test]
    fn an_unsigned_request_has_no_region() {
        let headers = headers(None);
        assert_eq!(
            RequestTelemetry::sanitize(&headers),
            sanitize_request_metadata(&headers, None),
        );
    }

    #[test]
    fn only_parsed_dimensions_are_retained() {
        let retained = format!(
            "{:?}",
            RequestTelemetry::sanitize(&headers(Some(&signed_for("eu-west-1"))))
        );
        for raw in [
            USER_AGENT,
            "customer@example.com",
            "AKIDEXAMPLE",
            "deadbeef",
        ] {
            assert!(!retained.contains(raw), "{raw} leaked into {retained}");
        }
    }

    #[test]
    fn nothing_is_derived_while_telemetry_is_not_recording() {
        let (parts, ()) = http::Request::builder()
            .header(http::header::USER_AGENT, USER_AGENT)
            .body(())
            .unwrap()
            .into_parts();
        let extracted = RequestTelemetry::from_smithy_parts(&parts).unwrap();
        assert_eq!(extracted.into_metadata(), None);
    }
}
