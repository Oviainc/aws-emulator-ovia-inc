// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what `UpdateSchema` does with a `ClientTokenId` the
//! request sent with nothing in it.
//!
//! RULE under test: an idempotency token is the identity of the call that carries it,
//! and a value with no characters in it is no identity — the member's model admits the
//! empty value (`@length(min: 0, max: 36)`, from
//! `smithy_query(query="operation", operation="UpdateSchema")`), so a request carrying
//! one is served exactly as the request that omits the member: it resolves to no
//! earlier call, it mints a version and stores the `Description` it sends, and it is
//! recorded under no token, so no later call can resolve to it either. A token that
//! does carry characters is an identity presented, so a retry under it still answers
//! the version its first attempt minted and writes nothing.
//!
//! Every case drives the real wire path through the harness in
//! `registry_schema_lifecycle.rs`; nothing here touches the state struct directly, so a
//! handler that answered from its input rather than from the store would still have to
//! make the later reads agree.

use serde_json::{json, Value};

use crate::registry_schema_lifecycle::{
    create_registry, listed_versions, schema_uri, versions_uri, Answer, Emulator,
};

/// A schema document marked so each version is distinguishable from every other.
fn document(marker: &str) -> String {
    json!({
        "openapi": "3.0.0",
        "info": {"title": marker, "version": "1.0.0"},
        "paths": {},
    })
    .to_string()
}

/// Create the registry and a schema holding version 1, asserting both succeeded.
async fn seed(emulator: &Emulator, registry: &str, schema: &str) -> String {
    create_registry(emulator, registry).await;
    let content = document("v1");
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": content, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {schema:?}: expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
    content
}

/// `PUT` an update carrying exactly the members this case sends: a `token` of `None`
/// leaves `ClientTokenId` out of the body entirely, where `Some(value)` sends it with
/// that value — including the empty one.
async fn update(
    emulator: &Emulator,
    registry: &str,
    schema: &str,
    token: Option<&str>,
    content: Option<&str>,
    description: Option<&str>,
) -> Answer {
    let mut body = json!({});
    if let Some(token) = token {
        body["ClientTokenId"] = json!(token);
    }
    if let Some(content) = content {
        body["Content"] = json!(content);
    }
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    emulator
        .call("PUT", &schema_uri(registry, schema), Some(body))
        .await
}

/// Read one version back (`None` means "the latest").
async fn describe(
    emulator: &Emulator,
    registry: &str,
    schema: &str,
    version: Option<&str>,
) -> Answer {
    let uri = match version {
        None => schema_uri(registry, schema),
        Some(version) => format!("{}?schemaVersion={version}", schema_uri(registry, schema)),
    };
    emulator.call("GET", &uri, None).await
}

/// One way of sending a `ClientTokenId` that names no call: the member left out, and
/// the member present with nothing in it.
const NAMES_NO_CALL: [(&str, Option<&str>); 2] =
    [("member-omitted", None), ("empty-value", Some(""))];

