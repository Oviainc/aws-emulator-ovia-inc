// SPDX-License-Identifier: CC0-1.0

//! `DeleteServerCertificate`, end to end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a certificate that was
//! PERSISTED by `UploadServerCertificate`, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in this crate
//! share, and the real key pair and self-signed certificate an upload needs come from
//! `tests/all/server_certificates.rs`.
//!
//! The rule under test is that the delete REMOVES the whole record the account holds under
//! the name it names — the metadata, the PEM material and the tags together — so the
//! reporting half no longer reports it, its tags are no longer listable, the name is free
//! to be uploaded again under a record that inherits nothing, and every other certificate
//! the account holds is untouched; while a name the account holds no certificate for is
//! refused with `NoSuchEntity` rather than answered as a success.
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over (the
//! name's alphabet and case, the path's depth, the key algorithm, the validity window,
//! whether a chain is carried and the tag count), so no single request is the
//! specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, optional, param, reported_tags};
use crate::server_certificates::material;
use crate::user_lifecycle::{call, element, error_code, router};

/// One certificate the account holds, as the upload that persists it describes it.
#[derive(Clone)]
struct DeleteCase {
    /// `ServerCertificateName`, spelled across the `^[\w+=,.@-]+$` alphabet the member
    /// declares (`smithy_query(query="operation", operation="DeleteServerCertificate")`).
    name: &'static str,
    /// `Path`, or `None` for an upload that omits it: an omitted path "defaults to a slash
    /// (/)" (`UploadServerCertificate$Path`).
    path: Option<&'static str>,
    /// The signature algorithm the generated key pair uses.
    algorithm: &'static rcgen::SignatureAlgorithm,
    /// The generated certificate's validity window, as `(not_before, not_after)` years.
    validity: (i32, i32),
    /// Whether the upload carries a `CertificateChain`.
    chain: bool,
    /// The tags the upload attaches, in request order.
    tags: Vec<(&'static str, &'static str)>,
}

/// The table every suite here is written against: the default path, a nested one, the
/// `/cloudfront/` path `$Path`'s note calls for and the root given explicitly; names
/// spanning the member's alphabet and both cases; three key algorithms; validity windows of
/// one year, ten and twenty, every one of them far enough from the wall clock that no row
/// depends on it; a chain carried and omitted; and tag counts from none to three.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            name: "web-server",
            path: None,
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            validity: (2020, 2030),
            chain: false,
            tags: Vec::new(),
        },
        DeleteCase {
            name: "CloudFront_Cert.2",
            path: Some("/cloudfront/"),
            algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
            validity: (2021, 2041),
            chain: true,
            tags: vec![("env", "prod"), ("alpha", "1")],
        },
        DeleteCase {
            name: "edge+east=1,a@example",
            path: Some("/division_abc/subdivision_xyz/"),
            algorithm: &rcgen::PKCS_ED25519,
            validity: (2024, 2034),
            chain: false,
            tags: vec![("zeta", "last"), ("alpha", "first"), ("mu", "middle")],
        },
        DeleteCase {
            name: "LEGACY-cert_9",
            path: Some("/"),
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            validity: (2040, 2041),
            chain: true,
            tags: vec![("owner", "platform")],
        },
    ]
}

