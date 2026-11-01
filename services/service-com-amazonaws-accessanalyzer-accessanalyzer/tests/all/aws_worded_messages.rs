// SPDX-License-Identifier: CC0-1.0

//! The TEXT this service puts on the wire for three message templates, driven end to end
//! through the real router.
//!
//! Each rule is asserted over a table that spans the class the template generalises over,
//! never over one request, and every expected string is written out here as a literal — so
//! a row cannot be satisfied by reading the implementation's own wording back:
//!
//! 1. `ValidatePolicy` reports a document that is not parseable JSON as one `ERROR`
//!    finding whose `findingDetails` is the service's FIXED syntax-error sentence, with
//!    the position of the parse failure as its only variable part, stated in the modelled
//!    `Position` coordinates (`offset` from 0, `line` from 1, `column` from 0 —
//!    `smithy_query(query="shape",shape="Position")`). No JSON parser's own diagnostic
//!    appears in it.
//! 2. A `jobId` that names no policy generation job is refused — on every operation that
//!    resolves a job — with ONE template naming the submitted id.
//! 3. A value outside a member's modelled `@pattern` is refused with the service's short
//!    `Invalid <member>: <value>` sentence, which never carries the pattern itself.

use serde_json::{json, Value};

use crate::fixed_refusal_texts::message;
use crate::harness::{wire_reason, Emulator, Reply, REGION};

/// The fixed halves of the syntax-error sentence, either side of the position triple.
const SYNTAX_ERROR_HEAD: &str = "Fix the JSON syntax error at index ";

/// The issue code a document that is not JSON is reported under, from the model's
/// `IssueCode` space as this service's checks name it.
const JSON_SYNTAX_ERROR: &str = "JSON_SYNTAX_ERROR";

/// The `ValidatePolicyFindingType` a non-functional policy is reported as, from the
/// model's own enum (`smithy_query(query="shape",shape="ValidatePolicyFindingType")`).
const ERROR: &str = "ERROR";

/// Vocabulary that belongs to a JSON PARSER rather than to this service: the diagnostics
/// `serde_json` and `jsonc-parser` phrase their failures with, plus the wording of the
/// framework's generic constraint report. None of it may reach a response.
const PARSER_VOCABULARY: &[&str] = &[
    "key must be a string",
    "expected value",
    "EOF while parsing",
    "trailing characters",
    "not valid JSON",
    "validation error detected",
    "regular expression pattern",
];

/// One malformed-document row: the document, and the text it stops being JSON AT.
///
/// `stops_at` is a marker the test resolves to a character index itself, so the expected
/// `index` is stated independently of whichever parser decided the document was invalid.
#[derive(Debug)]
struct MalformedCase {
    case: &'static str,
    document: &'static str,
    /// The first occurrence of this text is where the document stops being JSON, or
    /// `None` for a document whose stop position no reader of the row can state (a
    /// truncation, which stops at the end).
    stops_at: Option<&'static str>,
}

/// The `policyType` values, from the model's `PolicyType` enum
/// (`smithy_query(query="shape",shape="PolicyType")`).
const POLICY_TYPES: &[&str] = &[
    "IDENTITY_POLICY",
    "RESOURCE_POLICY",
    "SERVICE_CONTROL_POLICY",
    "RESOURCE_CONTROL_POLICY",
];

/// `ValidatePolicy` for a document and a policy type.
async fn validate(emulator: &Emulator, document: &str, policy_type: &str) -> Reply {
    emulator
        .send(
            "POST",
            "/policy/validation",
            Some(json!({ "policyDocument": document, "policyType": policy_type })),
        )
        .await
}

/// The line and column of a character offset, recomputed from the document: line from 1,
/// column from 0, as the model documents `Position`.
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

/// How many characters precede the first occurrence of `marker` in `document`.
fn character_index_of(document: &str, marker: &str) -> usize {
    let byte = document
        .find(marker)
        .unwrap_or_else(|| panic!("{marker:?} must occur in {document:?}"));
    document[..byte].chars().count()
}

/// A required integer member of a reported position.
fn integer(value: &Value, member: &str) -> usize {
    usize::try_from(
        value
            .get(member)
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("{member} is required, but is missing from {value}")),
    )
    .unwrap_or_else(|_| panic!("{member} of {value} must not be negative"))
}

