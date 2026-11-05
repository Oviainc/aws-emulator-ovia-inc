// SPDX-License-Identifier: CC0-1.0

//! How ACM words a refused request: the member-constraint envelope the boundary
//! validator renders, and the paired-parameter rule `ListCertificates` enforces.
//!
//! Two rules are under test, and each is asserted over a TABLE rather than the one
//! request a trace happened to record:
//!
//! 1. ACM states every member-constraint failure as
//!    `<subject> failed to satisfy constraint: <modelled requirement>`, joined into
//!    the AWS envelope (`N validation error(s) detected: `). The subject names the
//!    input POSITION that failed and NOTHING the request submitted there
//!    (`Value of the input at '<member path>'`): the member path is the only part of a
//!    clause that comes from the request, so no caller-supplied text is reflected back
//!    — at any position, of any constraint class, at any depth. A member the request
//!    never sent has no value to withhold and reads `Value null at '<path>'` instead.
//!    A `@pattern` requirement
//!    reports the expression without the model's `^` / `$` anchoring, an enum's value
//!    set is listed in the order this service states that enum's values in, and a
//!    failure of
//!    a collection's ELEMENT is reported against the collection member with the WHOLE
//!    set of constraints the element shape declares nested — so an element that fails
//!    two of them is still one clause and one error.
//!    The table spans the constraint kinds (`@required`, `@length` on a string and
//!    on a list, `@range`, `@pattern`, `@enum`), the member depth (a top-level
//!    member, a collection element, and a member of a collection element), both
//!    subjects, several operations, and the one- and many-clause envelopes.
//! 2. `ListCertificates` refuses a request that names exactly one of
//!    `SortBy` / `SortOrder` with the sentence for that paired-parameter rule.
//! 3. `RequestCertificate` refuses a `DomainValidationOptions` entry naming a domain
//!    the request does not cover, naming that domain and the two places a covered
//!    domain may come from.
//!
//! The `ResourceNotFoundException` sentence a certificate lookup answers with has its
//! own suite in [`crate::certificate_not_found_message`] and is not restated here.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the path the emulator
//! serves real traffic on.

use serde_json::{json, Value};

use crate::certificate_lifecycle::{call, describe, error_code, router, ACCOUNT, REGION};

/// How ACM names the subject of a constraint clause: the input position, never the
/// submitted value.
const WITHHOLDING_SUBJECT: &str = "Value of the input at";

/// How the subject reads for a member the request never sent, which has no submitted
/// value to withhold (`services/common/src/validation.rs` states the cross-service
/// rule).
const ABSENT_SUBJECT: &str = "Value null at";

/// Which of the two subjects a row's clauses open with.
#[derive(Clone, Copy)]
enum Subject {
    /// The position alone, which is what EVERY position a request submitted a value at
    /// states.
    Withheld,
    /// The absent-member form, for a member the request never sent.
    Absent,
}

impl Subject {
    /// The subject as it is rendered in front of a clause about `path`.
    fn rendered(self, path: &str) -> String {
        match self {
            Subject::Withheld => format!("{WITHHOLDING_SUBJECT} '{path}'"),
            Subject::Absent => format!("{ABSENT_SUBJECT} '{path}'"),
        }
    }
}

/// The `@pattern` of `com.amazonaws.acm#Arn`, as the model spells it
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#Arn")`), anchors included.
const ARN_PATTERN: &str = r"^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$";

/// The `@pattern` of `com.amazonaws.acm#PcaArn`
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#PcaArn")`), anchors included.
const PCA_ARN_PATTERN: &str =
    r"^arn:[\w+=/,.@-]+:acm-pca:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$";

/// The `@pattern` of `com.amazonaws.acm#IdempotencyToken`
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#IdempotencyToken")`).
const IDEMPOTENCY_TOKEN_PATTERN: &str = r"^\w+$";

/// The `@pattern` of `com.amazonaws.acm#TagKey`
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#TagKey")`).
const TAG_KEY_PATTERN: &str = r"^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$";

/// The `@length` maximum of `com.amazonaws.acm#TagKey` (`length 1..128`).
const TAG_KEY_MAX_LENGTH: usize = 128;

