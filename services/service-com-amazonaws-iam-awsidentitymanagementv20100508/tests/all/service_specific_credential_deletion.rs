// SPDX-License-Identifier: CC0-1.0

//! `DeleteServiceSpecificCredential`, end to end over the awsQuery wire.
//!
//! The operation removes ONE stored service-specific credential — the one the id
//! names, among those the resolved user holds — and nothing else: the user's other
//! credentials stay, the record stops being reachable by every later call, and the
//! room it took in the per-service quota is free again.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! service and so the FORM of credential it issues, the case the request spells the
//! user name in, which of several held credentials the request names, whether the
//! request names a user or leaves the caller's signing key to name one, and whether
//! the credential is still live or already expired.
//!
//! The awsQuery harness (router, call, element) is the one `tests/all/user_lifecycle.rs`
//! owns, so every suite in this crate builds and reads a request the same way; the
//! create and the listing this suite reads through are exercised by
//! `tests/all/service_specific_credentials.rs`.

use aws_smithy_types::DateTime;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

// `router_with_engine` is the shared harness's router whose engine the test also
// holds: one test rewinds an expiry the clock has already made due.
use crate::user_lifecycle::{
    call, call_signed_by, element, elements, error_code, router, router_with_engine,
};

/// One holder to delete a credential of: the IAM user the account holds, the service
/// that issues the credential, and the case the delete request spells the name in.
#[derive(Debug)]
struct Case {
    user: &'static str,
    service: &'static str,
    /// How the delete request spells the user name — IAM entity names "are not
    /// distinguished by case", so a row may name the holder in another case.
    request_user: &'static str,
}

/// Rows spanning both credential FORMS (`cassandra`/`codecommit` issue a user name
/// and a password, `bedrock`/`logs` a long-term API key — the catalog is
/// `CreateServiceSpecificCredential`'s documentation, recorded in
/// `state::SERVICES_WITH_SPECIFIC_CREDENTIALS`), user names drawn from across
/// `userNameType`'s `^[\w+=,.@-]+$` alphabet, and both a request that spells the
/// holder exactly as the account holds it and one that spells it in another case.
const CASES: &[Case] = &[
    Case {
        user: "delete.user",
        service: "cassandra.amazonaws.com",
        request_user: "delete.user",
    },
    Case {
        user: "Git_Delete",
        service: "codecommit.amazonaws.com",
        request_user: "GIT_DELETE",
    },
    Case {
        user: "api+deleter",
        service: "bedrock.amazonaws.com",
        request_user: "api+deleter",
    },
    Case {
        user: "logs=delete",
        service: "logs.amazonaws.com",
        request_user: "LOGS=delete",
    },
];

/// An id of the modelled shape (`serviceSpecificCredentialId`, len 20..128, pattern
/// `^[\w]+$`) that no create in this suite minted.
const UNMINTED_ID: &str = "EMULATORNEVERMINTEDTHIS02";

/// Create one IAM user, failing the test if the account would not take it.
async fn create_user(router: &MultiServiceRouter, user: &str) {
    let (status, body) = call(router, "CreateUser", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {user:?}: expected the user to be created, got {status}: {body}"
    );
}

