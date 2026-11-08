// SPDX-License-Identifier: CC0-1.0

//! The audit reports a private CA produces over the certificates it has issued.
//!
//! The rule under test: "Creates an audit report that lists every time that your CA private
//! key is used to issue a certificate. The `IssueCertificate` and `RevokeCertificate`
//! actions use the private key." So a CA that has not been given a certificate of its own
//! has used no key and is refused with the state error about the authority; a CA that holds
//! one deposits a report of what it has signed in the bucket the request names, under a key
//! it reports, and answers a report id the report is then described by. Producing the report
//! completes on the clock, so its status moves from `CREATING` to `SUCCESS` without anybody
//! asking.
//!
//! The rows span both `AuditReportResponseFormat` members and the statuses a CA is in before
//! and after it takes its certificate. The suite drives the private CA AND the object
//! storage its reports go to over one engine, so the `S3Key` it answers is checked by
//! fetching the object.

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::{
    not_in_state_to_have_issued_message, AUDIT_REPORT_NOT_FOUND_MESSAGE, BUCKET_ABSENT_MESSAGE,
    CA_NOT_FOUND_MESSAGE,
};

use crate::crl_bucket_resolution::Emulator;
use crate::crl_publication::active_ca;
use crate::private_ca_lifecycle::{blob, csr_pem, root_ca_request, string, wait_for_the_clock};
use crate::resource_policy::error_message;

/// The bucket every report in this suite is deposited in.
const BUCKET: &str = "acme-audit-reports";

/// Call `operation` requiring the modelled error `code` with HTTP 400, and answer the
/// sentence it carries.
async fn refused(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
    code: &str,
) -> String {
    let parsed = emulator.refused(case, operation, body, code).await;
    error_message(&parsed, case)
}

/// A `CreateCertificateAuthorityAuditReport` body.
fn report_request(arn: &str, format: &str) -> Value {
    report_request_to(arn, BUCKET, format)
}

/// The same, for a named bucket.
fn report_request_to(arn: &str, bucket: &str, format: &str) -> Value {
    json!({
        "CertificateAuthorityArn": arn,
        "S3BucketName": bucket,
        "AuditReportResponseFormat": format,
    })
}

/// The CA's id: the last segment of its ARN, which is the folder its reports are written
/// under.
fn certificate_authority_id(arn: &str) -> String {
    arn.rsplit_once('/')
        .map(|(_, id)| id.to_string())
        .unwrap_or_default()
}

/// A CA that has not been given its certificate has signed nothing, so it has no audit
/// information — in every status before it takes one, and for either report format.
#[tokio::test]
async fn a_ca_that_cannot_have_issued_certificates_reports_no_audit_information() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_bucket(BUCKET).await,
        200,
        "the bucket the reports are deposited in is created through S3"
    );

    let creating = string(
        &emulator
            .ok("creating", "CreateCertificateAuthority", &root_ca_request("audit-creating"))
            .await,
        "CertificateAuthorityArn",
        "creating",
    );
    let pending = string(
        &emulator
            .ok("pending", "CreateCertificateAuthority", &root_ca_request("audit-pending"))
            .await,
        "CertificateAuthorityArn",
        "pending",
    );

    // Still being created, in both formats.
    for format in ["JSON", "CSV"] {
        let case = format!("a CA still being created, {format}");
        let message = refused(
            &emulator,
            &case,
            "CreateCertificateAuthorityAuditReport",
            &report_request(&creating, format),
            "InvalidStateException",
        )
        .await;
        assert_eq!(
            message,
            not_in_state_to_have_issued_message(&creating),
            "case {case:?}: expected the sentence about the authority, got {message:?}"
        );
    }

    wait_for_the_clock();

    // Waiting for its certificate: the same refusal.
    for (case, arn) in [
        ("a CA waiting for its certificate", &pending),
        ("a CA whose creation has settled since", &creating),
    ] {
        let message = refused(
            &emulator,
            case,
            "CreateCertificateAuthorityAuditReport",
            &report_request(arn, "JSON"),
            "InvalidStateException",
        )
        .await;
        assert_eq!(
            message,
            not_in_state_to_have_issued_message(arn),
            "case {case:?}: expected the sentence about the authority, got {message:?}"
        );
    }

    // An ARN no CA of this service answers to is the absence of the authority instead.
    let case = "an ARN naming no CA";
    let unknown = "arn:aws:acm-pca:eu-west-1:000000000000:certificate-authority/\
                   00000000-0000-4000-8000-000000000000";
    let message = refused(
        &emulator,
        case,
        "CreateCertificateAuthorityAuditReport",
        &report_request(unknown, "JSON"),
        "ResourceNotFoundException",
    )
    .await;
    assert_eq!(
        message, CA_NOT_FOUND_MESSAGE,
        "case {case:?}: expected the sentence about the authority, got {message:?}"
    );

    // And one that names no private CA RESOURCE is refused on its form.
    let case = "an ARN naming another resource type";
    refused(
        &emulator,
        case,
        "CreateCertificateAuthorityAuditReport",
        &report_request(
            "arn:aws:acm-pca:eu-west-1:000000000000:private-ca/abc",
            "JSON",
        ),
        "InvalidArnException",
    )
    .await;
    // And a bucket this account does not hold is the absence of the BUCKET: the report has
    // nowhere to go, and nothing is deposited or recorded for it.
    let case = "a bucket the account does not hold";
    let (active, _) = active_ca(&emulator, case, "audit-bucketless-root", None).await;
    let message = refused(
        &emulator,
        case,
        "CreateCertificateAuthorityAuditReport",
        &report_request_to(&active, "no-such-audit-bucket", "JSON"),
        "ResourceNotFoundException",
    )
    .await;
    assert_eq!(
        message, BUCKET_ABSENT_MESSAGE,
        "case {case:?}: expected the sentence about the bucket, got {message:?}"
    );
}

