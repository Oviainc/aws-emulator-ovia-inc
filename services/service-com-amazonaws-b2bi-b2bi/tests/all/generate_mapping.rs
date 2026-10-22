// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GenerateMapping`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.GenerateMapping`) through the same router the emulator boots.
//!
//! RULE under test: `GenerateMapping` answers with a mapping template in the requested
//! language that is the ALIGNMENT of the two sample documents — every leaf of the target
//! document whose value also occurs in the source document becomes a reference to the
//! path it occurs at, every other leaf stays a literal — and `mappingAccuracy` is the
//! fraction of the target document's value-carrying leaves the template really sources
//! from the input, inside the modelled `Float range 0.0..1.0`.
//!
//! Each test asserts that rule over a TABLE whose rows differ along what it generalises
//! over (document format, nesting, repetition, scalar type, attribute vs element vs text,
//! match vs no match), not over one recorded request; the invariants (a document mapped
//! onto itself is sourced entirely, a document sharing no value is sourced not at all)
//! are asserted by a seeded property test over generated documents.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// One harness per test crate, not one per suite: `onboarding_loop` already exposes these
// for its siblings in this same `tests/all.rs` target.
use crate::onboarding_loop::{b2bi_router, call, error_code};

/// Floating-point slack when comparing `mappingAccuracy` against the fraction a row's
/// table computes by hand: the member is a `Float` (`f32`) on the wire.
const ACCURACY_EPSILON: f32 = 1e-6;


/// Call `GenerateMapping` and return `(status, body)`.
async fn generate_mapping(
    router: &MultiServiceRouter,
    mapping_type: &str,
    input_file_content: &str,
    output_file_content: &str,
) -> (u16, Value) {
    call(
        router,
        "GenerateMapping",
        json!({
            "mappingType": mapping_type,
            "inputFileContent": input_file_content,
            "outputFileContent": output_file_content
        }),
    )
    .await
}

/// The `message` of an error body. The awsJson1_0 envelope this service's modelled
/// `ValidationException` renders into names it `Message`; older SDKs read `message`, so
/// either spelling counts.
fn error_message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The `mappingTemplate` of a success body.
fn template(body: &Value) -> String {
    body["mappingTemplate"]
        .as_str()
        .unwrap_or("<no mappingTemplate>")
        .to_string()
}

/// The `mappingAccuracy` of a success body, which the model requires to sit in `0.0..1.0`.
fn accuracy(case: &str, body: &Value) -> f32 {
    let reported = body["mappingAccuracy"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: no mappingAccuracy in {body}")) as f32;
    assert!(
        (0.0..=1.0).contains(&reported),
        "case {case:?}: mappingAccuracy must sit in the modelled 0.0..1.0 range, got {reported}"
    );
    reported
}

/// Is `document` well-formed XML? Checked with the same reader the emulator parses the
/// samples with, so a generated stylesheet that would not parse fails the test.
fn xml_is_well_formed(document: &str) -> Result<(), String> {
    let mut reader = quick_xml::Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut depth = 0i32;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(_)) => depth += 1,
            Ok(quick_xml::events::Event::End(_)) => {
                depth -= 1;
                if depth < 0 {
                    return Err("an end tag closes no open element".to_string());
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    if depth != 0 {
        return Err(format!("{depth} element(s) left open"));
    }
    Ok(())
}

/// One row: the case name, the sample input, the sample output, the fraction of the
/// target's value-carrying leaves that the input can supply, the fragments the template
/// must carry, and the fragments it must not.
type Row<'a> = (
    &'a str,
    &'a str,
    &'a str,
    f32,
    &'a [&'a str],
    &'a [&'a str],
);

