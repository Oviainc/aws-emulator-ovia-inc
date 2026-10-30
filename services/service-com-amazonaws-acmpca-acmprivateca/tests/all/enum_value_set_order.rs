// SPDX-License-Identifier: CC0-1.0

//! How this service STATES an enumerated type's admissible set when it refuses a value
//! outside it, driven over the wire through the same router the emulator boots.
//!
//! Two rules are under test, neither about one operation or one member:
//!
//! * every constraint refusal opens with the count of values that failed —
//!   `<N> validation error(s) detected: ` — whichever constraint class failed, including a
//!   value outside an enumerated type's set;
//! * the admissible set is listed in the order the set's own hash table is walked in: each
//!   value's bucket ascending, and where two values share a bucket the later-declared one
//!   ahead of the earlier. That order belongs to the enumerated TYPE, so one type reads the
//!   same way under every member and operation carrying it, whatever order the model
//!   happens to declare its values in.
//!
//! The rows differ along what the rules generalise over: which enumerated type failed (two
//! to nine values; types whose stated order is the declared one and types whose is not;
//! types with a bucket collision and types without), how its values are spelled
//! (SCREAMING_SNAKE and mixed case), which operation carried it (seven, one of them still a
//! stub, so the refusal is the request-validation layer's rather than a handler's), where
//! in the request the member sits (top level, a structure, a structure inside a structure,
//! a list element at its 1-based index), and how many values failed at once.
//!
//! Each expected set names its source: the declared values come from
//! `smithy_query(query="shape", shape="<the type>")`, and the membership of every set a
//! refusal states is checked against the model-derived value lists in `ops::types`, so a
//! listing here cannot drift from the type the model declares.
//!
//! The harness (the engine, the router, the awsJson1_1 call) is
//! [`crate::private_ca_lifecycle::Emulator`].

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::ops::types::{
    ACTION_TYPE_VALUES, AUDIT_REPORT_RESPONSE_FORMAT_VALUES, CERTIFICATE_AUTHORITY_STATUS_VALUES,
    CERTIFICATE_AUTHORITY_TYPE_VALUES, RESOURCE_OWNER_VALUES, REVOCATION_REASON_VALUES,
    S3_OBJECT_ACL_VALUES, VALIDITY_PERIOD_TYPE_VALUES,
};

use crate::private_ca_lifecycle::{blob, root_ca_request, string, Emulator, ACCOUNT, REGION};
use crate::resource_policy::error_message;

/// The error code every constraint refusal of this service carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `CertificateAuthorityType`, declared `ROOT`, `SUBORDINATE`
/// (`smithy_query(query="shape", shape="CertificateAuthorityType")`): neither value shares
/// a bucket and the buckets rise with the declaration, so the stated order is the declared
/// one.
const CERTIFICATE_AUTHORITY_TYPE_SET: &str = "ROOT, SUBORDINATE";

/// `RevocationReason`, declared `UNSPECIFIED`, `KEY_COMPROMISE`,
/// `CERTIFICATE_AUTHORITY_COMPROMISE`, `AFFILIATION_CHANGED`, `SUPERSEDED`,
/// `CESSATION_OF_OPERATION`, `PRIVILEGE_WITHDRAWN`, `A_A_COMPROMISE`
/// (`smithy_query(query="shape", shape="RevocationReason")`): three of the eight share one
/// bucket, so they are stated in the reverse of their declaration.
const REVOCATION_REASON_SET: &str = "AFFILIATION_CHANGED, CESSATION_OF_OPERATION, \
                                     A_A_COMPROMISE, PRIVILEGE_WITHDRAWN, SUPERSEDED, \
                                     UNSPECIFIED, KEY_COMPROMISE, \
                                     CERTIFICATE_AUTHORITY_COMPROMISE";

/// `ValidityPeriodType`, declared `END_DATE`, `ABSOLUTE`, `DAYS`, `MONTHS`, `YEARS`
/// (`smithy_query(query="shape", shape="ValidityPeriodType")`): `ABSOLUTE` and `DAYS` share
/// a bucket, so the later-declared `DAYS` is stated ahead of `ABSOLUTE`.
const VALIDITY_PERIOD_TYPE_SET: &str = "END_DATE, MONTHS, YEARS, DAYS, ABSOLUTE";