/// An active CA holding two issued certificates, one of them revoked: every use of its key
/// a report then lists.
async fn audited_ca(
    emulator: &Emulator,
    case: &str,
    common_name: &str,
) -> (String, Vec<String>) {
    let (arn, _) = active_ca(emulator, case, common_name, None).await;
    let mut issued: Vec<String> = Vec::new();
    for index in 0..2 {
        let answer = emulator
            .ok(
                case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": blob(&csr_pem(&format!("{common_name}-leaf-{index}"))),
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "TemplateArn": "arn:aws:acm-pca:::template/EndEntityCertificate/V1",
                    "Validity": {"Type": "DAYS", "Value": 30},
                }),
            )
            .await;
        issued.push(string(&answer, "CertificateArn", case));
    }
    // A report lists what the CA has DONE with its key, so the issuances have to have
    // completed before one is asked for.
    wait_for_the_clock();
    let revoked_serial = issued
        .first()
        .and_then(|arn| arn.rsplit_once("/certificate/"))
        .map(|(_, serial)| serial.to_string())
        .expect("a certificate ARN carries its serial");
    emulator
        .ok(
            case,
            "RevokeCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "CertificateSerial": revoked_serial,
                "RevocationReason": "KEY_COMPROMISE",
            }),
        )
        .await;
    (arn, issued)
}

