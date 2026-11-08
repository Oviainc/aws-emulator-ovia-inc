// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rules Amazon SQS applies to the attributes a message
//! carries, driven over the wire (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`)
//! through the shared harness in [`super::support`].
//!
//! The rules pinned here:
//!
//! * a message attribute is taken only when its NAME, its `DataType` and its VALUE all
//!   satisfy the documented rules for them — up to 256 alphanumeric characters,
//!   underscores, hyphens and periods, no reserved `AWS.`/`Amazon.` prefix, no leading,
//!   trailing or repeated period; one of the `String`, `Number` and `Binary` logical
//!   types with an optional non-empty custom label; the value in the field its type
//!   names and holding at least one byte; and a `Number`'s value a number — and a
//!   request breaking any of them is refused with HTTP 400 `InvalidParameterValue` with
//!   nothing enqueued;
//! * a `Number` attribute's value is stored as the NUMBER it names rather than as the
//!   text of one, so what a consumer reads is that number's canonical plain decimal, and
//!   each digest covers the attribute set its own reader has — the send's the ones the
//!   request carried, a receive's the ones it hands over;
//! * `MessageSystemAttributes` takes the one name its key shape publishes
//!   (`AWSTraceHeader`) with a non-empty `String` value, is digested into
//!   `MD5OfMessageSystemAttributes`, comes back on a receive that selects it, and does
//!   not count toward the queue's `MaximumMessageSize`; any other name is refused the
//!   same way;
//! * `MessageAttributeNames` selects an attribute by exact name, or by the `prefix.*`
//!   wildcard, which selects every attribute whose name starts with `prefix`.
//!
//! Each table spans the class the rule generalises over — the kinds of name, the kinds
//! of type and value, the kinds of selector — rather than only the one case a defect was
//! reported for.

use std::collections::BTreeSet;

use md5::{Digest, Md5};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{attribute_count, call, create_ok, emulator, error_code, messages, ok};

/// A valid X-Ray trace header string, the value `AWSTraceHeader` carries.
const TRACE_HEADER: &str =
    "Root=1-5759e988-bd862e3fe1be46a994272793;Parent=53995c3f42cd8ad8;Sampled=1";

/// The refusal every rule here answers with: "a parameter's value is not one this
/// service accepts" is `InvalidParameterValue` at HTTP 400.
const REFUSAL: (u16, &str) = (400, "InvalidParameterValue");

/// How many messages the queue at `url` holds.
async fn held(router: &MultiServiceRouter, url: &str) -> i64 {
    attribute_count(router, url, "ApproximateNumberOfMessages").await
}

/// Send one message carrying `attributes` as its `MessageAttributes`.
async fn send_with_attributes(
    router: &MultiServiceRouter,
    url: &str,
    attributes: &Value,
) -> (u16, Value) {
    call(
        router,
        "SendMessage",
        &json!({
            "QueueUrl": url,
            "MessageBody": "payload",
            "MessageAttributes": attributes,
        }),
    )
    .await
}