/// RULE 1: a document that is not parseable JSON is answered 200 with one `ERROR` finding
/// whose details are the service's fixed syntax-error sentence, carrying the position of
/// the parse failure and nothing of the parser that found it.
///
/// The rows differ along the FORM of the syntax error (a quoting the grammar refuses, an
/// unquoted key, a comment, a trailing comma, a document that is not JSON at all, a
/// truncation, the empty document) and along WHERE the document stops being JSON — on the
/// first character, inside the first line, and on a later line of a multi-line document —
/// which is what the sentence's three variable parts report. The recorded case is a
/// first-line stop at index 1 (the `single-quoted property name` row).
#[tokio::test]
async fn an_unparseable_document_reports_the_fixed_syntax_error_sentence() {
    let cases = &[
        MalformedCase {
            case: "a single-quoted property name: the document stops at the quote",
            document: r#"{'Version':'2012-10-17'}"#,
            stops_at: Some("'Version'"),
        },
        MalformedCase {
            case: "an unquoted property name",
            document: r#"{Version:"2012-10-17"}"#,
            stops_at: Some("Version"),
        },
        MalformedCase {
            case: "not JSON at all, so it stops at its very first character",
            document: "allow everything",
            stops_at: Some("allow"),
        },
        MalformedCase {
            case: "a comment before the document",
            document: "// the policy\n{\"Version\":\"2012-10-17\"}",
            stops_at: Some("//"),
        },
        MalformedCase {
            case: "a single-quoted key on the THIRD line of a multi-line document",
            document: "{\n  \"Version\": \"2012-10-17\",\n  'Statement': []\n}",
            stops_at: Some("'Statement'"),
        },
        MalformedCase {
            case: "a trailing comma, which stops at the brace that follows it",
            document: r#"{"Version":"2012-10-17","Statement":[],}"#,
            stops_at: Some("}"),
        },
        MalformedCase {
            case: "the empty document",
            document: "",
            stops_at: Some(""),
        },
        MalformedCase {
            case: "a truncated object",
            document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow""#,
            stops_at: None,
        },
    ];

    let emulator = Emulator::new();
    for row in cases {
        for policy_type in POLICY_TYPES {
            let case = format!("{} ({policy_type})", row.case);
            let reply = validate(&emulator, row.document, policy_type).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a malformed document is reported as a finding, got {reply:?}"
            );
            let findings = reply
                .member("findings")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                findings.len(),
                1,
                "case {case:?}: expected exactly one finding, got {findings:?}"
            );
            let finding = &findings[0];
            assert_eq!(
                (
                    finding.get("issueCode").and_then(Value::as_str),
                    finding.get("findingType").and_then(Value::as_str)
                ),
                (Some(JSON_SYNTAX_ERROR), Some(ERROR)),
                "case {case:?}: expected an {ERROR} {JSON_SYNTAX_ERROR} finding, got {finding:?}"
            );

            let details = finding
                .get("findingDetails")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            // The position the finding's own location reports is the one the sentence
            // must state, so the two cannot disagree.
            let start = finding
                .get("locations")
                .and_then(Value::as_array)
                .and_then(|locations| locations.first())
                .and_then(|location| location.get("span"))
                .and_then(|span| span.get("start"))
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected a span start in {finding:?}"));
            let offset = integer(&start, "offset");
            let line = integer(&start, "line");
            let column = integer(&start, "column");
            let expected = format!("{SYNTAX_ERROR_HEAD}{offset} line {line} column {column}.");
            assert_eq!(
                details, expected,
                "case {case:?}: expected the fixed sentence {expected:?}, got {details:?}"
            );

            // The triple is a real position in the submitted document: its line and
            // column follow from its index, recomputed here without the handler.
            let characters = row.document.chars().count();
            assert!(
                offset <= characters,
                "case {case:?}: index {offset} is past the {characters}-character document"
            );
            assert_eq!(
                (line, column),
                position_at(row.document, offset),
                "case {case:?}: line {line} column {column} does not follow from index {offset}"
            );
            // And it is the position the ROW states the document stops being JSON at.
            if let Some(marker) = row.stops_at {
                let expected_index = character_index_of(row.document, marker);
                assert_eq!(
                    offset, expected_index,
                    "case {case:?}: expected the stop at index {expected_index} (at {marker:?}), \
                     got {offset}"
                );
            }

            // No parser's diagnostic, and none of the framework's constraint prose,
            // reaches the wire.
            for leaked in PARSER_VOCABULARY {
                assert!(
                    !details.contains(leaked),
                    "case {case:?}: the details must carry no parser wording, but {details:?} \
                     contains {leaked:?}"
                );
            }
        }
    }

    // The boundary of the rule: a document that IS parseable reports no syntax error.
    let good = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "s3:GetObject",
            "Resource": "arn:aws:s3:::reports/*",
        }],
    })
    .to_string();
    let reply = validate(&emulator, &good, "IDENTITY_POLICY").await;
    let codes: Vec<&str> = reply
        .member("findings")
        .and_then(Value::as_array)
        .map(|findings| {
            findings
                .iter()
                .filter_map(|finding| finding.get("issueCode").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !codes.contains(&JSON_SYNTAX_ERROR),
        "a parseable document must report no {JSON_SYNTAX_ERROR} finding, got {codes:?}"
    );
}

/// The two operations that resolve a `jobId` out of the path, so one table row is driven
/// on each: both must word an unknown job the same way.
#[derive(Debug, Clone, Copy)]
enum JobCall {
    Get,
    Cancel,
}

impl JobCall {
    async fn send(self, emulator: &Emulator, job_id: &str) -> Reply {
        match self {
            JobCall::Get => emulator.get_generated_policy(job_id, "").await,
            JobCall::Cancel => emulator.cancel_policy_generation(job_id).await,
        }
    }
}

/// An IAM role ARN in the form `PrincipalArn`'s pattern requires
/// (`^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$`).
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// Start a generation and answer the `jobId` it minted, so the table has a job that DOES
/// exist beside the ones that do not.
async fn start_generation(emulator: &Emulator, principal: &str) -> String {
    let reply = emulator
        .start_policy_generation(json!({
            "policyGenerationDetails": { "principalArn": principal },
            "cloudTrailDetails": {
                "trails": [{
                    "cloudTrailArn": format!(
                        "arn:aws:cloudtrail:{REGION}:{}:trail/audit",
                        engine::emulator_account_id()
                    ),
                }],
                "accessRole": role_arn("AccessAnalyzerMonitorServiceRole"),
                "startTime": 1_767_225_600.0,
            },
        }))
        .await;
    assert_eq!(
        reply.status, 200,
        "the fixture start must succeed, got {reply:?}"
    );
    reply
        .member("jobId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the start must answer a jobId, got {reply:?}"))
        .to_string()
}

