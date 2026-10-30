// SPDX-License-Identifier: CC0-1.0

//! IAM X.509 signing certificates, end to end over the awsQuery wire:
//! `UploadSigningCertificate` and its reporting half `ListSigningCertificates`, plus
//! `UpdateSigningCertificate` and `DeleteSigningCertificate`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a certificate that was
//! PERSISTED, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The rules under test are that an upload PERSISTS the certificate under an id minted
//! once, against the IAM user the request names (or, naming none, the user the signing
//! key belongs to), with the body kept verbatim and the status `Active`; that the list
//! REPORTS exactly what was persisted, one page at a time, for that user alone; that an
//! update moves ONE record's status and nothing else; that a delete removes ONE record
//! and nothing else; that a certificate is registered ONCE in the account, so its holder
//! re-uploading it is served the registration it already has while any other user is
//! refused; and that a body that is not a certificate and an upload past the published
//! quota are refused with nothing stored.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over (the
//! user name's alphabet and case, the case the request spells it in, the key algorithm,
//! the certificate's validity window, how many certificates the user already holds, and
//! whether the request names a user at all), so no single request is the specification.
//! Every certificate the service is fed is built by `rcgen` — something other than the
//! code under test — so a reported body that is not the uploaded one cannot pass.

use std::collections::{BTreeMap, BTreeSet};

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// How many signing certificates one IAM user may hold, from AWS's "IAM and STS quotas"
/// page ("Signing certificates per IAM user: 2") — the quota the service enforces.
///
/// Restated here as the published number rather than imported from the crate's own
/// `state::MAX_SIGNING_CERTIFICATES_PER_USER`: importing it would make the check
/// unfalsifiable, since the test would then follow the code wherever it went.
const MAX_SIGNING_CERTIFICATES_PER_USER: usize = 2;

/// One row of the table: the user the account holds, the case the request spells that
/// user's name in, and the shape of the certificate it uploads.
///
/// The two names differ on most rows because IAM entity names "are not distinguished by
/// case" (`CreateUser$UserName`), so a request may name the user in any case while the
/// response must report the name as the account holds it.
struct Case {
    /// The name the user is created with — the case IAM stores and reports.
    stored_name: &'static str,
    /// The name the signing-certificate call spells; the same entity, differing only in
    /// case.
    requested_name: &'static str,
    /// The signature algorithm the generated key pair uses, so the DER the service reads
    /// differs from row to row.
    algorithm: &'static rcgen::SignatureAlgorithm,
    /// The generated certificate's validity window, as `(not_before, not_after)` years.
    validity: (i32, i32),
}

/// Rows spanning the `^[\w+=,.@-]+$` alphabet the user-name constraint admits
/// (`smithy_query(query="shape", shape="existingUserNameType")`), both cases of the
/// stored name, requests that fold the name up, down and not at all, three key
/// algorithms, and validity windows of one, ten and twenty years — every one of them
/// opening and closing far enough from today that no row depends on the wall clock.
const CASES: &[Case] = &[
    Case {
        stored_name: "alice",
        requested_name: "alice",
        algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
        validity: (2020, 2030),
    },
    Case {
        stored_name: "Bob",
        requested_name: "bob",
        algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
        validity: (2021, 2031),
    },
    Case {
        stored_name: "carol_7",
        requested_name: "CAROL_7",
        algorithm: &rcgen::PKCS_ED25519,
        validity: (2024, 2044),
    },
    Case {
        stored_name: "dave.x+1",
        requested_name: "DAVE.X+1",
        algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
        validity: (2029, 2039),
    },
    Case {
        stored_name: "eve=2@4",
        requested_name: "eve=2@4",
        algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
        validity: (2040, 2041),
    },
];

/// The status a freshly uploaded certificate carries: "when you upload the certificate,
/// its default status is Active" (`UploadSigningCertificate`).
const ACTIVE: &str = "Active";

/// The other status `UpdateSigningCertificate` documents moving a certificate to:
/// "`Inactive` means that the certificate cannot be used".
const INACTIVE: &str = "Inactive";

/// Generate a real self-signed X.509 certificate, PEM-encoded, from a fresh key pair —
/// so every call yields a DIFFERENT certificate even for the same subject.
///
/// Shared with the suites that need a well-formed certificate for a refusal that happens
/// AFTER the certificate is read (`tests/all/implicit_user_and_credential_pair_refusals.rs`),
/// so the two cannot drift apart on what a valid `CertificateBody` is.
pub(crate) fn certificate_pem(
    algorithm: &'static rcgen::SignatureAlgorithm,
    validity: (i32, i32),
    common_name: &str,
) -> String {
    let key = rcgen::KeyPair::generate_for(algorithm).expect("a key pair");
    let mut params =
        rcgen::CertificateParams::new(vec![common_name.to_string()]).expect("certificate parameters");
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(rcgen::DnType::CommonName, common_name);
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(validity.0, 1, 1);
    params.not_after = rcgen::date_time_ymd(validity.1, 1, 1);
    params
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem()
}

impl Case {
    /// A certificate for this row, distinguished from the row's other certificates by
    /// `ordinal` in its subject.
    fn certificate(&self, ordinal: usize) -> String {
        certificate_pem(
            self.algorithm,
            self.validity,
            &format!("{}-{ordinal}.example.com", self.stored_name.to_lowercase()),
        )
    }

