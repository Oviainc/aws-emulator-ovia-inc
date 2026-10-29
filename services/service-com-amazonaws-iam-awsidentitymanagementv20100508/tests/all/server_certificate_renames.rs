// SPDX-License-Identifier: CC0-1.0

//! `UpdateServerCertificate` — renaming and re-pathing a stored server certificate — end
//! to end over the awsQuery wire.
//!
//! RULE: an update to a stored entity's name and/or path leaves the SAME entity — nothing
//! its create minted (the id, the upload instant) and nothing it holds (its PEM material,
//! its tags, the expiration read out of the certificate) is reminted or dropped — under
//! the name and path the request leaves it under (an absent member leaves that half as it
//! stands), with its ARN spelled from those two, and the name it released resolves
//! nowhere; a new name ANOTHER entity of that type is already held under is refused with
//! `EntityAlreadyExists` (HTTP 409), a name the account holds no such entity under with
//! `NoSuchEntity` (HTTP 404), and the call is decided against the caller's identity-based
//! policies on BOTH the old and the new name before anything changes.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which of
//! the two optional members the request carries (the name, the path, both, neither), the
//! name's alphabet and length (the shortest and longest `serverCertificateNameType`
//! admits), the path's depth and length (the root, one segment, nested, the longest
//! `pathType` admits), the case the request spells the addressed name in, the key
//! algorithm the certificate was uploaded with, whether it carries a chain, how many tags
//! it holds, and — for the authorization suite — the effect and the name a statement is
//! scoped to. No single request is the specification.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, the certificate
//! material the generator `tests/all/server_certificates.rs` owns (built by `rcgen`, so
//! nothing under test chose it), and the signed-call fixture the one
//! `tests/all/identity_policy_enforcement.rs` owns.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::role_lifecycle::{borrowed, optional, param, reported_tags};
use crate::server_certificates::{expected_arn, material};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// What an upload left behind, as the SERVICE reported it: the values a later update must
/// not change.
struct Uploaded {
    /// `ServerCertificateMetadata$ServerCertificateId`, minted once by the upload.
    id: String,
    /// `ServerCertificateMetadata$UploadDate`, the instant the certificate was uploaded.
    upload_date: String,
    /// `ServerCertificateMetadata$Expiration`, read out of the certificate itself.
    expiration: String,
    /// The `CertificateBody` the upload carried, byte for byte.
    certificate: String,
    /// The `CertificateChain` the upload carried, where the row carries one.
    chain: Option<String>,
    /// The tags the upload carried, as `Tag*` reports them.
    tags: BTreeMap<String, String>,
}

/// Upload one certificate, answering what the service reported about it.
#[allow(clippy::too_many_arguments)]
async fn upload_certificate(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    path: &str,
    algorithm: &'static rcgen::SignatureAlgorithm,
    chain: bool,
    tags: &[(&str, &str)],
) -> Uploaded {
    // A validity window far enough ahead that no row depends on the wall clock.
    let leaf = material(algorithm, (2024, 2044), "server.example.com");
    let chain_pem =
        chain.then(|| material(algorithm, (2024, 2044), "issuer.example.com").certificate);
    let mut params = vec![
        param("ServerCertificateName", name),
        param("Path", path),
        param("CertificateBody", &leaf.certificate),
        param("PrivateKey", &leaf.private_key),
    ];
    if let Some(chain) = &chain_pem {
        params.push(param("CertificateChain", chain));
    }
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Tags.member.{position}.Key"), key));
        params.push(param(&format!("Tags.member.{position}.Value"), value));
    }
    let (status, body) = call(router, "UploadServerCertificate", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture UploadServerCertificate must succeed, got {status}: {body}"
    );
    Uploaded {
        id: element(&body, "ServerCertificateId"),
        upload_date: element(&body, "UploadDate"),
        expiration: element(&body, "Expiration"),
        certificate: leaf.certificate,
        chain: chain_pem,
        tags: reported_tags(&body),
    }
}

