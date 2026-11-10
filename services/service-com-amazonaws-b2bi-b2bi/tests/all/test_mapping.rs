// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TestMapping`: the mapping dry run that evaluates a caller's
//! JSONata `mappingTemplate` over `inputFileContent`, driven over the wire
//! (awsJson1_0, `x-amz-target: B2BI.TestMapping`) through the same router the emulator
//! boots, so every assertion is about what a real SDK client observes.
//!
//! The rule under test: `mappedFileContent` is the serialized result of EVALUATING the
//! template over the input document — so it is a function of both, and of nothing else —
//! carried as the mapped file's TEXT, JSON-string-encoded into that `String` member, so a
//! client parses the member once for the text and parses the text for the document.
//! Each test states that rule over a TABLE of inputs (or, for the round-trip invariant,
//! over randomly generated documents) rather than one recorded request; the rows differ
//! along what the rule generalises over: the JSONata construct, the result's JSON type,
//! the document's shape and size, and the declared `fileFormat`.
//!
//! The request helpers come from `onboarding_loop.rs`, which exposes them `pub(crate)`
//! for every suite in this test target; only the `TestMapping`-shaped wrapper below is
//! local.
//!
//! Two suites here are about the ENGINE rather than the mapping rule, and are the
//! in-tree evidence for the bounds `src/ops/test_mapping.rs` documents: that caller code
//! reaches no host builtin, and that the string-building constructs are evaluated under
//! the arena clamp while the three whose single step has no bound ahead of it are refused
//! before they run. Two more fuzz and clamp the evaluator: random template and document
//! bytes are always ANSWERED, and an evaluation past the memory ceiling is refused by the
//! allocator with the emulator still serving.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{b2bi_router, call, error_code};

/// `TestMapping`'s success status: awsJson1_0 is an RPC protocol, so the `@http` code the
/// model puts on the operation (201) is inert on the wire and a success is 200.
const MAPPED_STATUS: u16 = 200;

/// Serialises the suites below whose requests occupy an evaluation slot for a long time.
///
/// `src/ops/test_mapping.rs` bounds concurrent evaluations with a PROCESS-WIDE semaphore
/// (`MAX_CONCURRENT_EVALUATIONS`, four permits), and `cargo test` runs the suites in this
/// binary in parallel, so without this gate several slow suites could hold every permit
/// and a row under test would come back as the modelled 429 instead of meeting the bound
/// it is asserting. Each heavy suite takes this lock for its duration, so the refusal a
/// row sees is the one it is about.
static HEAVY_EVALUATIONS: ::tokio::sync::Mutex<()> = ::tokio::sync::Mutex::const_new(());

/// How many random documents the identity round-trip invariant is checked over.
const ROUND_TRIP_CASES: usize = 64;

/// Invoke `TestMapping` with `fileFormat`, `inputFileContent` and `mappingTemplate`.
async fn test_mapping(
    router: &MultiServiceRouter,
    file_format: &str,
    input: &str,
    template: &str,
) -> (u16, Value) {
    call(
        router,
        "TestMapping",
        json!({
            "fileFormat": file_format,
            "inputFileContent": input,
            "mappingTemplate": template,
        }),
    )
    .await
}

/// The mapped file's TEXT, which `mappedFileContent` carries JSON-string-encoded: the
/// member is parsed once for the text, the way a client reads it.
fn mapped_text(body: &Value) -> String {
    let member = body["mappedFileContent"]
        .as_str()
        .unwrap_or_else(|| panic!("mappedFileContent must be a string, got {body}"));
    serde_json::from_str::<String>(member).unwrap_or_else(|failure| {
        panic!("mappedFileContent {member:?} must hold the mapped file's text: {failure}")
    })
}

/// The `mappedFileContent` of a success body, parsed back as JSON: the member holds the
/// mapped file's text as a JSON string literal, so the document takes two parses.
fn mapped_document(body: &Value) -> Value {
    let mapped = mapped_text(body);
    serde_json::from_str(&mapped)
        .unwrap_or_else(|failure| panic!("mappedFileContent {mapped:?} is not JSON: {failure}"))
}

/// JSON equality that holds `3` and `3.0` to be the same number, since JSONata has one
/// numeric type and a mapped integer may come back in either spelling.
fn same_json(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(l), Value::Number(r)) => match (l.as_f64(), r.as_f64()) {
            (Some(l), Some(r)) => l == r,
            _ => l == r,
        },
        (Value::Array(l), Value::Array(r)) => {
            l.len() == r.len() && l.iter().zip(r).all(|(l, r)| same_json(l, r))
        }
        (Value::Object(l), Value::Object(r)) => {
            l.len() == r.len()
                && l.iter()
                    .all(|(key, l)| r.get(key).is_some_and(|r| same_json(l, r)))
        }
        _ => left == right,
    }
}

/// One row of the mapping table: case name, `fileFormat`, input document, template, and
/// the JSON document the mapping must produce.
type MappingCase<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

