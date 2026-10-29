// SPDX-License-Identifier: CC0-1.0

//! The two refusals this service answers for an identity it cannot resolve, end to end
//! over the awsQuery wire.
//!
//! Two rules, each over a FAMILY of operations rather than over one call:
//!
//! 1. an operation whose `UserName` is optional resolves an absent one "based on the
//!    Amazon Web Services access key ID used to sign the request"; credentials that
//!    belong to no IAM user resolve to no user, and the call is refused with HTTP 400,
//!    the error code IAM answers a request it cannot accept with (`ValidationError`) and
//!    AWS's fixed sentence — never the Smithy-protocol spelling `ValidationException`,
//!    which IAM does not define, and never an invented identity;
//! 2. a service-specific credential is addressed by the (user, credential id) PAIR, so an
//!    id the account holds under ANOTHER user fails that pair — HTTP 400
//!    `ValidationError` naming the user and the id — while an id the account holds no
//!    credential under at all is the modelled missing entity (HTTP 404 `NoSuchEntity`).
//!    Neither writes anything, and the credential stays reachable under its real owner.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over: which
//! operation of the family is called, which credential the caller presents (none, the
//! fallback id of an unsigned request, an id of the modelled shape nothing minted, a name
//! that is no key id at all), which direction the cross-user pair is formed in, and the
//! FORM of credential the service issues.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does. The awsQuery harness is the
//! one `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::signing_certificates::certificate_pem;
use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// AWS's fixed sentence for a call whose implicit `UserName` resolved to no IAM user,
/// written out here rather than taken from the implementation's constant: what this suite
/// pins is the SENTENCE IAM answers, so a reworded constant has to fail here rather than
/// agree with itself.
const MUST_SPECIFY_USER_NAME: &str =
    "Must specify userName when calling with non-User credentials";

/// The error code IAM answers a request whose values it cannot accept with, and the one
/// its clients branch on.
const VALIDATION_ERROR: &str = "ValidationError";

/// The modelled missing-entity code (`NoSuchEntityException`'s `awsQueryError`).
const NO_SUCH_ENTITY: &str = "NoSuchEntity";

/// An access key id of the modelled shape (`accessKeyIdType`, len 16..128, pattern
/// `^[\w]+$`) that no create in this suite minted.
const UNMINTED_ACCESS_KEY_ID: &str = "EMULATORAKUNMINTED01";

/// A certificate id of the modelled shape (`certificateIdType`, len 24..128, pattern
/// `^[\w]+$`) that no upload in this suite minted.
const UNMINTED_CERTIFICATE_ID: &str = "EMULATORCERTUNMINTED00001";

/// A credential id of the modelled shape (`serviceSpecificCredentialId`, len 20..128,
/// pattern `^[\w]+$`) that no create in this suite minted.
const UNMINTED_CREDENTIAL_ID: &str = "EMULATORNEVERMINTEDTHIS01";

/// Create one IAM user, failing the test if the account would not take it.
async fn create_user(router: &MultiServiceRouter, user: &str) {
    let (status, body) = call(router, "CreateUser", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {user:?}: expected the user to be created, got {status}: {body}"
    );
}

/// Mint one access key for `user` and answer its id, so a request can be presented as
/// signed by an IAM user's own credential.
async fn access_key_of(router: &MultiServiceRouter, user: &str) -> String {
    let (status, body) = call(router, "CreateAccessKey", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {user:?}: expected an access key to be minted, got {status}: {body}"
    );
    element(&body, "AccessKeyId")
}

/// Create one service-specific credential for `user` at `service` and answer its id.
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

/// The credential ids `ListServiceSpecificCredentials` reports for `user`.
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

