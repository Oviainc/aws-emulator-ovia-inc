// SPDX-License-Identifier: CC0-1.0

//! Comprehend's PII entity detection: `DetectPiiEntities`.
//!
//! The operation is a pure function of the submitted document, so these suites assert
//! the RULE over a TABLE of documents rather than over one recorded request: every
//! value the service's PII recognizer places in the text is answered as one
//! `PiiEntity` carrying the modelled `PiiEntityType` of that value's published form,
//! a confidence in (0, 1], and the zero-based CHARACTER offsets of the span
//! (`EndOffset` one past its last character), in document order — and a value whose
//! published check digit does not close is not answered at all.
//!
//! The recognizer itself is `ops::contains_pii_entities::detect_pii_spans`, the one
//! rule set of the PII family, and its rules are asserted type by type in
//! `pii_detection.rs`. What is asserted HERE is the projection this operation adds:
//! the offsets and the per-entity shape `ContainsPiiEntities` does not expose. That
//! the two projections agree about a document drives BOTH operations, so it lives
//! beside neither, in `pii_entities.rs`.
//!
//! The per-document byte quota this operation enforces is one row of the service's
//! shared quota table, so it is asserted there (`realtime_text_quotas.rs`) with every
//! other real-time inference operation rather than a second time here.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::PII_ENTITY_TYPE_VALUES;

use crate::comprehend_client::Comprehend;

/// `ALL` is the catalog's "every type" selector — the value a redaction job passes to
/// ask for all of them — not a type a document can carry, so no entity is ever
/// reported as it.
const CATALOG_SELECTOR: &str = "ALL";

/// Documents paired with the entities the rule requires, as
/// `(case, text, &[(Type, the exact entity text)])`, the entities in DOCUMENT ORDER.
///
/// The rows differ along what the rule generalises over: which `PiiEntityType` the
/// value belongs to, whether its form carries a published check digit (and whether
/// that check closes), whether the value has a form of its own or is admitted only
/// because the text names it (where the entity is the VALUE, not the naming words),
/// how many entities one document holds, whether two forms claim overlapping spans,
/// and whether the characters before the entity are one byte each.
///
/// Every `Type` is a value of the modelled `PiiEntityType` enum (asserted against
/// `PII_ENTITY_TYPE_VALUES` below); every form is the one its defining authority
/// publishes, as named on the rule that recognises it in
/// `ops::contains_pii_entities`. The issue cited only an email address and a
/// social-security number, so every other row is an entry it did not cite.
///
/// The structurally checked values are the published test vectors for their checks:
/// `4111 1111 1111 1111` (the ISO/IEC 7812-1 Luhn digit), `GB82WEST12345698765432`
/// (the ISO 13616 mod-97 example), `046 454 286` (Luhn) and `021000021` (an allocated
/// ABA routing transit number).
#[allow(clippy::type_complexity)]
pub(crate) const DOCUMENTS: &[(&str, &str, &[(&str, &str)])] = &[
    (
        "email address",
        "Write to jane.roe@example.com when the report is ready.",
        &[("EMAIL", "jane.roe@example.com")],
    ),
    (
        "email address after multi-byte characters",
        "Mi correo electr\u{f3}nico es juan@example.com.",
        &[("EMAIL", "juan@example.com")],
    ),
    (
        "social security number in its 3-2-4 grouping",
        "The form lists 123-45-6789 on the second page.",
        &[("SSN", "123-45-6789")],
    ),
    (
        "social security number written without separators, named by the text",
        "SSN: 123456789 is on file.",
        &[("SSN", "123456789")],
    ),
    (
        "nine bare digits with nothing naming them",
        "Reference 123456789 was filed.",
        &[],
    ),
    (
        "aba routing transit number",
        "routing number 021000021",
        &[("BANK_ROUTING", "021000021")],
    ),
    (
        "payment card number whose luhn check closes",
        "The charge went to 4111 1111 1111 1111 yesterday.",
        &[("CREDIT_DEBIT_NUMBER", "4111 1111 1111 1111")],
    ),
    (
        "payment card number whose luhn check does not close",
        "The charge went to 4111111111111112 yesterday.",
        &[],
    ),
    (
        "iban",
        "Send it to GB82WEST12345698765432 by Friday.",
        &[(
            "INTERNATIONAL_BANK_ACCOUNT_NUMBER",
            "GB82WEST12345698765432",
        )],
    ),
    (
        "canadian social insurance number",
        "social insurance number 046 454 286",
        &[("CA_SOCIAL_INSURANCE_NUMBER", "046 454 286")],
    ),
    (
        "ipv4 address",
        "The request came from 192.0.2.47 last night.",
        &[("IP_ADDRESS", "192.0.2.47")],
    ),
    (
        "ipv6 address",
        "The request came from 2001:db8::1 last night.",
        &[("IP_ADDRESS", "2001:db8::1")],
    ),
    (
        "mac address",
        "The laptop reports 00:1B:44:11:3A:B7 as its hardware address.",
        &[("MAC_ADDRESS", "00:1B:44:11:3A:B7")],
    ),
    (
        "absolute url",
        "The dashboard lives at https://example.com/profile for now.",
        &[("URL", "https://example.com/profile")],
    ),
    (
        "aws access key id",
        "The script signs with AKIAIOSFODNN7EXAMPLE today.",
        &[("AWS_ACCESS_KEY", "AKIAIOSFODNN7EXAMPLE")],
    ),
    (
        "indian permanent account number",
        "The filing shows ABCDE1234F on page two.",
        &[("IN_PERMANENT_ACCOUNT_NUMBER", "ABCDE1234F")],
    ),
    (
        "uk national insurance number",
        "The record shows AB123456C on file.",
        &[("UK_NATIONAL_INSURANCE_NUMBER", "AB123456C")],
    ),
    (
        "iso 8601 date",
        "The account opened on 2024-03-17 in the morning.",
        &[("DATE_TIME", "2024-03-17")],
    ),
    (
        // A personal name has no form of its own: the entity is the NAME, not the
        // courtesy phrase that introduced it.
        "personal name introduced by the text",
        "My name is Jane Roe.",
        &[("NAME", "Jane Roe")],
    ),
    (
        // Likewise a password: the entity is the secret, not the word "password".
        "password named by the text",
        "password: hunter2!",
        &[("PASSWORD", "hunter2!")],
    ),
    (
        "phone number with a country and area code, after multi-byte characters",
        "Tel\u{e9}fono: +1 (206) 555-0147 por la ma\u{f1}ana.",
        &[("PHONE", "+1 (206) 555-0147")],
    ),
    (
        "two entities, answered in document order",
        "Write to jane.roe@example.com or call 206-555-0147 today.",
        &[("EMAIL", "jane.roe@example.com"), ("PHONE", "206-555-0147")],
    ),
    (
        // The address and the host inside it are both recognisable forms; they
        // overlap, so only one entity can be reported and it is the longer span.
        "two forms claiming overlapping spans",
        "Write to ops@www.example.com now.",
        &[("EMAIL", "ops@www.example.com")],
    ),
    (
        "a document carrying no personally identifiable information",
        "The quarterly report is attached for your review.",
        &[],
    ),
    (
        "clean prose with numbers that are not identifiers",
        "We shipped 12 units and 7 spares to the warehouse.",
        &[],
    ),
];

