// SPDX-License-Identifier: CC0-1.0

//! What `CreateCustomDataIdentifier` accepts as a `regex`, and how a call
//! addressing an identifier the registry does not hold is refused — end to end
//! over the wire.
//!
//! Each case drives the production `MultiServiceRouter` through the harness
//! `macie_session_and_custom_data_identifiers` builds, so what is pinned is the
//! HTTP behaviour and the response BODIES rather than a handler called
//! directly.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! request:
//!
//! * a create whose `regex` is not a pattern the service can run is refused 400
//!   `ValidationException` and stores nothing, so the enumeration is unchanged
//!   across the refused call, while a create whose pattern it can run is
//!   accepted — the table below is the same call with the pattern varied;
//! * the `name` is NOT unique: two creates naming the same name both succeed
//!   and mint distinct ids;
//! * a call addressing an id of no identifier in the region is refused 404
//!   `ResourceNotFoundException` with the service's wording, whatever the id
//!   looks like and whichever of the two id-addressed operations is called.

use smithy_server_runtime::MultiServiceRouter;

use super::macie_session_and_custom_data_identifiers::{
    call_in, emulator, enable_in, error_code, error_message,
};

/// The region every case signs for.
const REGION: &str = "us-west-2";

/// The registry's URI, from `CreateCustomDataIdentifier`'s `@http` binding.
const IDENTIFIERS_URI: &str = "/custom-data-identifiers";

/// How the service refuses a `regex` it will not run. The whole sentence is
/// fixed text, so it is spelled out here rather than built from the request.
const UNSUPPORTED_PATTERN_MESSAGE: &str = "Unsafe regex, keywords, or ignoreWords.";

/// Create an identifier named `name` with `pattern`, and hand back the status
/// and body as sent.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    pattern: &str,
) -> (u16, serde_json::Value) {
    let body = serde_json::json!({ "name": name, "regex": pattern });
    call_in(router, REGION, "POST", IDENTIFIERS_URI, &body.to_string()).await
}

/// Every id the registry currently enumerates, in the order the page reports
/// them.
async fn listed_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, page) = call_in(
        router,
        REGION,
        "POST",
        &format!("{IDENTIFIERS_URI}/list"),
        "{}",
    )
    .await;
    assert_eq!(
        status, 200,
        "ListCustomDataIdentifiers must answer 200, got {status} with {page:?}"
    );
    page["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A create is accepted exactly when its `regex` is a pattern the service can
/// run, and a refused create leaves the registry as it was.
///
/// The rows span the ways a pattern is malformed — an unclosed group, an
/// unclosed character class, a reversed class range, a quantifier with no
/// expression to repeat, a repetition range whose bounds are inverted, a
/// trailing escape, a repeated capture-group name, an unknown character class —
/// against patterns of the same shapes that ARE well formed, so what the table
/// pins is the syntax decision and not the length of the string.
#[tokio::test]
async fn a_create_refuses_every_pattern_the_service_cannot_run() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let rows: Vec<(&str, &str, bool)> = vec![
        ("an unclosed character class", "[A-Z(unclosed", false),
        ("an unclosed group", "(unclosed", false),
        ("a reversed class range", "[z-a]", false),
        ("a quantifier with nothing to repeat", "*abc", false),
        ("a repeated capture-group name", "(?P<n>a)(?P<n>b)", false),
        ("an inverted repetition range", "a{4,2}", false),
        ("a trailing escape", "abc\\", false),
        ("an unknown unicode class", "\\p{NotAScriptName}", false),
        ("a closed character class", "[A-Z]", true),
        ("a closed group", "(closed)", true),
        (
            "distinct capture-group names",
            "(?P<first>a)(?P<second>b)",
            true,
        ),
        ("an account-number shape", "[0-9]{3}-[0-9]{2}-[0-9]{4}", true),
        ("an anchored alternation", "^(foo|bar)+$", true),
        ("an escaped metacharacter", "a\\\\b", true),
        ("a unicode class", "\\p{Greek}+", true),
    ];

    for (case, pattern, accepted) in rows {
        let before = listed_ids(&router).await;
        let (status, reply) = create(&router, &format!("identifier-for-{case}"), pattern).await;
        let after = listed_ids(&router).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a pattern the service can run must be accepted, \
                 got {status} with {reply:?}"
            );
            let id = reply["customDataIdentifierId"].as_str().unwrap_or_default();
            assert!(
                after.iter().any(|listed| listed == id),
                "case {case:?}: the minted identifier must be enumerated, got {after:?}"
            );
            assert_eq!(
                after.len(),
                before.len() + 1,
                "case {case:?}: the accepted create must add exactly one identifier, \
                 got {after:?} after {before:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a pattern the service cannot run must be refused, \
                 got {status} with {reply:?}"
            );
            assert_eq!(
                error_code(&reply),
                "ValidationException",
                "case {case:?}: the refusal must be the modelled ValidationException, \
                 got {reply:?}"
            );
            assert_eq!(
                error_message(&reply),
                UNSUPPORTED_PATTERN_MESSAGE,
                "case {case:?}: expected {UNSUPPORTED_PATTERN_MESSAGE:?}, got {reply:?}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: the refused create must leave the registry unchanged, \
                 got {after:?} after {before:?}"
            );
            assert!(
                reply["customDataIdentifierId"].is_null(),
                "case {case:?}: a refused create mints no id, got {reply:?}"
            );
        }
    }
}