/// One `UpdateServerCertificate` call carrying only the members the row names.
async fn update(
    router: &MultiServiceRouter,
    server_certificate_name: &str,
    new_name: Option<&str>,
    new_path: Option<&str>,
) -> (u16, String) {
    let mut params = vec![("ServerCertificateName", server_certificate_name)];
    if let Some(new_name) = new_name {
        params.push(("NewServerCertificateName", new_name));
    }
    if let Some(new_path) = new_path {
        params.push(("NewPath", new_path));
    }
    call(router, "UpdateServerCertificate", &params).await
}

/// One `GetServerCertificate` read.
async fn read(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(router, "GetServerCertificate", &[("ServerCertificateName", name)]).await
}

/// One row of the update table: the certificate as uploaded, and the optional members the
/// update carries.
struct UpdateCase {
    case: &'static str,
    name: String,
    path: String,
    new_name: Option<String>,
    new_path: Option<String>,
    algorithm: &'static rcgen::SignatureAlgorithm,
    chain: bool,
    tags: Vec<(&'static str, &'static str)>,
}

/// The rows: every combination of the two optional members (name only, path only, both,
/// neither), over names spanning the `^[\w+=,.@-]+$` alphabet and the shortest and longest
/// lengths `serverCertificateNameType` admits (1 and 128 characters), and paths spanning
/// the account root, one segment, a nested path and the longest `pathType` admits (512
/// characters) — `smithy_query(query="operation", operation="UpdateServerCertificate")` —
/// each with a different key algorithm, chain and tag count, because what an update must
/// not disturb is the same whatever the certificate holds.
fn update_cases() -> Vec<UpdateCase> {
    // The longest path the model admits: 512 characters, both slashes included.
    let longest_path = format!("/{}/", "p".repeat(510));
    // The longest name it admits: 128 characters.
    let longest_name = "n".repeat(128);
    vec![
        UpdateCase {
            case: "a rename alone",
            name: "web-server".to_string(),
            path: "/".to_string(),
            new_name: Some("WebServerCert".to_string()),
            new_path: None,
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            chain: false,
            tags: Vec::new(),
        },
        UpdateCase {
            case: "a re-path alone",
            name: "edge".to_string(),
            path: "/cloudfront/".to_string(),
            new_name: None,
            new_path: Some("/cloudfront/east/".to_string()),
            algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
            chain: true,
            tags: vec![("env", "prod")],
        },
        UpdateCase {
            case: "a rename and a re-path at once",
            name: "Cert_7+x=y,z.w@example-1".to_string(),
            path: "/eng/".to_string(),
            new_name: Some("renamed+9=a,b.c@example-2".to_string()),
            new_path: Some("/eng/tools/platform/".to_string()),
            algorithm: &rcgen::PKCS_ED25519,
            chain: true,
            tags: vec![("zeta", "last"), ("alpha", "first"), ("mu", "middle")],
        },
        UpdateCase {
            case: "neither member, so nothing changes",
            name: "unchanged".to_string(),
            path: "/sales/west/".to_string(),
            new_name: None,
            new_path: None,
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            chain: false,
            tags: vec![("owner", "platform"), ("env", "test")],
        },
        UpdateCase {
            case: "a re-spelling of the certificate's own name",
            name: "mixedcase".to_string(),
            path: "/".to_string(),
            new_name: Some("MIXEDCASE".to_string()),
            new_path: None,
            algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
            chain: true,
            tags: vec![("alpha", "1")],
        },
        UpdateCase {
            case: "the shortest name the model admits",
            name: "c".to_string(),
            path: "/legacy/".to_string(),
            new_name: Some("d".to_string()),
            new_path: Some("/".to_string()),
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            chain: false,
            tags: Vec::new(),
        },
        UpdateCase {
            case: "to the longest name the model admits",
            name: "short".to_string(),
            path: "/".to_string(),
            new_name: Some(longest_name),
            new_path: None,
            algorithm: &rcgen::PKCS_ED25519,
            chain: false,
            tags: vec![("env", "prod")],
        },
        UpdateCase {
            case: "to the longest path the model admits",
            name: "deep".to_string(),
            path: "/".to_string(),
            new_name: Some("deeper".to_string()),
            new_path: Some(longest_path),
            algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
            chain: true,
            tags: vec![("mu", "middle")],
        },
    ]
}

