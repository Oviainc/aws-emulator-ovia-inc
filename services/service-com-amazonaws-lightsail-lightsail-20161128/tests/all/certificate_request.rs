// SPDX-License-Identifier: CC0-1.0

//! Requesting a Lightsail SSL/TLS certificate, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `CreateCertificate` stores ONE certificate record per
//! request — the name, the domain, the alternate domains and the tags the
//! request named, kept verbatim — assigns every other member from that request
//! plus the store's own allocators (the ARN and the support code from the minted
//! id, the serial number from that same id, the creation instant from the
//! clock), reports it `PENDING_VALIDATION` until the issuance the clock owes it
//! falls due, and answers the one operation-log entry the request appended; so
//! every later `GetCertificates` reports exactly what the create stored. A
//! request the service cannot serve — a domain that is not a domain name, more
//! alternate domains than the documented nine, a name this account already holds
//! a certificate under, or a request made outside the one Region the certificate
//! API serves — is refused with the modelled `InvalidInputException` and stores
//! nothing.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check and the issuance wait
// this test target already exposes: a sibling suite drives the service through
// the same front door, and waits for the same clock, rather than standing up a
// second one.
use crate::distribution_certificate_attachment::{certificate, wait_for_issued_certificate};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The one Region a Lightsail certificate lives in and is served from, from the
/// modelled `CreateCertificate` documentation ("Only certificates created in the
/// us-east-1 Amazon Web Services Region can be attached to Lightsail
/// distributions").
const CERTIFICATE_REGION: &str = "us-east-1";

/// The `CertificateStatus` a freshly requested certificate is in, and the one
/// the certificate authority issues it into, from the modelled enum.
const PENDING_VALIDATION: &str = "PENDING_VALIDATION";
/// See [`PENDING_VALIDATION`].
const ISSUED: &str = "ISSUED";

/// The `CertificateDomainValidationStatus` of a domain whose validation is still
/// outstanding, and of one that has been validated, from the modelled enum.
const DOMAIN_PENDING: &str = "PENDING_VALIDATION";
/// See [`DOMAIN_PENDING`].
const DOMAIN_SUCCESS: &str = "SUCCESS";

/// The algorithm a Lightsail certificate's key pair is generated with, as
/// `Certificate$keyAlgorithm` reports it, and the authority that issues it, as
/// `Certificate$issuerCA` reports it.
const KEY_ALGORITHM: &str = "RSA_2048";
/// See [`KEY_ALGORITHM`].
const ISSUER_CA: &str = "Amazon";

/// How long an issued certificate is valid for, in seconds: AWS documents that
/// an ACM certificate — which a Lightsail certificate is — is "valid for 13
/// months (395 days)".
const VALIDITY_SECONDS: f64 = 395.0 * 24.0 * 60.0 * 60.0;

/// The most alternate domains one certificate may carry, from the modelled
/// `CreateCertificate$subjectAlternativeNames` documentation ("You can specify a
/// maximum of nine alternate domains (in addition to the primary domain
/// name)").
const MAX_ALTERNATE_DOMAINS: usize = 9;

/// The `operationType` the log entry of this call carries, from the modelled
/// `OperationType` enum, and the `ResourceType` a certificate reports.
const OPERATION_TYPE: &str = "CreateCertificate";
/// See [`OPERATION_TYPE`].
const RESOURCE_TYPE: &str = "Certificate";

/// The `Operation$status` of work the clock still owes a terminal status, from
/// the modelled `OperationStatus` enum.
const STARTED: &str = "Started";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The wire error code of the refusal a request the model admits but the service
/// does not serve answers with.
const INVALID_INPUT: &str = "InvalidInputException";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Now, in epoch seconds, for the window a reported `createdAt` must fall in.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The serial number the certificate minted as `id` reports: an X.509 serial
/// number is printed as colon-separated hex byte pairs, and this service builds
/// one from the certificate's own minted id, so the same id always yields the
/// same serial.
///
/// `pub(crate)` so a sibling suite in this test target checks a minted serial the
/// same way rather than carrying a second copy of the derivation.
pub(crate) fn serial_number_of(id: &str) -> String {
    let digits: Vec<char> = id.chars().filter(char::is_ascii_hexdigit).collect();
    digits
        .chunks(2)
        .filter(|pair| pair.len() == 2)
        .map(|pair| pair.iter().collect::<String>())
        .collect::<Vec<String>>()
        .join(":")
}

