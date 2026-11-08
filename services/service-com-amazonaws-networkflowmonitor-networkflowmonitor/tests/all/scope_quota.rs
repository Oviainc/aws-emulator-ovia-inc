// SPDX-License-Identifier: CC0-1.0

//! The two rules this suite pins, each over a TABLE of inputs:
//!
//! * RULE Q (one scope per account and Region): `CreateScope` mints the account's
//!   single scope for the Region the request is made in; any further create that
//!   would claim that Region's slot is refused with the modelled
//!   `ServiceQuotaExceededException` (HTTP 402) naming the limit, the resource and
//!   the caller's account, leaving the scope that holds the slot untouched.
//!   Deleting it frees the slot, and another Region's slot is its own.
//! * RULE P (an addition the scope already holds is refused): `UpdateScope`
//!   `resourcesToAdd` naming an account/Region pair the scope already holds is the
//!   modelled `ValidationException` (HTTP 400) naming every such pair, decided
//!   before anything is written.
//!
//! Both are driven over the service's own `restJson1` wire protocol through the one
//! harness this crate shares.

use serde_json::{json, Value};

use crate::harness::{
    client_token, extra_region, targets, Answer, Emulator, ACCOUNT_ID, PAST_PROVISIONING, REGION,
};

/// `state::SCOPE_QUOTA`: the scopes one account holds in one Region.
const SCOPE_QUOTA: usize = 1;

/// The message every quota refusal of a scope carries: the limit, the resource type
/// and the caller's account, in the fixed text the real service words it with.
fn quota_message() -> String {
    format!(
        "Maximum limit of [{SCOPE_QUOTA}] for resource [scope] exceeded for account [{ACCOUNT_ID}]."
    )
}

/// The message an `UpdateScope` carries when `resourcesToAdd` names pairs the scope
/// already holds: the already-present pairs as `<accountId>#<region>`, in the order
/// the request listed them.
fn already_in_scope_message(pairs: &[(&str, &str)]) -> String {
    let rendered: Vec<String> = pairs
        .iter()
        .map(|(account, region)| format!("{account}#{region}"))
        .collect();
    format!(
        "invalid input -> (cannot add account/region pairs=[[{}]] as they are already in scope.)",
        rendered.join(", ")
    )
}

/// Assert `answer` is the modelled 402 with the quota wording.
fn assert_quota_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 402,
        "case {case:?}: expected 402, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.is_error("ServiceQuotaExceededException"),
        "case {case:?}: expected ServiceQuotaExceededException, got {:?} / body {}",
        answer.error_type,
        answer.body
    );
    assert_eq!(
        answer.text("message"),
        quota_message(),
        "case {case:?}: expected {:?}, got {:?}",
        quota_message(),
        answer.text("message")
    );
}

/// Assert `answer` is the modelled 400 with the already-in-scope wording for
/// `pairs`.
fn assert_already_in_scope(case: &str, answer: &Answer, pairs: &[(&str, &str)]) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.is_error("ValidationException"),
        "case {case:?}: expected ValidationException, got {:?} / body {}",
        answer.error_type,
        answer.body
    );
    let expected = already_in_scope_message(pairs);
    assert_eq!(
        answer.text("message"),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        answer.text("message")
    );
}