/// The `(BeginOffset, EndOffset)` the modelled members require for `needle` inside
/// `haystack`: zero-based CHARACTER offsets, `EndOffset` one past the entity's last
/// character, so the characters in that range are exactly the entity.
pub(crate) fn expected_offsets(case: &str, haystack: &str, needle: &str) -> (i64, i64) {
    let byte_offset = haystack
        .find(needle)
        .unwrap_or_else(|| panic!("case {case:?}: {needle:?} is not in {haystack:?}"));
    let begin = haystack[..byte_offset].chars().count() as i64;
    (begin, begin + needle.chars().count() as i64)
}

/// The `Entities` array of a successful response.
pub(crate) fn entities(case: &str, output: &Value) -> Vec<Value> {
    output["Entities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Entities array, got {output}"))
        .clone()
}

#[tokio::test]
async fn detect_pii_entities_answers_each_recognised_value_with_its_character_offsets() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in DOCUMENTS {
        let output = comprehend
            .ok(
                "DetectPiiEntities",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;
        let found = entities(case, &output);

        assert_eq!(
            found.len(),
            expected.len(),
            "case {case:?}: expected {} entities ({expected:?}), got {output}",
            expected.len()
        );

        for (entity, (expected_type, entity_text)) in found.iter().zip(expected.iter()) {
            let reported_type = entity["Type"].as_str().unwrap_or_default();
            assert!(
                PII_ENTITY_TYPE_VALUES.contains(&reported_type),
                "case {case:?}: {reported_type:?} is not a modelled PiiEntityType value"
            );
            assert_ne!(
                reported_type, CATALOG_SELECTOR,
                "case {case:?}: {CATALOG_SELECTOR} selects every type, so no entity is \
                 ever reported as it, got {entity}"
            );
            assert_eq!(
                reported_type, *expected_type,
                "case {case:?}: expected {expected_type:?} for {entity_text:?}, got {entity}"
            );

            let (begin, end) = expected_offsets(case, text, entity_text);
            let reported_begin = entity["BeginOffset"].as_i64().unwrap_or(-1);
            let reported_end = entity["EndOffset"].as_i64().unwrap_or(-1);
            assert_eq!(
                (reported_begin, reported_end),
                (begin, end),
                "case {case:?}: expected the character offsets ({begin}, {end}) for \
                 {entity_text:?}, got {entity}"
            );

            // The offsets must be CHARACTER offsets into the document: taking that
            // range of characters has to give the entity back.
            let sliced: String = text
                .chars()
                .skip(reported_begin.max(0) as usize)
                .take((reported_end - reported_begin).max(0) as usize)
                .collect();
            assert_eq!(
                sliced, *entity_text,
                "case {case:?}: the reported span does not read back as the entity, got \
                 {sliced:?} from {entity}"
            );

            let score = entity["Score"].as_f64().unwrap_or(f64::NAN);
            assert!(
                score > 0.0 && score <= 1.0,
                "case {case:?}: expected a Score in (0, 1] for {entity_text:?}, got {entity}"
            );
        }
    }
}