/// Every certificate name the account holds, walking every page of the
/// unfiltered listing.
async fn all_certificate_names(emulator: &Emulator) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => emulator.ok("GetCertificates", json!({"pageToken": token})).await,
            None => emulator.ok("GetCertificates", json!({})).await,
        };
        let page = body["certificates"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case listing certificates: expected a certificates array, got {body:?}")
            })
            .clone();
        for held in page {
            names.push(
                held["certificateName"]
                    .as_str()
                    .unwrap_or("<not a string>")
                    .to_string(),
            );
        }
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            names.len() < 1_000,
            "case listing certificates: the walk did not terminate after {} names",
            names.len()
        );
    }
    names
}

/// Assert that `name` names no certificate of this account: the read is refused
/// with the `DoesNotExist` envelope and the listing holds nothing under it.
async fn assert_nothing_stored(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = emulator
        .call("GetCertificates", json!({"certificateName": name}))
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected a refused create to store no certificate, and reading {name:?} \
         answered {status} {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected reading {name:?} to answer {DOES_NOT_EXIST}, got {body:?}"
    );
    let listed = all_certificate_names(emulator).await;
    assert!(
        !listed.iter().any(|held| held == name),
        "case {case}: expected {name:?} to be listed nowhere, got {listed:?}"
    );
}

/// The `tags` member for the `(key, value)` pairs `tags` names, left out
/// entirely when the request names none.
fn tag_list(tags: &[(&str, Option<&str>)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| match value {
                Some(value) => json!({"key": key, "value": value}),
                None => json!({"key": key}),
            })
            .collect(),
    )
}

/// Assert that `reported` is the tag list `want` asked for, in the order the
/// request named it.
fn assert_tags(case: &str, reported: &Value, want: &[(&str, Option<&str>)]) {
    let reported = reported.as_array().cloned().unwrap_or_default();
    assert_eq!(
        reported.len(),
        want.len(),
        "case {case}: expected the {} tags the request named, got {reported:?}",
        want.len()
    );
    for (entry, (key, value)) in reported.iter().zip(want) {
        assert_eq!(
            entry["key"].as_str(),
            Some(*key),
            "case {case}: expected the tag key {key:?}, got {entry:?}"
        );
        assert_eq!(
            entry["value"].as_str(),
            *value,
            "case {case}: expected the tag value {value:?}, got {entry:?}"
        );
    }
}

/// Assert that `reported` is the string list `want`, in order.
fn assert_string_list(case: &str, member: &str, reported: &Value, want: &[&str]) {
    let reported: Vec<&str> = reported
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or("<not a string>"))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported, want,
        "case {case}: expected {member} to be {want:?}, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The served table
// ---------------------------------------------------------------------------

/// One `CreateCertificate` request the service must serve, and what the stored
/// certificate then reports.
///
/// The rows differ along everything the rule generalises over: how many alternate
/// domains the request names (none, a couple, the documented maximum), whether
/// one of them repeats the primary domain, how many tags come with it, and the
/// shape of the domain the certificate is for (an apex, a deep subdomain,
/// hyphenated labels, a multi-label suffix).
struct Requested {
    case: &'static str,
    certificate_name: &'static str,
    domain_name: &'static str,
    /// The alternate domains the request names, kept and reported verbatim.
    subject_alternative_names: &'static [&'static str],
    tags: &'static [(&'static str, Option<&'static str>)],
    /// Every domain the certificate covers, in order: the domain it is for, then
    /// each alternate domain the request named that is not already covered. This
    /// is what the certificate's domain-validation records are about.
    covered_domains: &'static [&'static str],
}

