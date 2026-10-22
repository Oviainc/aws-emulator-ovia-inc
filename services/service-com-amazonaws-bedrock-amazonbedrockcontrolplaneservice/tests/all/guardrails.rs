// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the guardrail lifecycle, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots: `CreateGuardrail`, `GetGuardrail`,
//! `ListGuardrails`, `UpdateGuardrail`, `CreateGuardrailVersion`, `DeleteGuardrail`.
//!
//! Every table row is an independent input along the axis its rule generalises over
//! (which policies a guardrail carries, which form a KMS key id takes, how large a page
//! is, which constraint a request breaks), never a rename of one recorded request.

use serde_json::{json, Value};

use crate::support::{emulator, ACCOUNT, REGION};

/// One `CreateGuardrail` request and the members `GetGuardrail` must then report. An
/// expected `null` means the response must OMIT that member.
struct RoundTripCase {
    case: &'static str,
    request: Value,
    expected: Value,
}

/// A guardrail reads back exactly as it was stored, over inputs that differ in which
/// policies they carry, whether they carry a description, and which of the three forms
/// `kmsKeyId` admits they use.
#[tokio::test]
async fn create_then_get_reports_the_stored_guardrail() {
    let kms_key_id = "12345678-1234-1234-1234-123456789012";
    let kms_key_arn = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{kms_key_id}");
    // Exactly 50 characters, the upper bound `GuardrailName` (len 1..50) admits, using
    // both separators its pattern `^[0-9a-zA-Z-_]+$` allows.
    let longest_name = format!("a-_{}", "x".repeat(47));

    let cases = vec![
        RoundTripCase {
            case: "word policy only, described, no KMS key",
            request: json!({
                "name": "word-filter",
                "description": "blocks one word",
                "blockedInputMessaging": "input blocked",
                "blockedOutputsMessaging": "output blocked",
                "wordPolicyConfig": { "wordsConfig": [ { "text": "nope" } ] },
            }),
            expected: json!({
                "name": "word-filter",
                "description": "blocks one word",
                "version": "DRAFT",
                "blockedInputMessaging": "input blocked",
                "blockedOutputsMessaging": "output blocked",
                // `managedWordLists` admits zero entries, so it is reported as the
                // empty list rather than omitted.
                "wordPolicy": {
                    "words": [ { "text": "nope" } ],
                    "managedWordLists": [],
                },
                "topicPolicy": null,
                "contentPolicy": null,
                "kmsKeyArn": null,
            }),
        },
        RoundTripCase {
            case: "topic and content policies with tiers, bare KMS key id",
            request: json!({
                "name": "topics_and_content",
                "blockedInputMessaging": "no",
                "blockedOutputsMessaging": "also no",
                "kmsKeyId": kms_key_id,
                "topicPolicyConfig": {
                    "topicsConfig": [ {
                        "name": "Politics",
                        "definition": "Partisan political advocacy",
                        "type": "DENY",
                        "examples": [ "who should i vote for" ],
                    } ],
                    "tierConfig": { "tierName": "STANDARD" },
                },
                "contentPolicyConfig": {
                    "filtersConfig": [ {
                        "type": "HATE",
                        "inputStrength": "HIGH",
                        "outputStrength": "MEDIUM",
                    } ],
                },
            }),
            expected: json!({
                "name": "topics_and_content",
                "description": null,
                "version": "DRAFT",
                "topicPolicy": {
                    "topics": [ {
                        "name": "Politics",
                        "definition": "Partisan political advocacy",
                        "type": "DENY",
                        "examples": [ "who should i vote for" ],
                    } ],
                    "tier": { "tierName": "STANDARD" },
                },
                "contentPolicy": {
                    "filters": [ {
                        "type": "HATE",
                        "inputStrength": "HIGH",
                        "outputStrength": "MEDIUM",
                    } ],
                    // The request configured no content-filter tier, so the guardrail
                    // is on the default one.
                    "tier": { "tierName": "CLASSIC" },
                },
                "wordPolicy": null,
                "kmsKeyArn": kms_key_arn.clone(),
            }),
        },
        RoundTripCase {
            case: "sensitive information policy, KMS key ARN passed through",
            request: json!({
                "name": "pii",
                "blockedInputMessaging": "i cannot answer that",
                "blockedOutputsMessaging": "i cannot answer that",
                "kmsKeyId": kms_key_arn.clone(),
                "sensitiveInformationPolicyConfig": {
                    "piiEntitiesConfig": [ { "type": "EMAIL", "action": "ANONYMIZE" } ],
                    "regexesConfig": [ {
                        "name": "account-number",
                        "description": "nine digits",
                        "pattern": "[0-9]{9}",
                        "action": "BLOCK",
                    } ],
                },
            }),
            expected: json!({
                "name": "pii",
                "sensitiveInformationPolicy": {
                    "piiEntities": [ { "type": "EMAIL", "action": "ANONYMIZE" } ],
                    "regexes": [ {
                        "name": "account-number",
                        "description": "nine digits",
                        "pattern": "[0-9]{9}",
                        "action": "BLOCK",
                    } ],
                },
                "kmsKeyArn": kms_key_arn.clone(),
            }),
        },
        RoundTripCase {
            case: "contextual grounding policy, KMS alias reports no key ARN",
            request: json!({
                "name": "grounding",
                "blockedInputMessaging": "ungrounded",
                "blockedOutputsMessaging": "ungrounded",
                "kmsKeyId": "alias/my-guardrail-key",
                "contextualGroundingPolicyConfig": {
                    "filtersConfig": [ { "type": "GROUNDING", "threshold": 0.75 } ],
                },
            }),
            expected: json!({
                "name": "grounding",
                "contextualGroundingPolicy": {
                    "filters": [ { "type": "GROUNDING", "threshold": 0.75 } ],
                    "tier": { "tierName": "CLASSIC" },
                },
                // An alias resolves to a key id only inside KMS, so no key ARN is
                // reported rather than one being invented.
                "kmsKeyArn": null,
            }),
        },
        RoundTripCase {
            case: "cross-region profile identifier",
            request: json!({
                "name": "cross-region",
                "blockedInputMessaging": "no",
                "blockedOutputsMessaging": "no",
                "crossRegionConfig": { "guardrailProfileIdentifier": "us.guardrail.v1" },
            }),
            expected: json!({
                "name": "cross-region",
                "crossRegionDetails": {
                    "guardrailProfileId": "us.guardrail.v1",
                    "guardrailProfileArn":
                        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail-profile/us.guardrail.v1"),
                },
            }),
        },
        RoundTripCase {
            case: "no policies, longest admissible name",
            request: json!({
                "name": longest_name.as_str(),
                "blockedInputMessaging": "x",
                "blockedOutputsMessaging": "y",
            }),
            expected: json!({
                "name": longest_name.as_str(),
                "version": "DRAFT",
                "blockedInputMessaging": "x",
                "blockedOutputsMessaging": "y",
                "description": null,
                "topicPolicy": null,
                "contentPolicy": null,
                "wordPolicy": null,
                "sensitiveInformationPolicy": null,
                "contextualGroundingPolicy": null,
                "crossRegionDetails": null,
                "kmsKeyArn": null,
            }),
        },
    ];

    let emulator = emulator();
    for case in cases {
        let created = emulator.create(case.request.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {}: CreateGuardrail must answer 202, got {} {:?}",
            case.case, created.status, created.body
        );
        let id = created.text("guardrailId");
        let arn = created.text("guardrailArn");
        assert_eq!(
            created.text("version"),
            "DRAFT",
            "case {}: a created guardrail is always at DRAFT, got {:?}",
            case.case,
            created.get("version")
        );
        assert_eq!(
            arn,
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/{id}"),
            "case {}: the ARN must name the minted id in the signing region, got {arn:?}",
            case.case
        );

        // The same record must come back whether it is addressed by id or by ARN.
        for identifier in [id.as_str(), arn.as_str()] {
            let got = emulator.get(identifier, None).await;
            assert_eq!(
                got.status, 200,
                "case {}: GetGuardrail({identifier}) must answer 200, got {} {:?}",
                case.case, got.status, got.body
            );
            assert_eq!(
                got.text("guardrailId"),
                id,
                "case {}: GetGuardrail({identifier}) must report the stored id, got {:?}",
                case.case,
                got.get("guardrailId")
            );
            assert_eq!(
                got.text("guardrailArn"),
                arn,
                "case {}: GetGuardrail({identifier}) must report the stored ARN, got {:?}",
                case.case,
                got.get("guardrailArn")
            );
            for (member, expected) in case
                .expected
                .as_object()
                .expect("every expectation is an object")
            {
                let actual = got.get(member);
                assert_eq!(
                    &actual, expected,
                    "case {} via {identifier}: member {member}: expected {expected}, \
                     got {actual}",
                    case.case
                );
            }
            let created_at = got.instant("createdAt");
            assert!(
                created_at > 0.0 && (created_at - created.instant("createdAt")).abs() < f64::EPSILON,
                "case {}: GetGuardrail must report the creation instant CreateGuardrail \
                 minted, got {created_at} against {}",
                case.case,
                created.instant("createdAt")
            );
            assert!(
                got.instant("updatedAt") >= created_at,
                "case {}: updatedAt cannot precede createdAt, got {} against {created_at}",
                case.case,
                got.instant("updatedAt")
            );
        }
    }
}