/// Assert one table of rows for `mapping_type`. An XSLT template is additionally asserted
/// to be well-formed XML — a stylesheet that does not parse is not a stylesheet.
async fn assert_rows(mapping_type: &str, rows: &[Row<'_>]) {
    let router = b2bi_router();
    for (case, input, output, expected_accuracy, must_contain, must_not_contain) in rows {
        let (status, body) = generate_mapping(&router, mapping_type, input, output).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GenerateMapping must answer 200, got {status} {body}"
        );
        let rendered = template(&body);
        assert!(
            !rendered.is_empty(),
            "case {case:?}: mappingTemplate must not be empty, got {rendered:?}"
        );
        if mapping_type == "XSLT" {
            let well_formed = xml_is_well_formed(&rendered);
            assert!(
                well_formed.is_ok(),
                "case {case:?}: the generated stylesheet is not well-formed XML \
                 ({well_formed:?}):\n{rendered}"
            );
        }
        for fragment in *must_contain {
            assert!(
                rendered.contains(fragment),
                "case {case:?}: expected the template to carry {fragment:?}, got:\n{rendered}"
            );
        }
        for fragment in *must_not_contain {
            assert!(
                !rendered.contains(fragment),
                "case {case:?}: expected the template NOT to carry {fragment:?}, got:\n{rendered}"
            );
        }
        let reported = accuracy(case, &body);
        assert!(
            (reported - expected_accuracy).abs() < ACCURACY_EPSILON,
            "case {case:?}: expected mappingAccuracy {expected_accuracy}, got {reported}"
        );
    }
}

/// RULE: for `mappingType` `JSONATA` the template is a JSONata expression constructing the
/// target document, whose leaves are JSONata paths into the input document wherever the
/// input holds the target's value and literals otherwise.
///
/// The rows span a flat document, nesting, arrays (including a root-level array), a key
/// that JSONata must backtick-quote, every JSON scalar type, a value the input holds under
/// two names, a target that shares nothing with the input, and a target with no value at
/// all in a leaf.
#[tokio::test]
async fn jsonata_template_sources_each_target_leaf_the_input_document_holds() {
    let rows: &[Row<'_>] = &[
        (
            "flat, every leaf sourced",
            r#"{"a":"X","b":"Y"}"#,
            r#"{"x":"X","y":"Y"}"#,
            1.0,
            &["\"x\": $.a", "\"y\": $.b"],
            &["\"X\"", "\"Y\""],
        ),
        (
            "nested objects and an array of objects",
            r#"{"header":{"id":"ST-1","count":2},"items":[{"sku":"A1","qty":3},{"sku":"B2","qty":4}]}"#,
            r#"{"transactionId":"ST-1","lines":[{"product":"A1","quantity":3},{"product":"B2","quantity":4}]}"#,
            1.0,
            &[
                "\"transactionId\": $.header.id",
                "\"product\": $.items[0].sku",
                "\"quantity\": $.items[1].qty",
            ],
            &["\"A1\"", "\"ST-1\""],
        ),
        (
            "root-level array",
            r#"[{"a":"X"}]"#,
            r#"[{"b":"X"}]"#,
            1.0,
            &["\"b\": $[0].a"],
            &["\"X\""],
        ),
        (
            "a key JSONata must quote",
            r#"{"ST-01":"880"}"#,
            r#"{"id":"880"}"#,
            1.0,
            &["\"id\": $.`ST-01`"],
            &["\"880\""],
        ),
        (
            "a key holding JSONata's variable sigil is quoted too",
            r#"{"x$y":"V"}"#,
            r#"{"id":"V"}"#,
            1.0,
            &["\"id\": $.`x$y`"],
            &["$.x$y"],
        ),
        (
            "numbers and booleans keep their type",
            r#"{"flag":true,"count":7}"#,
            r#"{"enabled":true,"total":7}"#,
            1.0,
            &["\"enabled\": $.flag", "\"total\": $.count"],
            &["true,", "\"7\""],
        ),
        (
            "a value under two names resolves to the matching name",
            r#"{"amount":"100","total":"100"}"#,
            r#"{"total":"100"}"#,
            1.0,
            &["\"total\": $.total"],
            &["$.amount"],
        ),
        (
            "nothing in common stays literal",
            r#"{"a":"X"}"#,
            r#"{"z":"Q","n":7}"#,
            0.0,
            &["\"z\": \"Q\"", "\"n\": 7"],
            &["$.a"],
        ),
        (
            "half the leaves sourced",
            r#"{"a":"X"}"#,
            r#"{"one":"X","two":"W"}"#,
            0.5,
            &["\"one\": $.a", "\"two\": \"W\""],
            &[],
        ),
        (
            "a leaf with no value is no mapping opportunity",
            r#"{"a":"X"}"#,
            r#"{"a":"X","empty":"","nothing":null}"#,
            1.0,
            &["\"a\": $.a", "\"empty\": \"\"", "\"nothing\": null"],
            &[],
        ),
        (
            "an empty object and an empty array round-trip as themselves",
            r#"{"a":"X"}"#,
            r#"{"obj":{},"arr":[],"a":"X"}"#,
            1.0,
            &["\"obj\": {}", "\"arr\": []", "\"a\": $.a"],
            &[],
        ),
    ];

    assert_rows("JSONATA", rows).await;
}