/// The table.
const REQUESTED: &[Requested] = &[
    Requested {
        case: "the domain alone, with no alternate domain and no tag",
        certificate_name: "plain-cert",
        domain_name: "plain.example.com",
        subject_alternative_names: &[],
        tags: &[],
        covered_domains: &["plain.example.com"],
    },
    Requested {
        case: "two alternate domains and two tags, one of them valueless",
        certificate_name: "tagged-cert",
        domain_name: "tagged.example.net",
        subject_alternative_names: &["www.tagged.example.net", "img.tagged.example.net"],
        tags: &[("env", Some("staging")), ("owner", None)],
        covered_domains: &[
            "tagged.example.net",
            "www.tagged.example.net",
            "img.tagged.example.net",
        ],
    },
    Requested {
        // The published bound: nine alternate domains "in addition to the
        // primary domain name".
        case: "the documented nine alternate domains",
        certificate_name: "nine-cert",
        domain_name: "nine.example.org",
        subject_alternative_names: &[
            "a1.nine.example.org",
            "a2.nine.example.org",
            "a3.nine.example.org",
            "a4.nine.example.org",
            "a5.nine.example.org",
            "a6.nine.example.org",
            "a7.nine.example.org",
            "a8.nine.example.org",
            "a9.nine.example.org",
        ],
        tags: &[("tier", Some("edge"))],
        covered_domains: &[
            "nine.example.org",
            "a1.nine.example.org",
            "a2.nine.example.org",
            "a3.nine.example.org",
            "a4.nine.example.org",
            "a5.nine.example.org",
            "a6.nine.example.org",
            "a7.nine.example.org",
            "a8.nine.example.org",
            "a9.nine.example.org",
        ],
    },
    Requested {
        // An alternate domain that repeats the primary one adds no domain to the
        // certificate, so it is covered once — while the request's own list is
        // still reported as the caller sent it.
        case: "an alternate domain that repeats the certificate's domain",
        certificate_name: "repeat-cert",
        domain_name: "apex.example.com",
        subject_alternative_names: &["apex.example.com", "blog.apex.example.com"],
        tags: &[],
        covered_domains: &["apex.example.com", "blog.apex.example.com"],
    },
    Requested {
        case: "a deep subdomain with hyphenated labels under a multi-label suffix",
        certificate_name: "hyphen-cert",
        domain_name: "store-front.shop.example-site.co.uk",
        subject_alternative_names: &["cdn.store-front.shop.example-site.co.uk"],
        tags: &[("cost-centre", Some("42"))],
        covered_domains: &[
            "store-front.shop.example-site.co.uk",
            "cdn.store-front.shop.example-site.co.uk",
        ],
    },
];

/// The `CreateCertificate` body `row` describes.
fn create_body(row: &Requested) -> Value {
    let mut body = json!({
        "certificateName": row.certificate_name,
        "domainName": row.domain_name,
    });
    if !row.subject_alternative_names.is_empty() {
        body["subjectAlternativeNames"] = json!(row.subject_alternative_names);
    }
    if !row.tags.is_empty() {
        body["tags"] = tag_list(row.tags);
    }
    body
}