/// RULE: the update leaves the SAME certificate under the name and path it names — the
/// minted id, the upload instant, the expiration, the PEM material and the tags unchanged,
/// the ARN spelled from the name and path it now carries — and the name it no longer
/// carries resolves nowhere.
///
/// The rows are [`update_cases`]. Each addressed certificate is named in UPPER case,
/// because IAM entity names "are not distinguished by case", so the case a request spells
/// the addressed name in is not part of what it identifies.
#[tokio::test]
async fn an_update_moves_the_same_server_certificate_to_the_name_and_path_it_names() {
    for UpdateCase {
        case,
        name,
        path,
        new_name,
        new_path,
        algorithm,
        chain,
        tags,
    } in update_cases()
    {
        // A fresh account a row at a time, so the row's certificate is the only one held
        // and "the account holds exactly this certificate afterwards" is assertable.
        let router = router();
        let uploaded =
            upload_certificate(&router, case, &name, &path, algorithm, chain, &tags).await;

        let addressed = name.to_ascii_uppercase();
        let (status, body) = update(
            &router,
            &addressed,
            new_name.as_deref(),
            new_path.as_deref(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateServerCertificate 200 for a held certificate \
             spelled {addressed:?}, got {status}: {body}"
        );

        // An absent member leaves that half of the certificate's identity as it stands.
        let expected_name = new_name.clone().unwrap_or_else(|| name.clone());
        let expected_path = new_path.clone().unwrap_or_else(|| path.clone());

        let (status, got) = read(&router, &expected_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated certificate to read back under \
             {expected_name:?}, got {status}: {got}"
        );
        for (member, expected) in [
            ("ServerCertificateName", expected_name.clone()),
            ("Path", expected_path.clone()),
            ("Arn", expected_arn(&expected_path, &expected_name)),
            // The id and the upload instant identify the certificate over its lifetime,
            // and the expiration is the uploaded certificate's own `notAfter`, so an
            // update must remint none of them.
            ("ServerCertificateId", uploaded.id.clone()),
            ("UploadDate", uploaded.upload_date.clone()),
            ("Expiration", uploaded.expiration.clone()),
            ("CertificateBody", uploaded.certificate.clone()),
        ] {
            let actual = element(&got, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?} after the update, got {actual:?}"
            );
        }
        assert_eq!(
            optional(&got, "CertificateChain"),
            uploaded.chain,
            "case {case:?}: expected the update to keep the uploaded chain, got {got}"
        );
        assert_eq!(
            reported_tags(&got),
            uploaded.tags,
            "case {case:?}: expected the update to keep the certificate's tags, got {got}"
        );
        let (status, listed_tags) = call(
            &router,
            "ListServerCertificateTags",
            &[("ServerCertificateName", &expected_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListServerCertificateTags 200 under the name the \
             certificate now carries, got {status}: {listed_tags}"
        );
        assert_eq!(
            reported_tags(&listed_tags),
            uploaded.tags,
            "case {case:?}: expected the tag listing to follow the update, got {listed_tags}"
        );

        // One certificate, not two: the update MOVED the record rather than copying it.
        let (status, listed) = call(&router, "ListServerCertificates", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListServerCertificates 200, got {status}: {listed}"
        );
        assert_eq!(
            elements(&listed, "ServerCertificateName"),
            vec![expected_name.clone()],
            "case {case:?}: expected the account to hold exactly the updated certificate, \
             got {listed}"
        );
        assert_eq!(
            elements(&listed, "ServerCertificateId"),
            vec![uploaded.id.clone()],
            "case {case:?}: expected the listed certificate to keep its minted id, got {listed}"
        );
        assert_eq!(
            elements(&listed, "Path"),
            vec![expected_path.clone()],
            "case {case:?}: expected the listing to report the path the update left, got \
             {listed}"
        );

        // The name the certificate no longer carries resolves nowhere — unless the update
        // only re-spelled the name it already had, which is the same name: IAM entity
        // names "are not distinguished by case".
        let (status, body) = read(&router, &name).await;
        if name.eq_ignore_ascii_case(&expected_name) {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the re-spelled name to still name the certificate, \
                 got {status}: {body}"
            );
            assert_eq!(
                element(&body, "ServerCertificateName"),
                expected_name,
                "case {case:?}: expected the stored spelling to be the updated one, got {body}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: expected the released name {name:?} to resolve nowhere, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
        }
    }
}

/// RULE: a new name ANOTHER entity of the same type is already held under is refused with
/// `EntityAlreadyExists` (HTTP 409) rather than replacing it, and the refused update
/// changes nothing; the case the new name is spelled in does not matter, because IAM
/// entity names "are not distinguished by case".
///
/// The rows differ along the case the taken name is spelled in, the path the other
/// certificate sits at (so the conflict is about the NAME and not the ARN), and whether the
/// refused update also asked for a new path.
#[tokio::test]
async fn a_new_name_another_server_certificate_already_carries_is_refused() {
    // `(the spelling the update asks for, the other certificate's path, the new path asked
    // for)`.
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("auditors", "/", None),
        ("AUDITORS", "/", None),
        ("AuDiToRs", "/compliance/", None),
        ("auditors", "/compliance/deep/", Some("/moved/")),
    ];

    for (spelling, other_path, new_path) in rows {
        let case = format!("{spelling} at {other_path}");
        let router = router();
        let held = upload_certificate(
            &router,
            &case,
            "frontend",
            "/eng/",
            &rcgen::PKCS_ECDSA_P256_SHA256,
            false,
            &[],
        )
        .await;
        let other = upload_certificate(
            &router,
            &case,
            "auditors",
            other_path,
            &rcgen::PKCS_ECDSA_P384_SHA384,
            true,
            &[("env", "prod")],
        )
        .await;

        let (status, body) = update(&router, "frontend", Some(spelling), *new_path).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected UpdateServerCertificate 409 for a name another \
             certificate carries, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The Server Certificate with name {spelling} already exists."),
            "case {case:?}: expected the refusal to name the entity and the name the request \
             carried, got {:?}",
            element(&body, "Message")
        );

        // The refused update changed nothing: both certificates read back as their uploads
        // left them.
        for (name, path, uploaded) in [
            ("frontend", "/eng/", &held),
            ("auditors", other_path, &other),
        ] {
            let (status, got) = read(&router, name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {name:?} must survive the refused update, got {status}: {got}"
            );
            for (member, expected) in [
                ("ServerCertificateName", name.to_string()),
                ("Path", path.to_string()),
                ("Arn", expected_arn(path, name)),
                ("ServerCertificateId", uploaded.id.clone()),
                ("CertificateBody", uploaded.certificate.clone()),
            ] {
                let actual = element(&got, member);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: the refused update changed {name:?}'s {member}: expected \
                     {expected:?}, got {actual:?}"
                );
            }
        }
    }
}