/// RULE 1: every operation of this service that resolves an absent `UserName` from the
/// request's signing key refuses a caller those credentials name no IAM user for with
/// HTTP 400, IAM's `ValidationError` and AWS's fixed sentence — and invents no identity.
///
/// The rows span every operation of the family (the user read, the three access-key
/// calls, the three login-profile calls, the four signing-certificate calls and the four
/// service-specific-credential calls) crossed with the credentials a caller can present
/// that belong to no IAM user.
#[tokio::test]
async fn an_unresolvable_implicit_user_name_is_refused_with_iams_validation_error() {
    let router = router();
    create_user(&router, "key.holder").await;
    // The upload row needs a well-formed `CertificateBody`: the certificate is read
    // before the caller's identity is resolved, so a malformed one would reach another
    // refusal than the one under test. The fixture is the one `tests/all/signing_certificates.rs`
    // owns.
    let certificate = certificate_pem(
        &rcgen::PKCS_ECDSA_P256_SHA256,
        (2024, 2034),
        "nobody.example.com",
    );

    // One row per operation that resolves `UserName` implicitly, with the members its
    // model requires beside it. Each id is of the modelled shape and names nothing: the
    // caller's identity is resolved before any resource is looked up, so what the row
    // reaches is the refusal under test.
    let password = "S3cret-nobody-42";
    let family: &[(&str, Vec<(&str, &str)>)] = &[
        ("GetUser", vec![]),
        ("CreateAccessKey", vec![]),
        ("ListAccessKeys", vec![]),
        (
            "UpdateAccessKey",
            vec![
                ("AccessKeyId", UNMINTED_ACCESS_KEY_ID),
                ("Status", "Inactive"),
            ],
        ),
        (
            "DeleteAccessKey",
            vec![("AccessKeyId", UNMINTED_ACCESS_KEY_ID)],
        ),
        ("CreateLoginProfile", vec![("Password", password)]),
        ("GetLoginProfile", vec![]),
        ("DeleteLoginProfile", vec![]),
        ("ListSigningCertificates", vec![]),
        (
            "UploadSigningCertificate",
            vec![("CertificateBody", certificate.as_str())],
        ),
        (
            "UpdateSigningCertificate",
            vec![
                ("CertificateId", UNMINTED_CERTIFICATE_ID),
                ("Status", "Inactive"),
            ],
        ),
        (
            "DeleteSigningCertificate",
            vec![("CertificateId", UNMINTED_CERTIFICATE_ID)],
        ),
        ("ListServiceSpecificCredentials", vec![]),
        (
            "UpdateServiceSpecificCredential",
            vec![
                ("ServiceSpecificCredentialId", UNMINTED_CREDENTIAL_ID),
                ("Status", "Inactive"),
            ],
        ),
        (
            "ResetServiceSpecificCredential",
            vec![("ServiceSpecificCredentialId", UNMINTED_CREDENTIAL_ID)],
        ),
        (
            "DeleteServiceSpecificCredential",
            vec![("ServiceSpecificCredentialId", UNMINTED_CREDENTIAL_ID)],
        ),
    ];

    // Credentials that belong to no IAM user: none at all, the fallback id an unsigned
    // request resolves to, an id of the published AWS shape this emulator never issued,
    // an id of this emulator's own shape it never issued, and a value that is an IAM user
    // NAME rather than a key id.
    let signers: &[Option<&str>] = &[
        None,
        Some("default"),
        Some("AKIAIOSFODNN7EXAMPLE"),
        Some("EMULATORAKAAAAAAAAAA"),
        Some("key.holder"),
    ];

    for (action, params) in family {
        for signer in signers {
            let case = format!("{action} signed by {signer:?}");
            let (status, body) = call_signed_by(&router, *signer, action, params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                VALIDATION_ERROR,
                "case {case:?}: expected {VALIDATION_ERROR}, got {:?}: {body}",
                error_code(&body)
            );
            assert_eq!(
                element(&body, "Message"),
                MUST_SPECIFY_USER_NAME,
                "case {case:?}: expected AWS's fixed sentence, got {:?}",
                element(&body, "Message")
            );
            // No identity was invented for the caller, so nothing of a user is reported.
            assert!(
                elements(&body, "UserName").is_empty(),
                "case {case:?}: expected no user to be reported, got {body}"
            );
        }
    }

    // None of the refused calls wrote anything for the user whose name was never spelled:
    // it holds no password, and no key beyond the one this suite minted for it below.
    let (status, body) = call(&router, "GetLoginProfile", &[("UserName", "key.holder")]).await;
    assert_eq!(
        status, 404,
        "expected the refused creates to have created no password, got {status}: {body}"
    );
    let (status, body) = call(&router, "ListAccessKeys", &[("UserName", "key.holder")]).await;
    assert_eq!(
        (status, elements(&body, "AccessKeyId").len()),
        (200, 0),
        "expected the refused creates to have minted no key, got {status}: {body}"
    );
    let certificates = {
        let (status, body) = call(
            &router,
            "ListSigningCertificates",
            &[("UserName", "key.holder")],
        )
        .await;
        assert_eq!(
            status, 200,
            "expected the certificate listing to succeed, got {status}: {body}"
        );
        elements(&body, "CertificateId")
    };
    assert!(
        certificates.is_empty(),
        "expected the refused uploads to have stored nothing, got {certificates:?}"
    );
}