/// `AuditReportResponseFormat`, declared `JSON`, `CSV`
/// (`smithy_query(query="shape", shape="AuditReportResponseFormat")`): the two values'
/// buckets fall with the declaration, so the stated order is its reverse.
const AUDIT_REPORT_RESPONSE_FORMAT_SET: &str = "CSV, JSON";

/// `S3ObjectAcl`, declared `PUBLIC_READ`, `BUCKET_OWNER_FULL_CONTROL`
/// (`smithy_query(query="shape", shape="S3ObjectAcl")`).
const S3_OBJECT_ACL_SET: &str = "PUBLIC_READ, BUCKET_OWNER_FULL_CONTROL";

/// `ResourceOwner`, declared `SELF`, `OTHER_ACCOUNTS`
/// (`smithy_query(query="shape", shape="ResourceOwner")`).
const RESOURCE_OWNER_SET: &str = "SELF, OTHER_ACCOUNTS";

/// `ActionType`, declared `IssueCertificate`, `GetCertificate`, `ListPermissions`
/// (`smithy_query(query="shape", shape="ActionType")`) — mixed-case values, which the
/// buckets are taken over exactly as spelled.
const ACTION_TYPE_SET: &str = "GetCertificate, ListPermissions, IssueCertificate";

/// `CertificateAuthorityStatus`, declared `CREATING`, `PENDING_CERTIFICATE`, `ACTIVE`,
/// `DELETED`, `DISABLED`, `EXPIRED`, `FAILED`
/// (`smithy_query(query="shape", shape="CertificateAuthorityStatus")`): `CREATING` and
/// `DELETED` share a bucket, so `DELETED` is stated ahead of `CREATING`.
const CERTIFICATE_AUTHORITY_STATUS_SET: &str =
    "EXPIRED, ACTIVE, PENDING_CERTIFICATE, DELETED, CREATING, DISABLED, FAILED";

/// A well-formed CA ARN of this emulator's account and region. No CA answers to it, which
/// is immaterial: a value outside its enum is refused before any CA is resolved.
fn ca_arn() -> String {
    format!(
        "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
         6a1c2e44-9e4f-4b3a-9d91-0c7b5d2f8e13"
    )
}

/// One row: the request to make, the member whose value was outside its enumerated type,
/// the set the refusal must state, and the model-derived list of that type's values.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    path: &'static str,
    value_set: &'static str,
    declared: &'static [&'static str],
}

