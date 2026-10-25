// SPDX-License-Identifier: CC0-1.0

//! `UpdateServiceSpecificCredential`, end to end over the awsQuery wire.
//!
//! The operation moves the STATUS of one stored service-specific credential and
//! nothing else, and it reaches only a credential the resolved user holds. Each test
//! is a TABLE whose rows differ along what the rule generalises over: the status
//! value assigned (the whole `statusType` value set, plus a repeat for idempotence),
//! the service and so the FORM of credential it issues, the case the request spells
//! the user name in, whether the request names a user or leaves the caller's signing
//! key to name one, and which credential id the request carries.
//!
//! The awsQuery harness (router, call, element) is the one `tests/all/user_lifecycle.rs`
//! owns, so every suite in this crate builds and reads a request the same way; the
//! create and the listing beside it are exercised by
//! `tests/all/service_specific_credentials.rs`.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::STATUS_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// One credential to move the status of: the IAM user that holds it and the service
/// that issues it.
#[derive(Debug)]
struct Case {
    user: &'static str,
    service: &'static str,
    /// How the update request spells the user name — IAM entity names "are not
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
        user: "status.user",
        service: "cassandra.amazonaws.com",
        request_user: "status.user",
    },
    Case {
        user: "Git_Status",
        service: "codecommit.amazonaws.com",
        request_user: "GIT_STATUS",
    },
    Case {
        user: "api+keyholder",
        service: "bedrock.amazonaws.com",
        request_user: "api+keyholder",
    },
    Case {
        user: "logs=status",
        service: "logs.amazonaws.com",
        request_user: "LOGS=status",
    },
];

/// An id of the modelled shape (`serviceSpecificCredentialId`, len 20..128, pattern
/// `^[\w]+$`) that no create in this suite minted.
const UNMINTED_ID: &str = "EMULATORNEVERMINTEDTHIS01";

/// Create one IAM user, failing the test if the account would not take it.
async fn create_user(router: &MultiServiceRouter, user: &str) {
    let (status, body) = call(router, "CreateUser", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {user:?}: expected the user to be created, got {status}: {body}"
    );
}

/// Create one service-specific credential, answering the whole create response.
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
    body
}

/// One `ListServiceSpecificCredentials` call for a user, asserted to succeed.
async fn list_credentials(router: &MultiServiceRouter, user: &str) -> String {
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
    body
}

/// The member `member` the listing reports for the credential `id`.
///
/// Every credential in a listing carries each of these members exactly once, in
/// document order, so the member at the id's position is that credential's.
fn member_of(body: &str, id: &str, member: &str) -> String {
    let ids = elements(body, "ServiceSpecificCredentialId");
    let values = elements(body, member);
    assert_eq!(
        ids.len(),
        values.len(),
        "expected one {member} per credential, got {} ids and {} values: {body}",
        ids.len(),
        values.len()
    );
    let at = ids
        .iter()
        .position(|listed| listed == id)
        .unwrap_or_else(|| panic!("expected the listing to report {id:?}, got {ids:?}: {body}"));
    values[at].clone()
}

/// One `UpdateServiceSpecificCredential` call naming a user.
async fn update_status(
    router: &MultiServiceRouter,
    user: &str,
    id: &str,
    status: &str,
) -> (u16, String) {
    call(
        router,
        "UpdateServiceSpecificCredential",
        &[
            ("UserName", user),
            ("ServiceSpecificCredentialId", id),
            ("Status", status),
        ],
    )
    .await
}