/// The sentence `ListCertificates` answers the paired `SortBy` / `SortOrder` rule
/// with ("If you specify SortBy, you must also specify SortOrder" /
/// "If you specify SortOrder, you must also specify SortBy" —
/// `smithy_query(query="shape", shape="com.amazonaws.acm#ListCertificatesRequest")`).
const SORT_PAIR_REQUIRED: &str = "Request contains either SortBy or SortOrder. Both SortBy and \
                                  SortOrder parameters need to specified in the request.";

/// Every `SortOrder` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#SortOrder")`).
const SORT_ORDERS: &[&str] = &["ASCENDING", "DESCENDING"];

/// Every `SortBy` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#SortBy")`).
const SORT_BYS: &[&str] = &["CREATED_AT"];

/// The order ACM STATES `CertificateStatus`'s value set in, whichever seam built the
/// clause: the order recorded for ACM's own refusal of a status outside the set, which
/// is neither the order the model declares the enum in nor a sort of its values
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#CertificateStatus")` declares
/// `PENDING_VALIDATION, ISSUED, INACTIVE, EXPIRED, VALIDATION_TIMED_OUT, REVOKED,
/// FAILED`). [`crate::stated_enum_value_sets`] is that rule's own suite.
const STATED_CERTIFICATE_STATUSES: &[&str] = &[
    "VALIDATION_TIMED_OUT",
    "PENDING_VALIDATION",
    "EXPIRED",
    "INACTIVE",
    "ISSUED",
    "FAILED",
    "REVOKED",
];

/// One constraint-rendering row: what it exercises, the operation to call, the
/// request body, the subject its clauses open with, the `(member path, requirement)`
/// clauses the answer must state in order, and the caller-supplied strings that must
/// NOT appear in it.
struct Case<'a> {
    what: &'a str,
    operation: &'a str,
    body: Value,
    subject: Subject,
    clauses: Vec<(&'a str, String)>,
    withheld: Vec<&'a str>,
}

/// A syntactically well-formed ACM certificate ARN, for a row whose subject is some
/// OTHER member: it satisfies `Arn` so it contributes no clause of its own.
fn well_formed_arn() -> String {
    format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/12345678-1234-1234-1234-123456789012")
}

/// The requirement a `@pattern` violation states: the expression WITHOUT the
/// model's anchoring.
fn pattern_requirement(modelled: &str) -> String {
    format!(
        "Member must satisfy regular expression pattern: {}",
        unanchored(modelled)
    )
}

/// The requirement a collection ELEMENT's failure states: the WHOLE set of
/// constraints the element shape declares, nested, because the clause is reported
/// against the collection member rather than the index inside it — one clause stating
/// everything an element has to satisfy, not only the constraint this value tripped
/// over.
fn nested(element_constraints: &[String]) -> String {
    format!(
        "Member must satisfy constraint: [{}]",
        element_constraints.join(", ")
    )
}

/// The constraint set of `com.amazonaws.acm#TagKey`, the element shape of every
/// `TagKeyList` member, in the order ACM lists one: the length maximum, the length
/// minimum, the pattern, and that an element must not be null
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#TagKeyList")` reports its
/// member as `length 1..128`, `pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`).
fn tag_key_constraints() -> Vec<String> {
    vec![
        "Member must have length less than or equal to 128".to_string(),
        "Member must have length greater than or equal to 1".to_string(),
        pattern_requirement(TAG_KEY_PATTERN),
        ELEMENT_NOT_NULL.to_string(),
    ]
}

/// The constraint set of `com.amazonaws.acm#CertificateStatus`, the element shape of
/// `ListCertificatesRequest$CertificateStatuses`: its value set, and that an element
/// must not be null.
fn certificate_status_constraints() -> Vec<String> {
    vec![
        format!(
            "Member must satisfy enum value set: [{}]",
            STATED_CERTIFICATE_STATUSES.join(", ")
        ),
        ELEMENT_NOT_NULL.to_string(),
    ]
}

/// The clause that closes every element constraint set: an element of a list is never
/// null, which every element shape requires and no trait records.
const ELEMENT_NOT_NULL: &str = "Member must not be null";

