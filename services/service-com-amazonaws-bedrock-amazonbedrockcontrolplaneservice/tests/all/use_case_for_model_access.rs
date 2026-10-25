// SPDX-License-Identifier: CC0-1.0

//! The account's use case for model access end to end: `Put` then `Get` over HTTP,
//! through the same `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! The use case for model access is ONE account-level form, so
//! `PutUseCaseForModelAccess` replaces it whole with the `formData` bytes the request
//! carries, `GetUseCaseForModelAccess` reports back exactly those bytes — byte for byte,
//! nothing re-encoded and nothing merged in from an earlier put — and while no form has
//! been submitted the get has no `@required formData` to report, so it answers the
//! modelled `ResourceNotFoundException` (HTTP 404).
//!
//! # What the table below generalises over
//!
//! One row per independent form along the axes a blob spans: its SIZE (both published
//! bounds of `AcknowledgementFormDataBody`, `@length(10..16384)` in this service's Smithy
//! model, and sizes between them), its CONTENT (ASCII JSON, UTF-8 beyond ASCII, bytes
//! that are not text at all — including NUL, 0xFF and every byte value — so nothing may
//! round-trip the form as a string), and its ORDER (the rows are driven in sequence
//! against ONE emulator, so each get also witnesses that the put before it replaced the
//! previous row rather than appending to or merging with it).
//! [`a_form_outside_the_published_size_bounds_is_refused_and_changes_nothing`] drives one
//! form under and one past each of those bounds.

use serde_json::{json, Value};

use crate::support::{emulator, Emulator, Reply};

/// The URI both `PutUseCaseForModelAccess` and `GetUseCaseForModelAccess` are bound to.
const USE_CASE_URI: &str = "/use-case-for-model-access";

/// `AcknowledgementFormDataBody` is `@length(min: 10, max: 16384)`, from this service's
/// Smithy model.
const MIN_FORM_BYTES: usize = 10;
const MAX_FORM_BYTES: usize = 16384;

/// One form this suite submits and reads back.
struct Case {
    name: &'static str,
    form_data: Vec<u8>,
}

/// `n` bytes of `filler`, for a form driven at a bound of its `@length`.
fn filler_bytes(filler: u8, n: usize) -> Vec<u8> {
    vec![filler; n]
}

/// A form padded to `n` bytes so a chosen payload can be driven at a chosen size.
fn padded(payload: &[u8], n: usize) -> Vec<u8> {
    let mut bytes = payload.to_vec();
    bytes.resize(n, b' ');
    bytes
}

/// The forms the rule must round-trip, one per independent shape of the blob.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "an acknowledgement form as JSON text",
            form_data: br#"{"companyName":"Acme","useCase":"document summarisation"}"#.to_vec(),
        },
        Case {
            name: "the shortest form the model admits (10 bytes)",
            form_data: padded(b"acme", MIN_FORM_BYTES),
        },
        Case {
            name: "the longest form the model admits (16384 bytes)",
            form_data: filler_bytes(b'f', MAX_FORM_BYTES),
        },
        Case {
            name: "UTF-8 beyond ASCII",
            form_data: "use case: résumé analysis — 日本語 🙂".as_bytes().to_vec(),
        },
        Case {
            name: "bytes that are not text: NUL, 0xFF and everything between",
            form_data: (0u8..=255).collect(),
        },
        Case {
            name: "a form of nothing but NUL bytes",
            form_data: filler_bytes(0, 64),
        },
        Case {
            name: "one byte past the shortest admitted form",
            form_data: padded(b"x", MIN_FORM_BYTES + 1),
        },
        Case {
            name: "one byte under the longest admitted form",
            form_data: filler_bytes(b'z', MAX_FORM_BYTES - 1),
        },
    ]
}

/// A blob member as restJson1 carries it: base64 of the raw bytes.
fn blob_member(bytes: &[u8]) -> Value {
    Value::String(::aws_smithy_types::base64::encode(bytes))
}

/// The raw bytes a response's blob member carries, decoded with the same codec the wire
/// uses; panics naming the case when the member is absent or not base64.
fn blob_bytes(case: &str, reply: &Reply, member: &str) -> Vec<u8> {
    let value = reply.get(member);
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case}: member {member} must be a base64 string, got {value} in {:?}", reply.body)
    });
    ::aws_smithy_types::base64::decode(text)
        .unwrap_or_else(|error| panic!("case {case}: {text:?} does not decode as base64: {error}"))
}

/// `POST /use-case-for-model-access` with `formData`.
async fn put(emulator: &Emulator, form_data: &[u8]) -> Reply {
    emulator
        .call(
            "POST",
            USE_CASE_URI,
            Some(json!({ "formData": blob_member(form_data) })),
        )
        .await
}

/// `GET /use-case-for-model-access`.
async fn get(emulator: &Emulator) -> Reply {
    emulator.call("GET", USE_CASE_URI, None).await
}