/// RULE 1, the other half: an IAM user's OWN credential resolves, so the same nameless
/// calls are served for the user that signed them — the refusal above is about an identity
/// the request carries no user for, not about the absent member.
#[tokio::test]
async fn an_omitted_user_name_resolves_the_signing_iam_user() {
    let router = router();

    // Rows spanning user names drawn from across `userNameType`'s `^[\w+=,.@-]+$`
    // alphabet, so the resolution is exercised on more than one holder.
    for user in ["resolved.one", "Resolved_Two", "resolved+three"] {
        create_user(&router, user).await;
        let akid = access_key_of(&router, user).await;
        // IAM authorizes its own API, so the signing user needs the grants AWS requires
        // for these reads before it can make them; what this suite is about is which user
        // a nameless request resolves to, not the decision
        // (`tests/all/identity_policy_enforcement.rs` owns that).
        let (status, put) = call(
            &router,
            "PutUserPolicy",
            &[
                ("UserName", user),
                ("PolicyName", "allow-nameless-reads"),
                (
                    "PolicyDocument",
                    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:GetUser","iam:ListAccessKeys","iam:ListSigningCertificates","iam:ListServiceSpecificCredentials"],"Resource":"*"}]}"#,
                ),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {user:?}: the fixture PutUserPolicy must succeed, got {status}: {put}"
        );

        // The reads of the family: each reports the SIGNING user, not a refusal.
        let (status, body) = call_signed_by(&router, Some(&akid), "GetUser", &[]).await;
        assert_eq!(
            (status, element(&body, "UserName")),
            (200, user.to_string()),
            "case {user:?}: expected the nameless read to resolve the signing user, \
             got {status}: {body}"
        );
        let (status, body) = call_signed_by(&router, Some(&akid), "ListAccessKeys", &[]).await;
        assert_eq!(
            (status, elements(&body, "AccessKeyId")),
            (200, vec![akid.clone()]),
            "case {user:?}: expected the nameless listing to report the signing user's key, \
             got {status}: {body}"
        );
        let (status, body) =
            call_signed_by(&router, Some(&akid), "ListSigningCertificates", &[]).await;
        assert_eq!(
            status, 200,
            "case {user:?}: expected the nameless certificate listing to be served, \
             got {status}: {body}"
        );
        let (status, body) =
            call_signed_by(&router, Some(&akid), "ListServiceSpecificCredentials", &[]).await;
        assert_eq!(
            status, 200,
            "case {user:?}: expected the nameless credential listing to be served, \
             got {status}: {body}"
        );
    }
}

/// One operation that reaches a service-specific credential by id: its name and the
/// members it needs beside `UserName` and `ServiceSpecificCredentialId`.
#[derive(Debug)]
struct CredentialCall {
    action: &'static str,
    extra: &'static [(&'static str, &'static str)],
}

/// Every operation of this service addressed by a (user, credential id) pair.
const CREDENTIAL_CALLS: &[CredentialCall] = &[
    CredentialCall {
        action: "UpdateServiceSpecificCredential",
        extra: &[("Status", "Inactive")],
    },
    CredentialCall {
        action: "ResetServiceSpecificCredential",
        extra: &[],
    },
    CredentialCall {
        action: "DeleteServiceSpecificCredential",
        extra: &[],
    },
];

/// Call `call` for the pair `(user, id)`.
async fn address_pair(
    router: &MultiServiceRouter,
    credential_call: &CredentialCall,
    user: &str,
    id: &str,
) -> (u16, String) {
    let mut params: Vec<(&str, &str)> =
        vec![("UserName", user), ("ServiceSpecificCredentialId", id)];
    params.extend_from_slice(credential_call.extra);
    call(router, credential_call.action, &params).await
}

/// RULE 2: a credential is addressed by the (user, id) pair, so an id the account holds
/// under ANOTHER user fails that pair — HTTP 400 `ValidationError` naming the user and the
/// id — while an id the account holds no credential under at all is `NoSuchEntity` (HTTP
/// 404); neither answer writes anything, and the credential stays reachable under its real
/// owner.
///
/// The rows span every operation addressed by such a pair, both directions the pair can be
/// crossed in, both credential FORMS (`cassandra`/`codecommit` issue a user name and a
/// password, `bedrock`/`logs` a long-term API key — the catalog is
/// `CreateServiceSpecificCredential`'s own documentation), and an id of the modelled shape
/// that nothing minted.
#[tokio::test]
async fn a_credential_another_user_holds_fails_the_user_and_id_pair() {
    let router = router();

    // Holders spanning the two credential forms and user names from across
    // `userNameType`'s alphabet, so the isolation is exercised in both directions.
    let holders = [
        ("pair.owner", "cassandra.amazonaws.com"),
        ("Pair_Owner_Two", "bedrock.amazonaws.com"),
        ("pair+owner.three", "logs.amazonaws.com"),
    ];
    let mut held = Vec::new();
    for (user, service) in holders {
        create_user(&router, user).await;
        held.push((user, create_credential(&router, user, service).await));
    }

    for credential_call in CREDENTIAL_CALLS {
        for (user, _) in &held {
            // Every OTHER holder's credential: the account holds it, so the pair this
            // request names is what fails.
            for (owner, id) in &held {
                if owner == user {
                    continue;
                }
                let case = format!("{} by {user:?} for {owner:?}'s {id:?}", credential_call.action);
                let (status, body) = address_pair(&router, credential_call, user, id).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    VALIDATION_ERROR,
                    "case {case:?}: expected {VALIDATION_ERROR}, got {:?}: {body}",
                    error_code(&body)
                );
                assert_eq!(
                    element(&body, "Message"),
                    format!("User {user} does not have a service specific credential with id {id}"),
                    "case {case:?}: expected the pair refusal to name the user and the id, \
                     got {:?}",
                    element(&body, "Message")
                );
            }

            // An id the account holds no credential under at all is the missing entity,
            // which is what makes the pair refusal above observable as something else.
            let case = format!("{} by {user:?} for an unminted id", credential_call.action);
            let (status, body) =
                address_pair(&router, credential_call, user, UNMINTED_CREDENTIAL_ID).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                NO_SUCH_ENTITY,
                "case {case:?}: expected {NO_SUCH_ENTITY}, got {:?}: {body}",
                error_code(&body)
            );
            assert_eq!(
                element(&body, "Message"),
                format!("No such credential {UNMINTED_CREDENTIAL_ID} exists"),
                "case {case:?}: expected the missing-credential wording, got {:?}",
                element(&body, "Message")
            );
        }
    }

    // Every credential is still held by its real owner, and still `Active`: no refusal
    // deleted one, reset one or moved its status.
    for (owner, id) in &held {
        let listed = listed_ids(&router, owner).await;
        assert_eq!(
            listed,
            vec![id.clone()],
            "case {owner:?}: expected the refusals to leave the credential where it is, \
             got {listed:?}"
        );
        let (status, body) = call(
            &router,
            "ListServiceSpecificCredentials",
            &[("UserName", owner)],
        )
        .await;
        assert_eq!(
            (status, element(&body, "Status")),
            (200, "Active".to_string()),
            "case {owner:?}: expected the refused calls to leave the status alone, \
             got {status}: {body}"
        );
    }
}

