// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the policy-store-alias lifecycle: create, read back,
//! resolve as a `policyStoreId`, soft-delete and hard-delete.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so
//! a write made by one request is only visible to the next if it really reached
//! state.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness is shared with every other suite in this crate; this suite
// adds only its own alias-specific helpers below.
use super::harness::{call, error_code, ok, store_with_validation_off as store, SIGNING_REGION};

/// The prefix every alias name carries, from
/// `CreatePolicyStoreAliasInput$aliasName`'s model documentation: "The alias name
/// must always be prefixed with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// `Alias`'s published upper length bound in the Smithy model (`length 0..150`).
const ALIAS_MAX_LEN: usize = 150;

/// `MAX_POLICY_STORE_ALIASES` in `src/state.rs`.
const MAX_POLICY_STORE_ALIASES: usize = 100;

/// The shared harness router, behind an `Arc` so the concurrency cases below can
/// hand one router to several tasks.
fn router() -> Arc<MultiServiceRouter> {
    Arc::new(super::harness::router())
}

/// The ARN an alias of `alias_name` must carry: the canonical emulator account
/// (`000000000000`), the region the request was SIGNED for — Verified Permissions
/// is regional and an alias name is unique in the account and Region, so
/// `ResourceArn`'s third colon-separated field names it — and the alias name as
/// the resource, which already states its own resource type through its prefix.
/// Satisfies `ResourceArn`'s `^arn:[^:]*:[^:]*:[^:]*:[^:]*:.*$`.
fn expected_alias_arn(region: &str, alias_name: &str) -> String {
    format!("arn:aws:verifiedpermissions:{region}:000000000000:{alias_name}")
}

/// Create an alias and return the parsed response, requiring success.
async fn create_alias(router: &MultiServiceRouter, alias_name: &str, store_id: &str) -> Value {
    ok(
        router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
    )
    .await
}

