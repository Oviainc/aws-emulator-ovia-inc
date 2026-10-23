// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the TEXT of two refusals: a body this service could not
//! read as JSON, and a `RevisionId` precondition that is not the revision the stored
//! resource policy is at.
//!
//! Two rules are pinned here.
//!
//! 1. A refusal for a JSON body the service could not read says WHERE the body stops
//!    being JSON in the caller's own coordinates: the offending character, what was
//!    being read when the parser reached it, and that character's BYTE OFFSET into the
//!    submitted bytes — the count of bytes read to reach it, so its 1-based byte
//!    position — never a line/column pair out of whichever parser happened to read it.
//!    The offset is the only part of the refusal that points a client at the defective
//!    byte, and a byte position stays meaningful for a document of any shape, where a
//!    line/column pair taken from another parser points somewhere else.
//! 2. A refusal whose sentence the service states as a CONSTANT carries that constant:
//!    nothing request-derived and nothing minted by the service goes into it.
//!
//! The rows below differ along what the rules generalise over: WHERE the body stops
//! being JSON (which JSON construct was being read — a literal, a member name, a
//! separator, a string escape, a number's fraction or exponent, the end of the
//! document), WHICH character offends (printable, control, non-ASCII), how far INTO
//! the body it is (first byte, a byte on a later line, a byte a thousand bytes in) and
//! how large the body is. The recorded instance is one row.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The
//! HTTP harness is the crate's one harness, imported from `registry_schema_lifecycle`.

use serde_json::json;
use service_com_amazonaws_schemas_schemas::ops::types::TYPE_VALUES;
use service_com_amazonaws_schemas_schemas::state::CODE_MALFORMED_REQUEST;

use crate::registry_schema_lifecycle::{create_registry, schema_uri, Answer, Emulator};
use crate::resource_policy::policy_uri;

/// The sentence this service refuses a stale `RevisionId` with: a constant, naming
/// neither the revision the caller sent nor the one the store holds.
const STALE_REVISION_MESSAGE: &str =
    "The RevisionId provided does not match the latest RevisionId for the registry policy.";

/// One malformed body, and the diagnostic the service must report for it.
///
/// `detail` is the part after `Content is not valid JSON: ` — the offending character,
/// what was being read, and the byte offset of that character (the bytes read to reach
/// it: byte one of the body is `offset 1`). Each is written out rather than computed
/// from the implementation, so a row states what the rule produces.
#[derive(Debug)]
struct FaultCase {
    case: &'static str,
    body: String,
    detail: String,
}

fn fault_case(case: &'static str, body: impl Into<String>, detail: impl Into<String>) -> FaultCase {
    FaultCase {
        case,
        body: body.into(),
        detail: detail.into(),
    }
}