/// `modelled` without a leading `^` and a trailing `$`.
fn unanchored(modelled: &str) -> &str {
    let body = modelled.strip_prefix('^').unwrap_or(modelled);
    body.strip_suffix('$').unwrap_or(body)
}

/// The full message the clauses must compose into: the AWS envelope over one clause
/// per violation, in order.
fn envelope(subject: Subject, clauses: &[(&str, String)]) -> String {
    let rendered: Vec<String> = clauses
        .iter()
        .map(|(path, requirement)| {
            format!(
                "{} failed to satisfy constraint: {requirement}",
                subject.rendered(path)
            )
        })
        .collect();
    match rendered.len() {
        1 => format!("1 validation error detected: {}", rendered[0]),
        n => format!(
            "{n} validation errors detected: {}",
            rendered.join("; ")
        ),
    }
}

/// The rows the constraint-rendering rule is asserted over.
fn cases() -> Vec<Case<'static>> {
    let arn = well_formed_arn();
    let many_keys: Vec<String> = (0..51).map(|n| format!("Key{n}")).collect();
    vec![
        Case {
            what: "a malformed resourceArn on a read operation",
            operation: "ListTagsForResource",
            body: json!({ "ResourceArn": "not-a-valid-arn-at-all" }),
            subject: Subject::Withheld,
            clauses: vec![("resourceArn", pattern_requirement(ARN_PATTERN))],
            withheld: vec!["not-a-valid-arn-at-all"],
        },
        Case {
            what: "the same malformed resourceArn on a write operation",
            operation: "UntagResource",
            body: json!({ "ResourceArn": "not-a-valid-arn-at-all", "TagKeys": ["Env"] }),
            subject: Subject::Withheld,
            clauses: vec![("resourceArn", pattern_requirement(ARN_PATTERN))],
            withheld: vec!["not-a-valid-arn-at-all"],
        },
        Case {
            what: "a resourceArn that is both too short and malformed",
            operation: "UntagResource",
            body: json!({ "ResourceArn": "arn:aws:acm:x", "TagKeys": ["Env"] }),
            subject: Subject::Withheld,
            clauses: vec![
                (
                    "resourceArn",
                    "Member must have length greater than or equal to 20".to_string(),
                ),
                ("resourceArn", pattern_requirement(ARN_PATTERN)),
            ],
            withheld: vec!["arn:aws:acm:x"],
        },
        Case {
            what: "a malformed certificateAuthorityArn (a different ARN pattern)",
            operation: "RequestCertificate",
            body: json!({
                "DomainName": "ca.example.com",
                "ValidationMethod": "DNS",
                "CertificateAuthorityArn": "not-a-valid-certificate-authority-arn-at-all",
            }),
            subject: Subject::Withheld,
            clauses: vec![(
                "certificateAuthorityArn",
                pattern_requirement(PCA_ARN_PATTERN),
            )],
            withheld: vec!["not-a-valid-certificate-authority-arn-at-all"],
        },
        Case {
            what: "an idempotencyToken over the maximum length AND outside its pattern",
            operation: "PutAccountConfiguration",
            body: json!({ "IdempotencyToken": "edcf2492-b054-4720-b528-4e3096c39531" }),
            subject: Subject::Withheld,
            clauses: vec![
                (
                    "idempotencyToken",
                    "Member must have length less than or equal to 32".to_string(),
                ),
                (
                    "idempotencyToken",
                    pattern_requirement(IDEMPOTENCY_TOKEN_PATTERN),
                ),
            ],
            withheld: vec!["edcf2492-b054-4720-b528-4e3096c39531"],
        },
        Case {
            what: "an idempotencyToken under the minimum length AND outside its pattern",
            operation: "PutAccountConfiguration",
            body: json!({ "IdempotencyToken": "" }),
            subject: Subject::Withheld,
            clauses: vec![
                (
                    "idempotencyToken",
                    "Member must have length greater than or equal to 1".to_string(),
                ),
                (
                    "idempotencyToken",
                    pattern_requirement(IDEMPOTENCY_TOKEN_PATTERN),
                ),
            ],
            withheld: vec![],
        },
        Case {
            what: "an idempotencyToken of a legal length but outside its pattern",
            operation: "PutAccountConfiguration",
            body: json!({ "IdempotencyToken": "token-with-dashes" }),
            subject: Subject::Withheld,
            clauses: vec![(
                "idempotencyToken",
                pattern_requirement(IDEMPOTENCY_TOKEN_PATTERN),
            )],
            withheld: vec!["token-with-dashes"],
        },
        Case {
            what: "an integer member below its @range minimum",
            operation: "ListCertificates",
            body: json!({ "MaxItems": 0 }),
            subject: Subject::Withheld,
            clauses: vec![(
                "maxItems",
                "Member must have value greater than or equal to 1".to_string(),
            )],
            withheld: vec!["0"],
        },
        Case {
            what: "an integer member above its @range maximum",
            operation: "ListCertificates",
            body: json!({ "MaxItems": 1001 }),
            subject: Subject::Withheld,
            clauses: vec![(
                "maxItems",
                "Member must have value less than or equal to 1000".to_string(),
            )],
            withheld: vec!["1001"],
        },
        // An ABSENT required member: the one subject that is not the withholding form,
        // since there is no submitted value to withhold.
        Case {
            what: "an absent @required member",
            operation: "ListTagsForResource",
            body: json!({}),
            subject: Subject::Absent,
            clauses: vec![("resourceArn", "Member must not be null".to_string())],
            withheld: vec![],
        },
        Case {
            what: "a list member below its @length minimum (entry count)",
            operation: "UntagResource",
            body: json!({ "ResourceArn": arn, "TagKeys": [] }),
            subject: Subject::Withheld,
            clauses: vec![(
                "tagKeys",
                "Member must have length greater than or equal to 1".to_string(),
            )],
            withheld: vec![],
        },
        Case {
            what: "a list member above its @length maximum (entry count)",
            operation: "UntagResource",
            body: json!({ "ResourceArn": arn, "TagKeys": many_keys }),
            subject: Subject::Withheld,
            clauses: vec![(
                "tagKeys",
                "Member must have length less than or equal to 50".to_string(),
            )],
            withheld: vec![],
        },
        // A member of a list element, reported at its nested path — and on the TAG
        // surface, whose text ACM withholds wherever it is reached, so the clause
        // names the nested position alone. [`crate::tag_surface_refusal_subject`]
        // is that rule's own suite.
        Case {
            what: "a member of a list element, reported at its nested path",
            operation: "AddTagsToCertificate",
            body: json!({
                "CertificateArn": arn,
                "Tags": [{ "Key": "{not-a-legal-key}", "Value": "ok" }],
            }),
            subject: Subject::Withheld,
            clauses: vec![("tags.1.member.key", pattern_requirement(TAG_KEY_PATTERN))],
            withheld: vec!["{not-a-legal-key}"],
        },
        Case {
            what: "a list ELEMENT outside its enum value set",
            operation: "ListCertificates",
            body: json!({ "CertificateStatuses": ["NOT_A_STATUS"] }),
            subject: Subject::Withheld,
            clauses: vec![("certificateStatuses", nested(&certificate_status_constraints()))],
            withheld: vec!["NOT_A_STATUS"],
        },
        Case {
            what: "a list ELEMENT below the element's @length minimum",
            operation: "UntagResource",
            body: json!({ "ResourceArn": arn, "TagKeys": [""] }),
            subject: Subject::Withheld,
            clauses: vec![("tagKeys", nested(&tag_key_constraints()))],
            withheld: vec![],
        },
        Case {
            what: "a list ELEMENT outside the element's @pattern",
            operation: "UntagResource",
            body: json!({ "ResourceArn": arn, "TagKeys": ["{not-a-legal-key}"] }),
            subject: Subject::Withheld,
            clauses: vec![("tagKeys", nested(&tag_key_constraints()))],
            withheld: vec!["{not-a-legal-key}"],
        },
        Case {
            what: "a list ELEMENT failing TWO of the element's constraints at once",
            operation: "UntagResource",
            body: json!({
                "ResourceArn": arn,
                "TagKeys": [format!("{}#", "k".repeat(TAG_KEY_MAX_LENGTH))],
            }),
            // One clause, and one error: the set the clause states already names every
            // constraint the element failed.
            subject: Subject::Withheld,
            clauses: vec![("tagKeys", nested(&tag_key_constraints()))],
            withheld: vec![],
        },
        Case {
            what: "two list ELEMENTS of one member, each failing a different constraint",
            operation: "UntagResource",
            body: json!({ "ResourceArn": arn, "TagKeys": ["", "{not-a-legal-key}"] }),
            subject: Subject::Withheld,
            clauses: vec![("tagKeys", nested(&tag_key_constraints()))],
            withheld: vec!["{not-a-legal-key}"],
        },
        // The same rule at the OTHER seam: a value sitting directly on a UNION
        // member, which the framework's validating walk does not descend into, so
        // the handler refuses it itself — same subject, same stated value set, and the
        // same wire member path.
        Case {
            what: "an enum on a union member, refused by the handler rather than the walk",
            operation: "SearchCertificates",
            body: json!({
                "FilterStatement": {
                    "Filter": { "AcmCertificateMetadataFilter": { "Status": "NOT_A_STATUS" } },
                },
            }),
            subject: Subject::Withheld,
            clauses: vec![(
                "filterStatement.filter.acmCertificateMetadataFilter.status",
                format!(
                    "Member must satisfy enum value set: [{}]",
                    STATED_CERTIFICATE_STATUSES.join(", ")
                ),
            )],
            withheld: vec!["NOT_A_STATUS"],
        },
    ]
}

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("no message in {body}"))
        .to_string()
}

