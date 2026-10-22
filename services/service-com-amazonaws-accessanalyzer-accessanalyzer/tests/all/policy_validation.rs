// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ValidatePolicy`, driven over the wire.
//!
//! Every case goes through the real router: an `http::Request` is dispatched to the
//! mounted service, so restJson1 routing, boundary constraint validation, the handler
//! and the response serialization all run exactly as they do in the emulator binary.
//!
//! The rule under test is that each finding is COMPUTED by a policy check over the
//! parsed policy document, and is located at the element the check is about. So the
//! tables below span the classes the checks generalise over — the policy type, the
//! element that is wrong, the JSON type it holds, the wildcard form, the statement
//! count, the page size — and the two recorded cases from the issue (a wildcard policy
//! that must be flagged with a security warning, and a malformed document that must be
//! reported as an ERROR finding rather than a request failure) are two rows among them.
//!
//! The span assertions are independent of the handler: the test recomputes line and
//! column from the reported character offset and compares, and slices the document
//! with the reported span to check the finding points at the element it names.
//!
//! The wire harness (`Emulator`, `Reply`) is the shared one in `tests/all/harness.rs`;
//! only the `ValidatePolicy`-shaped accessors are added here.

// The wire harness is shared by every suite in this target; see `tests/all/harness.rs`.
use crate::harness::{Emulator, Reply};
use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::ops::validate_policy::{
    MAX_FINDINGS, MAX_POLICY_DOCUMENT_BYTES, MAX_STATEMENTS,
};

/// The `policyType` values, from the model's `PolicyType` enum
/// (`smithy_query(query="shape", shape="PolicyType")`).
const IDENTITY: &str = "IDENTITY_POLICY";
const RESOURCE: &str = "RESOURCE_POLICY";
const SERVICE_CONTROL: &str = "SERVICE_CONTROL_POLICY";
const RESOURCE_CONTROL: &str = "RESOURCE_CONTROL_POLICY";

/// The `ValidatePolicyFindingType` values, from the model's own enum
/// (`smithy_query(query="shape", shape="ValidatePolicyFindingType")`).
const ERROR: &str = "ERROR";
const SECURITY_WARNING: &str = "SECURITY_WARNING";
const SUGGESTION: &str = "SUGGESTION";

/// What the assertions read off a `ValidatePolicy` response, added to the shared
/// harness's [`Reply`] rather than copied beside it.
trait Findings {
    /// The findings the response carries.
    fn findings(&self) -> Vec<&Value>;
    /// Every reported `issueCode`, in the order they were reported.
    fn issue_codes(&self) -> Vec<String>;
    /// The first finding reported under `issue_code`.
    fn finding(&self, issue_code: &str) -> Option<&Value>;
    /// The continuation token, when the report did not fit on one page.
    fn next_token(&self) -> Option<String>;
}

impl Findings for Reply {
    fn findings(&self) -> Vec<&Value> {
        self.body
            .get("findings")
            .and_then(Value::as_array)
            .map(|findings| findings.iter().collect())
            .unwrap_or_default()
    }

    fn issue_codes(&self) -> Vec<String> {
        self.findings()
            .iter()
            .map(|finding| string(finding, "issueCode"))
            .collect()
    }

    fn finding(&self, issue_code: &str) -> Option<&Value> {
        self.findings()
            .into_iter()
            .find(|finding| string(finding, "issueCode") == issue_code)
    }

    fn next_token(&self) -> Option<String> {
        self.body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// A required string member of a JSON object, as the model declares every member of
/// `ValidatePolicyFinding` required.
fn string(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{member} is a required member, but it is missing from {value}"))
        .to_string()
}

/// A required integer member.
fn integer(value: &Value, member: &str) -> i64 {
    value
        .get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("{member} is a required member, but it is missing from {value}"))
}

/// `ValidatePolicy` for a document and a policy type.
async fn validate(emulator: &Emulator, document: &str, policy_type: &str) -> Reply {
    validate_with(
        emulator,
        json!({ "policyDocument": document, "policyType": policy_type }),
        "",
    )
    .await
}

/// `ValidatePolicy` with the request body and query string spelled out.
async fn validate_with(emulator: &Emulator, body: Value, query: &str) -> Reply {
    emulator
        .send("POST", &format!("/policy/validation{query}"), Some(body))
        .await
}

/// One row of the check table: a document, the policy type it is validated as, the
/// checks that must hit, and the checks that must not.
#[derive(Debug)]
struct Row {
    case: &'static str,
    policy_type: &'static str,
    document: &'static str,
    /// `(issueCode, findingType)` for every check that must report.
    expected: &'static [(&'static str, &'static str)],
    /// Issue codes that must NOT be reported for this document.
    forbidden: &'static [&'static str],
}