/// The `name` is not unique: the service mints a distinct identifier per
/// accepted create, however many of them name the same name. The rows are the
/// names the repeat is exercised on.
#[tokio::test]
async fn the_same_name_may_be_created_more_than_once() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    for case in ["repeated", "a name with spaces", "RepeatedMixedCase"] {
        let mut minted: Vec<String> = Vec::new();
        for attempt in 0..3 {
            let (status, reply) = create(&router, case, "[0-9]+").await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: a repeated name must be accepted, \
                 got {status} with {reply:?}"
            );
            let id = reply["customDataIdentifierId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !minted.contains(&id),
                "case {case:?} attempt {attempt}: each create mints its own id, \
                 got {id:?} again among {minted:?}"
            );
            minted.push(id);
        }
    }
}

/// A call addressing an id the region's registry does not hold is refused 404
/// `ResourceNotFoundException`, and the message is the service's: fixed text
/// followed by the id the request named. The rows span the id forms a caller
/// sends — a UUID, an id of the Macie grammar, a short value, one carrying
/// padding — across both id-addressed operations.
#[tokio::test]
async fn an_unknown_identifier_is_refused_with_the_services_wording() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    // One identifier really exists, so the refusal below is about the id and
    // not about an empty registry.
    let (status, reply) = create(&router, "known", "[0-9]+").await;
    assert_eq!(
        status, 200,
        "case \"a known identifier\": expected 200, got {status} with {reply:?}"
    );
    let known = reply["customDataIdentifierId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let unknown = [
        "417f5a0c-7101-4ff9-a58f-7c11f6cba020",
        "0123456789abcdefghijkl",
        "nope",
        "00000000000000000000000000",
    ];

    for id in unknown {
        assert_ne!(
            id, known,
            "the table must name ids the registry does not hold"
        );
        for (operation, method) in [
            ("GetCustomDataIdentifier", "GET"),
            ("DeleteCustomDataIdentifier", "DELETE"),
        ] {
            let case = format!("{operation} {id}");
            let (status, reply) = call_in(
                &router,
                REGION,
                method,
                &format!("{IDENTIFIERS_URI}/{id}"),
                "",
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: an unknown id must answer 404, got {status} with {reply:?}"
            );
            assert_eq!(
                error_code(&reply),
                "ResourceNotFoundException",
                "case {case:?}: the refusal must be the modelled ResourceNotFoundException, \
                 got {reply:?}"
            );
            let expected = format!("Custom Data Identifier not found for id: {id}");
            assert_eq!(
                error_message(&reply),
                expected,
                "case {case:?}: expected {expected:?}, got {reply:?}"
            );
        }
    }
}
