// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of a compact JWS — a JSON Web Token — for every service that is handed
//! one as a caller-supplied string and has to decide whether it is a token at all.
//!
//! A JWT is three `.`-separated base64url segments: a JOSE header and a claim set, each
//! base64url-encoded JSON, and a signature (RFC 7515 §3.1, RFC 7519 §3). Several emulated
//! services are handed such a value — an HTTP api's JWT authorizer evaluates a presented
//! bearer token, a cluster's authentication reads a service-account token, a Linux
//! subscription registration reads the provider access token held in a secret — and each
//! has to answer the same first question before it can look at a single claim: is this a
//! token, and what does it claim? That reading lives here exactly once, so two services
//! handed the same value cannot disagree about whether it is a token.
//!
//! What is checked here is the FORM, and the form only. Each caller then applies its own
//! rule to the claims it cares about ([`CompactJws::claim_str`]), and words its own
//! refusal: this module reports WHY a value is not a token ([`FormDefect`], which renders
//! the reason), never what any one service should answer for it.
//!
//! The SIGNATURE is not verified, and cannot be: verifying it needs the issuer's
//! verification key, published at that issuer's own key-set endpoint, and this emulator
//! makes no outbound connection to fetch one. What IS decidable from the token alone is
//! refused — a token whose header names the unsecured algorithm of RFC 7519 §6 (`alg`
//! `none`), or whose signature segment is empty, is one no issuer signed.
//!
//! Services with their own copy of this reading — `service/jwt.rs` in
//! `service-com-amazonaws-apigatewayv2-apigatewayv2` and `parse_service_account_token` in
//! `service-com-amazonaws-eksauth-eksauthfrontend` — are to be MIGRATED onto it, each in
//! the change that owns its service (the convention [`crate::iam_client`] records for the
//! same situation). Nothing new should read a token by hand.
//!
//! A presentation prefix is NOT this module's concern: a token travelling in an
//! `Authorization` header carries the RFC 6750 `Bearer` scheme in front of it, which the
//! service reading that header strips before calling [`read`].

/// The largest token this module reads, in bytes.
///
/// A CHOSEN bound, not a documented one: it caps the base64 decode and the JSON parse one
/// request can ask for, so a caller-supplied value cannot be turned into unbounded work. It
/// is far above every token an issuer mints (an access token is roughly one kilobyte) and
/// far below the request budget any front door accepts.
pub const MAX_TOKEN_BYTES: usize = 8 * 1024;

/// The JOSE header parameter naming the algorithm a JWS was signed with (RFC 7515 §4.1.1,
/// the one header JWS makes mandatory).
const ALGORITHM_HEADER: &str = "alg";

/// The `alg` value of an UNSECURED JWT — "a JWS using the 'alg' value of 'none'"
/// (RFC 7519 §6.1) — which is a token with no signature at all.
const UNSECURED_ALGORITHM: &str = "none";

/// Why a value is not a compact JWS carrying a JSON claim set.
///
/// `Display` renders the reason as a clause about the value ("its header segment is not
/// base64url-encoded JSON"), so a service can state its own refusal without holding a
/// second copy of these reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormDefect {
    /// There is nothing to read: the value is empty, or all whitespace.
    Empty,
    /// Longer than [`MAX_TOKEN_BYTES`], so it is not read at all.
    TooLarge,
    /// Not three `.`-separated segments.
    Segments,
    /// The header segment is not base64url-encoded JSON, or does not decode to a JSON
    /// object — neither is a JOSE header.
    Header,
    /// The header segment names no signing algorithm.
    Algorithm,
    /// The claim-set segment is not base64url-encoded JSON, or does not decode to a JSON
    /// object.
    Claims,
    /// Unsecured: the header names `alg` `none`, or the signature segment is empty, so
    /// nothing signed the token.
    Unsecured,
}