/// Create one service-specific credential, answering the id it was minted under.
async fn create_credential(router: &MultiServiceRouter, user: &str, service: &str) -> String {
    let (status, body) = call(
        router,
        "CreateServiceSpecificCredential",
        &[("UserName", user), ("ServiceName", service)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {user:?}/{service:?}: expected the credential to be created, got {status}: {body}"
    );
    element(&body, "ServiceSpecificCredentialId")
}

/// The ids `ListServiceSpecificCredentials` reports for a user.
async fn listed_ids(router: &MultiServiceRouter, user: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListServiceSpecificCredentials",
        &[("UserName", user)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {user:?}: expected the listing to succeed, got {status}: {body}"
    );
    elements(&body, "ServiceSpecificCredentialId")
}

/// One `DeleteServiceSpecificCredential` call naming a user.
async fn delete_credential(router: &MultiServiceRouter, user: &str, id: &str) -> (u16, String) {
    call(
        router,
        "DeleteServiceSpecificCredential",
        &[("UserName", user), ("ServiceSpecificCredentialId", id)],
    )
    .await
}

/// RULE: the delete removes exactly the stored credential its id names from the
/// resolved user's holdings, leaving every other credential the user holds readable,
/// and a later read reports the store the delete wrote rather than the request.
#[tokio::test]
async fn delete_removes_only_the_credential_the_id_names() {
    let router = router();
    for case in CASES {
        create_user(&router, case.user).await;
        // Two credentials for the one service — the most a user may hold for it —
        // plus one for another service, so the delete has siblings to leave alone in
        // both the same and a different service.
        let first = create_credential(&router, case.user, case.service).await;
        let second = create_credential(&router, case.user, case.service).await;
        let other_service = if case.service == "cassandra.amazonaws.com" {
            "bedrock.amazonaws.com"
        } else {
            "cassandra.amazonaws.com"
        };
        let elsewhere = create_credential(&router, case.user, other_service).await;

        let before = listed_ids(&router, case.user).await;
        assert_eq!(
            before.len(),
            3,
            "case {case:?}: expected three credentials before the delete, got {before:?}"
        );

        let (status, body) = delete_credential(&router, case.request_user, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to succeed, got {status}: {body}"
        );
        // A `Unit` output: the answer carries no credential at all, and no secret.
        for member in [
            "ServiceSpecificCredentialId",
            "ServicePassword",
            "ServiceCredentialSecret",
        ] {
            assert!(
                elements(&body, member).is_empty(),
                "case {case:?}: expected no {member} in the delete's answer, got {:?}",
                elements(&body, member)
            );
        }

        let after = listed_ids(&router, case.user).await;
        assert!(
            !after.contains(&first),
            "case {case:?}: expected {first:?} to be gone from the listing, got {after:?}"
        );
        for kept in [&second, &elsewhere] {
            assert!(
                after.contains(kept),
                "case {case:?}: expected {kept:?} to survive the delete, got {after:?}"
            );
        }
        assert_eq!(
            after.len(),
            2,
            "case {case:?}: expected exactly one credential to be removed, got {after:?}"
        );

        // The same for the second of the pair: deleting the last credential a user
        // holds for a service leaves the user holding none for it, not an error.
        let (status, body) = delete_credential(&router, case.request_user, &second).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the second delete to succeed, got {status}: {body}"
        );
        let after = listed_ids(&router, case.user).await;
        assert_eq!(
            after,
            vec![elsewhere.clone()],
            "case {case:?}: expected only the other service's credential to remain, got {after:?}"
        );
    }
}

/// RULE: a deleted credential is gone from the account, so every later call that
/// reaches a credential by id — another delete, a status update, a password reset —
/// answers `NoSuchEntity` (HTTP 404) for it.
#[tokio::test]
async fn a_deleted_credential_is_unreachable_by_every_later_call() {
    let router = router();
    // Rows: the follow-up operation attempted on the deleted id, spanning every
    // operation that names a service-specific credential by id.
    let follow_ups: &[(&str, &[(&str, &str)])] = &[
        ("DeleteServiceSpecificCredential", &[]),
        ("UpdateServiceSpecificCredential", &[("Status", "Inactive")]),
        ("ResetServiceSpecificCredential", &[]),
    ];

    for case in CASES {
        let user = format!("again.{}", case.user);
        create_user(&router, &user).await;
        for (action, extra) in follow_ups {
            let id = create_credential(&router, &user, case.service).await;
            let (status, body) = delete_credential(&router, &user, &id).await;
            assert_eq!(
                status, 200,
                "case {user:?}/{action}: expected the delete to succeed, got {status}: {body}"
            );

            let mut params: Vec<(&str, &str)> = vec![
                ("UserName", user.as_str()),
                ("ServiceSpecificCredentialId", id.as_str()),
            ];
            params.extend_from_slice(extra);
            let (status, body) = call(&router, action, &params).await;
            assert_eq!(
                status, 404,
                "case {user:?}/{action}: expected a deleted id to be unreachable, \
                 got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {user:?}/{action}: expected NoSuchEntity, got {:?}",
                error_code(&body)
            );
            // Every operation that reaches a credential by id words the failed
            // lookup the same way, around the id the request carried.
            assert_eq!(
                element(&body, "Message"),
                format!("No such credential {id} exists"),
                "case {user:?}/{action}: expected the missing-credential wording naming \
                 {id:?}, got {:?}",
                element(&body, "Message")
            );
        }
    }
}

