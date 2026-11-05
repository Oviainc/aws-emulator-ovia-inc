// SPDX-License-Identifier: CC0-1.0

//! What this service says when a request carries a value outside a constraint its model
//! declares, driven over the wire through the same router the emulator boots.
//!
//! The RULE under test, which is about the refusal CLASS and not about any one request or
//! member: a constraint violation is refused 400 `ValidationException` whose message states
//! how many values failed and then, per failed value, the wire path of the MEMBER and the
//! requirement it failed — `<N> validation error(s) detected: Value at '<member path>'
//! failed to satisfy constraint: <requirement>` — with
//!
//! * the submitted VALUE never stated (only an absent member is named, as `Value null`);
//! * a `@pattern` quoted as the expression the value must match, with no `^`/`$` anchors;
//! * one member that failed two of its constraints stating the EXPRESSION before the SIZE;
//! * a violation of an enumerated type's value set stating the member and the admissible
//!   set, under the same count prefix as every other constraint class (the ORDER the set
//!   is listed in is `crate::enum_value_set_order`'s subject).
//!
//! The rows differ along what the rule generalises over: which constraint failed (length
//! floor, length ceiling, range ceiling, pattern, enum value set, an absent required
//! member), which operation carried it, how deep in the request the member sits (a
//! top-level member, a nested structure's member, a list element's member at its 1-based
//! index), and how many constraints one value failed at once. The last test crosses each
//! bounded member with its published bounds, so a value AT a bound is not refused at all.
//!
//! The harness (the engine, the router, the awsJson1_1 call) is
//! [`crate::private_ca_lifecycle::Emulator`].

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::CA_NOT_FOUND_MESSAGE;

use crate::private_ca_lifecycle::{root_ca_request, string, Emulator, ACCOUNT, REGION};
use crate::resource_policy::error_message;

/// The error code every constraint refusal of this service carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One clause of the message: the member that failed and the requirement it failed, with
/// the submitted value not stated.
fn clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The message over `clauses`: how many values failed, then the clauses in order.
fn envelope(clauses: &[String]) -> String {
    let errors = if clauses.len() == 1 { "error" } else { "errors" };
    format!(
        "{} validation {errors} detected: {}",
        clauses.len(),
        clauses.join("; ")
    )
}

/// A `CreateCertificateAuthority` request whose `Subject` carries `members` beside the
/// common name, so one row can place a bad value anywhere inside the nested subject.
fn ca_with_subject(label: &str, members: &[(&str, Value)]) -> Value {
    let mut body = root_ca_request(label);
    for (member, value) in members {
        body["CertificateAuthorityConfiguration"]["Subject"][*member] = value.clone();
    }
    body
}

/// A `CreateCertificateAuthority` request carrying `tags`.
fn ca_with_tags(label: &str, tags: Value) -> Value {
    let mut body = root_ca_request(label);
    body["Tags"] = tags;
    body
}

/// A well-formed CA ARN of this emulator's account and region, which no CA answers to —
/// the handle a row uses when the point of the row is not the ARN.
fn unknown_ca_arn() -> String {
    format!(
        "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
         d0904488-127f-411c-a2d0-b289cadd9abc"
    )
}

/// A string of `length` ASCII letters, for a row that is about a length bound.
fn letters(length: usize) -> String {
    "k".repeat(length)
}

/// One row: the request to make, and the message the rule says it is refused with.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The submitted values the message must NOT echo back.
    not_echoed: Vec<String>,
    expected: String,
}

