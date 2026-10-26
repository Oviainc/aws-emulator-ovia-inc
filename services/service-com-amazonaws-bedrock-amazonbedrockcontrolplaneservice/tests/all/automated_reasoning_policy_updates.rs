// SPDX-License-Identifier: CC0-1.0

//! `UpdateAutomatedReasoningPolicy` end-to-end: the write to an Automated Reasoning
//! policy's DRAFT, read back through `GetAutomatedReasoningPolicy` and measured against
//! the versions `CreateAutomatedReasoningPolicyVersion` cut, over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **An update rewrites the draft and round-trips**: the policy's unversioned ARN
//!    reports the definition hash, name and description the update left, its `createdAt`
//!    is untouched and its `updatedAt` is the instant of the write — for requests
//!    differing in how large the new definition is and in which optional members they
//!    carry. The hash the update answers is a function of the NEW definition alone: it is
//!    the hash a freshly created policy carrying that same definition reports.
//! 2. **An optional member the request omits leaves the policy's value in place**, the
//!    operation being bound to HTTP `PATCH`
//!    (`smithy_query(query="operation",operation="UpdateAutomatedReasoningPolicy")`).
//! 3. **A numbered version is immutable under the write**: every version cut before the
//!    update still reports what it froze, only the draft moves, and the version-cutting
//!    concurrency token follows the draft — the pre-update hash now conflicts and the
//!    post-update hash is admitted.
//! 4. **The ARN the call is addressed by**: an ARN this account holds no policy under is
//!    404, over a set of well-formed ARNs that name something else, and a `policyArn` that
//!    already carries a version is refused with `ValidationException` — `policyArn` "must
//!    be the ARN of a draft policy".
//! 5. **A name stays unique per account per region**: a name another policy of the region
//!    holds is refused with `ConflictException`, a policy keeping its own name is
//!    admitted, and a name an update moved away from is free for another policy to take.
//!
//! Every expectation is derived from the model, cited per rule
//! (`smithy_query(query="operation",operation="UpdateAutomatedReasoningPolicy,GetAutomatedReasoningPolicy,CreateAutomatedReasoningPolicyVersion")`).
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

/// A policy definition with `count` types, variables and rules, as the definition shapes
/// spell them (`smithy_query(query="closure",operation="UpdateAutomatedReasoningPolicy")`:
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