/// RULE: for `mappingType` `XSLT` the template is an XSLT 1.0 stylesheet building the
/// target document, whose values are `xsl:value-of` selections of XPaths into the input
/// document wherever the input holds the target's value and literal content otherwise.
///
/// The rows span elements, attributes, repeated siblings (which XPath positions 1-based),
/// mixed literal and selected values, XML metacharacters that have to be escaped, an
/// empty element, and a namespace-prefixed input document whose declaration has to reach
/// the stylesheet for its XPaths to resolve.
#[tokio::test]
async fn xslt_template_selects_each_target_value_the_input_document_holds() {
    let rows: &[Row<'_>] = &[
        (
            "elements only",
            "<Parsed><Id>ST-1</Id><Qty>3</Qty></Parsed>",
            "<Order><Number>ST-1</Number><Quantity>3</Quantity></Order>",
            1.0,
            &[
                "<xsl:stylesheet version=\"1.0\"",
                "<Number><xsl:value-of select=\"/Parsed/Id\"/></Number>",
                "<Quantity><xsl:value-of select=\"/Parsed/Qty\"/></Quantity>",
            ],
            &["<Number>ST-1</Number>"],
        ),
        (
            "attributes and repeated siblings",
            "<Doc><Line sku=\"A1\">3</Line><Line sku=\"B2\">4</Line></Doc>",
            "<Shipment><Item id=\"A1\">3</Item><Item id=\"B2\">4</Item></Shipment>",
            1.0,
            &[
                "<xsl:attribute name=\"id\"><xsl:value-of select=\"/Doc/Line[1]/@sku\"/></xsl:attribute>",
                "<xsl:value-of select=\"/Doc/Line[2]/text()\"/>",
            ],
            &["id=\"A1\""],
        ),
        (
            "a value the input lacks stays literal, escaped",
            "<A><B>X</B></A>",
            "<Out><Mapped>X</Mapped><Fixed>a &amp; b &lt; c</Fixed></Out>",
            0.5,
            &[
                "<Mapped><xsl:value-of select=\"/A/B\"/></Mapped>",
                "<Fixed>a &amp; b &lt; c</Fixed>",
            ],
            &[],
        ),
        (
            "an empty element carries no value to map",
            "<ns:Doc xmlns:ns=\"urn:example\"><ns:Id>7</ns:Id></ns:Doc>",
            "<Out><Id>7</Id><Note/></Out>",
            1.0,
            &[
                "xmlns:ns=\"urn:example\"",
                "<Id><xsl:value-of select=\"/ns:Doc/ns:Id\"/></Id>",
                "<Note/>",
            ],
            &[],
        ),
        (
            "an attribute the input lacks stays a literal attribute",
            "<A><B>X</B></A>",
            "<Out kind=\"837\"><Mapped>X</Mapped></Out>",
            0.5,
            &["<Out kind=\"837\">", "select=\"/A/B\""],
            &["<xsl:attribute name=\"kind\">"],
        ),
        (
            "a prefix declared below the source root still reaches the stylesheet",
            "<Doc><p:Id xmlns:p=\"urn:deep\">9</p:Id></Doc>",
            "<Out><Id>9</Id></Out>",
            1.0,
            &[
                "xmlns:p=\"urn:deep\"",
                "<xsl:value-of select=\"/Doc/p:Id/text()\"/>",
            ],
            &[],
        ),
        (
            "a source redeclaring the xsl prefix cannot redeclare it on the stylesheet",
            "<Doc xmlns:xsl=\"urn:not-xslt\"><Id>5</Id></Doc>",
            "<Out><Id>5</Id></Out>",
            1.0,
            &[
                "xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\"",
                "select=\"/Doc/Id\"",
            ],
            &["urn:not-xslt"],
        ),
        (
            "a target redeclaring the xsl prefix cannot rebind it on a result element",
            "<A><B>X</B></A>",
            "<Out xmlns:xsl=\"urn:target-xsl\"><Mapped>X</Mapped></Out>",
            1.0,
            &["<Mapped><xsl:value-of select=\"/A/B\"/></Mapped>"],
            &["urn:target-xsl"],
        ),
    ];

    assert_rows("XSLT", rows).await;
}