/// RULE: the request's own members are stored verbatim, every other member is
/// computed from that request plus the store's allocators, the certificate is
/// reported pending validation until the issuance the clock owes it falls due,
/// and a later read reports exactly what the create stored.
#[tokio::test]
async fn requesting_a_certificate_stores_the_record_every_later_read_reports() {
    let emulator = Emulator::new();

    for row in REQUESTED {
        let case = row.case;
        let requested_after = now_epoch_seconds();
        let answer = emulator.ok("CreateCertificate", create_body(row)).await;
        let created = answer["certificate"].clone();

        // What the request asked for, answered back.
        assert_eq!(
            created["certificateName"].as_str(),
            Some(row.certificate_name),
            "case {case}: expected the name the request asked for, got {created:?}"
        );
        assert_eq!(
            created["domainName"].as_str(),
            Some(row.domain_name),
            "case {case}: expected the domain the request asked for, got {created:?}"
        );
        assert_tags(case, &created["tags"], row.tags);

        // The detail of the record the caller just asked for.
        let detail = created["certificateDetail"].clone();
        assert!(
            detail.is_object(),
            "case {case}: expected the create to answer the certificate it created in full, got \
             {created:?}"
        );
        assert_eq!(
            detail["name"].as_str(),
            Some(row.certificate_name),
            "case {case}: expected the detail to carry the same name, got {detail:?}"
        );
        assert_string_list(
            case,
            "subjectAlternativeNames",
            &detail["subjectAlternativeNames"],
            row.subject_alternative_names,
        );

        // What the service assigns, each computed from the request plus the
        // store's own allocators.
        let arn_prefix = format!("arn:aws:lightsail:{CERTIFICATE_REGION}:{ACCOUNT}:Certificate/");
        let arn = created["certificateArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a certificateArn, got {created:?}"));
        let id = arn.strip_prefix(&arn_prefix).unwrap_or_else(|| {
            panic!("case {case}: expected an arn under {arn_prefix:?}, got {arn:?}")
        });
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped arn id, got {id:?}"
        );
        assert_eq!(
            detail["arn"].as_str(),
            Some(arn),
            "case {case}: expected the summary and the detail to name one ARN, got {created:?}"
        );
        assert_eq!(
            detail["supportCode"].as_str(),
            Some(format!("{ACCOUNT}/{id}").as_str()),
            "case {case}: the support code must name the account and this certificate's id, got \
             {detail:?}"
        );
        let want_serial = serial_number_of(id);
        assert_eq!(
            detail["serialNumber"].as_str(),
            Some(want_serial.as_str()),
            "case {case}: expected a serial number built from this certificate's id, got \
             {detail:?}"
        );
        assert_eq!(
            detail["keyAlgorithm"].as_str(),
            Some(KEY_ALGORITHM),
            "case {case}: expected the {KEY_ALGORITHM} key algorithm, got {detail:?}"
        );
        assert_eq!(
            detail["inUseResourceCount"].as_i64(),
            Some(0),
            "case {case}: expected a certificate attached to nothing to be in use by 0 resources, \
             got {detail:?}"
        );
        let created_at = detail["createdAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: expected a createdAt, got {detail:?}"));
        assert!(
            created_at >= requested_after - 300.0 && created_at <= now_epoch_seconds() + 300.0,
            "case {case}: expected the creation time to be now, got {created_at}"
        );

        // The create's own answer, so no clock has run yet: the certificate
        // authority validates the domains before it issues anything, so nothing
        // of the issuance is reported yet.
        assert_eq!(
            detail["status"].as_str(),
            Some(PENDING_VALIDATION),
            "case {case}: expected a freshly requested certificate to be {PENDING_VALIDATION}, \
             got {detail:?}"
        );
        for member in ["issuedAt", "issuerCA", "notBefore", "notAfter"] {
            assert!(
                detail[member].is_null(),
                "case {case}: expected no {member} before the certificate is issued, got {detail:?}"
            );
        }
        let records = detail["domainValidationRecords"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            records.len(),
            row.covered_domains.len(),
            "case {case}: expected one validation record per domain the certificate covers \
             ({:?}), got {records:?}",
            row.covered_domains
        );
        for (record, domain) in records.iter().zip(row.covered_domains) {
            assert_eq!(
                record["domainName"].as_str(),
                Some(*domain),
                "case {case}: expected a validation record for {domain}, got {record:?}"
            );
            assert_eq!(
                record["validationStatus"].as_str(),
                Some(DOMAIN_PENDING),
                "case {case}: expected the validation of {domain} to still be {DOMAIN_PENDING}, \
                 got {record:?}"
            );
        }

        // The one operation-log entry this request appended: it asked for one
        // certificate, so it answers one entry, left non-terminal until the
        // issuance the clock owes the certificate is due.
        let operations = answer["operations"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected exactly one operation-log entry, got {operations:?}"
        );
        let operation = &operations[0];
        assert!(
            operation["id"].as_str().is_some_and(is_uuid_shaped),
            "case {case}: expected a uuid-shaped operation id, got {operation:?}"
        );
        assert_eq!(
            operation["operationType"].as_str(),
            Some(OPERATION_TYPE),
            "case {case}: expected the {OPERATION_TYPE} operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some(RESOURCE_TYPE),
            "case {case}: expected the {RESOURCE_TYPE} resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.certificate_name),
            "case {case}: expected the entry to be about the certificate requested, got \
             {operation:?}"
        );
        assert_eq!(
            operation["status"].as_str(),
            Some(STARTED),
            "case {case}: expected work the clock still owes to be {STARTED}, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: expected work the clock still owes to be non-terminal, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(CERTIFICATE_REGION),
            "case {case}: expected the entry located where the certificate is, got {operation:?}"
        );

        // The round trip: what the create stored is what a later read reports.
        // Only the members the clock cannot touch are compared here — the
        // issuance it owes is asserted below.
        let read = certificate(&emulator, row.certificate_name).await;
        let read_detail = read["certificateDetail"].clone();
        for member in ["certificateArn", "certificateName", "domainName", "tags"] {
            assert_eq!(
                read[member], created[member],
                "case {case}: expected the read to report the {member} the create stored, got \
                 {read:?}"
            );
        }
        for member in [
            "arn",
            "name",
            "domainName",
            "subjectAlternativeNames",
            "serialNumber",
            "supportCode",
            "keyAlgorithm",
            "createdAt",
            "tags",
        ] {
            assert_eq!(
                read_detail[member], detail[member],
                "case {case}: expected the read to report the {member} the create stored, got \
                 {read_detail:?}"
            );
        }

        // Without `includeCertificateDetails` the same record is reported as the
        // summary alone.
        let summary = emulator
            .ok(
                "GetCertificates",
                json!({"certificateName": row.certificate_name}),
            )
            .await["certificates"][0]
            .clone();
        assert!(
            summary["certificateDetail"].is_null(),
            "case {case}: expected no detail unless the read asked for it, got {summary:?}"
        );
        assert_eq!(
            summary["certificateArn"].as_str(),
            Some(arn),
            "case {case}: expected the summary to carry the stored ARN, got {summary:?}"
        );

        // The issuance the clock owes: it arrives without anyone asking, and
        // leaves everything the request stored alone.
        let issued = wait_for_issued_certificate(&emulator, row.certificate_name).await;
        let issued_detail = issued["certificateDetail"].clone();
        assert_eq!(
            issued_detail["status"].as_str(),
            Some(ISSUED),
            "case {case}: expected the clock to issue the certificate, got {issued_detail:?}"
        );
        assert_eq!(
            issued_detail["issuerCA"].as_str(),
            Some(ISSUER_CA),
            "case {case}: expected {ISSUER_CA} as the issuing authority, got {issued_detail:?}"
        );
        for member in ["arn", "serialNumber", "supportCode", "createdAt"] {
            assert_eq!(
                issued_detail[member], detail[member],
                "case {case}: expected issuance to leave the {member} the create stored alone, \
                 got {issued_detail:?}"
            );
        }
        let not_before = issued_detail["notBefore"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: expected an issued certificate to report notBefore, got {issued_detail:?}")
        });
        let not_after = issued_detail["notAfter"].as_f64().unwrap_or_else(|| {
            panic!("case {case}: expected an issued certificate to report notAfter, got {issued_detail:?}")
        });
        assert_eq!(
            issued_detail["issuedAt"].as_f64(),
            Some(not_before),
            "case {case}: expected the validity window to start when the certificate was issued, \
             got {issued_detail:?}"
        );
        assert!(
            (not_after - not_before - VALIDITY_SECONDS).abs() < 1.0,
            "case {case}: expected a validity window of {VALIDITY_SECONDS} seconds, got \
             {not_before} to {not_after}"
        );
        assert!(
            not_before >= created_at,
            "case {case}: expected the certificate issued no earlier than it was requested, got \
             {not_before} against {created_at}"
        );
        let issued_records = issued_detail["domainValidationRecords"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            issued_records.len(),
            row.covered_domains.len(),
            "case {case}: expected issuance to leave the validation records in place, got \
             {issued_records:?}"
        );
        for (record, domain) in issued_records.iter().zip(row.covered_domains) {
            assert_eq!(
                record["domainName"].as_str(),
                Some(*domain),
                "case {case}: expected a validation record for {domain}, got {record:?}"
            );
            assert_eq!(
                record["validationStatus"].as_str(),
                Some(DOMAIN_SUCCESS),
                "case {case}: expected the validation of {domain} to be {DOMAIN_SUCCESS} once the \
                 certificate is issued, got {record:?}"
            );
        }
    }

    // Every certificate the table asked for, and nothing else.
    let mut want: Vec<&str> = REQUESTED.iter().map(|row| row.certificate_name).collect();
    want.sort_unstable();
    let listed = all_certificate_names(&emulator).await;
    assert_eq!(
        listed, want,
        "case the account's certificates: expected one record per served request, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusal table
// ---------------------------------------------------------------------------

/// One `CreateCertificate` request the model admits and the service does not
/// serve.
///
/// The rows differ along what the rule generalises over: which member is wrong
/// (the name, the domain, the alternate domains), and how it is wrong (a domain
/// that is not a domain name at all, a wildcard, an empty or over-long label, a
/// label hyphenated at its edge, one entry more than the documented maximum).
struct Refused {
    case: &'static str,
    certificate_name: &'static str,
    domain_name: &'static str,
    subject_alternative_names: &'static [&'static str],
    /// The request member the refusal must name, so a caller learns what to
    /// change.
    names_member: &'static str,
}

