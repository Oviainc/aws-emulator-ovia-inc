// SPDX-License-Identifier: CC0-1.0

//! The `CreateGuardrail` ENVELOPE, over the same HTTP harness the other guardrail suites
//! drive (`crate::support`): the status, the content type and the four members the model
//! requires, plus the instant the create minted being the one every later projection of
//! that guardrail reports.
//!
//! This is the envelope the issue this suite was written for saw replaced by a `500
//! AmbiguousService` in `application/x-amz-json-1.1`, so the rules are stated from the
//! model rather than from that recording:
//!
//! 1. `CreateGuardrail` is bound to `@http(code: 202, method: POST, uri: /guardrails)`
//!    and this service's one protocol is `restJson1`, so a create answers 202 with
//!    `content-type: application/json` — never a 5xx and never another media type.
//! 2. `CreateGuardrailResponse` makes `guardrailId`, `guardrailArn`, `version` and
//!    `createdAt` all `@required`, with `version` constrained to `^DRAFT$` and
//!    `guardrailArn` to `^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+$`,
//!    so every answer carries all four and the ARN names the minted id in the region the
//!    request was signed for.
//! 3. `createdAt` is the guardrail's stored creation instant, so `GetGuardrail` and
//!    `ListGuardrails` report the very same value, byte for byte, as the create did.
//!
//! The WIRE FORM of that instant (an RFC 3339 string, never the protocol's default epoch
//! number) and the member schemas that decide it are `guardrail_wire_shape.rs`'s subject;
//! this suite reads it through `support::Reply::instant`, which refuses anything else, so
//! the two suites cannot disagree about it.

use serde_json::{json, Value};

use crate::support::{emulator, ACCOUNT, REGION};

/// A `CreateGuardrail` body: the two `@required` messaging strings plus whatever `extra`
/// adds.
fn create_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "name": name,
        "blockedInputMessaging": format!("{name} blocked the prompt"),
        "blockedOutputsMessaging": format!("{name} blocked the response"),
    });
    let members = body.as_object_mut().expect("an object");
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        members.insert(member, value);
    }
    body
}

/// Rules 1 and 2, over creates that differ in what the request carries: only the required
/// members, a described guardrail with a word policy, and one carrying a topic policy, a
/// content policy and an idempotency token.
#[tokio::test]
async fn create_answers_the_modelled_202_guardrail_envelope() {
    let emulator = emulator();
    let cases: Vec<(&str, Value)> = vec![
        ("required members only", json!({})),
        (
            "described, word policy",
            json!({
                "description": "blocks one word",
                "wordPolicyConfig": { "wordsConfig": [ { "text": "nope" } ] },
            }),
        ),
        (
            "topic and content policies, idempotency token",
            json!({
                "clientRequestToken": "token-for-the-third-case",
                "topicPolicyConfig": {
                    "topicsConfig": [ {
                        "name": "Topic",
                        "definition": "a denied topic",
                        "type": "DENY",
                    } ],
                },
                "contentPolicyConfig": {
                    "filtersConfig": [ {
                        "type": "HATE",
                        "inputStrength": "HIGH",
                        "outputStrength": "HIGH",
                    } ],
                },
            }),
        ),
    ];

    for (index, (case, extra)) in cases.into_iter().enumerate() {
        let reply = emulator
            .create(create_body(&format!("envelope-{index}"), extra))
            .await;

        assert_eq!(
            reply.status, 202,
            "case {case:?}: CreateGuardrail is bound to @http(code: 202), got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.content_type.as_deref(),
            Some("application/json"),
            "case {case:?}: a restJson1 response carries application/json, got {:?}",
            reply.content_type
        );
        assert!(
            reply.error.is_none(),
            "case {case:?}: a create that succeeded names no error, got {:?}",
            reply.error
        );
        for member in ["guardrailId", "guardrailArn", "version", "createdAt"] {
            assert!(
                !reply.get(member).is_null(),
                "case {case:?}: {member} is @required on CreateGuardrailResponse, got {:?}",
                reply.body
            );
        }
        assert_eq!(
            reply.text("version"),
            "DRAFT",
            "case {case:?}: GuardrailDraftVersion is ^DRAFT$, got {:?}",
            reply.get("version")
        );
        let id = reply.text("guardrailId");
        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {case:?}: GuardrailId is ^[a-z0-9]+$ and never empty, got {id:?}"
        );
        assert_eq!(
            reply.text("guardrailArn"),
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/{id}"),
            "case {case:?}: the ARN must name the minted id in the signing region"
        );
        // Read as an instant, which refuses anything but the modelled date-time string.
        assert!(
            reply.instant("createdAt") > 0.0,
            "case {case:?}: createdAt must name a real instant, got {:?}",
            reply.get("createdAt")
        );
    }
}

/// Rule 3: the creation instant is the guardrail's own, so every projection of it reports
/// the same value — and an `updatedAt` never precedes it.
///
/// Rows: the DRAFT read back by id, the same guardrail inside an unfiltered listing, and
/// inside the listing of that one guardrail's versions, after an update has moved
/// `updatedAt` on.
#[tokio::test]
async fn every_projection_reports_the_instant_the_create_minted() {
    let emulator = emulator();
    let created = emulator.create(create_body("projected", json!({}))).await;
    let id = created.text("guardrailId");
    let minted = created.text("createdAt");

    let updated = emulator
        .update(&id, create_body("projected", json!({ "description": "revised" })))
        .await;
    assert_eq!(
        updated.status, 202,
        "UpdateGuardrail must succeed for this suite to say anything, got {} with {:?}",
        updated.status, updated.body
    );

    let got = emulator.get(&id, None).await;
    let listed = emulator.list("").await;
    let versions = emulator.list(&format!("guardrailIdentifier={id}")).await;
    let summary_of = |reply: &crate::support::Reply| -> Value {
        reply
            .get("guardrails")
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row.get("id").and_then(Value::as_str) == Some(id.as_str()))
                    .cloned()
            })
            .unwrap_or_else(|| panic!("the listing must carry guardrail {id}: {:?}", reply.body))
    };

    let rows: Vec<(&str, Value)> = vec![
        ("GetGuardrail, DRAFT", got.body.clone()),
        ("ListGuardrails, unfiltered", summary_of(&listed)),
        ("ListGuardrails of one guardrail", summary_of(&versions)),
    ];
    for (case, projection) in rows {
        assert_eq!(
            projection.get("createdAt").and_then(Value::as_str),
            Some(minted.as_str()),
            "case {case:?}: createdAt must be the instant CreateGuardrail minted, got \
             {projection:?}"
        );
        let updated_at = projection
            .get("updatedAt")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: updatedAt is @required, got {projection:?}"));
        assert!(
            crate::support::date_time_seconds(case, updated_at)
                >= crate::support::date_time_seconds(case, &minted),
            "case {case:?}: updatedAt cannot precede createdAt, got {updated_at} against \
             {minted}"
        );
    }
}