/// The names of the message attributes one received message carries.
fn attribute_names(message: &Value) -> BTreeSet<String> {
    message["MessageAttributes"]
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// The Amazon SQS digest encoding of one `StringValue` attribute, written out by hand
/// from the Developer Guide ("Calculating the MD5 message digest for message
/// attributes"): the name, the `DataType` and the value each prefixed with a 4-byte
/// big-endian length, with the transport-type byte `1` (String) before the value.
fn string_attribute_encoding(name: &str, data_type: &str, value: &str) -> Vec<u8> {
    let length_prefixed = |part: &str| {
        let bytes = part.as_bytes();
        let length = u32::try_from(bytes.len()).expect("a test value shorter than 4 GiB");
        [&length.to_be_bytes()[..], bytes].concat()
    };
    [
        length_prefixed(name),
        length_prefixed(data_type),
        vec![1],
        length_prefixed(value),
    ]
    .concat()
}

/// A `Number` attribute's value is judged as a NUMBER: the published precision bound is
/// taken and one digit past it refused, as is a magnitude outside the published range,
/// and a value that is not a number at all.
///
/// Source for the bounds: the Amazon SQS Developer Guide's message attribute data types
/// ("a number can have up to 38 digits of precision, and it must be within the range
/// -10^128 to 10^126").
#[tokio::test]
async fn a_number_attribute_is_bounded_by_its_published_precision_and_range() {
    let router = emulator();
    let url = create_ok(&router, "number-bounds", &json!({})).await;

    // `(case, the Number's value, taken)`
    let rows: Vec<(String, String, bool)> = vec![
        ("one digit".to_string(), "7".to_string(), true),
        (
            "the published precision bound".to_string(),
            "4".repeat(38),
            true,
        ),
        (
            "one digit past the precision bound".to_string(),
            "4".repeat(39),
            false,
        ),
        (
            "leading zeros are no precision".to_string(),
            format!("0.{}", "4".repeat(38)),
            true,
        ),
        (
            "a magnitude inside the published range".to_string(),
            "1e100".to_string(),
            true,
        ),
        (
            "a magnitude past the published range".to_string(),
            "1e400".to_string(),
            false,
        ),
        (
            "a magnitude far below the published range".to_string(),
            "1e-400".to_string(),
            false,
        ),
        (
            "not a number at all".to_string(),
            "not-a-number".to_string(),
            false,
        ),
    ];

    for (case, value, taken) in &rows {
        let before = held(&router, &url).await;
        let attributes = json!({ "N": { "DataType": "Number", "StringValue": value } });
        let (status, answer) = send_with_attributes(&router, &url, &attributes).await;
        let after = held(&router, &url).await;
        if *taken {
            assert_eq!(
                status, 200,
                "case {case}: expected the value to be taken, got {status} with {answer}"
            );
            assert_eq!(
                after,
                before + 1,
                "case {case}: an accepted send must enqueue one message, held {before} then \
                 {after}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                REFUSAL,
                "case {case}: expected the value to be refused, got {status} with {answer}"
            );
            assert_eq!(
                after, before,
                "case {case}: a refused send must enqueue nothing, held {before} then {after}"
            );
        }
    }
}

/// A `Number` attribute carries a NUMBER, so the queue holds the number the value names
/// rather than the text it was written as: the value comes back as that number's
/// canonical plain decimal — no exponent, no redundant `+`, none of the leading zeros of
/// the integer part and none of the insignificant trailing zeros of the fraction.
///
/// The two digests are therefore of two different attribute sets, and each is of the one
/// its own reader has: `MD5OfMessageAttributes` on the SEND is "an MD5 digest of the
/// non-URL-encoded message attribute string" a sender compares against its own digest to
/// "verify that Amazon SQS received the message correctly", so it covers the attributes the
/// REQUEST carried; the one a RECEIVE reports covers the attributes it is handing over.
///
/// The rows differ along how the same number can be spelled: in exponent form in either
/// casing and with either sign of exponent, with a redundant sign, with leading zeros,
/// with insignificant trailing zeros in the fraction, with both at once, as a zero written
/// several ways, and as a value already canonical (which must be left alone, including the
/// trailing zeros of an INTEGER, which carry its magnitude).
#[tokio::test]
async fn a_number_attribute_is_stored_as_the_number_it_names() {
    let router = emulator();

    // `(the value as sent, the value the queue holds)`
    let rows: &[(&str, &str)] = &[
        ("1e3", "1000"),
        ("1E3", "1000"),
        ("2e2", "200"),
        ("1e-3", "0.001"),
        ("+7", "7"),
        ("007", "7"),
        ("-12.5", "-12.5"),
        ("1.50", "1.5"),
        ("0042.50", "42.5"),
        ("1.2300", "1.23"),
        ("-12.500", "-12.5"),
        ("0.00", "0"),
        ("-0.0", "0"),
        ("7.0", "7"),
        ("1200", "1200"),
        ("0.000001", "0.000001"),
        ("42", "42"),
    ];

    for (index, (sent, stored)) in rows.iter().enumerate() {
        let url = create_ok(&router, &format!("number-form-{index}"), &json!({})).await;
        let sent_digest = hex::encode(Md5::digest(string_attribute_encoding("N", "Number", sent)));
        let stored_digest = hex::encode(Md5::digest(string_attribute_encoding(
            "N", "Number", stored,
        )));
        let answer = ok(
            &router,
            "SendMessage",
            json!({
                "QueueUrl": url,
                "MessageBody": "payload",
                "MessageAttributes": { "N": { "DataType": "Number", "StringValue": sent } },
            }),
        )
        .await;
        assert_eq!(
            answer["MD5OfMessageAttributes"].as_str(),
            Some(sent_digest.as_str()),
            "case {sent}: the send's digest must cover the attributes the request carried \
             ({sent}), got {answer}"
        );

        let received = messages(
            &ok(
                &router,
                "ReceiveMessage",
                json!({"QueueUrl": url, "MessageAttributeNames": ["All"]}),
            )
            .await,
        );
        assert_eq!(
            received.len(),
            1,
            "case {sent}: expected the one sent message, got {received:?}"
        );
        let actual = received[0]["MessageAttributes"]["N"]["StringValue"].as_str();
        assert_eq!(
            actual,
            Some(*stored),
            "case {sent}: expected the queue to hold {stored:?}, got {actual:?}"
        );
        assert_eq!(
            received[0]["MD5OfMessageAttributes"].as_str(),
            Some(stored_digest.as_str()),
            "case {sent}: the receive's digest must cover the attributes it handed over \
             ({stored}), got {:?}",
            received[0]
        );
    }
}

/// A message attribute name is taken only when it satisfies every documented rule for
/// one, and a name breaking any of them is refused with nothing enqueued.
///
/// The rows walk the rules, each on a name the reported defect did not cite: the
/// published length bound and one character past it, the reserved prefixes in both
/// casings, the period placements, and the characters outside the allowed set.
#[tokio::test]
async fn message_attribute_names_are_taken_only_inside_the_documented_rules() {
    let router = emulator();
    let url = create_ok(&router, "attribute-names", &json!({})).await;

    // `(case, the attribute name, taken)`
    let rows: &[(&str, &str, bool)] = &[
        ("a plain name", "order", true),
        (
            "every allowed character class",
            "Order_1-2.item3",
            true,
        ),
        ("a name at the 256-character bound", &"n".repeat(256), true),
        ("one character past the bound", &"n".repeat(257), false),
        ("the empty name", "", false),
        ("the reserved AWS. prefix", "AWS.Reserved", false),
        ("the reserved Amazon. prefix", "Amazon.Reserved", false),
        ("the reserved prefix in lower case", "aws.reserved", false),
        (
            "the reserved prefix in mixed case",
            "AmAzOn.Reserved",
            false,
        ),
        // The reserved prefixes include the period, so a name merely starting with
        // those letters is not reserved.
        ("those letters without the period", "AWSRegion", true),
        ("a leading period", ".order", false),
        ("a trailing period", "order.", false),
        ("periods in succession", "order..id", false),
        ("a space", "order id", false),
        ("a character outside the set", "order$id", false),
        ("a non-ASCII letter", "ordenación", false),
        ("a colon", "order:id", false),
    ];

    for (case, name, taken) in rows {
        let before = held(&router, &url).await;
        let attributes = json!({ *name: { "DataType": "String", "StringValue": "v" } });
        let (status, answer) = send_with_attributes(&router, &url, &attributes).await;
        let after = held(&router, &url).await;
        if *taken {
            assert_eq!(
                status, 200,
                "case {case}: expected the name to be taken, got {status} with {answer}"
            );
            assert_eq!(
                after,
                before + 1,
                "case {case}: an accepted send must enqueue one message, held {before} then \
                 {after}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                REFUSAL,
                "case {case}: expected the name to be refused, got {status} with {answer}"
            );
            assert_eq!(
                after, before,
                "case {case}: a refused send must enqueue nothing, held {before} then {after}"
            );
        }
    }
}

/// An attribute's `DataType` and value are validated together: the type names one of the
/// logical types with an optional non-empty label, the value lives in the field that
/// type names and holds at least one byte, and a `Number` carries a number.
#[tokio::test]
async fn message_attribute_types_and_values_are_validated() {
    let router = emulator();
    let url = create_ok(&router, "attribute-values", &json!({})).await;

    // `(case, the attribute's members, taken)`
    let rows: &[(&str, Value, bool)] = &[
        (
            "a String in StringValue",
            json!({ "DataType": "String", "StringValue": "green" }),
            true,
        ),
        (
            "a Binary in BinaryValue",
            // 3q2+7w== is base64 for DE AD BE EF.
            json!({ "DataType": "Binary", "BinaryValue": "3q2+7w==" }),
            true,
        ),
        (
            "a custom label on a logical type",
            json!({ "DataType": "Number.float", "StringValue": "1.5" }),
            true,
        ),
        (
            "a custom label on String",
            json!({ "DataType": "String.uuid", "StringValue": "1-2-3" }),
            true,
        ),
        (
            "an empty custom label",
            json!({ "DataType": "String.", "StringValue": "green" }),
            false,
        ),
        (
            "an unsupported type",
            json!({ "DataType": "Boolean", "StringValue": "true" }),
            false,
        ),
        (
            "a logical type in the wrong case",
            json!({ "DataType": "string", "StringValue": "green" }),
            false,
        ),
        (
            "an empty type",
            json!({ "DataType": "", "StringValue": "green" }),
            false,
        ),
        // The Number rows: "numbers are positive or negative integers or floating-point
        // numbers".
        (
            "a positive integer Number",
            json!({ "DataType": "Number", "StringValue": "123" }),
            true,
        ),
        (
            "a negative floating-point Number",
            json!({ "DataType": "Number", "StringValue": "-3.5" }),
            true,
        ),
        (
            "a signed Number",
            json!({ "DataType": "Number", "StringValue": "+7" }),
            true,
        ),
        (
            "a Number that is not a number",
            json!({ "DataType": "Number", "StringValue": "not-a-number" }),
            false,
        ),
        (
            "a Number with trailing text",
            json!({ "DataType": "Number", "StringValue": "12a" }),
            false,
        ),
        (
            "a Number in exponent notation",
            json!({ "DataType": "Number", "StringValue": "1e5" }),
            true,
        ),
        (
            "a Number in signed exponent notation",
            json!({ "DataType": "Number", "StringValue": "-1.5E-3" }),
            true,
        ),
        (
            "a Number whose exponent is not a number",
            json!({ "DataType": "Number", "StringValue": "1e" }),
            false,
        ),
        (
            "a Number that is only a sign",
            json!({ "DataType": "Number", "StringValue": "-" }),
            false,
        ),
        // The value must be in the field its type names.
        (
            "a String supplied in BinaryValue",
            json!({ "DataType": "String", "BinaryValue": "3q2+7w==" }),
            false,
        ),
        (
            "a Number supplied in BinaryValue",
            json!({ "DataType": "Number", "BinaryValue": "3q2+7w==" }),
            false,
        ),
        (
            "a Binary supplied in StringValue",
            json!({ "DataType": "Binary", "StringValue": "3q2+7w==" }),
            false,
        ),
        (
            "a labelled String supplied in BinaryValue",
            json!({ "DataType": "String.uuid", "BinaryValue": "3q2+7w==" }),
            false,
        ),
        // "Name, type, value and the message body must not be empty or null": an empty
        // value is not a value, which is what makes it differ from an absent one.
        (
            "an empty StringValue",
            json!({ "DataType": "String", "StringValue": "" }),
            false,
        ),
        (
            "an empty BinaryValue",
            json!({ "DataType": "Binary", "BinaryValue": "" }),
            false,
        ),
        (
            "no value at all",
            json!({ "DataType": "String" }),
            false,
        ),
    ];

    for (case, value, taken) in rows {
        let before = held(&router, &url).await;
        let attributes = json!({ "attr": value });
        let (status, answer) = send_with_attributes(&router, &url, &attributes).await;
        let after = held(&router, &url).await;
        if *taken {
            assert_eq!(
                status, 200,
                "case {case}: expected the attribute to be taken, got {status} with {answer}"
            );
            assert!(
                answer["MD5OfMessageAttributes"].is_string(),
                "case {case}: an accepted send reports MD5OfMessageAttributes, got {answer}"
            );
            assert_eq!(
                after,
                before + 1,
                "case {case}: an accepted send must enqueue one message, held {before} then \
                 {after}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                REFUSAL,
                "case {case}: expected the attribute to be refused, got {status} with {answer}"
            );
            assert_eq!(
                after, before,
                "case {case}: a refused send must enqueue nothing, held {before} then {after}"
            );
        }
    }
}

/// A taken attribute round-trips: every member the send supplied comes back on the
/// receive, under the same name, for each logical type.
#[tokio::test]
async fn a_taken_attribute_comes_back_on_the_receive() {
    let router = emulator();
    let url = create_ok(&router, "attribute-roundtrip", &json!({})).await;
    let attributes = json!({
        "order.id": { "DataType": "String", "StringValue": "A-1" },
        "size": { "DataType": "Number", "StringValue": "-12.5" },
        "blob": { "DataType": "Binary", "BinaryValue": "3q2+7w==" },
        "labelled": { "DataType": "Number.float", "StringValue": "0.5" },
    });
    let sent = ok(
        &router,
        "SendMessage",
        json!({
            "QueueUrl": url,
            "MessageBody": "payload",
            "MessageAttributes": attributes,
        }),
    )
    .await;
    assert!(
        sent["MD5OfMessageAttributes"].is_string(),
        "case the round trip: expected MD5OfMessageAttributes, got {sent}"
    );

    let received = messages(
        &ok(
            &router,
            "ReceiveMessage",
            json!({"QueueUrl": url, "MessageAttributeNames": ["All"]}),
        )
        .await,
    );
    assert_eq!(
        received.len(),
        1,
        "case the round trip: expected the one sent message, got {received:?}"
    );
    let reported = &received[0]["MessageAttributes"];
    for (name, expected) in attributes.as_object().expect("an object of attributes") {
        assert_eq!(
            &reported[name], expected,
            "case the round trip of {name}: expected {expected}, got {}",
            reported[name]
        );
    }
    assert_eq!(
        received[0]["MD5OfMessageAttributes"], sent["MD5OfMessageAttributes"],
        "case the round trip: the receive must report the digest the send did, got {}",
        received[0]["MD5OfMessageAttributes"]
    );
}

/// `MessageSystemAttributes` takes the one name its key shape publishes, digests it into
/// `MD5OfMessageSystemAttributes` and hands it back on a receive that selects it; every
/// other name, and a value that is not a non-empty `String`, is refused with nothing
/// enqueued.
#[tokio::test]
async fn message_system_attributes_take_only_the_trace_header() {
    let router = emulator();
    let url = create_ok(&router, "system-attributes", &json!({})).await;

    // The accepted case: the digest is the documented encoding of the one attribute.
    let expected_digest = hex::encode(Md5::digest(string_attribute_encoding(
        "AWSTraceHeader",
        "String",
        TRACE_HEADER,
    )));
    let sent = ok(
        &router,
        "SendMessage",
        json!({
            "QueueUrl": url,
            "MessageBody": "payload",
            "MessageSystemAttributes": {
                "AWSTraceHeader": { "DataType": "String", "StringValue": TRACE_HEADER },
            },
        }),
    )
    .await;
    assert_eq!(
        sent["MD5OfMessageSystemAttributes"].as_str(),
        Some(expected_digest.as_str()),
        "case the trace header: expected MD5OfMessageSystemAttributes {expected_digest}, got \
         {sent}"
    );
    assert!(
        sent.get("MD5OfMessageAttributes").is_none(),
        "case the trace header: a message with no message attributes reports no digest for \
         them, got {sent}"
    );

    // It comes back on a receive that selects it, by name and under `All`, and only
    // then.
    for (case, requested, wanted) in [
        ("selected by name", json!(["AWSTraceHeader"]), true),
        ("selected by All", json!(["All"]), true),
        ("another name selected", json!(["SentTimestamp"]), false),
    ] {
        let received = messages(
            &ok(
                &router,
                "ReceiveMessage",
                json!({
                    "QueueUrl": url,
                    "VisibilityTimeout": 0,
                    "MessageSystemAttributeNames": requested,
                }),
            )
            .await,
        );
        assert_eq!(
            received.len(),
            1,
            "case {case}: expected the one sent message, got {received:?}"
        );
        let reported = received[0]["Attributes"]["AWSTraceHeader"].as_str();
        let expected = wanted.then_some(TRACE_HEADER);
        assert_eq!(
            reported, expected,
            "case {case}: expected AWSTraceHeader {expected:?}, got {reported:?}"
        );
    }

    // `(case, the system attribute map, taken)`
    let rows: &[(&str, Value, bool)] = &[
        (
            "a name that is not the published one",
            json!({ "traceparent": { "DataType": "String", "StringValue": TRACE_HEADER } }),
            false,
        ),
        (
            "another unpublished name",
            json!({ "XRayTraceHeader": { "DataType": "String", "StringValue": TRACE_HEADER } }),
            false,
        ),
        (
            "the published name in the wrong case",
            json!({ "AWSTraceheader": { "DataType": "String", "StringValue": TRACE_HEADER } }),
            false,
        ),
        (
            "a message system attribute name that selects, not sends",
            json!({ "SentTimestamp": { "DataType": "String", "StringValue": "0" } }),
            false,
        ),
        (
            "the published name with a Number type",
            json!({ "AWSTraceHeader": { "DataType": "Number", "StringValue": "1" } }),
            false,
        ),
        (
            "the published name with a Binary value",
            json!({ "AWSTraceHeader": { "DataType": "Binary", "BinaryValue": "3q2+7w==" } }),
            false,
        ),
        (
            "the published name with an empty value",
            json!({ "AWSTraceHeader": { "DataType": "String", "StringValue": "" } }),
            false,
        ),
        (
            "the published name with no value",
            json!({ "AWSTraceHeader": { "DataType": "String" } }),
            false,
        ),
        (
            "the published name with a well-formed value",
            json!({ "AWSTraceHeader": { "DataType": "String", "StringValue": TRACE_HEADER } }),
            true,
        ),
    ];

    let refusals = create_ok(&router, "system-attribute-rules", &json!({})).await;
    for (case, system_attributes, taken) in rows {
        let before = held(&router, &refusals).await;
        let (status, answer) = call(
            &router,
            "SendMessage",
            &json!({
                "QueueUrl": refusals,
                "MessageBody": "payload",
                "MessageSystemAttributes": system_attributes,
            }),
        )
        .await;
        let after = held(&router, &refusals).await;
        if *taken {
            assert_eq!(
                status, 200,
                "case {case}: expected acceptance, got {status} with {answer}"
            );
            assert_eq!(
                after,
                before + 1,
                "case {case}: an accepted send must enqueue one message, held {before} then \
                 {after}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                REFUSAL,
                "case {case}: expected a refusal, got {status} with {answer}"
            );
            assert_eq!(
                after, before,
                "case {case}: a refused send must enqueue nothing, held {before} then {after}"
            );
        }
    }
}

/// "The size of a message system attribute doesn't count towards the total size of a
/// message": a body and message attributes that together reach the queue's
/// `MaximumMessageSize` are accepted however large the trace header beside them is,
/// while one byte more of BODY is refused.
#[tokio::test]
async fn a_message_system_attribute_does_not_count_toward_the_message_size() {
    // `k` (1) + `String` (6) + `v` (1): the message attribute's counted bytes.
    const ATTRIBUTE_BYTES: usize = 8;
    let attribute = json!({ "k": { "DataType": "String", "StringValue": "v" } });

    let router = emulator();
    for maximum in [1_024_usize, 2_048] {
        let url = create_ok(
            &router,
            &format!("system-size-{maximum}"),
            &json!({ "MaximumMessageSize": maximum.to_string() }),
        )
        .await;

        // `(case, body length, accepted)`
        let rows: [(&str, usize, bool); 3] = [
            ("body and attribute exactly at the limit", maximum - ATTRIBUTE_BYTES, true),
            ("one body byte over the limit", maximum - ATTRIBUTE_BYTES + 1, false),
            ("well under the limit", 1, true),
        ];
        for (case, body_len, accepted) in rows {
            let (status, answer) = call(
                &router,
                "SendMessage",
                &json!({
                    "QueueUrl": url,
                    "MessageBody": "x".repeat(body_len),
                    "MessageAttributes": attribute,
                    "MessageSystemAttributes": {
                        "AWSTraceHeader": { "DataType": "String", "StringValue": TRACE_HEADER },
                    },
                }),
            )
            .await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case maximum {maximum} / {case}: expected acceptance, got {status} with \
                     {answer}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case maximum {maximum} / {case}: expected a refusal, got {status} with \
                     {answer}"
                );
            }
        }
    }
}