/// A domain label of 64 characters — one past the 63 a DNS label may carry.
const OVER_LONG_LABEL: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.example.com";

/// Ten alternate domains: one past the documented
/// [`MAX_ALTERNATE_DOMAINS`].
const TEN_ALTERNATE_DOMAINS: &[&str] = &[
    "b1.ten.example.com",
    "b2.ten.example.com",
    "b3.ten.example.com",
    "b4.ten.example.com",
    "b5.ten.example.com",
    "b6.ten.example.com",
    "b7.ten.example.com",
    "b8.ten.example.com",
    "b9.ten.example.com",
    "b10.ten.example.com",
];

/// The table.
const REFUSED: &[Refused] = &[
    Refused {
        case: "a certificate name of whitespace alone",
        certificate_name: "   ",
        domain_name: "blank.example.com",
        subject_alternative_names: &[],
        names_member: "certificateName",
    },
    Refused {
        case: "a domain name of a single label",
        certificate_name: "one-label-cert",
        domain_name: "example",
        subject_alternative_names: &[],
        names_member: "domainName",
    },
    Refused {
        // "Wildcard domain entries (*.example.com) are not supported."
        case: "a wildcard domain name",
        certificate_name: "wildcard-cert",
        domain_name: "*.example.com",
        subject_alternative_names: &[],
        names_member: "domainName",
    },
    Refused {
        case: "a domain name with an empty label",
        certificate_name: "empty-label-cert",
        domain_name: "empty..example.com",
        subject_alternative_names: &[],
        names_member: "domainName",
    },
    Refused {
        case: "a domain label that ends with a hyphen",
        certificate_name: "trailing-hyphen-cert",
        domain_name: "trailing-.example.com",
        subject_alternative_names: &[],
        names_member: "domainName",
    },
    Refused {
        case: "a domain label one character past the 63 a label may carry",
        certificate_name: "long-label-cert",
        domain_name: OVER_LONG_LABEL,
        subject_alternative_names: &[],
        names_member: "domainName",
    },
    Refused {
        case: "a wildcard among the alternate domains",
        certificate_name: "wildcard-alternate-cert",
        domain_name: "alt.example.com",
        subject_alternative_names: &["*.alt.example.com"],
        names_member: "subjectAlternativeNames",
    },
    Refused {
        case: "an alternate domain of a single label",
        certificate_name: "one-label-alternate-cert",
        domain_name: "alt2.example.com",
        subject_alternative_names: &["localhost"],
        names_member: "subjectAlternativeNames",
    },
    Refused {
        case: "one alternate domain more than the documented nine",
        certificate_name: "ten-alternates-cert",
        domain_name: "ten.example.com",
        subject_alternative_names: TEN_ALTERNATE_DOMAINS,
        names_member: "subjectAlternativeNames",
    },
];