/// The instant a `@timestampFormat("date-time")` member carries, in epoch seconds.
///
/// Such a member is an RFC 3339 STRING on the wire (see
/// `tests/all/date_time_wire_format.rs`), so an ORDERING comparison parses it instead of
/// reading a number; a value that is not a date-time string yields `NaN`, which fails
/// every comparison rather than passing one.
fn date_time_secs(value: &Value) -> f64 {
    ::aws_smithy_types::DateTime::from_str(
        value.as_str().unwrap_or_default(),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .map_or(f64::NAN, |parsed| parsed.as_secs_f64())
}

// ---------------------------------------------------------------------------
// Create, read back, resolve
// ---------------------------------------------------------------------------

/// Every member of a created alias is read back out of state by
/// `GetPolicyStoreAlias`, and the alias resolves as a `policyStoreId`.
///
/// The rows differ along the alias name: its length (one character up to the
/// model's 150-character bound), its character classes (letters, digits, `-`,
/// `_`) and how many `/` separators it carries past the required prefix.
#[tokio::test]
async fn alias_round_trips_every_name_the_model_admits() {
    // The suffix after `policy-store-alias/`; the longest row fills `Alias`'s
    // published 150-character bound exactly.
    let longest = "l".repeat(ALIAS_MAX_LEN - ALIAS_PREFIX.len());
    let cases: &[(&str, &str)] = &[
        ("one character", "a"),
        ("hyphenated", "photos-prod-2026"),
        ("underscored", "team_photos"),
        ("nested separators", "eu/prod/photos"),
        ("digits only", "20261022"),
        ("at the length bound", longest.as_str()),
    ];

    let router = router();
    for (case, suffix) in cases {
        let alias_name = format!("{ALIAS_PREFIX}{suffix}");
        assert!(
            alias_name.len() <= ALIAS_MAX_LEN,
            "case {case:?}: the row itself must stay inside the model's bound, got {}",
            alias_name.len()
        );
        let store_id = store(&router).await;

        let created = create_alias(&router, &alias_name, &store_id).await;
        assert_eq!(
            created["aliasName"].as_str().unwrap_or_default(),
            alias_name,
            "case {case:?}: unexpected aliasName, got {created}"
        );
        assert_eq!(
            created["policyStoreId"].as_str().unwrap_or_default(),
            store_id,
            "case {case:?}: unexpected policyStoreId, got {created}"
        );
        assert_eq!(
            created["aliasArn"].as_str().unwrap_or_default(),
            expected_alias_arn(SIGNING_REGION, &alias_name),
            "case {case:?}: unexpected aliasArn, got {created}"
        );
        // `createdAt` targets `TimestampFormat`, which carries
        // `@timestampFormat("date-time")`, so the wire form is an RFC 3339 string
        // (`tests/all/date_time_wire_format.rs` pins that for every such member).
        assert!(
            ::aws_smithy_types::DateTime::from_str(
                created["createdAt"].as_str().unwrap_or_default(),
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {case:?}: createdAt must be a date-time string, got {created}"
        );
        let created_at = created["createdAt"].as_str().unwrap_or_default().to_string();

        // The reader reports exactly what was persisted, plus the state.
        let read = ok(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        assert_eq!(
            (
                read["aliasName"].as_str().unwrap_or_default(),
                read["policyStoreId"].as_str().unwrap_or_default(),
                read["aliasArn"].as_str().unwrap_or_default(),
                read["createdAt"].as_str().unwrap_or_default(),
                read["state"].as_str().unwrap_or_default(),
            ),
            (
                alias_name.as_str(),
                store_id.as_str(),
                expected_alias_arn(SIGNING_REGION, &alias_name).as_str(),
                created_at.as_str(),
                "Active",
            ),
            "case {case:?}: GetPolicyStoreAlias must report the created alias, got {read}"
        );

        // And the alias is usable wherever a `policyStoreId` is accepted.
        let resolved = ok(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": alias_name }),
        )
        .await;
        assert_eq!(
            resolved["policyStoreId"].as_str().unwrap_or_default(),
            store_id,
            "case {case:?}: the alias must resolve to its store, got {resolved}"
        );
    }
}

/// A name one character past `Alias`'s published bound is refused by the
/// boundary validator, so nothing is persisted under it.
#[tokio::test]
async fn an_alias_name_past_the_published_bound_is_refused() {
    let router = router();
    let store_id = store(&router).await;
    let too_long = format!(
        "{ALIAS_PREFIX}{}",
        "x".repeat(ALIAS_MAX_LEN - ALIAS_PREFIX.len() + 1)
    );

    let (status, body) = call(
        &router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": too_long, "policyStoreId": store_id }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the length bound\": expected 400, got {status} {body}"
    );

    let (status, body) = call(
        &router,
        "GetPolicyStoreAlias",
        &json!({ "aliasName": too_long }),
    )
    .await;
    assert!(
        status != 200,
        "case \"one past the length bound\": nothing may have been persisted, got {status} {body}"
    );
}

// ---------------------------------------------------------------------------
// Uniqueness and idempotence
// ---------------------------------------------------------------------------

/// Creating the same `(aliasName, policyStoreId)` pair again is the documented
/// no-op — the same persisted alias, timestamp included — while the same name
/// against a different store conflicts and leaves the first association alone.
///
/// The rows differ along the alias name and along how many times the duplicate is
/// repeated.
#[tokio::test]
async fn creating_an_alias_is_idempotent_on_its_name_and_store() {
    let cases: &[(&str, &str, usize)] = &[
        ("single repeat", "alpha", 1),
        ("repeated three times", "beta/gamma", 3),
        ("repeated with digits in the name", "d4ta_store", 2),
    ];

    let router = router();
    for (case, suffix, repeats) in cases {
        let alias_name = format!("{ALIAS_PREFIX}{suffix}");
        let first_store = store(&router).await;
        let other_store = store(&router).await;

        let first = create_alias(&router, &alias_name, &first_store).await;
        for repeat in 0..*repeats {
            let again = create_alias(&router, &alias_name, &first_store).await;
            assert_eq!(
                (
                    again["aliasArn"].as_str().unwrap_or_default(),
                    again["createdAt"].as_str().unwrap_or_default(),
                    again["policyStoreId"].as_str().unwrap_or_default(),
                ),
                (
                    first["aliasArn"].as_str().unwrap_or_default(),
                    first["createdAt"].as_str().unwrap_or_default(),
                    first["policyStoreId"].as_str().unwrap_or_default(),
                ),
                "case {case:?} repeat {repeat}: a duplicate must answer with the persisted alias, got {again}"
            );
        }

        // The same name for another store is a conflict, and changes nothing.
        let (status, body) = call(
            &router,
            "CreatePolicyStoreAlias",
            &json!({ "aliasName": alias_name, "policyStoreId": other_store }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: retargeting a live alias must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: unexpected error code, got {body}"
        );
        let read = ok(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        assert_eq!(
            read["policyStoreId"].as_str().unwrap_or_default(),
            first_store,
            "case {case:?}: the refused request must not have retargeted the alias, got {read}"
        );
    }
}

/// Concurrent creations of ONE name against DIFFERENT stores have exactly one
/// winner, and the winner is the association the reader then reports.
///
/// The rows differ along how many requests race for the name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creations_of_one_name_have_exactly_one_winner() {
    for racers in [2usize, 3, 8] {
        let case = format!("{racers} racers");
        let router = router();
        let alias_name = format!("{ALIAS_PREFIX}contested");
        let mut stores = Vec::new();
        for _ in 0..racers {
            stores.push(store(&router).await);
        }

        let mut tasks = Vec::new();
        for store_id in &stores {
            let router = Arc::clone(&router);
            let alias_name = alias_name.clone();
            let store_id = store_id.clone();
            tasks.push(tokio::spawn(async move {
                let (status, body) = call(
                    &router,
                    "CreatePolicyStoreAlias",
                    &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
                )
                .await;
                (store_id, status, body)
            }));
        }

        let mut winners = Vec::new();
        let mut losers = Vec::new();
        for task in tasks {
            let (store_id, status, body) = task.await.expect("the racing task must not panic");
            if status == 200 {
                winners.push((store_id, body));
            } else {
                losers.push((store_id, status, body));
            }
        }

        assert_eq!(
            winners.len(),
            1,
            "case {case:?}: expected exactly 1 winner, got winners={winners:?} losers={losers:?}"
        );
        for (store_id, status, body) in &losers {
            assert_eq!(
                *status, 409,
                "case {case:?}: loser {store_id} must be refused with 409, got {status} {body}"
            );
            assert_eq!(
                error_code(body),
                "ConflictException",
                "case {case:?}: loser {store_id}: unexpected error code, got {body}"
            );
        }

        let read = ok(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        assert_eq!(
            read["policyStoreId"].as_str().unwrap_or_default(),
            winners[0].0,
            "case {case:?}: the reader must report the winner's store, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// Deletion
// ---------------------------------------------------------------------------

/// A soft delete moves the alias to `PendingDeletion`: it is still reported by
/// `GetPolicyStoreAlias`, it no longer resolves as a `policyStoreId`, and its
/// name cannot be taken. A hard delete removes it and frees its name.
///
/// The rows differ along the `deletionMode` the soft delete is asked for —
/// absent (the documented default) or named explicitly — and along which store
/// the blocked re-creation names.
#[tokio::test]
async fn soft_delete_pends_the_alias_and_hard_delete_removes_it() {
    // `deletionMode` for the first delete, and whether the blocked re-creation
    // names the same store as the pending alias or a different one.
    let cases: &[(&str, Option<&str>, bool)] = &[
        ("default deletion mode, same store", None, true),
        ("default deletion mode, other store", None, false),
        ("explicit SoftDelete, same store", Some("SoftDelete"), true),
        (
            "explicit SoftDelete, other store",
            Some("SoftDelete"),
            false,
        ),
    ];

    for (case, deletion_mode, same_store) in cases {
        let router = router();
        let alias_name = format!("{ALIAS_PREFIX}lifecycle");
        let first_store = store(&router).await;
        let other_store = store(&router).await;
        let created = create_alias(&router, &alias_name, &first_store).await;
        let created_at = created["createdAt"].as_str().unwrap_or_default().to_string();

        let mut delete = json!({ "aliasName": alias_name });
        if let Some(mode) = deletion_mode {
            delete["deletionMode"] = json!(mode);
        }
        ok(&router, "DeletePolicyStoreAlias", &delete).await;

        // Still reported, now as PendingDeletion, with everything else intact.
        let pending = ok(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        assert_eq!(
            (
                pending["state"].as_str().unwrap_or_default(),
                pending["policyStoreId"].as_str().unwrap_or_default(),
                pending["createdAt"].as_str().unwrap_or_default(),
            ),
            ("PendingDeletion", first_store.as_str(), created_at.as_str()),
            "case {case:?}: a soft-deleted alias must report PendingDeletion, got {pending}"
        );

        // It no longer resolves as a `policyStoreId`.
        let (status, body) = call(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": alias_name }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a pending alias must not resolve, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code for a pending alias, got {body}"
        );

        // And its name cannot be taken while it pends.
        let target = if *same_store {
            first_store.clone()
        } else {
            other_store.clone()
        };
        let (status, body) = call(
            &router,
            "CreatePolicyStoreAlias",
            &json!({ "aliasName": alias_name, "policyStoreId": target }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a pending name must not be re-created, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: unexpected error code for a pending name, got {body}"
        );

        // A repeated soft delete is still a success and changes nothing.
        ok(&router, "DeletePolicyStoreAlias", &delete).await;
        let still_pending = ok(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        assert_eq!(
            still_pending["state"].as_str().unwrap_or_default(),
            "PendingDeletion",
            "case {case:?}: a repeated soft delete must not change the state, got {still_pending}"
        );

        // The hard delete removes the record, and the name is free again.
        ok(
            &router,
            "DeletePolicyStoreAlias",
            &json!({ "aliasName": alias_name, "deletionMode": "HardDelete" }),
        )
        .await;
        let (status, body) = call(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a hard-deleted alias must be gone, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code after a hard delete, got {body}"
        );

        let recreated = create_alias(&router, &alias_name, &target).await;
        assert_eq!(
            recreated["policyStoreId"].as_str().unwrap_or_default(),
            target,
            "case {case:?}: the freed name must be re-usable, got {recreated}"
        );
        assert!(
            date_time_secs(&recreated["createdAt"]) >= date_time_secs(&created["createdAt"]),
            "case {case:?}: the re-created alias must carry its own timestamp, got {recreated}"
        );
    }
}

/// Deleting an alias that does not exist is a success in either mode, and a hard
/// delete straight from `Active` skips `PendingDeletion` entirely.
#[tokio::test]
async fn deleting_an_absent_alias_succeeds_and_a_hard_delete_skips_pending() {
    let router = router();
    let store_id = store(&router).await;

    for mode in [None, Some("SoftDelete"), Some("HardDelete")] {
        let case = format!("absent alias, deletionMode={mode:?}");
        let mut request = json!({ "aliasName": format!("{ALIAS_PREFIX}never-created") });
        if let Some(mode) = mode {
            request["deletionMode"] = json!(mode);
        }
        let (status, body) = call(&router, "DeletePolicyStoreAlias", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting an absent alias must succeed, got {status} {body}"
        );
    }

    let alias_name = format!("{ALIAS_PREFIX}straight-to-gone");
    create_alias(&router, &alias_name, &store_id).await;
    ok(
        &router,
        "DeletePolicyStoreAlias",
        &json!({ "aliasName": alias_name, "deletionMode": "HardDelete" }),
    )
    .await;
    let (status, body) = call(
        &router,
        "GetPolicyStoreAlias",
        &json!({ "aliasName": alias_name }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"hard delete from Active\": the alias must be gone, got {status} {body}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// What `CreatePolicyStoreAlias` refuses: a name without the documented prefix,
/// and a `policyStoreId` that names no store — including one given as an alias
/// reference, which the model says "cannot be used" for this member.
#[tokio::test]
async fn creation_refuses_a_bad_name_or_an_unknown_store() {
    let router = router();
    let store_id = store(&router).await;
    let live_alias = format!("{ALIAS_PREFIX}live");
    create_alias(&router, &live_alias, &store_id).await;

    // Rows differ along HOW the prefix is missing: absent, truncated, a
    // look-alike, and a name that merely contains it.
    let bad_names: &[(&str, String)] = &[
        ("empty name", String::new()),
        ("no prefix at all", "photos".to_string()),
        ("prefix without its separator", "policy-store-alias".to_string()),
        ("look-alike prefix", "policy-store-aliases/photos".to_string()),
        ("prefix not at the start", format!("x/{ALIAS_PREFIX}photos")),
    ];
    for (case, alias_name) in bad_names {
        let (status, body) = call(
            &router,
            "CreatePolicyStoreAlias",
            &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }

    // Rows differ along WHAT the policyStoreId names: nothing, a deleted store,
    // and an alias of a store that does exist.
    let deleted_store = store(&router).await;
    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": deleted_store }),
    )
    .await;
    let bad_stores: &[(&str, String)] = &[
        ("unknown store id", "PSnope".to_string()),
        ("a store that was deleted", deleted_store),
        ("an alias reference, which this member refuses", live_alias),
    ];
    for (case, policy_store_id) in bad_stores {
        let (status, body) = call(
            &router,
            "CreatePolicyStoreAlias",
            &json!({
                "aliasName": format!("{ALIAS_PREFIX}for-{}", case.len()),
                "policyStoreId": policy_store_id,
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }
}

/// The alias quota bounds how many aliases exist, so the alias table cannot grow
/// without limit: creation succeeds up to it and is refused with the modeled
/// quota error past it.
#[tokio::test]
async fn the_alias_quota_bounds_how_many_aliases_exist() {
    let router = router();
    let store_id = store(&router).await;

    for n in 0..MAX_POLICY_STORE_ALIASES {
        let (status, body) = call(
            &router,
            "CreatePolicyStoreAlias",
            &json!({
                "aliasName": format!("{ALIAS_PREFIX}bulk-{n}"),
                "policyStoreId": store_id,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"alias {n} of {MAX_POLICY_STORE_ALIASES}\": creation inside the quota must succeed, got {status} {body}"
        );
    }

    let (status, body) = call(
        &router,
        "CreatePolicyStoreAlias",
        &json!({
            "aliasName": format!("{ALIAS_PREFIX}one-too-many"),
            "policyStoreId": store_id,
        }),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"one alias past the quota\": expected 402, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case \"one alias past the quota\": unexpected error code, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Resolution wherever a policyStoreId is accepted
// ---------------------------------------------------------------------------

/// An `Active` alias stands in for its store's ID in every operation that takes
/// a `policyStoreId`, and stops standing in for it the moment it is soft-deleted.
///
/// The rows are the operations themselves, so the rule is asserted across the
/// set rather than on one of them.
#[tokio::test]
async fn an_active_alias_stands_in_for_its_store_id() {
    let router = router();
    let store_id = store(&router).await;
    let alias_name = format!("{ALIAS_PREFIX}resolves");
    create_alias(&router, &alias_name, &store_id).await;

    // A policy created THROUGH the alias lands in the aliased store.
    let created = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": alias_name,
            "definition": { "static": { "statement":
                "permit(principal == User::\"alice\", action == Action::\"view\", resource == Photo::\"vacation\");" } },
        }),
    )
    .await;
    assert_eq!(
        created["policyStoreId"].as_str().unwrap_or_default(),
        store_id,
        "the policy must be created in the aliased store, got {created}"
    );
    let policy_id = created["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();

    let authorize = json!({
        "policyStoreId": alias_name,
        "principal": { "entityType": "User", "entityId": "alice" },
        "action": { "actionType": "Action", "actionId": "view" },
        "resource": { "entityType": "Photo", "entityId": "vacation" },
    });
    let reads: &[(&str, Value)] = &[
        ("GetPolicyStore", json!({ "policyStoreId": alias_name })),
        (
            "GetPolicy",
            json!({ "policyStoreId": alias_name, "policyId": policy_id }),
        ),
        ("ListPolicies", json!({ "policyStoreId": alias_name })),
        ("IsAuthorized", authorize.clone()),
    ];
    for (operation, request) in reads {
        let answer = ok(&router, operation, request).await;
        match *operation {
            "GetPolicyStore" => assert_eq!(
                answer["policyStoreId"].as_str().unwrap_or_default(),
                store_id,
                "case {operation:?}: the alias must resolve to its store, got {answer}"
            ),
            "GetPolicy" => assert_eq!(
                answer["policyId"].as_str().unwrap_or_default(),
                policy_id,
                "case {operation:?}: the alias must reach the store's policy, got {answer}"
            ),
            "ListPolicies" => assert_eq!(
                answer["policies"].as_array().map(Vec::len),
                Some(1),
                "case {operation:?}: the alias must list the store's policies, got {answer}"
            ),
            _ => assert_eq!(
                answer["decision"].as_str().unwrap_or_default(),
                "ALLOW",
                "case {operation:?}: the aliased store's permit must allow, got {answer}"
            ),
        }
    }

    // Soft-deleted, the same references stop resolving — the model's
    // "operation will fail with a ResourceNotFound exception".
    ok(
        &router,
        "DeletePolicyStoreAlias",
        &json!({ "aliasName": alias_name }),
    )
    .await;
    for (operation, request) in reads {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 404,
            "case {operation:?} after a soft delete: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {operation:?} after a soft delete: unexpected error code, got {body}"
        );
    }

    // The store itself is untouched: only the reference stopped working.
    let direct = ok(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": policy_id }),
    )
    .await;
    assert_eq!(
        direct["policyId"].as_str().unwrap_or_default(),
        policy_id,
        "the store's policy must survive deleting an alias of the store, got {direct}"
    );
}

/// Deleting the policy store removes the aliases that named it, so a later
/// create may take the name again.
#[tokio::test]
async fn deleting_a_store_removes_its_aliases() {
    let router = router();
    let store_id = store(&router).await;
    let other_store = store(&router).await;
    let alias_name = format!("{ALIAS_PREFIX}goes-with-the-store");
    create_alias(&router, &alias_name, &store_id).await;

    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": store_id }),
    )
    .await;

    let (status, body) = call(
        &router,
        "GetPolicyStoreAlias",
        &json!({ "aliasName": alias_name }),
    )
    .await;
    assert_eq!(
        status, 404,
        "an alias of a deleted store must be gone, got {status} {body}"
    );

    let recreated = create_alias(&router, &alias_name, &other_store).await;
    assert_eq!(
        recreated["policyStoreId"].as_str().unwrap_or_default(),
        other_store,
        "the freed name must be usable for another store, got {recreated}"
    );
}