/// How many scopes the store holds, through `ListScopes`.
async fn listed_scope_ids(emulator: &Emulator) -> Vec<String> {
    let listing = emulator.list_scopes("?maxResults=25").await;
    assert_eq!(
        listing.status, 200,
        "the listing must succeed, got {} with body {}",
        listing.status, listing.body
    );
    listing
        .member("scopes")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item.get("scopeId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The create bodies the quota table spans: rows differ along the target count,
/// whether a `clientToken` is carried, and whether `tags` are.
fn create_bodies() -> Vec<(&'static str, Value)> {
    // 25 targets: the count dimension, well inside `TargetResourceList`'s 1..99.
    let many: Vec<(String, String)> = (0..25)
        .map(|index| {
            (
                format!("{:012}", 300_000_000_000u64 + index),
                REGION.to_string(),
            )
        })
        .collect();
    let many_refs: Vec<(&str, &str)> = many
        .iter()
        .map(|(account, region)| (account.as_str(), region.as_str()))
        .collect();
    vec![
        (
            "one target, nothing else",
            json!({ "targets": targets(&[(ACCOUNT_ID, REGION)]) }),
        ),
        (
            "a clientToken and tags",
            json!({
                "targets": targets(&[("123456789012", "us-east-1")]),
                "clientToken": client_token(31),
                "tags": { "owner": "net-team" }
            }),
        ),
        (
            "three targets spanning accounts and Regions",
            json!({
                "targets": targets(&[
                    ("111111111111", "eu-west-1"),
                    ("222222222222", "ap-southeast-2"),
                    ("333333333333", "us-east-2"),
                ])
            }),
        ),
        (
            "25 targets: the list's count dimension",
            json!({ "targets": targets(&many_refs) }),
        ),
    ]
}

/// RULE Q: whatever the create asks for, the account's second scope in one Region
/// is the modelled quota refusal, and the scope holding the slot is untouched.
#[tokio::test]
async fn a_second_scope_in_one_region_is_the_modelled_quota_refusal() {
    let bodies = create_bodies();
    for (case, body) in &bodies {
        let emulator = Emulator::new();
        let first = emulator.create_scope(body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the account's first scope must be created, got {} with body {}",
            first.status, first.body
        );
        let scope_id = first.text("scopeId");

        // Every other row's body is refused too, including the row's own body with
        // a fresh `clientToken`: what is full is the Region's slot, not one shape
        // of request.
        for (other_case, other) in &bodies {
            let case = format!("{case} holds the slot, {other_case} asks for another");
            let mut asked = other.clone();
            asked["clientToken"] = json!(client_token(900));
            assert_quota_refusal(&case, &emulator.create_scope(asked).await);
        }

        // The refusals stored nothing: the account still holds exactly the one
        // scope, with the targets it was created with.
        assert_eq!(
            listed_scope_ids(&emulator).await,
            vec![scope_id.clone()],
            "case {case:?}: the store must still hold exactly {SCOPE_QUOTA} scope"
        );
        let read = emulator.get_scope(&scope_id).await;
        assert_eq!(
            read.member("targets"),
            body.get("targets").expect("every row carries targets"),
            "case {case:?}: a refused create must not edit the live scope, got {:?}",
            read.member("targets")
        );
    }
}

/// RULE Q, under concurrency: two creates of the IDENTICAL target on a cold
/// account yield exactly one winner, the other the quota refusal — and once the
/// slot is taken, both of a further pair are refused.
#[tokio::test]
async fn concurrent_creates_of_one_target_leave_exactly_one_winner() {
    for (case, pairs) in [
        ("the caller's own account", vec![(ACCOUNT_ID, REGION)]),
        (
            "two accounts in one target list",
            vec![("111111111111", REGION), ("222222222222", "eu-west-1")],
        ),
    ] {
        let emulator = Emulator::new();
        let body = json!({ "targets": targets(&pairs) });
        let (left, right) = tokio::join!(
            emulator.create_scope(body.clone()),
            emulator.create_scope(body.clone())
        );
        let winners = [&left, &right]
            .iter()
            .filter(|answer| answer.status == 200)
            .count();
        assert_eq!(
            winners, SCOPE_QUOTA,
            "case {case:?}: expected exactly {SCOPE_QUOTA} winner, got {} and {} with bodies {} / {}",
            left.status, right.status, left.body, right.body
        );
        let loser = if left.status == 200 { &right } else { &left };
        assert_quota_refusal(&format!("{case}: the loser"), loser);
        assert_eq!(
            listed_scope_ids(&emulator).await.len(),
            SCOPE_QUOTA,
            "case {case:?}: one scope must exist after two concurrent creates"
        );

        // The slot is occupied now, so BOTH of a further pair are refused.
        let (left, right) = tokio::join!(
            emulator.create_scope(body.clone()),
            emulator.create_scope(body.clone())
        );
        assert_quota_refusal(&format!("{case}: the first of an occupied slot"), &left);
        assert_quota_refusal(&format!("{case}: the second of an occupied slot"), &right);
    }
}

/// RULE Q's two complements: the quota is per REGION — each Region's slot is its
/// own — and it bounds what the store HOLDS, so a delete frees the slot.
#[tokio::test]
async fn each_region_holds_its_own_scope_and_a_delete_frees_the_slot() {
    let emulator = Emulator::new();
    let body = json!({ "targets": targets(&[(ACCOUNT_ID, "us-east-1")]) });

    // One scope per Region, over Regions the issue does not cite, plus the
    // harness's own: each create is accepted, and the second create in the SAME
    // Region is refused while every other Region is still free.
    let mut minted: Vec<String> = Vec::new();
    for index in 0..4usize {
        let region = extra_region(index);
        let created = emulator.create_scope_in(region, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {region}: a Region's own slot must take a scope, got {} with body {}",
            created.status, created.body
        );
        minted.push(created.text("scopeId"));
        assert_quota_refusal(
            &format!("a second scope in {region}"),
            &emulator.create_scope_in(region, body.clone()).await,
        );
    }
    // Each Region's scope was really stored, read back through the reader that
    // serves one scope by id. (`ListScopes` is not Region-partitioned in this
    // emulator, so the store-wide listing is left to `scope_listing.rs`; what this
    // row is about is the quota, not how a listing is scoped.)
    for (index, scope_id) in minted.iter().enumerate() {
        let read = emulator.get_scope(scope_id).await;
        assert_eq!(
            read.status, 200,
            "case {}: the Region's scope must be stored, got {} with body {}",
            extra_region(index),
            read.status,
            read.body
        );
    }

    // Deleting a Region's scope frees that Region's slot, and only that one.
    let freed = extra_region(0);
    assert_eq!(
        emulator.delete_scope(&minted[0]).await.status,
        200,
        "the delete must succeed"
    );
    let again = emulator.create_scope_in(freed, body.clone()).await;
    assert_eq!(
        again.status, 200,
        "case {freed}: a freed slot must take a new scope, got {} with body {}",
        again.status, again.body
    );
    assert_ne!(
        again.text("scopeId"),
        minted[0],
        "the new scope must not be the deleted one, got {:?}",
        again.text("scopeId")
    );
    assert_quota_refusal(
        &format!("another Region ({}) is still occupied", extra_region(1)),
        &emulator
            .create_scope_in(extra_region(1), body.clone())
            .await,
    );
}

/// RULE Q's belt: the Region a create claims a slot in comes from the request's
/// own credential scope, so the store is ALSO held to a ceiling across Regions
/// (`state::MAX_SCOPES`) and cannot grow one record per Region a caller names.
#[tokio::test]
async fn the_store_is_bounded_across_regions_too() {
    // `state::MAX_SCOPES`, the store-wide ceiling.
    const MAX_SCOPES: usize = 100;

    let emulator = Emulator::new();
    let body = json!({ "targets": targets(&[(ACCOUNT_ID, "us-east-1")]) });
    // Region strings a credential scope can carry (what `state::request_region`
    // accepts), which is the input a caller actually controls here — not a
    // published Region list.
    let named: Vec<String> = (0..=MAX_SCOPES).map(|index| format!("r{index}-1")).collect();

    for (index, region) in named.iter().take(MAX_SCOPES).enumerate() {
        let created = emulator.create_scope_in(region, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {index} ({region}): every create up to the ceiling succeeds, got {} with body {}",
            created.status, created.body
        );
    }
    let past = emulator
        .create_scope_in(&named[MAX_SCOPES], body.clone())
        .await;
    assert_eq!(
        past.status, 402,
        "case {} ({}): past the store's ceiling, got {} with body {}",
        MAX_SCOPES, named[MAX_SCOPES], past.status, past.body
    );
    assert!(
        past.is_error("ServiceQuotaExceededException"),
        "expected ServiceQuotaExceededException, got {:?} / body {}",
        past.error_type,
        past.body
    );
    assert_eq!(
        past.text("message"),
        format!(
            "Maximum limit of [{MAX_SCOPES}] for resource [scope] exceeded for account [{ACCOUNT_ID}]."
        ),
        "the refusal names the limit that was reached, got {:?}",
        past.text("message")
    );
}

/// RULE Q does not swallow the `@idempotent` replay: the token that minted the
/// account's scope still answers with that scope rather than with the quota
/// refusal.
#[tokio::test]
async fn the_minting_client_token_still_replays_under_the_quota() {
    let emulator = Emulator::new();
    let body = json!({
        "targets": targets(&[(ACCOUNT_ID, REGION)]),
        "clientToken": client_token(42)
    });
    let first = emulator.create_scope(body.clone()).await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} with body {}",
        first.status, first.body
    );
    for attempt in 0..3 {
        let replay = emulator.create_scope(body.clone()).await;
        assert_eq!(
            replay.status, 200,
            "attempt {attempt}: the replay must answer with the minted scope, got {} with body {}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.text("scopeId"),
            first.text("scopeId"),
            "attempt {attempt}: expected {:?}, got {:?}",
            first.text("scopeId"),
            replay.text("scopeId")
        );
    }
}