/// Four whole envelopes pinned as LITERALS, so a shared mistake in the way the
/// table below assembles its expectation — the count wording, the `; ` separator,
/// the clause order, the nesting of a collection element's requirement, the
/// absent-member subject — cannot hide behind the same formula on both sides.
#[tokio::test]
async fn a_whole_envelope_is_pinned_as_a_literal() {
    let router = router();

    // (operation, body, the whole expected message)
    let rows: [(&str, Value, &str); 4] = [
        (
            "ListTagsForResource",
            json!({ "ResourceArn": "not-a-valid-arn-at-all" }),
            "1 validation error detected: Value of the input at 'resourceArn' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: \
             arn:[\\w+=/,.@-]+:acm:[\\w+=/,.@-]*:[0-9]+:[\\w+=,.@-]+(/[\\w+=,.@-]+)*",
        ),
        (
            "PutAccountConfiguration",
            json!({ "IdempotencyToken": "edcf2492-b054-4720-b528-4e3096c39531" }),
            "2 validation errors detected: Value of the input at 'idempotencyToken' failed to \
             satisfy constraint: Member must have length less than or equal to 32; Value of the \
             input at 'idempotencyToken' failed to satisfy constraint: Member must satisfy \
             regular expression pattern: \\w+",
        ),
        // An ABSENT member has no submitted value to withhold, so the clause opens
        // with the absent-member subject the tree states cross-service
        // (`services/common/src/validation.rs`: "an ABSENT member keeps
        // `Value null at '<path>' …`").
        (
            "ListTagsForResource",
            json!({}),
            "1 validation error detected: Value null at 'resourceArn' failed to satisfy \
             constraint: Member must not be null",
        ),
        // A collection ELEMENT's failure, reported against the collection member with
        // the element shape's whole constraint set nested — and with the value
        // withheld, because the position the clause names is the collection and the
        // submitted scalar is one element inside it.
        (
            "ListCertificates",
            json!({ "CertificateStatuses": ["NOT_A_STATUS"] }),
            "1 validation error detected: Value of the input at 'certificateStatuses' failed to \
             satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value \
             set: [VALIDATION_TIMED_OUT, PENDING_VALIDATION, EXPIRED, INACTIVE, ISSUED, FAILED, \
             REVOKED], Member must not be null]",
        ),
    ];

    for (operation, body, expected) in rows {
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {operation}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            message(&answer),
            expected,
            "case {operation}: expected the whole envelope verbatim, got {answer}"
        );
    }
}