/// Every enumerated type a request of this service can carry a bad value for, each on an
/// operation and a member of its own.
fn rows() -> Vec<Row> {
    let arn = ca_arn();
    vec![
        Row {
            case: "a CA kind that is neither of the two",
            operation: "CreateCertificateAuthority",
            body: {
                let mut body = root_ca_request("stated-ca-type");
                body["CertificateAuthorityType"] = json!("INTERMEDIATE");
                body
            },
            path: "certificateAuthorityType",
            value_set: CERTIFICATE_AUTHORITY_TYPE_SET,
            declared: CERTIFICATE_AUTHORITY_TYPE_VALUES,
        },
        Row {
            case: "a revocation reason the registry does not define",
            operation: "RevokeCertificate",
            body: json!({
                "CertificateAuthorityArn": arn,
                "CertificateSerial": "0f1e2d3c4b5a69788796a5b4c3d2e1f0",
                "RevocationReason": "LOST_INTEREST",
            }),
            path: "revocationReason",
            value_set: REVOCATION_REASON_SET,
            declared: REVOCATION_REASON_VALUES,
        },
        Row {
            case: "a validity period type inside the validity structure",
            operation: "IssueCertificate",
            body: json!({
                "CertificateAuthorityArn": arn,
                "Csr": blob("a certification request"),
                "SigningAlgorithm": "SHA256WITHRSA",
                "Validity": {"Value": 1, "Type": "FORTNIGHTS"},
            }),
            path: "validity.type",
            value_set: VALIDITY_PERIOD_TYPE_SET,
            declared: VALIDITY_PERIOD_TYPE_VALUES,
        },
        Row {
            case: "an audit report format, on an operation that is still a stub",
            operation: "CreateCertificateAuthorityAuditReport",
            body: json!({
                "CertificateAuthorityArn": arn,
                "S3BucketName": "audit-reports",
                "AuditReportResponseFormat": "XML",
            }),
            path: "auditReportResponseFormat",
            value_set: AUDIT_REPORT_RESPONSE_FORMAT_SET,
            declared: AUDIT_REPORT_RESPONSE_FORMAT_VALUES,
        },
        Row {
            case: "a CRL object ACL, two structures deep",
            operation: "CreateCertificateAuthority",
            body: {
                let mut body = root_ca_request("stated-object-acl");
                body["RevocationConfiguration"] = json!({
                    "CrlConfiguration": {
                        "Enabled": true,
                        "S3BucketName": "crl-bucket",
                        "ExpirationInDays": 7,
                        "S3ObjectAcl": "PRIVATE",
                    }
                });
                body
            },
            path: "revocationConfiguration.crlConfiguration.s3ObjectAcl",
            value_set: S3_OBJECT_ACL_SET,
            declared: S3_OBJECT_ACL_VALUES,
        },
        Row {
            case: "a resource owner a listing cannot filter on",
            operation: "ListCertificateAuthorities",
            body: json!({"ResourceOwner": "EVERY_ACCOUNT"}),
            path: "resourceOwner",
            value_set: RESOURCE_OWNER_SET,
            declared: RESOURCE_OWNER_VALUES,
        },
        Row {
            case: "an action at the second element of a grant's list",
            operation: "CreatePermission",
            body: json!({
                "CertificateAuthorityArn": arn,
                "Principal": "acm.amazonaws.com",
                "Actions": ["GetCertificate", "DeleteCertificate"],
            }),
            path: "actions.2.member",
            value_set: ACTION_TYPE_SET,
            declared: ACTION_TYPE_VALUES,
        },
        Row {
            case: "a status no CA is ever in",
            operation: "UpdateCertificateAuthority",
            body: json!({"CertificateAuthorityArn": arn, "Status": "SUSPENDED"}),
            path: "status",
            value_set: CERTIFICATE_AUTHORITY_STATUS_SET,
            declared: CERTIFICATE_AUTHORITY_STATUS_VALUES,
        },
    ]
}

/// The admissible set a refusal states, as the values it lists in the order it listed them.
fn listed_set(message: &str, case: &str) -> Vec<String> {
    message
        .split_once("enum value set: [")
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .unwrap_or_else(|| panic!("case {case:?}: expected a stated value set, got {message:?}"))
        .split(", ")
        .map(str::to_string)
        .collect()
}

/// Every enumerated type, under the count prefix and in the order this service states that
/// type's values in.
#[tokio::test]
async fn an_enum_refusal_states_the_set_under_the_count_in_the_stated_order() {
    let emulator = Emulator::new();
    for row in rows() {
        let Row {
            case,
            operation,
            body,
            path,
            value_set,
            declared,
        } = row;
        let expected = format!(
            "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
             Member must satisfy enum value set: [{value_set}]"
        );
        let answer = emulator
            .refused(case, operation, &body, VALIDATION_EXCEPTION)
            .await;
        let message = error_message(&answer, case);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // The set STATED is the set the model declares, whatever order it is stated in: a
        // listing that dropped or invented a value would pass the string above only by
        // being wrong in both places at once.
        let mut listed = listed_set(&message, case);
        listed.sort();
        let mut model = declared.to_vec();
        model.sort_unstable();
        assert_eq!(
            listed, model,
            "case {case:?}: expected the declared values {model:?}, got {listed:?}"
        );
    }
}