/// RULE: a JSONata template is an expression over JSON and an XSLT stylesheet a transform
/// over XML, so a sample document that is not in the format the requested `mappingType`
/// works over — or is not a JSON or XML document at all — is refused with the modelled
/// `ValidationException` (HTTP 400) naming the member, never with a 501 or an empty 200.
///
/// The rows span both members, both mapping types, a document in the other format, a
/// document in neither, and a malformed document of each format.
#[tokio::test]
async fn a_sample_document_the_mapping_type_cannot_consume_is_refused() {
    // (case, mappingType, inputFileContent, outputFileContent, the member named)
    let rows: &[(&str, &str, &str, &str, &str)] = &[
        (
            "JSONATA over an XML input",
            "JSONATA",
            "<a>1</a>",
            r#"{"a":1}"#,
            "inputFileContent",
        ),
        (
            "XSLT over a JSON output",
            "XSLT",
            "<a>1</a>",
            r#"{"a":1}"#,
            "outputFileContent",
        ),
        (
            "raw EDI is neither JSON nor XML",
            "JSONATA",
            "ISA*00*          *00*          *ZZ*SENDER",
            r#"{"a":1}"#,
            "inputFileContent",
        ),
        (
            "malformed JSON",
            "JSONATA",
            r#"{"a":"#,
            r#"{"a":1}"#,
            "inputFileContent",
        ),
        (
            "malformed XML",
            "XSLT",
            "<a><b></a>",
            "<a>1</a>",
            "inputFileContent",
        ),
        (
            "an XML document with two roots",
            "XSLT",
            "<a>1</a><b>2</b>",
            "<a>1</a>",
            "inputFileContent",
        ),
        (
            "a name with two colons is not a QName",
            "XSLT",
            "<a:b:c>1</a:b:c>",
            "<a>1</a>",
            "inputFileContent",
        ),
        (
            "a target element in the prefix XSLT reserves for itself",
            "XSLT",
            "<A><B>X</B></A>",
            "<Out><xsl:value-of>X</xsl:value-of></Out>",
            "outputFileContent",
        ),
        (
            "a target binding the XSLT namespace under another prefix",
            "XSLT",
            "<A><B>X</B></A>",
            "<Out xmlns:q=\"http://www.w3.org/1999/XSL/Transform\"><q:value-of>X</q:value-of></Out>",
            "outputFileContent",
        ),
    ];

    let router = b2bi_router();
    for (case, mapping_type, input, output, member) in rows {
        let (status, body) = generate_mapping(&router, mapping_type, input, output).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(member),
            "case {case:?}: expected the refusal to name {member:?}, got {message:?}"
        );
    }
}