/// RULE: a name the account holds no entity of that type under names no entity, so the
/// update is refused with `NoSuchEntity` (HTTP 404) and creates nothing.
///
/// The rows span names the account never held — including near-misses of a held one — and
/// each combination of the optional members, because a name that names no certificate is
/// refused whatever the update asked to change.
#[tokio::test]
async fn an_update_of_a_name_the_account_holds_no_server_certificate_under_is_refused() {
    let router = router();
    upload_certificate(
        &router,
        "the fixture",
        "frontend",
        "/eng/",
        &rcgen::PKCS_ECDSA_P256_SHA256,
        false,
        &[],
    )
    .await;

    for absent in ["nobody", "frontend1", "rontend", "front-end"] {
        for (new_name, new_path) in [
            (Some("renamed"), None),
            (None, Some("/moved/")),
            (Some("renamed"), Some("/moved/")),
            (None, None),
        ] {
            let case = format!("{absent} -> {new_name:?} at {new_path:?}");
            let (status, body) = update(&router, absent, new_name, new_path).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected UpdateServerCertificate 404 for an unheld name, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            assert_eq!(
                element(&body, "Message"),
                format!("The Server Certificate with name {absent} cannot be found."),
                "case {case:?}: expected the refusal to name the entity and the name the \
                 request carried, got {:?}",
                element(&body, "Message")
            );

            // The refused update created nothing, under either name.
            for name in [absent, new_name.unwrap_or(absent)] {
                let (status, got) = read(&router, name).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the refused update to leave {name:?} naming no \
                     certificate, got {status}: {got}"
                );
            }
        }
    }
}

