// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how much metadata ONE vector may carry, served over the real
//! `restJson1` wire through the router production uses.
//!
//! THE RULE: a vector's metadata is bounded by two SEPARATE per-vector budgets — the
//! FILTERABLE part, every key the index does not declare in
//! `MetadataConfiguration$nonFilterableMetadataKeys`, has the smaller one, and the whole
//! document, filterable and non-filterable together, the larger — and `PutVectors` refuses
//! a vector over either with `ValidationException` (HTTP 400), storing nothing of the
//! batch. Size is counted over the document as a whole rather than per key, so several
//! small filterable keys that together cross the budget are refused exactly as one big one
//! is, and declaring a key non-filterable takes its bytes out of the smaller budget and
//! leaves them in the larger — which is what that declaration is for.
//!
//! The rows span what the rule generalises over: which budget is at stake, whether the key
//! carrying the bytes is declared non-filterable, the size (far under, exactly at each
//! published bound, one byte past it, and far over), how the bytes are spread (one key,
//! several keys, a nested document, an array), and whether the index declares any
//! non-filterable keys at all.

use serde_json::{json, Value};
use service_com_amazonaws_s3vectors_s3vectors::state::metadata_budget::{
    MAX_FILTERABLE_METADATA_BYTES, MAX_TOTAL_METADATA_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{call, error_code, ok, router};

/// The bucket both indexes live in.
const BUCKET: &str = "metadata-budget-bucket";

/// The index that declares one metadata key non-filterable, and the key it declares.
const DECLARED_INDEX: &str = "metadata-budget-declared";
const DECLARED_KEY: &str = "blob";

/// The index that declares none, where every metadata key is filterable.
const PLAIN_INDEX: &str = "metadata-budget-plain";

/// A key the neither index declares, so its bytes always count against the filterable
/// budget.
const FILTERABLE_KEY: &str = "note";

/// A router holding the bucket and both indexes.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": BUCKET }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": DECLARED_INDEX,
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "cosine",
            "metadataConfiguration": { "nonFilterableMetadataKeys": [DECLARED_KEY] },
        }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": PLAIN_INDEX,
            "dataType": "float32",
            "dimension": 2,
            "distanceMetric": "cosine",
        }),
    )
    .await;
    router
}

/// How many bytes the JSON of a one-member object costs BESIDES the member's value: the
/// braces, the quoted key and the colon after it.
fn object_overhead(key: &str) -> usize {
    // `{` `}` + `"` key `"` + `:` + the value's own two quotes.
    2 + key.len() + 2 + 1 + 2
}

/// A metadata document `{ key: "x…" }` whose JSON is exactly `bytes` long.
fn metadata_of_size(key: &str, bytes: usize) -> Value {
    let filler = "x".repeat(bytes - object_overhead(key));
    json!({ key: filler })
}

/// `PutVectors` of one vector carrying `metadata` into `index`.
async fn put(router: &MultiServiceRouter, index: &str, key: &str, metadata: &Value) -> (u16, Value) {
    call(
        router,
        "PutVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": index,
            "vectors": [{
                "key": key,
                "data": { "float32": [1.0, 0.0] },
                "metadata": metadata,
            }],
        }),
    )
    .await
}

/// One row: the index it is put into, the document, and whether the put must be refused.
struct BudgetCase {
    case: String,
    index: &'static str,
    metadata: Value,
    refused: bool,
}