/// RULE: ACM states every member-constraint failure against the input POSITION that
/// failed and the requirement the model declares there — one clause per violation in the
/// AWS envelope, with nothing the caller submitted echoed into it, an enum's value set
/// in the order the service states it, and a `@pattern` reported without the model's
/// anchors.
#[tokio::test]
async fn every_constraint_failure_is_stated_against_the_input_position() {
    let router = router();

    for case in cases() {
        let Case {
            what,
            operation,
            body,
            subject,
            clauses,
            withheld,
        } = case;
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {what:?} ({operation}): expected 400, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {what:?} ({operation}): expected ValidationException, got {answer}"
        );
        let stated = message(&answer);
        assert_eq!(
            stated,
            envelope(subject, &clauses),
            "case {what:?} ({operation}): expected the ACM constraint envelope, got {stated:?}"
        );
        for value in withheld {
            assert!(
                !stated.contains(value),
                "case {what:?} ({operation}): the submitted value {value:?} must not be \
                 echoed, got {stated:?}"
            );
        }
    }
}

/// RULE: the `@pattern` requirement names the expression a value must match, without
/// the `^` / `$` the model anchors it with.
///
/// The rows are every ACM `@pattern` the table above reaches, each cited from the
/// model; the assertion is about the anchoring, independent of the clause wording.
#[tokio::test]
async fn a_reported_pattern_carries_no_anchors() {
    let router = router();
    let arn = well_formed_arn();

    // (what, operation, body, the model's anchored pattern)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "Arn",
            "ListTagsForResource",
            json!({ "ResourceArn": "not-a-valid-arn-at-all" }),
            ARN_PATTERN,
        ),
        (
            "PcaArn",
            "RequestCertificate",
            json!({
                "DomainName": "ca.example.com",
                "ValidationMethod": "DNS",
                "CertificateAuthorityArn": "not-a-valid-certificate-authority-arn-at-all",
            }),
            PCA_ARN_PATTERN,
        ),
        (
            "IdempotencyToken",
            "PutAccountConfiguration",
            json!({ "IdempotencyToken": "token-with-dashes" }),
            IDEMPOTENCY_TOKEN_PATTERN,
        ),
        (
            "TagKey",
            "AddTagsToCertificate",
            json!({
                "CertificateArn": arn,
                "Tags": [{ "Key": "{not-a-legal-key}", "Value": "ok" }],
            }),
            TAG_KEY_PATTERN,
        ),
    ];

    const MARKER: &str = "Member must satisfy regular expression pattern: ";

    for (what, operation, body, modelled) in rows {
        assert!(
            modelled.starts_with('^') && modelled.ends_with('$'),
            "case {what}: the model's pattern is anchored, got {modelled:?}"
        );
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {what}: expected 400, got {status} {answer}"
        );
        let stated = message(&answer);
        let reported = stated
            .rsplit_once(MARKER)
            .unwrap_or_else(|| panic!("case {what}: no pattern clause in {stated:?}"))
            .1;
        assert_eq!(
            reported,
            unanchored(modelled),
            "case {what}: expected the unanchored expression, got {reported:?}"
        );
        assert!(
            !reported.starts_with('^') && !reported.ends_with('$'),
            "case {what}: the reported pattern must carry no anchors, got {reported:?}"
        );
    }
}

