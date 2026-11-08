// SPDX-License-Identifier: CC0-1.0

//! The audit reports a private CA produces over the certificates it has issued.
//!
//! "Creates an audit report that lists every time that your CA private key is used to issue
//! a certificate. The `IssueCertificate` and `RevokeCertificate` actions use the private
//! key." (`CreateCertificateAuthorityAuditReport` docs.) So a report is a snapshot of what
//! the CA has done with its key: one entry per certificate it issued, carrying the
//! revocation when that certificate has one.
//!
//! The report is deposited in the Amazon S3 bucket the request names, under the key this
//! module builds, and the record of it stays on the CA so
//! `DescribeCertificateAuthorityAuditReport` can report where it went and whether it is
//! finished.

use super::*;

/// The `AuditReportStatus` member a report holds while it is being produced.
pub const AUDIT_REPORT_STATUS_CREATING: &str = "CREATING";
/// The `AuditReportStatus` member a finished report holds.
pub const AUDIT_REPORT_STATUS_SUCCESS: &str = "SUCCESS";

/// What this service answers when the private CA a request names is resolved and it has
/// produced no audit report under the id the request asked about.
///
/// `DescribeCertificateAuthorityAuditReport` models ONE absence
/// (`ResourceNotFoundException`) for both the CA and the report, so the sentence is what
/// says which was not found — this one is about the REPORT, in the form this service's
/// other not-found refusals take ([`CERTIFICATE_NOT_FOUND_MESSAGE`]), naming no id of its
/// own.
pub const AUDIT_REPORT_NOT_FOUND_MESSAGE: &str = "Could not find specified audit report.";

/// The folder in the bucket a CA's audit reports are written under: `audit-report/<ca-id>`,
/// so one bucket holds the reports of every CA that writes to it without their keys
/// colliding.
pub const AUDIT_REPORT_PATH: &str = "audit-report";

/// How many reports of one CA this service keeps the record of.
///
/// Every report is held in memory for the life of the process, and a caller may ask for one
/// per call, so the per-CA store is bounded: past the bound the CA's OLDEST report record is
/// dropped ([`CertificateAuthorityRecord::record_audit_report`]). The objects already
/// deposited in the bucket are untouched by that — what is dropped is this service's record
/// of them, which is what `DescribeCertificateAuthorityAuditReport` reads.
pub const MAX_AUDIT_REPORTS_PER_CA: usize = 64;

/// One audit report a CA produced.
#[derive(Debug, Clone)]
pub struct AuditReportRecord {
    /// The `AuditReportId` the create call minted.
    pub id: String,
    /// The bucket the report was deposited in.
    pub s3_bucket_name: String,
    /// The key the report object was written at ([`audit_report_key`]).
    pub s3_key: String,
    /// When `CreateCertificateAuthorityAuditReport` accepted the request.
    pub created_at: SystemTime,
    /// When producing the report is due to finish, which is when it reads `SUCCESS`.
    pub completes_at: SystemTime,
}

impl AuditReportRecord {
    /// The report's `AuditReportStatus` as of `now`: producing a report takes the service a
    /// moment, so a report reads `CREATING` until the instant it was due to be finished and
    /// `SUCCESS` from then on.
    ///
    /// Derived from the clock rather than stored, so reading a report neither advances nor
    /// delays it.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.completes_at {
            AUDIT_REPORT_STATUS_SUCCESS
        } else {
            AUDIT_REPORT_STATUS_CREATING
        }
    }
}

/// The key a CA's audit report object is written at:
/// `audit-report/<ca-id>/<report-id>.<format>`.
///
/// The format is the extension, lower-cased, as the `AuditReportResponseFormat` member the
/// request carried names it (`JSON` or `CSV`).
pub fn audit_report_key(
    certificate_authority_id: &str,
    report_id: &str,
    format: &str,
) -> String {
    format!(
        "{AUDIT_REPORT_PATH}/{certificate_authority_id}/{report_id}.{}",
        format.to_lowercase()
    )
}

/// The media type a report object is stored with: the one its format names.
pub fn audit_report_content_type(format: &str) -> &'static str {
    match format {
        "CSV" => "text/csv",
        _ => "application/json",
    }
}

/// The report body itself: one entry per certificate the CA has issued, in the order their
/// serials were allocated.
///
/// The columns are the ones an audit report carries — the account that owns the CA, the
/// certificate's ARN, its serial, its subject, its validity window, when it was issued,
/// when it was revoked and why, and the template it was issued under — rendered as the
/// format the request asked for: a JSON array of objects, or a CSV with those columns in
/// the same order and a header row.
///
/// A certificate the CA has accepted but not finished issuing is not in it: the report
/// lists what the CA has DONE with its key, and that issuance completes on the clock like
/// every other.
pub fn audit_report_body(
    ca: &CertificateAuthorityRecord,
    format: &str,
    now: SystemTime,
) -> Vec<u8> {
    let entries = audited_certificates(ca, now);
    if format == "CSV" {
        return audit_report_csv(&entries).into_bytes();
    }
    let rows: Vec<::serde_json::Value> = entries
        .iter()
        .map(|entry| {
            let mut row = ::serde_json::Map::new();
            for (column, value) in entry {
                row.insert((*column).to_string(), ::serde_json::Value::String(value.clone()));
            }
            ::serde_json::Value::Object(row)
        })
        .collect();
    ::serde_json::Value::Array(rows).to_string().into_bytes()
}