/// A JSONata template is EVALUATED over the input document, and `mappedFileContent` is
/// the serialized result.
///
/// The rows differ along the JSONata construct (identity, path, filter, constructor,
/// aggregate, string function, concatenation, serialization, transform, conditional), the
/// JSON type of the result (object, array, string, number), the shape of the input
/// (object, array, nested) and the declared `fileFormat` (`JSON`, and `NOT_USED` which
/// declares none). The three constructs this emulator refuses (a regular-expression
/// literal, `$distinct`, `$pad`) have their own suite below.
#[tokio::test]
async fn a_jsonata_template_is_evaluated_over_the_input_document() {
    let router = b2bi_router();

    let cases: &[MappingCase] = &[
        (
            "identity over an object",
            "JSON",
            r#"{"claimId":"C-1","amount":12,"paid":false,"note":null}"#,
            "$",
            r#"{"claimId":"C-1","amount":12,"paid":false,"note":null}"#,
        ),
        (
            "identity over an array",
            "JSON",
            r#"[1,2,3]"#,
            "$",
            r#"[1,2,3]"#,
        ),
        (
            "a path step selects a nested member",
            "JSON",
            r#"{"interchange":{"header":{"senderId":"ACME"}}}"#,
            "interchange.header.senderId",
            r#""ACME""#,
        ),
        (
            "a path step over an array collects every member",
            "JSON",
            r#"{"items":[{"sku":"a"},{"sku":"b"},{"sku":"c"}]}"#,
            "items.sku",
            r#"["a","b","c"]"#,
        ),
        (
            "a predicate filters the array",
            "JSON",
            r#"{"items":[{"sku":"a","amount":5},{"sku":"b","amount":20},{"sku":"c","amount":30}]}"#,
            "items[amount > 10].sku",
            r#"["b","c"]"#,
        ),
        (
            "an object constructor reshapes the document",
            "JSON",
            r#"{"claim":{"id":"X-7"},"items":[{"amount":1},{"amount":2.5}]}"#,
            r#"{"id": claim.id, "total": $sum(items.amount)}"#,
            r#"{"id":"X-7","total":3.5}"#,
        ),
        (
            "an aggregate counts the input",
            "JSON",
            r#"{"items":[{"sku":"a"},{"sku":"b"},{"sku":"c"},{"sku":"d"}]}"#,
            "$count(items)",
            "4",
        ),
        (
            "a string function transforms a member",
            "JSON",
            r#"{"name":"acme supplies"}"#,
            "$uppercase(name)",
            r#""ACME SUPPLIES""#,
        ),
        (
            "a string function slices a member",
            "JSON",
            r#"{"name":"edi-4010"}"#,
            "$substring(name, 0, 3)",
            r#""edi""#,
        ),
        (
            "a conditional takes its true branch",
            "JSON",
            r#"{"amount":40}"#,
            r#"amount > 10 ? "HIGH" : "LOW""#,
            r#""HIGH""#,
        ),
        (
            "the same conditional takes its false branch",
            "JSON",
            r#"{"amount":4}"#,
            r#"amount > 10 ? "HIGH" : "LOW""#,
            r#""LOW""#,
        ),
        (
            "a transform reshapes an interchange into new members",
            "JSON",
            r#"{"interchange":{"sender":"ACME","receiver":"GLOBEX"},"control":4010}"#,
            r#"{"from": $uppercase(interchange.sender), "to": $lowercase(interchange.receiver), "version": control}"#,
            r#"{"from":"ACME","to":"globex","version":4010}"#,
        ),
        (
            "a transform maps and aggregates line items",
            "JSON",
            r#"{"items":[{"sku":"a","qty":1},{"sku":"b","qty":2}]}"#,
            r#"{"skus": items.sku, "lines": $count(items), "units": $sum(items.qty)}"#,
            r#"{"skus":["a","b"],"lines":2,"units":3}"#,
        ),
        (
            "the & operator concatenates a member and a literal",
            "JSON",
            r#"{"sender":"ACME"}"#,
            r#"sender & "-GLOBEX""#,
            r#""ACME-GLOBEX""#,
        ),
        (
            "a transform builds its output by concatenating members",
            "JSON",
            r#"{"interchange":{"sender":"ACME","receiver":"GLOBEX"},"control":4010}"#,
            r#"{"route": interchange.sender & "/" & interchange.receiver, "version": $string(control)}"#,
            r#"{"route":"ACME/GLOBEX","version":"4010"}"#,
        ),
        (
            "$join concatenates the values a path selected",
            "JSON",
            r#"{"items":[{"sku":"a"},{"sku":"b"},{"sku":"c"}]}"#,
            r#"$join(items.sku, "-")"#,
            r#""a-b-c""#,
        ),
        (
            "$replace rewrites a matched substring",
            "JSON",
            r#"{"version":"edi-4010"}"#,
            r#"$replace(version, "4010", "5010")"#,
            r#""edi-5010""#,
        ),
        (
            "$split on a string separator",
            "JSON",
            r#"{"line":"a,b,c"}"#,
            r#"$split(line, ",")"#,
            r#"["a","b","c"]"#,
        ),
        (
            "fileFormat NOT_USED declares no format and is mapped as JSON",
            "NOT_USED",
            r#"{"a":{"b":7}}"#,
            "a.b",
            "7",
        ),
        (
            "an array input is mapped as the root context",
            "NOT_USED",
            r#"[{"qty":2},{"qty":5}]"#,
            "$sum(qty)",
            "7",
        ),
    ];

    for (case, file_format, input, template, expected) in cases {
        let (status, body) = test_mapping(&router, file_format, input, template).await;
        assert_eq!(
            status, MAPPED_STATUS,
            "case {case:?}: expected {MAPPED_STATUS}, got {status} {body}"
        );
        let actual = mapped_document(&body);
        let expected: Value = serde_json::from_str(expected).expect("the expectation is JSON");
        assert!(
            same_json(&actual, &expected),
            "case {case:?}: expected {expected}, got {actual}"
        );
    }
}

/// A seeded xorshift64* generator, so a failing round-trip case can be replayed from the
/// seed this test prints.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.0 = state;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A value in `0..bound`.
    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

/// A short string over an alphabet with no JSON escapes, for a generated member or key.
fn random_text(rng: &mut Rng, min_len: u64) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let length = min_len + rng.below(8);
    (0..length)
        .map(|_| ALPHABET[rng.below(ALPHABET.len() as u64) as usize] as char)
        .collect()
}

/// The seed both generators below start from.
///
/// A CONSTANT, not the clock: a failure in a gate run has to replay by re-running the
/// test, which a clock seed does not allow. Set `B2BI_TEST_MAPPING_SEED` to sweep other
/// cases locally; either way the seed in force is printed.
const GENERATOR_SEED: u64 = 0x5EED_B2B1_0000_0001;

/// [`GENERATOR_SEED`], or the override in `B2BI_TEST_MAPPING_SEED`. The low bit is set
/// because an xorshift generator seeded with 0 stays at 0.
fn generator_seed() -> u64 {
    std::env::var("B2BI_TEST_MAPPING_SEED")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(GENERATOR_SEED)
        | 1
}

/// A random JSON document, nesting at most `depth` levels further: null, a boolean, an
/// integer, a string, an array or an object.
fn random_document(rng: &mut Rng, depth: u64) -> Value {
    let kinds = if depth == 0 { 4 } else { 6 };
    match rng.below(kinds) {
        0 => Value::Null,
        1 => json!(rng.below(2) == 1),
        2 => json!(rng.below(2_001) as i64 - 1_000),
        3 => json!(random_text(rng, 0)),
        4 => Value::Array(
            (0..rng.below(5))
                .map(|_| random_document(rng, depth - 1))
                .collect(),
        ),
        _ => Value::Object(
            (0..rng.below(5))
                .map(|_| (random_text(rng, 1), random_document(rng, depth - 1)))
                .collect(),
        ),
    }
}

/// The invariant: the identity template `$` maps any input document to itself, whatever
/// its JSON type, nesting or size.
///
/// Checked over [`ROUND_TRIP_CASES`] generated documents plus the degenerate ones a
/// generator rarely produces (an empty object and array, a lone scalar, a 200-element
/// array), so the property is exercised rather than one recorded shape.
#[tokio::test]
async fn the_identity_template_round_trips_any_input_document() {
    let _heavy = HEAVY_EVALUATIONS.lock().await;
    let router = b2bi_router();

    let seed = generator_seed();
    println!("the_identity_template_round_trips_any_input_document seed = {seed}");
    let mut rng = Rng(seed);

    let mut documents = vec![
        json!({}),
        json!([]),
        json!(null),
        json!(0),
        json!("edi"),
        Value::Array((0..200).map(|index| json!({ "line": index })).collect()),
    ];
    documents.extend((0..ROUND_TRIP_CASES).map(|_| random_document(&mut rng, 4)));

    for document in documents {
        let input = serde_json::to_string(&document).expect("the document serializes");
        let (status, body) = test_mapping(&router, "JSON", &input, "$").await;
        assert_eq!(
            status, MAPPED_STATUS,
            "seed {seed}, document {input}: expected {MAPPED_STATUS}, got {status} {body}"
        );
        let actual = mapped_document(&body);
        assert!(
            same_json(&actual, &document),
            "seed {seed}, document {input}: identity mapping returned {actual}"
        );
    }
}