/// RULE: the room a credential takes in the per-service quota is freed by its
/// delete, so a user at the quota can hold a new credential again — the count is
/// taken against what the store holds, not against how many were ever minted.
#[tokio::test]
async fn delete_frees_the_room_the_credential_took_in_the_quota() {
    // The quota this test is taken against, from the service's own record of it, so
    // the test cannot drift from the handler.
    let quota = state::MAX_SERVICE_SPECIFIC_CREDENTIALS_PER_SERVICE;
    let router = router();

    for case in CASES {
        let user = format!("quota.{}", case.user);
        create_user(&router, &user).await;
        let mut held = Vec::new();
        for _ in 0..quota {
            held.push(create_credential(&router, &user, case.service).await);
        }

        // At the quota, the next create is refused.
        let (status, body) = call(
            &router,
            "CreateServiceSpecificCredential",
            &[("UserName", user.as_str()), ("ServiceName", case.service)],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the create at the quota of {quota} to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {case:?}: expected LimitExceeded, got {:?}",
            error_code(&body)
        );

        // One delete makes room for exactly one more.
        let (status, body) = delete_credential(&router, &user, &held[0]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to succeed, got {status}: {body}"
        );
        let minted = create_credential(&router, &user, case.service).await;
        assert_ne!(
            minted, held[0],
            "case {case:?}: expected a new credential id after the delete, got the deleted one"
        );
        let after = listed_ids(&router, &user).await;
        assert_eq!(
            after.len(),
            quota,
            "case {case:?}: expected the user back at the quota of {quota}, got {after:?}"
        );

        // And only one: the account is at the quota again.
        let (status, body) = call(
            &router,
            "CreateServiceSpecificCredential",
            &[("UserName", user.as_str()), ("ServiceName", case.service)],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the quota to bind again after one create, \
             got {status}: {body}"
        );
    }
}