/// Each create mints its own id: `^[a-z0-9]+$`, never an echo of the request and never
/// the same id twice.
#[tokio::test]
async fn every_created_guardrail_gets_its_own_minted_id() {
    let emulator = emulator();
    let names = ["alpha", "beta", "gamma", "alpha-2", "ALPHA"];
    let mut seen: Vec<String> = Vec::new();

    for name in names {
        let id = emulator.create_named(name).await;
        assert!(
            !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
            "case {name}: a guardrail id must match ^[a-z0-9]+$, got {id:?}"
        );
        assert!(
            id.len() <= 64,
            "case {name}: a guardrail id is at most 64 characters, got {} in {id:?}",
            id.len()
        );
        assert!(
            !id.contains(name.to_lowercase().as_str()),
            "case {name}: the id must be minted, not derived from the request, got {id:?}"
        );
        assert!(
            !seen.contains(&id),
            "case {name}: ids must be unique, but {id:?} was already minted ({seen:?})"
        );
        seen.push(id);
    }
}

/// A replayed `clientRequestToken` answers with the guardrail the first request minted;
/// a different token on an otherwise identical request is a different guardrail.
#[tokio::test]
async fn create_is_idempotent_on_the_client_request_token() {
    let emulator = emulator();
    let request = |name: &str, token: &str| {
        json!({
            "name": name,
            "blockedInputMessaging": "blocked in",
            "blockedOutputsMessaging": "blocked out",
            "clientRequestToken": token,
        })
    };

    let first = emulator.create(request("my-guard", "token-one")).await;
    let replay = emulator.create(request("my-guard", "token-one")).await;
    assert_eq!(
        replay.status, 202,
        "case replay: a replayed token is not an error, got {} {:?}",
        replay.status, replay.body
    );
    assert_eq!(
        replay.text("guardrailId"),
        first.text("guardrailId"),
        "case replay: the same token must answer with the first id, got {:?}",
        replay.get("guardrailId")
    );
    assert_eq!(
        replay.get("createdAt"),
        first.get("createdAt"),
        "case replay: the replay reports the FIRST creation instant, got {:?}",
        replay.get("createdAt")
    );

    let listed = emulator.list("").await;
    assert_eq!(
        listed.get("guardrails").as_array().map(Vec::len),
        Some(1),
        "case replay: a replay must not create a second guardrail, got {:?}",
        listed.body
    );

    // A fresh token with the SAME name is a second creation attempt, and the name is
    // what refuses it.
    let other_token = emulator.create(request("my-guard", "token-two")).await;
    assert_eq!(
        (other_token.status, other_token.error.as_deref()),
        (400, Some("ConflictException")),
        "case fresh token, taken name: expected 400 ConflictException, got {} {:?}",
        other_token.status,
        other_token.body
    );
}

