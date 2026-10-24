// SPDX-License-Identifier: CC0-1.0

//! `DeleteAccessKey`, end to end over the awsQuery wire.
//!
//! RULE: `DeleteAccessKey` removes the access key the request names from the IAM
//! user that holds it — the user `UserName` names, or, with no `UserName`, the user
//! the signing access key id resolves to — so nothing resolves a caller through
//! that id afterwards and the user's quota frees up; an id that user holds none of
//! (never minted, already deleted, or held by another user) is `NoSuchEntity` and
//! removes nothing.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! owning user's name alphabet and case, the case the request spells that name in,
//! which of the user's keys is deleted, whether the request names a user at all,
//! and which credentials the request is presented as signed by. The awsQuery
//! harness (router, call, element) is the one `tests/all/user_lifecycle.rs` owns.

use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// One deletion: the user the account holds, and the case the request spells that
/// user's name in.
///
/// The two differ on most rows because IAM entity names "are not distinguished by
/// case" (`CreateUser$UserName`), so a request may name the owning user in any case.
struct Case {
    /// The name the user is created with — the case IAM stores and reports.
    stored_name: &'static str,
    /// The name `DeleteAccessKey` spells; the same entity, differing only in case.
    requested_name: &'static str,
}

/// Rows spanning the whole `^[\w+=,.@-]+$` alphabet the name constraint admits
/// (`smithy_query(query="operation", operation="DeleteAccessKey")`), both cases of
/// the stored name, and requests that fold the name up, down and not at all.
const CASES: &[Case] = &[
    Case {
        stored_name: "alice",
        requested_name: "alice",
    },
    Case {
        stored_name: "Bob",
        requested_name: "bob",
    },
    Case {
        stored_name: "carol_7",
        requested_name: "CAROL_7",
    },
    Case {
        stored_name: "dave.x",
        requested_name: "dave.X",
    },
    Case {
        stored_name: "Eve+1",
        requested_name: "eve+1",
    },
    Case {
        stored_name: "frank=2",
        requested_name: "frank=2",
    },
    Case {
        stored_name: "grace@3",
        requested_name: "GRACE@3",
    },
    Case {
        stored_name: "heidi-4",
        requested_name: "heidi-4",
    },
];

/// How many access keys one IAM user may hold, from AWS's "IAM and STS quotas"
/// page ("Access keys per IAM user: 2") — the quota a delete frees room under.
///
/// Restated here as the published number rather than imported from the crate's own
/// `state::MAX_ACCESS_KEYS_PER_USER`, which would make the check unfalsifiable.
const MAX_ACCESS_KEYS_PER_USER: usize = 2;