/// Character offsets, not byte offsets: the same entity preceded by padding of
/// different byte widths keeps the character offsets of its position.
///
/// Each prefix is a handful of characters that are one, two, three and four UTF-8
/// bytes wide respectively, so a byte-offset answer differs from a character-offset
/// answer on every row but the first.
#[tokio::test]
async fn offsets_are_character_offsets_whatever_the_preceding_text_encodes_to() {
    let comprehend = Comprehend::new();
    let entity_text = "jane.roe@example.com";

    for (case, prefix) in [
        ("one-byte characters", "Mail: "),
        ("two-byte characters", "Correo \u{e9} \u{e0} \u{f6}: "),
        ("three-byte characters", "\u{30e1}\u{30fc}\u{30eb} \u{30a2}\u{30c9}\u{30ec}\u{30b9}: "),
        ("four-byte characters", "Mail \u{1f642}\u{1f642}\u{1f642}: "),
    ] {
        let text = format!("{prefix}{entity_text} please.");
        let output = comprehend
            .ok(
                "DetectPiiEntities",
                &json!({ "Text": &text, "LanguageCode": "en" }),
            )
            .await;
        let found = entities(case, &output);

        assert_eq!(
            found.len(),
            1,
            "case {case:?}: expected the one email address, got {output}"
        );
        let (begin, end) = expected_offsets(case, &text, entity_text);
        assert_eq!(
            (
                found[0]["BeginOffset"].as_i64().unwrap_or(-1),
                found[0]["EndOffset"].as_i64().unwrap_or(-1)
            ),
            (begin, end),
            "case {case:?}: expected the character offsets ({begin}, {end}), got {output}"
        );
    }
}

/// Every language this operation's own accepted set admits is analysed, and the
/// entities answered are the ones the document's own forms support, so they do not
/// depend on which accepted code was sent. A code OUTSIDE that set is refused as a
/// request-validation constraint — the rule `language_value_sets` asserts over every
/// value the modelled enum names, for every operation applying a set of its own.
#[tokio::test]
async fn every_accepted_language_code_answers_the_same_entities() {
    let comprehend = Comprehend::new();
    let text = "Write to jane.roe@example.com or call 206-555-0147 today.";
    let mut baseline: Option<Value> = None;

    // `DetectPiiEntitiesRequest$LanguageCode`: "Enter the language code for English
    // (en) or Spanish (es)."
    for code in ["en", "es"] {
        let (status, body) = comprehend
            .call(
                "DetectPiiEntities",
                &json!({ "Text": text, "LanguageCode": code }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {code:?}: expected 200, got {status}: {body}"
        );
        match &baseline {
            None => baseline = Some(body),
            Some(expected) => assert_eq!(
                &body, expected,
                "case {code:?}: expected the same entities as the first language, got {body}"
            ),
        }
    }
}

#[tokio::test]
async fn a_language_code_outside_the_modelled_enum_is_refused() {
    let comprehend = Comprehend::new();

    for code in ["xx", "EN", "english", ""] {
        let (status, body) = comprehend
            .call(
                "DetectPiiEntities",
                &json!({ "Text": "Write to jane.roe@example.com.", "LanguageCode": code }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {code:?}: expected 400 for a value outside the LanguageCode enum, got \
             {status}: {body}"
        );
    }
}

#[tokio::test]
async fn an_absent_or_empty_text_is_refused_by_the_modelled_length_floor() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        ("empty Text", json!({ "Text": "", "LanguageCode": "en" })),
        ("absent Text", json!({ "LanguageCode": "en" })),
    ] {
        let (status, response) = comprehend.call("DetectPiiEntities", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
    }
}

#[tokio::test]
async fn detect_pii_entities_is_deterministic_and_changes_nothing_a_later_caller_reads() {
    let comprehend = Comprehend::new();
    let body = json!({
        "Text": "My name is Jane Roe and my card is 4111 1111 1111 1111.",
        "LanguageCode": "en",
    });

    let first = comprehend.ok("DetectPiiEntities", &body).await;
    let second = comprehend.ok("DetectPiiEntities", &body).await;
    assert_eq!(
        first, second,
        "case \"repeated identical call\": expected the identical answer, got {first} \
         then {second}"
    );
}