/// The order belongs to the enumerated type, not to the member carrying it: one request
/// carrying a bad value for the SAME type at two different member paths states one order
/// twice, under the count of both failures.
#[tokio::test]
async fn one_type_reads_the_same_way_under_every_member_carrying_it() {
    let emulator = Emulator::new();
    let case = "both validity structures outside the period type";
    let body = json!({
        "CertificateAuthorityArn": ca_arn(),
        "Csr": blob("a certification request"),
        "SigningAlgorithm": "SHA256WITHRSA",
        "Validity": {"Value": 1, "Type": "FORTNIGHTS"},
        "ValidityNotBefore": {"Value": 1, "Type": "DECADES"},
    });
    let answer = emulator
        .refused(case, "IssueCertificate", &body, VALIDATION_EXCEPTION)
        .await;
    let message = error_message(&answer, case);
    for path in ["validity.type", "validityNotBefore.type"] {
        let clause = format!(
            "Value at '{path}' failed to satisfy constraint: Member must satisfy enum value \
             set: [{VALIDITY_PERIOD_TYPE_SET}]"
        );
        assert!(
            message.contains(&clause),
            "case {case:?}: expected the clause {clause:?}, got {message:?}"
        );
    }
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: expected the count of both failures, got {message:?}"
    );
}

/// The count is how many values failed, and it is stated for an enumerated type's set
/// beside any other constraint class: one request failing an enum and a length at once is
/// refused under the plural count, with both clauses in it.
#[tokio::test]
async fn the_count_prefix_counts_the_values_that_failed() {
    let emulator = Emulator::new();
    let case = "a CA kind outside its type and a common name past its ceiling";
    let mut body = root_ca_request(&"k".repeat(65));
    body["CertificateAuthorityType"] = json!("INTERMEDIATE");
    let answer = emulator
        .refused(case, "CreateCertificateAuthority", &body, VALIDATION_EXCEPTION)
        .await;
    let message = error_message(&answer, case);
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: expected the plural count of two failures, got {message:?}"
    );
    for clause in [
        format!(
            "Value at 'certificateAuthorityType' failed to satisfy constraint: Member must \
             satisfy enum value set: [{CERTIFICATE_AUTHORITY_TYPE_SET}]"
        ),
        "Value at 'certificateAuthorityConfiguration.subject.commonName' failed to satisfy \
         constraint: Member must have length less than or equal to 64"
            .to_string(),
    ] {
        assert!(
            message.contains(&clause),
            "case {case:?}: expected the clause {clause:?}, got {message:?}"
        );
    }
}

/// The set a refusal states is the set the operation ACCEPTS: every value listed for the CA
/// kind is admitted by a create and read back off the stored CA, and asking in the reverse
/// of the stated order changes nothing about how the set is stated.
#[tokio::test]
async fn every_value_a_stated_set_lists_is_accepted_and_stored() {
    let emulator = Emulator::new();
    let case = "the stated CA kinds";
    let mut body = root_ca_request("stated-kinds");
    body["CertificateAuthorityType"] = json!("INTERMEDIATE");
    let answer = emulator
        .refused(case, "CreateCertificateAuthority", &body, VALIDATION_EXCEPTION)
        .await;
    let listed = listed_set(&error_message(&answer, case), case);
    assert_eq!(
        listed,
        CERTIFICATE_AUTHORITY_TYPE_SET
            .split(", ")
            .map(str::to_string)
            .collect::<Vec<String>>(),
        "case {case:?}: expected the stated order, got {listed:?}"
    );

    for kind in listed.iter().rev() {
        let case = format!("a {kind} CA");
        let mut body = root_ca_request(&format!("stated-{kind}"));
        body["CertificateAuthorityType"] = json!(kind);
        let created = emulator
            .ok(&case, "CreateCertificateAuthority", &body)
            .await;
        let arn = string(&created, "CertificateAuthorityArn", &case);
        let described = emulator
            .ok(
                &case,
                "DescribeCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        let authority = described
            .get("CertificateAuthority")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no CertificateAuthority in {described:?}"));
        assert_eq!(
            string(&authority, "Type", &case),
            *kind,
            "case {case:?}: expected the stored kind {kind}, got {authority:?}"
        );
    }
}