/// The class of documents the checks run over: one row per kind of defect, plus clean
/// policies of each policy type that must report nothing.
const ROWS: &[Row] = &[
    // --- security warnings: what a granting statement reaches ------------------
    Row {
        // The recorded wildcard case: an identity policy that allows every action on
        // every resource reaches `iam:PassRole` and `iam:CreateServiceLinkedRole`.
        case: "identity policy allowing * on *",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
        expected: &[
            ("PASS_ROLE_WITH_STAR_IN_RESOURCE", SECURITY_WARNING),
            ("CREATE_SLR_WITH_STAR_IN_RESOURCE", SECURITY_WARNING),
        ],
        forbidden: &["JSON_SYNTAX_ERROR", "MISSING_VERSION", "MISSING_RESOURCE"],
    },
    Row {
        case: "identity policy allowing iam:PassRole on *",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:PassRole"],"Resource":["*"]}]}"#,
        expected: &[("PASS_ROLE_WITH_STAR_IN_RESOURCE", SECURITY_WARNING)],
        forbidden: &["CREATE_SLR_WITH_STAR_IN_RESOURCE"],
    },
    Row {
        case: "identity policy allowing iam:* on *",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:*","Resource":"*"}]}"#,
        expected: &[
            ("PASS_ROLE_WITH_STAR_IN_RESOURCE", SECURITY_WARNING),
            ("CREATE_SLR_WITH_STAR_IN_RESOURCE", SECURITY_WARNING),
        ],
        forbidden: &[],
    },
    Row {
        // A named role ARN is not the wildcard, so the same action is not flagged.
        case: "identity policy allowing iam:PassRole on one role",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:PassRole","Resource":"arn:aws:iam::123456789012:role/app"}]}"#,
        expected: &[],
        forbidden: &[
            "PASS_ROLE_WITH_STAR_IN_RESOURCE",
            "CREATE_SLR_WITH_STAR_IN_RESOURCE",
        ],
    },
    Row {
        // Only a grant can be overly permissive: the same wildcards under Deny are not.
        case: "identity policy denying * on *",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"*","Resource":"*"}]}"#,
        expected: &[],
        forbidden: &[
            "PASS_ROLE_WITH_STAR_IN_RESOURCE",
            "CREATE_SLR_WITH_STAR_IN_RESOURCE",
        ],
    },
    Row {
        case: "resource policy granting to a wildcard principal",
        policy_type: RESOURCE,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket/*"}]}"#,
        expected: &[("ALLOW_WITH_WILDCARD_PRINCIPAL", SECURITY_WARNING)],
        forbidden: &["MISSING_PRINCIPAL"],
    },
    Row {
        case: "resource policy granting to every account root",
        policy_type: RESOURCE,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":["arn:aws:iam::*:root"]},"Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket/*"}]}"#,
        expected: &[("ALLOW_WITH_WILDCARD_PRINCIPAL", SECURITY_WARNING)],
        forbidden: &[],
    },
    Row {
        // A condition narrows who the wildcard reaches, so the grant is not public.
        case: "resource policy with a wildcard principal and a condition",
        policy_type: RESOURCE,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket/*","Condition":{"StringEquals":{"aws:PrincipalOrgID":"o-abc123"}}}]}"#,
        expected: &[],
        forbidden: &["ALLOW_WITH_WILDCARD_PRINCIPAL"],
    },
    Row {
        case: "resource policy granting with NotPrincipal",
        policy_type: RESOURCE,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotPrincipal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket/*"}]}"#,
        expected: &[("ALLOW_WITH_NOT_PRINCIPAL", SECURITY_WARNING)],
        forbidden: &["MISSING_PRINCIPAL"],
    },
    // --- the elements a statement must and must not name -----------------------
    Row {
        case: "identity policy naming no resource",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject"}]}"#,
        expected: &[("MISSING_RESOURCE", ERROR)],
        forbidden: &["MISSING_PRINCIPAL"],
    },
    Row {
        case: "resource policy naming no principal",
        policy_type: RESOURCE,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket/*"}]}"#,
        expected: &[("MISSING_PRINCIPAL", ERROR)],
        forbidden: &["MISSING_RESOURCE"],
    },
    Row {
        case: "resource control policy naming no principal",
        policy_type: RESOURCE_CONTROL,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"s3:*","Resource":"*"}]}"#,
        expected: &[("MISSING_PRINCIPAL", ERROR)],
        forbidden: &["MISSING_RESOURCE"],
    },
    Row {
        case: "identity policy naming a principal",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"*"}]}"#,
        expected: &[("UNSUPPORTED_ELEMENT_COMBINATION", ERROR)],
        forbidden: &["ALLOW_WITH_WILDCARD_PRINCIPAL"],
    },
    Row {
        case: "statement naming both Action and NotAction",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","NotAction":"s3:PutObject","Resource":"*"}]}"#,
        expected: &[("UNSUPPORTED_ELEMENT_COMBINATION", ERROR)],
        forbidden: &["MISSING_ACTION"],
    },
    Row {
        case: "statement with no Effect",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Action":"s3:GetObject","Resource":"*"}]}"#,
        expected: &[("MISSING_EFFECT", ERROR)],
        forbidden: &["INVALID_EFFECT"],
    },
    Row {
        case: "statement with an Effect IAM does not define",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Permit","Action":"s3:GetObject","Resource":"*"}]}"#,
        expected: &[("INVALID_EFFECT", ERROR)],
        forbidden: &["MISSING_EFFECT"],
    },
    Row {
        case: "statement naming no action",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Resource":"*"}]}"#,
        expected: &[("MISSING_ACTION", ERROR)],
        forbidden: &["INVALID_ACTION"],
    },
    Row {
        case: "statement with an action that is not service:action",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"getobject","Resource":"*"}]}"#,
        expected: &[("INVALID_ACTION", ERROR)],
        forbidden: &["MISSING_ACTION"],
    },
    Row {
        case: "statement with a resource that is not an ARN",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"my-bucket"}]}"#,
        expected: &[("INVALID_ARN", ERROR)],
        forbidden: &["MISSING_RESOURCE"],
    },
    // --- the document's own grammar --------------------------------------------
    Row {
        case: "policy with no Version",
        policy_type: IDENTITY,
        document: r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        expected: &[("MISSING_VERSION", SUGGESTION)],
        forbidden: &["INVALID_VERSION"],
    },
    Row {
        case: "policy with a Version IAM does not define",
        policy_type: IDENTITY,
        document: r#"{"Version":"2020-01-01","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        expected: &[("INVALID_VERSION", ERROR)],
        forbidden: &["MISSING_VERSION"],
    },
    Row {
        case: "policy with the older Version IAM still defines",
        policy_type: IDENTITY,
        document: r#"{"Version":"2008-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        expected: &[],
        forbidden: &["INVALID_VERSION", "MISSING_VERSION"],
    },
    Row {
        case: "policy with no Statement",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17"}"#,
        expected: &[("MISSING_STATEMENT", ERROR)],
        forbidden: &["MISSING_EFFECT"],
    },
    Row {
        case: "policy whose Statement array is empty",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[]}"#,
        expected: &[("MISSING_STATEMENT", ERROR)],
        forbidden: &["MISSING_EFFECT"],
    },
    Row {
        case: "policy with one statement object rather than an array",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}}"#,
        expected: &[],
        forbidden: &["DATA_TYPE_MISMATCH", "MISSING_STATEMENT"],
    },
    Row {
        case: "policy with a top-level element the grammar does not define",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statements":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        expected: &[
            ("INVALID_POLICY_ELEMENT", ERROR),
            ("MISSING_STATEMENT", ERROR),
        ],
        forbidden: &[],
    },
    Row {
        case: "statement with an element the grammar does not define",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*","Conditions":{"StringEquals":{"aws:PrincipalOrgID":"o-abc123"}}}]}"#,
        expected: &[("INVALID_POLICY_ELEMENT", ERROR)],
        forbidden: &["EMPTY_OBJECT_CONDITION"],
    },
    Row {
        case: "two statements carrying the same Sid",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Sid":"Read","Effect":"Allow","Action":"s3:GetObject","Resource":"*"},{"Sid":"Read","Effect":"Allow","Action":"s3:ListBucket","Resource":"*"}]}"#,
        expected: &[("UNIQUE_SIDS_REQUIRED", ERROR)],
        forbidden: &["EMPTY_SID_VALUE"],
    },
    Row {
        case: "statement with an empty Sid",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Sid":"","Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        expected: &[("EMPTY_SID_VALUE", SUGGESTION)],
        forbidden: &["UNIQUE_SIDS_REQUIRED"],
    },
    Row {
        case: "statement with an empty Action array",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":[],"Resource":"*"}]}"#,
        expected: &[("EMPTY_ARRAY_ACTION", SUGGESTION)],
        forbidden: &["MISSING_ACTION"],
    },
    Row {
        case: "statement with an empty Resource array",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":[]}]}"#,
        expected: &[("EMPTY_ARRAY_RESOURCE", SUGGESTION)],
        forbidden: &["MISSING_RESOURCE"],
    },
    Row {
        case: "statement with an empty Principal object",
        policy_type: RESOURCE,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{},"Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket/*"}]}"#,
        expected: &[("EMPTY_OBJECT_PRINCIPAL", SUGGESTION)],
        forbidden: &["MISSING_PRINCIPAL"],
    },
    Row {
        case: "statement with an empty Condition object",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*","Condition":{}}]}"#,
        expected: &[("EMPTY_OBJECT_CONDITION", SUGGESTION)],
        forbidden: &["DATA_TYPE_MISMATCH"],
    },
    Row {
        case: "statement with Null paired with IfExists",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*","Condition":{"NullIfExists":{"aws:TokenIssueTime":"true"}}}]}"#,
        expected: &[("NULL_WITH_IF_EXISTS", ERROR)],
        forbidden: &["EMPTY_OBJECT_CONDITION"],
    },
    Row {
        // The set qualifier is a prefix on the operator name, so the same pairing is
        // reported however it is spelled.
        case: "statement with a set-qualified Null paired with IfExists",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*","Condition":{"ForAnyValue:NullIfExists":{"aws:TokenIssueTime":"true"}}}]}"#,
        expected: &[("NULL_WITH_IF_EXISTS", ERROR)],
        forbidden: &["EMPTY_OBJECT_CONDITION"],
    },
    // --- JSON types where the grammar wants something else ----------------------
    Row {
        case: "statement whose Action is a number",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":5,"Resource":"*"}]}"#,
        expected: &[("DATA_TYPE_MISMATCH", ERROR)],
        forbidden: &["INVALID_ACTION"],
    },
    Row {
        case: "statement whose Action array holds a boolean",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject",true],"Resource":"*"}]}"#,
        expected: &[("DATA_TYPE_MISMATCH", ERROR)],
        forbidden: &["MISSING_ACTION"],
    },
    Row {
        case: "policy whose Statement is a string",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":"allow everything"}"#,
        expected: &[("DATA_TYPE_MISMATCH", ERROR)],
        forbidden: &["MISSING_STATEMENT"],
    },
    Row {
        case: "policy document that is a JSON array",
        policy_type: IDENTITY,
        document: r#"[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]"#,
        expected: &[("DATA_TYPE_MISMATCH", ERROR)],
        forbidden: &["JSON_SYNTAX_ERROR"],
    },
    // --- clean policies, which must report nothing -------------------------------
    Row {
        case: "a clean identity policy",
        policy_type: IDENTITY,
        document: r#"{"Version":"2012-10-17","Statement":[{"Sid":"ReadOneBucket","Effect":"Allow","Action":["s3:GetObject","s3:ListBucket"],"Resource":["arn:aws:s3:::bucket","arn:aws:s3:::bucket/*"]}]}"#,
        expected: &[],
        forbidden: &[],
    },
    Row {
        case: "a clean resource policy",
        policy_type: RESOURCE,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket/*"}]}"#,
        expected: &[],
        forbidden: &[],
    },
    Row {
        case: "a clean service control policy",
        policy_type: SERVICE_CONTROL,
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"ec2:RunInstances","Resource":"*"}]}"#,
        expected: &[],
        forbidden: &[],
    },
];

/// Every row of the check table reports the checks that hit and none that do not, and
/// every finding carries the members the model declares required.
#[tokio::test]
async fn every_check_reports_what_it_found() {
    let emulator = Emulator::new();
    for row in ROWS {
        let reply = validate(&emulator, row.document, row.policy_type).await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected HTTP 200, got {} ({})",
            row.case,
            reply.status,
            reply.code()
        );
        let codes = reply.issue_codes();
        for (issue_code, finding_type) in row.expected {
            let finding = reply.finding(issue_code).unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a {issue_code} finding, got {codes:?}",
                    row.case
                )
            });
            assert_eq!(
                string(finding, "findingType"),
                *finding_type,
                "case {:?}: expected {issue_code} to be a {finding_type} finding, got {:?}",
                row.case,
                string(finding, "findingType")
            );
        }
        for issue_code in row.forbidden {
            assert!(
                reply.finding(issue_code).is_none(),
                "case {:?}: expected no {issue_code} finding, got {codes:?}",
                row.case
            );
        }
        if row.expected.is_empty() && row.forbidden.is_empty() {
            assert!(
                codes.is_empty(),
                "case {:?}: expected a clean policy to report nothing, got {codes:?}",
                row.case
            );
        }
        for finding in reply.findings() {
            let code = string(finding, "issueCode");
            assert!(
                !string(finding, "findingDetails").is_empty(),
                "case {:?}: {code} reported empty findingDetails",
                row.case
            );
            // The link is derived from the check: the reference page's anchor for it.
            let link = string(finding, "learnMoreLink");
            assert!(
                link.starts_with("https://docs.aws.amazon.com/")
                    && link.contains(&code.to_ascii_lowercase().replace('_', "-")),
                "case {:?}: {code} reported learnMoreLink {link:?}, which does not name the check",
                row.case
            );
            assert!(
                !finding
                    .get("locations")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| panic!(
                        "case {:?}: {code} reported no locations",
                        row.case
                    ))
                    .is_empty(),
                "case {:?}: {code} reported an empty location list",
                row.case
            );
        }
    }
}

/// Documents that are not JSON at all, in each form the strict-JSON grammar refuses.
/// The recorded malformed case is the first row.
const MALFORMED: &[(&str, &str)] = &[
    ("truncated object", r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow""#),
    ("trailing comma", r#"{"Version":"2012-10-17","Statement":[],}"#),
    ("unquoted property name", r#"{Version:"2012-10-17"}"#),
    ("a comment", "// the policy\n{\"Version\":\"2012-10-17\"}"),
    ("single quotes", r#"{'Version':'2012-10-17'}"#),
    ("not JSON at all", "allow everything"),
    ("the empty document", ""),
];

/// A document that is not JSON is reported AS A FINDING — HTTP 200 with one ERROR
/// finding — not as a request failure, because the operation's answer is the list of
/// findings and "this is not JSON" is one of them.
#[tokio::test]
async fn a_malformed_document_is_reported_as_an_error_finding() {
    let emulator = Emulator::new();
    for (case, document) in MALFORMED {
        let reply = validate(&emulator, document, IDENTITY).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.code()
        );
        let codes = reply.issue_codes();
        assert_eq!(
            codes,
            vec!["JSON_SYNTAX_ERROR".to_string()],
            "case {case:?}: expected exactly one JSON_SYNTAX_ERROR finding, got {codes:?}"
        );
        let finding = reply.finding("JSON_SYNTAX_ERROR").expect("the finding");
        assert_eq!(
            string(finding, "findingType"),
            ERROR,
            "case {case:?}: expected an ERROR finding, got {:?}",
            string(finding, "findingType")
        );
    }
}

/// The line and column of a character offset in `document`, computed independently of
/// the handler: line from 1, column from 0, as the model documents `Position`.
fn position_at(document: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut column = 0usize;
    for character in document.chars().take(offset) {
        if character == '\n' {
            line += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// The document characters a span covers.
fn slice(document: &str, span: &Value) -> String {
    let start = integer(span.get("start").expect("a start position"), "offset") as usize;
    let end = integer(span.get("end").expect("an end position"), "offset") as usize;
    document
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

/// Every span a finding reports is a real span of the document: inside it, ordered,
/// and with line and column that agree with the offset.
#[tokio::test]
async fn every_reported_span_is_a_real_span_of_the_document() {
    let emulator = Emulator::new();
    let documents = ROWS
        .iter()
        .map(|row| (row.case, row.policy_type, row.document))
        .chain(MALFORMED.iter().map(|(case, document)| (*case, IDENTITY, *document)));
    for (case, policy_type, document) in documents {
        let reply = validate(&emulator, document, policy_type).await;
        let characters = document.chars().count();
        for finding in reply.findings() {
            let code = string(finding, "issueCode");
            let locations = finding
                .get("locations")
                .and_then(Value::as_array)
                .expect("locations is a required member");
            for location in locations {
                let span = location.get("span").expect("span is a required member");
                for end in ["start", "end"] {
                    let position = span.get(end).expect("a position");
                    let offset = integer(position, "offset");
                    assert!(
                        offset >= 0 && offset as usize <= characters,
                        "case {case:?}: {code} reported a {end} offset of {offset} in a {characters}-character document"
                    );
                    let (line, column) = position_at(document, offset as usize);
                    assert_eq!(
                        (integer(position, "line"), integer(position, "column")),
                        (line as i64, column as i64),
                        "case {case:?}: {code} reported {end} line/column {:?} at offset {offset}, but that offset is at {:?}",
                        (integer(position, "line"), integer(position, "column")),
                        (line, column)
                    );
                }
                let start = integer(span.get("start").expect("a start"), "offset");
                let end = integer(span.get("end").expect("an end"), "offset");
                assert!(
                    start <= end,
                    "case {case:?}: {code} reported the span {start}..{end}, which ends before it starts"
                );
                // A path is a sequence of key/index elements, each naming one step.
                let path = location
                    .get("path")
                    .and_then(Value::as_array)
                    .expect("path is a required member");
                for step in path {
                    assert!(
                        step.get("key").is_some() || step.get("index").is_some(),
                        "case {case:?}: {code} reported the path step {step}, which names neither a key nor an index"
                    );
                }
            }
        }
    }
}

/// A finding points at the element it is about: its path names the element and its
/// span covers that element's own text, wherever in the document the element sits.
#[tokio::test]
async fn a_finding_points_at_the_element_it_is_about() {
    let emulator = Emulator::new();
    // The same defect, written at three different places in the document, so the
    // reported span has to be computed from the text rather than fixed.
    let documents = [
        (
            "one line",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:PassRole","Resource":"*"}]}"#,
        ),
        (
            "indented over several lines",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \"Effect\": \"Allow\",\n      \"Action\": \"iam:PassRole\",\n      \"Resource\": \"*\"\n    }\n  ]\n}",
        ),
        (
            "padded with blank lines",
            "\n\n{\"Version\":\"2012-10-17\",\n\n\"Statement\":[{\"Effect\":\"Allow\",\n\"Action\":\"iam:PassRole\",\n\"Resource\":\"*\"}]}\n",
        ),
    ];
    for (case, document) in documents {
        let reply = validate(&emulator, document, IDENTITY).await;
        let finding = reply
            .finding("PASS_ROLE_WITH_STAR_IN_RESOURCE")
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a PASS_ROLE_WITH_STAR_IN_RESOURCE finding, got {:?}",
                    reply.issue_codes()
                )
            });
        let locations = finding
            .get("locations")
            .and_then(Value::as_array)
            .expect("locations");
        let covered: Vec<String> = locations
            .iter()
            .map(|location| slice(document, location.get("span").expect("a span")))
            .collect();
        assert!(
            covered.iter().any(|text| text.trim_matches('"') == "iam:PassRole"),
            "case {case:?}: expected a location covering the action, got {covered:?}"
        );
        assert!(
            covered.iter().any(|text| text.trim_matches('"') == "*"),
            "case {case:?}: expected a location covering the wildcard resource, got {covered:?}"
        );
        // The path names the element, not just its text.
        let paths: Vec<Vec<Value>> = locations
            .iter()
            .map(|location| {
                location
                    .get("path")
                    .and_then(Value::as_array)
                    .expect("a path")
                    .clone()
            })
            .collect();
        assert!(
            paths.iter().any(|path| path.contains(&json!({"key": "Resource"}))),
            "case {case:?}: expected a path naming the Resource element, got {paths:?}"
        );
        assert!(
            paths
                .iter()
                .any(|path| path.first() == Some(&json!({"key": "Statement"}))),
            "case {case:?}: expected every path to start at Statement, got {paths:?}"
        );
    }
}

/// A finding reported for one statement of many names that statement's index, so a
/// caller can tell which statement is wrong.
#[tokio::test]
async fn a_finding_names_the_statement_it_came_from() {
    let emulator = Emulator::new();
    // Three statements, with the defect in the last one.
    let document = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"},{"Effect":"Allow","Action":"s3:ListBucket","Resource":"*"},{"Effect":"Allow","Action":"nonsense","Resource":"*"}]}"#;
    let reply = validate(&emulator, document, IDENTITY).await;
    let finding = reply
        .finding("INVALID_ACTION")
        .unwrap_or_else(|| panic!("expected an INVALID_ACTION finding, got {:?}", reply.issue_codes()));
    let path = finding
        .get("locations")
        .and_then(Value::as_array)
        .and_then(|locations| locations.first())
        .and_then(|location| location.get("path"))
        .and_then(Value::as_array)
        .expect("the finding's path")
        .clone();
    assert_eq!(
        path,
        vec![
            json!({"key": "Statement"}),
            json!({"index": 2}),
            json!({"key": "Action"}),
        ],
        "expected the path to name the third statement's Action, got {path:?}"
    );
}

/// A repeated `Sid` is reported once, with a location for every statement that carries
/// it, however many that is.
#[tokio::test]
async fn a_repeated_sid_is_reported_once_per_value_with_every_occurrence() {
    let emulator = Emulator::new();
    for repeats in [2usize, 3, 5] {
        let statements: Vec<String> = (0..repeats)
            .map(|_| {
                r#"{"Sid":"Read","Effect":"Allow","Action":"s3:GetObject","Resource":"*"}"#
                    .to_string()
            })
            .collect();
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
            statements.join(",")
        );
        let reply = validate(&emulator, &document, IDENTITY).await;
        let findings: Vec<&Value> = reply
            .findings()
            .into_iter()
            .filter(|finding| string(finding, "issueCode") == "UNIQUE_SIDS_REQUIRED")
            .collect();
        assert_eq!(
            findings.len(),
            1,
            "case {repeats} repeats: expected one UNIQUE_SIDS_REQUIRED finding, got {:?}",
            reply.issue_codes()
        );
        let locations = findings[0]
            .get("locations")
            .and_then(Value::as_array)
            .expect("locations");
        assert_eq!(
            locations.len(),
            repeats,
            "case {repeats} repeats: expected one location per statement carrying the Sid, got {}",
            locations.len()
        );
    }
}

/// `findings` is the paginated member: a page holds at most `maxResults` findings and
/// carries a token only while more remain, and following the tokens reports every
/// finding exactly once, in the same order as the unpaginated report.
#[tokio::test]
async fn following_the_pagination_tokens_reports_every_finding_once() {
    let emulator = Emulator::new();
    // A policy with several independent defects, so the report is long enough to page.
    let document = r#"{"Version":"2020-01-01","Statement":[{"Action":"nonsense","Resource":"my-bucket"},{"Effect":"Permit","Action":[],"Resource":"*","Sid":""}]}"#;
    let whole = validate(&emulator, document, IDENTITY).await;
    let expected = whole.issue_codes();
    assert!(
        expected.len() >= 3,
        "the paging fixture must report several findings, got {expected:?}"
    );
    assert!(
        whole.next_token().is_none(),
        "a report that fits on one page carries no nextToken, got {:?}",
        whole.next_token()
    );

    for page_size in [1usize, 2, 3] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("?maxResults={page_size}"),
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            };
            let reply = validate_with(
                &emulator,
                    json!({ "policyDocument": document, "policyType": IDENTITY }),
                    &query,
                )
                .await;
            assert_eq!(
                reply.status,
                200,
                "case page size {page_size}: expected HTTP 200, got {} ({})",
                reply.status,
                reply.code()
            );
            let codes = reply.issue_codes();
            assert!(
                codes.len() <= page_size,
                "case page size {page_size}: a page held {} findings",
                codes.len()
            );
            collected.extend(codes);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case page size {page_size}: paging did not terminate, collected {collected:?}"
            );
            match reply.next_token() {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case page size {page_size}: expected the pages to rebuild the whole report {expected:?}, got {collected:?}"
        );
    }
}

/// A `nextToken` this service did not issue is refused, rather than resumed from.
#[tokio::test]
async fn a_token_the_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    for token in ["not-a-token", "00", "6163636573"] {
        let reply = validate_with(
            &emulator,
                json!({
                    "policyDocument": r#"{"Version":"2012-10-17","Statement":[]}"#,
                    "policyType": IDENTITY,
                }),
                &format!("?nextToken={token}"),
            )
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {token:?}: expected HTTP 400, got {} ({})",
            reply.status,
            reply.code()
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {token:?}: expected a ValidationException, got {:?}",
            reply.code()
        );
    }
}

/// `validatePolicyResourceType` is documented as "Specify a value for the policy
/// validation resource type only if the policy type is RESOURCE_POLICY", so it is
/// accepted with a resource policy and refused with any other policy type.
#[tokio::test]
async fn the_validation_resource_type_belongs_to_a_resource_policy() {
    let emulator = Emulator::new();
    let document = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket/*"}]}"#;
    // Every value of the model's `ValidatePolicyResourceType` enum
    // (`smithy_query(query="shape", shape="ValidatePolicyResourceType")`).
    let resource_types = [
        "AWS::S3::Bucket",
        "AWS::S3::AccessPoint",
        "AWS::S3::MultiRegionAccessPoint",
        "AWS::S3ObjectLambda::AccessPoint",
        "AWS::IAM::AssumeRolePolicyDocument",
        "AWS::DynamoDB::Table",
    ];
    for resource_type in resource_types {
        let reply = validate_with(
            &emulator,
                json!({
                    "policyDocument": document,
                    "policyType": RESOURCE,
                    "validatePolicyResourceType": resource_type,
                }),
                "",
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {resource_type:?} with a resource policy: expected HTTP 200, got {} ({})",
            reply.status,
            reply.code()
        );

        for policy_type in [IDENTITY, SERVICE_CONTROL, RESOURCE_CONTROL] {
            let reply = validate_with(
                &emulator,
                    json!({
                        "policyDocument": document,
                        "policyType": policy_type,
                        "validatePolicyResourceType": resource_type,
                    }),
                    "",
                )
                .await;
            assert_eq!(
                reply.status,
                400,
                "case {resource_type:?} with {policy_type}: expected HTTP 400, got {} ({})",
                reply.status,
                reply.code()
            );
            assert_eq!(
                reply.code(),
                "ValidationException",
                "case {resource_type:?} with {policy_type}: expected a ValidationException, got {:?}",
                reply.code()
            );
        }
    }
}

/// A locale is accepted whatever it is, and does not change which checks hit: the
/// `issueCode` a client matches on is the same in every locale.
#[tokio::test]
async fn the_locale_does_not_change_which_checks_hit() {
    let emulator = Emulator::new();
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#;
    let baseline = validate(&emulator, document, IDENTITY).await.issue_codes();
    // Every value of the model's `Locale` enum (`LOCALE_VALUES`).
    for locale in [
        "DE", "EN", "ES", "FR", "IT", "JA", "KO", "PT_BR", "ZH_CN", "ZH_TW",
    ] {
        let reply = validate_with(
            &emulator,
                json!({
                    "policyDocument": document,
                    "policyType": IDENTITY,
                    "locale": locale,
                }),
                "",
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {locale:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.code()
        );
        assert_eq!(
            reply.issue_codes(),
            baseline,
            "case {locale:?}: expected the same checks to hit as with no locale ({baseline:?}), got {:?}",
            reply.issue_codes()
        );
    }
}

/// Validating a policy changes nothing: the operation is `@readonly`, so a later call
/// with the same document answers the same findings, and a validation neither creates
/// nor touches any resource the service holds.
#[tokio::test]
async fn validating_a_policy_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#;
    let first = validate(&emulator, document, IDENTITY).await;
    let analyzers_before = emulator.send("GET", "/analyzer", None).await;
    let second = validate(&emulator, document, IDENTITY).await;
    let analyzers_after = emulator.send("GET", "/analyzer", None).await;

    assert_eq!(
        first.body, second.body,
        "expected the same document to answer the same findings, got {:?} then {:?}",
        first.issue_codes(),
        second.issue_codes()
    );
    assert_eq!(
        analyzers_before.body, analyzers_after.body,
        "expected validation to leave the analyzer listing untouched, got {:?} then {:?}",
        analyzers_before.body, analyzers_after.body
    );
}

/// A document at the emulator's own ceilings is checked; one past them is REFUSED with
/// the modelled `ValidationException` naming `policyDocument`, rather than answered
/// with the findings of a document the checks did not finish reading.
#[tokio::test]
async fn a_document_past_the_emulators_ceilings_is_refused() {
    let emulator = Emulator::new();
    let statement = r#"{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}"#;
    let policy = |count: usize| {
        let statements: Vec<&str> = (0..count).map(|_| statement).collect();
        format!(
            r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
            statements.join(",")
        )
    };
    // One statement under the ceiling, exactly at it, and one past it.
    for count in [MAX_STATEMENTS - 1, MAX_STATEMENTS] {
        let reply = validate(&emulator, &policy(count), IDENTITY).await;
        assert_eq!(
            reply.status,
            200,
            "case {count} statements: expected HTTP 200, got {} ({})",
            reply.status,
            reply.code()
        );
    }
    let reply = validate(&emulator, &policy(MAX_STATEMENTS + 1), IDENTITY).await;
    assert_eq!(
        reply.status,
        400,
        "case {} statements: expected HTTP 400, got {} ({})",
        MAX_STATEMENTS + 1,
        reply.status,
        reply.code()
    );
    assert_eq!(
        reply.code(),
        "ValidationException",
        "case {} statements: expected a ValidationException, got {:?}",
        MAX_STATEMENTS + 1,
        reply.code()
    );

    // The same for the document size ceiling: the `Id` element is padded past it.
    let oversized = format!(
        r#"{{"Version":"2012-10-17","Id":"{}","Statement":[{statement}]}}"#,
        "a".repeat(MAX_POLICY_DOCUMENT_BYTES)
    );
    assert!(
        oversized.len() > MAX_POLICY_DOCUMENT_BYTES,
        "the oversized fixture must exceed the ceiling, got {} bytes",
        oversized.len()
    );
    let reply = validate(&emulator, &oversized, IDENTITY).await;
    assert_eq!(
        reply.status,
        400,
        "case {} bytes: expected HTTP 400, got {} ({})",
        oversized.len(),
        reply.status,
        reply.code()
    );
    let message = reply
        .body
        .get("fieldList")
        .and_then(Value::as_array)
        .and_then(|fields| fields.first())
        .map(|field| string(field, "name"))
        .unwrap_or_default();
    assert_eq!(
        message, "policyDocument",
        "case {} bytes: expected the refusal to name policyDocument, got {message:?}",
        oversized.len()
    );
}

/// A document that repeats an element is judged on the value a JSON reader keeps — the
/// LAST one — so the checks do not report on a value the policy does not take.
#[tokio::test]
async fn a_repeated_element_is_judged_on_the_value_that_wins() {
    let emulator = Emulator::new();
    let cases = [
        // (case, the two Effect values in document order, whether the statement grants)
        ("deny then allow", r#""Effect":"Deny","Effect":"Allow""#, true),
        ("allow then deny", r#""Effect":"Allow","Effect":"Deny""#, false),
    ];
    for (case, effects, grants) in cases {
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{{effects},"Action":"iam:PassRole","Resource":"*"}}]}}"#
        );
        let reply = validate(&emulator, &document, IDENTITY).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.code()
        );
        assert_eq!(
            reply.finding("PASS_ROLE_WITH_STAR_IN_RESOURCE").is_some(),
            grants,
            "case {case:?}: expected the security warning to be reported: {grants}, got {:?}",
            reply.issue_codes()
        );
    }
}

/// A `Condition` answers the wildcard-principal warning only when it narrows WHO the
/// grant reaches; a condition on anything else leaves the grant as wide as it was.
#[tokio::test]
async fn only_a_condition_on_the_caller_answers_the_wildcard_principal_warning() {
    let emulator = Emulator::new();
    // The keys are global condition context keys naming the caller or the call's
    // source; the last two name something else about the request.
    let cases = [
        (
            "the calling organization",
            r#"{"StringEquals":{"aws:PrincipalOrgID":"o-abc123"}}"#,
            false,
        ),
        (
            "the calling principal",
            r#"{"ArnLike":{"aws:PrincipalArn":"arn:aws:iam::123456789012:role/app"}}"#,
            false,
        ),
        (
            "the calling service's resource",
            r#"{"ArnLike":{"aws:SourceArn":"arn:aws:sns:us-east-1:123456789012:topic"}}"#,
            false,
        ),
        (
            "a tag on the calling principal",
            r#"{"StringEquals":{"aws:PrincipalTag/team":"cloud"}}"#,
            false,
        ),
        (
            "the transport, not the caller",
            r#"{"Bool":{"aws:SecureTransport":"true"}}"#,
            true,
        ),
        (
            "a request header, not the caller",
            r#"{"StringEquals":{"s3:x-amz-server-side-encryption":"AES256"}}"#,
            true,
        ),
        (
            // `Null` asks only whether the key is present, so it names nobody.
            "whether the caller has a principal ARN at all",
            r#"{"Null":{"aws:PrincipalArn":"true"}}"#,
            true,
        ),
        (
            // A negation excludes one principal rather than naming who is allowed.
            "every account except one",
            r#"{"StringNotEquals":{"aws:PrincipalAccount":"111122223333"}}"#,
            true,
        ),
        (
            // An `IfExists` form is skipped entirely for a caller without the key.
            "the calling organization if the caller has one",
            r#"{"StringEqualsIfExists":{"aws:PrincipalOrgID":"o-abc123"}}"#,
            true,
        ),
        (
            // The key names the caller, but the value matches every caller.
            "any principal ARN at all",
            r#"{"StringLike":{"aws:PrincipalArn":"*"}}"#,
            true,
        ),
        (
            // An empty value list narrows nothing either.
            "an empty list of accounts",
            r#"{"StringEquals":{"aws:PrincipalAccount":[]}}"#,
            true,
        ),
        (
            // The set qualifier is part of the operator name, so it still resolves.
            "any of several organizations",
            r#"{"ForAnyValue:StringEquals":{"aws:PrincipalOrgID":["o-abc123","o-def456"]}}"#,
            false,
        ),
        (
            // A prefix IAM does not define is not a qualifier, so the operator is not
            // one either.
            "an operator with an undefined qualifier",
            r#"{"Foo:StringEquals":{"aws:PrincipalOrgID":"o-abc123"}}"#,
            true,
        ),
    ];
    for (case, condition, warned) in cases {
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket/*","Condition":{condition}}}]}}"#
        );
        let reply = validate(&emulator, &document, RESOURCE).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.code()
        );
        assert_eq!(
            reply.finding("ALLOW_WITH_WILDCARD_PRINCIPAL").is_some(),
            warned,
            "case {case:?}: expected the public-access warning to be reported: {warned}, got {:?}",
            reply.issue_codes()
        );
    }
}

/// The resource check accepts the wildcard and a well-formed ARN, and reports anything
/// else — including an ARN whose required segments are missing.
#[tokio::test]
async fn the_resource_check_accepts_an_arn_and_reports_anything_else() {
    let emulator = Emulator::new();
    // (case, resource, whether INVALID_ARN must be reported)
    let cases = [
        ("the full wildcard", "*", false),
        ("a bucket in every region", "arn:aws:s3:::bucket/*", false),
        (
            "a role in one account",
            "arn:aws:iam::123456789012:role/app",
            false,
        ),
        (
            "an AWS-owned managed policy",
            "arn:aws:iam::aws:policy/ReadOnlyAccess",
            false,
        ),
        (
            "a regional resource",
            "arn:aws:dynamodb:us-east-1:123456789012:table/orders",
            false,
        ),
        ("a wildcard account", "arn:aws:iam::*:role/app", false),
        ("the China partition", "arn:aws-cn:s3:::bucket", false),
        ("a bare name", "my-bucket", true),
        ("an ARN with no segments", "arn:::::", true),
        ("an ARN with no service", "arn:aws::us-east-1:123456789012:thing", true),
        ("an ARN with no resource", "arn:aws:s3:us-east-1:123456789012:", true),
        ("an ARN with too few segments", "arn:aws:s3:::", true),
        ("not an ARN at all", "arn", true),
        ("an account id that is too short", "arn:aws:s3::12345:bucket", true),
        ("an account id that is not digits", "arn:aws:s3::my-account:bucket", true),
    ];
    for (case, resource, invalid) in cases {
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"s3:GetObject","Resource":"{resource}"}}]}}"#
        );
        let reply = validate(&emulator, &document, IDENTITY).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.code()
        );
        assert_eq!(
            reply.finding("INVALID_ARN").is_some(),
            invalid,
            "case {case:?} ({resource}): expected INVALID_ARN to be reported: {invalid}, got {:?}",
            reply.issue_codes()
        );
    }
}

/// A grant to every principal is recognised however it is written, in any partition.
#[tokio::test]
async fn a_grant_to_every_principal_is_recognised_in_every_form() {
    let emulator = Emulator::new();
    // (case, the `Principal` element, whether the public-access warning must fire)
    let cases = [
        ("the bare wildcard", r#""*""#, true),
        ("the wildcard under AWS", r#"{"AWS":"*"}"#, true),
        (
            "every account root",
            r#"{"AWS":"arn:aws:iam::*:root"}"#,
            true,
        ),
        (
            "every account root in the China partition",
            r#"{"AWS":"arn:aws-cn:iam::*:root"}"#,
            true,
        ),
        (
            "every account root in the GovCloud partition",
            r#"{"AWS":"arn:aws-us-gov:iam::*:root"}"#,
            true,
        ),
        (
            "every principal of every account",
            r#"{"AWS":"arn:aws:iam::*:*"}"#,
            true,
        ),
        (
            "one account root",
            r#"{"AWS":"arn:aws:iam::123456789012:root"}"#,
            false,
        ),
        (
            "one service principal",
            r#"{"Service":"cloudtrail.amazonaws.com"}"#,
            false,
        ),
    ];
    for (case, principal, public) in cases {
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{principal},"Action":"s3:GetObject","Resource":"arn:aws:s3:::bucket/*"}}]}}"#
        );
        let reply = validate(&emulator, &document, RESOURCE).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200, got {} ({})",
            reply.status,
            reply.code()
        );
        assert_eq!(
            reply.finding("ALLOW_WITH_WILDCARD_PRINCIPAL").is_some(),
            public,
            "case {case:?}: expected the public-access warning to be reported: {public}, got {:?}",
            reply.issue_codes()
        );
    }
}

/// A document with more findings than the emulator will report is refused, not
/// reported in part: a truncated page with no token to resume from would be
/// indistinguishable from a complete report.
#[tokio::test]
async fn a_report_longer_than_the_ceiling_is_refused_rather_than_truncated() {
    let emulator = Emulator::new();
    // Each malformed action is one INVALID_ACTION finding, so the finding count is the
    // length of the list — one statement, well inside the document and statement
    // ceilings.
    let policy = |findings: usize| {
        let actions: Vec<String> = (0..findings).map(|index| format!("\"bad{index}\"")).collect();
        format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":[{}],"Resource":"*"}}]}}"#,
            actions.join(",")
        )
    };

    // Just inside the ceiling: every finding is reported, across as many pages as the
    // shared pager hands out.
    let document = policy(MAX_FINDINGS - 1);
    let mut reported = 0usize;
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            None => String::new(),
            Some(token) => format!("?nextToken={token}"),
        };
        let reply = validate_with(
            &emulator,
            json!({ "policyDocument": document, "policyType": IDENTITY }),
            &query,
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "case {} findings: expected HTTP 200, got {} ({})",
            MAX_FINDINGS - 1,
            reply.status,
            reply.code()
        );
        reported += reply.findings().len();
        assert!(
            reported <= MAX_FINDINGS,
            "case {} findings: paging reported {reported} findings, more than the ceiling",
            MAX_FINDINGS - 1
        );
        match reply.next_token() {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        reported,
        MAX_FINDINGS - 1,
        "case {} findings: expected every finding to be reported across the pages, got {reported}",
        MAX_FINDINGS - 1
    );

    // Past it: refused, rather than a short report that looks complete.
    let reply = validate(&emulator, &policy(MAX_FINDINGS + 1), IDENTITY).await;
    assert_eq!(
        reply.status,
        400,
        "case {} findings: expected HTTP 400, got {} ({})",
        MAX_FINDINGS + 1,
        reply.status,
        reply.code()
    );
    assert_eq!(
        reply.code(),
        "ValidationException",
        "case {} findings: expected a ValidationException, got {:?}",
        MAX_FINDINGS + 1,
        reply.code()
    );
}
