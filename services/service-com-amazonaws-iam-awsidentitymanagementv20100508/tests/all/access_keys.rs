// SPDX-License-Identifier: CC0-1.0

//! IAM access keys, end to end over the awsQuery wire.
//!
//! `CreateAccessKey` mints a long-term credential for an IAM user and persists the
//! binding from the key id to that user; `GetUser` with no `UserName` is the read
//! that resolves a caller through that binding, so it is what reads the created
//! key back. The awsQuery harness (router, call, element) is the one
//! `tests/all/user_lifecycle.rs` owns, so every suite in this crate builds and
//! reads a request the same way.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over:
//! the owning user's name alphabet and case, the case the request spells that name
//! in, how many keys the user already holds, whether the request names a user at
//! all, and which credentials the request is presented as signed by.

use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// One create: the user the account holds, and the case the request spells that
/// user's name in.
///
/// The two differ on most rows because IAM entity names "are not distinguished by
/// case" (`CreateUser$UserName`), so a request may name the user in any case while
/// the response must report the name as the account holds it.
struct Case {
    /// The name the user is created with — the case IAM stores and reports.
    stored_name: &'static str,
    /// The name `CreateAccessKey` spells; the same entity, differing only in case.
    requested_name: &'static str,
}

/// Rows spanning the whole `^[\w+=,.@-]+$` alphabet the name constraint admits
/// (`smithy_query(query="operation", operation="CreateAccessKey")`), both cases of
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
/// page ("Access keys per IAM user: 2") — the quota the service enforces.
///
/// Restated here as the published number rather than imported from the crate's
/// own `state::MAX_ACCESS_KEYS_PER_USER`: importing it would make the check
/// unfalsifiable, since the test would then follow the code wherever it went.
const MAX_ACCESS_KEYS_PER_USER: usize = 2;

/// Assert that `access_key_id` satisfies `accessKeyIdType`: length 16..128 and the
/// pattern `^[\w]+$` (`smithy_query(query="shape", shape="accessKeyIdType")`).
fn assert_modelled_access_key_id(case: &str, access_key_id: &str) {
    let length = access_key_id.chars().count();
    assert!(
        (16..=128).contains(&length),
        "case {case:?}: expected an AccessKeyId of 16..128 characters, got {access_key_id:?}"
    );
    assert!(
        access_key_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "case {case:?}: expected an AccessKeyId matching ^[\\w]+$, got {access_key_id:?}"
    );
}

