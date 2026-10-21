// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! Sanitization of request metadata at the router boundary.

use crate::policy;
use crate::types::{AgentClient, LanguageVersion, Region, SdkName, SdkVersion};

/// A per-session merged dimension: unset until observed, a single stable
/// value, or `Mixed` once two different values have been observed. `Mixed` is
/// sticky for the session — a later agreeing observation can never un-mix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Merged<T> {
    Unset,
    Value(T),
    Mixed,
}

impl<T: PartialEq> Merged<T> {
    pub(crate) fn absorb(&mut self, incoming: Merged<T>) {
        match (&*self, &incoming) {
            (_, Merged::Unset) => {}
            (Merged::Mixed, _) => {}
            (Merged::Unset, _) => *self = incoming,
            (Merged::Value(current), Merged::Value(new)) if current == new => {}
            (Merged::Value(_), _) => *self = Merged::Mixed,
        }
    }

    pub(crate) fn observe(&mut self, incoming: Option<T>) {
        if let Some(value) = incoming {
            self.absorb(Merged::Value(value));
        }
    }

    pub(crate) fn value(&self) -> Option<&T> {
        match self {
            Merged::Value(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn is_mixed(&self) -> bool {
        matches!(self, Merged::Mixed)
    }
}

/// Closed, non-identifying request context safe to aggregate into telemetry.
/// Every field is a validated domain type; raw header values are dropped
/// before [`sanitize_request_metadata`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedRequestMetadata {
    pub(crate) sdk: Merged<SdkName>,
    pub(crate) sdk_version: Option<SdkVersion>,
    pub(crate) language_version: Option<LanguageVersion>,
    pub(crate) region: Option<Region>,
    pub(crate) agent_client: Option<AgentClient>,
}

impl Default for SanitizedRequestMetadata {
    fn default() -> Self {
        Self {
            sdk: Merged::Unset,
            sdk_version: None,
            language_version: None,
            region: None,
            agent_client: None,
        }
    }
}

/// Read request headers once and retain only closed or tightly validated
/// dimensions. The caller passes the request's SigV4 credential-scope region
/// (parsed once at the router by the shared SigV4 scope parser) already
/// validated as a [`Region`]; no authorization material enters this function.
pub fn sanitize_request_metadata(
    headers: &http::HeaderMap,
    region: Option<Region>,
) -> SanitizedRequestMetadata {
    let mut metadata = SanitizedRequestMetadata {
        region,
        ..SanitizedRequestMetadata::default()
    };
    if let Some(user_agent) = headers
        .get(http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
    {
        let lower = user_agent.to_ascii_lowercase();
        (metadata.sdk, metadata.sdk_version) = detect_sdk(&lower);
        metadata.language_version = policy::LANGUAGE_VERSION_MARKERS
            .iter()
            .find_map(|marker| bounded_version_after(&lower, marker))
            .and_then(|fragment| LanguageVersion::new(fragment).ok());
        metadata.agent_client = detect_agent_client(&lower);
    }
    metadata
}

fn detect_sdk(user_agent: &str) -> (Merged<SdkName>, Option<SdkVersion>) {
    let recognized: Vec<_> = policy::SDK_USER_AGENT_MARKERS
        .iter()
        .filter(|(_, markers)| markers.iter().any(|marker| user_agent.contains(marker)))
        .collect();
    match recognized.as_slice() {
        [] => (Merged::Unset, None),
        [(sdk_name, markers)] => (
            Merged::Value(*sdk_name),
            markers
                .iter()
                .find_map(|marker| bounded_version_after(user_agent, marker))
                .and_then(|fragment| SdkVersion::new(fragment).ok()),
        ),
        _ => (Merged::Mixed, None),
    }
}

fn detect_agent_client(user_agent: &str) -> Option<AgentClient> {
    for (agent, markers) in policy::AGENT_CLIENT_MARKERS {
        if markers.iter().any(|marker| user_agent.contains(marker)) {
            return Some(*agent);
        }
    }
    if policy::AGENT_GENERIC_MARKERS
        .iter()
        .any(|marker| user_agent.contains(marker))
    {
        return Some(AgentClient::Other);
    }
    None
}

fn bounded_version_after(value: &str, marker: &str) -> Option<String> {
    let token = value
        .split_ascii_whitespace()
        .find(|token| token.starts_with(marker))?;
    // The version starts after the token's last slash, not after the marker:
    // a detection marker may pin leading version digits (`aws-sdk-js/3.`)
    // that belong to the recorded version, and the Rust SDK nests the
    // language before its version (`lang/rust/1.85.1`).
    let (_, tail) = token.rsplit_once('/')?;
    let version: String = tail
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .take(policy::VERSION_FRAGMENT_MAX_LEN)
        .collect();
    if version.is_empty()
        || version.starts_with('.')
        || version.ends_with('.')
        || version.contains("..")
    {
        None
    } else {
        Some(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_closed_sdk_language_and_validated_region() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            "aws-sdk-rust/1.82.0 rust/1.85.1 md/agent customer@example.com /private/key"
                .parse()
                .unwrap(),
        );
        headers.insert(
            http::header::HOST,
            "bucket.customer.example".parse().unwrap(),
        );

        let metadata = sanitize_request_metadata(&headers, Some(Region::new("us-west-2").unwrap()));
        assert_eq!(metadata.sdk, Merged::Value(SdkName::AwsSdkRust));
        assert_eq!(
            metadata.sdk_version,
            Some(SdkVersion::new("1.82.0").unwrap())
        );
        assert_eq!(
            metadata.language_version,
            Some(LanguageVersion::new("1.85.1").unwrap())
        );
        assert_eq!(metadata.region, Some(Region::new("us-west-2").unwrap()));
        assert_eq!(metadata.agent_client, Some(AgentClient::Other));
        let debug = format!("{metadata:?}");
        for forbidden in [
            "customer@example.com",
            "/private/key",
            "bucket.customer.example",
        ] {
            assert!(!debug.contains(forbidden));
        }
    }

    #[test]
    fn reads_the_rust_sdk_nested_language_token() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            "aws-sdk-rust/1.82.0 ua/2.1 api/sqs/1.60.0 os/linux lang/rust/1.85.1"
                .parse()
                .unwrap(),
        );
        let metadata = sanitize_request_metadata(&headers, None);
        assert_eq!(metadata.sdk, Merged::Value(SdkName::AwsSdkRust));
        assert_eq!(
            metadata.language_version,
            Some(LanguageVersion::new("1.85.1").unwrap())
        );
    }

    #[test]
    fn unrecognized_or_malformed_values_collapse_to_safe_unknowns() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            "custom-client raw-user-data".parse().unwrap(),
        );
        let metadata = sanitize_request_metadata(&headers, None);
        assert_eq!(metadata.sdk, Merged::Unset);
        assert_eq!(metadata.sdk_version, None);
        assert_eq!(metadata.language_version, None);
        assert_eq!(metadata.region, None);
        assert_eq!(metadata.agent_client, None);
    }

    #[test]
    fn folds_multiple_sdks_and_identifies_closed_agent_clients() {
        let mut mixed = http::HeaderMap::new();
        mixed.insert(
            http::header::USER_AGENT,
            "aws-sdk-rust/1.82.0 aws-sdk-java/2.30.0".parse().unwrap(),
        );
        let mixed = sanitize_request_metadata(&mixed, None);
        assert_eq!(mixed.sdk, Merged::Mixed);
        assert_eq!(mixed.sdk_version, None);

        let mut javascript = http::HeaderMap::new();
        javascript.insert(
            http::header::USER_AGENT,
            "aws-sdk-js-v3/3.900.0 nodejs/22.1.0 claude-code"
                .parse()
                .unwrap(),
        );
        let javascript = sanitize_request_metadata(&javascript, None);
        assert_eq!(javascript.sdk, Merged::Value(SdkName::AwsSdkJsV3));
        assert_eq!(
            javascript.sdk_version,
            Some(SdkVersion::new("3.900.0").unwrap())
        );
        assert_eq!(
            javascript.language_version,
            Some(LanguageVersion::new("22.1.0").unwrap())
        );
        assert_eq!(javascript.agent_client, Some(AgentClient::ClaudeCode));
    }

    #[test]
    fn the_js_sdk_production_user_agent_form_keeps_the_full_version() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            "aws-sdk-js/3.400.0 nodejs/22.1.0".parse().unwrap(),
        );
        let metadata = sanitize_request_metadata(&headers, None);
        assert_eq!(metadata.sdk, Merged::Value(SdkName::AwsSdkJsV3));
        assert_eq!(
            metadata.sdk_version,
            Some(SdkVersion::new("3.400.0").unwrap())
        );

        let mut second_major = http::HeaderMap::new();
        second_major.insert(
            http::header::USER_AGENT,
            "aws-sdk-js/2.1692.0 nodejs/18.20.0".parse().unwrap(),
        );
        let second_major = sanitize_request_metadata(&second_major, None);
        assert_eq!(second_major.sdk, Merged::Unset, "v2 is not a v3 marker");
        assert_eq!(second_major.sdk_version, None);
    }

    #[test]
    fn merged_dimensions_stay_mixed_for_ever_once_conflicting() {
        let mut merged = Merged::Unset;
        merged.observe(None::<Region>);
        assert_eq!(merged, Merged::Unset);

        let west = Region::new("us-west-2").unwrap();
        let east = Region::new("us-east-1").unwrap();
        merged.observe(Some(west.clone()));
        assert_eq!(merged.value(), Some(&west));

        merged.observe(None);
        assert_eq!(merged.value(), Some(&west), "absence never conflicts");

        merged.observe(Some(east));
        assert!(merged.is_mixed());

        merged.observe(Some(west.clone()));
        assert!(
            merged.is_mixed(),
            "a later agreeing observation must never un-mix the session"
        );
        merged.observe(None);
        assert!(merged.is_mixed());
    }
}
