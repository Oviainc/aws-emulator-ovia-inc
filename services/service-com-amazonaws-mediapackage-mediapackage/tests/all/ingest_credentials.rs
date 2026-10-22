// SPDX-License-Identifier: CC0-1.0

//! The shape of the values MediaPackage system-generates for a Channel's HLS ingest
//! endpoints, asserted end-to-end over real HTTP through the `MultiServiceRouter`.
//!
//! The rule under test: every value MediaPackage system-generates for an ingest endpoint
//! — `IngestEndpoint.Id`, `IngestEndpoint.Username` and `IngestEndpoint.Password` — is a
//! 32-character lowercase-hexadecimal token, unique per endpoint, minted once at
//! CreateChannel and echoed unchanged by every later projection of the channel
//! (`DescribeChannel`, `ListChannels`).
//!
//! The width comes from recorded MediaPackage `CreateChannel` responses: the Smithy model
//! puts no `length`/`pattern` constraint on the three members
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#IngestEndpoint")`
//! reports only `@sensitive` on the two credentials and the "system generated"
//! documentation), so the recorded shape is the contract. The literal recorded values are
//! examples, never requirements — nothing below pins a value.

use serde_json::{json, Value};

use crate::channel_lifecycle::{call, emulator};

/// Characters in each system-generated ingest value: recorded MediaPackage responses
/// return a 32-character lowercase-hex token for `Id`, `Username` and `Password`.
const GENERATED_HEX_CHARS: usize = 32;

/// The members of `IngestEndpoint` MediaPackage generates itself, all three with the same
/// generated shape.
const GENERATED_MEMBERS: [&str; 3] = ["id", "username", "password"];

/// `true` when `text` is exactly `GENERATED_HEX_CHARS` lowercase-hex characters.
fn is_generated_token(text: &str) -> bool {
    text.chars().count() == GENERATED_HEX_CHARS
        && text.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// The `CreateChannel` bodies under test. The rows differ along what the rule
/// generalises over: the channel id's length (one character up to 64) and form (hyphens,
/// underscores, digits, mixed case), and whether the request carries a description or
/// tags — none of which the generated credentials may depend on.
fn create_bodies() -> Vec<(String, Value)> {
    vec![
        ("c".to_string(), json!({ "id": "c" })),
        (
            "mp-channel".to_string(),
            json!({ "id": "mp-channel", "description": "two endpoints" }),
        ),
        (
            "Mixed_Case_99".to_string(),
            json!({ "id": "Mixed_Case_99", "tags": { "team": "video" } }),
        ),
        (
            "sim2real-probe-mediapackage-mp-channel-818202c7".to_string(),
            json!({ "id": "sim2real-probe-mediapackage-mp-channel-818202c7" }),
        ),
        ("x".repeat(64), json!({ "id": "x".repeat(64) })),
    ]
}

/// The `hlsIngest.ingestEndpoints` array of a channel projection.
fn endpoints_of(channel: &Value) -> Vec<Value> {
    channel
        .pointer("/hlsIngest/ingestEndpoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn generated_ingest_values_are_32_char_lowercase_hex_on_every_channel_projection() {
    let router = emulator();
    // Every generated token seen anywhere, to prove uniqueness across endpoints and
    // across channels.
    let mut all_tokens: Vec<(String, String)> = Vec::new();

    for (id, body) in create_bodies() {
        let case = format!("CreateChannel {id}");
        let (status, created) = call(&router, "POST", "/channels", Some(&body)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");

        let endpoints = endpoints_of(&created);
        assert_eq!(
            endpoints.len(),
            2,
            "case {case:?}: expected 2 ingest endpoints, got {created:?}"
        );
        for (index, endpoint) in endpoints.iter().enumerate() {
            for member in GENERATED_MEMBERS {
                let actual = endpoint.get(member).and_then(Value::as_str).unwrap_or("");
                assert!(
                    is_generated_token(actual),
                    "case {case:?}: ingest endpoint {index} {member} must be \
                     {GENERATED_HEX_CHARS} lowercase-hex characters, got {actual:?} \
                     ({} characters)",
                    actual.chars().count()
                );
                all_tokens.push((format!("{case} endpoint {index} {member}"), actual.to_string()));
            }
        }

        // DescribeChannel echoes the credentials stored at create time, unchanged.
        let (status, described) =
            call(&router, "GET", &format!("/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel expected 200, got {status}"
        );
        assert_eq!(
            described.get("hlsIngest"),
            created.get("hlsIngest"),
            "case {case:?}: DescribeChannel must echo the stored ingest endpoints, \
             got {described:?}"
        );
    }

    // The list projection carries the same credentials for every channel.
    let (status, listed) = call(&router, "GET", "/channels", None).await;
    assert_eq!(status, 200, "ListChannels expected 200, got {status}");
    let channels = listed
        .get("channels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        channels.len(),
        create_bodies().len(),
        "ListChannels must list every created channel, got {listed:?}"
    );
    for channel in &channels {
        let id = channel.get("id").and_then(Value::as_str).unwrap_or_default();
        let (_, described) = call(&router, "GET", &format!("/channels/{id}"), None).await;
        assert_eq!(
            channel.get("hlsIngest"),
            described.get("hlsIngest"),
            "case {id:?}: the listed ingest endpoints must be the stored ones, \
             got {channel:?}"
        );
        for (index, endpoint) in endpoints_of(channel).iter().enumerate() {
            for member in GENERATED_MEMBERS {
                let actual = endpoint.get(member).and_then(Value::as_str).unwrap_or("");
                assert!(
                    is_generated_token(actual),
                    "case {id:?}: listed endpoint {index} {member} must be \
                     {GENERATED_HEX_CHARS} lowercase-hex characters, got {actual:?} \
                     ({} characters)",
                    actual.chars().count()
                );
            }
        }
    }

    // Each generated value is its own: no credential is shared between the two endpoints
    // of a channel, between channels, or with an endpoint id.
    for (left_index, (left_case, left)) in all_tokens.iter().enumerate() {
        for (right_case, right) in all_tokens.iter().skip(left_index + 1) {
            assert_ne!(
                left, right,
                "{left_case} and {right_case} must be distinct generated values, \
                 both are {left:?}"
            );
        }
    }
}

#[tokio::test]
async fn repeated_reads_do_not_remint_the_generated_ingest_values() {
    let router = emulator();

    // Rows differ along the number of channels in the store when the read happens, so a
    // generator that keyed off store size or call count would show up.
    for (id, body) in create_bodies() {
        let case = format!("stability {id}");
        let (status, created) = call(&router, "POST", "/channels", Some(&body)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}");
        let expected = created.get("hlsIngest").cloned().unwrap_or(Value::Null);

        for read in 0..3 {
            let (_, described) = call(&router, "GET", &format!("/channels/{id}"), None).await;
            assert_eq!(
                described.get("hlsIngest"),
                Some(&expected),
                "case {case:?}: read {read} must return the ingest endpoints minted at \
                 create time, got {described:?}"
            );
        }
    }
}
