// SPDX-License-Identifier: CC0-1.0

//! Automated Reasoning policy VERSIONS end-to-end:
//! `CreateAutomatedReasoningPolicyVersion` cuts a numbered snapshot of a policy's draft and
//! `GetAutomatedReasoningPolicy` reads it back under the versioned
//! `AutomatedReasoningPolicyArn`, over the same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A cut version round-trips, and is a SNAPSHOT of the draft**: the version reports the
//!    name, description and `definitionHash` the policy carried when it was cut, under the
//!    policy's ARN plus the `:<version>` suffix the shape admits — for drafts differing in
//!    whether a definition, a description and tags are present and in how large the
//!    definition is. Cutting a version leaves the draft itself alone: the unversioned ARN
//!    still reports `DRAFT` and the instants and hash it reported before.
//! 2. **Version numbers ascend from 1**, one per admitted call, and each number names its
//!    own revision of the one policy (same `policyId`, own ARN).
//! 3. **`lastUpdatedDefinitionHash` is a concurrency token**: a hash that is not the draft's
//!    current one is refused with `ConflictException`, over a set of well-formed hashes that
//!    name something else.
//! 4. **A replayed `clientRequestToken` cuts nothing new**, and two distinct tokens cut two
//!    versions; a matched token is answered with the version it already cut whatever else
//!    the replayed request carries, as the member documents.
//! 5. **An ARN this account holds no policy under is 404**, and a policyArn that already
//!    carries a version is refused with `ValidationException` — a version is cut from the
//!    policy, not from another version. A version number nothing cut is 404 on the read.
//! 6. **A resource carries at most 50 tags**, the bound `TooManyTagsException` documents.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateAutomatedReasoningPolicyVersion,GetAutomatedReasoningPolicy")`).
//! The requests go through `crate::support` — the ONE harness every suite in this crate
//! drives the emulator with.

use serde_json::{json, Value};

use crate::support::{emulator, label, wall_clock_seconds, Emulator, Reply, ACCOUNT, REGION};

/// The working version a policy's unversioned ARN names:
/// `CreateAutomatedReasoningPolicyResponse$version` documents "The initial version is
/// always DRAFT".
const DRAFT: &str = "DRAFT";

/// How many digits `AutomatedReasoningPolicyHash` carries (`@length(128, 128)`, pattern
/// `^[0-9a-z]{128}$`).
const HASH_DIGITS: usize = 128;

/// The most tags one resource may carry, as `TooManyTagsException` documents it ("50 tags
/// per resource").
const MAX_TAGS: usize = 50;

/// A policy definition with `count` types, variables and rules, as the definition shapes
/// spell them (`smithy_query(query="closure",operation="CreateAutomatedReasoningPolicy")`:
/// a type name is `^\p{L}[\p{L}\p{M}0-9_]*( [\p{L}\p{M}0-9_]+)*$`, a rule id is
/// `^[A-Z][0-9A-Z]{11}$`).
fn definition(count: usize) -> Value {
    let types: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("Type{index}"),
                "description": format!("type {index}"),
                "values": [{ "value": format!("Value{index}A") }],
            })
        })
        .collect();
    let variables: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("variable{index}"),
                "type": format!("Type{index}"),
                "description": format!("variable {index}"),
            })
        })
        .collect();
    let rules: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "id": format!("R{:011}", index),
                "expression": format!("variable{index} == Value{index}A"),
            })
        })
        .collect();
    json!({ "version": "1", "types": types, "variables": variables, "rules": rules })
}

async fn create_policy(emu: &Emulator, body: Value) -> Reply {
    emu.call("POST", "/automated-reasoning-policies", Some(body))
        .await
}

async fn create_version(emu: &Emulator, policy_arn: &str, body: Value) -> Reply {
    emu.call(
        "POST",
        &format!(
            "/automated-reasoning-policies/{}/versions",
            label(policy_arn)
        ),
        Some(body),
    )
    .await
}

async fn get_policy(emu: &Emulator, policy_arn: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/automated-reasoning-policies/{}", label(policy_arn)),
        None,
    )
    .await
}