/// The rows.
fn budget_cases() -> Vec<BudgetCase> {
    // A value whose own JSON is this long sits exactly at the filterable budget; one byte
    // more is one byte past it.
    let at_filterable = MAX_FILTERABLE_METADATA_BYTES;
    let past_filterable = MAX_FILTERABLE_METADATA_BYTES + 1;
    // The ~8 KiB value the recorded case put under an undeclared key.
    let far_over_filterable = 8 * 1024;
    vec![
        BudgetCase {
            case: "a small filterable value".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({ FILTERABLE_KEY: "scifi" }),
            refused: false,
        },
        BudgetCase {
            case: "no metadata at all".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({}),
            refused: false,
        },
        BudgetCase {
            case: format!("a filterable value exactly at the {at_filterable}-byte budget"),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(FILTERABLE_KEY, at_filterable),
            refused: false,
        },
        BudgetCase {
            case: format!("a filterable value one byte past the {at_filterable}-byte budget"),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(FILTERABLE_KEY, past_filterable),
            refused: true,
        },
        BudgetCase {
            case: format!("a {far_over_filterable}-byte value under an UNDECLARED key"),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(FILTERABLE_KEY, far_over_filterable),
            refused: true,
        },
        BudgetCase {
            case: format!("the same {far_over_filterable} bytes under the DECLARED key"),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(DECLARED_KEY, far_over_filterable),
            refused: false,
        },
        BudgetCase {
            case: "the declared key's name on an index that declares nothing".to_string(),
            index: PLAIN_INDEX,
            metadata: metadata_of_size(DECLARED_KEY, far_over_filterable),
            refused: true,
        },
        BudgetCase {
            case: "the budget counted over the whole document, not per key".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({
                "one": "y".repeat(MAX_FILTERABLE_METADATA_BYTES / 2),
                "two": "z".repeat(MAX_FILTERABLE_METADATA_BYTES / 2),
            }),
            refused: true,
        },
        BudgetCase {
            case: "the declared key's bytes left out of the sum".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({
                FILTERABLE_KEY: "y".repeat(MAX_FILTERABLE_METADATA_BYTES / 2),
                DECLARED_KEY: "z".repeat(MAX_FILTERABLE_METADATA_BYTES),
            }),
            refused: false,
        },
        BudgetCase {
            case: "a NESTED filterable document over the budget".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({
                FILTERABLE_KEY: { "inner": { "deeper": "y".repeat(MAX_FILTERABLE_METADATA_BYTES) } },
            }),
            refused: true,
        },
        BudgetCase {
            case: "a filterable ARRAY over the budget".to_string(),
            index: DECLARED_INDEX,
            metadata: json!({
                FILTERABLE_KEY: vec!["y".repeat(MAX_FILTERABLE_METADATA_BYTES / 4); 8],
            }),
            refused: true,
        },
        BudgetCase {
            case: format!(
                "a declared value exactly at the {MAX_TOTAL_METADATA_BYTES}-byte total budget"
            ),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(DECLARED_KEY, MAX_TOTAL_METADATA_BYTES),
            refused: false,
        },
        BudgetCase {
            case: format!(
                "a declared value one byte past the {MAX_TOTAL_METADATA_BYTES}-byte total budget"
            ),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(DECLARED_KEY, MAX_TOTAL_METADATA_BYTES + 1),
            refused: true,
        },
        BudgetCase {
            case: "a declared value far past the total budget".to_string(),
            index: DECLARED_INDEX,
            metadata: metadata_of_size(DECLARED_KEY, 2 * MAX_TOTAL_METADATA_BYTES),
            refused: true,
        },
    ]
}

/// Each document is accepted exactly when it fits both budgets of the index it is put
/// into, and refused with the modelled 400 when it does not.
#[tokio::test]
async fn a_vectors_metadata_is_accepted_exactly_when_it_fits_both_budgets() {
    let router = fixture().await;
    for (position, row) in budget_cases().into_iter().enumerate() {
        let BudgetCase {
            case,
            index,
            metadata,
            refused,
        } = row;
        let vector_key = format!("v{position}");
        let (status, body) = put(&router, index, &vector_key, &metadata).await;
        if refused {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: must be refused, got {status} {body}"
            );
            // The refusal says WHICH budget was crossed and what its bound is, so a client
            // can tell the two apart and knows what to trim for; the bound is the
            // published one, and the record it is about is named by its own key.
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&format!("'{vector_key}'"))
                    && message.contains("size exceeds the maximum allowed size of"),
                "case {case:?}: the refusal must name the record and the budget, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: must be accepted, got {status} {body}"
            );
            // A stored vector reads back with the metadata it was stored with.
            let reported = ok(
                &router,
                "GetVectors",
                &json!({
                    "vectorBucketName": BUCKET,
                    "indexName": index,
                    "keys": [vector_key],
                    "returnMetadata": true,
                }),
            )
            .await;
            assert_eq!(
                reported["vectors"][0]["metadata"], metadata,
                "case {case:?}: the stored metadata must read back, got {reported}"
            );
        }
    }
}

/// A refused put stores NOTHING of its batch: the vector it would have written is not in
/// the index afterwards, and a vector the same batch carried beside it is not either.
#[tokio::test]
async fn a_refused_put_stores_nothing_of_its_batch() {
    let router = fixture().await;
    let over_budget = metadata_of_size(FILTERABLE_KEY, 8 * 1024);
    let (status, body) = call(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": DECLARED_INDEX,
            "vectors": [
                { "key": "fits", "data": { "float32": [1.0, 0.0] } },
                {
                    "key": "over",
                    "data": { "float32": [0.0, 1.0] },
                    "metadata": over_budget,
                },
            ],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "the batch must be refused, got {status} {body}"
    );
    // The refusal names the element of `vectors` the over-budget record travelled in, and
    // the key the caller submitted it under.
    assert_eq!(
        body["fieldList"][0]["path"].as_str(),
        Some("vectors[1]"),
        "the refusal must name the record's element, got {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("'over'"),
        "the refusal must name the record's key, got {body}"
    );

    let listed = ok(
        &router,
        "ListVectors",
        &json!({ "vectorBucketName": BUCKET, "indexName": DECLARED_INDEX }),
    )
    .await;
    assert_eq!(
        listed["vectors"],
        json!([]),
        "the refused batch must have stored nothing, got {listed}"
    );
}