/// A JSON document of `levels` nested one-member objects around a single value.
fn nested_json(levels: usize) -> String {
    let mut document = "\"V\"".to_string();
    for _ in 0..levels {
        document = format!("{{\"a\":{document}}}");
    }
    document
}

/// A JSON array of `count` distinct string values.
fn wide_json(count: usize) -> String {
    let values: Vec<String> = (0..count).map(|n| format!("\"VALUE-{n:06}\"")).collect();
    format!("[{}]", values.join(","))
}

/// A JSON array of `count` EMPTY arrays: containers carrying no value at all.
fn container_only_json(count: usize) -> String {
    format!("[{}]", vec!["[]"; count].join(","))
}

/// A JSON object of `count` members whose names are `name_len` characters long.
fn long_named_json(count: usize, name_len: usize) -> String {
    let members: Vec<String> = (0..count)
        .map(|n| {
            let key = format!("{:0>width$}", format!("K{n}"), width = name_len);
            format!("\"{key}\":\"VALUE-{n:06}\"")
        })
        .collect();
    format!("{{{}}}", members.join(","))
}

/// An XML document whose root declares `prefix_count` distinct namespace prefixes.
fn prefixed_xml(prefix_count: usize) -> String {
    let declarations: Vec<String> = (0..prefix_count)
        .map(|n| format!("xmlns:p{n}=\"urn:p{n}\""))
        .collect();
    format!("<Doc {}><Id>7</Id></Doc>", declarations.join(" "))
}

/// RULE: the bounds this emulator adds beyond the model — the deepest nesting
/// (`MAX_DOCUMENT_DEPTH` = 64, the same count for JSON and XML), the most nodes, values
/// AND containers alike (`MAX_DOCUMENT_NODES` = 50 000), the longest single value path and
/// the most path text it will index (`MAX_PATH_BYTES` = 1024,
/// `MAX_INDEXED_PATH_BYTES` = 350 000), the most namespace prefixes it carries onto a
/// stylesheet (`MAX_SOURCE_NAMESPACES` = 64) and the longest template it will answer with
/// (`MAX_TEMPLATE_CHARS` = 350 000, the modelled `MappingTemplate len 0..350000`) — each
/// refuse with the same modelled `ValidationException` naming the member, so one request
/// cannot drive unbounded work and no bound is answered by a silently degraded template; a
/// document just INSIDE each bound still answers 200.
///
/// The rows sit on each published bound and one step past it, and cover the
/// container-only document that carries no value at all.
#[tokio::test]
async fn a_sample_document_past_this_emulator_s_own_bounds_is_refused() {
    let tiny = r#"{"a":"V"}"#.to_string();
    let tiny_xml = "<Out><Id>7</Id></Out>".to_string();
    // (case, mappingType, inputFileContent, outputFileContent, expected status, the member)
    let rows: &[(&str, &str, String, String, u16, &str)] = &[
        (
            "nesting at the depth bound",
            "JSONATA",
            nested_json(63),
            tiny.clone(),
            200,
            "",
        ),
        (
            "nesting one level past the depth bound",
            "JSONATA",
            nested_json(64),
            tiny.clone(),
            400,
            "inputFileContent",
        ),
        (
            "XML nesting at the same depth bound",
            "XSLT",
            format!("{}{}", "<e>".repeat(63), "</e>".repeat(63)),
            tiny_xml.clone(),
            200,
            "",
        ),
        (
            "XML nesting at the last accepted level",
            "XSLT",
            format!("{}{}", "<e>".repeat(64), "</e>".repeat(64)),
            tiny_xml.clone(),
            200,
            "",
        ),
        (
            "XML nesting one level past the depth bound",
            "XSLT",
            format!("{}{}", "<e>".repeat(65), "</e>".repeat(65)),
            tiny_xml.clone(),
            400,
            "inputFileContent",
        ),
        (
            "namespace prefixes at the bound",
            "XSLT",
            prefixed_xml(64),
            tiny_xml.clone(),
            200,
            "",
        ),
        (
            "one namespace prefix past the bound",
            "XSLT",
            prefixed_xml(65),
            tiny_xml.clone(),
            400,
            "inputFileContent",
        ),
        (
            "a wide document inside the node bound",
            "JSONATA",
            wide_json(30_000),
            tiny.clone(),
            200,
            "",
        ),
        (
            "a wide document past the node bound",
            "JSONATA",
            wide_json(60_000),
            tiny.clone(),
            400,
            "inputFileContent",
        ),
        (
            "a container-only document past the node bound carries no value, and is still bounded",
            "JSONATA",
            container_only_json(60_000),
            tiny.clone(),
            400,
            "inputFileContent",
        ),
        (
            "a member name inside the single-path bound",
            "JSONATA",
            long_named_json(1, 900),
            tiny.clone(),
            200,
            "",
        ),
        (
            "a member name past the single-path bound",
            "JSONATA",
            long_named_json(1, 1100),
            tiny.clone(),
            400,
            "inputFileContent",
        ),
        (
            "a document whose paths together pass the path text this emulator indexes",
            "JSONATA",
            long_named_json(20_000, 22),
            tiny.clone(),
            400,
            "inputFileContent",
        ),
        (
            "a target whose template would pass the length a mapping template may carry",
            "JSONATA",
            tiny.clone(),
            wide_json(40_000),
            400,
            "outputFileContent",
        ),
    ];

    let router = b2bi_router();
    for (case, mapping_type, input, output, expected_status, member) in rows {
        let (status, body) = generate_mapping(&router, mapping_type, input, output).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {}",
            error_message(&body)
        );
        if *expected_status != 400 {
            continue;
        }
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(member),
            "case {case:?}: expected the refusal to name {member:?}, got {message:?}"
        );
    }
}