impl ::std::fmt::Display for FormDefect {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self {
            Self::Empty => f.write_str("it is empty"),
            Self::TooLarge => write!(f, "it is longer than {MAX_TOKEN_BYTES} bytes"),
            Self::Segments => f.write_str(
                "it is not a JSON Web Token (three '.'-separated base64url segments)",
            ),
            Self::Header => {
                f.write_str("its header segment is not base64url-encoded JSON")
            }
            Self::Algorithm => f.write_str("its header segment names no signing algorithm"),
            Self::Claims => {
                f.write_str("its payload segment is not base64url-encoded JSON")
            }
            Self::Unsecured => {
                f.write_str("it is unsecured, so no issuer signed it")
            }
        }
    }
}

/// A compact JWS whose FORM this module accepted: the algorithm its header names, that
/// header, and its claim set. Nothing has been read out of the claims yet — which claims
/// matter is the calling service's rule, not this module's.
#[derive(Debug, Clone)]
pub struct CompactJws {
    /// The `alg` the JOSE header names, trimmed and non-empty.
    pub algorithm: String,
    /// The JOSE header, as the JSON object it decoded to.
    pub header: ::serde_json::Map<String, ::serde_json::Value>,
    /// The claim set, as the JSON object it decoded to.
    pub claims: ::serde_json::Map<String, ::serde_json::Value>,
}

impl CompactJws {
    /// The claim `name`, when the token carries it as a string.
    #[must_use]
    pub fn claim_str(&self, name: &str) -> Option<&str> {
        self.claims.get(name).and_then(::serde_json::Value::as_str)
    }
}

/// Read `presented` as a compact JWS and report its header and claim set.
///
/// Total: a value that is not a token is a [`FormDefect`], never a panic — nothing is
/// indexed, sliced or unwrapped on the way through.
pub fn read(presented: &str) -> Result<CompactJws, FormDefect> {
    let token = presented.trim();
    if token.is_empty() {
        return Err(FormDefect::Empty);
    }
    if token.len() > MAX_TOKEN_BYTES {
        return Err(FormDefect::TooLarge);
    }
    let mut segments = token.split('.');
    let (Some(header), Some(claims), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(FormDefect::Segments);
    };
    let header = json_segment(header).ok_or(FormDefect::Header)?;
    let algorithm = header
        .get(ALGORITHM_HEADER)
        .and_then(::serde_json::Value::as_str)
        .map(str::trim)
        .filter(|algorithm| !algorithm.is_empty())
        .ok_or(FormDefect::Algorithm)?
        .to_string();
    let claims = json_segment(claims).ok_or(FormDefect::Claims)?;
    // Decidable from the token alone: an unsecured JWT (RFC 7519 §6) is one nothing
    // signed, whatever its claims say.
    if algorithm.eq_ignore_ascii_case(UNSECURED_ALGORITHM) || signature.is_empty() {
        return Err(FormDefect::Unsecured);
    }
    Ok(CompactJws {
        algorithm,
        header,
        claims,
    })
}

/// One base64url segment of a token, read as a JSON object. `None` when the segment is not
/// base64url or does not decode to a JSON object — neither is a JOSE header or a claim set.
fn json_segment(segment: &str) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
    let bytes = decode_segment(segment)?;
    match ::serde_json::from_slice::<::serde_json::Value>(&bytes) {
        Ok(::serde_json::Value::Object(members)) => Some(members),
        _ => None,
    }
}

/// Decode one segment of a compact JWS: base64url without padding (RFC 7515 §2, which is
/// RFC 4648 §5 "with all trailing '=' characters omitted"), accepting a presenter that
/// padded it anyway.
///
/// The codec is `aws_smithy_types::base64`, the standard-alphabet decoder this workspace
/// already uses for every Smithy blob, plus the two alphabet substitutions (`-` -> `+`,
/// `_` -> `/`) and the restored padding that turn §5 back into §4. No second codec is
/// introduced for it.
fn decode_segment(segment: &str) -> Option<Vec<u8>> {
    let mut standard = segment
        .trim_end_matches('=')
        .replace('-', "+")
        .replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    ::aws_smithy_types::base64::decode(&standard).ok()
}

#[cfg(test)]
mod tests {
    //! The form rule, over the shapes a value handed to this module takes. The SERVED
    //! behaviour — which refusal each defect becomes — is pinned by the suites of the
    //! services that call it.

    use super::{read, CompactJws, FormDefect, MAX_TOKEN_BYTES};

