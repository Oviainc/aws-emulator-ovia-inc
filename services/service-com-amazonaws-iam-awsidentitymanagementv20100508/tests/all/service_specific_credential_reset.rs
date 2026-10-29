// SPDX-License-Identifier: CC0-1.0

//! `ResetServiceSpecificCredential`, end to end over the awsQuery wire.
//!
//! The operation replaces the GENERATED secret of one stored service-specific
//! credential and reports the record otherwise unchanged, and it reaches only a
//! credential the resolved user holds. Each test is a TABLE whose rows differ along
//! what the rule generalises over: the service and so the FORM of credential it
//! issues, the case the request spells the user name in, how many times the same
//! credential is reset, the status the record carries when it is reset, whether the
//! credential expires, and which credential id the request carries.
//!
//! The awsQuery harness (router, call, element) is the one `tests/all/user_lifecycle.rs`
//! owns, so every suite in this crate builds and reads a request the same way; the
//! create and the listing beside it are exercised by
//! `tests/all/service_specific_credentials.rs` and the status update by
//! `tests/all/service_specific_credential_status.rs`.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, call_signed_by, element, elements, error_code, router};

/// The FORM of credential a service issues, which decides which member of
/// `ServiceSpecificCredential` the reset's fresh secret arrives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `ServiceUserName` plus the regenerated `ServicePassword`.
    UserNameAndPassword,
    /// `ServiceCredentialAlias` plus the regenerated `ServiceCredentialSecret`.
    LongTermApiKey,
}

impl Form {
    /// The member carrying the generated secret for this form, and the member
    /// carrying the public half the create minted.
    fn secret_member(self) -> &'static str {
        match self {
            Form::UserNameAndPassword => "ServicePassword",
            Form::LongTermApiKey => "ServiceCredentialSecret",
        }
    }

    /// The member no credential of this form carries.
    fn absent_secret_member(self) -> &'static str {
        match self {
            Form::UserNameAndPassword => "ServiceCredentialSecret",
            Form::LongTermApiKey => "ServicePassword",
        }
    }

    /// The members the create minted and no reset may move.
    fn public_members(self) -> &'static [&'static str] {
        match self {
            Form::UserNameAndPassword => &["ServiceUserName"],
            Form::LongTermApiKey => &["ServiceCredentialAlias"],
        }
    }
}

/// One credential to reset: the IAM user holding it, the service that issues it, the
/// form that service issues, and how the reset request spells the holder's name.
#[derive(Debug)]
struct Case {
    user: &'static str,
    service: &'static str,
    form: Form,
    /// IAM entity names "are not distinguished by case", so a row may name the
    /// holder in another case than the account holds it in.
    request_user: &'static str,
}