#[tokio::test]
async fn a_get_before_any_put_is_a_not_found() {
    let emulator = emulator();
    for attempt in 1..=2 {
        let reply = get(&emulator).await;
        assert_eq!(
            reply.status, 404,
            "case unsubmitted attempt {attempt}: GetUseCaseForModelAccess must answer 404, \
             got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case unsubmitted attempt {attempt}: the refusal must be the modelled \
             ResourceNotFoundException, got {:?}",
            reply.error
        );
        assert_eq!(
            reply.get("formData"),
            Value::Null,
            "case unsubmitted attempt {attempt}: a refusal reports no form, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn every_form_is_reported_back_exactly_as_it_was_put() {
    let emulator = emulator();
    for case in cases() {
        let put_reply = put(&emulator, &case.form_data).await;
        assert_eq!(
            put_reply.status, 201,
            "case {}: PutUseCaseForModelAccess must answer 201, got {} {:?}",
            case.name, put_reply.status, put_reply.body
        );

        let reply = get(&emulator).await;
        assert_eq!(
            reply.status, 200,
            "case {}: GetUseCaseForModelAccess must answer 200, got {} {:?}",
            case.name, reply.status, reply.body
        );
        let got = blob_bytes(case.name, &reply, "formData");
        assert_eq!(
            got, case.form_data,
            "case {}: the stored form must be reported byte for byte; got {} bytes, expected {}",
            case.name,
            got.len(),
            case.form_data.len()
        );

        // Reading twice changes nothing: this operation is @readonly.
        let again = get(&emulator).await;
        let got_again = blob_bytes(case.name, &again, "formData");
        assert_eq!(
            got_again, case.form_data,
            "case {}: a second read must report the same form; got {} bytes, expected {}",
            case.name,
            got_again.len(),
            case.form_data.len()
        );
    }
}

#[tokio::test]
async fn putting_the_same_form_twice_leaves_the_same_form() {
    let emulator = emulator();
    for case in cases() {
        for attempt in 1..=2 {
            let put_reply = put(&emulator, &case.form_data).await;
            assert_eq!(
                put_reply.status, 201,
                "case {} attempt {attempt}: the put must answer 201, got {} {:?}",
                case.name, put_reply.status, put_reply.body
            );
        }
        let reply = get(&emulator).await;
        let got = blob_bytes(case.name, &reply, "formData");
        assert_eq!(
            got, case.form_data,
            "case {}: a put applied twice must leave one form; got {} bytes, expected {}",
            case.name,
            got.len(),
            case.form_data.len()
        );
    }
}

#[tokio::test]
async fn a_form_outside_the_published_size_bounds_is_refused_and_changes_nothing() {
    // Each row drives the form past a bound its Smithy model declares; the bounds
    // themselves are exercised in `cases()`.
    let refused: Vec<(String, Vec<u8>)> = vec![
        (
            format!("one byte under @length min {MIN_FORM_BYTES}"),
            filler_bytes(b'a', MIN_FORM_BYTES - 1),
        ),
        ("an empty form".to_string(), Vec::new()),
        (
            format!("one byte past @length max {MAX_FORM_BYTES}"),
            filler_bytes(b'a', MAX_FORM_BYTES + 1),
        ),
        (
            format!("far past @length max {MAX_FORM_BYTES}"),
            filler_bytes(b'a', MAX_FORM_BYTES * 2),
        ),
    ];

    let emulator = emulator();
    // A form that IS admitted, so each refusal is checked against a known stored form
    // rather than against an empty store.
    let stored = b"{\"useCase\":\"text generation\"}".to_vec();
    let put_reply = put(&emulator, &stored).await;
    assert_eq!(
        put_reply.status, 201,
        "the baseline put must answer 201, got {} {:?}",
        put_reply.status, put_reply.body
    );

    for (case, form_data) in refused {
        let reply = put(&emulator, &form_data).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: the put must be refused 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case}: a constraint violation is a ValidationException, got {:?}",
            reply.error
        );
        let after = get(&emulator).await;
        let got = blob_bytes(&case, &after, "formData");
        assert_eq!(
            got, stored,
            "case {case}: a refused put must store nothing; got {} bytes, expected {}",
            got.len(),
            stored.len()
        );
    }

    // The member is @required: a request carrying no formData is refused the same way,
    // and stores nothing either.
    let missing = emulator.call("POST", USE_CASE_URI, Some(json!({}))).await;
    assert_eq!(
        missing.status, 400,
        "case missing formData: the put must be refused 400, got {} {:?}",
        missing.status, missing.body
    );
    let after = get(&emulator).await;
    let got = blob_bytes("missing formData", &after, "formData");
    assert_eq!(
        got, stored,
        "case missing formData: a refused put must store nothing; got {} bytes, expected {}",
        got.len(),
        stored.len()
    );
}