/// An `inputFileContent` that is not a well-formed document of the declared format is
/// refused with the modelled `ValidationException` (HTTP 400) — never mapped, and never
/// answered with a 501, which would claim the operation is unimplemented. The sentence is
/// the JSON parser's own diagnostic of where the document stopped parsing, with the
/// document itself withheld (`aws_worded_refusals.rs` asserts that wording over a table of
/// its own).
#[tokio::test]
async fn a_malformed_input_document_is_refused_as_a_validation_exception() {
    let router = b2bi_router();

    let cases: &[(&str, &str, String)] = &[
        ("truncated object", "JSON", r#"{"a":"#.to_string()),
        ("unquoted word", "JSON", "not json at all".to_string()),
        ("unterminated array", "NOT_USED", "[1,2".to_string()),
        ("trailing comma", "JSON", r#"{"a":1,}"#.to_string()),
        ("two documents", "JSON", r#"{"a":1}{"b":2}"#.to_string()),
        // Whitespace is content, not an absent document: only a zero-length
        // `inputFileContent` means "no document".
        ("whitespace only", "JSON", "   \n\t ".to_string()),
        (
            // Past the JSON reader's own recursion limit, which is what bounds how
            // deeply either parser can be driven.
            "nested past the reader's recursion limit",
            "JSON",
            format!("{}1{}", "[".repeat(300), "]".repeat(300)),
        ),
    ];

    for (case, file_format, input) in cases {
        let (status, body) = test_mapping(&router, file_format, input, "$").await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        // `Message` is the member this service's model declares (capital M).
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains("at [Source: REDACTED"),
            "case {case:?}: the refusal must carry the parser's own diagnostic with the \
             document withheld, got {body}"
        );
        assert!(
            !message.contains("inputFileContent"),
            "case {case:?}: the parser's diagnostic names no input member, got {body}"
        );
    }
}

/// A `mappingTemplate` the JSONata interpreter will not accept is refused the same way:
/// a 400 `ValidationException` carrying the engine's own account of the failure under the
/// fixed clause it reports a JSONata failure with (`jsonata_probe.rs` asserts that
/// wording over a table of its own), or — for the one template this emulator refuses
/// BEFORE the engine sees it, because its scan cannot read past an unterminated literal —
/// this service's own sentence naming the member.
#[tokio::test]
async fn an_unusable_mapping_template_is_refused_as_a_validation_exception() {
    let router = b2bi_router();

    /// The fixed clause a refusal carrying the engine's diagnosis begins with.
    const JSONATA_FAILURE: &str = "JSONata error. Failed to map input file:";

    // (case, the template, whether the ENGINE is what diagnosed it)
    let cases: &[(&str, &str, bool)] = &[
        ("unclosed parenthesis", "(", true),
        ("dangling operator", "1 +", true),
        // Refused by this emulator's own pre-parse scan, which cannot read past an
        // unterminated literal, so there is no engine diagnosis to report.
        ("unterminated string", r#""abc"#, false),
        ("unclosed object constructor", r#"{"a": 1"#, true),
        ("unknown function", "$notAFunction(a)", true),
        ("arithmetic on a string member", r#""text" - 1"#, true),
    ];

    for (case, template, engine_diagnosed) in cases {
        let (status, body) = test_mapping(&router, "JSON", r#"{"a":1}"#, template).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        // `Message`, not `message`: this service's model names
        // `ValidationException$Message` with a capital M, and the emulator renders the
        // member the model declares.
        let message = body["Message"].as_str().unwrap_or_default();
        if *engine_diagnosed {
            assert!(
                message.starts_with(JSONATA_FAILURE),
                "case {case:?}: the refusal must carry the engine's diagnosis under \
                 {JSONATA_FAILURE:?}, got {body}"
            );
        } else {
            assert!(
                message.contains("mappingTemplate"),
                "case {case:?}: the refusal must name mappingTemplate, got {body}"
            );
        }
    }
}

/// An XSLT `mappingTemplate` — the other `Mapping$templateLanguage` this service models
/// — needs an XSLT engine this emulator does not have, so it is answered with 501
/// `NotImplemented` rather than with a fabricated mapping.
#[tokio::test]
async fn an_xslt_mapping_template_is_answered_with_not_implemented() {
    let router = b2bi_router();

    let stylesheet = r#"<?xml version="1.0"?>
<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:template match="/"><out><xsl:value-of select="claim/id"/></out></xsl:template>
</xsl:stylesheet>"#;

    for (case, file_format, input) in [
        ("XML input", "XML", "<claim><id>X-1</id></claim>"),
        ("JSON input", "JSON", r#"{"claim":{"id":"X-1"}}"#),
    ] {
        let (status, body) = test_mapping(&router, file_format, input, stylesheet).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected 501, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: expected NotImplemented, got {body}"
        );
    }
}

/// A JSONata template cannot be evaluated over a document the caller declares to be XML:
/// that pairing needs an XSLT engine this emulator does not have, so it is answered with
/// the 501 naming that missing capability — the same shape as an XSLT template — and never
/// with an empty success.
#[tokio::test]
async fn a_jsonata_template_over_an_xml_input_document_is_refused() {
    let router = b2bi_router();

    for (case, input) in [
        ("an XML document", "<claim><id>X-1</id></claim>"),
        ("JSON content declared as XML", r#"{"claim":{"id":"X-1"}}"#),
    ] {
        let (status, body) = test_mapping(&router, "XML", input, "claim.id").await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected 501, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: expected NotImplemented, got {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("XSLT")),
            "case {case:?}: the refusal must name the missing engine, got {body}"
        );
    }
}

/// Caller-supplied code runs under bounds, and each one answers a refusal that names
/// the emulator's own limit rather than running on: a template that never finishes, one
/// that recurses past the evaluation depth, and one that allocates without limit.
///
/// The rows differ along WHICH resource the template tries to exhaust (time, stack,
/// memory); each asserts the 400 and that the message names this emulator's bound, not
/// which bound fired first — several of these bounds cover the same template. The two
/// SIZE ceilings are a different answer (the 501 for work this emulator declines to
/// attempt) and have their own test below.
#[tokio::test]
async fn a_template_that_cannot_finish_is_refused_within_the_evaluation_bounds() {
    let _heavy = HEAVY_EVALUATIONS.lock().await;
    let router = b2bi_router();

    let small = r#"{"a":1}"#.to_string();

    let cases: Vec<(&str, String, String)> = vec![
        (
            "unbounded recursion",
            small.clone(),
            "($f := function($x) { $f($x + 1) }; $f(0))".to_string(),
        ),
        (
            "nesting past the evaluation depth",
            small.clone(),
            // A function that recurses at evaluation time: the interpreter's OWN depth
            // box ends it, which is a failure found WHILE evaluating and so the modelled
            // 400. (The pre-parse nesting ceiling is a different answer — the 501 — and
            // has its own suite.)
            "($deep := function($n) { $n > 0 ? [$deep($n - 1)] : 0 }; $deep(400))".to_string(),
        ),
        (
            "unbounded allocation",
            small,
            // A million 32-character strings: past the 64 MiB arena ceiling
            // `src/ops/test_mapping.rs` publishes, which the arena's own allocation
            // limit refuses BEFORE the memory is taken. The range operator is the
            // engine's own bounded construct (1e7 elements), so this row allocates
            // through it rather than through a free width argument.
            r#"$count($map([1..1000000], function($x) { "0123456789abcdef0123456789abcdef" }))"#
                .to_string(),
        ),
    ];

    for (case, input, template) in &cases {
        let (status, body) = test_mapping(&router, "JSON", input, template).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert!(
            body["Message"]
                .as_str()
                .is_some_and(|message| message.contains("emulator")),
            "case {case:?}: the refusal must name this emulator's bound, got {body}"
        );
    }
}

/// A request INSIDE the modelled lengths that is past one of this emulator's own, smaller
/// size ceilings is work this emulator declines to attempt, so it is answered with the
/// 501 naming that ceiling — the same shape as the missing XSLT engine — rather than with
/// a 400 that would tell the caller a valid request is invalid.
///
/// The rows are one byte past each published ceiling in `src/ops/test_mapping.rs`
/// (MAX_TEMPLATE_BYTES, 16 KiB; MAX_DOCUMENT_BYTES, 1 MiB) and, as the control, one just
/// INSIDE each, which must be mapped.
#[tokio::test]
async fn a_request_past_this_emulators_size_ceilings_is_not_implemented() {
    let router = b2bi_router();

    let over_template_bound = format!("{}1", "1 + ".repeat(5_000));
    let over_document_bound = format!(r#"{{"big":"{}"}}"#, "p".repeat(1_100_000));
    let small_document = r#"{"a":1}"#.to_string();

    let refused: Vec<(&str, String, String)> = vec![
        (
            "a template past the size ceiling",
            small_document.clone(),
            over_template_bound,
        ),
        (
            "a document past the size ceiling",
            over_document_bound,
            "$".to_string(),
        ),
    ];

    for (case, input, template) in &refused {
        let (status, body) = test_mapping(&router, "JSON", input, template).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected 501, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: expected NotImplemented, got {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("emulator")),
            "case {case:?}: the refusal must name this emulator's ceiling, got {body}"
        );
    }

    // Just inside each ceiling: still mapped, so the refusals above are about the ceiling
    // and not about size as such. The template is padded with a COMMENT, so its bytes
    // are inside the size ceiling without nesting past the evaluation depth.
    let inside_template_bound = format!("/* {} */ a", "p".repeat(15_000));
    let inside_document_bound = format!(r#"{{"big":"{}"}}"#, "p".repeat(900_000));

    let accepted: Vec<(&str, String, String)> = vec![
        (
            "a template just inside the size ceiling",
            small_document,
            inside_template_bound,
        ),
        (
            "a document just inside the size ceiling",
            inside_document_bound,
            "$count($keys($))".to_string(),
        ),
    ];

    for (case, input, template) in &accepted {
        let (status, body) = test_mapping(&router, "JSON", input, template).await;
        assert_eq!(
            status, MAPPED_STATUS,
            "case {case:?}: expected {MAPPED_STATUS}, got {status} {body}"
        );
    }
}

/// A template at exactly the size ceiling, in every nesting-dense shape, is ANSWERED —
/// the parse never overflows the evaluation thread's stack, which is the one failure that
/// would abort the serving process instead of unwinding into a refusal.
///
/// This is the test behind `EVALUATION_STACK_BYTES`'s sizing. The rows whose nesting the
/// pre-parse count can see are refused with the 501 naming `MAX_TEMPLATE_NESTING`
/// (256 levels) — work this emulator declines before attempting it, the same answer its
/// size ceilings give; the rows that are long but FLAT, and the rows that HIDE 16 KiB of
/// one-byte nesting behind a token that count reads differently from the engine's lexer
/// (a quote inside a regex literal or a backtick-quoted name, an unterminated quote),
/// reach the parser at the full 16 KiB and must be answered too — which is the property
/// the stack, rather than the count, is what provides. Every row is built to the
/// published MAX_TEMPLATE_BYTES, so the worst template the length bound admits is
/// exercised rather than inferred.
#[tokio::test]
async fn a_nesting_dense_template_at_the_size_ceiling_is_answered_not_fatal() {
    let _heavy = HEAVY_EVALUATIONS.lock().await;
    let router = b2bi_router();

    // 16 KiB = MAX_TEMPLATE_BYTES in `src/ops/test_mapping.rs`.
    const CEILING: usize = 16 * 1024;

    // Shapes whose depth is past the nesting ceiling: refused by the pre-parse count.
    let nested: Vec<(&str, String)> = vec![
        ("unbalanced parentheses", "(".repeat(CEILING)),
        (
            "balanced parentheses",
            format!(
                "{}1{}",
                "(".repeat(CEILING / 2 - 1),
                ")".repeat(CEILING / 2 - 1)
            ),
        ),
        (
            "balanced brackets",
            format!(
                "{}1{}",
                "[".repeat(CEILING / 2 - 1),
                "]".repeat(CEILING / 2 - 1)
            ),
        ),
        ("open braces", "{".repeat(CEILING)),
        ("prefix operators", format!("{}1", "-".repeat(CEILING - 1))),
        (
            "nested function calls",
            format!(
                "{}[]{}",
                "$count(".repeat(CEILING / 8 - 1),
                ")".repeat(CEILING / 8 - 1)
            ),
        ),
    ];

    for (case, template) in &nested {
        assert!(
            template.len() <= CEILING,
            "case {case:?}: the row must be inside the size ceiling, got {} bytes",
            template.len()
        );
        let (status, body) = test_mapping(&router, "JSON", r#"{"a":1}"#, template).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected the 501 naming the nesting ceiling, got {status} {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("nests")),
            "case {case:?}: the refusal must name the nesting ceiling, got {body}"
        );
    }

    // Shapes that are long but flat, or whose structure the nesting scan cannot see:
    // they reach the parser at the ceiling, and each is answered (mapped, or refused by
    // a bound this emulator publishes) rather than ending the process. The last three
    // are 16 KiB of one-byte nesting hidden behind a token the scan reads differently
    // from the engine's lexer — the case the stack, not the scan, is what covers.
    let flat: Vec<(&str, String)> = vec![
        ("a path run", format!("a{}", ".a".repeat(8_000))),
        ("an arithmetic run", format!("1{}", "+1".repeat(8_000))),
        (
            "a concatenation run",
            format!("a{}", "&a".repeat(8_000)),
        ),
        (
            "a conditional run",
            format!("{}1", "1?1:".repeat(4_000)),
        ),
        (
            "a boolean run",
            format!("{}true", "true and ".repeat(1_800)),
        ),
        (
            "a chain-operator run",
            format!("[1]{}", "~>$count".repeat(2_000)),
        ),
        (
            "nesting behind a quote inside a regex literal",
            format!(r#"/"/{}"#, "(".repeat(16_000)),
        ),
        (
            "nesting behind a quote inside a backtick-quoted name",
            format!("`a\"b`{}", "(".repeat(16_000)),
        ),
        (
            "nesting behind an unterminated quote",
            format!("\"{}", "(".repeat(16_000)),
        ),
    ];

    for (case, template) in &flat {
        assert!(
            template.len() <= CEILING,
            "case {case:?}: the row must be inside the size ceiling, got {} bytes",
            template.len()
        );
        let (status, body) = test_mapping(&router, "JSON", r#"{"a":1}"#, template).await;
        assert!(
            status == MAPPED_STATUS || status == 400 || status == 501,
            "case {case:?}: the template must be ANSWERED, got {status} {body}"
        );
    }

    // And the process is still serving every one of them.
    let (status, body) = test_mapping(&router, "JSON", r#"{"a":"edi"}"#, "$uppercase(a)").await;
    assert_eq!(
        status, MAPPED_STATUS,
        "the emulator must still serve after the densest templates, got {status} {body}"
    );
}

/// A builtin that walks a WHOLE large array in one uninterruptible step still answers
/// inside this emulator's bounds, and the emulator keeps serving afterwards.
///
/// The interpreter checks its budget BETWEEN steps, so a native whole-array pass is time-
/// bounded only by how its work grows with the array: the ones here walk their operand
/// once (or sort it in `n log n`), so the arena ceiling bounds them, and the rows drive
/// each over an array built to hundreds of thousands of members. `$distinct`, whose
/// native pass compares each member against the ones already kept and is therefore
/// quadratic in the member count, is the one refused with the 501 instead — pinned here
/// beside the others, at the same array size, so the difference is asserted rather than
/// asserted about.
#[tokio::test]
async fn a_whole_array_builtin_over_a_large_array_still_answers() {
    let _heavy = HEAVY_EVALUATIONS.lock().await;
    let router = b2bi_router();

    // 200 000 members: an array that fits the published arena ceiling, so the bound each
    // row meets is the one its builtin's own growth implies. Where the pass is cheap and
    // native, the row asserts the VALUE it must compute; the rows whose cost is a
    // function call per member (the predicate filter, `$reduce`) may legitimately meet
    // the 500 ms box instead, and assert only that they are answered.
    let computed: &[(&str, &str, Value)] = &[
        ("counting a large array", r#"$count([1..200000])"#, json!(200_000)),
        (
            "counting a sorted large array",
            r#"$count($sort([1..200000]))"#,
            json!(200_000),
        ),
        (
            "the first member of a reversed large array",
            r#"$reverse([1..200000])[0]"#,
            json!(200_000),
        ),
        (
            "counting two appended large arrays",
            r#"$count($append([1..200000], [1..200000]))"#,
            json!(400_000),
        ),
        (
            "counting two zipped large arrays",
            r#"$count($zip([1..200000], [1..200000]))"#,
            json!(200_000),
        ),
        ("summing a large array", r#"$sum([1..200000])"#, json!(20_000_100_000u64)),
        ("membership in a large array", r#"199999 in [1..200000]"#, json!(true)),
    ];

    for (case, template, expected) in computed {
        let (status, body) = test_mapping(&router, "JSON", r#"{"a":1}"#, template).await;
        assert_eq!(
            status, MAPPED_STATUS,
            "case {case:?}: expected {MAPPED_STATUS}, got {status} {body}"
        );
        let actual = mapped_document(&body);
        assert!(
            same_json(&actual, expected),
            "case {case:?}: expected {expected}, got {actual}"
        );
    }

    let answered: &[(&str, &str)] = &[
        ("filtering a large array", r#"$count([1..200000][$ > 199000])"#),
        (
            "reducing a large array",
            r#"$reduce([1..200000], function($acc, $x) { $acc + $x })"#,
        ),
    ];

    for (case, template) in answered {
        let (status, body) = test_mapping(&router, "JSON", r#"{"a":1}"#, template).await;
        assert!(
            status == MAPPED_STATUS || status == 400,
            "case {case:?}: expected a mapped answer or a modelled refusal, got {status} {body}"
        );
        if status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: a refusal must be the modelled one, got {body}"
            );
        }
    }

    // The quadratic one is refused before it runs, at the same size.
    let (status, body) = test_mapping(
        &router,
        "JSON",
        r#"{"a":1}"#,
        r#"$count($distinct([1..200000]))"#,
    )
    .await;
    assert_eq!(
        status, 501,
        "a quadratic whole-array builtin must be refused with the 501, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotImplemented",
        "expected NotImplemented, got {body}"
    );

    // And the emulator is still serving: no row held a thread past its answer.
    let (status, body) = test_mapping(&router, "JSON", r#"{"a":"edi"}"#, "$uppercase(a)").await;
    assert_eq!(
        status, MAPPED_STATUS,
        "the emulator must still serve after the whole-array rows, got {status} {body}"
    );
}

/// A string-building builtin asks the ARENA for the buffer it writes its result into, so
/// the arena's allocation limit is what refuses an aliasing amplification — BEFORE the
/// memory is asked of the system, and never by a process that stops serving.
///
/// This suite is the evidence behind `src/ops/test_mapping.rs`'s claim that the clamp
/// precedes the allocation for `$string`, `&`, `$join` and `$replace`. The interpreter
/// holds values behind arena references, so an array of N aliases to an M-byte member
/// costs the arena O(N) pointers while the builtin's buffer is O(N*M); the question this
/// suite answers is WHICH bound that buffer meets. Each row asserts the bound by NAME:
///
/// * past the 64 MiB arena ceiling (a 100 KB member aliased a thousand times, ~100 MB of
///   buffer), the refusal names 67108864 — the engine asked the arena for the whole
///   buffer and `bumpalo`'s allocation limit declined the layout, which unwinds into the
///   modelled 400. An engine upgrade that moved such a buffer onto the global heap (where
///   a failure aborts instead of unwinding) fails this assertion;
/// * inside the arena ceiling but past the 5 000 000-byte answer ceiling (the same member
///   aliased a hundred times, ~10 MB), the refusal names 5000000, charged on the value
///   before it is serialized.
///
/// The last row drives the same amplification through a nested alias.
#[tokio::test]
async fn a_reference_multiplying_string_builtin_is_refused_by_the_arena_clamp() {
    let _heavy = HEAVY_EVALUATIONS.lock().await;
    let router = b2bi_router();

    // A 100 KB member, so a mapping that materialised the buffer needs N × 100 KB. The
    // numbers in the expectations are the ceilings `src/ops/test_mapping.rs` publishes:
    // MAX_EVALUATION_ARENA_BYTES = 67108864 and MAX_MAPPED_FILE_BYTES = 5000000.
    let document = format!(r#"{{"big":"{}"}}"#, "x".repeat(100_000));

    let cases: &[(&str, &str, &str)] = &[
        (
            "one member joined a thousand times",
            r#"($b := big; $join($map([1..1000], function($i) { $b }), ""))"#,
            "67108864",
        ),
        (
            "one member concatenated through an array",
            r#"($b := big; $map([1..1000], function($i) { $b }) & "")"#,
            "67108864",
        ),
        (
            "one member serialized once per reference",
            r#"($b := big; $string($map([1..1000], function($i) { $b })))"#,
            "67108864",
        ),
        (
            "one member concatenated by folding over the aliases",
            r#"($b := big; $reduce($map([1..1000], function($i) { $b }), function($acc, $x) { $acc & $x }))"#,
            "67108864",
        ),
        (
            "one member replaced into a thousand times its size",
            r#"($b := big; $replace($b, "x", "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"))"#,
            "67108864",
        ),
        (
            "one member joined a hundred times",
            r#"($b := big; $join($map([1..100], function($i) { $b }), ""))"#,
            "5000000",
        ),
        (
            "one member joined through a nested alias",
            r#"($b := big; $row := $map([1..200], function($i) { $b }); $join([$row, $row, $row, $row], ""))"#,
            "67108864",
        ),
    ];

    for (case, template, ceiling) in cases {
        let (status, body) = test_mapping(&router, "JSON", &document, template).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400 naming a published bound, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert!(
            body["Message"]
                .as_str()
                .is_some_and(|message| message.contains(ceiling)),
            "case {case:?}: the refusal must name the {ceiling}-byte ceiling, got {body}"
        );
    }

    // And the process is still serving: every row unwound into a refusal.
    let (status, body) = test_mapping(&router, "JSON", r#"{"a":"edi"}"#, "$uppercase(a)").await;
    assert_eq!(
        status, MAPPED_STATUS,
        "the emulator must still serve after a refused amplification, got {status} {body}"
    );
}

/// `TestMapping` is a dry run: it is a function of its request alone, so the same
/// request answers the same document and nothing a later caller reads has changed.
#[tokio::test]
async fn mapping_is_repeatable_and_persists_nothing() {
    let router = b2bi_router();

    let (status, before) = call(&router, "ListProfiles", json!({})).await;
    assert_eq!(status, 200, "ListProfiles failed: {status} {before}");

    let input = r#"{"items":[{"amount":3},{"amount":4}]}"#;
    let template = r#"{"total": $sum(items.amount), "count": $count(items)}"#;
    let (status, first) = test_mapping(&router, "JSON", input, template).await;
    assert_eq!(
        status, MAPPED_STATUS,
        "the first mapping failed: {status} {first}"
    );
    let (status, second) = test_mapping(&router, "JSON", input, template).await;
    assert_eq!(
        status, MAPPED_STATUS,
        "the replayed mapping failed: {status} {second}"
    );
    assert_eq!(
        first, second,
        "the same request must map to the same document, got {first} then {second}"
    );
    // Byte-identical, not merely equivalent: the mapped document is written as text with
    // every object's members in sorted key order, so the answer does not depend on the
    // interpreter's hash order.
    assert_eq!(
        mapped_text(&first),
        "{\n  \"count\": 2,\n  \"total\": 7\n}",
        "expected the mapped document as pretty-printed, key-sorted text, got {first}"
    );
    assert!(
        same_json(&mapped_document(&first), &json!({"total": 7, "count": 2})),
        "expected the computed totals, got {first}"
    );

    let (status, after) = call(&router, "ListProfiles", json!({})).await;
    assert_eq!(status, 200, "ListProfiles failed: {status} {after}");
    assert_eq!(
        before, after,
        "a mapping dry run must not change what a later caller reads: {before} then {after}"
    );
}

/// The mapping engine reaches nothing on the host: every name a host escape would be
/// called by is simply an unknown function, refused with the modelled 400.
///
/// This is the observable half of the engine's capability surface — the function table
/// is fixed by the interpreter (`jsonata-rs`'s `bind_native!` bindings) and holds no
/// filesystem, process, socket, environment, import or eval builtin. The table below is
/// the set of names such a builtin would carry, including the two the interpreter's own
/// dependency graph might suggest (`glob` for filesystem globbing, a regex engine's
/// compile step); a KNOWN builtin is checked in the same test so a pass cannot come from
/// every template failing.
#[tokio::test]
async fn the_mapping_engine_exposes_no_host_builtin() {
    let router = b2bi_router();

    let host_escapes: &[(&str, &str)] = &[
        ("read a file", r#"$readFile("/etc/passwd")"#),
        ("write a file", r#"$writeFile("/tmp/x", "y")"#),
        ("open a file", r#"$open("/etc/hosts")"#),
        ("glob the filesystem", r#"$glob("/**/*")"#),
        ("run a process", r#"$exec("id")"#),
        ("run a shell", r#"$shell("id")"#),
        ("spawn", r#"$spawn("id")"#),
        ("read the environment", r#"$env("AWS_SECRET_ACCESS_KEY")"#),
        ("read the environment, again", r#"$getenv("PATH")"#),
        ("fetch a URL", r#"$fetch("http://169.254.169.254/")"#),
        ("make an HTTP call", r#"$http("http://example.invalid")"#),
        ("open a socket", r#"$socket("127.0.0.1", 9297)"#),
        ("import a module", r#"$import("fs")"#),
        ("require a module", r#"$require("fs")"#),
        ("evaluate more code", r#"$eval("1+1")"#),
        ("call out to the system", r#"$system("id")"#),
    ];

    for (case, template) in host_escapes {
        let (status, body) = test_mapping(&router, "JSON", r#"{"a":1}"#, template).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a host escape must be an unknown function, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // The control: a builtin the engine DOES have still evaluates, so the refusals above
    // are about those names and not about every template.
    let (status, body) = test_mapping(&router, "JSON", r#"{"a":"edi"}"#, "$uppercase(a)").await;
    assert_eq!(
        status, MAPPED_STATUS,
        "a known builtin must still evaluate, got {status} {body}"
    );
    assert!(
        same_json(&mapped_document(&body), &json!("EDI")),
        "expected the uppercased member, got {body}"
    );
}

/// The memory clamp is the arena's own allocation limit, so an evaluation that asks for
/// more than the ceiling is refused by the allocator BEFORE the memory is taken, and the
/// refusal is answered as the modelled 400 naming that ceiling — never by the process
/// dying.
///
/// The rows differ by how the allocation is reached (many small values, one large value),
/// since the clamp is on the arena rather than on any one builtin; the
/// reference-multiplying case a string builtin could amplify has its own test above.
#[tokio::test]
async fn an_evaluation_past_the_memory_ceiling_is_refused_by_the_allocator() {
    let _heavy = HEAVY_EVALUATIONS.lock().await;
    let router = b2bi_router();

    let cases: &[(&str, &str)] = &[
        (
            "a million small strings",
            r#"$count($map([1..1000000], function($x) { "0123456789abcdef0123456789abcdef" }))"#,
        ),
        (
            "a million numbers held as an array",
            r#"$count($sort([1..1000000]))"#,
        ),
    ];

    for (case, template) in cases {
        let (status, body) = test_mapping(&router, "JSON", r#"{"a":1}"#, template).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        // 67108864 = the 64 MiB MAX_EVALUATION_ARENA_BYTES in `src/ops/test_mapping.rs`.
        // Either refusal is correct for these templates — several of this emulator's
        // bounds cover them, and which one is reached first depends on how fast the host
        // evaluates — so the assertion accepts the arena ceiling or the time bound,
        // never a success and never a 5xx.
        assert!(
            body["Message"].as_str().is_some_and(|message| {
                message.contains("67108864") || message.contains("did not finish evaluating")
            }),
            "case {case:?}: the refusal must name the memory ceiling or the time bound, got {body}"
        );
    }

    // And the process is still serving: the refusal unwound the evaluation instead of
    // ending the emulator.
    let (status, body) = test_mapping(&router, "JSON", r#"{"a":"edi"}"#, "$uppercase(a)").await;
    assert_eq!(
        status, MAPPED_STATUS,
        "the next request after a refused allocation must still be served, got {status} {body}"
    );
}

/// Fuzzing the evaluator's two untrusted inputs: whatever bytes arrive as a
/// `mappingTemplate` or an `inputFileContent`, the operation ANSWERS — a mapped document
/// (200), a refusal this emulator models (400), or the 501 for a template it reads as
/// XSLT or whose construct it will not evaluate — and never a 5xx, never a hang, and
/// never a process that stops serving.
///
/// The generator is seeded and its seed printed, so a failing case replays; the alphabet
/// is JSONata's own punctuation and identifier bytes, so the cases reach the tokenizer,
/// the parser and the evaluator rather than bouncing off the boundary validator.
#[tokio::test]
async fn random_templates_and_documents_are_always_answered() {
    let _heavy = HEAVY_EVALUATIONS.lock().await;
    let router = b2bi_router();

    let seed = generator_seed();
    println!("random_templates_and_documents_are_always_answered seed = {seed}");
    let mut rng = Rng(seed);

    /// JSONata's punctuation, plus identifier and literal bytes.
    const ALPHABET: &[u8] =
        b"abcxyz0129$.()[]{}&|*+-/<>=!~?:,'\" \n\\^%@#;_abc[1..9]$sum$map$count";

    for case in 0..128u32 {
        let template: String = (0..1 + rng.below(24))
            .map(|_| ALPHABET[rng.below(ALPHABET.len() as u64) as usize] as char)
            .collect();
        let document: String = if rng.below(2) == 0 {
            serde_json::to_string(&random_document(&mut rng, 3)).expect("the document serializes")
        } else {
            (0..rng.below(24))
                .map(|_| ALPHABET[rng.below(ALPHABET.len() as u64) as usize] as char)
                .collect()
        };

        let (status, body) = test_mapping(&router, "JSON", &document, &template).await;
        assert!(
            status == MAPPED_STATUS || status == 400 || status == 501,
            "seed {seed}, case {case}: template {template:?} over document {document:?} \
             answered {status} {body}"
        );
    }

    // Still serving after every one of them.
    let (status, body) = test_mapping(&router, "JSON", r#"{"a":"edi"}"#, "$uppercase(a)").await;
    assert_eq!(
        status, MAPPED_STATUS,
        "seed {seed}: the emulator must still serve after the fuzz run, got {status} {body}"
    );
}

/// A template the ENGINE itself mishandles — applying a regular-expression literal as a
/// function makes `jsonata-rs`'s parser reach `unreachable!()` — is contained: the
/// unwind is caught on the evaluation thread and answered as the modelled 400 naming the
/// member, and the emulator keeps serving. A single request cannot end the process.
#[tokio::test]
async fn an_engine_panic_is_contained_and_the_emulator_keeps_serving() {
    let _heavy = HEAVY_EVALUATIONS.lock().await;
    let router = b2bi_router();

    for (case, template) in [
        ("a regex applied as a function", r#"/^(a+)+$/("aaaa")"#),
        (
            "a regex applied as a function after and",
            r#"true and /^(a+)+$/("aaaa")"#,
        ),
        ("a template of bare operators", r#"*()z91]!1a"#),
    ] {
        let (status, body) = test_mapping(&router, "JSON", r#"{"a":1}"#, template).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert!(
            body["Message"]
                .as_str()
                .is_some_and(|message| message.contains("mappingTemplate")),
            "case {case:?}: the refusal must name mappingTemplate, got {body}"
        );
    }

    let (status, body) = test_mapping(&router, "JSON", r#"{"a":"edi"}"#, "$uppercase(a)").await;
    assert_eq!(
        status, MAPPED_STATUS,
        "the emulator must still serve after an engine panic, got {status} {body}"
    );
}

/// The ceiling on the answered document is enforced on the VALUE, before it is
/// serialized: the interpreter holds values behind arena references, so a template that
/// aliases one value into many places has a small arena footprint and a large
/// serialization, and that serialization would be allocated outside the arena's clamp.
///
/// The rows differ by how the result is grown past the ceiling — by aliasing one value
/// repeatedly, and by one long string repeated — and each asserts the 400 naming the
/// ceiling, with the emulator still serving afterwards.
#[tokio::test]
async fn a_result_past_the_answer_ceiling_is_refused_before_it_is_serialized() {
    let _heavy = HEAVY_EVALUATIONS.lock().await;
    let router = b2bi_router();

    let cases: &[(&str, &str)] = &[
        (
            "one value aliased ten times",
            r#"($big := $map([1..20000], function($x) { "0123456789abcdef0123456789abcdef" }); [$big, $big, $big, $big, $big, $big, $big, $big, $big, $big])"#,
        ),
        (
            "one value aliased into a nested array",
            r#"($row := $map([1..30000], function($x) { "0123456789abcdef0123456789abcdef" }); [[$row, $row, $row], [$row, $row, $row], [$row, $row, $row]])"#,
        ),
    ];

    for (case, template) in cases {
        let (status, body) = test_mapping(&router, "JSON", r#"{"a":1}"#, template).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        // 5000000 = MAX_MAPPED_FILE_BYTES in `src/ops/test_mapping.rs`. As above, a
        // slower host may reach a time bound first, which is equally correct.
        assert!(
            body["Message"].as_str().is_some_and(|message| {
                message.contains("5000000") || message.contains("did not finish evaluating")
            }),
            "case {case:?}: the refusal must name the answer ceiling or the time bound, got {body}"
        );
    }

    let (status, body) = test_mapping(&router, "JSON", r#"{"a":"edi"}"#, "$uppercase(a)").await;
    assert_eq!(
        status, MAPPED_STATUS,
        "the emulator must still serve after a refused result, got {status} {body}"
    );
}

/// A result of 200 000 small numbers is INSIDE the answer ceiling, and is mapped: the
/// size walk charges each number the width it is written at rather than the width the
/// widest number would need, so a mapping that really fits is not refused by the clamp
/// that exists for the ones that do not.
#[tokio::test]
async fn a_large_result_that_fits_the_answer_ceiling_is_mapped() {
    let _heavy = HEAVY_EVALUATIONS.lock().await;
    let router = b2bi_router();

    let (status, body) = test_mapping(&router, "JSON", r#"{"a":1}"#, "[1..200000]").await;
    assert_eq!(
        status, MAPPED_STATUS,
        "a result inside the ceiling must be mapped, got {status}"
    );
    assert_eq!(
        mapped_document(&body).as_array().map(Vec::len),
        Some(200_000),
        "expected every mapped number"
    );
}

/// The string-building constructs an EDI mapping needs — `$string`, the `&` operator,
/// `$join`, `$replace` with a string pattern — are EVALUATED, and the three whose single
/// step this emulator cannot bound ahead of it — a regular-expression literal, `$distinct`
/// and `$pad` — are refused with the 501 that names them.
///
/// This is the suite behind "What the engine evaluates" in `src/ops/test_mapping.rs`. A
/// mapping template that cannot concatenate or serialize a value cannot express what an
/// EDI transform does with its output, so every row of the first table asserts the VALUE
/// the construct computes, in the positions a dispatch decision could get wrong — bound to
/// a variable, inside an object constructor, after a word-shaped operator (`and`, `or`,
/// `in`), through the chain operator — beside the shapes that must not be confused with
/// them: `/` as division, `/` inside a string, a comment, and a variable whose name merely
/// starts with a refused builtin's. The second table asserts the refusal in the same
/// spread of positions, because the decision is read off the engine's own parse rather
/// than off the template's text.
#[tokio::test]
async fn the_string_building_constructs_are_evaluated_and_the_unboundable_ones_refused() {
    let router = b2bi_router();

    let cases: &[MappingCase] = &[
        // The string-SERIALIZING builtins.
        (
            "serializing a number",
            "JSON",
            r#"{"a":42}"#,
            r#"$string(a)"#,
            r#""42""#,
        ),
        (
            "serializing a boolean through an alias",
            "JSON",
            r#"{"a":true}"#,
            r#"($s := $string; $s(a))"#,
            r#""true""#,
        ),
        (
            "concatenating a member and a literal",
            "JSON",
            r#"{"a":"abc"}"#,
            r#"a & "-suffix""#,
            r#""abc-suffix""#,
        ),
        (
            "concatenating two members",
            "JSON",
            r#"{"a":"abc"}"#,
            r#"a & "/" & a"#,
            r#""abc/abc""#,
        ),
        (
            "concatenating inside a constructor",
            "JSON",
            r#"{"a":"abc"}"#,
            r#"{"route": a & "-x"}"#,
            r#"{"route":"abc-x"}"#,
        ),
        (
            "joining an array",
            "JSON",
            r#"{"a":"abc"}"#,
            r#"$join([a, a], ",")"#,
            r#""abc,abc""#,
        ),
        (
            "replacing with a string pattern",
            "JSON",
            r#"{"a":"abc"}"#,
            r#"$replace(a, "a", "xx")"#,
            r#""xxbc""#,
        ),
        // The shapes that must not be confused with any of the above.
        (
            "division",
            "JSON",
            r#"{"items":[{"n":10},{"n":30}]}"#,
            "$sum(items.n) / 2",
            "20",
        ),
        (
            "division by a member",
            "JSON",
            r#"{"total":9,"count":3}"#,
            "total / count",
            "3",
        ),
        (
            "a slash inside a string",
            "JSON",
            r#"{"a":1}"#,
            r#"{"path": "in/out", "value": a}"#,
            r#"{"path":"in/out","value":1}"#,
        ),
        (
            "a comment",
            "JSON",
            r#"{"a":2}"#,
            "/* double it */ a * 2",
            "4",
        ),
        (
            "a variable whose name starts with pad",
            "JSON",
            r#"{"a":5}"#,
            "($padding := a; $padding)",
            "5",
        ),
        // The bounded string builtins, beside the serializing ones.
        (
            "uppercasing a member",
            "JSON",
            r#"{"a":"abc"}"#,
            "$uppercase(a)",
            r#""ABC""#,
        ),
        (
            "slicing a member",
            "JSON",
            r#"{"a":"abcdef"}"#,
            "$substring(a, 0, 3)",
            r#""abc""#,
        ),
        (
            "trimming a member",
            "JSON",
            r#"{"a":"  abc  "}"#,
            "$trim(a)",
            r#""abc""#,
        ),
        (
            "splitting on a string separator",
            "JSON",
            r#"{"a":"a,b"}"#,
            r#"$split(a, ",")"#,
            r#"["a","b"]"#,
        ),
        (
            "containing a string",
            "JSON",
            r#"{"a":"abc"}"#,
            r#"$contains(a, "b")"#,
            "true",
        ),
        (
            "a base64 round trip",
            "JSON",
            r#"{"a":"abc"}"#,
            "$base64decode($base64encode(a))",
            r#""abc""#,
        ),
        // A function-valued result: the serializer writes `""` for it, and the size walk
        // must charge it rather than measure it as a string.
        (
            "a function-valued result",
            "JSON",
            r#"{"a":1}"#,
            "function($x) { $x }",
            r#""""#,
        ),
    ];

    for (case, file_format, input, template, expected) in cases {
        let (status, body) = test_mapping(&router, file_format, input, template).await;
        assert_eq!(
            status, MAPPED_STATUS,
            "case {case:?}: expected {MAPPED_STATUS}, got {status} {body}"
        );
        let actual = mapped_document(&body);
        let expected: Value = serde_json::from_str(expected).expect("the expectation is JSON");
        assert!(
            same_json(&actual, &expected),
            "case {case:?}: expected {expected}, got {actual}"
        );
    }

    // The three constructs whose ONE step has no bound this emulator can place ahead of
    // it: a regular-expression literal and `$distinct` (no time bound, and a `std::thread`
    // cannot be cancelled) and `$pad` (a global-heap buffer sized by a number in the
    // template). Each is refused on the engine's own parse, in every position a template
    // can put it, with the 501 that names it.
    //
    // This table is also the guard on HOW that decision is made: the handler reads the
    // engine's own parse through its `Debug` rendering, which is not a stability contract,
    // so an interpreter upgrade that reshapes one of those nodes turns a row here from the
    // 501 into a mapped 200 and fails the build instead of silently evaluating the step
    // the handler says it cannot bound.
    let refused: &[(&str, &str, &str)] = &[
        (
            "a regex through $match",
            r#"{"a":"abc"}"#,
            r#"$match(a, /^a/)"#,
        ),
        (
            "a regex through $replace",
            r#"{"a":"abc"}"#,
            r#"$replace(a, /b/, "Z")"#,
        ),
        (
            "a regex through $split",
            r#"{"a":"a,b"}"#,
            r#"$split(a, /,/)"#,
        ),
        (
            "a regex through $contains",
            r#"{"a":"abc"}"#,
            r#"$contains(a, /b/)"#,
        ),
        (
            "a regex in an object constructor",
            r#"{"a":"abc"}"#,
            r#"{"m": $match(a, /b/).match}"#,
        ),
        (
            "a regex after and",
            r#"{"a":"aa"}"#,
            r#"true and $exists($match(a, /^(a+)+$/))"#,
        ),
        (
            "a regex after or",
            r#"{"a":"aa"}"#,
            r#"false or $exists($match(a, /^(a+)+$/))"#,
        ),
        (
            "a regex after in",
            r#"{"a":"aa"}"#,
            r#""aa" in [$match(a, /^(a+)+$/).match]"#,
        ),
        (
            "a regex bound to a variable",
            r#"{"a":"aaaa"}"#,
            r#"($re := /^(a+)+$/; $exists($match(a, $re)))"#,
        ),
        (
            "a regex through the chain operator",
            r#"{"a":"aaaa"}"#,
            r#"$count(a ~> $match(/a/))"#,
        ),
        ("padding to a width", r#"{"a":"x"}"#, r#"$pad(a, 4, "-")"#),
        (
            "padding inside a constructor",
            r#"{"a":"x"}"#,
            r#"{"p": $pad(a, 3, "0")}"#,
        ),
        (
            "padding through a variable",
            r#"{"a":"x"}"#,
            r#"($p := $pad; $p(a, 3, "0"))"#,
        ),
        (
            "de-duplicating an array",
            r#"{"a":"abc"}"#,
            r#"$count($distinct([a, a]))"#,
        ),
        (
            "de-duplicating a mapped path",
            r#"{"items":[{"sku":"a"},{"sku":"a"}]}"#,
            r#"$distinct(items.sku)"#,
        ),
        (
            "de-duplicating through a variable",
            r#"{"a":"abc"}"#,
            r#"($d := $distinct; $d([a, a]))"#,
        ),
    ];

    for (case, input, template) in refused {
        let (status, body) = test_mapping(&router, "JSON", input, template).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected the 501 naming the construct, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotImplemented",
            "case {case:?}: expected NotImplemented, got {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("emulator")),
            "case {case:?}: the refusal must name this emulator, got {body}"
        );
    }
}