/// The columns of one audit-report entry, in the order a report states them.
const AUDIT_REPORT_COLUMNS: &[&str] = &[
    "awsAccountId",
    "certificateArn",
    "serial",
    "subject",
    "notBefore",
    "notAfter",
    "issuedAt",
    "revokedAt",
    "revocationReason",
    "templateArn",
];

/// One entry per certificate the CA's key has been used for, as the (column, value) pairs
/// the report states, ordered by the serial the CA allocated.
fn audited_certificates(
    ca: &CertificateAuthorityRecord,
    now: SystemTime,
) -> Vec<Vec<(&'static str, String)>> {
    let mut issued: Vec<&IssuedCertificate> = ca
        .certificates
        .values()
        .filter(|certificate| certificate.is_available(now))
        .collect();
    issued.sort_by(|left, right| (left.issued_at, &left.serial).cmp(&(right.issued_at, &right.serial)));
    issued
        .into_iter()
        .map(|certificate| audited_certificate(ca, certificate))
        .collect()
}

/// One certificate as the entry a report states for it.
fn audited_certificate(
    ca: &CertificateAuthorityRecord,
    certificate: &IssuedCertificate,
) -> Vec<(&'static str, String)> {
    let revocation = certificate.revocation.as_ref();
    let subject = certificate_subject(certificate).unwrap_or_default();
    let values = [
        ca.owner_account.clone(),
        certificate.arn.clone(),
        certificate.serial.clone(),
        subject,
        format_timestamp(certificate.not_before),
        format_timestamp(certificate.not_after),
        format_timestamp(certificate.issued_at),
        revocation
            .map(|revocation| format_timestamp(revocation.revoked_at))
            .unwrap_or_default(),
        revocation
            .map(|revocation| revocation.reason.clone())
            .unwrap_or_default(),
        certificate.template_arn.clone().unwrap_or_default(),
    ];
    AUDIT_REPORT_COLUMNS
        .iter()
        .copied()
        .zip(values)
        .collect()
}

/// The subject of an issued certificate, as the X.500 distinguished name it carries.
fn certificate_subject(certificate: &IssuedCertificate) -> Option<String> {
    let der = parse_pem_block(&certificate.certificate_pem, PEM_LABEL_CERTIFICATE)?;
    Some(Certificate::from_der(&der).ok()?.tbs_certificate.subject.to_string())
}

/// The CSV rendering: the column names, then one line per entry, each value quoted so a
/// value carrying a comma or a quote cannot be read as two columns (RFC 4180 section 2).
fn audit_report_csv(entries: &[Vec<(&'static str, String)>]) -> String {
    let mut out = String::new();
    out.push_str(&AUDIT_REPORT_COLUMNS.join(","));
    out.push('\n');
    for entry in entries {
        let line: Vec<String> = entry
            .iter()
            .map(|(_, value)| format!("\"{}\"", value.replace('"', "\"\"")))
            .collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

impl CertificateAuthorityRecord {
    /// The report `id` names among the ones this CA has produced, when it holds one.
    pub fn audit_report(&self, id: &str) -> Option<&AuditReportRecord> {
        self.audit_reports.iter().find(|report| report.id == id)
    }

    /// Keep the record of `report`, dropping the oldest ones past
    /// [`MAX_AUDIT_REPORTS_PER_CA`] so one CA's reports cannot grow without bound.
    pub fn record_audit_report(&mut self, report: AuditReportRecord) {
        self.audit_reports.push(report);
        while self.audit_reports.len() > MAX_AUDIT_REPORTS_PER_CA {
            self.audit_reports.remove(0);
        }
    }
}

/// The object-storage write an audit report goes out as, built while this service's store is
/// held and performed once it has been released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditReportDeposit {
    /// The bucket `CreateCertificateAuthorityAuditReportRequest$S3BucketName` names.
    pub bucket: String,
    /// The object key the report is written at ([`audit_report_key`]).
    pub key: String,
    /// The media type of the format the report was asked for
    /// ([`audit_report_content_type`]).
    pub content_type: &'static str,
    /// The report body itself ([`audit_report_body`]).
    pub body: Vec<u8>,
}

/// Deposit an audit report in object storage, through the same in-process S3 client the CRL
/// bucket is resolved with ([`super::publish_crl`] writes a CRL the same way).
///
/// Answers whether the object was stored: a report the bucket would not take is one the CA
/// cannot report an `S3Key` for, so the call that asked for it is refused rather than
/// answering with a key nothing is at.
pub async fn deposit_audit_report(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    deposit: &AuditReportDeposit,
) -> bool {
    let answer = s3_client(ctx, region)
        .put_object()
        .bucket(&deposit.bucket)
        .key(&deposit.key)
        .content_type(deposit.content_type)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            deposit.body.clone(),
        ))
        .send()
        .await;
    match answer {
        Ok(_) => true,
        Err(error) => {
            ::tracing::warn!(
                bucket = %deposit.bucket,
                key = %deposit.key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the audit report could not be written to object storage"
            );
            false
        }
    }
}