/// `MessageAttributeNames` selects by exact name or by the `prefix.*` wildcard, which
/// takes every attribute whose name STARTS WITH what stands before the `.*` — the
/// period being the wildcard's own separator, not part of the prefix.
#[tokio::test]
async fn the_attribute_name_wildcard_selects_by_name_prefix() {
    let router = emulator();
    let url = create_ok(&router, "wildcard-selection", &json!({})).await;
    let carried = ["order.id", "order.total", "orderly", "ordinal", "shipment"];
    let attributes: Map<String, Value> = carried
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                json!({ "DataType": "String", "StringValue": "v" }),
            )
        })
        .collect();
    ok(
        &router,
        "SendMessage",
        json!({
            "QueueUrl": url,
            "MessageBody": "payload",
            "MessageAttributes": Value::Object(attributes),
        }),
    )
    .await;

    // `(the selectors the request carries, the attribute names the answer must carry)`
    let rows: &[(&[&str], &[&str])] = &[
        (&["All"], &carried),
        (&[".*"], &carried),
        (&["order.*"], &["order.id", "order.total", "orderly"]),
        (
            &["ord.*"],
            &["order.id", "order.total", "orderly", "ordinal"],
        ),
        (&["orderly.*"], &["orderly"]),
        (&["order.id"], &["order.id"]),
        (&["order.id", "shipment"], &["order.id", "shipment"]),
        (&["order.*", "ordinal"], &["order.id", "order.total", "orderly", "ordinal"]),
        (&["nothing.*"], &[]),
        (&["order"], &[]),
        (&["order*"], &[]),
    ];

    for (requested, expected) in rows {
        let received = messages(
            &ok(
                &router,
                "ReceiveMessage",
                json!({
                    "QueueUrl": url,
                    "VisibilityTimeout": 0,
                    "MessageAttributeNames": requested,
                }),
            )
            .await,
        );
        assert_eq!(
            received.len(),
            1,
            "case {requested:?}: expected the one sent message, got {received:?}"
        );
        let actual = attribute_names(&received[0]);
        let wanted: BTreeSet<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            actual, wanted,
            "case {requested:?}: expected the attributes {wanted:?}, got {actual:?}"
        );
    }
}