/// RULE 2: a `jobId` that names no policy generation job is refused with ONE template —
/// `Invalid `, the member, `: `, the submitted id, ` does not exist` — on every operation
/// that resolves a job, whatever form the id takes.
///
/// The rows differ along the FORM of the unknown id (a UUID of the kind a start mints, a
/// UUID that differs from a live job's in one character, an id that is not a UUID at all,
/// a single character, a long id) — the id is the template's only variable part — and the
/// table is driven on both job-addressed operations.
#[tokio::test]
async fn an_unknown_job_id_is_refused_with_one_aws_worded_template() {
    let emulator = Emulator::new();
    // A job that DOES exist, so the table is not answering out of an empty store, and so
    // one row can differ from a live id by a single character.
    let live = start_generation(&emulator, &role_arn("message_template")).await;
    let near_miss: String = {
        let mut characters: Vec<char> = live.chars().collect();
        let last = characters.len() - 1;
        characters[last] = if characters[last] == 'a' { 'b' } else { 'a' };
        characters.into_iter().collect()
    };

    let unknown: Vec<(&str, String)> = vec![
        (
            "a UUID of the form a start mints",
            "acef2041-5fb4-4bdd-a201-aee420aa09d0".to_string(),
        ),
        (
            "a UUID differing from a live job's in its last character",
            near_miss.clone(),
        ),
        ("an id that is not a UUID at all", "no-such-job".to_string()),
        ("a single-character id", "j".to_string()),
        ("a long id", "job-".to_string() + &"9".repeat(80)),
    ];

    for (row, job_id) in &unknown {
        for call in [JobCall::Get, JobCall::Cancel] {
            let case = format!("{call:?} with {row}");
            let reply = call.send(&emulator, job_id).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: an unknown job is HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {reply:?}"
            );
            let expected = format!("Invalid jobId: {job_id} does not exist");
            assert_eq!(
                message(&reply),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                message(&reply)
            );
            assert_eq!(
                reply.member("reason").and_then(Value::as_str),
                Some(wire_reason("fieldValidationFailed")),
                "case {case:?}: the refusal must carry the required reason, got {reply:?}"
            );
            let named: Vec<&str> = reply
                .member("fieldList")
                .and_then(Value::as_array)
                .map(|fields| {
                    fields
                        .iter()
                        .filter_map(|field| field.get("name").and_then(Value::as_str))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                named,
                vec!["jobId"],
                "case {case:?}: expected fieldList to name jobId, got {reply:?}"
            );
        }
    }

    // The boundary of the rule: the job that exists is served, not refused — read back
    // through the operation the table drove over the unknown ids.
    let read = emulator.get_generated_policy(&live, "").await;
    assert_eq!(
        read.status, 200,
        "the job that exists must be served, got {read:?}"
    );
    assert_eq!(
        read.member("jobDetails")
            .and_then(|details| details.get("jobId"))
            .and_then(Value::as_str),
        Some(live.as_str()),
        "the read must report the job that was started, got {read:?}"
    );
    let canceled = emulator.cancel_policy_generation(&live).await;
    assert_eq!(
        canceled.status, 204,
        "the job that exists must be cancelable, got {canceled:?}"
    );
}