/// RULE: a request the service cannot serve is refused with the modelled
/// `InvalidInputException`, whose message names the member that is wrong, and
/// stores no certificate at all.
#[tokio::test]
async fn requesting_a_certificate_the_service_cannot_serve_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    for refused in REFUSED {
        let case = refused.case;
        let mut body = json!({
            "certificateName": refused.certificate_name,
            "domainName": refused.domain_name,
        });
        if !refused.subject_alternative_names.is_empty() {
            body["subjectAlternativeNames"] = json!(refused.subject_alternative_names);
        }
        let (status, answer) = emulator.call("CreateCertificate", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, INVALID_INPUT,
            "case {case}: expected the wire error code {INVALID_INPUT}, got {code} {answer:?}"
        );
        let member = refused.names_member;
        assert!(
            answer["message"]
                .as_str()
                .is_some_and(|message| message.contains(member)),
            "case {case}: expected a message naming {member}, got {answer:?}"
        );
        assert_nothing_stored(&emulator, case, refused.certificate_name).await;
    }

    // Nine alternate domains is the documented maximum, so the request the table
    // refused for carrying ten is served when it carries nine: the bound is
    // where the refusal starts, not one short of it.
    let nine: Vec<&str> = TEN_ALTERNATE_DOMAINS[..MAX_ALTERNATE_DOMAINS].to_vec();
    let served = emulator
        .ok(
            "CreateCertificate",
            json!({
                "certificateName": "nine-alternates-cert",
                "domainName": "ten.example.com",
                "subjectAlternativeNames": nine,
            }),
        )
        .await["certificate"]
        .clone();
    assert_string_list(
        "the documented maximum of alternate domains",
        "subjectAlternativeNames",
        &served["certificateDetail"]["subjectAlternativeNames"],
        &nine,
    );
}