/// A policy created from `body`, as `(policyArn, definitionHash)`.
async fn policy(emu: &Emulator, body: Value) -> (String, String) {
    let created = create_policy(emu, body).await;
    assert_eq!(
        created.status, 200,
        "case setup: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
    (created.text("policyArn"), created.text("definitionHash"))
}

/// Assert `hash` is one `AutomatedReasoningPolicyHash` admits.
fn assert_hash_shape(case: &str, hash: &str) {
    assert!(
        hash.len() == HASH_DIGITS
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
        "case {case:?}: definitionHash must be {HASH_DIGITS} digits of [0-9a-z], got {hash:?} \
         ({} chars)",
        hash.len()
    );
}

// ---------------------------------------------------------------------------------------
// 1. What a version froze is what a read of that version reports, and the draft is left
//    alone.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_cut_version_snapshots_the_draft_and_round_trips_through_get() {
    // Rows differ along what the DRAFT the version is cut from carries: whether a
    // definition is present and how large it is, whether a description is present, and how
    // many tags the create-version request attaches.
    struct Row {
        case: &'static str,
        description: Option<&'static str>,
        definition: Option<Value>,
        version_tags: usize,
    }

    let rows = [
        Row {
            case: "a draft with no definition and no description",
            description: None,
            definition: None,
            version_tags: 0,
        },
        Row {
            case: "a described draft with no definition",
            description: Some("validates the returns rules"),
            definition: None,
            version_tags: 1,
        },
        Row {
            case: "a draft with one rule",
            description: Some("a single rule"),
            definition: Some(definition(1)),
            version_tags: 0,
        },
        Row {
            case: "a draft with four rules",
            description: Some("four types, variables and rules"),
            definition: Some(definition(4)),
            version_tags: MAX_TAGS,
        },
    ];

    let emu = emulator();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let mut body = json!({ "name": format!("snapshot policy {index}") });
        if let Some(description) = row.description {
            body["description"] = json!(description);
        }
        if let Some(definition) = &row.definition {
            body["policyDefinition"] = definition.clone();
        }
        let (policy_arn, draft_hash) = policy(&emu, body).await;

        let before = get_policy(&emu, &policy_arn).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the draft must read back before the version is cut, got {} {:?}",
            before.status, before.body
        );

        let tags: Vec<Value> = (0..row.version_tags)
            .map(|tag| json!({ "key": format!("key{tag}"), "value": format!("value{tag}") }))
            .collect();
        let request = json!({
            "lastUpdatedDefinitionHash": draft_hash,
            "tags": Value::Array(tags),
        });

        let opened = wall_clock_seconds();
        let cut = create_version(&emu, &policy_arn, request).await;
        let closed = wall_clock_seconds();
        assert_eq!(
            cut.status, 200,
            "case {case:?}: CreateAutomatedReasoningPolicyVersion must answer 200, got {} {:?}",
            cut.status, cut.body
        );
        assert_eq!(
            cut.text("version"),
            "1",
            "case {case:?}: the first version a policy cuts is 1, got {:?}",
            cut.get("version")
        );
        assert_eq!(
            cut.text("policyArn"),
            format!("{policy_arn}:1"),
            "case {case:?}: the version is reported under the policy ARN plus its version \
             suffix, got {:?}",
            cut.get("policyArn")
        );
        assert_eq!(
            cut.text("name"),
            before.text("name"),
            "case {case:?}: the version carries the name the draft carried, got {:?}",
            cut.get("name")
        );
        assert_eq!(
            cut.get("description"),
            before.get("description"),
            "case {case:?}: the version carries the description the draft carried, got {:?}",
            cut.get("description")
        );
        assert_eq!(
            cut.text("definitionHash"),
            draft_hash,
            "case {case:?}: the version froze the draft's definition, so it reports that \
             definition's hash, got {:?}",
            cut.get("definitionHash")
        );
        assert_hash_shape(case, &cut.text("definitionHash"));
        let created_at = cut.instant("createdAt");
        assert!(
            created_at >= opened - 1.0 && created_at <= closed + 1.0,
            "case {case:?}: createdAt must be the instant the version was cut, expected \
             within [{opened}, {closed}], got {created_at}"
        );

        // The version reads back under its own versioned ARN.
        let read = get_policy(&emu, &cut.text("policyArn")).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the versioned ARN must read back, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("policyArn"),
            cut.text("policyArn"),
            "case {case:?}: the read must report the versioned ARN it was addressed by"
        );
        assert_eq!(
            read.text("version"),
            "1",
            "case {case:?}: the versioned ARN names version 1, got {:?}",
            read.get("version")
        );
        assert_eq!(
            read.text("policyId"),
            before.text("policyId"),
            "case {case:?}: a version is a revision of the one policy, so it reports that \
             policy's policyId, got {:?}",
            read.get("policyId")
        );
        assert_eq!(
            read.text("name"),
            cut.text("name"),
            "case {case:?}: the read must report the name the cut answered"
        );
        assert_eq!(
            read.get("description"),
            cut.get("description"),
            "case {case:?}: the read must report the description the cut answered, got {:?}",
            read.get("description")
        );
        assert_eq!(
            read.text("definitionHash"),
            cut.text("definitionHash"),
            "case {case:?}: the read must report the hash the cut answered"
        );
        assert!(
            (read.instant("createdAt") - created_at).abs() < f64::EPSILON
                && (read.instant("updatedAt") - created_at).abs() < f64::EPSILON,
            "case {case:?}: nothing rewrites a version, so both instants are the one it was \
             cut at ({created_at}), got {:?}",
            read.body
        );

        // Cutting a version is not a write to the draft: the unversioned ARN reports
        // exactly what it reported before.
        let after = get_policy(&emu, &policy_arn).await;
        assert_eq!(
            after.text("version"),
            DRAFT,
            "case {case:?}: the unversioned ARN still names the DRAFT, got {:?}",
            after.get("version")
        );
        assert_eq!(
            after.body, before.body,
            "case {case:?}: cutting a version must leave the draft unchanged"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. Version numbers ascend from 1, one per admitted call.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn version_numbers_ascend_from_one_over_repeated_cuts() {
    // Policies differing in the draft the versions are cut from, so the numbering is not
    // keyed to one definition.
    let drafts: Vec<(&str, Option<Value>)> = vec![
        ("no definition", None),
        ("one rule", Some(definition(1))),
        ("three rules", Some(definition(3))),
    ];
    // How many versions each policy cuts.
    const CUTS: u32 = 4;

    let emu = emulator();
    for (index, (case, draft)) in drafts.iter().enumerate() {
        let mut body = json!({ "name": format!("numbered policy {index}") });
        if let Some(draft) = draft {
            body["policyDefinition"] = draft.clone();
        }
        let (policy_arn, draft_hash) = policy(&emu, body).await;
        let policy_id = get_policy(&emu, &policy_arn).await.text("policyId");

        for number in 1..=CUTS {
            let cut = create_version(
                &emu,
                &policy_arn,
                json!({ "lastUpdatedDefinitionHash": draft_hash }),
            )
            .await;
            assert_eq!(
                cut.status, 200,
                "case {case:?} cut {number}: must answer 200, got {} {:?}",
                cut.status, cut.body
            );
            assert_eq!(
                cut.text("version"),
                number.to_string(),
                "case {case:?} cut {number}: version numbers ascend from 1, got {:?}",
                cut.get("version")
            );
            assert_eq!(
                cut.text("policyArn"),
                format!("{policy_arn}:{number}"),
                "case {case:?} cut {number}: each number has its own ARN, got {:?}",
                cut.get("policyArn")
            );
        }

        // Every number cut reads back as a revision of the one policy; the next number
        // names nothing.
        for number in 1..=CUTS {
            let read = get_policy(&emu, &format!("{policy_arn}:{number}")).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} version {number}: must read back, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.text("policyId"),
                policy_id,
                "case {case:?} version {number}: every version reports the policy's own id, \
                 got {:?}",
                read.get("policyId")
            );
            assert_eq!(
                read.text("definitionHash"),
                draft_hash,
                "case {case:?} version {number}: each version froze the same draft, got {:?}",
                read.get("definitionHash")
            );
        }
        let unminted = get_policy(&emu, &format!("{policy_arn}:{}", CUTS + 1)).await;
        assert_eq!(
            unminted.status, 404,
            "case {case:?}: version {} was never cut, so the read must answer 404, got {} {:?}",
            CUTS + 1,
            unminted.status,
            unminted.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. The concurrency token.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_hash_that_is_not_the_drafts_current_one_conflicts() {
    let emu = emulator();
    let (policy_arn, draft_hash) = policy(
        &emu,
        json!({ "name": "token policy", "policyDefinition": definition(2) }),
    )
    .await;
    // The hash of ANOTHER definition, taken from a second policy rather than written by
    // hand, so the row is a hash this service really mints for something else.
    let (_, other_hash) = policy(
        &emu,
        json!({ "name": "other policy", "policyDefinition": definition(5) }),
    )
    .await;
    let (_, empty_hash) = policy(&emu, json!({ "name": "empty policy" })).await;

    // Rows: hashes `AutomatedReasoningPolicyHash` admits (`^[0-9a-z]{128}$`) that are not
    // this draft's, plus the draft's own hash, which must be admitted.
    let rows: Vec<(&str, String, bool)> = vec![
        ("the hash of another definition", other_hash, false),
        ("the hash of the empty definition", empty_hash, false),
        ("all zeroes", "0".repeat(HASH_DIGITS), false),
        ("all z", "z".repeat(HASH_DIGITS), false),
        ("the draft's own hash", draft_hash.clone(), true),
        (
            "the draft's own hash again, the draft being unchanged",
            draft_hash,
            true,
        ),
    ];

    for (case, hash, admitted) in &rows {
        let cut = create_version(
            &emu,
            &policy_arn,
            json!({ "lastUpdatedDefinitionHash": hash }),
        )
        .await;
        if *admitted {
            assert_eq!(
                cut.status, 200,
                "case {case:?}: the draft's current hash must be admitted, got {} {:?}",
                cut.status, cut.body
            );
        } else {
            assert_eq!(
                cut.status, 400,
                "case {case:?}: a hash that is not the draft's current one must be refused, \
                 got {} {:?}",
                cut.status, cut.body
            );
            assert_eq!(
                cut.error.as_deref(),
                Some("ConflictException"),
                "case {case:?}: expected ConflictException, got {:?}",
                cut.error
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 4. The idempotency token.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_cuts_nothing_new() {
    // Tokens differing in the characters `IdempotencyToken`
    // (`^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$`) admits.
    let tokens = ["token1", "A-token-2", "0123456789abcdef"];

    let emu = emulator();
    let (policy_arn, draft_hash) = policy(
        &emu,
        json!({ "name": "idempotent policy", "policyDefinition": definition(1) }),
    )
    .await;

    let mut cut_arns: Vec<String> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let body = json!({
            "lastUpdatedDefinitionHash": draft_hash,
            "clientRequestToken": token,
        });

        let first = create_version(&emu, &policy_arn, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: the first cut must answer 200, got {} {:?}",
            first.status, first.body
        );
        assert_eq!(
            first.text("version"),
            (index + 1).to_string(),
            "case {token:?}: a fresh token cuts the next number, got {:?}",
            first.get("version")
        );

        let replay = create_version(&emu, &policy_arn, body).await;
        assert_eq!(
            replay.status, 200,
            "case {token:?}: a replay must answer 200, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.body, first.body,
            "case {token:?}: a replay must answer with the version the token already cut"
        );

        let arn = first.text("policyArn");
        assert!(
            !cut_arns.contains(&arn),
            "case {token:?}: a distinct token must cut a distinct version, got {arn:?} twice"
        );
        cut_arns.push(arn);
    }

    // The member documents what a matched token does to the REST of the request: "If this
    // token matches a previous request, Amazon Bedrock ignores the request, but does not
    // return an error" — so a replay carrying a hash that is not the draft's (which on a
    // fresh token would conflict) is still answered with the version the token already cut.
    let replayed = create_version(
        &emu,
        &policy_arn,
        json!({
            "lastUpdatedDefinitionHash": "0".repeat(HASH_DIGITS),
            "clientRequestToken": tokens[0],
        }),
    )
    .await;
    assert_eq!(
        replayed.status, 200,
        "case a matched token with another hash: the replay is ignored rather than refused, \
         got {} {:?}",
        replayed.status, replayed.body
    );
    assert_eq!(
        replayed.text("policyArn"),
        cut_arns[0],
        "case a matched token with another hash: the replay must answer with the version the \
         token already cut, got {:?}",
        replayed.get("policyArn")
    );

    assert_eq!(
        cut_arns.len(),
        tokens.len(),
        "case all: {} distinct tokens must cut {} versions, got {cut_arns:?}",
        tokens.len(),
        tokens.len()
    );
}

// ---------------------------------------------------------------------------------------
// 5. The ARN the call is addressed by.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_that_names_no_policy_draft_is_refused() {
    let emu = emulator();
    let (policy_arn, draft_hash) = policy(
        &emu,
        json!({ "name": "addressed policy", "policyDefinition": definition(1) }),
    )
    .await;
    let cut = create_version(
        &emu,
        &policy_arn,
        json!({ "lastUpdatedDefinitionHash": draft_hash }),
    )
    .await;
    assert_eq!(
        cut.status, 200,
        "case setup: the first cut must answer 200, got {} {:?}",
        cut.status, cut.body
    );
    let prefix = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/");
    let id = policy_arn
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("case setup: ARN {policy_arn:?} is not under {prefix:?}"))
        .to_string();

    // (case, the policyArn the call is addressed by, the status and error expected). Rows:
    // an id nothing minted, this id in another region — both name no policy — and this
    // policy's own versioned ARNs, which name a version rather than the draft a version is
    // cut from.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "an id nothing minted",
            format!("{prefix}00000000000z"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the minted id in another region",
            format!("arn:aws:bedrock:eu-west-1:{ACCOUNT}:automated-reasoning-policy/{id}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the ARN of a version this policy cut",
            format!("{policy_arn}:1"),
            400,
            "ValidationException",
        ),
        (
            "the ARN of a version nothing cut",
            format!("{policy_arn}:7"),
            400,
            "ValidationException",
        ),
    ];

    for (case, arn, status, error) in &rows {
        let refused = create_version(
            &emu,
            arn,
            json!({ "lastUpdatedDefinitionHash": "0".repeat(HASH_DIGITS) }),
        )
        .await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: {arn:?} must be refused with {status}, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some(*error),
            "case {case:?}: expected {error}, got {:?}",
            refused.error
        );
    }
}

// ---------------------------------------------------------------------------------------
// 6. The tag bound.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn more_tags_than_one_resource_may_hold_is_refused() {
    // The published bound, and one past it. `TagList` admits up to 200 entries, so only the
    // per-resource bound can refuse the last row.
    let rows: &[(usize, bool)] = &[(0, true), (1, true), (MAX_TAGS, true), (MAX_TAGS + 1, false)];

    let emu = emulator();
    for (count, accepted) in rows {
        let case = format!("{count} tags");
        let (policy_arn, draft_hash) = policy(
            &emu,
            json!({ "name": format!("tagged version policy {count}") }),
        )
        .await;
        let tags: Vec<Value> = (0..*count)
            .map(|index| json!({ "key": format!("key{index}"), "value": format!("value{index}") }))
            .collect();

        let cut = create_version(
            &emu,
            &policy_arn,
            json!({ "lastUpdatedDefinitionHash": draft_hash, "tags": Value::Array(tags) }),
        )
        .await;
        if *accepted {
            assert_eq!(
                cut.status, 200,
                "case {case:?}: at most {MAX_TAGS} tags must be accepted, got {} {:?}",
                cut.status, cut.body
            );
        } else {
            assert_eq!(
                cut.status, 400,
                "case {case:?}: more than {MAX_TAGS} tags must be refused, got {} {:?}",
                cut.status, cut.body
            );
            assert_eq!(
                cut.error.as_deref(),
                Some("TooManyTagsException"),
                "case {case:?}: expected TooManyTagsException, got {:?}",
                cut.error
            );
        }
    }
}