/// One row of the already-in-scope table: what the scope holds, what the update
/// asks to add, and the pairs the refusal must name.
struct DuplicateAdd {
    case: &'static str,
    held: Vec<(&'static str, &'static str)>,
    to_add: Vec<(&'static str, &'static str)>,
    named: Vec<(&'static str, &'static str)>,
}

fn duplicate_add_rows() -> Vec<DuplicateAdd> {
    vec![
        DuplicateAdd {
            case: "the scope's only target, re-added",
            held: vec![("111111111111", REGION)],
            to_add: vec![("111111111111", REGION)],
            named: vec![("111111111111", REGION)],
        },
        DuplicateAdd {
            case: "one of three targets, re-added",
            held: vec![
                ("111111111111", REGION),
                ("222222222222", REGION),
                ("333333333333", REGION),
            ],
            to_add: vec![("222222222222", REGION)],
            named: vec![("222222222222", REGION)],
        },
        DuplicateAdd {
            case: "a new target first, an already-held one second",
            held: vec![("111111111111", REGION)],
            to_add: vec![("444444444444", REGION), ("111111111111", REGION)],
            named: vec![("111111111111", REGION)],
        },
        DuplicateAdd {
            case: "an already-held target first, a new one second",
            held: vec![("111111111111", REGION)],
            to_add: vec![("111111111111", REGION), ("444444444444", REGION)],
            named: vec![("111111111111", REGION)],
        },
        DuplicateAdd {
            case: "two already-held targets at once: the refusal names both, in request order",
            held: vec![("111111111111", REGION), ("222222222222", REGION)],
            to_add: vec![("222222222222", REGION), ("111111111111", REGION)],
            named: vec![("222222222222", REGION), ("111111111111", REGION)],
        },
        DuplicateAdd {
            case: "the caller's own account, re-added",
            held: vec![(ACCOUNT_ID, REGION)],
            to_add: vec![(ACCOUNT_ID, REGION)],
            named: vec![(ACCOUNT_ID, REGION)],
        },
    ]
}