/// The table. Offsets are counted by hand off the body in each row.
fn fault_cases() -> Vec<FaultCase> {
    // A body whose defect is a thousand bytes in: the offset is a byte count, so it
    // keeps counting past anything the document holds before the defect.
    let filler = "x".repeat(1_000);
    let deep_body = format!("{{\"d\":\"{filler}\",\"a\":tru1}}");
    // `{"d":"` is six bytes, then the filler, then `","a":tru` — the `1` that breaks
    // the literal is the next byte, so the bytes read to reach it are all of those.
    let deep_offset = 6 + filler.len() + "\",\"a\":tru".len() + 1;

    vec![
        // A literal that goes wrong, which is the recorded instance's shape: a body
        // starting with a letter a JSON parser reads as the start of `null`.
        fault_case(
            "prose read as the null literal",
            "not a schema",
            "invalid character 'o' in literal null (expecting 'u') at offset 2",
        ),
        fault_case(
            "a broken null literal inside an array",
            "[nope]",
            "invalid character 'o' in literal null (expecting 'u') at offset 3",
        ),
        fault_case(
            "a null literal cut short by the closing brace",
            r#"{"a": nul}"#,
            "invalid character '}' in literal null (expecting 'l') at offset 10",
        ),
        fault_case(
            "a broken true literal",
            r#"{"t": tru1}"#,
            "invalid character '1' in literal true (expecting 'e') at offset 10",
        ),
        fault_case(
            "a broken false literal",
            r#"{"f": falze}"#,
            "invalid character 'z' in literal false (expecting 's') at offset 10",
        ),
        // The point of the rule: in a multi-line document the byte offset is nothing
        // like the column, so a line/column pair out of another parser points
        // elsewhere.
        fault_case(
            "a defect on the second line of the document",
            "{\n  \"openapi\": tru1\n}",
            "invalid character '1' in literal true (expecting 'e') at offset 19",
        ),
        fault_case(
            "a defect on the third line, ended by the newline itself",
            "{\n  \"a\": 1,\n  \"b\": nul\n}",
            "invalid character '\\n' in literal null (expecting 'l') at offset 23",
        ),
        fault_case(
            "a defect a thousand bytes into the body",
            deep_body,
            format!("invalid character '1' in literal true (expecting 'e') at offset {deep_offset}"),
        ),
        // Member names and separators.
        fault_case(
            "an unquoted member name",
            "{openapi: 3}",
            "invalid character 'o' looking for beginning of object key string at offset 2",
        ),
        fault_case(
            "a trailing comma in an object",
            r#"{"a":1,}"#,
            "invalid character '}' looking for beginning of object key string at offset 8",
        ),
        fault_case(
            "a member name with no colon after it",
            r#"{"a" 1}"#,
            "invalid character '1' after object key at offset 6",
        ),
        fault_case(
            "two members with no comma between them",
            r#"{"a":1 "b":2}"#,
            "invalid character '\"' after object key:value pair at offset 8",
        ),
        fault_case(
            "two array elements with no comma between them",
            "[1,2 3]",
            "invalid character '3' after array element at offset 6",
        ),
        // Past the end of the document's one value.
        fault_case(
            "a stray brace after the document",
            "{} }",
            "invalid character '}' after top-level value at offset 4",
        ),
        fault_case(
            "two documents in a row",
            "{}{}",
            "invalid character '{' after top-level value at offset 3",
        ),
        // Numbers.
        fault_case(
            "a number with a leading zero",
            r#"{"a":01}"#,
            "invalid character '1' after object key:value pair at offset 7",
        ),
        fault_case(
            "a fraction with no digits",
            r#"{"a":1.}"#,
            "invalid character '}' after decimal point in numeric literal at offset 8",
        ),
        fault_case(
            "an exponent with no digits",
            r#"{"a":1e+}"#,
            "invalid character '}' in exponent of numeric literal at offset 9",
        ),
        fault_case(
            "a minus sign followed by no number",
            r#"{"a":-x}"#,
            "invalid character 'x' in numeric literal at offset 7",
        ),
        // Strings.
        fault_case(
            "an escape JSON does not define",
            "{\"a\":\"b\\qc\"}",
            "invalid character 'q' in string escape code at offset 9",
        ),
        fault_case(
            "a \\u escape with a non-hexadecimal digit",
            "{\"a\":\"\\u00zz\"}",
            "invalid character 'z' in \\u hexadecimal character escape at offset 11",
        ),
        fault_case(
            "an unescaped newline inside a string",
            "{\"a\":\"b\nc\"}",
            "invalid character '\\n' in string literal at offset 8",
        ),
        fault_case(
            "an unescaped tab inside a string",
            "{\"a\":\"\tb\"}",
            "invalid character '\\t' in string literal at offset 7",
        ),
        // A character that is not ASCII at all: reported by the offending byte.
        fault_case(
            "a non-ASCII character where a value must start",
            "[é]",
            "invalid character '\\xc3' looking for beginning of value at offset 2",
        ),
        fault_case(
            "a single character that starts no JSON value",
            "x",
            "invalid character 'x' looking for beginning of value at offset 1",
        ),
        // Bodies that simply run out: there is no offending character to point at.
        fault_case(
            "an object cut off after a colon",
            r#"{"openapi":"#,
            "unexpected end of JSON input",
        ),
        fault_case(
            "an unterminated string",
            r#"{"openapi":"3.0.0"#,
            "unexpected end of JSON input",
        ),
        fault_case(
            "whitespace only",
            "   \n\t ",
            "unexpected end of JSON input",
        ),
        fault_case(
            "five thousand unclosed brackets",
            "[".repeat(5_000),
            "unexpected end of JSON input",
        ),
    ]
}