/// RULE: `ListCertificates` needs `SortBy` and `SortOrder` together — a request
/// naming exactly one of them is refused with the sentence for that rule, and a
/// request naming both or neither is served.
#[tokio::test]
async fn list_certificates_requires_the_sort_parameters_as_a_pair() {
    let router = router();

    // Exactly one half of the pair: refused, whichever half it is and whatever
    // value it carries.
    let mut lone: Vec<(String, Value)> = Vec::new();
    for sort_by in SORT_BYS {
        lone.push((
            format!("SortBy={sort_by} alone"),
            json!({ "SortBy": sort_by }),
        ));
    }
    for order in SORT_ORDERS {
        lone.push((
            format!("SortOrder={order} alone"),
            json!({ "SortOrder": order }),
        ));
    }
    for (what, body) in lone {
        let (status, answer) = call(&router, "ListCertificates", &body).await;
        assert_eq!(
            status, 400,
            "case {what}: expected 400, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidArgsException",
            "case {what}: expected InvalidArgsException, got {answer}"
        );
        assert_eq!(
            message(&answer),
            SORT_PAIR_REQUIRED,
            "case {what}: expected the paired-parameter sentence, got {answer}"
        );
    }

    // Both halves, or neither: served.
    let mut paired: Vec<(String, Value)> = vec![("neither named".to_string(), json!({}))];
    for sort_by in SORT_BYS {
        for order in SORT_ORDERS {
            paired.push((
                format!("SortBy={sort_by} with SortOrder={order}"),
                json!({ "SortBy": sort_by, "SortOrder": order }),
            ));
        }
    }
    for (what, body) in paired {
        let (status, answer) = call(&router, "ListCertificates", &body).await;
        assert_eq!(
            status, 200,
            "case {what}: expected 200, got {status} {answer}"
        );
        assert!(
            answer["CertificateSummaryList"].is_array(),
            "case {what}: expected a certificate list, got {answer}"
        );
    }
}