/// RULE: a `ClientTokenId` that names no call makes every update a call of its own —
/// each one mints the next version and stores the `Description` it carries, however
/// many times the same token member is sent that way.
#[tokio::test]
async fn a_client_token_that_names_no_call_mints_every_time() {
    for (case, token) in NAMES_NO_CALL {
        let emulator = Emulator::new();
        let registry = "unidentified";
        let schema = "orders.placed";
        let first_content = seed(&emulator, registry, schema).await;

        // Rows: three successive updates, differing along what the rule generalises
        // over — content and description together, content alone, and content with a
        // description that replaces the one before it.
        let rounds: [(&str, Option<&str>); 3] = [
            ("content-and-description", Some("second write")),
            ("content-only", None),
            ("content-and-another-description", Some("fourth write")),
        ];

        let mut stored: Vec<(u64, String)> = vec![(1, first_content)];
        let mut description: Option<String> = None;
        for (round, (marker, sent_description)) in rounds.into_iter().enumerate() {
            let case = format!("{case}/{marker}");
            let content = document(marker);
            let expected_version = 2 + round as u64;
            let answered = update(
                &emulator,
                registry,
                schema,
                token,
                Some(&content),
                sent_description,
            )
            .await;
            assert_eq!(
                answered.status, 200,
                "case {case:?}: expected 200 from UpdateSchema, got {} {:?}",
                answered.status, answered.body
            );
            assert_eq!(
                answered.str_at("SchemaVersion"),
                expected_version.to_string(),
                "case {case:?}: expected version {expected_version} to be minted, got {:?}",
                answered.body
            );
            if let Some(sent) = sent_description {
                description = Some(sent.to_string());
            }
            assert_eq!(
                answered.body.get("Description").and_then(Value::as_str),
                description.as_deref(),
                "case {case:?}: expected the stored description {description:?}, got {:?}",
                answered.body
            );
            stored.push((expected_version, content));
        }

        // Every write is in the store: each version reports the bytes it was minted
        // from, and the schema reports the last description sent.
        for (version, content) in &stored {
            let read = describe(&emulator, registry, schema, Some(&version.to_string())).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: version {version} must be readable, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                &read.str_at("Content"),
                content,
                "case {case:?}: version {version} must report the content it was minted from"
            );
        }
        let latest = describe(&emulator, registry, schema, None).await;
        assert_eq!(
            latest.body.get("Description").and_then(Value::as_str),
            description.as_deref(),
            "case {case:?}: expected the schema to report {description:?}, got {:?}",
            latest.body
        );
        let listed = describe(&emulator, registry, schema, None).await;
        assert_eq!(
            listed.str_at("SchemaVersion"),
            stored
                .last()
                .expect("the table minted at least one version")
                .0
                .to_string(),
            "case {case:?}: the latest version is the last one minted, got {:?}",
            listed.body
        );
        let versions = emulator
            .call("GET", &versions_uri(registry, schema), None)
            .await;
        let expected: std::collections::BTreeSet<String> =
            stored.iter().map(|(version, _)| version.to_string()).collect();
        assert_eq!(
            listed_versions(&versions),
            expected,
            "case {case:?}: expected the schema to hold {expected:?}, got {:?}",
            versions.body
        );
    }
}

/// RULE (the same sentence, from the other side): the empty value is served exactly as
/// the omitted member, so the two ways of naming no call do not name each OTHER either
/// — interleaving them on one schema mints once per call.
#[tokio::test]
async fn interleaving_the_two_ways_of_naming_no_call_still_mints_once_per_call() {
    let emulator = Emulator::new();
    let registry = "unidentified";
    let schema = "orders.shipped";
    seed(&emulator, registry, schema).await;

    // Rows: the two ways of naming no call, alternated so each follows the other as
    // well as itself.
    let order = [
        NAMES_NO_CALL[1],
        NAMES_NO_CALL[0],
        NAMES_NO_CALL[1],
        NAMES_NO_CALL[1],
        NAMES_NO_CALL[0],
    ];

    for (round, (case, token)) in order.into_iter().enumerate() {
        let case = format!("round {round}: {case}");
        let marker = format!("round-{round}");
        let content = document(&marker);
        let description = format!("written in round {round}");
        let expected_version = 2 + round as u64;
        let answered = update(
            &emulator,
            registry,
            schema,
            token,
            Some(&content),
            Some(&description),
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.str_at("SchemaVersion"),
            expected_version.to_string(),
            "case {case:?}: expected version {expected_version}, got {:?}",
            answered.body
        );
        assert_eq!(
            answered.str_at("Description"),
            description,
            "case {case:?}: the description this call sent must be the stored one, got {:?}",
            answered.body
        );

        let latest = describe(&emulator, registry, schema, None).await;
        assert_eq!(
            latest.str_at("Content"),
            content,
            "case {case:?}: the schema must read back as this call's content"
        );
        assert_eq!(
            latest.str_at("Description"),
            description,
            "case {case:?}: the schema must read back as this call's description, got {:?}",
            latest.body
        );
    }
}