/// The seed the generated documents below come from. Fixed, so a failure is reproducible;
/// it is named in every assertion message.
const PROPERTY_SEED: u64 = 0x5EED_B2B1;

/// How many generated documents the property test walks.
const PROPERTY_CASES: usize = 24;

/// Keys a generated document uses: plain identifiers plus the kinds of name a JSONata
/// path has to quote (a dash, a space, a `$`).
const GENERATED_KEYS: [&str; 7] = ["id", "amount", "items", "nested", "ST-01", "a b", "x$y"];

/// The seeded generator the property test draws its documents from: SplitMix64, the
/// mixing function `rand`'s own `SeedableRng::seed_from_u64` uses to expand a seed.
///
/// Written here rather than taken from a generator crate ON PURPOSE: a dev-dependency
/// would change this crate's `Cargo.toml` and `Cargo.lock`, the two files every other fix
/// also touches, and this fix has to land against a mainline that moves between submits.
/// Eight lines of arithmetic, nothing on disk, and the seed is a constant every assertion
/// message names.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..bound` — every bound here is tiny, so the modulo bias cannot affect
    /// whether the invariant under test holds.
    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

/// A generated JSON document: an object root (what a parsed EDI document is), with
/// objects and arrays down to `depth` and non-empty scalar leaves, so every leaf is a
/// mapping opportunity.
fn generated_document(rng: &mut SplitMix64, depth: usize, counter: &mut u32) -> Value {
    let members = 1 + rng.below(3);
    let mut root = serde_json::Map::new();
    for _ in 0..members {
        let key = GENERATED_KEYS[rng.below(GENERATED_KEYS.len() as u64) as usize];
        root.insert(key.to_string(), generated_value(rng, depth, counter));
    }
    Value::Object(root)
}