/// A name identifies a guardrail within the account: taking one that is held is a
/// conflict, and a name freed by a delete can be taken again.
#[tokio::test]
async fn a_guardrail_name_is_unique_within_the_account() {
    let emulator = emulator();
    let id = emulator.create_named("only-one").await;

    let duplicate = emulator
        .create(json!({
            "name": "only-one",
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        }))
        .await;
    assert_eq!(
        (duplicate.status, duplicate.error.as_deref()),
        (400, Some("ConflictException")),
        "case duplicate name: expected 400 ConflictException, got {} {:?}",
        duplicate.status,
        duplicate.body
    );

    assert_eq!(
        emulator.delete(&id, None).await.status,
        202,
        "case delete: DeleteGuardrail must answer 202"
    );
    let reused = emulator
        .create(json!({
            "name": "only-one",
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        }))
        .await;
    assert_eq!(
        reused.status, 202,
        "case name freed by delete: expected 202, got {} {:?}",
        reused.status, reused.body
    );
    assert_ne!(
        reused.text("guardrailId"),
        id,
        "case name freed by delete: the new guardrail must be a new resource"
    );
}

/// A guardrail is READY on the read that follows the call that created, updated or
/// versioned it, and stays READY however often it is read.
#[tokio::test]
async fn a_guardrail_is_ready_on_the_read_that_follows_every_call() {
    let emulator = emulator();
    let id = emulator.create_named("transitions").await;

    // Straight after the create, with no wait: there is no window in which a guardrail
    // reports a non-terminal status, because no call here returns one.
    for read in 0..3 {
        let got = emulator.get(&id, None).await;
        assert_eq!(
            got.text("status"),
            "READY",
            "case read {read} after create: expected READY, got {:?}",
            got.get("status")
        );
    }

    let updated = emulator
        .update(
            &id,
            json!({
                "name": "transitions",
                "blockedInputMessaging": "in",
                "blockedOutputsMessaging": "out",
            }),
        )
        .await;
    assert_eq!(
        updated.status, 202,
        "case update: expected 202, got {} {:?}",
        updated.status, updated.body
    );
    let after_update = emulator.get(&id, None).await;
    assert_eq!(
        after_update.text("status"),
        "READY",
        "case straight after update: expected READY, got {:?}",
        after_update.get("status")
    );

    let versioned = emulator.create_version(&id, json!({})).await;
    assert_eq!(
        versioned.status, 202,
        "case version: expected 202, got {} {:?}",
        versioned.status, versioned.body
    );
    for version in [None, Some("1")] {
        let settled = emulator.get(&id, version).await;
        assert_eq!(
            settled.text("status"),
            "READY",
            "case {version:?} straight after CreateGuardrailVersion: expected READY, got {:?}",
            settled.get("status")
        );
    }

    // The listing reports the same status as the read, for the DRAFT and for the
    // numbered version.
    let listed = emulator.list(&format!("guardrailIdentifier={id}")).await;
    let rows = listed
        .get("guardrails")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        rows.len(),
        2,
        "case listing: the DRAFT and version 1, got {:?}",
        listed.body
    );
    for row in rows {
        assert_eq!(
            row.get("status").and_then(Value::as_str),
            Some("READY"),
            "case listed {:?}: expected READY, got {row}",
            row.get("version")
        );
    }
}