/// RULE: a `ClientTokenId` that carries characters IS an identity — a retry under it
/// answers the version its first attempt minted and writes nothing, whatever `Content`
/// and `Description` the retry carries; and an update that names no call is never
/// resolved to one of those remembered identities.
#[tokio::test]
async fn a_client_token_that_names_a_call_still_replays_it() {
    // Rows: token values a client sends, spanning what the rule generalises over — the
    // modelled length bounds (one character, and the 36 the member's
    // `@length(min: 0, max: 36)` admits at most), the UUID an SDK generates, a value
    // whose only content is a space, and one of punctuation. None of them is empty, so
    // each names a call.
    let tokens: [(&str, String); 5] = [
        ("one-character", "a".to_string()),
        ("at-the-modelled-maximum", "t".repeat(36)),
        (
            "sdk-generated-uuid",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("a-single-space", " ".to_string()),
        ("punctuation-only", "--".to_string()),
    ];

    let emulator = Emulator::new();
    let registry = "identified";
    let schema = "orders.cancelled";
    seed(&emulator, registry, schema).await;

    let mut next_version = 1_u64;
    for (case, token) in &tokens {
        next_version += 1;
        let minted_content = document(&format!("{case}-first"));
        let minted_description = format!("{case}: the first attempt");
        let minted = update(
            &emulator,
            registry,
            schema,
            Some(token),
            Some(&minted_content),
            Some(&minted_description),
        )
        .await;
        assert_eq!(
            minted.str_at("SchemaVersion"),
            next_version.to_string(),
            "case {case:?}: a token no call has used mints version {next_version}, got {:?}",
            minted.body
        );

        // The retry: the same identity, a different definition and a different
        // description. It answers the first attempt's version and writes neither.
        let retried = update(
            &emulator,
            registry,
            schema,
            Some(token),
            Some(&document(&format!("{case}-retry"))),
            Some(&format!("{case}: the retry")),
        )
        .await;
        assert_eq!(
            retried.status, 200,
            "case {case:?}: expected 200 from the retry, got {} {:?}",
            retried.status, retried.body
        );
        assert_eq!(
            retried.str_at("SchemaVersion"),
            next_version.to_string(),
            "case {case:?}: a retry must answer the version its first attempt minted, got {:?}",
            retried.body
        );
        assert_eq!(
            retried.str_at("Description"),
            minted_description,
            "case {case:?}: a retry must not replace the description, got {:?}",
            retried.body
        );
        let pinned = describe(
            &emulator,
            registry,
            schema,
            Some(&next_version.to_string()),
        )
        .await;
        assert_eq!(
            pinned.str_at("Content"),
            minted_content,
            "case {case:?}: a retry must not rewrite the version it de-duplicated"
        );

        // An update that names NO call is not a retry of any remembered identity: it
        // mints the next version and stores what it sends.
        for (unnamed, unnamed_token) in NAMES_NO_CALL {
            next_version += 1;
            let case = format!("{case}/then-{unnamed}");
            let content = document(&format!("{case}-unnamed"));
            let description = format!("{case}: names no call");
            let answered = update(
                &emulator,
                registry,
                schema,
                unnamed_token,
                Some(&content),
                Some(&description),
            )
            .await;
            assert_eq!(
                answered.str_at("SchemaVersion"),
                next_version.to_string(),
                "case {case:?}: expected version {next_version} to be minted, got {:?}",
                answered.body
            );
            assert_eq!(
                answered.str_at("Description"),
                description,
                "case {case:?}: the description it sent must be stored, got {:?}",
                answered.body
            );
            let latest = describe(&emulator, registry, schema, None).await;
            assert_eq!(
                latest.str_at("Content"),
                content,
                "case {case:?}: the content it sent must be stored"
            );
        }
    }

    // Every call that named no identity minted, and every retry de-duplicated: the
    // schema holds one version per call that named no call, plus one per token, plus
    // the create's.
    let expected: std::collections::BTreeSet<String> =
        (1..=next_version).map(|version| version.to_string()).collect();
    let versions = emulator
        .call("GET", &versions_uri(registry, schema), None)
        .await;
    assert_eq!(
        listed_versions(&versions),
        expected,
        "expected the schema to hold {expected:?}, got {:?}",
        versions.body
    );
}