/// The byte offset a diagnostic reports, where it reports one.
fn offset_in(detail: &str) -> Option<usize> {
    detail
        .rsplit_once(" at offset ")
        .and_then(|(_, offset)| offset.parse().ok())
}

/// The character a diagnostic names, where that character is printable ASCII (the
/// quoted forms — `'\n'`, `'\xc3'` — stand for a byte that is not).
fn plain_character_in(detail: &str) -> Option<u8> {
    let quoted = detail.strip_prefix("invalid character ")?;
    let mut bytes = quoted.bytes();
    match (bytes.next(), bytes.next(), bytes.next()) {
        (Some(b'\''), Some(character), Some(b'\'')) if character != b'\\' => Some(character),
        _ => None,
    }
}

/// Every refusal of an unreadable body is checked the same way: the status, the wire
/// shape, the condition code, the exact sentence, and then the INVARIANT the sentence
/// has to keep — the offset it reports is an index into the body the caller sent, and
/// it indexes the very character the sentence names.
fn assert_unreadable_body(case: &str, answer: &Answer, body: &str, expected: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a body that is not JSON must be refused 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "BadRequestException",
        "case {case:?}: expected the wire shape BadRequestException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Code"),
        CODE_MALFORMED_REQUEST,
        "case {case:?}: expected Code {CODE_MALFORMED_REQUEST}, got {:?}",
        answer.body
    );
    let message = answer.str_at("Message");
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains("line ") && !message.contains("column "),
        "case {case:?}: a refusal must report a byte offset, not a parser's line and \
         column, got {message:?}"
    );
    if let Some(offset) = offset_in(&message) {
        assert!(
            offset >= 1 && offset <= body.len(),
            "case {case:?}: the offset must name a byte of the submitted body of {} \
             bytes, got {offset} in {message:?}",
            body.len()
        );
        if let Some(character) = plain_character_in(
            message
                .rsplit_once("JSON: ")
                .map_or(message.as_str(), |(_, detail)| detail),
        ) {
            assert_eq!(
                body.as_bytes()[offset - 1],
                character,
                "case {case:?}: the offset must point at the character the message \
                 names ({:?}), but the byte it names is {:?}",
                character as char,
                body.as_bytes()[offset - 1] as char
            );
        }
    }
}

/// RULE 1, on `CreateSchema`'s `Content`: a body the service cannot read as JSON is
/// refused 400 `BadRequestException` / `MalformedRequest` with a message that names
/// the offending character, what was being read, and that character's byte offset into
/// the submitted `Content` — for every value of the modelled `Type`, since whether a
/// body is JSON does not depend on which schema language it declares.
#[tokio::test]
async fn create_schema_reports_where_content_stops_being_json() {
    let emulator = Emulator::new();
    let registry = "json-diagnostics";
    create_registry(&emulator, registry).await;

    // Source: `com.amazonaws.schemas#Type`, re-exported as `TYPE_VALUES`.
    for schema_type in TYPE_VALUES {
        for (index, row) in fault_cases().into_iter().enumerate() {
            let case = format!("{schema_type}/{}", row.case);
            let expected = format!("Content is not valid JSON: {}", row.detail);
            let answer = emulator
                .call(
                    "POST",
                    &schema_uri(registry, &format!("schema-{schema_type}-{index}")),
                    Some(json!({"Content": row.body, "Type": schema_type})),
                )
                .await;
            assert_unreadable_body(&case, &answer, &row.body, &expected);
        }
    }
}