/// RULE: the update assigns the requested `statusType` value to the stored
/// credential the id names, is idempotent in that value, and moves no other member
/// of the record; a later read reports the stored status rather than one recomputed
/// from the reading request.
#[tokio::test]
async fn update_assigns_each_modelled_status_and_moves_nothing_else() {
    // The catalog this test spans, taken from the service's own record of
    // `statusType`'s value set (`smithy_query(query="shape", shape="statusType")`),
    // so the test cannot drift from the model.
    assert_eq!(
        STATUS_TYPE_VALUES,
        ["Active", "Expired", "Inactive"],
        "statusType's value set moved, so this test no longer spans it: got {STATUS_TYPE_VALUES:?}"
    );
    // Rows: every value the enum declares, a REPEAT of one (the assignment is
    // idempotent), and a return to the value the create left the record at.
    let assignments = ["Inactive", "Inactive", "Expired", "Active"];
    for value in STATUS_TYPE_VALUES {
        assert!(
            assignments.contains(value),
            "expected the assignments to span statusType, {value:?} is missing"
        );
    }

    let router = router();
    for case in CASES {
        create_user(&router, case.user).await;
        let created = create_credential(&router, case.user, case.service).await;
        let id = element(&created, "ServiceSpecificCredentialId");
        // What the create persisted, which only `Status` may move from here on.
        let before: Vec<(&str, String)> = ["UserName", "ServiceName", "CreateDate"]
            .iter()
            .map(|member| (*member, element(&created, member)))
            .collect();
        assert_eq!(
            element(&created, "Status"),
            "Active",
            "case {case:?}: expected a new credential to be Active, got {:?}",
            element(&created, "Status")
        );

        for assigned in assignments {
            let (status, body) = update_status(&router, case.request_user, &id, assigned).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected Status={assigned} to be accepted, got {status}: {body}"
            );

            let listed = list_credentials(&router, case.user).await;
            assert_eq!(
                member_of(&listed, &id, "Status"),
                assigned,
                "case {case:?}: expected the stored status to be {assigned}, got {:?}",
                member_of(&listed, &id, "Status")
            );
            for (member, value) in &before {
                assert_eq!(
                    &member_of(&listed, &id, member),
                    value,
                    "case {case:?}: Status={assigned} moved {member}, which the update must leave \
                     as the account holds it; got {:?}",
                    member_of(&listed, &id, member)
                );
            }
            // No `CredentialAgeDays` was supplied, so the credential does not expire
            // and the update mints no expiry of its own.
            assert!(
                elements(&listed, "ExpirationDate").is_empty(),
                "case {case:?}: Status={assigned} added an ExpirationDate, got {:?}",
                elements(&listed, "ExpirationDate")
            );
            // The update is not a credential issue: no secret is reported by it or by
            // the read that follows.
            for secret in ["ServicePassword", "ServiceCredentialSecret"] {
                assert!(
                    elements(&body, secret).is_empty() && elements(&listed, secret).is_empty(),
                    "case {case:?}: Status={assigned} reported a {secret}"
                );
            }
        }
    }
}

/// RULE: the update leaves the credential's expiry alone — a credential created with
/// an age keeps the expiration instant the create minted, whatever status is
/// assigned.
#[tokio::test]
async fn update_keeps_the_expiry_the_create_minted() {
    let router = router();
    // Rows: ages spanning `credentialAgeDays`'s published range (1..36600) at its
    // lower bound, a value inside it, and its upper bound.
    for days in ["1", "30", "36600"] {
        let user = format!("age{days}");
        create_user(&router, &user).await;
        // Only a service issuing long-term API keys takes `CredentialAgeDays`.
        let (status, created) = call(
            &router,
            "CreateServiceSpecificCredential",
            &[
                ("UserName", &user),
                ("ServiceName", "bedrock.amazonaws.com"),
                ("CredentialAgeDays", days),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {days:?}: expected the credential to be created, got {status}: {created}"
        );
        let id = element(&created, "ServiceSpecificCredentialId");
        let expires_at = element(&created, "ExpirationDate");

        let (status, body) = update_status(&router, &user, &id, "Inactive").await;
        assert_eq!(
            status, 200,
            "case {days:?}: expected Status=Inactive to be accepted, got {status}: {body}"
        );

        let listed = list_credentials(&router, &user).await;
        assert_eq!(
            member_of(&listed, &id, "ExpirationDate"),
            expires_at,
            "case {days:?}: expected the create's ExpirationDate to survive the update, got {:?}",
            member_of(&listed, &id, "ExpirationDate")
        );
        assert_eq!(
            member_of(&listed, &id, "Status"),
            "Inactive",
            "case {days:?}: expected the assigned status, got {:?}",
            member_of(&listed, &id, "Status")
        );
    }
}

/// RULE: a credential is reachable only through the user that holds it, so an id the
/// account never minted and an id another user holds are the same answer —
/// `NoSuchEntity` (HTTP 404), with no stored status moved. A user the account does
/// not hold is the same refusal.
#[tokio::test]
async fn update_reaches_only_a_credential_the_named_user_holds() {
    let router = router();

    // Two holders, each with a credential of a different form, so the isolation is
    // exercised in both directions and across forms.
    let holders = [
        ("owner.one", "cassandra.amazonaws.com"),
        ("Owner_Two", "bedrock.amazonaws.com"),
    ];
    let mut held = Vec::new();
    for (user, service) in holders {
        create_user(&router, user).await;
        let created = create_credential(&router, user, service).await;
        held.push((user, element(&created, "ServiceSpecificCredentialId")));
    }

    // Rows: each holder asked to update the OTHER holder's credential, each holder
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
        "absent.user".to_string(),
        held[0].1.clone(),
        "a user the account does not hold",
    ));

    for (user, id, why) in &cases {
        let (status, body) = update_status(&router, user, id, "Inactive").await;
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
    }

    // Nothing was written by any refusal: every credential is still as created.
    for (user, id) in &held {
        let listed = list_credentials(&router, user).await;
        assert_eq!(
            member_of(&listed, id, "Status"),
            "Active",
            "case {user:?}: a refused update moved the stored status, got {:?}",
            member_of(&listed, id, "Status")
        );
    }
}