    /// One base64url segment (unpadded, as a compact JWS carries it) of `value`.
    fn segment(value: &str) -> String {
        ::aws_smithy_types::base64::encode(value)
            .trim_end_matches('=')
            .replace('+', "-")
            .replace('/', "_")
    }

    /// A token whose header and claim set are `header` and `claims`.
    fn token(header: &str, claims: &str) -> String {
        format!("{}.{}.{}", segment(header), segment(claims), segment("sig"))
    }

    #[test]
    fn a_compact_jws_reports_its_algorithm_and_claims() {
        let presented = token(r#"{"alg":"RS256","typ":"JWT"}"#, r#"{"iss":"issuer","typ":"Offline"}"#);
        let read = read(&presented).expect("a well-formed token reads");
        let CompactJws { algorithm, .. } = &read;
        assert_eq!(
            algorithm, "RS256",
            "the algorithm the header names must be reported, got {algorithm:?}"
        );
        assert_eq!(
            read.claim_str("typ"),
            Some("Offline"),
            "a string claim must be readable, got {:?}",
            read.claims
        );
        assert_eq!(
            read.claim_str("absent"),
            None,
            "a claim the token does not carry must read as absent"
        );
    }

    #[test]
    fn a_value_that_is_not_a_token_reports_which_part_is_wrong() {
        let oversized = format!(
            "{}.{}.{}",
            segment(r#"{"alg":"RS256"}"#),
            "a".repeat(MAX_TOKEN_BYTES),
            segment("sig")
        );
        let rows: Vec<(&str, String, FormDefect)> = vec![
            ("empty", String::new(), FormDefect::Empty),
            ("whitespace only", "  \n".to_string(), FormDefect::Empty),
            ("oversized", oversized, FormDefect::TooLarge),
            ("no segments", "opaque-value".to_string(), FormDefect::Segments),
            (
                "two segments",
                format!("{}.{}", segment(r#"{"alg":"RS256"}"#), segment("{}")),
                FormDefect::Segments,
            ),
            (
                "four segments",
                format!("{}.{}.{}.{}", segment(r#"{"alg":"RS256"}"#), segment("{}"), segment("sig"), segment("extra")),
                FormDefect::Segments,
            ),
            (
                "header not base64url",
                format!("@@@@.{}.{}", segment("{}"), segment("sig")),
                FormDefect::Header,
            ),
            (
                "header not a JSON object",
                format!("{}.{}.{}", segment(r#"["RS256"]"#), segment("{}"), segment("sig")),
                FormDefect::Header,
            ),
            (
                "header names no algorithm",
                token(r#"{"typ":"JWT"}"#, "{}"),
                FormDefect::Algorithm,
            ),
            (
                "claims not JSON",
                format!("{}.{}.{}", segment(r#"{"alg":"RS256"}"#), segment("not json"), segment("sig")),
                FormDefect::Claims,
            ),
            (
                "unsecured algorithm",
                token(r#"{"alg":"none"}"#, "{}"),
                FormDefect::Unsecured,
            ),
            (
                "empty signature segment",
                format!("{}.{}.", segment(r#"{"alg":"RS256"}"#), segment("{}")),
                FormDefect::Unsecured,
            ),
        ];

        for (case, presented, expected) in rows {
            let actual = read(&presented).err();
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !expected.to_string().is_empty(),
                "case {case:?}: every defect must render a reason"
            );
        }
    }

    #[test]
    fn a_padded_segment_reads_the_same_as_an_unpadded_one() {
        let header = r#"{"alg":"RS256","typ":"JWT"}"#;
        let claims = r#"{"typ":"Offline"}"#;
        let padded = format!(
            "{}.{}.{}",
            ::aws_smithy_types::base64::encode(header),
            ::aws_smithy_types::base64::encode(claims),
            ::aws_smithy_types::base64::encode("sig")
        );
        let read_padded = read(&padded).expect("a padded token reads");
        let read_plain = read(&token(header, claims)).expect("an unpadded token reads");
        assert_eq!(
            read_padded.claim_str("typ"),
            read_plain.claim_str("typ"),
            "padding must not change what a token claims, got {:?} and {:?}",
            read_padded.claims,
            read_plain.claims
        );
    }
}