/// One value of a generated document: a scalar, an array or an object.
fn generated_value(rng: &mut SplitMix64, depth: usize, counter: &mut u32) -> Value {
    if depth == 0 || rng.below(10) < 3 {
        *counter += 1;
        return match rng.below(3) {
            0 => Value::String(format!("V{counter}")),
            1 => json!(i64::from(*counter) * 7),
            _ => json!((*counter).is_multiple_of(2)),
        };
    }
    if rng.below(10) < 3 {
        let items = 1 + rng.below(3);
        return Value::Array(
            (0..items)
                .map(|_| generated_value(rng, depth - 1, counter))
                .collect(),
        );
    }
    let members = 1 + rng.below(3);
    let mut object = serde_json::Map::new();
    for _ in 0..members {
        let key = GENERATED_KEYS[rng.below(GENERATED_KEYS.len() as u64) as usize];
        object.insert(key.to_string(), generated_value(rng, depth - 1, counter));
    }
    Value::Object(object)
}

/// Every leaf of `document`, with its value replaced by one no generated document holds.
fn with_unrelated_values(document: &Value) -> Value {
    match document {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, value)| (key.clone(), with_unrelated_values(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(with_unrelated_values).collect()),
        other => Value::String(format!("ZZ-{other}")),
    }
}

/// RULE, as the two invariants it implies: a document mapped onto ITSELF is sourced
/// entirely (`mappingAccuracy` 1.0, since every target value occurs in the input), and a
/// document sharing no value with the input is sourced not at all (0.0) — and the same
/// request always answers with the same template, because the operation computes it rather
/// than persisting or sampling anything.
#[tokio::test]
async fn a_document_mapped_onto_itself_is_sourced_entirely_and_a_disjoint_one_not_at_all() {
    let router = b2bi_router();
    let mut rng = SplitMix64(PROPERTY_SEED);

    for case in 0..PROPERTY_CASES {
        let mut counter = 0u32;
        let document = generated_document(&mut rng, 4, &mut counter);
        // A document with no leaf at all offers nothing to source; the generator can
        // produce one (an empty object is not reachable, but a shallow array is), so skip
        // it rather than assert a fraction of nothing.
        if counter == 0 {
            continue;
        }
        let same = serde_json::to_string(&document).expect("the document serializes");
        let unrelated = serde_json::to_string(&with_unrelated_values(&document))
            .expect("the document serializes");

        let (status, body) = generate_mapping(&router, "JSONATA", &same, &same).await;
        assert_eq!(
            status, 200,
            "seed {PROPERTY_SEED:#x} case {case}: expected 200 for {same}, got {status} {body}"
        );
        let identical = accuracy(&format!("seed {PROPERTY_SEED:#x} case {case} (itself)"), &body);
        assert!(
            (identical - 1.0).abs() < ACCURACY_EPSILON,
            "seed {PROPERTY_SEED:#x} case {case}: a document mapped onto itself must report \
             accuracy 1.0, got {identical} for {same}"
        );

        // The same request again: same template, so nothing was persisted or sampled.
        let (_, repeat) = generate_mapping(&router, "JSONATA", &same, &same).await;
        assert_eq!(
            template(&repeat),
            template(&body),
            "seed {PROPERTY_SEED:#x} case {case}: two identical calls must answer with the \
             same template for {same}"
        );

        let (status, body) = generate_mapping(&router, "JSONATA", &same, &unrelated).await;
        assert_eq!(
            status, 200,
            "seed {PROPERTY_SEED:#x} case {case}: expected 200 for {unrelated}, got {status} {body}"
        );
        let disjoint = accuracy(
            &format!("seed {PROPERTY_SEED:#x} case {case} (disjoint)"),
            &body,
        );
        assert!(
            disjoint.abs() < ACCURACY_EPSILON,
            "seed {PROPERTY_SEED:#x} case {case}: a target sharing no value with the input \
             must report accuracy 0.0, got {disjoint} for input {same} and target {unrelated}"
        );
    }
}