/// RULE: `CreateAccessKey` mints a credential for the IAM user the request names,
/// returning every required member of the `AccessKey` shape — the stored user's
/// name, an id inside `accessKeyIdType`, the status `Active`, a secret, and the
/// instant it was created — and PERSISTS the binding from that id to the user, so
/// a later request signed by the key resolves to that same user.
#[tokio::test]
async fn create_access_key_mints_a_credential_bound_to_the_named_user() {
    let router = router();
    let mut minted_ids: Vec<String> = Vec::new();
    let mut minted_secrets: Vec<String> = Vec::new();

    for Case {
        stored_name,
        requested_name,
    } in CASES
    {
        let case = format!("{stored_name} requested as {requested_name}");
        let (status, body) = call(&router, "CreateUser", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateUser must succeed, got {status}: {body}"
        );

        let (status, created) =
            call(&router, "CreateAccessKey", &[("UserName", requested_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {created}"
        );

        assert_eq!(
            element(&created, "UserName"),
            *stored_name,
            "case {case:?}: expected the key to report the stored user name, got {:?}",
            element(&created, "UserName")
        );
        assert_eq!(
            element(&created, "Status"),
            "Active",
            "case {case:?}: expected a new key to be Active, got {:?}",
            element(&created, "Status")
        );

        let access_key_id = element(&created, "AccessKeyId");
        assert_modelled_access_key_id(&case, &access_key_id);
        assert!(
            !minted_ids.contains(&access_key_id),
            "case {case:?}: expected an AccessKeyId unique across keys, got the repeat {access_key_id:?}"
        );

        // `AccessKey$SecretAccessKey` is the signing secret, so it is a value of
        // its own: never the id, never empty, and never repeated.
        let secret = element(&created, "SecretAccessKey");
        assert!(
            !secret.is_empty() && secret != access_key_id,
            "case {case:?}: expected a secret distinct from the id, got {secret:?}"
        );
        assert!(
            !minted_secrets.contains(&secret),
            "case {case:?}: expected a SecretAccessKey unique across keys, got a repeat"
        );

        // `AccessKey$CreateDate` is a timestamp, rendered by awsQuery in date-time
        // form.
        let create_date = element(&created, "CreateDate");
        assert!(
            aws_smithy_types::DateTime::from_str(
                &create_date,
                aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {case:?}: expected an ISO-8601 CreateDate, got {create_date:?}"
        );

        // The binding was persisted: `GetUser` with no `UserName` resolves the
        // caller from the signing key id, and answers with this user.
        let (status, fetched) =
            call_signed_by(&router, Some(&access_key_id), "GetUser", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the minted key to resolve to its user, got {status}: {fetched}"
        );
        assert_eq!(
            element(&fetched, "UserName"),
            *stored_name,
            "case {case:?}: expected the key to be bound to the stored user, got {:?}",
            element(&fetched, "UserName")
        );

        // Reading through the key again cannot change what the next caller sees.
        let (status, again) = call_signed_by(&router, Some(&access_key_id), "GetUser", &[]).await;
        assert_eq!(
            (status, element(&again, "UserName")),
            (200, stored_name.to_string()),
            "case {case:?}: a second read through the key answered differently: {again}"
        );

        minted_ids.push(access_key_id);
        minted_secrets.push(secret);
    }
}

/// RULE: a user may hold up to the published quota of access keys, each a distinct
/// credential; a create past the quota is refused with `LimitExceeded` and mints
/// nothing, and the case the request spells the user's name in does not open a
/// second allowance.
#[tokio::test]
async fn creating_past_the_access_key_quota_is_refused() {
    let router = router();

    for Case {
        stored_name,
        requested_name,
    } in CASES
    {
        let case = format!("{stored_name} requested as {requested_name}");
        let (status, body) = call(&router, "CreateUser", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateUser must succeed, got {status}: {body}"
        );

        // Every key up to the quota is minted, each a distinct credential. The
        // request alternates between the stored case and the requested case, so the
        // quota is taken over the ENTITY and not over the spelling.
        let mut held: Vec<String> = Vec::new();
        for index in 0..MAX_ACCESS_KEYS_PER_USER {
            let name = if index.is_multiple_of(2) {
                stored_name
            } else {
                requested_name
            };
            let (status, created) = call(&router, "CreateAccessKey", &[("UserName", name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected key {index} to be minted, got {status}: {created}"
            );
            let access_key_id = element(&created, "AccessKeyId");
            assert!(
                !held.contains(&access_key_id),
                "case {case:?}: expected key {index} to be a new credential, got the repeat {access_key_id:?}"
            );
            held.push(access_key_id);
        }

        let (status, refused) =
            call(&router, "CreateAccessKey", &[("UserName", requested_name)]).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the create past the quota to be refused with 409, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "LimitExceeded",
            "case {case:?}: expected LimitExceeded, got {:?}",
            error_code(&refused)
        );
        assert!(
            elements(&refused, "AccessKeyId").is_empty()
                && elements(&refused, "SecretAccessKey").is_empty(),
            "case {case:?}: expected the refusal to mint nothing, got {refused}"
        );

        // The refusal left the keys the user already held untouched.
        for (index, access_key_id) in held.iter().enumerate() {
            let (status, fetched) = call_signed_by(&router, Some(access_key_id), "GetUser", &[]).await;
            assert_eq!(
                (status, element(&fetched, "UserName")),
                (200, stored_name.to_string()),
                "case {case:?}: expected key {index} to still resolve to its user, got {status}: {fetched}"
            );
        }
    }
}

/// RULE: `CreateAccessKey` with no `UserName` mints the key for the IAM user the
/// request was signed by, resolved from the signing access key id.
#[tokio::test]
async fn an_omitted_name_mints_the_key_for_the_signing_user() {
    let router = router();

    for Case { stored_name, .. } in CASES {
        let (status, body) = call(&router, "CreateUser", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {stored_name:?}: CreateUser must succeed, got {status}: {body}"
        );
        let (status, first) = call(&router, "CreateAccessKey", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {stored_name:?}: the first key must be minted, got {status}: {first}"
        );
        let signing_key = element(&first, "AccessKeyId");

        let (status, second) =
            call_signed_by(&router, Some(&signing_key), "CreateAccessKey", &[]).await;
        assert_eq!(
            status, 200,
            "case {stored_name:?}: expected 200 for the signing user's own key, got {status}: {second}"
        );
        assert_eq!(
            element(&second, "UserName"),
            *stored_name,
            "case {stored_name:?}: expected the key to belong to the signing user, got {:?}",
            element(&second, "UserName")
        );
        let access_key_id = element(&second, "AccessKeyId");
        assert_modelled_access_key_id(stored_name, &access_key_id);
        assert_ne!(
            access_key_id, signing_key,
            "case {stored_name:?}: expected a new credential, got the signing key back"
        );

        // The new key is bound to the same user as the one that signed for it.
        let (status, fetched) = call_signed_by(&router, Some(&access_key_id), "GetUser", &[]).await;
        assert_eq!(
            (status, element(&fetched, "UserName")),
            (200, stored_name.to_string()),
            "case {stored_name:?}: expected the new key to resolve to the signing user, got {status}: {fetched}"
        );
    }
}

/// RULE: credentials that belong to no IAM user resolve to no user, so a request
/// that names none is refused rather than having an identity invented for it.
#[tokio::test]
async fn an_omitted_name_refuses_non_user_credentials() {
    let router = router();
    let (status, body) = call(&router, "CreateUser", &[("UserName", "keyholder")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");

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
        let (status, refused) = call_signed_by(&router, *signer, "CreateAccessKey", &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {refused}"
        );
        // The modelled behaviour under test is the REFUSAL: a 400 carrying no
        // credential. The code is the framework's single client-input refusal
        // shape, because `CreateAccessKey` models no error for a request that named
        // no entity; only the status and the absence of a credential are claims
        // about IAM.
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected the framework's client-input refusal code, got {:?}",
            error_code(&refused)
        );
        assert!(
            elements(&refused, "AccessKeyId").is_empty()
                && elements(&refused, "SecretAccessKey").is_empty(),
            "case {case:?}: expected no credential to be minted, got {refused}"
        );
    }
}

/// RULE: a key can only belong to a user the account holds, so a create naming an
/// absent entity is not found and mints nothing.
#[tokio::test]
async fn creating_a_key_for_an_absent_user_is_not_found() {
    let router = router();
    let (status, body) = call(&router, "CreateUser", &[("UserName", "present")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");

    // Rows spanning a plain absent name, one differing from a held name by more
    // than case, names drawn from the rest of the accepted alphabet, and a name at
    // the 128-character bound `existingUserNameType` admits.
    let absent: Vec<String> = vec![
        "absent".to_string(),
        "present2".to_string(),
        "no.such@user".to_string(),
        "NOT_HELD+1".to_string(),
        "x".repeat(128),
    ];

    for name in &absent {
        let (status, refused) = call(&router, "CreateAccessKey", &[("UserName", name)]).await;
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
        assert!(
            elements(&refused, "AccessKeyId").is_empty()
                && elements(&refused, "SecretAccessKey").is_empty(),
            "case {name:?}: expected no credential to be minted, got {refused}"
        );
    }
}

/// RULE: an access key is a dependent of its user, so IAM refuses to delete a user
/// that still holds one rather than cascade-deleting the credential.
#[tokio::test]
async fn a_user_holding_an_access_key_cannot_be_deleted() {
    let router = router();

    for Case {
        stored_name,
        requested_name,
    } in CASES
    {
        let case = format!("{stored_name} requested as {requested_name}");
        let (status, body) = call(&router, "CreateUser", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateUser must succeed, got {status}: {body}"
        );

        // Before any key exists the user deletes cleanly, so the refusal below is
        // the key's doing and not the user's.
        let (status, deleted) = call(&router, "DeleteUser", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a keyless user to delete, got {status}: {deleted}"
        );

        let (status, body) = call(&router, "CreateUser", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateUser must succeed again, got {status}: {body}"
        );
        let (status, created) =
            call(&router, "CreateAccessKey", &[("UserName", requested_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the key must be minted, got {status}: {created}"
        );

        let (status, refused) = call(&router, "DeleteUser", &[("UserName", stored_name)]).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the delete to be refused with 409, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "DeleteConflict",
            "case {case:?}: expected DeleteConflict, got {:?}",
            error_code(&refused)
        );

        // The user is still there, and so is its key.
        let access_key_id = element(&created, "AccessKeyId");
        let (status, fetched) = call_signed_by(&router, Some(&access_key_id), "GetUser", &[]).await;
        assert_eq!(
            (status, element(&fetched, "UserName")),
            (200, stored_name.to_string()),
            "case {case:?}: expected the refused delete to leave the user and its key, got {status}: {fetched}"
        );
    }
}