/// Rows spanning the whole catalog of services that support service-specific
/// credentials (`state::SERVICES_WITH_SPECIFIC_CREDENTIALS`, whose source is
/// `CreateServiceSpecificCredential`'s documentation), both credential forms, user
/// names drawn from across `userNameType`'s `^[\w+=,.@-]+$` alphabet, and both a
/// request that spells the holder exactly as the account holds it and one that
/// spells it in another case.
const CASES: &[Case] = &[
    Case {
        user: "reset.keyspaces",
        service: "cassandra.amazonaws.com",
        form: Form::UserNameAndPassword,
        request_user: "reset.keyspaces",
    },
    Case {
        user: "Git_Reset",
        service: "codecommit.amazonaws.com",
        form: Form::UserNameAndPassword,
        request_user: "GIT_RESET",
    },
    Case {
        user: "bedrock+reset",
        service: "bedrock.amazonaws.com",
        form: Form::LongTermApiKey,
        request_user: "bedrock+reset",
    },
    Case {
        user: "logs=reset",
        service: "logs.amazonaws.com",
        form: Form::LongTermApiKey,
        request_user: "LOGS=reset",
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

/// One `ResetServiceSpecificCredential` call naming a user.
async fn reset(router: &MultiServiceRouter, user: &str, id: &str) -> (u16, String) {
    call(
        router,
        "ResetServiceSpecificCredential",
        &[("UserName", user), ("ServiceSpecificCredentialId", id)],
    )
    .await
}

/// RULE: a reset generates a NEW secret for the credential the id names — a
/// different one on every call, in the member that credential's form carries it in —
/// and reports every other member exactly as the account holds it; no secret is
/// stored, so a later read reports the record and no secret at all.
#[tokio::test]
async fn reset_issues_a_fresh_secret_and_moves_no_other_member() {
    let router = router();
    for case in CASES {
        create_user(&router, case.user).await;
        let created = create_credential(&router, case.user, case.service).await;
        let id = element(&created, "ServiceSpecificCredentialId");

        // What the create persisted, which no reset may move. The reported holder is
        // the name the ACCOUNT holds, whatever case the reset request spells.
        let mut unchanged: Vec<(&str, String)> = ["UserName", "ServiceName", "CreateDate", "Status"]
            .iter()
            .map(|member| (*member, element(&created, member)))
            .collect();
        for member in case.form.public_members() {
            unchanged.push((member, element(&created, member)));
        }
        assert!(
            elements(&created, case.form.absent_secret_member()).is_empty(),
            "case {case:?}: the create reported a {}, which this form does not carry",
            case.form.absent_secret_member()
        );

        // Rows: the same credential reset more than once, because each reset mints
        // material of its own rather than re-reporting the last one.
        let mut seen = vec![element(&created, case.form.secret_member())];
        for attempt in 1..=3 {
            let (status, body) = reset(&router, case.request_user, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?} reset {attempt}: expected the reset to succeed, got {status}: {body}"
            );

            let secret = element(&body, case.form.secret_member());
            assert!(
                !secret.is_empty(),
                "case {case:?} reset {attempt}: expected a generated {}, got an empty one",
                case.form.secret_member()
            );
            assert!(
                !seen.contains(&secret),
                "case {case:?} reset {attempt}: expected a secret the credential never had, \
                 got one of {seen:?}"
            );
            assert!(
                elements(&body, case.form.absent_secret_member()).is_empty(),
                "case {case:?} reset {attempt}: reported a {}, which this form does not carry",
                case.form.absent_secret_member()
            );
            seen.push(secret);

            for (member, value) in &unchanged {
                assert_eq!(
                    &element(&body, member),
                    value,
                    "case {case:?} reset {attempt}: moved {member}, which the reset must report \
                     as the account holds it; got {:?}",
                    element(&body, member)
                );
            }
            // No `CredentialAgeDays` was supplied, so the credential does not expire
            // and the reset mints no expiry of its own.
            assert!(
                elements(&body, "ExpirationDate").is_empty(),
                "case {case:?} reset {attempt}: added an ExpirationDate, got {:?}",
                elements(&body, "ExpirationDate")
            );

            // The record a later caller sees: the reset wrote no secret into it, and
            // moved nothing else either.
            let listed = list_credentials(&router, case.user).await;
            for (member, value) in &unchanged {
                assert_eq!(
                    &member_of(&listed, &id, member),
                    value,
                    "case {case:?} reset {attempt}: the stored {member} moved, got {:?}",
                    member_of(&listed, &id, member)
                );
            }
            for secret_member in ["ServicePassword", "ServiceCredentialSecret"] {
                assert!(
                    elements(&listed, secret_member).is_empty(),
                    "case {case:?} reset {attempt}: the listing reports a {secret_member}, which \
                     is available only from the call that generated it"
                );
            }
        }

        // The `ServiceUserName` of a user-name-and-password credential is the holder
        // joined to the account id, so the reset reports the value the create
        // generated rather than one derived from the request's spelling.
        if case.form == Form::UserNameAndPassword {
            assert_eq!(
                member_of(&list_credentials(&router, case.user).await, &id, "ServiceUserName"),
                format!("{}-at-{}", case.user, account_id()),
                "case {case:?}: expected the stored ServiceUserName to survive the resets"
            );
        }
    }
}

/// RULE: a reset replaces the secret and nothing else, so the status the record
/// carries and the expiry the create minted both survive it.
#[tokio::test]
async fn reset_keeps_the_status_and_the_expiry_the_record_holds() {
    let router = router();
    // Rows: ages spanning `credentialAgeDays`'s published range (1..36600) at its
    // lower bound, a value inside it and its upper bound, each crossed with the
    // status the record carries when it is reset — including `Inactive`, which a
    // reset must not reactivate.
    for days in ["1", "180", "36600"] {
        for status_value in ["Active", "Inactive"] {
            let user = format!("reset{days}{status_value}");
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
                "case {days:?}/{status_value:?}: expected the credential to be created, \
                 got {status}: {created}"
            );
            let id = element(&created, "ServiceSpecificCredentialId");
            let expires_at = element(&created, "ExpirationDate");
            let alias = element(&created, "ServiceCredentialAlias");

            let (status, body) = call(
                &router,
                "UpdateServiceSpecificCredential",
                &[
                    ("UserName", &user),
                    ("ServiceSpecificCredentialId", &id),
                    ("Status", status_value),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {days:?}/{status_value:?}: expected Status={status_value} to be accepted, \
                 got {status}: {body}"
            );

            let (status, body) = reset(&router, &user, &id).await;
            assert_eq!(
                status, 200,
                "case {days:?}/{status_value:?}: expected the reset to succeed, got {status}: {body}"
            );
            assert_eq!(
                element(&body, "ExpirationDate"),
                expires_at,
                "case {days:?}/{status_value:?}: expected the create's ExpirationDate to survive \
                 the reset, got {:?}",
                element(&body, "ExpirationDate")
            );
            assert_eq!(
                element(&body, "Status"),
                status_value,
                "case {days:?}/{status_value:?}: expected the stored status to survive the reset, \
                 got {:?}",
                element(&body, "Status")
            );
            assert_eq!(
                element(&body, "ServiceCredentialAlias"),
                alias,
                "case {days:?}/{status_value:?}: expected the create's ServiceCredentialAlias to \
                 survive the reset, got {:?}",
                element(&body, "ServiceCredentialAlias")
            );

            let listed = list_credentials(&router, &user).await;
            for (member, value) in [
                ("ExpirationDate", &expires_at),
                ("Status", &status_value.to_string()),
                ("ServiceCredentialAlias", &alias),
            ] {
                assert_eq!(
                    &member_of(&listed, &id, member),
                    value,
                    "case {days:?}/{status_value:?}: the stored {member} moved, got {:?}",
                    member_of(&listed, &id, member)
                );
            }
        }
    }
}

/// RULE: a credential is addressed by the (user, id) pair, so the two ways that pair
/// can fail are different refusals: an id the account holds no credential under at all
/// is `NoSuchEntity` (HTTP 404), while an id another user holds is a validation failure
/// of the pair — `ValidationError` (HTTP 400) naming the user and the id — and no secret
/// is generated by either. A user the account does not hold is the missing-entity refusal.
#[tokio::test]
async fn reset_reaches_only_a_credential_the_named_user_holds() {
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

    // Rows: each holder asked to reset the OTHER holder's credential, each holder
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
        let (status, body) = reset(&router, user, id).await;
        // An id another user holds EXISTS, so the refusal is the validation failure of
        // the pair naming both halves of it; the other rows name no credential at all.
        let (expected_status, expected_code, expected_message) = if *why == "another holder's id"
        {
            (
                400,
                "ValidationError",
                format!("User {user} does not have a service specific credential with id {id}"),
            )
        } else if *why == "a user the account does not hold" {
            (
                404,
                "NoSuchEntity",
                format!("The user with name {user} cannot be found."),
            )
        } else {
            (
                404,
                "NoSuchEntity",
                format!("No such credential {id} exists"),
            )
        };
        assert_eq!(
            status, expected_status,
            "case {user:?} with {why} ({id:?}): expected {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {user:?} with {why} ({id:?}): expected {expected_code}, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            expected_message,
            "case {user:?} with {why} ({id:?}): expected {expected_message:?}, got {:?}",
            element(&body, "Message")
        );
        for secret_member in ["ServicePassword", "ServiceCredentialSecret"] {
            assert!(
                elements(&body, secret_member).is_empty(),
                "case {user:?} with {why} ({id:?}): a refused reset reported a {secret_member}"
            );
        }
    }

    // Nothing was written by any refusal: every credential is still as created, and
    // each holder can still reset its own.
    for (user, id) in &held {
        let listed = list_credentials(&router, user).await;
        assert_eq!(
            member_of(&listed, id, "Status"),
            "Active",
            "case {user:?}: a refused reset moved the stored status, got {:?}",
            member_of(&listed, id, "Status")
        );
        let (status, body) = reset(&router, user, id).await;
        assert_eq!(
            status, 200,
            "case {user:?}: expected the holder's own reset to succeed, got {status}: {body}"
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

    // Two users, each with an access key to sign with and a credential to reset, so
    // the resolution is exercised on more than one holder and the cross-holder
    // refusal has a second identity to come from.
    let mut holders = Vec::new();
    for (user, service, form) in [
        ("signer.one", "codecommit.amazonaws.com", Form::UserNameAndPassword),
        ("Signer_Two", "logs.amazonaws.com", Form::LongTermApiKey),
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
            form,
            element(&created, form.secret_member()),
        ));
    }

    for (user, akid, id, form, created_secret) in &holders {
        let (status, body) = call_signed_by(
            &router,
            Some(akid),
            "ResetServiceSpecificCredential",
            &[("ServiceSpecificCredentialId", id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {user:?}: expected the signing user's own credential to be reset, \
             got {status}: {body}"
        );
        assert_eq!(
            &element(&body, "ServiceSpecificCredentialId"),
            id,
            "case {user:?}: expected the nameless reset to reach the signing user's credential, \
             got {:?}",
            element(&body, "ServiceSpecificCredentialId")
        );
        assert_eq!(
            &element(&body, "UserName"),
            user,
            "case {user:?}: expected the holder to be reported, got {:?}",
            element(&body, "UserName")
        );
        assert_ne!(
            &element(&body, form.secret_member()),
            created_secret,
            "case {user:?}: expected the reset to invalidate the created secret by generating \
             another, got the created one back"
        );
    }

    // A nameless request signed by one holder cannot reach the other's credential: the
    // id resolves against the signing user, and a credential that user does not hold
    // fails the pair the call addressed.
    let (status, body) = call_signed_by(
        &router,
        Some(&holders[0].1),
        "ResetServiceSpecificCredential",
        &[("ServiceSpecificCredentialId", &holders[1].2)],
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a nameless reset to reach only the signing user's credentials, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationError",
        "expected ValidationError, got {:?}",
        error_code(&body)
    );
    // The implicitly resolved user is named as the account holds it.
    assert_eq!(
        element(&body, "Message"),
        format!(
            "User {} does not have a service specific credential with id {}",
            holders[0].0, holders[1].2
        ),
        "expected the pair refusal to name the signing user and the id, got {:?}",
        element(&body, "Message")
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
            "ResetServiceSpecificCredential",
            &[("ServiceSpecificCredentialId", &holders[0].2)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {signer:?}: expected a request with no caller identity to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {signer:?}: expected ValidationError, got {:?}",
            error_code(&body)
        );
    }
}