/// RULE: a certificate name addresses one certificate of the account, so a
/// request under a name this account already holds one under is refused and
/// leaves the record it holds untouched.
#[tokio::test]
async fn requesting_a_certificate_under_a_held_name_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let held = emulator
        .ok(
            "CreateCertificate",
            json!({
                "certificateName": "held-name-cert",
                "domainName": "first.example.com",
                "subjectAlternativeNames": ["www.first.example.com"],
                "tags": [{"key": "generation", "value": "first"}],
            }),
        )
        .await["certificate"]
        .clone();

    // Every way a second request can differ from the first and still name the
    // same certificate: another domain, another set of alternate domains,
    // another set of tags, or the very same request again.
    let repeats = [
        (
            "another domain under the held name",
            json!({"certificateName": "held-name-cert", "domainName": "second.example.com"}),
        ),
        (
            "the held name with other alternate domains",
            json!({
                "certificateName": "held-name-cert",
                "domainName": "first.example.com",
                "subjectAlternativeNames": ["other.first.example.com"],
            }),
        ),
        (
            "the very same request again",
            json!({
                "certificateName": "held-name-cert",
                "domainName": "first.example.com",
                "subjectAlternativeNames": ["www.first.example.com"],
                "tags": [{"key": "generation", "value": "first"}],
            }),
        ),
    ];

    for (case, body) in repeats {
        let (status, answer) = emulator.call("CreateCertificate", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, INVALID_INPUT,
            "case {case}: expected the wire error code {INVALID_INPUT}, got {code} {answer:?}"
        );
        assert!(
            answer["message"]
                .as_str()
                .is_some_and(|message| message.contains("held-name-cert")),
            "case {case}: expected a message naming the certificate already held, got {answer:?}"
        );

        // The record the account holds is the one the FIRST request stored.
        let read = certificate(&emulator, "held-name-cert").await;
        for member in ["certificateArn", "certificateName", "domainName", "tags"] {
            assert_eq!(
                read[member], held[member],
                "case {case}: expected the refused request to leave the {member} of the held \
                 certificate alone, got {read:?}"
            );
        }
        assert_eq!(
            read["certificateDetail"]["serialNumber"],
            held["certificateDetail"]["serialNumber"],
            "case {case}: expected the refused request to mint no second certificate, got {read:?}"
        );
        assert_string_list(
            case,
            "subjectAlternativeNames",
            &read["certificateDetail"]["subjectAlternativeNames"],
            &["www.first.example.com"],
        );
    }

    let listed = all_certificate_names(&emulator).await;
    assert_eq!(
        listed,
        vec!["held-name-cert"],
        "case the account's certificates: expected the one record the served request stored, got \
         {listed:?}"
    );
}