/// RULE: with no `UserName` the operation acts for "the user whose credentials are
/// used to call the operation", resolved from the request's signing key; credentials
/// that belong to no IAM user resolve to no user and are refused rather than having
/// an identity invented for them.
#[tokio::test]
async fn an_omitted_name_resolves_the_signing_users_credential() {
    let router = router();

    // Two users, each with an access key to sign with and a credential to move, so
    // the resolution is exercised on more than one holder and the cross-holder
    // refusal has a second identity to come from.
    let mut holders = Vec::new();
    for (user, service) in [
        ("signer.one", "codecommit.amazonaws.com"),
        ("Signer_Two", "logs.amazonaws.com"),
    ] {
        create_user(&router, user).await;
        let (status, key) = call(&router, "CreateAccessKey", &[("UserName", user)]).await;
        assert_eq!(
            status, 200,
            "case {user:?}: CreateAccessKey must succeed, got {status}: {key}"
        );
        let created = create_credential(&router, user, service).await;
        holders.push((
            user,
            element(&key, "AccessKeyId"),
            element(&created, "ServiceSpecificCredentialId"),
        ));
    }

    for (user, akid, id) in &holders {
        let (status, body) = call_signed_by(
            &router,
            Some(akid),
            "UpdateServiceSpecificCredential",
            &[("ServiceSpecificCredentialId", id), ("Status", "Inactive")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {user:?}: expected the signing user's own credential to be updated, \
             got {status}: {body}"
        );
        let listed = list_credentials(&router, user).await;
        assert_eq!(
            member_of(&listed, id, "Status"),
            "Inactive",
            "case {user:?}: expected the nameless update to reach the stored credential, got {:?}",
            member_of(&listed, id, "Status")
        );
    }

    // A nameless request signed by one holder cannot reach the other's credential:
    // the id resolves against the signing user, so it is not found.
    let (status, body) = call_signed_by(
        &router,
        Some(&holders[0].1),
        "UpdateServiceSpecificCredential",
        &[
            ("ServiceSpecificCredentialId", &holders[1].2),
            ("Status", "Active"),
        ],
    )
    .await;
    assert_eq!(
        status, 404,
        "expected a nameless update to reach only the signing user's credentials, got {status}: {body}"
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
        Some("signer.one"),
    ];
    for signer in signers {
        let (status, body) = call_signed_by(
            &router,
            *signer,
            "UpdateServiceSpecificCredential",
            &[
                ("ServiceSpecificCredentialId", &holders[0].2),
                ("Status", "Active"),
            ],
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

    // Every refusal above left the stored statuses where the accepted updates put
    // them.
    for (user, _, id) in &holders {
        let listed = list_credentials(&router, user).await;
        assert_eq!(
            member_of(&listed, id, "Status"),
            "Inactive",
            "case {user:?}: a refused update moved the stored status, got {:?}",
            member_of(&listed, id, "Status")
        );
    }
}
