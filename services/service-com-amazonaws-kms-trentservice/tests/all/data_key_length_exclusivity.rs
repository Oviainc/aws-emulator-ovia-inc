// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a data key request states its LENGTH twice, driven over the
//! wire through the shared [`super::harness`].
//!
//! The rule under test: `KeySpec` and `NumberOfBytes` are two ways of stating ONE
//! length — "you must specify either the KeySpec or the NumberOfBytes parameter (but not
//! both)"
//! (`smithy_query(query="doc",shape="GenerateDataKeyRequest",member="KeySpec")`) — so a
//! request that does not state the length exactly once, by carrying both members or
//! neither, is refused `ValidationException` with the service's own sentence for that
//! pair, which is fixed text naming neither value, and nothing is generated.
//!
//! The rows differ along what the rule generalises over: the operation that takes the
//! pair (`GenerateDataKey` and `GenerateDataKeyWithoutPlaintext`, the two that do), the
//! `DataKeySpec` the request names (every value the enum declares), the `NumberOfBytes`
//! it names (both published bounds of the modelled range and the length the named spec
//! would have given, so a pair that agrees about the length is refused exactly like one
//! that does not), and the order the members appear in the request body.
//!
//! Which requests are SERVED, and how long the data key they answer is, is
//! [`super::data_key`]'s rule; this suite is only about the refusal and its wording.

use serde_json::json;

use super::harness::{call, enabled_key, error_code, error_message, ok, text, REGION};

/// The whole of the message KMS answers a data key request that does not state its
/// length exactly once: fixed text with no variable parts, written out here so the
/// assertion is about AWS's sentence rather than about the one the handler holds.
const EXCLUSIVE_LENGTH_MESSAGE: &str = "Please specify either number of bytes or key spec.";

/// The operations that take both members (`smithy_query(query="digest")`: the two data
/// key operations whose input carries `KeySpec` and `NumberOfBytes`).
const OPERATIONS: [&str; 2] = ["GenerateDataKey", "GenerateDataKeyWithoutPlaintext"];

/// Every `DataKeySpec` value (`smithy_query(query="shape",shape="DataKeySpec")`), with
/// the number of bytes it names: "use AES_128 to generate a 128-bit symmetric key, or
/// AES_256 to generate a 256-bit symmetric key"
/// (`smithy_query(query="doc",shape="GenerateDataKeyRequest",member="KeySpec")`).
const KEY_SPECS: [(&str, i64); 2] = [("AES_128", 16), ("AES_256", 32)];

/// `NumberOfBytesType`'s published bounds (`@range(1..1024)`), which a row names beside
/// a `KeySpec` so the refusal cannot depend on the value being out of range.
const NUMBER_OF_BYTES_BOUNDS: [i64; 2] = [1, 1024];

/// A request that does not state its length exactly once is refused with the service's
/// sentence for that pair, and answers no data key at all.
#[tokio::test]
async fn a_request_that_does_not_state_one_length_is_refused() {
    let (router, key_id) = router_with_key().await;

    for operation in OPERATIONS {
        // The request that states NO length at all: the same rule, missed the other way,
        // and the same sentence asking for the one length it did not state.
        let case = format!("{operation} with neither KeySpec nor NumberOfBytes");
        let (status, payload) = call(&router, REGION, operation, &json!({"KeyId": key_id})).await;
        assert_eq!(
            (
                status,
                error_code(&payload).as_str(),
                error_message(&payload).as_str()
            ),
            (
                400,
                "ValidationException",
                EXCLUSIVE_LENGTH_MESSAGE
            ),
            "case {case:?}: expected the service's sentence for the pair, got {status} \
             with {payload}"
        );

        for (key_spec, spec_bytes) in KEY_SPECS {
            // Rows: the byte count named beside the spec — each published bound of the
            // range, and the count the spec itself names, which is the pair that agrees
            // about the length and is refused all the same.
            let counts: Vec<i64> = NUMBER_OF_BYTES_BOUNDS
                .into_iter()
                .chain(std::iter::once(spec_bytes))
                .collect();
            for number_of_bytes in counts {
                // Rows: the order the two members appear in, which a JSON object does
                // not fix and which nothing about the refusal may depend on.
                for spec_first in [true, false] {
                    let case = format!(
                        "{operation} with KeySpec {key_spec} and NumberOfBytes \
                         {number_of_bytes}, spec first: {spec_first}"
                    );
                    let mut body = json!({"KeyId": key_id});
                    if spec_first {
                        body["KeySpec"] = json!(key_spec);
                        body["NumberOfBytes"] = json!(number_of_bytes);
                    } else {
                        body["NumberOfBytes"] = json!(number_of_bytes);
                        body["KeySpec"] = json!(key_spec);
                    }

                    let (status, payload) = call(&router, REGION, operation, &body).await;
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400, got {status} with {payload}"
                    );
                    assert_eq!(
                        error_code(&payload),
                        "ValidationException",
                        "case {case:?}: stating the length twice is a malformed request, \
                         got {payload}"
                    );
                    assert_eq!(
                        error_message(&payload),
                        EXCLUSIVE_LENGTH_MESSAGE,
                        "case {case:?}: expected the service's sentence for the pair, \
                         got {:?}",
                        error_message(&payload)
                    );
                    assert!(
                        payload.get("Plaintext").is_none()
                            && payload.get("CiphertextBlob").is_none(),
                        "case {case:?}: a refusal carries no data key, got {payload}"
                    );
                }
            }
        }
    }
}

/// The control: the same requests with ONE of the two members are served, so the
/// refusal is about the pair and not about either member.
#[tokio::test]
async fn either_member_alone_is_served() {
    let (router, key_id) = router_with_key().await;

    for operation in OPERATIONS {
        for (key_spec, spec_bytes) in KEY_SPECS {
            for (case, length) in [
                (format!("{operation} with KeySpec {key_spec}"), json!({"KeySpec": key_spec})),
                (
                    format!("{operation} with NumberOfBytes {spec_bytes}"),
                    json!({"NumberOfBytes": spec_bytes}),
                ),
            ] {
                let mut body = json!({"KeyId": key_id});
                for (member, value) in length
                    .as_object()
                    .expect("every row states its length as an object")
                {
                    body[member] = value.clone();
                }
                let answer = ok(&router, operation, &body).await;
                assert!(
                    answer["CiphertextBlob"].is_string(),
                    "case {case:?}: a request stating one length is served, got {answer}"
                );
            }
        }
    }
}

/// A router with one symmetric encryption key, which is what a data key is generated
/// under.
async fn router_with_key() -> (smithy_server_runtime::MultiServiceRouter, String) {
    let router = super::harness::router();
    let key_id = text("setup", &enabled_key(&router).await, "KeyId");
    (router, key_id)
}