/// What a row of the authorization table expects the signed update to answer.
#[derive(Debug, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force permit the call on both names.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// One row of the authorization table: the inline policies the calling user holds, whether
/// the update asks for a new name at all, the answer the rule requires, and — for a
/// refusal — the certificate the refusal must NAME, which is the one of the two names the
/// decision went against.
struct AuthorizationCase {
    case: &'static str,
    policies: Vec<(String, String)>,
    renames: bool,
    expect: Expect,
    refused_on: &'static str,
}

/// The certificate the fixture account holds, and the name an update moves it to.
const HELD_CERTIFICATE: &str = "payroll";
const NEW_CERTIFICATE: &str = "finance";

/// The rows: an `Allow` over every resource, one scoped to each of the two names alone, one
/// scoped to both, none at all, one for another action, and a `Deny` scoped to each of the
/// two names on top of a blanket `Allow` — the dimensions the "both names" rule generalises
/// over.
fn authorization_cases() -> Vec<AuthorizationCase> {
    let action = "iam:UpdateServerCertificate";
    let held_arn = expected_arn("/", HELD_CERTIFICATE);
    let new_arn = expected_arn("/", NEW_CERTIFICATE);
    vec![
        AuthorizationCase {
            case: "an Allow naming the action on every resource",
            policies: vec![("allow-all".to_string(), statement("Allow", action, "*"))],
            renames: true,
            expect: Expect::Served,
            refused_on: HELD_CERTIFICATE,
        },
        AuthorizationCase {
            case: "an Allow scoped to both the old and the new name",
            policies: vec![
                ("allow-old".to_string(), statement("Allow", action, &held_arn)),
                ("allow-new".to_string(), statement("Allow", action, &new_arn)),
            ],
            renames: true,
            expect: Expect::Served,
            refused_on: HELD_CERTIFICATE,
        },
        AuthorizationCase {
            case: "an Allow scoped to the old name only",
            policies: vec![("allow-old".to_string(), statement("Allow", action, &held_arn))],
            renames: true,
            expect: Expect::NoAllow,
            refused_on: NEW_CERTIFICATE,
        },
        AuthorizationCase {
            case: "an Allow scoped to the new name only",
            policies: vec![("allow-new".to_string(), statement("Allow", action, &new_arn))],
            renames: true,
            expect: Expect::NoAllow,
            refused_on: HELD_CERTIFICATE,
        },
        AuthorizationCase {
            case: "an Allow scoped to the old name, for an update that renames nothing",
            policies: vec![("allow-old".to_string(), statement("Allow", action, &held_arn))],
            renames: false,
            expect: Expect::Served,
            refused_on: HELD_CERTIFICATE,
        },
        AuthorizationCase {
            case: "no identity-based policy at all",
            policies: Vec::new(),
            renames: true,
            expect: Expect::NoAllow,
            refused_on: HELD_CERTIFICATE,
        },
        AuthorizationCase {
            case: "an Allow for another action only",
            policies: vec![(
                "allow-upload".to_string(),
                statement("Allow", "iam:UploadServerCertificate", "*"),
            )],
            renames: true,
            expect: Expect::NoAllow,
            refused_on: HELD_CERTIFICATE,
        },
        AuthorizationCase {
            case: "a Deny scoped to the old name on top of a blanket Allow",
            policies: vec![
                ("allow-all".to_string(), statement("Allow", action, "*")),
                ("deny-old".to_string(), statement("Deny", action, &held_arn)),
            ],
            renames: true,
            expect: Expect::ExplicitDeny,
            refused_on: HELD_CERTIFICATE,
        },
        AuthorizationCase {
            case: "a Deny scoped to the new name on top of a blanket Allow",
            policies: vec![
                ("allow-all".to_string(), statement("Allow", action, "*")),
                ("deny-new".to_string(), statement("Deny", action, &new_arn)),
            ],
            renames: true,
            expect: Expect::ExplicitDeny,
            refused_on: NEW_CERTIFICATE,
        },
    ]
}

