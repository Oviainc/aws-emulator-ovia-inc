// SPDX-License-Identifier: CC0-1.0

//! The sensitive-information check over telephone numbers, including the local
//! subscriber form.
//!
//! The rule asserted here: the check reports one detection per entity type the
//! request named and the recognizer PLACED, addressed to the content block the value
//! was found in — so a telephone number written in any of the forms the North
//! American Numbering Plan publishes is a `PHONE` detection, and a digit run that is
//! not one of those forms is not.
//!
//! The NANP's own rule for the subscriber form is what the rows span: a central
//! office code (NXX — first digit 2-9) and a four-digit line number, with and
//! without the area code the three-part form carries, on each side of that first
//! digit's published bound, and with the separators the forms are written with. The
//! recognizer behind it is the one Comprehend's PII family is served from, whose own
//! suite asserts the same forms as spans; here the rule is asserted end to end
//! through `InvokeGuardrailChecks`, where a missed number is a safety check reporting
//! "nothing found". Every assertion names its case and the actual value.

use serde_json::{json, Value};

use crate::bedrock_runtime_inference::{emulator, post_json};

/// The URI `InvokeGuardrailChecks` is bound to
/// (`smithy_query(query="operation",operation="InvokeGuardrailChecks")`).
const PATH: &str = "/guardrail-checks/invoke";

/// One row: a telephone-number candidate, the text around it, and whether the
/// recognizer must place a `PHONE` at it.
struct NumberRow {
    case: &'static str,
    /// The text before the candidate, which is also its expected `beginOffset` in
    /// characters when the row is a telephone number.
    prefix: &'static str,
    /// The candidate exactly as the block carries it.
    number: &'static str,
    suffix: &'static str,
    is_phone: bool,
}

const NUMBER_ROWS: &[NumberRow] = &[
    NumberRow {
        case: "a local subscriber number",
        prefix: "Reach the front desk on ",
        number: "555-0142",
        suffix: " before noon.",
        is_phone: true,
    },
    NumberRow {
        case: "a local subscriber number at the lowest published office code",
        prefix: "The night line is ",
        number: "200-0199",
        suffix: " until Friday.",
        is_phone: true,
    },
    NumberRow {
        case: "a local subscriber number at the highest published office code",
        prefix: "The night line is ",
        number: "999-4477",
        suffix: " until Friday.",
        is_phone: true,
    },
    NumberRow {
        case: "a local subscriber number written with a dot",
        prefix: "Dial ",
        number: "555.0188",
        suffix: " for the warehouse.",
        is_phone: true,
    },
    NumberRow {
        case: "a local subscriber number the text names as a telephone number",
        prefix: "My phone is ",
        number: "555 0170",
        suffix: " most afternoons.",
        is_phone: true,
    },
    NumberRow {
        case: "a three-part number carrying its area code",
        prefix: "Call ",
        number: "415-555-0123",
        suffix: " in the morning.",
        is_phone: true,
    },
    NumberRow {
        case: "an international number in its E.164 form",
        prefix: "Try ",
        number: "+14155550123",
        suffix: " out of hours.",
        is_phone: true,
    },
    // One past the published bound on the office code's first digit: N is 2-9, so a
    // run beginning 0 or 1 is not a subscriber number however it is written.
    NumberRow {
        case: "a digit run whose office code starts past the published bound",
        prefix: "Order ",
        number: "112-0100",
        suffix: " shipped today.",
        is_phone: false,
    },
    NumberRow {
        case: "a digit run whose office code starts at zero",
        prefix: "Order ",
        number: "012-3456",
        suffix: " shipped today.",
        is_phone: false,
    },
    NumberRow {
        case: "a five-digit postal code with its add-on",
        prefix: "The depot is in ",
        number: "98101-1234",
        suffix: " near the water.",
        is_phone: false,
    },
];

/// The detections the sensitive-information check reports for one conversation,
/// with every finding's shared invariants checked.
async fn detections(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    messages: &Value,
    entity_types: &[&str],
) -> Vec<Value> {
    let entities: Value = Value::Array(
        entity_types
            .iter()
            .map(|entity_type| json!({ "type": entity_type }))
            .collect(),
    );
    let body = json!({
        "messages": messages,
        "checks": { "sensitiveInformation": { "entities": entities } },
    });
    let call = post_json(router, PATH, &body).await;
    assert_eq!(
        call.status,
        200,
        "case {case:?}: expected 200, got {} / {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    let response = call.json();
    let result = &response["results"]["sensitiveInformation"];
    assert_eq!(
        result["truncated"],
        json!(false),
        "case {case:?}: expected an untruncated result, got {result}"
    );
    let findings = result["results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a findings list, got {response}"))
        .clone();
    for finding in &findings {
        let confidence = finding["confidenceScore"].as_f64().unwrap_or(-1.0);
        assert!(
            confidence > 0.0 && confidence <= 1.0,
            "case {case:?}: expected a confidenceScore in (0, 1], got {confidence} for \
             {finding}"
        );
    }
    findings
}