impl DeleteCase {
    /// Upload this row's certificate, so the delete has something persisted to remove, and
    /// report the upload's response body. A real key pair and a real self-signed
    /// certificate are generated per upload, so nothing here depends on a canned PEM.
    async fn persist(&self, router: &MultiServiceRouter) -> String {
        let leaf = material(self.algorithm, self.validity, "server.example.com");
        let chain = self
            .chain
            .then(|| material(self.algorithm, self.validity, "issuer.example.com").certificate);
        let mut params = vec![
            param("ServerCertificateName", self.name),
            param("CertificateBody", &leaf.certificate),
            param("PrivateKey", &leaf.private_key),
        ];
        if let Some(path) = self.path {
            params.push(param("Path", path));
        }
        if let Some(chain) = chain.as_deref() {
            params.push(param("CertificateChain", chain));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        let (status, body) = call(router, "UploadServerCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture certificate must upload, got {status} with body {body}",
            self.name
        );
        body
    }
}

/// One `DeleteServerCertificate` call.
async fn delete(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    let params = vec![param("ServerCertificateName", name)];
    call(router, "DeleteServerCertificate", &borrowed(&params)).await
}

/// One `GetServerCertificate` call — the reporting half a delete is read back through.
async fn read(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    let params = vec![param("ServerCertificateName", name)];
    call(router, "GetServerCertificate", &borrowed(&params)).await
}

/// One `ListServerCertificateTags` call, which reports the tags that hang off the record.
async fn list_tags(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    let params = vec![param("ServerCertificateName", name)];
    call(router, "ListServerCertificateTags", &borrowed(&params)).await
}

/// RULE: a delete removes the whole record the account holds under the name, so the
/// reporting half answers `NoSuchEntity` (HTTP 404), the record's tags are gone with it, and
/// a second delete of the same name is refused rather than answered as a success.
#[tokio::test]
async fn deleting_a_held_certificate_removes_the_whole_record() {
    let router = router();

    for case in delete_cases() {
        case.persist(&router).await;
        let (status, read_back) = read(&router, case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture certificate must read back before the delete, got \
             {status} with body {read_back}",
            case.name
        );

        let (status, body) = delete(&router, case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DeleteServerCertificate, got {status} with \
             body {body}",
            case.name
        );
        // The output shape is `Unit`: the delete reports nothing of the record it removed.
        assert!(
            optional(&body, "ServerCertificateId").is_none()
                && optional(&body, "CertificateBody").is_none(),
            "case {:?}: the delete reports no member of the removed certificate, got {body}",
            case.name
        );

        let (status, body) = read(&router, case.name).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected HTTP 404 after the delete, got {status} with body {body}",
            case.name
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {:?}: expected NoSuchEntity after the delete, got {body}",
            case.name
        );
        assert!(
            optional(&body, "CertificateBody").is_none(),
            "case {:?}: a deleted certificate reports no PEM material, got {body}",
            case.name
        );

        // The tags are members of the one record, so they go with it rather than
        // outliving the certificate they hung off.
        let (status, body) = list_tags(&router, case.name).await;
        assert_eq!(
            status, 404,
            "case {:?}: the deleted record's tags must be unlistable, got {status} with \
             body {body}",
            case.name
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {:?}: expected NoSuchEntity listing a deleted record's tags, got {body}",
            case.name
        );

        // A name the account now holds nothing under is the entity-absent answer, whatever
        // it held a moment ago.
        let (status, body) = delete(&router, case.name).await;
        assert_eq!(
            status, 404,
            "case {:?}: a second delete must be refused, got {status} with body {body}",
            case.name
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {:?}: expected NoSuchEntity from a second delete, got {body}",
            case.name
        );
    }
}

/// RULE: the delete frees the name, and the record that follows inherits NOTHING from the
/// deleted one — a fresh id, the new path, the new certificate's expiration and only the
/// tags the new upload carries.
#[tokio::test]
async fn an_upload_after_a_delete_inherits_nothing_from_the_deleted_record() {
    let router = router();

    for case in delete_cases() {
        let first = case.persist(&router).await;
        let (status, body) = delete(&router, case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture certificate must delete, got {status} with body {body}",
            case.name
        );

        // The second upload differs from the first along every member a reader reports:
        // another path, another validity window, and no tags at all.
        let replacement = DeleteCase {
            name: case.name,
            path: Some("/replacement/"),
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            validity: (2045, 2046),
            chain: false,
            tags: Vec::new(),
        };
        let second = replacement.persist(&router).await;

        assert_ne!(
            element(&second, "ServerCertificateId"),
            element(&first, "ServerCertificateId"),
            "case {:?}: the upload after a delete must mint its own id, got {second}",
            case.name
        );

        let (status, read_back) = read(&router, case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: the name must be free to upload again, got {status} with body \
             {read_back}",
            case.name
        );
        assert_eq!(
            element(&read_back, "Path"),
            "/replacement/",
            "case {:?}: the read must report the new record's path, got {read_back}",
            case.name
        );
        assert_eq!(
            element(&read_back, "Expiration"),
            element(&second, "Expiration"),
            "case {:?}: the read must report the new certificate's expiration, got \
             {read_back}",
            case.name
        );
        assert_ne!(
            element(&read_back, "Expiration"),
            element(&first, "Expiration"),
            "case {:?}: the new record must not report the deleted certificate's \
             expiration, got {read_back}",
            case.name
        );
        assert!(
            reported_tags(&read_back).is_empty(),
            "case {:?}: the deleted record's tags must not survive into the new one, got \
             {read_back}",
            case.name
        );

        // Leave the account as the suite found it, so each row starts from nothing held.
        let (status, body) = delete(&router, case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: the replacement certificate must delete, got {status} with body \
             {body}",
            case.name
        );
    }
}