/// RULE: a credential is reachable only through the user that holds it, so an id the
/// account never minted and an id another user holds are the same answer —
/// `NoSuchEntity` (HTTP 404), with nothing removed. A user the account does not hold
/// is the same refusal.
#[tokio::test]
async fn delete_reaches_only_a_credential_the_named_user_holds() {
    let router = router();

    // Two holders, each with a credential of a different form, so the isolation is
    // exercised in both directions and across forms.
    let holders = [
        ("holder.one", "cassandra.amazonaws.com"),
        ("Holder_Two", "bedrock.amazonaws.com"),
    ];
    let mut held = Vec::new();
    for (user, service) in holders {
        create_user(&router, user).await;
        held.push((user, create_credential(&router, user, service).await));
    }

    // Rows: each holder asked to delete the OTHER holder's credential, each holder
    // asked for an id nothing minted, and an absent user named with a real id.
    let mut cases: Vec<(String, String, &str)> = Vec::new();
    for (user, _) in &held {
        for (other, id) in &held {
            if user != other {
                cases.push((user.to_string(), id.clone(), "another holder's id"));
            }
        }
        cases.push((user.to_string(), UNMINTED_ID.to_string(), "an unminted id"));
    }
    cases.push((
        "absent.deleter".to_string(),
        held[0].1.clone(),
        "a user the account does not hold",
    ));

    for (user, id, why) in &cases {
        let (status, body) = delete_credential(&router, user, id).await;
        assert_eq!(
            status, 404,
            "case {user:?} with {why} ({id:?}): expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {user:?} with {why} ({id:?}): expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
        // The refusal names the id the request asked for: the fixed text is IAM's,
        // the variable part is this request's id, so each row's message differs.
        if *why == "a user the account does not hold" {
            assert_eq!(
                element(&body, "Message"),
                format!("The user with name {user} cannot be found."),
                "case {user:?} with {why}: expected the missing-user wording, got {:?}",
                element(&body, "Message")
            );
        } else {
            assert_eq!(
                element(&body, "Message"),
                format!("No such credential {id} exists"),
                "case {user:?} with {why}: expected the missing-credential wording naming \
                 {id:?}, got {:?}",
                element(&body, "Message")
            );
        }
    }

    // Nothing was removed by any refusal: every credential is still held.
    for (user, id) in &held {
        let listed = listed_ids(&router, user).await;
        assert!(
            listed.contains(id),
            "case {user:?}: a refused delete removed {id:?}, got {listed:?}"
        );
    }
}

/// RULE: with no `UserName` the operation acts for "the user whose credentials are
/// used to call the operation", resolved from the request's signing key; credentials
/// that belong to no IAM user resolve to no user and are refused rather than having
/// an identity invented for them.
#[tokio::test]
async fn an_omitted_name_deletes_the_signing_users_credential() {
    let router = router();

    // Two users, each with an access key to sign with and two credentials to delete,
    // so the resolution is exercised on more than one holder and the cross-holder
    // refusal has a second identity to come from.
    let mut holders = Vec::new();
    for (user, service) in [
        ("nameless.one", "codecommit.amazonaws.com"),
        ("Nameless_Two", "logs.amazonaws.com"),
    ] {
        create_user(&router, user).await;
        let (status, key) = call(&router, "CreateAccessKey", &[("UserName", user)]).await;
        assert_eq!(
            status, 200,
            "case {user:?}: CreateAccessKey must succeed, got {status}: {key}"
        );
        let first = create_credential(&router, user, service).await;
        let second = create_credential(&router, user, service).await;
        holders.push((user, element(&key, "AccessKeyId"), first, second));
    }

    for (user, akid, first, second) in &holders {
        let (status, body) = call_signed_by(
            &router,
            Some(akid),
            "DeleteServiceSpecificCredential",
            &[("ServiceSpecificCredentialId", first)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {user:?}: expected the signing user's own credential to be deleted, \
             got {status}: {body}"
        );
        let listed = listed_ids(&router, user).await;
        assert_eq!(
            listed,
            vec![second.clone()],
            "case {user:?}: expected the nameless delete to remove exactly the named \
             credential, got {listed:?}"
        );
    }

    // A nameless request signed by one holder cannot reach the other's credential:
    // the id resolves against the signing user, so it is not found.
    let (status, body) = call_signed_by(
        &router,
        Some(&holders[0].1),
        "DeleteServiceSpecificCredential",
        &[("ServiceSpecificCredentialId", &holders[1].3)],
    )
    .await;
    assert_eq!(
        status, 404,
        "expected a nameless delete to reach only the signing user's credentials, \
         got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "NoSuchEntity",
        "expected NoSuchEntity, got {:?}",
        error_code(&body)
    );

    // Rows spanning no signing key at all, the fallback id an unsigned request
    // resolves to, a key id of the modelled shape this emulator never issued, and an
    // id that names a user rather than a key: none names an IAM user.
    let signers: &[Option<&str>] = &[
        None,
        Some("default"),
        Some("AKIAIOSFODNN7EXAMPLE"),
        Some("nameless.one"),
    ];
    for signer in signers {
        let (status, body) = call_signed_by(
            &router,
            *signer,
            "DeleteServiceSpecificCredential",
            &[("ServiceSpecificCredentialId", &holders[0].3)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {signer:?}: expected a request with no caller identity to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {signer:?}: expected ValidationException, got {:?}",
            error_code(&body)
        );
    }

    // Every refusal above removed nothing: each holder still holds the credential the
    // accepted delete left.
    for (user, _, _, second) in &holders {
        let listed = listed_ids(&router, user).await;
        assert_eq!(
            listed,
            vec![second.clone()],
            "case {user:?}: a refused delete changed what the user holds, got {listed:?}"
        );
    }
}

/// RULE: the status the credential carries does not gate its delete — a credential
/// the caller made `Inactive`, and one the clock has already expired, are deleted
/// exactly like a live one.
#[tokio::test]
async fn delete_removes_a_credential_in_any_status() {
    let (router, engine) = router_with_engine();
    create_user(&router, "status.deleter").await;

    // Rows: the status the credential is in when the delete arrives — the one the
    // create leaves, one a caller assigned, and one the clock made due.
    for case in ["Active", "Inactive", "Expired"] {
        // A long-term API key, the form that can carry an expiry at all.
        let (status, created) = call(
            &router,
            "CreateServiceSpecificCredential",
            &[
                ("UserName", "status.deleter"),
                ("ServiceName", "bedrock.amazonaws.com"),
                ("CredentialAgeDays", "1"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the credential to be created, got {status}: {created}"
        );
        let id = element(&created, "ServiceSpecificCredentialId");

        match case {
            "Inactive" => {
                let (status, body) = call(
                    &router,
                    "UpdateServiceSpecificCredential",
                    &[
                        ("UserName", "status.deleter"),
                        ("ServiceSpecificCredentialId", &id),
                        ("Status", "Inactive"),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the status to be assigned, got {status}: {body}"
                );
            }
            "Expired" => {
                // Rewinding the stored expiry is how a day-granular expiry is
                // reached without waiting for it; the handler reads the clock, never
                // the test.
                let store = state::service_state(&*engine).expect("the IAM store");
                let mut store = store.lock();
                let credential = store
                    .service_specific_credentials
                    .get_mut(&id)
                    .unwrap_or_else(|| panic!("case {case:?}: the created credential is not stored"));
                let now = DateTime::from(std::time::SystemTime::now()).secs();
                credential.expiration_date = Some(DateTime::from_secs(now - 1));
            }
            _ => {}
        }

        let (status, body) = delete_credential(&router, "status.deleter", &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a {case} credential to be deleted, got {status}: {body}"
        );
        let listed = listed_ids(&router, "status.deleter").await;
        assert!(
            !listed.contains(&id),
            "case {case:?}: expected {id:?} to be gone from the listing, got {listed:?}"
        );
    }
}