/// RULE: the certificate API is served from the one Region its resources live in
/// ("Domain and distribution APIs are only available in the N. Virginia
/// (us-east-1) Amazon Web Services Region"), so a request made in any other
/// Region is refused and stores nothing.
#[tokio::test]
async fn requesting_a_certificate_outside_the_region_the_api_serves_is_refused() {
    let emulator = Emulator::new();

    // Regions from the modelled `RegionName` enum, one per continent the service
    // publishes, plus the one Region the API is served from.
    for region in ["us-west-2", "eu-west-1", "ap-northeast-1", "ca-central-1"] {
        let case = format!("a request made in {region}");
        let name = format!("{region}-cert");
        let (status, answer) = emulator
            .call_in(
                Some(region),
                "CreateCertificate",
                json!({"certificateName": &name, "domainName": "elsewhere.example.com"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, INVALID_INPUT,
            "case {case}: expected the wire error code {INVALID_INPUT}, got {code} {answer:?}"
        );
        assert!(
            answer["message"]
                .as_str()
                .is_some_and(|message| message.contains(CERTIFICATE_REGION)),
            "case {case}: expected a message naming the Region the API is served from, got \
             {answer:?}"
        );
        assert_nothing_stored(&emulator, &case, &name).await;
    }

    // The same request, made in the Region the API is served from, is served —
    // and the certificate it stores is located there.
    let served = emulator
        .ok_at(
            None,
            Some(CERTIFICATE_REGION),
            "CreateCertificate",
            json!({"certificateName": "in-region-cert", "domainName": "elsewhere.example.com"}),
        )
        .await;
    assert_eq!(
        served["certificate"]["certificateName"].as_str(),
        Some("in-region-cert"),
        "case a request made in {CERTIFICATE_REGION}: expected the certificate to be created, got \
         {served:?}"
    );
    assert_eq!(
        served["operations"][0]["location"]["regionName"].as_str(),
        Some(CERTIFICATE_REGION),
        "case a request made in {CERTIFICATE_REGION}: expected the entry located where the \
         certificate is, got {served:?}"
    );
}