/// RULE: a delete removes ONLY the certificate it names — every other certificate the
/// account holds keeps its record and its tags.
#[tokio::test]
async fn a_delete_leaves_every_other_held_certificate_alone() {
    let router = router();
    let cases = delete_cases();
    let mut expected_tags = Vec::new();
    for case in &cases {
        case.persist(&router).await;
        expected_tags.push(
            case.tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect::<std::collections::BTreeMap<String, String>>(),
        );
    }

    for (deleted, case) in cases.iter().enumerate() {
        let (status, body) = delete(&router, case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the delete, got {status} with body {body}",
            case.name
        );

        for (index, other) in cases.iter().enumerate() {
            let (status, body) = read(&router, other.name).await;
            if index <= deleted {
                assert_eq!(
                    status, 404,
                    "case {:?}: a deleted certificate must stay deleted, got {status} with \
                     body {body}",
                    other.name
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {:?}: deleting {:?} must leave it readable, got {status} with body \
                 {body}",
                other.name, case.name
            );
            assert_eq!(
                reported_tags(&body),
                expected_tags[index],
                "case {:?}: deleting {:?} must leave its tags alone, got {body}",
                other.name,
                case.name
            );
        }
    }
}

/// RULE: a certificate is addressed by the name the account holds it under, and IAM entity
/// names "are not distinguished by case", so any spelling of a held name deletes that
/// certificate.
#[tokio::test]
async fn a_held_name_is_deleted_by_any_spelling_of_it() {
    let router = router();

    for case in delete_cases() {
        for spelling in [case.name.to_uppercase(), case.name.to_lowercase()] {
            case.persist(&router).await;

            let (status, body) = delete(&router, &spelling).await;
            assert_eq!(
                status, 200,
                "case {spelling:?}: expected HTTP 200, got {status} with body {body}"
            );
            let (status, body) = read(&router, case.name).await;
            assert_eq!(
                status, 404,
                "case {spelling:?}: every spelling of a held name must delete that \
                 certificate, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {spelling:?}: expected NoSuchEntity after the delete, got {body}"
            );
        }
    }
}

/// RULE: a name the account holds no certificate for is `NoSuchEntity` (HTTP 404), naming
/// the name the REQUEST asked for, and nothing the account holds is removed.
#[tokio::test]
async fn deleting_what_was_never_uploaded_is_refused_and_removes_nothing() {
    let router = router();
    let cases = delete_cases();
    for case in &cases {
        case.persist(&router).await;
    }

    // Rows: a name nothing holds, near-misses of two held names, and a name spanning the
    // member's alphabet.
    for name in [
        "never-uploaded",
        "web-server-2",
        "LEGACY-cert_90",
        "A+B=C,d@e-f_g",
    ] {
        let (status, body) = delete(&router, name).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The Server Certificate with name {name} cannot be found."),
            "case {name:?}: the refusal must name the certificate the request asked for, \
             got {body}"
        );
    }

    for case in &cases {
        let (status, body) = read(&router, case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: a refused delete must remove nothing, got {status} with body \
             {body}",
            case.name
        );
    }
}