/// RULE 1, on `UpdateSchema`'s `Content`: the same body is refused with the same
/// sentence on the update path — the diagnostic is a property of the body, not of the
/// operation that carried it — and the stored schema is left at the version and the
/// content it already had.
#[tokio::test]
async fn update_schema_reports_where_content_stops_being_json() {
    let emulator = Emulator::new();
    let registry = "json-diagnostics-updates";
    create_registry(&emulator, registry).await;

    let schema = "updated-schema";
    let stored = r#"{"openapi":"3.0.0"}"#;
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": stored, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the seed create must succeed, got {} {:?}",
        created.status, created.body
    );
    let version = created.str_at("SchemaVersion");

    for row in fault_cases() {
        let expected = format!("Content is not valid JSON: {}", row.detail);
        let answer = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({"Content": row.body, "Type": "OpenApi3"})),
            )
            .await;
        assert_unreadable_body(row.case, &answer, &row.body, &expected);

        // A refused update wrote nothing: the schema is still at the version and the
        // content the create stored.
        let described = emulator
            .call("GET", &schema_uri(registry, schema), None)
            .await;
        assert_eq!(
            described.str_at("Content"),
            stored,
            "case {:?}: a refused update must not change the stored content",
            row.case
        );
        assert_eq!(
            described.str_at("SchemaVersion"),
            version,
            "case {:?}: a refused update must not mint a version",
            row.case
        );
    }
}

/// RULE 1, on `PutResourcePolicy`'s `Policy`: the same diagnostic, for the other
/// member of this service that carries a JSON document. The sentence this refusal is
/// framed in is the policy member's own; what the rule fixes is the diagnostic inside
/// it, and that nothing is written.
#[tokio::test]
async fn put_resource_policy_reports_where_the_policy_stops_being_json() {
    let emulator = Emulator::new();
    let registry = "json-diagnostics-policies";
    create_registry(&emulator, registry).await;

    for row in fault_cases() {
        let answer = emulator
            .call(
                "PUT",
                &policy_uri(Some(registry)),
                Some(json!({ "Policy": row.body })),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: a policy that is not JSON must be refused 400, got {} {:?}",
            row.case, answer.status, answer.body
        );
        let message = answer.str_at("Message");
        assert!(
            message.contains(&row.detail),
            "case {:?}: expected the refusal to carry {:?}, got {message:?}",
            row.case,
            row.detail
        );
        assert!(
            !message.contains("line ") && !message.contains("column "),
            "case {:?}: a refusal must report a byte offset, not a parser's line and \
             column, got {message:?}",
            row.case
        );

        // Nothing was written: the registry still holds no policy.
        let got = emulator
            .call("GET", &policy_uri(Some(registry)), None)
            .await;
        assert_eq!(
            got.status, 404,
            "case {:?}: a refused write must not create a policy, got {} {:?}",
            row.case, got.status, got.body
        );
    }
}