/// Create `user` and mint `count` access keys for it, answering their ids.
async fn user_with_keys(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    user: &str,
    count: usize,
) -> Vec<String> {
    let (status, body) = call(router, "CreateUser", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateUser must succeed, got {status}: {body}"
    );
    let mut ids = Vec::new();
    for index in 0..count {
        let (status, created) = call(router, "CreateAccessKey", &[("UserName", user)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: key {index} must be minted, got {status}: {created}"
        );
        ids.push(element(&created, "AccessKeyId"));
    }
    ids
}

/// Whether the key `access_key_id` still resolves a nameless caller to an IAM user:
/// `GetUser` signed by the key is the read that follows the binding
/// `CreateAccessKey` persisted, so it answers 200 while the key exists and refuses
/// once it is gone.
async fn key_resolves_to_a_user(
    router: &smithy_server_runtime::MultiServiceRouter,
    access_key_id: &str,
) -> (u16, String) {
    call_signed_by(router, Some(access_key_id), "GetUser", &[]).await
}

/// RULE: deleting a key the named user holds succeeds, the id stops resolving to
/// that user, the user's OTHER keys are untouched, and the user itself stays.
#[tokio::test]
async fn deleting_a_key_removes_only_that_key() {
    let router = router();

    for Case {
        stored_name,
        requested_name,
    } in CASES
    {
        let case = format!("{stored_name} requested as {requested_name}");
        let held = user_with_keys(&router, &case, stored_name, MAX_ACCESS_KEYS_PER_USER).await;

        // Delete each key in turn, so the row covers the first key of a full set and
        // the last key the user holds.
        for (index, doomed) in held.iter().enumerate() {
            let (status, body) = key_resolves_to_a_user(&router, doomed).await;
            assert_eq!(
                (status, element(&body, "UserName")),
                (200, stored_name.to_string()),
                "case {case:?}: key {index} must resolve before the delete, got {status}: {body}"
            );

            let (status, deleted) = call(
                &router,
                "DeleteAccessKey",
                &[("UserName", requested_name), ("AccessKeyId", doomed)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected key {index} to delete, got {status}: {deleted}"
            );

            // The binding is gone: a nameless call signed by the deleted id resolves
            // to no IAM user and is refused, naming no user at all.
            let (status, refused) = key_resolves_to_a_user(&router, doomed).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the deleted key {index} to resolve to nobody, got {status}: {refused}"
            );
            assert!(
                elements(&refused, "UserName").is_empty(),
                "case {case:?}: expected the deleted key {index} to name no user, got {refused}"
            );

            // Every key the user still holds resolves exactly as before.
            for (other, survivor) in held.iter().enumerate().skip(index + 1) {
                let (status, body) = key_resolves_to_a_user(&router, survivor).await;
                assert_eq!(
                    (status, element(&body, "UserName")),
                    (200, stored_name.to_string()),
                    "case {case:?}: expected key {other} to survive the delete of key {index}, got {status}: {body}"
                );
            }

            // The user is not deleted with its credential.
            let (status, user) = call(&router, "GetUser", &[("UserName", requested_name)]).await;
            assert_eq!(
                (status, element(&user, "UserName")),
                (200, stored_name.to_string()),
                "case {case:?}: expected the user to outlive key {index}, got {status}: {user}"
            );
        }
    }
}

/// RULE: the deleted key is really gone from the user's set, so the room it took
/// under the access-keys-per-user quota is free again, and a user whose last key is
/// deleted has no credential dependent left to refuse its own delete.
#[tokio::test]
async fn a_deleted_key_frees_the_quota_and_the_user() {
    let router = router();

    for Case {
        stored_name,
        requested_name,
    } in CASES
    {
        let case = format!("{stored_name} requested as {requested_name}");
        let held = user_with_keys(&router, &case, stored_name, MAX_ACCESS_KEYS_PER_USER).await;

        // At the quota, a create is refused.
        let (status, refused) = call(&router, "CreateAccessKey", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the create at the quota to be refused, got {status}: {refused}"
        );

        // Deleting one key frees exactly one slot: the next create succeeds and the
        // one after it is refused again.
        let (status, deleted) = call(
            &router,
            "DeleteAccessKey",
            &[("UserName", requested_name), ("AccessKeyId", &held[0])],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to succeed, got {status}: {deleted}"
        );
        let (status, created) = call(&router, "CreateAccessKey", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the freed slot to admit a create, got {status}: {created}"
        );
        let replacement = element(&created, "AccessKeyId");
        assert!(
            !held.contains(&replacement),
            "case {case:?}: expected a new credential, got the deleted id {replacement:?} back"
        );
        let (status, refused) = call(&router, "CreateAccessKey", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the quota to bind again, got {status}: {refused}"
        );

        // With every key deleted the user has no credential dependent left, so the
        // delete `tests/all/access_keys.rs` sees refused now succeeds.
        for (index, doomed) in held[1..].iter().chain(std::iter::once(&replacement)).enumerate() {
            let (status, body) = call(
                &router,
                "DeleteAccessKey",
                &[("UserName", requested_name), ("AccessKeyId", doomed)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected remaining key {index} to delete, got {status}: {body}"
            );
        }
        let (status, body) = call(&router, "DeleteUser", &[("UserName", requested_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the keyless user to delete, got {status}: {body}"
        );
    }
}

/// RULE: a key id the named user holds none of is `NoSuchEntity` (HTTP 404) and
/// removes nothing — whether the account never minted it, it was already deleted,
/// or another user holds it.
#[tokio::test]
async fn deleting_a_key_the_user_does_not_hold_is_not_found() {
    let router = router();

    for Case {
        stored_name,
        requested_name,
    } in CASES
    {
        let case = format!("{stored_name} requested as {requested_name}");
        let own = user_with_keys(&router, &case, stored_name, 1).await;
        // A second user, so a row covers an id that exists but belongs elsewhere.
        let other_name = format!("other_{stored_name}");
        let other = user_with_keys(&router, &case, &other_name, 1).await;

        let (status, deleted) = call(
            &router,
            "DeleteAccessKey",
            &[("UserName", requested_name), ("AccessKeyId", &own[0])],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first delete must succeed, got {status}: {deleted}"
        );

        // Rows: the id just deleted (a repeat delete), an id of the modelled shape
        // this emulator never issued, an id at each bound `accessKeyIdType` admits
        // (length 16 and 128), and an id another user holds.
        let absent: Vec<String> = vec![
            own[0].clone(),
            "AKIAIOSFODNN7EXAMPLE".to_string(),
            "A".repeat(16),
            "B".repeat(128),
            other[0].clone(),
        ];

        for access_key_id in &absent {
            let (status, refused) = call(
                &router,
                "DeleteAccessKey",
                &[
                    ("UserName", requested_name),
                    ("AccessKeyId", access_key_id),
                ],
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for {access_key_id:?}, got {status}: {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity for {access_key_id:?}, got {:?}",
                error_code(&refused)
            );
        }

        // The other user's key was never touched by any of those refusals.
        let (status, body) = key_resolves_to_a_user(&router, &other[0]).await;
        assert_eq!(
            (status, element(&body, "UserName")),
            (200, other_name.clone()),
            "case {case:?}: expected the other user's key to survive, got {status}: {body}"
        );
    }
}

/// RULE: a key can only be deleted from a user the account holds, so a request
/// naming an absent entity is `NoSuchEntity` and removes nothing.
#[tokio::test]
async fn deleting_from_an_absent_user_is_not_found() {
    let router = router();
    let held = user_with_keys(&router, "present", "present", 1).await;

    // Rows spanning a plain absent name, one differing from a held name by more than
    // case, names drawn from the rest of the accepted alphabet, and a name at the
    // 128-character bound `existingUserNameType` admits.
    let absent: Vec<String> = vec![
        "absent".to_string(),
        "present2".to_string(),
        "no.such@user".to_string(),
        "NOT_HELD+1".to_string(),
        "x".repeat(128),
    ];

    for name in &absent {
        let (status, refused) = call(
            &router,
            "DeleteAccessKey",
            &[("UserName", name), ("AccessKeyId", &held[0])],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {:?}",
            error_code(&refused)
        );
    }

    // The real owner's key is untouched by every refusal above.
    let (status, body) = key_resolves_to_a_user(&router, &held[0]).await;
    assert_eq!(
        (status, element(&body, "UserName")),
        (200, "present".to_string()),
        "expected the held key to survive the refusals, got {status}: {body}"
    );
}

/// RULE: with no `UserName` the key is deleted from the user the signing access key
/// id resolves to — including the signing key itself — and a key another user holds
/// is still not that caller's to delete.
#[tokio::test]
async fn an_omitted_name_deletes_from_the_signing_user() {
    let router = router();

    for Case { stored_name, .. } in CASES {
        let case = stored_name.to_string();
        let held = user_with_keys(&router, &case, stored_name, MAX_ACCESS_KEYS_PER_USER).await;
        let other_name = format!("bystander_{stored_name}");
        let other = user_with_keys(&router, &case, &other_name, 1).await;

        // A key of another user is not reachable from these credentials.
        let (status, refused) = call_signed_by(
            &router,
            Some(&held[0]),
            "DeleteAccessKey",
            &[("AccessKeyId", &other[0])],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected another user's key to be out of reach, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "NoSuchEntity",
            "case {case:?}: expected NoSuchEntity, got {:?}",
            error_code(&refused)
        );

        // The signing user's OTHER key deletes with no name given.
        let (status, deleted) = call_signed_by(
            &router,
            Some(&held[0]),
            "DeleteAccessKey",
            &[("AccessKeyId", &held[1])],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the signing user's other key to delete, got {status}: {deleted}"
        );
        let (status, gone) = key_resolves_to_a_user(&router, &held[1]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted key to resolve to nobody, got {status}: {gone}"
        );

        // And so does the signing key itself: the caller may delete the credential it
        // is using, after which that credential resolves to nobody.
        let (status, deleted) = call_signed_by(
            &router,
            Some(&held[0]),
            "DeleteAccessKey",
            &[("AccessKeyId", &held[0])],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the signing key to delete itself, got {status}: {deleted}"
        );
        let (status, gone) = key_resolves_to_a_user(&router, &held[0]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the signing key to resolve to nobody, got {status}: {gone}"
        );

        // The bystander's key, which no successful call named, still resolves.
        let (status, body) = key_resolves_to_a_user(&router, &other[0]).await;
        assert_eq!(
            (status, element(&body, "UserName")),
            (200, other_name.clone()),
            "case {case:?}: expected the bystander's key to survive, got {status}: {body}"
        );
    }
}

/// RULE: credentials that belong to no IAM user resolve to no user, so a request
/// that names none is refused rather than having an identity invented for it, and
/// nothing is deleted.
#[tokio::test]
async fn an_omitted_name_refuses_non_user_credentials() {
    let router = router();
    let held = user_with_keys(&router, "keyholder", "keyholder", 1).await;

    // Rows spanning no signing key at all, the fallback id an unsigned request
    // resolves to (`emulator_server::extract_akid` answers "default"), a key id of
    // the modelled shape this emulator never issued, and an id that names a user
    // rather than a key.
    let signers: &[Option<&str>] = &[
        None,
        Some("default"),
        Some("AKIAIOSFODNN7EXAMPLE"),
        Some("EMULATORAKAAAAAAAAAA"),
        Some("keyholder"),
    ];

    for signer in signers {
        let case = format!("signed by {signer:?}");
        let (status, refused) = call_signed_by(
            &router,
            *signer,
            "DeleteAccessKey",
            &[("AccessKeyId", &held[0])],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {refused}"
        );
        // The modelled behaviour under test is the REFUSAL: a 400 that deletes
        // nothing. The code is the framework's single client-input refusal shape,
        // because `DeleteAccessKey` models no error for a request that named no
        // entity; only the status and the key's survival are claims about IAM.
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected the framework's client-input refusal code, got {:?}",
            error_code(&refused)
        );
    }

    // Every refusal above left the key where it was.
    let (status, body) = key_resolves_to_a_user(&router, &held[0]).await;
    assert_eq!(
        (status, element(&body, "UserName")),
        (200, "keyholder".to_string()),
        "expected the key to survive the refusals, got {status}: {body}"
    );
}