/// RULE P: an addition the scope already holds is refused, and the scope is left
/// exactly as it was.
#[tokio::test]
async fn adding_a_target_the_scope_already_holds_is_refused() {
    for row in duplicate_add_rows() {
        let emulator = Emulator::new();
        let created = emulator
            .create_scope(json!({ "targets": targets(&row.held) }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the scope must be created, got {} with body {}",
            row.case, created.status, created.body
        );
        let scope_id = created.text("scopeId");
        // Past the provisioning span, so the row also pins that the refusal does
        // not move a scope that is already SUCCEEDED.
        tokio::time::sleep(PAST_PROVISIONING).await;
        let before = emulator.get_scope(&scope_id).await;
        assert_eq!(
            before.text("status"),
            "SUCCEEDED",
            "case {:?}: the scope must be SUCCEEDED before the update, got {:?}",
            row.case,
            before.text("status")
        );

        let answer = emulator
            .update_scope(
                &scope_id,
                json!({ "resourcesToAdd": targets(&row.to_add) }),
            )
            .await;
        assert_already_in_scope(row.case, &answer, &row.named);

        let after = emulator.get_scope(&scope_id).await;
        assert_eq!(
            after.member("targets"),
            before.member("targets"),
            "case {:?}: a refused add must not edit the targets, got {:?}",
            row.case,
            after.member("targets")
        );
        assert_eq!(
            after.text("status"),
            before.text("status"),
            "case {:?}: a refused add must not move the status, got {:?}",
            row.case,
            after.text("status")
        );
    }
}

/// RULE P's complement, and its reach through an earlier update: a target the
/// scope does NOT hold is added, and asking for it a second time is then refused —
/// so the refusal is about what the scope holds, not about the request's shape.
#[tokio::test]
async fn an_add_is_accepted_once_and_refused_the_second_time() {
    let emulator = Emulator::new();
    let held = ("111111111111", REGION);
    let created = emulator
        .create_scope(json!({ "targets": targets(&[held]) }))
        .await;
    assert_eq!(
        created.status, 200,
        "the scope must be created, got {} with body {}",
        created.status, created.body
    );
    let scope_id = created.text("scopeId");

    for account in ["222222222222", "333333333333", "444444444444"] {
        let body = json!({ "resourcesToAdd": targets(&[(account, REGION)]) });
        let accepted = emulator.update_scope(&scope_id, body.clone()).await;
        assert_eq!(
            accepted.status, 200,
            "case {account}: a target the scope does not hold must be added, got {} with body {}",
            accepted.status, accepted.body
        );
        assert_already_in_scope(
            &format!("{account}, asked for twice"),
            &emulator.update_scope(&scope_id, body.clone()).await,
            &[(account, REGION)],
        );

        // Deleting it makes the same add admissible again: what decides the
        // refusal is the stored target set.
        let removed = emulator
            .update_scope(
                &scope_id,
                json!({ "resourcesToDelete": targets(&[(account, REGION)]) }),
            )
            .await;
        assert_eq!(
            removed.status, 200,
            "case {account}: the removal must succeed, got {} with body {}",
            removed.status, removed.body
        );
        let re_added = emulator.update_scope(&scope_id, body).await;
        assert_eq!(
            re_added.status, 200,
            "case {account}: a removed target may be added again, got {} with body {}",
            re_added.status, re_added.body
        );
        let cleanup = emulator
            .update_scope(
                &scope_id,
                json!({ "resourcesToDelete": targets(&[(account, REGION)]) }),
            )
            .await;
        assert_eq!(
            cleanup.status, 200,
            "case {account}: the cleanup removal must succeed, got {} with body {}",
            cleanup.status, cleanup.body
        );
    }
}