/// An update rewrites the DRAFT in place — including CLEARING a policy the request
/// omits — and a numbered version keeps the configuration it was cut from.
#[tokio::test]
async fn versions_are_snapshots_the_draft_cannot_change() {
    let emulator = emulator();
    let created = emulator
        .create(json!({
            "name": "versioned",
            "description": "the first configuration",
            "blockedInputMessaging": "first in",
            "blockedOutputsMessaging": "first out",
            "wordPolicyConfig": { "wordsConfig": [ { "text": "first" } ] },
            "topicPolicyConfig": {
                "topicsConfig": [ {
                    "name": "Topic",
                    "definition": "the first topic",
                    "type": "DENY",
                } ],
            },
        }))
        .await;
    let id = created.text("guardrailId");
    let draft_before = emulator.get(&id, None).await;

    let cut = emulator
        .create_version(&id, json!({ "description": "snapshot one" }))
        .await;
    assert_eq!(
        cut.text("version"),
        "1",
        "case first version: numbering starts at 1, got {:?}",
        cut.get("version")
    );
    assert_eq!(
        cut.text("guardrailId"),
        id,
        "case first version: the version belongs to the same guardrail"
    );

    // The update replaces the whole policy set: the word policy changes and the topic
    // policy, which this request omits, is cleared.
    let updated = emulator
        .update(
            &id,
            json!({
                "name": "versioned",
                "description": "the second configuration",
                "blockedInputMessaging": "second in",
                "blockedOutputsMessaging": "second out",
                "wordPolicyConfig": { "wordsConfig": [ { "text": "second" } ] },
            }),
        )
        .await;
    assert_eq!(
        updated.status, 202,
        "case update: expected 202, got {} {:?}",
        updated.status, updated.body
    );
    assert_eq!(
        updated.text("version"),
        "DRAFT",
        "case update: an update always answers for DRAFT, got {:?}",
        updated.get("version")
    );

    let draft_after = emulator.get(&id, None).await;
    let expected_draft = json!({
        "description": "the second configuration",
        "blockedInputMessaging": "second in",
        "blockedOutputsMessaging": "second out",
        "wordPolicy": {
            "words": [ { "text": "second" } ],
            "managedWordLists": [],
        },
        "topicPolicy": null,
        "version": "DRAFT",
    });
    for (member, expected) in expected_draft.as_object().expect("an object") {
        let actual = draft_after.get(member);
        assert_eq!(
            &actual, expected,
            "case DRAFT after update: member {member}: expected {expected}, got {actual}"
        );
    }
    assert!(
        draft_after.instant("updatedAt") > draft_before.instant("updatedAt"),
        "case DRAFT after update: updatedAt must advance, got {} against {}",
        draft_after.instant("updatedAt"),
        draft_before.instant("updatedAt")
    );
    assert_eq!(
        draft_after.get("createdAt"),
        draft_before.get("createdAt"),
        "case DRAFT after update: createdAt must not move, got {:?}",
        draft_after.get("createdAt")
    );

    let snapshot = emulator.get(&id, Some("1")).await;
    let expected_snapshot = json!({
        "version": "1",
        "description": "snapshot one",
        "blockedInputMessaging": "first in",
        "blockedOutputsMessaging": "first out",
        "wordPolicy": {
            "words": [ { "text": "first" } ],
            "managedWordLists": [],
        },
        "topicPolicy": {
            "topics": [ {
                "name": "Topic",
                "definition": "the first topic",
                "type": "DENY",
                "examples": [],
            } ],
            "tier": { "tierName": "CLASSIC" },
        },
    });
    for (member, expected) in expected_snapshot.as_object().expect("an object") {
        let actual = snapshot.get(member);
        assert_eq!(
            &actual, expected,
            "case version 1 after the DRAFT moved on: member {member}: expected {expected}, \
             got {actual}"
        );
    }

    // The next version is cut from the CURRENT draft and numbers 2.
    let second = emulator.create_version(&id, json!({})).await;
    assert_eq!(
        second.text("version"),
        "2",
        "case second version: numbering increments per guardrail, got {:?}",
        second.get("version")
    );
    let second_snapshot = emulator.get(&id, Some("2")).await;
    assert_eq!(
        second_snapshot.get("wordPolicy"),
        json!({ "words": [ { "text": "second" } ], "managedWordLists": [] }),
        "case version 2: it snapshots the draft as it is now, got {:?}",
        second_snapshot.get("wordPolicy")
    );
    assert_eq!(
        second_snapshot.text("description"),
        "the second configuration",
        "case version 2 with no description: it inherits the DRAFT's, got {:?}",
        second_snapshot.get("description")
    );

    // `DRAFT` named explicitly is the same revision as naming no version at all.
    let explicit_draft = emulator.get(&id, Some("DRAFT")).await;
    assert_eq!(
        explicit_draft.get("wordPolicy"),
        draft_after.get("wordPolicy"),
        "case guardrailVersion=DRAFT: it names the DRAFT revision"
    );
}