/// RULE 2, the other half: the pair resolves on the user the account HOLDS the credential
/// under, not on the spelling the request happened to use — IAM entity names "are not
/// distinguished by case" — so the owner's own call is served whichever case it names
/// itself in.
#[tokio::test]
async fn the_pair_resolves_for_the_holder_in_any_case() {
    let router = router();

    // One fixture per operation, so a served call cannot change what the next row sees.
    // The rows name the holder in a case the account does not hold it in.
    let fixtures = [
        ("Owner_Update", "OWNER_update", "cassandra.amazonaws.com"),
        ("Owner_Reset", "owner_RESET", "bedrock.amazonaws.com"),
        ("Owner_Delete", "owner_delete", "codecommit.amazonaws.com"),
    ];

    for (credential_call, (stored, requested, service)) in CREDENTIAL_CALLS.iter().zip(fixtures) {
        create_user(&router, stored).await;
        let id = create_credential(&router, stored, service).await;
        let case = format!("{} by {requested:?} for {stored:?}", credential_call.action);
        let (status, body) = address_pair(&router, credential_call, requested, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the holder's own call to be served, got {status}: {body}"
        );

        // What the served call did, read back through the listing: the delete removed it,
        // the update moved its status, and the reset left it where it was.
        let listed = listed_ids(&router, stored).await;
        if credential_call.action == "DeleteServiceSpecificCredential" {
            assert!(
                listed.is_empty(),
                "case {case:?}: expected the credential to be gone, got {listed:?}"
            );
        } else {
            assert_eq!(
                listed,
                vec![id.clone()],
                "case {case:?}: expected the credential to be held still, got {listed:?}"
            );
            let (_, body) = call(
                &router,
                "ListServiceSpecificCredentials",
                &[("UserName", stored)],
            )
            .await;
            let expected = if credential_call.action == "UpdateServiceSpecificCredential" {
                "Inactive"
            } else {
                "Active"
            };
            assert_eq!(
                element(&body, "Status"),
                expected,
                "case {case:?}: expected the status {expected:?}, got {:?}",
                element(&body, "Status")
            );
        }
    }
}