    /// Create this row's user, and report the case the account then holds.
    async fn with_user(&self, router: &smithy_server_runtime::MultiServiceRouter) {
        let (status, body) = call(router, "CreateUser", &[("UserName", self.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateUser must succeed, got {status}: {body}",
            self.stored_name
        );
    }
}

/// Assert that `certificate_id` satisfies `certificateIdType`: length 24..128 and the
/// pattern `^[\w]+$` (`smithy_query(query="shape", shape="certificateIdType")`).
fn assert_modelled_certificate_id(case: &str, certificate_id: &str) {
    let length = certificate_id.chars().count();
    assert!(
        (24..=128).contains(&length),
        "case {case:?}: expected a CertificateId of 24..128 characters, got {certificate_id:?}"
    );
    assert!(
        certificate_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_'),
        "case {case:?}: expected a CertificateId matching ^[\\w]+$, got {certificate_id:?}"
    );
}

/// Every `SigningCertificate` the body reports, as `CertificateId -> (UserName, Status,
/// CertificateBody)`.
///
/// The members of one list entry arrive in document order, so the four element scans line
/// up position by position.
fn reported_certificates(body: &str) -> BTreeMap<String, (String, String, String)> {
    let ids = elements(body, "CertificateId");
    let names = elements(body, "UserName");
    let statuses = elements(body, "Status");
    let bodies = elements(body, "CertificateBody");
    assert!(
        ids.len() == names.len() && ids.len() == statuses.len() && ids.len() == bodies.len(),
        "every reported certificate must carry each required member, got {body}"
    );
    ids.into_iter()
        .zip(names)
        .zip(statuses)
        .zip(bodies)
        .map(|(((id, name), status), certificate)| (id, (name, status, certificate)))
        .collect()
}

/// Upload `certificate` for `user_name`, asserting the call succeeds, and report the
/// response body.
async fn upload(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    user_name: &str,
    certificate: &str,
) -> String {
    let params = vec![
        param("UserName", user_name),
        param("CertificateBody", certificate),
    ];
    let (status, body) = call(router, "UploadSigningCertificate", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UploadSigningCertificate, got {status}: {body}"
    );
    body
}

// ---------------------------------------------------------------------------
// Upload, then read back
// ---------------------------------------------------------------------------

/// RULE: an upload PERSISTS the certificate against the IAM user the request names,
/// reporting every required member of the `SigningCertificate` shape — the stored user's
/// name, an id inside `certificateIdType`, the body verbatim, the status `Active` and the
/// instant it was uploaded — and `ListSigningCertificates` then reports that same record.
#[tokio::test]
async fn an_uploaded_certificate_round_trips_through_list_signing_certificates() {
    let router = router();
    let mut minted_ids: BTreeSet<String> = BTreeSet::new();

    for case in CASES {
        let label = format!("{} requested as {}", case.stored_name, case.requested_name);
        case.with_user(&router).await;
        let certificate = case.certificate(1);

        let uploaded = upload(&router, &label, case.requested_name, &certificate).await;
        assert_eq!(
            element(&uploaded, "UserName"),
            case.stored_name,
            "case {label:?}: the upload must report the user name the account holds, got \
             {uploaded}"
        );
        assert_eq!(
            element(&uploaded, "Status"),
            ACTIVE,
            "case {label:?}: a freshly uploaded certificate must be {ACTIVE}, got {uploaded}"
        );
        assert_eq!(
            element(&uploaded, "CertificateBody"),
            certificate,
            "case {label:?}: the upload must report the body it was given, got {uploaded}"
        );
        let certificate_id = element(&uploaded, "CertificateId");
        assert_modelled_certificate_id(&label, &certificate_id);
        assert!(
            minted_ids.insert(certificate_id.clone()),
            "case {label:?}: CertificateId {certificate_id:?} was already minted for another \
             certificate"
        );
        let upload_date = element(&uploaded, "UploadDate");
        assert!(
            aws_smithy_types::DateTime::from_str(
                &upload_date,
                aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {label:?}: expected an ISO-8601 UploadDate, got {upload_date:?}"
        );

        // The read reports the SAME record, resolved through the user that holds it.
        let (status, listed) = call(
            &router,
            "ListSigningCertificates",
            &[("UserName", case.requested_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from ListSigningCertificates, got {status}: \
             {listed}"
        );
        assert_eq!(
            reported_certificates(&listed),
            BTreeMap::from([(
                certificate_id.clone(),
                (
                    case.stored_name.to_string(),
                    ACTIVE.to_string(),
                    certificate.clone(),
                ),
            )]),
            "case {label:?}: the list must report exactly what was persisted, got {listed}"
        );
        assert_eq!(
            element(&listed, "UploadDate"),
            upload_date,
            "case {label:?}: the list must report the stored upload date, got {listed}"
        );
        assert_eq!(
            element(&listed, "IsTruncated"),
            "false",
            "case {label:?}: one certificate within the default page size is not truncated, \
             got {listed}"
        );

        // Listing again cannot change what the next caller sees.
        let (status, again) = call(
            &router,
            "ListSigningCertificates",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            (status, reported_certificates(&again)),
            (200, reported_certificates(&listed)),
            "case {label:?}: a second read answered differently: {again}"
        );
    }
}

/// RULE: with no `UserName` the certificate belongs to the IAM user the request was
/// signed by, resolved from the signing access key id — and the same resolution answers
/// the list, the update and the delete.
#[tokio::test]
async fn an_omitted_name_resolves_the_signing_user() {
    let router = router();

    for case in CASES {
        case.with_user(&router).await;
        let (status, created) = call(
            &router,
            "CreateAccessKey",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the signing key must be minted, got {status}: {created}",
            case.stored_name
        );
        let signing_key = element(&created, "AccessKeyId");

        let certificate = case.certificate(1);
        let params = vec![param("CertificateBody", &certificate)];
        let (status, uploaded) = call_signed_by(
            &router,
            Some(&signing_key),
            "UploadSigningCertificate",
            &borrowed(&params),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the signing user's own upload to be served, got {status}: \
             {uploaded}",
            case.stored_name
        );
        assert_eq!(
            element(&uploaded, "UserName"),
            case.stored_name,
            "case {:?}: the certificate must belong to the signing user, got {uploaded}",
            case.stored_name
        );
        let certificate_id = element(&uploaded, "CertificateId");

        let (status, listed) =
            call_signed_by(&router, Some(&signing_key), "ListSigningCertificates", &[]).await;
        assert_eq!(
            (status, reported_certificates(&listed)),
            (
                200,
                BTreeMap::from([(
                    certificate_id.clone(),
                    (
                        case.stored_name.to_string(),
                        ACTIVE.to_string(),
                        certificate.clone(),
                    ),
                )])
            ),
            "case {:?}: the nameless list must report the signing user's certificate, got \
             {listed}",
            case.stored_name
        );

        // The same resolution carries the update and the delete.
        let (status, updated) = call_signed_by(
            &router,
            Some(&signing_key),
            "UpdateSigningCertificate",
            &[("CertificateId", &certificate_id), ("Status", INACTIVE)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the nameless update to be served, got {status}: {updated}",
            case.stored_name
        );
        let (_, listed) =
            call_signed_by(&router, Some(&signing_key), "ListSigningCertificates", &[]).await;
        assert_eq!(
            reported_certificates(&listed)
                .get(&certificate_id)
                .map(|(_, status, _)| status.clone()),
            Some(INACTIVE.to_string()),
            "case {:?}: the nameless update must have moved the status, got {listed}",
            case.stored_name
        );
        let (status, deleted) = call_signed_by(
            &router,
            Some(&signing_key),
            "DeleteSigningCertificate",
            &[("CertificateId", &certificate_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the nameless delete to be served, got {status}: {deleted}",
            case.stored_name
        );
        let (_, listed) =
            call_signed_by(&router, Some(&signing_key), "ListSigningCertificates", &[]).await;
        assert!(
            reported_certificates(&listed).is_empty(),
            "case {:?}: the deleted certificate must be gone, got {listed}",
            case.stored_name
        );
    }
}

/// RULE: credentials that belong to no IAM user resolve to no user, so a request naming
/// none is refused rather than having an identity invented for it, and nothing is stored.
#[tokio::test]
async fn an_omitted_name_refuses_non_user_credentials() {
    let router = router();
    let certificate = certificate_pem(
        &rcgen::PKCS_ECDSA_P256_SHA256,
        (2024, 2034),
        "nobody.example.com",
    );

    // Rows spanning no signing key at all, the fallback id an unsigned request resolves
    // to, a key id of the modelled shape this emulator never issued, and an id that names
    // no credential.
    let signers: &[Option<&str>] = &[
        None,
        Some("default"),
        Some("AKIAIOSFODNN7EXAMPLE"),
        Some("EMULATORAKAAAAAAAAAA"),
    ];

    for signer in signers {
        let label = format!("signed by {signer:?}");
        let params = vec![param("CertificateBody", &certificate)];
        let (status, refused) = call_signed_by(
            &router,
            *signer,
            "UploadSigningCertificate",
            &borrowed(&params),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status}: {refused}"
        );
        // The modelled behaviour under test is the REFUSAL: a 400 storing nothing, under
        // the error code IAM answers a request it cannot accept with, because this
        // operation models no error for a request that named no entity.
        assert_eq!(
            error_code(&refused),
            "ValidationError",
            "case {label:?}: expected IAM's validation refusal code, got {:?}",
            error_code(&refused)
        );
        assert!(
            elements(&refused, "CertificateId").is_empty(),
            "case {label:?}: expected nothing to be stored, got {refused}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals that store nothing
// ---------------------------------------------------------------------------

/// RULE: a certificate is registered ONCE in the account, and an upload naming the user it
/// is already associated with asks for the registration that exists: it is served with that
/// registration exactly as it stands — the same `CertificateId`, body, status and upload
/// date — stores nothing new, and recognises the certificate whatever whitespace framed the
/// PEM.
#[tokio::test]
async fn re_uploading_a_held_certificate_reports_the_held_registration() {
    let router = router();

    for case in CASES {
        let label = format!("{} requested as {}", case.stored_name, case.requested_name);
        case.with_user(&router).await;
        let certificate = case.certificate(1);
        let uploaded = upload(&router, &label, case.stored_name, &certificate).await;
        let certificate_id = element(&uploaded, "CertificateId");
        let upload_date = element(&uploaded, "UploadDate");

        // The body as it was sent, the same certificate with the newline after its END
        // line added or removed, and the same certificate with whitespace around the
        // framing: the same CERTIFICATE every time, which is what the registration is of.
        let repeats = [
            certificate.clone(),
            format!("{certificate}\n"),
            certificate.trim_end().to_string(),
            format!("\n  {}  \n", certificate.trim()),
        ];
        for (index, repeat) in repeats.iter().enumerate() {
            let params = vec![
                param("UserName", case.requested_name),
                param("CertificateBody", repeat),
            ];
            let (status, again) = call(&router, "UploadSigningCertificate", &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {label:?}: repeat {index} must be served, got {status}: {again}"
            );
            assert_eq!(
                element(&again, "CertificateId"),
                certificate_id,
                "case {label:?}: repeat {index} must report the held registration's id, got \
                 {again}"
            );
            assert_eq!(
                element(&again, "UserName"),
                case.stored_name,
                "case {label:?}: repeat {index} must report the holder, got {again}"
            );
            assert_eq!(
                element(&again, "Status"),
                ACTIVE,
                "case {label:?}: repeat {index} must report the held status, got {again}"
            );
            assert_eq!(
                element(&again, "UploadDate"),
                upload_date,
                "case {label:?}: repeat {index} must report the date the certificate was \
                 first uploaded, got {again}"
            );
            assert_eq!(
                element(&again, "CertificateBody"),
                certificate,
                "case {label:?}: repeat {index} must report the body as it is held, got {again}"
            );
        }

        // The held certificate is untouched, and still the only one: the repeats stored
        // nothing, so the user's set did not grow past the one registration.
        let (_, listed) = call(
            &router,
            "ListSigningCertificates",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            reported_certificates(&listed),
            BTreeMap::from([(
                certificate_id.clone(),
                (
                    case.stored_name.to_string(),
                    ACTIVE.to_string(),
                    certificate.clone(),
                ),
            )]),
            "case {label:?}: the repeats must leave the one held certificate, got {listed}"
        );

        // A DIFFERENT certificate for the same user is a different resource and is stored.
        let second = case.certificate(2);
        let stored = upload(&router, &label, case.requested_name, &second).await;
        assert_ne!(
            element(&stored, "CertificateId"),
            certificate_id,
            "case {label:?}: a different certificate must get an id of its own, got {stored}"
        );
    }
}

/// One user asking for a certificate: the name the account holds it under, the case its
/// own request spells that name in, and whether the request names the user at all.
struct Claimant {
    /// The name the user is created with — the case IAM stores and reports.
    stored: &'static str,
    /// The case this user's upload spells its name in; the same entity, differing only in
    /// case, because IAM entity names "are not distinguished by case".
    spelled: &'static str,
    /// When true the upload carries NO `UserName` and is signed by this user's own access
    /// key, so the user is the implicitly resolved caller rather than a named one.
    implicit: bool,
}

/// One row: the user that registers the certificate first, the users that then ask for the
/// SAME certificate, and the shape of that certificate.
struct ClaimCase {
    label: &'static str,
    holder: Claimant,
    others: &'static [Claimant],
    algorithm: &'static rcgen::SignatureAlgorithm,
    validity: (i32, i32),
}

/// The message IAM refuses an upload of an already registered certificate with: entirely
/// fixed text, naming neither the certificate nor the user that holds it.
const DUPLICATE_MESSAGE: &str = "Duplicate certificate uploaded.";

/// Create `user_name`, asserting the fixture call succeeds.
async fn with_user_named(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    user_name: &str,
) {
    let (status, body) = call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateUser {user_name:?} must succeed, got {status}: {body}"
    );
}

/// The access key this claimant's uploads are signed by, or `None` when its uploads name
/// the user outright. Minted ONCE per claimant, so a row that makes several uploads does
/// not spend the per-user access-key quota.
async fn signing_key_for(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    claimant: &Claimant,
    row: usize,
) -> Option<String> {
    if !claimant.implicit {
        return None;
    }
    let stored = format!("{}-{row}", claimant.stored);
    let (status, key) = call(router, "CreateAccessKey", &[("UserName", &stored)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the signing key for {stored:?} must be minted, got {status}: {key}"
    );
    Some(element(&key, "AccessKeyId"))
}

/// Upload `certificate` for `claimant` — naming the user, or, when the row leaves the name
/// implicit, carrying no `UserName` and signed by that user's own access key.
async fn upload_claim(
    router: &smithy_server_runtime::MultiServiceRouter,
    claimant: &Claimant,
    row: usize,
    signing_key: Option<&String>,
    certificate: &str,
) -> (u16, String) {
    match signing_key {
        Some(key) => {
            let params = vec![param("CertificateBody", certificate)];
            call_signed_by(
                router,
                Some(key),
                "UploadSigningCertificate",
                &borrowed(&params),
            )
            .await
        }
        None => {
            let spelled = format!("{}-{row}", claimant.spelled);
            let params = vec![
                param("UserName", &spelled),
                param("CertificateBody", certificate),
            ];
            call(router, "UploadSigningCertificate", &borrowed(&params)).await
        }
    }
}

/// RULE: a certificate is registered ONCE in the account, so an upload of a certificate
/// another IAM user already holds is refused with `DuplicateCertificate` (HTTP 409) and
/// stores nothing — however that other user is named and whatever whitespace framed the
/// PEM — while the user that holds it keeps its registration exactly as it stands.
///
/// Rows differ along what the rule generalises over: how many other users ask for the one
/// certificate, the case each upload spells its user's name in, whether the upload names
/// the user or leaves it to the signing key, and the algorithm and validity window the
/// certificate was generated with.
#[tokio::test]
async fn a_certificate_another_user_holds_is_refused() {
    const CASES: &[ClaimCase] = &[
        ClaimCase {
            label: "a second user naming itself",
            holder: Claimant {
                stored: "holder.one",
                spelled: "holder.one",
                implicit: false,
            },
            others: &[Claimant {
                stored: "second.one",
                spelled: "second.one",
                implicit: false,
            }],
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            validity: (2020, 2030),
        },
        ClaimCase {
            label: "a second user naming itself in another case",
            holder: Claimant {
                stored: "Holder_two",
                spelled: "holder_TWO",
                implicit: false,
            },
            others: &[Claimant {
                stored: "second+two",
                spelled: "SECOND+TWO",
                implicit: false,
            }],
            algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
            validity: (2024, 2044),
        },
        ClaimCase {
            label: "two other users, one of them resolved from its signing key",
            holder: Claimant {
                stored: "holder=three",
                spelled: "holder=three",
                implicit: false,
            },
            others: &[
                Claimant {
                    stored: "second_three",
                    spelled: "second_three",
                    implicit: true,
                },
                Claimant {
                    stored: "third@three",
                    spelled: "THIRD@THREE",
                    implicit: false,
                },
            ],
            algorithm: &rcgen::PKCS_ED25519,
            validity: (2029, 2039),
        },
        ClaimCase {
            label: "a holder and a second user both resolved from their signing keys",
            holder: Claimant {
                stored: "holder-four",
                spelled: "holder-four",
                implicit: true,
            },
            others: &[Claimant {
                stored: "second-four",
                spelled: "second-four",
                implicit: true,
            }],
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            validity: (2040, 2041),
        },
    ];

    let router = router();
    for (row, case) in CASES.iter().enumerate() {
        let label = case.label;
        // The one certificate every user of this row asks for, generated by `rcgen` —
        // something other than the code under test.
        let certificate = certificate_pem(
            case.algorithm,
            case.validity,
            &format!("claimed-{row}.example.com"),
        );

        // The user that registers the certificate first holds it from here on.
        let holder = format!("{}-{row}", case.holder.stored);
        with_user_named(&router, label, &holder).await;
        let holder_key = signing_key_for(&router, label, &case.holder, row).await;
        let (status, registered) = upload_claim(
            &router,
            &case.holder,
            row,
            holder_key.as_ref(),
            &certificate,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first upload must be served, got {status}: {registered}"
        );
        let certificate_id = element(&registered, "CertificateId");
        let held = BTreeMap::from([(
            certificate_id.clone(),
            (holder.clone(), ACTIVE.to_string(), certificate.clone()),
        )]);

        for other in case.others {
            let stored = format!("{}-{row}", other.stored);
            with_user_named(&router, label, &stored).await;
            let other_key = signing_key_for(&router, label, other, row).await;

            // The body as it was sent, and the same certificate re-framed with whitespace:
            // the same CERTIFICATE every time, which is what is already registered.
            let repeats = [
                certificate.clone(),
                format!("{certificate}\n"),
                format!("\n  {}  \n", certificate.trim()),
            ];
            for (index, repeat) in repeats.iter().enumerate() {
                let (status, refused) =
                    upload_claim(&router, other, row, other_key.as_ref(), repeat).await;
                assert_eq!(
                    status, 409,
                    "case {label:?}: {stored:?} repeat {index} must be refused with 409, got \
                     {status}: {refused}"
                );
                assert_eq!(
                    error_code(&refused),
                    "DuplicateCertificate",
                    "case {label:?}: {stored:?} repeat {index} expected DuplicateCertificate, \
                     got {:?}",
                    error_code(&refused)
                );
                assert_eq!(
                    element(&refused, "Message"),
                    DUPLICATE_MESSAGE,
                    "case {label:?}: {stored:?} repeat {index} expected the refusal \
                     {DUPLICATE_MESSAGE:?}, got {refused}"
                );
                assert!(
                    elements(&refused, "CertificateId").is_empty(),
                    "case {label:?}: {stored:?} repeat {index} must report no certificate, got \
                     {refused}"
                );
            }

            // The refused user holds nothing: the refusal stored no association for it.
            let (status, listed) =
                call(&router, "ListSigningCertificates", &[("UserName", &stored)]).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the list for {stored:?} must be served, got {status}: {listed}"
            );
            assert!(
                reported_certificates(&listed).is_empty(),
                "case {label:?}: {stored:?} must hold nothing after the refusals, got {listed}"
            );

            // The refusal is about THAT certificate: a certificate of its own is stored for
            // the very same user, under an id of its own.
            let own = certificate_pem(
                case.algorithm,
                case.validity,
                &format!("own-{row}-{}.example.com", other.stored.to_lowercase()),
            );
            let mine = upload(&router, label, &stored, &own).await;
            assert_ne!(
                element(&mine, "CertificateId"),
                certificate_id,
                "case {label:?}: {stored:?}'s own certificate must get an id of its own, got \
                 {mine}"
            );
        }

        // The holder's registration came through every refusal untouched.
        let (_, again) = call(&router, "ListSigningCertificates", &[("UserName", &holder)]).await;
        assert_eq!(
            reported_certificates(&again),
            held,
            "case {label:?}: the refusals must leave {holder:?} as it stood, got {again}"
        );
    }
}

/// RULE: a `CertificateBody` that is not one PEM-encoded X.509 certificate is refused as
/// malformed, with nothing stored.
#[tokio::test]
async fn a_body_that_is_not_a_certificate_is_malformed() {
    let router = router();
    let user = "certificate-holder";
    let (status, body) = call(&router, "CreateUser", &[("UserName", user)]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");

    let valid = certificate_pem(
        &rcgen::PKCS_ECDSA_P256_SHA256,
        (2024, 2034),
        "malformed.example.com",
    );
    // Rows spanning text that is not PEM at all, PEM framing around something that is not
    // a certificate, a certificate whose base64 payload is truncated, a body carrying
    // only the framing lines, and the DER-less inner base64 on its own.
    let bodies: Vec<(&str, String)> = vec![
        ("plain text", "not a certificate".to_string()),
        (
            "a private key",
            rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
                .expect("a key pair")
                .serialize_pem(),
        ),
        (
            "a truncated certificate",
            valid[..valid.len() / 2].to_string(),
        ),
        (
            "framing only",
            "-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n".to_string(),
        ),
        (
            "the payload without framing",
            valid
                .lines()
                .filter(|line| !line.starts_with("-----"))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    ];

    for (label, certificate) in &bodies {
        let params = vec![
            param("UserName", user),
            param("CertificateBody", certificate),
        ];
        let (status, refused) = call(&router, "UploadSigningCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "MalformedCertificate",
            "case {label:?}: expected MalformedCertificate, got {:?}",
            error_code(&refused)
        );
    }

    let (_, listed) = call(&router, "ListSigningCertificates", &[("UserName", user)]).await;
    assert!(
        reported_certificates(&listed).is_empty(),
        "a refused upload must store nothing, got {listed}"
    );
}

/// RULE: a user may hold up to the published quota of signing certificates; an upload
/// past it is refused with `LimitExceeded` and stores nothing, and the case the request
/// spells the user's name in does not open a second allowance.
#[tokio::test]
async fn uploading_past_the_certificate_quota_is_refused() {
    let router = router();

    for case in CASES {
        let label = format!("{} requested as {}", case.stored_name, case.requested_name);
        case.with_user(&router).await;

        // Every certificate up to the quota is stored. The request alternates between
        // the stored case and the requested case, so the quota is taken over the ENTITY
        // and not over the spelling.
        let mut held: BTreeMap<String, String> = BTreeMap::new();
        for index in 0..MAX_SIGNING_CERTIFICATES_PER_USER {
            let name = if index.is_multiple_of(2) {
                case.stored_name
            } else {
                case.requested_name
            };
            let certificate = case.certificate(index);
            let uploaded = upload(&router, &label, name, &certificate).await;
            held.insert(element(&uploaded, "CertificateId"), certificate);
        }

        let beyond = case.certificate(MAX_SIGNING_CERTIFICATES_PER_USER);
        let params = vec![
            param("UserName", case.requested_name),
            param("CertificateBody", &beyond),
        ];
        let (status, refused) = call(&router, "UploadSigningCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 409,
            "case {label:?}: the upload past the quota must be refused with 409, got {status}: \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "LimitExceeded",
            "case {label:?}: expected LimitExceeded, got {:?}",
            error_code(&refused)
        );

        // The refusal stored nothing and left the held certificates untouched.
        let (_, listed) = call(
            &router,
            "ListSigningCertificates",
            &[("UserName", case.stored_name)],
        )
        .await;
        let reported = reported_certificates(&listed);
        assert_eq!(
            reported.len(),
            MAX_SIGNING_CERTIFICATES_PER_USER,
            "case {label:?}: expected exactly the quota to be held, got {listed}"
        );
        for (certificate_id, certificate) in &held {
            assert_eq!(
                reported.get(certificate_id).map(|(_, _, body)| body.clone()),
                Some(certificate.clone()),
                "case {label:?}: certificate {certificate_id:?} must still be held as stored, \
                 got {listed}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Status updates and deletes
// ---------------------------------------------------------------------------

/// RULE: an update moves the status of the ONE certificate it names, in either direction,
/// leaving every other member of that record and every other certificate as they stand;
/// an update to the status the certificate already carries is a no-op.
#[tokio::test]
async fn updating_a_certificate_moves_only_that_status() {
    let router = router();

    for case in CASES {
        let label = format!("{} requested as {}", case.stored_name, case.requested_name);
        case.with_user(&router).await;

        let first_body = case.certificate(1);
        let second_body = case.certificate(2);
        let first = element(
            &upload(&router, &label, case.stored_name, &first_body).await,
            "CertificateId",
        );
        let second = element(
            &upload(&router, &label, case.requested_name, &second_body).await,
            "CertificateId",
        );

        // Rows: out to `Inactive`, back to `Active` ("or vice versa"), and the status it
        // already carries, which changes nothing.
        for (index, status_wanted) in [INACTIVE, ACTIVE, ACTIVE].iter().enumerate() {
            let (status, updated) = call(
                &router,
                "UpdateSigningCertificate",
                &[
                    ("UserName", case.requested_name),
                    ("CertificateId", &first),
                    ("Status", status_wanted),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: update {index} to {status_wanted} must be served, got \
                 {status}: {updated}"
            );

            let (_, listed) = call(
                &router,
                "ListSigningCertificates",
                &[("UserName", case.stored_name)],
            )
            .await;
            let reported = reported_certificates(&listed);
            assert_eq!(
                reported.get(&first),
                Some(&(
                    case.stored_name.to_string(),
                    (*status_wanted).to_string(),
                    first_body.clone(),
                )),
                "case {label:?}: update {index} must move only the status, got {listed}"
            );
            assert_eq!(
                reported.get(&second),
                Some(&(
                    case.stored_name.to_string(),
                    ACTIVE.to_string(),
                    second_body.clone(),
                )),
                "case {label:?}: update {index} must leave the other certificate, got {listed}"
            );
        }
    }
}

/// RULE: a delete removes the ONE certificate it names and nothing else; a certificate id
/// the user does not hold — one already deleted, one another user holds, one never minted
/// — is not found, and removes nothing.
#[tokio::test]
async fn deleting_a_certificate_removes_only_that_one() {
    let router = router();
    // A second account user, so a certificate id that exists but belongs to SOMEONE ELSE
    // can be named.
    let other = "other-holder";
    let (status, body) = call(&router, "CreateUser", &[("UserName", other)]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");
    let other_certificate = certificate_pem(
        &rcgen::PKCS_ED25519,
        (2025, 2035),
        "other-holder.example.com",
    );
    let other_id = element(
        &upload(&router, other, other, &other_certificate).await,
        "CertificateId",
    );

    for case in CASES {
        let label = format!("{} requested as {}", case.stored_name, case.requested_name);
        case.with_user(&router).await;
        let kept_body = case.certificate(1);
        let doomed_body = case.certificate(2);
        let kept = element(
            &upload(&router, &label, case.stored_name, &kept_body).await,
            "CertificateId",
        );
        let doomed = element(
            &upload(&router, &label, case.stored_name, &doomed_body).await,
            "CertificateId",
        );

        let (status, deleted) = call(
            &router,
            "DeleteSigningCertificate",
            &[
                ("UserName", case.requested_name),
                ("CertificateId", &doomed),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the delete must be served, got {status}: {deleted}"
        );

        let (_, listed) = call(
            &router,
            "ListSigningCertificates",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            reported_certificates(&listed),
            BTreeMap::from([(
                kept.clone(),
                (
                    case.stored_name.to_string(),
                    ACTIVE.to_string(),
                    kept_body.clone(),
                ),
            )]),
            "case {label:?}: the delete must remove only the certificate it named, got {listed}"
        );

        // Ids this user does not hold: the one just deleted, another user's, and one no
        // upload ever minted (of the modelled shape).
        let absent = [
            ("already deleted", doomed.clone()),
            ("another user's", other_id.clone()),
            ("never minted", "A".repeat(32)),
        ];
        for (reason, certificate_id) in &absent {
            for action in ["DeleteSigningCertificate", "UpdateSigningCertificate"] {
                let mut params = vec![
                    param("UserName", case.requested_name),
                    param("CertificateId", certificate_id),
                ];
                if action == "UpdateSigningCertificate" {
                    params.push(param("Status", INACTIVE));
                }
                let (status, refused) = call(&router, action, &borrowed(&params)).await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: {action} on an {reason} id must be 404, got {status}: \
                     {refused}"
                );
                assert_eq!(
                    error_code(&refused),
                    "NoSuchEntity",
                    "case {label:?}: {action} on an {reason} id expected NoSuchEntity, got {:?}",
                    error_code(&refused)
                );
            }
        }

        // Neither this user's remaining certificate nor the other user's was touched.
        let (_, listed) = call(
            &router,
            "ListSigningCertificates",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            reported_certificates(&listed).keys().collect::<Vec<_>>(),
            vec![&kept],
            "case {label:?}: the refusals must leave the held certificate, got {listed}"
        );
        let (_, others) = call(&router, "ListSigningCertificates", &[("UserName", other)]).await;
        assert_eq!(
            reported_certificates(&others).keys().collect::<Vec<_>>(),
            vec![&other_id],
            "case {label:?}: another user's certificate must be untouched, got {others}"
        );
    }
}

/// RULE: a signing certificate is a dependent of its user, so IAM refuses to delete a
/// user that still holds one rather than cascade-deleting the certificate; once the
/// certificate is gone the user deletes.
#[tokio::test]
async fn a_user_holding_a_certificate_cannot_be_deleted() {
    let router = router();

    for case in CASES {
        let label = format!("{} requested as {}", case.stored_name, case.requested_name);
        case.with_user(&router).await;
        let certificate = case.certificate(1);
        let certificate_id = element(
            &upload(&router, &label, case.requested_name, &certificate).await,
            "CertificateId",
        );

        let (status, refused) = call(&router, "DeleteUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 409,
            "case {label:?}: the user delete must be refused with 409, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "DeleteConflict",
            "case {label:?}: expected DeleteConflict, got {:?}",
            error_code(&refused)
        );

        // The refused delete left both the user and its certificate.
        let (status, listed) = call(
            &router,
            "ListSigningCertificates",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            (status, reported_certificates(&listed).keys().collect()),
            (200, vec![&certificate_id]),
            "case {label:?}: the refused delete must leave the certificate, got {listed}"
        );

        let (status, deleted) = call(
            &router,
            "DeleteSigningCertificate",
            &[
                ("UserName", case.stored_name),
                ("CertificateId", &certificate_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the certificate must delete, got {status}: {deleted}"
        );
        let (status, deleted) = call(&router, "DeleteUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the user must delete once its certificate is gone, got {status}: \
             {deleted}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination and absent entities
// ---------------------------------------------------------------------------

/// RULE: the list pages through every certificate of the named user exactly once, in a
/// total order, resuming strictly after the position a marker names; a token this service
/// did not issue is refused.
#[tokio::test]
async fn the_list_pages_through_every_certificate_exactly_once() {
    let router = router();

    for case in CASES {
        let label = format!("{} requested as {}", case.stored_name, case.requested_name);
        case.with_user(&router).await;
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for index in 0..MAX_SIGNING_CERTIFICATES_PER_USER {
            let certificate = case.certificate(index);
            let uploaded = upload(&router, &label, case.stored_name, &certificate).await;
            expected.insert(element(&uploaded, "CertificateId"), certificate);
        }

        // One certificate per page, so every page but the last is truncated and carries a
        // marker.
        let mut seen: BTreeMap<String, String> = BTreeMap::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![
                param("UserName", case.requested_name),
                param("MaxItems", "1"),
            ];
            if let Some(marker) = &marker {
                params.push(param("Marker", marker));
            }
            let (status, page) =
                call(&router, "ListSigningCertificates", &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {label:?}: page {pages} must be served, got {status}: {page}"
            );
            let reported = reported_certificates(&page);
            assert!(
                reported.len() <= 1,
                "case {label:?}: page {pages} must hold at most the page size, got {page}"
            );
            for (certificate_id, (_, _, body)) in reported {
                assert!(
                    seen.insert(certificate_id.clone(), body).is_none(),
                    "case {label:?}: certificate {certificate_id:?} was reported on two pages"
                );
            }
            pages += 1;
            if element(&page, "IsTruncated") == "true" {
                let next = element(&page, "Marker");
                assert!(
                    !next.is_empty(),
                    "case {label:?}: a truncated page must carry a marker, got {page}"
                );
                marker = Some(next);
            } else {
                assert!(
                    elements(&page, "Marker").is_empty(),
                    "case {label:?}: an untruncated page must carry no marker, got {page}"
                );
                break;
            }
            assert!(
                pages <= MAX_SIGNING_CERTIFICATES_PER_USER + 1,
                "case {label:?}: the listing did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, expected,
            "case {label:?}: paging must report every stored certificate exactly once"
        );

        // A token this service's encoding cannot have produced is refused rather than
        // read as "start from the beginning".
        for foreign in ["not-a-marker", "aWFtOnVzZXJzOmFsaWNl", ""] {
            let params = vec![
                param("UserName", case.stored_name),
                param("Marker", foreign),
            ];
            let (status, refused) =
                call(&router, "ListSigningCertificates", &borrowed(&params)).await;
            assert_eq!(
                status, 400,
                "case {label:?}: marker {foreign:?} must be refused with 400, got {status}: \
                 {refused}"
            );
        }
    }
}

/// RULE: a certificate belongs to a user the account holds, so every operation on one
/// answers a name the account does not hold with `NoSuchEntity`, and stores nothing.
#[tokio::test]
async fn an_absent_user_is_not_found() {
    let router = router();
    let certificate = certificate_pem(
        &rcgen::PKCS_ECDSA_P384_SHA384,
        (2026, 2036),
        "absent.example.com",
    );
    let (status, body) = call(&router, "CreateUser", &[("UserName", "present")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");

    // Rows spanning a plain absent name, one differing from a held name by more than
    // case, names from the rest of the accepted alphabet, and a name at the
    // 128-character bound `existingUserNameType` admits.
    let absent: Vec<String> = vec![
        "absent".to_string(),
        "present2".to_string(),
        "no.such@user".to_string(),
        "NOT_HELD+1".to_string(),
        "x".repeat(128),
    ];

    for name in &absent {
        let calls: Vec<(&str, Vec<(String, String)>)> = vec![
            (
                "UploadSigningCertificate",
                vec![
                    param("UserName", name),
                    param("CertificateBody", &certificate),
                ],
            ),
            ("ListSigningCertificates", vec![param("UserName", name)]),
            (
                "UpdateSigningCertificate",
                vec![
                    param("UserName", name),
                    param("CertificateId", &"B".repeat(32)),
                    param("Status", INACTIVE),
                ],
            ),
            (
                "DeleteSigningCertificate",
                vec![
                    param("UserName", name),
                    param("CertificateId", &"B".repeat(32)),
                ],
            ),
        ];
        for (action, params) in &calls {
            let (status, refused) = call(&router, action, &borrowed(params)).await;
            assert_eq!(
                status, 404,
                "case {name:?}: {action} must be 404, got {status}: {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "NoSuchEntity",
                "case {name:?}: {action} expected NoSuchEntity, got {:?}",
                error_code(&refused)
            );
            assert!(
                elements(&refused, "CertificateId").is_empty(),
                "case {name:?}: {action} must report no certificate, got {refused}"
            );
        }
    }
}