/// Every constraint class, each on a member of its own, refused with AWS's message.
#[tokio::test]
async fn a_constraint_violation_is_refused_with_the_member_and_the_requirement() {
    let emulator = Emulator::new();

    // The requirement texts below are the ones AWS states per constraint; the member paths
    // are the wire (lowerCamel) paths, with a list element's 1-based index and the
    // collection step before the element's own member.
    let country_pattern = r"[A-Za-z]{2}";
    let tag_key_pattern = r"([\p{L}\p{Z}\p{N}_.:/=+\-@]*)";
    let arn_pattern = r"arn:[\w+=/,.@-]+:acm-pca:[\w+=/,.@-]*:[0-9]*:[\w+=,.@-]+(/[\w+=,.@-]+)*";

    let rows = vec![
        Row {
            case: "a country above its length ceiling, which also fails its expression",
            operation: "CreateCertificateAuthority",
            body: ca_with_subject("country-long", &[("Country", json!("USA"))]),
            not_echoed: vec!["USA".to_string()],
            expected: envelope(&[
                clause(
                    "certificateAuthorityConfiguration.subject.country",
                    &format!("Member must satisfy regular expression pattern: {country_pattern}"),
                ),
                clause(
                    "certificateAuthorityConfiguration.subject.country",
                    "Member must have length less than or equal to 2",
                ),
            ]),
        },
        Row {
            case: "a country below its length floor, which also fails its expression",
            operation: "CreateCertificateAuthority",
            body: ca_with_subject("country-short", &[("Country", json!("U"))]),
            not_echoed: vec!["'U'".to_string()],
            expected: envelope(&[
                clause(
                    "certificateAuthorityConfiguration.subject.country",
                    &format!("Member must satisfy regular expression pattern: {country_pattern}"),
                ),
                clause(
                    "certificateAuthorityConfiguration.subject.country",
                    "Member must have length greater than or equal to 2",
                ),
            ]),
        },
        Row {
            case: "a common name one character past its ceiling",
            operation: "CreateCertificateAuthority",
            body: ca_with_subject("common-name", &[("CommonName", json!(letters(65)))]),
            not_echoed: vec![letters(65)],
            expected: envelope(&[clause(
                "certificateAuthorityConfiguration.subject.commonName",
                "Member must have length less than or equal to 64",
            )]),
        },
        Row {
            case: "a generation qualifier one character past its ceiling",
            operation: "CreateCertificateAuthority",
            body: ca_with_subject("generation", &[("GenerationQualifier", json!("IIII"))]),
            not_echoed: vec!["IIII".to_string()],
            expected: envelope(&[clause(
                "certificateAuthorityConfiguration.subject.generationQualifier",
                "Member must have length less than or equal to 3",
            )]),
        },
        Row {
            case: "a tag key past its ceiling, at the first element of the list",
            operation: "CreateCertificateAuthority",
            body: ca_with_tags(
                "tag-key-long",
                json!([{"Key": letters(129), "Value": "x"}]),
            ),
            not_echoed: vec![letters(129)],
            expected: envelope(&[clause(
                "tags.1.member.key",
                "Member must have length less than or equal to 128",
            )]),
        },
        Row {
            case: "a tag key outside its expression, at the second element of the list",
            operation: "CreateCertificateAuthority",
            body: ca_with_tags(
                "tag-key-pattern",
                json!([
                    {"Key": "owner", "Value": "platform"},
                    {"Key": "bad#tag!", "Value": "x"},
                ]),
            ),
            not_echoed: vec!["bad#tag!".to_string()],
            expected: envelope(&[clause(
                "tags.2.member.key",
                &format!("Member must satisfy regular expression pattern: {tag_key_pattern}"),
            )]),
        },
        Row {
            case: "a tag key outside its expression, carried by a tagging call",
            operation: "TagCertificateAuthority",
            body: json!({
                "CertificateAuthorityArn": unknown_ca_arn(),
                "Tags": [{"Key": "bad#tag!", "Value": "x"}],
            }),
            not_echoed: vec!["bad#tag!".to_string()],
            expected: envelope(&[clause(
                "tags.1.member.key",
                &format!("Member must satisfy regular expression pattern: {tag_key_pattern}"),
            )]),
        },
        Row {
            case: "an ARN outside its expression, on a listing",
            operation: "ListPermissions",
            body: json!({"CertificateAuthorityArn": "not-an-acm-pca-arn"}),
            not_echoed: vec!["not-an-acm-pca-arn".to_string()],
            expected: envelope(&[clause(
                "certificateAuthorityArn",
                &format!("Member must satisfy regular expression pattern: {arn_pattern}"),
            )]),
        },
        Row {
            case: "an ARN below its length floor, which also fails its expression",
            operation: "DescribeCertificateAuthority",
            body: json!({"CertificateAuthorityArn": "arn:"}),
            not_echoed: vec!["'arn:'".to_string()],
            expected: envelope(&[
                clause(
                    "certificateAuthorityArn",
                    &format!("Member must satisfy regular expression pattern: {arn_pattern}"),
                ),
                clause(
                    "certificateAuthorityArn",
                    "Member must have length greater than or equal to 5",
                ),
            ]),
        },
        Row {
            case: "a page size past its range ceiling",
            operation: "ListTags",
            body: json!({"CertificateAuthorityArn": unknown_ca_arn(), "MaxResults": 1001}),
            not_echoed: vec!["1001".to_string()],
            expected: envelope(&[clause(
                "maxResults",
                "Member must have value less than or equal to 1000",
            )]),
        },
        Row {
            case: "a CRL expiration past its range ceiling, nested two structures deep",
            operation: "CreateCertificateAuthority",
            body: {
                let mut body = root_ca_request("crl-expiration");
                body["RevocationConfiguration"] = json!({
                    "CrlConfiguration": {
                        "Enabled": true,
                        "S3BucketName": "crl-bucket",
                        "ExpirationInDays": 5001,
                    }
                });
                body
            },
            not_echoed: vec!["5001".to_string()],
            expected: envelope(&[clause(
                "revocationConfiguration.crlConfiguration.expirationInDays",
                "Member must have value less than or equal to 5000",
            )]),
        },
        Row {
            case: "a required member the request omits",
            operation: "ListTags",
            body: json!({"MaxResults": 10}),
            not_echoed: Vec::new(),
            expected: "1 validation error detected: Value null at 'certificateAuthorityArn' \
                       failed to satisfy constraint: Member must not be null"
                .to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let body = emulator
            .refused(case, row.operation, &row.body, VALIDATION_EXCEPTION)
            .await;
        let message = error_message(&body, case);
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected {:?}, got {message:?}",
            row.expected
        );
        for value in &row.not_echoed {
            assert!(
                !message.contains(value.as_str()),
                "case {case:?}: the submitted value {value:?} must not be echoed, \
                 got {message:?}"
            );
        }
        assert!(
            !message.contains('^') && !message.contains('$'),
            "case {case:?}: a quoted expression carries no anchors, got {message:?}"
        );
    }
}