/// RULE: the call is decided against the caller's identity-based policies on BOTH the name
/// the entity carries and the name the update would leave it under — the principal must be
/// permitted on each of them — and the decision is reached before anything changes, so a
/// refused update leaves the entity exactly as it was.
#[tokio::test]
async fn an_update_is_decided_against_the_old_and_the_new_name() {
    let iam_action = "iam:UpdateServerCertificate";

    for (
        row,
        AuthorizationCase {
            case,
            policies,
            renames,
            expect,
            refused_on,
        },
    ) in authorization_cases().into_iter().enumerate()
    {
        // A fresh account a row at a time, so a row's policies are the only documents in
        // force when it calls.
        let router = router();
        let uploaded = upload_certificate(
            &router,
            case,
            HELD_CERTIFICATE,
            "/",
            &rcgen::PKCS_ECDSA_P256_SHA256,
            false,
            &[],
        )
        .await;

        let user_name = format!("caller{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        let mut params = vec![("ServerCertificateName", HELD_CERTIFICATE)];
        if renames {
            params.push(("NewServerCertificateName", NEW_CERTIFICATE));
        }
        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "UpdateServerCertificate",
            &params,
        )
        .await;

        if expect == Expect::Served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be served, got {status}: {body}"
            );
            let expected_name = if renames {
                NEW_CERTIFICATE
            } else {
                HELD_CERTIFICATE
            };
            let (status, got) = read(&router, expected_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the served update to leave the certificate under \
                 {expected_name:?}, got {status}: {got}"
            );
            assert_eq!(
                element(&got, "ServerCertificateId"),
                uploaded.id,
                "case {case:?}: expected the served update to keep the minted id, got {got}"
            );
            continue;
        }

        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected the error code AccessDenied, got {:?}",
            error_code(&body)
        );
        let reason = if expect == Expect::ExplicitDeny {
            "with an explicit deny in an identity-based policy".to_string()
        } else {
            format!("because no identity-based policy allows the {iam_action} action")
        };
        assert_access_denied_message(
            case,
            &element(&body, "Message"),
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            iam_action,
            &format!("server certificate {refused_on}"),
            &reason,
        );

        // The refused update changed nothing: the certificate is still held under the name
        // and path its upload gave it, and the name it was asked to move to names none.
        let (status, got) = read(&router, HELD_CERTIFICATE).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the certificate must survive the refused update, got {status}: \
             {got}"
        );
        for (member, expected) in [
            ("ServerCertificateName", HELD_CERTIFICATE.to_string()),
            ("Path", "/".to_string()),
            ("Arn", expected_arn("/", HELD_CERTIFICATE)),
            ("ServerCertificateId", uploaded.id.clone()),
            ("UploadDate", uploaded.upload_date.clone()),
        ] {
            let actual = element(&got, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: the refused update changed {member}: expected {expected:?}, \
                 got {actual:?}"
            );
        }
        let (status, got) = read(&router, NEW_CERTIFICATE).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused update to leave {NEW_CERTIFICATE:?} naming \
             no certificate, got {status}: {got}"
        );
    }
}