/// The member whose modelled `@pattern` rule 3 is asserted over, and the prefix of the
/// sentence that refuses it.
const PRINCIPAL_ARN: &str = "principalArn";

/// How many bytes of a rejected value a message echoes before it is reported by its
/// leading characters instead; past that the response would amplify the request.
const ECHO_LIMIT: usize = 256;

/// RULE 3: a value outside its member's modelled `@pattern` is refused with the service's
/// short sentence — `Invalid `, the member's wire name, `: `, the rejected value — and the
/// pattern itself never reaches the text.
///
/// The rows differ along HOW the value breaks the pattern
/// (`^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$`, from
/// `smithy_query(query="shape",shape="PrincipalArn")`): not an ARN at all (the recorded
/// case), another service's ARN, an IAM ARN carrying a region, an entity type the pattern
/// does not list, an empty resource segment (below the published 1-character minimum), one
/// character past the published 576-character maximum, and a value long enough that the
/// echo is capped.
#[tokio::test]
async fn a_value_outside_a_modelled_pattern_is_refused_with_the_services_own_sentence() {
    let emulator = Emulator::new();
    let rejected: Vec<(&str, String)> = vec![
        ("not an ARN at all", "not-an-arn".to_string()),
        (
            "another service's ARN",
            "arn:aws:s3:::my-bucket".to_string(),
        ),
        (
            "an IAM ARN carrying a region, which the pattern forbids",
            "arn:aws:iam:us-east-1:123456789012:role/app".to_string(),
        ),
        (
            "an entity type the pattern does not list",
            "arn:aws:iam::123456789012:group/admins".to_string(),
        ),
        (
            "a role ARN with no resource segment (below the 1-character minimum)",
            "arn:aws:iam::123456789012:role/".to_string(),
        ),
        (
            "a role ARN one character past the 576-character maximum",
            format!("arn:aws:iam::123456789012:role/{}", "p".repeat(577)),
        ),
        (
            "a value long enough for the echo to be capped",
            format!("not-an-arn-{}", "z".repeat(ECHO_LIMIT * 2)),
        ),
    ];

    for (case, value) in &rejected {
        let reply = emulator
            .list_policy_generations(&format!("?principalArn={}", crate::harness::label(value)))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a value outside the pattern is HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {reply:?}"
        );
        let text = message(&reply).to_string();
        let prefix = format!("Invalid {PRINCIPAL_ARN}: ");
        let shown = text.strip_prefix(prefix.as_str()).unwrap_or_else(|| {
            panic!("case {case:?}: expected a message starting {prefix:?}, got {text:?}")
        });
        if value.len() <= ECHO_LIMIT {
            assert_eq!(
                shown, value,
                "case {case:?}: the sentence must name the rejected value, got {text:?}"
            );
        } else {
            // A value past the echo cap is named by its leading characters.
            let head = shown.split("...").next().unwrap_or(shown);
            assert!(
                !head.is_empty() && value.starts_with(head),
                "case {case:?}: expected the leading characters of the rejected value, \
                 got {text:?}"
            );
        }
        // Neither the pattern nor the framework's constraint report reaches the text.
        for leaked in PARSER_VOCABULARY.iter().chain(["^arn:", "satisfy"].iter()) {
            assert!(
                !text.contains(leaked),
                "case {case:?}: the message must not carry {leaked:?}, got {text:?}"
            );
        }
        // `fieldList` still names the member a client has to correct.
        let named: Vec<&str> = reply
            .member("fieldList")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            named,
            vec![PRINCIPAL_ARN],
            "case {case:?}: expected fieldList to name {PRINCIPAL_ARN}, got {reply:?}"
        );
    }

    // The boundary of the rule: a value the pattern accepts — at each published bound of
    // its resource segment — is not refused at all.
    for (case, value) in [
        ("a role ARN at the 1-character minimum", role_arn("a")),
        (
            "a user ARN at the 576-character maximum",
            format!(
                "arn:aws:iam::{}:user/{}",
                engine::emulator_account_id(),
                "u".repeat(576)
            ),
        ),
    ] {
        let reply = emulator
            .list_policy_generations(&format!("?principalArn={}", crate::harness::label(&value)))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a value inside the pattern must be served, got {reply:?}"
        );
    }
}
