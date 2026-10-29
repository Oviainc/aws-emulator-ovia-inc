// SPDX-License-Identifier: CC0-1.0

//! The refusal every operation of this service that resolves a `guardrailIdentifier`
//! answers when that identifier resolves to no guardrail, driven end to end over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! Amazon Bedrock reports a guardrail that is not there with one FIXED sentence, and the
//! sentence carries no part of the request: no guardrail id, no version, no account
//! reference. Every operation that resolves such an identifier answers alike, with the
//! modelled `ResourceNotFoundException` at `@httpError(404)`
//! (`smithy_query(query="operation",operation="GetGuardrail,UpdateGuardrail,DeleteGuardrail,CreateGuardrailVersion,ListGuardrails")`),
//! so it is the refusal of the shared guardrail LOOKUP rather than of one read. The one
//! operation whose own BODY names the guardrail — `PutEnforcedGuardrailConfiguration`,
//! through `guardrailInferenceConfig` — answers the service's OTHER fixed sentence for this
//! miss, the one that says the guardrail came from the request; it is pinned here beside the
//! lookup's so the two stay distinct.
//!
//! # What the table below generalises over
//!
//! The rows differ along what the rule spans: which operation resolves the identifier (a
//! read, an update, a delete, a version cut, a listing and the enforced-configuration
//! put), how the identifier fails to resolve (a `guardrailId` nothing created, the ARN
//! spelling of one, a guardrail that was created and then DELETED, and a live guardrail at
//! a version it does not carry), and — for the rows that take one — the version named
//! beside it.

use serde_json::{json, Value};

use crate::support::{emulator, Emulator, Reply, ACCOUNT, REGION};

/// The sentence Amazon Bedrock answers for a guardrail that is not there, written out here
/// rather than read off the crate so the recorded text is pinned by this suite
/// independently of the constant that produces it.
const GUARDRAIL_NOT_FOUND: &str = "This guardrail does not exist.";

/// The sentence Amazon Bedrock answers when the guardrail the REQUEST BODY provided is not
/// there, written out here for the reason the one above is.
const REQUEST_GUARDRAIL_NOT_FOUND: &str = "The guardrail provided in the request does not exist.";

/// A `guardrailId` nothing created: `GuardrailId` is `^[a-z0-9]+$`.
const UNKNOWN_ID: &str = "zzzzzzzzzzzz";

/// The `GuardrailArn` of `id`:
/// `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+$`.
fn guardrail_arn(id: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/{id}")
}

/// The update body with only the members the model requires.
fn update_body() -> Value {
    json!({
        "name": "renamed",
        "blockedInputMessaging": "blocked in",
        "blockedOutputsMessaging": "blocked out",
    })
}

/// `PUT /enforcedGuardrailsConfiguration` for the guardrail `identifier` at `version`.
async fn put_enforced(emu: &Emulator, identifier: &str, version: &str) -> Reply {
    emu.call(
        "PUT",
        "/enforcedGuardrailsConfiguration",
        Some(json!({
            "guardrailInferenceConfig": {
                "guardrailIdentifier": identifier,
                "guardrailVersion": version,
            },
        })),
    )
    .await
}

/// Every answer this suite checks: 404, the modelled code, and the fixed sentence.
fn assert_not_found(case: &str, reply: &Reply) {
    assert_sentence(case, reply, GUARDRAIL_NOT_FOUND);
}

/// The same, for a refusal whose sentence is `expected` rather than the lookup's.
fn assert_sentence(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case {case:?}: expected 404 ResourceNotFoundException, got {} {:?}",
        reply.status,
        reply.body
    );
    let message = reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
}

#[tokio::test]
async fn every_guardrail_lookup_reports_a_missing_guardrail_in_one_sentence() {
    let emu = emulator();

    // A guardrail that WAS created and then deleted, so a row that names it is refused for
    // being gone rather than for an empty store; and a live one, so a row that names a
    // version it does not carry is refused for the version.
    let deleted = emu.create_named("deleted-guardrail").await;
    let removed = emu.delete(&deleted, None).await;
    assert_eq!(
        removed.status, 202,
        "case \"the delete that sets this suite up\": DeleteGuardrail must answer 202, got {} \
         {:?}",
        removed.status, removed.body
    );
    let live = emu.create_named("live-guardrail").await;

    // (case, the identifier, the version named beside it when the row carries one)
    let identifiers: Vec<(String, String, Option<&str>)> = vec![
        (
            "a guardrailId nothing created".to_string(),
            UNKNOWN_ID.to_string(),
            None,
        ),
        (
            "the ARN spelling of an id nothing created".to_string(),
            guardrail_arn(UNKNOWN_ID),
            None,
        ),
        (
            "a deleted guardrail's id".to_string(),
            deleted.clone(),
            None,
        ),
        (
            "the ARN spelling of a deleted guardrail".to_string(),
            guardrail_arn(&deleted),
            None,
        ),
        (
            "a live guardrail at a version it does not carry".to_string(),
            live.clone(),
            Some("7"),
        ),
    ];

    for (case, identifier, version) in identifiers {
        assert_not_found(
            &format!("GetGuardrail / {case}"),
            &emu.get(&identifier, version).await,
        );
        assert_not_found(
            &format!("DeleteGuardrail / {case}"),
            &emu.delete(&identifier, version).await,
        );
        assert_sentence(
            &format!("PutEnforcedGuardrailConfiguration / {case}"),
            &put_enforced(&emu, &identifier, version.unwrap_or("1")).await,
            REQUEST_GUARDRAIL_NOT_FOUND,
        );

        // The three operations that resolve the identifier WITHOUT a version member: a row
        // that only fails on the version is live for them, so it is not driven through
        // them.
        if version.is_some() {
            continue;
        }
        assert_not_found(
            &format!("UpdateGuardrail / {case}"),
            &emu.update(&identifier, update_body()).await,
        );
        assert_not_found(
            &format!("CreateGuardrailVersion / {case}"),
            &emu.create_version(&identifier, json!({})).await,
        );
        assert_not_found(
            &format!("ListGuardrails / {case}"),
            &emu.list(&format!("guardrailIdentifier={identifier}")).await,
        );
    }

    // The live guardrail is untouched by every refusal above: a refused call changes
    // nothing a later caller sees.
    let read = emu.get(&live, None).await;
    assert_eq!(
        read.status, 200,
        "case \"the live guardrail after the refusals\": expected 200, got {} {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn the_missing_guardrail_sentence_carries_no_part_of_the_request() {
    // The sentence is FIXED: an identifier a caller submitted is not reflected back into
    // it, and it reads as a sentence rather than as this emulator's own phrasing.
    let emu = emulator();
    let created = emu.create_named("short-lived-guardrail").await;
    emu.delete(&created, None).await;

    for (case, identifier) in [
        ("an id nothing created", UNKNOWN_ID.to_string()),
        ("a deleted guardrail's id", created.clone()),
        ("the ARN of a deleted guardrail", guardrail_arn(&created)),
    ] {
        let reply = emu.get(&identifier, None).await;
        let message = reply
            .body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.contains(&identifier),
            "case {case:?}: the sentence must not carry the submitted identifier \
             {identifier:?}, got {message:?}"
        );
        assert!(
            message.ends_with('.') && message.starts_with(|first: char| first.is_uppercase()),
            "case {case:?}: every AWS guardrail message is a sentence, got {message:?}"
        );
    }
}