/// An active CA deposits a report of every certificate it has signed, in the format asked
/// for, and describes it until it is finished.
///
/// Each format goes to a CA of its own: this operation's documentation publishes a limit of
/// one report per CA every 30 minutes, so a suite that asked one CA for two reports in the
/// same instant would be asserting on a call production refuses.
#[tokio::test]
async fn an_active_ca_deposits_a_report_of_what_it_has_signed() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_bucket(BUCKET).await,
        200,
        "the bucket the reports are deposited in is created through S3"
    );

    // (the format asked for, the extension its object key carries)
    for (format, extension) in [("JSON", "json"), ("CSV", "csv")] {
        let case = format!("a {format} report");
        let (arn, issued) =
            audited_ca(&emulator, &case, &format!("audited-{}", format.to_lowercase())).await;
        let created = emulator
            .ok(&case, "CreateCertificateAuthorityAuditReport", &report_request(&arn, format))
            .await;
        let report_id = string(&created, "AuditReportId", &case);
        let key = string(&created, "S3Key", &case);
        assert_eq!(
            report_id.len(),
            36,
            "case {case:?}: an AuditReportId is 36 characters, got {report_id:?}"
        );
        assert!(
            report_id
                .chars()
                .all(|character| character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '-'),
            "case {case:?}: an AuditReportId is lower-case hexadecimal and hyphens, \
             got {report_id:?}"
        );
        assert_eq!(
            key,
            format!(
                "audit-report/{}/{report_id}.{extension}",
                certificate_authority_id(&arn)
            ),
            "case {case:?}: a report's key names the CA and the report"
        );

        // Describing it: producing the report completes on the clock, so it is reported as
        // being created first and as finished after. The CREATING read comes FIRST, before
        // anything slower, because the window it is about is the span this service gives
        // the work and not an artefact of how long the suite takes.
        let describe = json!({
            "CertificateAuthorityArn": arn,
            "AuditReportId": report_id,
        });
        let described = emulator
            .ok(&case, "DescribeCertificateAuthorityAuditReport", &describe)
            .await;
        assert_eq!(
            string(&described, "AuditReportStatus", &case),
            "CREATING",
            "case {case:?}: a report is being created until it is due to be finished"
        );
        assert_eq!(
            string(&described, "S3BucketName", &case),
            BUCKET,
            "case {case:?}: the report is described by where it went"
        );
        assert_eq!(
            string(&described, "S3Key", &case),
            key,
            "case {case:?}: the report is described by the key it was written at"
        );
        assert!(
            described["CreatedAt"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: a report carries the instant it was created, got {described:?}"
        );

        // The key names an object that is THERE, and it carries what the CA signed.
        let (status, body) = emulator
            .s3("GET", &format!("{BUCKET}/{key}"), &[], Vec::new())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the report to be deposited at {key:?}, got {status}"
        );
        let text = String::from_utf8(body).expect("the report is text");
        for certificate_arn in &issued {
            assert!(
                text.contains(certificate_arn.as_str()),
                "case {case:?}: the report must list {certificate_arn:?}, got {text}"
            );
        }
        assert!(
            text.contains("KEY_COMPROMISE") && text.contains("EndEntityCertificate/V1"),
            "case {case:?}: the report must carry the revocation and the template of what it \
             lists, got {text}"
        );
        if format == "JSON" {
            let rows: Value = serde_json::from_str(&text)
                .unwrap_or_else(|error| panic!("case {case:?}: the report is JSON: {error}"));
            let rows = rows
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: a JSON report is an array: {rows:?}"));
            assert_eq!(
                rows.len(),
                issued.len(),
                "case {case:?}: one entry per certificate the CA signed, got {rows:?}"
            );
            assert_eq!(
                rows.iter()
                    .filter(|row| row["revocationReason"] == json!("KEY_COMPROMISE"))
                    .count(),
                1,
                "case {case:?}: the revoked certificate carries its reason, got {rows:?}"
            );
        } else {
            let mut lines = text.lines();
            assert_eq!(
                lines.next(),
                Some("awsAccountId,certificateArn,serial,subject,notBefore,notAfter,issuedAt,revokedAt,revocationReason,templateArn"),
                "case {case:?}: a CSV report opens with its columns, got {text}"
            );
            assert_eq!(
                lines.count(),
                issued.len(),
                "case {case:?}: one line per certificate the CA signed, got {text}"
            );
        }

        // Describing it once the span it was given has passed: finished.
        wait_for_the_clock();
        let finished = emulator
            .ok(&case, "DescribeCertificateAuthorityAuditReport", &describe)
            .await;
        assert_eq!(
            string(&finished, "AuditReportStatus", &case),
            "SUCCESS",
            "case {case:?}: a report is finished once it was due to be"
        );
        // Reading it again changes nothing a later caller sees.
        assert_eq!(
            emulator
                .ok(&case, "DescribeCertificateAuthorityAuditReport", &describe)
                .await,
            finished,
            "case {case:?}: describing a report twice answers the same report"
        );

        // A report id this CA has not produced is the absence of the REPORT.
        let case = format!("{case}: an unknown report id");
        let message = refused(
            &emulator,
            &case,
            "DescribeCertificateAuthorityAuditReport",
            &json!({
                "CertificateAuthorityArn": arn,
                "AuditReportId": "00000000-0000-4000-8000-000000000000",
            }),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message, AUDIT_REPORT_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the sentence about the report, got {message:?}"
        );
    }
}

/// A report belongs to the CA that produced it: another CA does not describe it.
#[tokio::test]
async fn a_report_is_described_only_by_the_ca_that_produced_it() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_bucket(BUCKET).await,
        200,
        "the bucket the reports are deposited in is created through S3"
    );
    let case = "two audited CAs";
    let (mine, _) = audited_ca(&emulator, case, "report-owner").await;
    let (other, _) = active_ca(&emulator, case, "report-neighbour", None).await;

    let report_id = string(
        &emulator
            .ok(case, "CreateCertificateAuthorityAuditReport", &report_request(&mine, "JSON"))
            .await,
        "AuditReportId",
        case,
    );
    // The CA that produced it describes it.
    emulator
        .ok(
            case,
            "DescribeCertificateAuthorityAuditReport",
            &json!({"CertificateAuthorityArn": mine, "AuditReportId": report_id}),
        )
        .await;
    // Its neighbour, which exists and is active, does not.
    let message = refused(
        &emulator,
        case,
        "DescribeCertificateAuthorityAuditReport",
        &json!({"CertificateAuthorityArn": other, "AuditReportId": report_id}),
        "ResourceNotFoundException",
    )
    .await;
    assert_eq!(
        message, AUDIT_REPORT_NOT_FOUND_MESSAGE,
        "case {case:?}: expected the sentence about the report, got {message:?}"
    );
}