/// The characters of `text` from `begin` up to (not including) `end`.
fn characters(text: &str, begin: usize, end: usize) -> String {
    text.chars().skip(begin).take(end - begin).collect()
}

/// The rule over the table: a telephone number in any published form is placed at
/// its own offsets in the block that carried it, and a digit run that is not one is
/// not reported at all.
#[tokio::test]
async fn the_check_places_a_telephone_number_in_every_published_form() {
    let router = emulator();

    for row in NUMBER_ROWS {
        let case = row.case;
        let carrier = format!("{}{}{}", row.prefix, row.number, row.suffix);
        // The candidate sits in the SECOND block of the FIRST message, so a handler
        // that reported zeros, or counted either index from one, fails here.
        let messages = json!([
            { "role": "user", "content": [{ "text": "Booking confirmed." }, { "text": carrier }] },
            { "role": "assistant", "content": [{ "text": "Noted." }] },
        ]);
        let findings = detections(&router, case, &messages, &["PHONE"]).await;

        if !row.is_phone {
            assert!(
                findings.is_empty(),
                "case {case:?}: {:?} is not a telephone number, so no PHONE may be \
                 reported, got {findings:?}",
                row.number
            );
            continue;
        }

        assert_eq!(
            findings.len(),
            1,
            "case {case:?}: expected exactly one PHONE detection for {:?}, got {findings:?}",
            row.number
        );
        let finding = &findings[0];
        assert_eq!(
            finding["type"],
            json!("PHONE"),
            "case {case:?}: expected a PHONE detection, got {finding}"
        );
        let begin = finding["beginOffset"].as_u64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a beginOffset, got {finding}")
        }) as usize;
        let end = finding["endOffset"]
            .as_u64()
            .unwrap_or_else(|| panic!("case {case:?}: expected an endOffset, got {finding}"))
            as usize;
        assert_eq!(
            characters(&carrier, begin, end),
            row.number,
            "case {case:?}: expected the offsets {begin}..{end} to cut out {:?}, got {:?}",
            row.number,
            characters(&carrier, begin, end)
        );
        assert_eq!(
            finding["messageIndex"],
            json!(0),
            "case {case:?}: expected messageIndex 0, got {}",
            finding["messageIndex"]
        );
        assert_eq!(
            finding["contentIndex"],
            json!(1),
            "case {case:?}: expected contentIndex 1, got {}",
            finding["contentIndex"]
        );
    }
}

/// One detection per entity type the request named and the recognizer placed: a
/// block carrying two values of two named types is answered about both, and a type
/// whose value the block does not carry is answered about not at all.
#[tokio::test]
async fn a_block_carrying_two_named_entity_types_is_answered_about_both() {
    let router = emulator();
    let carrier = "My email is dana@example.org and my phone is 555-0100.";
    let messages = json!([
        { "role": "user", "content": [{ "text": "Booking confirmed." }, { "text": carrier }] },
        { "role": "assistant", "content": [{ "text": "Noted." }] },
    ]);

    // (case, the types the request names, the types it must be answered about)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "both types the block carries",
            vec!["EMAIL", "PHONE"],
            vec!["EMAIL", "PHONE"],
        ),
        ("the telephone type alone", vec!["PHONE"], vec!["PHONE"]),
        (
            "a named type the block does not carry",
            vec!["PHONE", "US_SOCIAL_SECURITY_NUMBER"],
            vec!["PHONE"],
        ),
    ];

    for (case, named, expected) in rows {
        let findings = detections(&router, case, &messages, &named).await;
        let mut reported: Vec<String> = findings
            .iter()
            .map(|finding| finding["type"].as_str().unwrap_or_default().to_string())
            .collect();
        reported.sort();
        let mut wanted: Vec<String> = expected.iter().map(|value| (*value).to_string()).collect();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected detections for exactly {wanted:?}, got {reported:?} \
             in {findings:?}"
        );

        for finding in &findings {
            let begin = finding["beginOffset"].as_u64().unwrap_or_default() as usize;
            let end = finding["endOffset"].as_u64().unwrap_or_default() as usize;
            let sliced = characters(carrier, begin, end);
            let expected_value = match finding["type"].as_str().unwrap_or_default() {
                "EMAIL" => "dana@example.org",
                _ => "555-0100",
            };
            assert_eq!(
                sliced, expected_value,
                "case {case:?}: expected the offsets {begin}..{end} of {finding} to cut \
                 out {expected_value:?}, got {sliced:?}"
            );
            assert_eq!(
                (
                    finding["messageIndex"].as_i64(),
                    finding["contentIndex"].as_i64()
                ),
                (Some(0), Some(1)),
                "case {case:?}: expected the block that carried the value to be named, \
                 got {finding}"
            );
        }
    }
}