/// RULE 1 at its edge: a document this emulator's parser refuses for a reason that is
/// not a character in the wrong place — one nested deeper than it will walk — is still
/// reported in the caller's coordinates, by a byte offset into the submitted body and
/// in no parser's line/column vocabulary.
#[tokio::test]
async fn a_body_refused_for_its_nesting_is_still_reported_by_byte_offset() {
    let emulator = Emulator::new();
    let registry = "json-diagnostics-nesting";
    create_registry(&emulator, registry).await;

    for depth in [200usize, 2_000] {
        let case = format!("{depth} nested arrays");
        let body = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let answer = emulator
            .call(
                "POST",
                &schema_uri(registry, &format!("nested-{depth}")),
                Some(json!({"Content": body, "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a document the parser will not walk must be refused 400, \
             got {} {:?}",
            answer.status, answer.body
        );
        let message = answer.str_at("Message");
        assert!(
            message.starts_with("Content is not valid JSON: "),
            "case {case:?}: expected the member's own refusal, got {message:?}"
        );
        assert!(
            !message.contains("line ") && !message.contains("column "),
            "case {case:?}: a refusal must report a byte offset, not a parser's line \
             and column, got {message:?}"
        );
        let offset = offset_in(&message)
            .unwrap_or_else(|| panic!("case {case:?}: expected a byte offset in {message:?}"));
        assert!(
            offset >= 1 && offset <= body.len(),
            "case {case:?}: the offset must name a byte of the submitted body of {} \
             bytes, got {offset} in {message:?}",
            body.len()
        );
    }
}

/// RULE 2: the 412 a stale `RevisionId` precondition is refused with carries the
/// service's constant sentence — the SAME bytes for every rejected revision, every
/// registry and every stored policy, with neither the revision the caller sent nor the
/// one the store minted anywhere in it.
#[tokio::test]
async fn a_stale_revision_precondition_is_refused_with_the_services_constant_sentence() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: which registry, what the
    // stored policy is, and what kind of value the rejected `RevisionId` is — a
    // superseded revision of this policy, another registry's current revision, an id
    // shaped like the ones this store mints but never minted, and values that are not
    // ids at all. Plus the registry that holds no policy, which is at no revision.
    let registries = ["conditional-a", "conditional-b", "unwritten"];
    for registry in registries {
        create_registry(&emulator, registry).await;
    }

    let mut revisions = Vec::new();
    for (index, registry) in registries.iter().enumerate() {
        if *registry == "unwritten" {
            continue;
        }
        let document = format!(r#"{{"Sid":"policy-{index}"}}"#);
        let written = emulator
            .call(
                "PUT",
                &policy_uri(Some(registry)),
                Some(json!({ "Policy": document })),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {registry:?}: the seed write must succeed, got {} {:?}",
            written.status, written.body
        );
        let first = written.str_at("RevisionId");
        // A second write supersedes it, so `first` is a revision this policy really
        // held and no longer does.
        let again = emulator
            .call(
                "PUT",
                &policy_uri(Some(registry)),
                Some(json!({ "Policy": document, "RevisionId": first })),
            )
            .await;
        assert_eq!(
            again.status, 200,
            "case {registry:?}: a write at the current revision must succeed, got {} {:?}",
            again.status, again.body
        );
        revisions.push((registry.to_string(), first, again.str_at("RevisionId")));
    }

    let mut rejected_rows: Vec<(String, String, String)> = Vec::new();
    for (registry, superseded, current) in &revisions {
        for (what, rejected) in [
            ("its own superseded revision", superseded.clone()),
            (
                "an id shaped like ours that was never minted",
                "ffffffff-0000-4000-8000-ffffffffffff".to_string(),
            ),
            ("free text", "not-a-revision".to_string()),
            ("the empty string", String::new()),
            (
                "its current revision with a trailing space",
                format!("{current} "),
            ),
        ] {
            rejected_rows.push((
                format!("{registry}: {what}"),
                registry.clone(),
                rejected,
            ));
        }
    }
    // Another registry's current revision is not this registry's revision, and a
    // registry with no policy is at no revision at all.
    if let [(_, _, first_current), (second, _, _)] = &revisions[..] {
        rejected_rows.push((
            format!("{second}: another registry's current revision"),
            second.clone(),
            first_current.clone(),
        ));
    }
    rejected_rows.push((
        "unwritten: a registry that holds no policy".to_string(),
        "unwritten".to_string(),
        "00000001-0000-4000-8000-000000000001".to_string(),
    ));

    let mut sentences = std::collections::BTreeSet::new();
    for (case, registry, rejected) in rejected_rows {
        let refused = emulator
            .call(
                "PUT",
                &policy_uri(Some(&registry)),
                Some(json!({ "Policy": "{}", "RevisionId": rejected })),
            )
            .await;
        assert_eq!(
            refused.status, 412,
            "case {case:?}: expected 412 from a stale-revision write, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "PreconditionFailedException",
            "case {case:?}: expected PreconditionFailedException, got {:?}",
            refused.body
        );
        let message = refused.str_at("Message");
        assert_eq!(
            message, STALE_REVISION_MESSAGE,
            "case {case:?}: expected the service's constant sentence, got {message:?}"
        );
        if !rejected.trim().is_empty() {
            assert!(
                !message.contains(rejected.trim()),
                "case {case:?}: the refusal must not echo the rejected revision back, \
                 got {message:?}"
            );
        }
        for (_, superseded, current) in &revisions {
            assert!(
                !message.contains(superseded.as_str()) && !message.contains(current.as_str()),
                "case {case:?}: the refusal must carry no revision id the store minted, \
                 got {message:?}"
            );
        }
        sentences.insert(message);
    }
    assert_eq!(
        sentences.len(),
        1,
        "every stale-revision refusal must carry the same sentence, got {sentences:?}"
    );
}