/// The batch send applies the same rules per ENTRY: an entry whose attributes or system
/// attributes break one fails alone under its own `Id` with the sender's fault, while
/// the entries beside it are enqueued and report both digests.
#[tokio::test]
async fn send_message_batch_judges_each_entry_by_the_same_rules() {
    let router = emulator();
    let url = create_ok(&router, "batch-attribute-rules", &json!({})).await;

    let answer = ok(
        &router,
        "SendMessageBatch",
        json!({
            "QueueUrl": url,
            "Entries": [
                {
                    "Id": "good",
                    "MessageBody": "payload",
                    "MessageAttributes": {
                        "order.id": { "DataType": "String", "StringValue": "A-1" },
                    },
                    "MessageSystemAttributes": {
                        "AWSTraceHeader": { "DataType": "String", "StringValue": TRACE_HEADER },
                    },
                },
                {
                    "Id": "reserved-name",
                    "MessageBody": "payload",
                    "MessageAttributes": {
                        "AWS.Reserved": { "DataType": "String", "StringValue": "v" },
                    },
                },
                {
                    "Id": "bad-number",
                    "MessageBody": "payload",
                    "MessageAttributes": {
                        "size": { "DataType": "Number", "StringValue": "not-a-number" },
                    },
                },
                {
                    "Id": "bad-system-name",
                    "MessageBody": "payload",
                    "MessageSystemAttributes": {
                        "traceparent": { "DataType": "String", "StringValue": TRACE_HEADER },
                    },
                },
            ],
        }),
    )
    .await;

    let successful: Vec<String> = answer["Successful"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        successful,
        vec!["good".to_string()],
        "case the batch: only the well-formed entry is taken, got {answer}"
    );
    let expected_digest = hex::encode(Md5::digest(string_attribute_encoding(
        "AWSTraceHeader",
        "String",
        TRACE_HEADER,
    )));
    assert_eq!(
        answer["Successful"][0]["MD5OfMessageSystemAttributes"].as_str(),
        Some(expected_digest.as_str()),
        "case the batch: the taken entry reports the documented system-attribute digest, got \
         {answer}"
    );

    for (case, id) in [
        ("a reserved attribute name", "reserved-name"),
        ("a Number that is not a number", "bad-number"),
        ("an unpublished system attribute name", "bad-system-name"),
    ] {
        let failed = answer["Failed"]
            .as_array()
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry["Id"].as_str() == Some(id))
                    .cloned()
            })
            .unwrap_or_else(|| panic!("case {case}: no Failed entry for {id} in {answer}"));
        assert_eq!(
            (
                failed["Code"].as_str().unwrap_or_default(),
                failed["SenderFault"].as_bool()
            ),
            (REFUSAL.1, Some(true)),
            "case {case}: expected a sender-fault {} entry, got {failed}",
            REFUSAL.1
        );
    }

    assert_eq!(
        held(&router, &url).await,
        1,
        "case the batch: only the taken entry is enqueued"
    );
}