/// RULE: a `DomainValidationOptions` entry may only name a domain the certificate
/// request covers — its `DomainName` (the primary domain) or one of its
/// `SubjectAlternativeNames`; one that does not is refused with
/// `InvalidDomainValidationOptionsException` naming that domain and the two places a
/// domain may come from.
///
/// The rows span the ways a domain can be covered (the primary domain, each
/// alternative name) and the ways it can miss (a sibling, a subdomain of a covered
/// name, the apex of a wildcard) — a covered row round-trips the entry through
/// `DescribeCertificate`, so the accepted path is proven to reach the record.
#[tokio::test]
async fn a_validation_option_must_name_a_domain_the_request_covers() {
    let router = router();

    // (what, DomainName, SubjectAlternativeNames, the option's DomainName, covered)
    let rows: Vec<(&str, &str, &[&str], &str, bool)> = vec![
        (
            "the primary domain",
            "primary.example.com",
            &[],
            "primary.example.com",
            true,
        ),
        (
            "the first alternative name",
            "primary.example.com",
            &["alt.example.com", "other.example.com"],
            "alt.example.com",
            true,
        ),
        (
            "the last alternative name",
            "primary.example.com",
            &["alt.example.com", "other.example.com"],
            "other.example.com",
            true,
        ),
        (
            "a domain the request never names",
            "primary.example.com",
            &[],
            "stranger.example.com",
            false,
        ),
        (
            "a subdomain of a covered name",
            "primary.example.com",
            &["alt.example.com"],
            "deep.alt.example.com",
            false,
        ),
        (
            "the apex of a wildcard the request covers",
            "*.wild.example.com",
            &[],
            "wild.example.com",
            false,
        ),
    ];

    for (what, domain, sans, option_domain, covered) in rows {
        let mut body = json!({
            "DomainName": domain,
            "ValidationMethod": "EMAIL",
            "DomainValidationOptions": [{
                "DomainName": option_domain,
                "ValidationDomain": "example.com",
            }],
        });
        if !sans.is_empty() {
            body["SubjectAlternativeNames"] = json!(sans);
        }
        let (status, answer) = call(&router, "RequestCertificate", &body).await;
        if covered {
            assert_eq!(
                status, 200,
                "case {what:?}: a covered validation option must be accepted, got \
                 {status} {answer}"
            );
            let arn = answer["CertificateArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {what:?}: no CertificateArn in {answer}"))
                .to_string();
            // The accepted entry reached the record: the certificate reads back.
            let detail = describe(&router, &arn).await;
            assert_eq!(
                detail["DomainName"].as_str(),
                Some(domain),
                "case {what:?}: expected the stored certificate's DomainName, got {detail}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {what:?}: an uncovered validation option must be refused, got \
             {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidDomainValidationOptionsException",
            "case {what:?}: expected InvalidDomainValidationOptionsException, got {answer}"
        );
        assert_eq!(
            message(&answer),
            format!(
                "The domain specified in ValidationOptions, {option_domain}, is not the \
                 PrimaryDomain or part of SubjectAlternativeNames."
            ),
            "case {what:?}: expected the uncovered-domain sentence, got {answer}"
        );
    }
}