/// `ListGuardrails` lists the DRAFT of every guardrail, and pages partition that
/// listing exactly: every id once, in order, for every page size.
#[tokio::test]
async fn list_pages_partition_the_listing() {
    let emulator = emulator();
    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        created.push(emulator.create_named(&format!("guard-{index}")).await);
    }

    // Page sizes below, at and above the number of stored guardrails, plus one that
    // divides it and one that does not.
    for page_size in [1_usize, 2, 3, 5, 7] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match token.as_deref() {
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
                None => format!("maxResults={page_size}"),
            };
            let reply = emulator.list(&query).await;
            assert_eq!(
                reply.status, 200,
                "case maxResults={page_size}: expected 200, got {} {:?}",
                reply.status, reply.body
            );
            let items = reply
                .get("guardrails")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case maxResults={page_size}: a page holds at most {page_size} items, got {}",
                items.len()
            );
            for item in &items {
                walked.push(
                    item.get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            token = reply.get("nextToken").as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= 10,
                "case maxResults={page_size}: pagination must terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, created,
            "case maxResults={page_size}: the pages must walk every guardrail exactly once, \
             got {walked:?}"
        );
    }

    // Every summary reports the stored identity of its guardrail.
    let all = emulator.list("").await;
    let items = all.get("guardrails").as_array().cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        created.len(),
        "case no maxResults: the default page holds every guardrail, got {:?}",
        all.body
    );
    for (index, item) in items.iter().enumerate() {
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        assert_eq!(
            item.get("arn").and_then(Value::as_str),
            Some(format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/{id}").as_str()),
            "case summary {index}: the ARN must name the id, got {item:?}"
        );
        assert_eq!(
            item.get("name").and_then(Value::as_str),
            Some(format!("guard-{index}").as_str()),
            "case summary {index}: expected the stored name, got {item:?}"
        );
        assert_eq!(
            item.get("version").and_then(Value::as_str),
            Some("DRAFT"),
            "case summary {index}: an unfiltered listing is the DRAFTs, got {item:?}"
        );
        for member in ["status", "createdAt", "updatedAt"] {
            assert!(
                item.get(member).is_some(),
                "case summary {index}: {member} is required on GuardrailSummary, got {item:?}"
            );
        }
    }
}