async fn update_policy(emu: &Emulator, policy_arn: &str, body: Value) -> Reply {
    emu.call(
        "PATCH",
        &format!("/automated-reasoning-policies/{}", label(policy_arn)),
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
// 1 and 2. What the update wrote is what the draft reports, and an omitted member is left
//          alone.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_update_rewrites_the_draft_and_round_trips_through_get() {
    // Rows differ along what the update CARRIES: how large the new definition is (including
    // the empty one), and whether it also renames the policy and re-describes it.
    struct Row {
        case: &'static str,
        definition: Value,
        name: Option<&'static str>,
        description: Option<&'static str>,
    }

    let rows = [
        Row {
            case: "an empty definition, nothing else",
            definition: definition(0),
            name: None,
            description: None,
        },
        Row {
            case: "one rule and a new description",
            definition: definition(1),
            name: None,
            description: Some("one rule now"),
        },
        Row {
            case: "three rules and a new name",
            definition: definition(3),
            name: Some("renamed update policy"),
            description: None,
        },
        Row {
            case: "seven rules, a new name and a new description",
            definition: definition(7),
            name: Some("renamed and redescribed policy"),
            description: Some("seven types, variables and rules"),
        },
    ];

    let emu = emulator();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (policy_arn, created_hash) = policy(
            &emu,
            json!({
                "name": format!("update policy {index}"),
                "description": "as created",
                "policyDefinition": definition(2),
            }),
        )
        .await;
        let before = get_policy(&emu, &policy_arn).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the draft must read back before the update, got {} {:?}",
            before.status, before.body
        );

        let mut request = json!({ "policyDefinition": row.definition.clone() });
        if let Some(name) = row.name {
            request["name"] = json!(name);
        }
        if let Some(description) = row.description {
            request["description"] = json!(description);
        }

        let opened = wall_clock_seconds();
        let updated = update_policy(&emu, &policy_arn, request).await;
        let closed = wall_clock_seconds();
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateAutomatedReasoningPolicy must answer 200, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.text("policyArn"),
            policy_arn,
            "case {case:?}: the update reports the policy's own ARN, got {:?}",
            updated.get("policyArn")
        );
        // Rule 2: an omitted `name` leaves the policy's own name in place.
        let expected_name = row.name.map_or_else(|| before.text("name"), str::to_string);
        assert_eq!(
            updated.text("name"),
            expected_name,
            "case {case:?}: expected name {expected_name:?}, got {:?}",
            updated.get("name")
        );
        assert_hash_shape(case, &updated.text("definitionHash"));
        assert_ne!(
            updated.text("definitionHash"),
            created_hash,
            "case {case:?}: the definition changed, so its hash must change too"
        );
        let updated_at = updated.instant("updatedAt");
        assert!(
            updated_at >= opened - 1.0 && updated_at <= closed + 1.0,
            "case {case:?}: updatedAt must be the instant of the write, expected within \
             [{opened}, {closed}], got {updated_at}"
        );

        // The hash is a function of the NEW definition alone: a policy created with that
        // same definition, under another name and description, reports the same hash.
        let (_, twin_hash) = policy(
            &emu,
            json!({
                "name": format!("twin policy {index}"),
                "policyDefinition": row.definition.clone(),
            }),
        )
        .await;
        assert_eq!(
            updated.text("definitionHash"),
            twin_hash,
            "case {case:?}: the hash is a function of the definition alone, so the updated \
             draft and a policy created with that definition must agree, got {:?}",
            updated.get("definitionHash")
        );

        // The draft reads back exactly what the update answered.
        let after = get_policy(&emu, &policy_arn).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: the updated draft must read back, got {} {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.text("version"),
            DRAFT,
            "case {case:?}: the unversioned ARN still names the DRAFT, got {:?}",
            after.get("version")
        );
        assert_eq!(
            after.text("name"),
            updated.text("name"),
            "case {case:?}: the read must report the name the update answered, got {:?}",
            after.get("name")
        );
        assert_eq!(
            after.text("definitionHash"),
            updated.text("definitionHash"),
            "case {case:?}: the read must report the hash the update answered, got {:?}",
            after.get("definitionHash")
        );
        assert_eq!(
            after.text("policyId"),
            before.text("policyId"),
            "case {case:?}: an update writes the one policy, so its policyId is unchanged, \
             got {:?}",
            after.get("policyId")
        );
        // Rule 2 again: an omitted `description` leaves the policy's own description in
        // place.
        let expected_description = row
            .description
            .map_or_else(|| before.get("description"), |text| json!(text));
        assert_eq!(
            after.get("description"),
            expected_description,
            "case {case:?}: expected description {expected_description:?}, got {:?}",
            after.get("description")
        );
        assert!(
            (after.instant("createdAt") - before.instant("createdAt")).abs() < f64::EPSILON,
            "case {case:?}: an update is no creation, so createdAt must not move: expected \
             {}, got {}",
            before.instant("createdAt"),
            after.instant("createdAt")
        );
        assert!(
            (after.instant("updatedAt") - updated_at).abs() < f64::EPSILON,
            "case {case:?}: the read must report the instant the update answered \
             ({updated_at}), got {}",
            after.instant("updatedAt")
        );
        assert!(
            after.instant("updatedAt") >= before.instant("updatedAt"),
            "case {case:?}: updatedAt must not move backwards: was {}, got {}",
            before.instant("updatedAt"),
            after.instant("updatedAt")
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. A version froze its definition, so the write to the draft is invisible to it.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_update_leaves_every_cut_version_alone() {
    // Rows differ in how many versions were cut before the update and in the definition the
    // update writes, so the snapshot rule is not keyed to one shape of request.
    let rows: &[(&str, u32, Value)] = &[
        ("one version, update to the empty definition", 1, definition(0)),
        ("two versions, update to one rule", 2, definition(1)),
        ("three versions, update to five rules", 3, definition(5)),
    ];

    let emu = emulator();
    for (index, (case, cuts, new_definition)) in rows.iter().enumerate() {
        let (policy_arn, draft_hash) = policy(
            &emu,
            json!({
                "name": format!("snapshotted policy {index}"),
                "description": "before the update",
                "policyDefinition": definition(2),
            }),
        )
        .await;

        // Every version is cut from the one draft, so each froze that draft's hash.
        let mut frozen: Vec<(String, Value)> = Vec::new();
        for number in 1..=*cuts {
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
            let arn = cut.text("policyArn");
            let read = get_policy(&emu, &arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} cut {number}: the version must read back, got {} {:?}",
                read.status, read.body
            );
            frozen.push((arn, read.body));
        }

        let updated = update_policy(
            &emu,
            &policy_arn,
            json!({
                "policyDefinition": new_definition.clone(),
                "name": format!("snapshotted policy {index} renamed"),
                "description": "after the update",
            }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: the update must answer 200, got {} {:?}",
            updated.status, updated.body
        );
        assert_ne!(
            updated.text("definitionHash"),
            draft_hash,
            "case {case:?}: the draft's hash must move with its definition"
        );

        for (arn, body) in &frozen {
            let read = get_policy(&emu, arn).await;
            assert_eq!(
                &read.body, body,
                "case {case:?}: version {arn:?} is a snapshot, so the update must leave every \
                 member it reports alone, got {:?}",
                read.body
            );
        }

        // The concurrency token follows the draft: the hash read before the update now
        // names a definition the draft has moved past, while the hash the update answered
        // is admitted.
        let stale = create_version(
            &emu,
            &policy_arn,
            json!({ "lastUpdatedDefinitionHash": draft_hash }),
        )
        .await;
        assert_eq!(
            stale.status, 400,
            "case {case:?}: a cut against the pre-update hash must be refused, got {} {:?}",
            stale.status, stale.body
        );
        assert_eq!(
            stale.error.as_deref(),
            Some("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?}",
            stale.error
        );
        let fresh = create_version(
            &emu,
            &policy_arn,
            json!({ "lastUpdatedDefinitionHash": updated.text("definitionHash") }),
        )
        .await;
        assert_eq!(
            fresh.status, 200,
            "case {case:?}: a cut against the hash the update answered must be admitted, got \
             {} {:?}",
            fresh.status, fresh.body
        );
        assert_eq!(
            fresh.text("version"),
            (cuts + 1).to_string(),
            "case {case:?}: the next version number follows the ones already cut, got {:?}",
            fresh.get("version")
        );
        assert_eq!(
            fresh.text("definitionHash"),
            updated.text("definitionHash"),
            "case {case:?}: the new version froze the definition the update wrote, got {:?}",
            fresh.get("definitionHash")
        );
        assert_eq!(
            fresh.text("name"),
            updated.text("name"),
            "case {case:?}: the new version carries the name the update left, got {:?}",
            fresh.get("name")
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. The ARN the call is addressed by.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_that_names_no_policy_draft_is_refused() {
    let emu = emulator();
    let (policy_arn, draft_hash) = policy(
        &emu,
        json!({ "name": "addressed update policy", "policyDefinition": definition(1) }),
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
    // an id nothing minted and this id in another region — both name no policy of this
    // account — and versioned ARNs, which name a snapshot rather than the draft.
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
            format!("{policy_arn}:9"),
            400,
            "ValidationException",
        ),
    ];

    for (case, arn, status, error) in &rows {
        let refused = update_policy(&emu, arn, json!({ "policyDefinition": definition(2) })).await;
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

    // None of those refusals wrote anything: the draft still reports the hash it carried.
    let read = get_policy(&emu, &policy_arn).await;
    assert_eq!(
        read.text("definitionHash"),
        draft_hash,
        "case all: a refused update must write nothing, got {:?}",
        read.get("definitionHash")
    );
}

// ---------------------------------------------------------------------------------------
// 5. A name stays unique within an account and region.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_name_another_policy_holds_is_refused() {
    let emu = emulator();
    // Names differing along what `AutomatedReasoningPolicyName` (`^[0-9a-zA-Z-_ ]+$`)
    // admits, each held by a policy of this region.
    let held = ["held policy", "held-policy-2", "held_POLICY_3"];
    for name in &held {
        policy(&emu, json!({ "name": name })).await;
    }
    let (policy_arn, _) = policy(&emu, json!({ "name": "mover policy" })).await;

    for name in &held {
        let refused = update_policy(
            &emu,
            &policy_arn,
            json!({ "policyDefinition": definition(1), "name": name }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: a name another policy of this region holds must be refused, got \
             {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ConflictException"),
            "case {name:?}: expected ConflictException, got {:?}",
            refused.error
        );
        let read = get_policy(&emu, &policy_arn).await;
        assert_eq!(
            read.text("name"),
            "mover policy",
            "case {name:?}: a refused rename must leave the name alone, got {:?}",
            read.get("name")
        );
    }

    // Its OWN name is not another policy's, so an update that keeps it is admitted.
    let kept = update_policy(
        &emu,
        &policy_arn,
        json!({ "policyDefinition": definition(1), "name": "mover policy" }),
    )
    .await;
    assert_eq!(
        kept.status, 200,
        "case its own name: a policy keeping its name must be admitted, got {} {:?}",
        kept.status, kept.body
    );

    // A free name is taken, and the name it moved away from is then free for a create.
    let moved = update_policy(
        &emu,
        &policy_arn,
        json!({ "policyDefinition": definition(2), "name": "moved policy" }),
    )
    .await;
    assert_eq!(
        moved.status, 200,
        "case a free name: the rename must be admitted, got {} {:?}",
        moved.status, moved.body
    );
    assert_eq!(
        moved.text("name"),
        "moved policy",
        "case a free name: the update must report the new name, got {:?}",
        moved.get("name")
    );
    let retaken = create_policy(&emu, json!({ "name": "mover policy" })).await;
    assert_eq!(
        retaken.status, 200,
        "case the vacated name: the name the rename freed must be mintable again, got {} {:?}",
        retaken.status, retaken.body
    );
    assert_ne!(
        retaken.text("policyArn"),
        policy_arn,
        "case the vacated name: the create must mint a second policy"
    );
    // And the renamed policy is now the only holder of its new name.
    let taken_again = create_policy(&emu, json!({ "name": "moved policy" })).await;
    assert_eq!(
        taken_again.status, 400,
        "case the new name: the name the rename took must now conflict, got {} {:?}",
        taken_again.status, taken_again.body
    );
    assert_eq!(
        taken_again.error.as_deref(),
        Some("ConflictException"),
        "case the new name: expected ConflictException, got {:?}",
        taken_again.error
    );
}