/// A value outside an enumerated type's value set: the member and the set, under the count
/// of how many values failed.
///
/// Both rows name a type the request-validation stage judges, which is every type each
/// declared value of which this front door offers. A type it offers only a subset of is
/// refused one layer deeper, in the service's own clause, which
/// `crate::validation_stage_refusals` owns.
#[tokio::test]
async fn an_enum_violation_states_the_value_set_under_the_count() {
    let emulator = Emulator::new();

    // The enum members each row names, and the member path of the one that failed. Both
    // rows carry a value of the right TYPE and the wrong value, so what is refused is the
    // value set and nothing else.
    let rows: [(&str, &str, Value, &[&str]); 2] = [
        (
            "a CA kind that is neither of the two",
            "certificateAuthorityType",
            {
                let mut body = root_ca_request("enum-ca-type");
                body["CertificateAuthorityType"] = json!("INTERMEDIATE");
                body
            },
            &["ROOT", "SUBORDINATE"],
        ),
        (
            "a signing algorithm the service does not sign with",
            "certificateAuthorityConfiguration.signingAlgorithm",
            {
                let mut body = root_ca_request("enum-signing-algorithm");
                body["CertificateAuthorityConfiguration"]["SigningAlgorithm"] =
                    json!("MD5WITHRSA");
                body
            },
            &["SHA256WITHRSA"],
        ),
    ];

    for (case, path, body, members) in rows {
        let answer = emulator
            .refused(case, "CreateCertificateAuthority", &body, VALIDATION_EXCEPTION)
            .await;
        let message = error_message(&answer, case);
        let opening = format!(
            "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
             Member must satisfy enum value set: ["
        );
        assert!(
            message.starts_with(&opening) && message.ends_with(']'),
            "case {case:?}: expected {opening:?} with the value set, got {message:?}"
        );
        for member in members {
            assert!(
                message.contains(member),
                "case {case:?}: the value set must list {member}, got {message:?}"
            );
        }
    }
}

/// The other side of the rule: a value AT a published bound is not a constraint violation
/// at all, so the request is served.
#[tokio::test]
async fn a_value_at_a_published_bound_is_not_refused() {
    let emulator = Emulator::new();

    // Every bounded member this suite refuses past its bound, now AT the bound: the
    // country's exact length, the longest common name, the longest tag key, and the CRL
    // expiration's floor and ceiling.
    let mut body = root_ca_request("at-the-bounds");
    body["CertificateAuthorityConfiguration"]["Subject"]["Country"] = json!("US");
    body["CertificateAuthorityConfiguration"]["Subject"]["CommonName"] = json!(letters(64));
    body["CertificateAuthorityConfiguration"]["Subject"]["GenerationQualifier"] = json!("III");
    body["Tags"] = json!([{"Key": letters(128), "Value": "x"}]);

    let created = emulator
        .ok("every member at its bound", "CreateCertificateAuthority", &body)
        .await;
    let arn = string(&created, "CertificateAuthorityArn", "every member at its bound");
    assert!(
        arn.starts_with(&format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/")),
        "every member at its bound: expected a CA ARN, got {arn:?}"
    );

    // The page size at its ceiling, and a listing that is served rather than refused.
    emulator
        .ok(
            "a page size at its ceiling",
            "ListTags",
            &json!({"CertificateAuthorityArn": arn, "MaxResults": 1000}),
        )
        .await;

    // The CRL expiration at each end of its published range. No object storage is mounted
    // beside this service here, so the bucket the configuration names is not resolvable
    // either way and what the rows are about is the range alone.
    for days in [1, 5000] {
        let case = format!("a CRL expiration of {days} day(s)");
        let mut body = root_ca_request(&format!("crl-{days}"));
        body["RevocationConfiguration"] = json!({
            "CrlConfiguration": {
                "Enabled": true,
                "S3BucketName": "crl-bucket",
                "ExpirationInDays": days,
            }
        });
        emulator.ok(&case, "CreateCertificateAuthority", &body).await;
    }
}

/// The sibling rule for a request whose values are all well formed and name no CA: the
/// refusal is `ResourceNotFoundException` with the service's own FIXED sentence, which
/// carries no ARN of its own — the same sentence whichever operation resolved the CA.
#[tokio::test]
async fn an_unknown_but_well_formed_ca_arn_is_refused_with_the_services_fixed_sentence() {
    let emulator = Emulator::new();
    let arn = unknown_ca_arn();

    // Every operation that resolves a CA by its ARN and is implemented, with the members
    // each one needs beside it.
    let operations: [(&str, Value); 6] = [
        ("DescribeCertificateAuthority", json!({"CertificateAuthorityArn": arn})),
        ("GetCertificateAuthorityCsr", json!({"CertificateAuthorityArn": arn})),
        ("ListTags", json!({"CertificateAuthorityArn": arn})),
        ("ListPermissions", json!({"CertificateAuthorityArn": arn})),
        (
            "TagCertificateAuthority",
            json!({"CertificateAuthorityArn": arn, "Tags": [{"Key": "owner", "Value": "x"}]}),
        ),
        (
            "UpdateCertificateAuthority",
            json!({"CertificateAuthorityArn": arn, "Status": "DISABLED"}),
        ),
    ];

    for (operation, body) in &operations {
        let answer = emulator
            .refused(operation, operation, body, "ResourceNotFoundException")
            .await;
        let message = error_message(&answer, operation);
        assert_eq!(
            message, CA_NOT_FOUND_MESSAGE,
            "case {operation:?}: expected {CA_NOT_FOUND_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(arn.as_str()),
            "case {operation:?}: the sentence names no ARN of its own, got {message:?}"
        );
    }
}