/// With a `guardrailIdentifier`, the listing is every version of that one guardrail —
/// the model's own documented split — and the identifier may be the id or the ARN.
#[tokio::test]
async fn list_with_an_identifier_lists_that_guardrail_s_versions() {
    let emulator = emulator();
    let id = emulator.create_named("versioned").await;
    let other = emulator.create_named("untouched").await;
    for _ in 0..2 {
        assert_eq!(
            emulator.create_version(&id, json!({})).await.status,
            202,
            "case cut a version: expected 202"
        );
    }
    let arn = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:guardrail/{id}");

    for identifier in [id.as_str(), arn.as_str()] {
        let reply = emulator
            .list(&format!("guardrailIdentifier={identifier}"))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {identifier}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
        let versions: Vec<String> = reply
            .get("guardrails")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| {
                item.get("version")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(
            versions,
            vec![
                "DRAFT".to_string(),
                "1".to_string(),
                "2".to_string()
            ],
            "case {identifier}: expected the DRAFT then each version, got {versions:?}"
        );
        let ids: Vec<Value> = reply
            .get("guardrails")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| item.get("id").cloned().unwrap_or(Value::Null))
            .collect();
        assert!(
            ids.iter().all(|listed| listed == &json!(id)),
            "case {identifier}: the filter must exclude guardrail {other}, got {ids:?}"
        );
    }

    // Paging a filtered listing partitions it the same way.
    let first = emulator
        .list(&format!("guardrailIdentifier={id}&maxResults=2"))
        .await;
    let token = first.text("nextToken");
    let second = emulator
        .list(&format!(
            "guardrailIdentifier={id}&maxResults=2&nextToken={token}"
        ))
        .await;
    let remaining: Vec<Value> = second
        .get("guardrails")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item.get("version").cloned().unwrap_or(Value::Null))
        .collect();
    assert_eq!(
        remaining,
        vec![json!("2")],
        "case filtered page two: expected the one remaining version, got {remaining:?}"
    );
    assert_eq!(
        second.get("nextToken"),
        Value::Null,
        "case filtered page two: the last page carries no nextToken, got {:?}",
        second.get("nextToken")
    );

    // That cursor belongs to THIS filtered listing: it is not a token for the
    // unfiltered listing, nor for a different guardrail's.
    let crossed = emulator.list(&format!("nextToken={token}")).await;
    assert_eq!(
        (crossed.status, crossed.error.as_deref()),
        (400, Some("ValidationException")),
        "case a filtered cursor on the unfiltered listing: expected 400 ValidationException, \
         got {} {:?}",
        crossed.status,
        crossed.body
    );
    let other_guardrail = emulator
        .list(&format!("guardrailIdentifier={other}&nextToken={token}"))
        .await;
    assert_eq!(
        (other_guardrail.status, other_guardrail.error.as_deref()),
        (400, Some("ValidationException")),
        "case a cursor for another guardrail: expected 400 ValidationException, got {} {:?}",
        other_guardrail.status,
        other_guardrail.body
    );
}

/// A delete with no version removes the guardrail and every version of it; a delete
/// naming a version removes only that snapshot.
#[tokio::test]
async fn delete_removes_what_the_request_names() {
    let emulator = emulator();
    let id = emulator.create_named("doomed").await;
    for _ in 0..2 {
        emulator.create_version(&id, json!({})).await;
    }

    // One version only.
    assert_eq!(
        emulator.delete(&id, Some("1")).await.status,
        202,
        "case delete version 1: expected 202"
    );
    let gone = emulator.get(&id, Some("1")).await;
    assert_eq!(
        (gone.status, gone.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case deleted version: expected 404 ResourceNotFoundException, got {} {:?}",
        gone.status,
        gone.body
    );
    assert_eq!(
        emulator.get(&id, Some("2")).await.status,
        200,
        "case surviving version: version 2 must still be readable"
    );
    assert_eq!(
        emulator.get(&id, None).await.status,
        200,
        "case surviving DRAFT: the guardrail itself must still be readable"
    );
    let repeat = emulator.delete(&id, Some("1")).await;
    assert_eq!(
        (repeat.status, repeat.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case delete an absent version: expected 404, got {} {:?}",
        repeat.status,
        repeat.body
    );

    // The whole guardrail, versions and all.
    assert_eq!(
        emulator.delete(&id, None).await.status,
        202,
        "case delete the guardrail: expected 202"
    );
    for version in [None, Some("DRAFT"), Some("2")] {
        let reply = emulator.get(&id, version).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {version:?} after the guardrail was deleted: expected 404, got {} {:?}",
            reply.status,
            reply.body
        );
    }
    let listed = emulator.list("").await;
    assert_eq!(
        listed.get("guardrails"),
        json!([]),
        "case list after delete: a deleted guardrail must not be listed, got {:?}",
        listed.body
    );
    let again = emulator.delete(&id, None).await;
    assert_eq!(
        (again.status, again.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case delete an absent guardrail: expected 404, got {} {:?}",
        again.status,
        again.body
    );
}

/// Every operation that addresses a guardrail answers 404 `ResourceNotFoundException`
/// for an identifier this account does not hold — never 501 and never an empty success.
#[tokio::test]
async fn an_unknown_identifier_is_a_404_on_every_operation() {
    let emulator = emulator();
    // A well-formed but never-created identifier, in both forms `GuardrailIdentifier`
    // admits.
    let unknown = ["abcdef123456", "arn:aws:bedrock:us-west-2:000000000000:guardrail/abcdef123456"];

    for identifier in unknown {
        let update = json!({
            "name": "whatever",
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        });
        let replies = vec![
            ("GetGuardrail", emulator.get(identifier, None).await),
            (
                "GetGuardrail?guardrailVersion=1",
                emulator.get(identifier, Some("1")).await,
            ),
            (
                "ListGuardrails?guardrailIdentifier",
                emulator
                    .list(&format!("guardrailIdentifier={identifier}"))
                    .await,
            ),
            ("UpdateGuardrail", emulator.update(identifier, update).await),
            (
                "CreateGuardrailVersion",
                emulator.create_version(identifier, json!({})).await,
            ),
            ("DeleteGuardrail", emulator.delete(identifier, None).await),
        ];
        for (operation, reply) in replies {
            assert_eq!(
                (reply.status, reply.error.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {operation} on {identifier}: expected 404 ResourceNotFoundException, \
                 got {} {:?}",
                reply.status,
                reply.body
            );
        }
    }

    // A version of a guardrail that DOES exist, but was never cut, is equally absent.
    let id = emulator.create_named("real").await;
    let missing_version = emulator.get(&id, Some("7")).await;
    assert_eq!(
        (missing_version.status, missing_version.error.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case an uncut version: expected 404, got {} {:?}",
        missing_version.status,
        missing_version.body
    );
}

/// The store is bounded server-side: a create past the account's guardrail quota and a
/// version past the per-guardrail version quota are both refused with
/// `ServiceQuotaExceededException`, so no caller can grow the store without limit.
#[tokio::test]
async fn the_store_is_bounded_by_the_service_quotas() {
    // The quotas this emulator enforces, from `crate::state`.
    const MAX_GUARDRAILS: usize = 100;
    const MAX_VERSIONS: usize = 20;

    let emulator = emulator();

    // Fill the account to its guardrail quota with distinct names, then one more.
    for index in 0..MAX_GUARDRAILS {
        let reply = emulator
            .create(json!({
                "name": format!("guard-{index}"),
                "blockedInputMessaging": "in",
                "blockedOutputsMessaging": "out",
            }))
            .await;
        assert_eq!(
            reply.status, 202,
            "case guardrail {index} of the quota: expected 202, got {} {:?}",
            reply.status, reply.body
        );
    }
    let over = emulator
        .create(json!({
            "name": "one-too-many",
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        }))
        .await;
    assert_eq!(
        (over.status, over.error.as_deref()),
        (400, Some("ServiceQuotaExceededException")),
        "case one guardrail past the quota: expected 400 ServiceQuotaExceededException, \
         got {} {:?}",
        over.status,
        over.body
    );

    // Freeing one admits exactly one more, so the cap is a live bound and not a
    // one-way latch.
    let listed = emulator.list("maxResults=1").await;
    let victim = listed.get("guardrails")[0]["id"]
        .as_str()
        .expect("a listed guardrail id")
        .to_string();
    assert_eq!(
        emulator.delete(&victim, None).await.status,
        202,
        "case free a slot: DeleteGuardrail must answer 202"
    );
    let after_free = emulator
        .create(json!({
            "name": "one-too-many",
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        }))
        .await;
    assert_eq!(
        after_free.status, 202,
        "case the freed slot: expected 202, got {} {:?}",
        after_free.status, after_free.body
    );

    // The same bound on the versions of one guardrail.
    let id = after_free.text("guardrailId");
    for index in 1..=MAX_VERSIONS {
        let reply = emulator.create_version(&id, json!({})).await;
        assert_eq!(
            reply.status, 202,
            "case version {index} of the quota: expected 202, got {} {:?}",
            reply.status, reply.body
        );
    }
    let over_versions = emulator.create_version(&id, json!({})).await;
    assert_eq!(
        (over_versions.status, over_versions.error.as_deref()),
        (400, Some("ServiceQuotaExceededException")),
        "case one version past the quota: expected 400 ServiceQuotaExceededException, \
         got {} {:?}",
        over_versions.status,
        over_versions.body
    );
}

/// An idempotency token never outlives the resource it minted: deleting that resource
/// releases the token, so a later replay is a fresh request rather than a reference to
/// something that no longer exists. That release is also what keeps the token
/// bookkeeping bounded by the live resources the quotas admit.
#[tokio::test]
async fn an_idempotency_token_is_released_with_the_resource_it_minted() {
    let emulator = emulator();

    // CreateGuardrailVersion: the token names one version.
    let id = emulator.create_named("tokens").await;
    let cut = emulator
        .create_version(&id, json!({ "clientRequestToken": "version-token" }))
        .await;
    assert_eq!(
        cut.text("version"),
        "1",
        "case first version: expected 1, got {:?}",
        cut.get("version")
    );
    let replay = emulator
        .create_version(&id, json!({ "clientRequestToken": "version-token" }))
        .await;
    assert_eq!(
        replay.text("version"),
        "1",
        "case replay while the version lives: the token answers with the version it cut, \
         got {:?}",
        replay.get("version")
    );
    assert_eq!(
        emulator.delete(&id, Some("1")).await.status,
        202,
        "case delete version 1: expected 202"
    );
    let after_delete = emulator
        .create_version(&id, json!({ "clientRequestToken": "version-token" }))
        .await;
    assert_eq!(
        after_delete.text("version"),
        "2",
        "case replay after that version was deleted: the released token must cut a new \
         version, got {:?}",
        after_delete.get("version")
    );

    // CreateGuardrail: the token names one guardrail.
    let request = |token: &str| {
        json!({
            "name": "token-guard",
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
            "clientRequestToken": token,
        })
    };
    let created = emulator.create(request("guard-token")).await;
    let created_id = created.text("guardrailId");
    assert_eq!(
        emulator.delete(&created_id, None).await.status,
        202,
        "case delete the guardrail: expected 202"
    );
    let recreated = emulator.create(request("guard-token")).await;
    assert_eq!(
        recreated.status, 202,
        "case replay after the guardrail was deleted: expected 202, got {} {:?}",
        recreated.status, recreated.body
    );
    assert_ne!(
        recreated.text("guardrailId"),
        created_id,
        "case replay after the guardrail was deleted: the released token must mint a new \
         guardrail, not name the deleted one"
    );
}

/// A request outside a modelled constraint is refused with 400 `ValidationException`
/// rather than stored, over every constraint the guardrail surface declares.
#[tokio::test]
async fn requests_outside_a_modelled_constraint_are_refused() {
    let emulator = emulator();
    let valid = |extra: Value| {
        let mut body = json!({
            "name": "valid",
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        });
        for (key, value) in extra.as_object().expect("an object") {
            body[key] = value.clone();
        }
        body
    };

    // Each row names the constraint it breaks; the bounds come from the model
    // (GuardrailName len 1..50 pattern ^[0-9a-zA-Z-_]+$, GuardrailBlockedMessaging
    // len 1..500, GuardrailDescription len 1..200).
    let bad_creates = vec![
        ("name outside ^[0-9a-zA-Z-_]+$", valid(json!({ "name": "spaces are not allowed" }))),
        ("name one past the 50 character bound", valid(json!({ "name": "a".repeat(51) }))),
        ("name below the 1 character bound", valid(json!({ "name": "" }))),
        (
            "blockedInputMessaging one past the 500 character bound",
            valid(json!({ "blockedInputMessaging": "b".repeat(501) })),
        ),
        (
            "blockedOutputsMessaging below the 1 character bound",
            valid(json!({ "blockedOutputsMessaging": "" })),
        ),
        (
            "description one past the 200 character bound",
            valid(json!({ "description": "d".repeat(201) })),
        ),
        ("name absent", json!({
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        })),
        ("blockedInputMessaging absent", json!({
            "name": "valid",
            "blockedOutputsMessaging": "out",
        })),
    ];

    for (case, body) in bad_creates {
        let reply = emulator.create(body).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // `maxResults` is `range 1..1000`: each published bound is accepted and one past
    // each is refused.
    for accepted in ["1", "1000"] {
        let reply = emulator.list(&format!("maxResults={accepted}")).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={accepted}: a published bound must be accepted, got {} {:?}",
            reply.status, reply.body
        );
    }
    for refused in ["0", "1001"] {
        let reply = emulator.list(&format!("maxResults={refused}")).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case maxResults={refused}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // A pagination token this service never issued is refused, not read as an empty
    // page.
    for token in ["not-a-cursor", "abcdef%2F1", "abcdef/1"] {
        let reply = emulator.list(&format!("nextToken={token}")).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case nextToken={token}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    // Nothing above was stored.
    let listed = emulator.list("").await;
    assert_eq!(
        listed.get("guardrails"),
        json!([]),
        "case after every refusal: no refused request may have been stored, got {:?}",
        listed.body
    );
}
