// SPDX-License-Identifier: CC0-1.0

//! The REGION segment of every ARN this service mints, over the wire.
//!
//! An invocation ARN is minted from the region the request signed for, and it is handed
//! back in a member the model constrains (`invocationArn`, `@length(1..128)` and its
//! pattern) and read back through members carrying the same constraints
//! (`GetDataAutomationStatus$invocationArn`, the tagging operations' `resourceARN`). So
//! the rule under test is an invariant of the mint rather than a property of one signed
//! region: whatever a caller signs for, the minted ARN stays inside the modelled bounds
//! and the record it names is readable back through the operations that resolve it.
//!
//! The rows differ along the region label itself — its length (including the published
//! bound for the field and one character past it), its case, its charset, and its
//! absence — because that is the only part of a minted ARN a request can influence. A
//! label the grammar admits is the ARN's region; anything else is not spliced into an ARN
//! at all and the service's own `DEFAULT_REGION` answers instead.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use smithy_server_runtime::MultiServiceRouter;

/// This service's sigv4 signing name (`OPERATIONS.md`): the credential scope the router
/// routes on.
const SIGNING_NAME: &str = "bedrock";

/// A legal `dataAutomationProfileArn` (pattern from `smithy_query` shape
/// `DataAutomationProfileArn`, whose account segment accepts the literal `aws`).
const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// The data-automation project every submission below names, seeded in the store of the
/// build-time service that owns projects (pattern from `smithy_query` shape
/// `DataAutomationProjectArn`).
const PROJECT_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/invoices";

/// The modelled `InvocationArn` `@length` bounds (`smithy_query` shape `InvocationArn`,
/// restated at `OPERATIONS.md:55` and `:61`) — carried by the output member this
/// operation answers with AND by the input members that read the record back.
const INVOCATION_ARN_LENGTH: std::ops::RangeInclusive<usize> = 1..=128;

/// The longest region label an ARN's region field carries, from the grammar
/// `service_common::arn::region_label` states: every AWS model's `*Arn` pattern bounds
/// that field at `[a-z0-9-]{1,20}`, so a longer value is not a region any endpoint has.
const MAX_REGION_LABEL_LENGTH: usize = 20;

/// One row: the case name, the region the request signs for, and the region segment the
/// minted ARN must then carry.
///
/// A request carrying NO parsable credential scope is not a row here: the router resolves
/// a request's routing key from that scope, so such a request reaches no service at all
/// and never gets as far as minting. Its fallback is covered by the mint sweep below,
/// which calls the mint with the empty region that case hands it.
type RegionCase = (&'static str, String, String);

/// A region label of exactly `length` characters, inside the `[a-z0-9-]` charset.
fn region_of_length(length: usize) -> String {
    let mut label = String::from("us-");
    while label.len() < length {
        label.push('a');
    }
    label.truncate(length);
    label
}

/// A router serving only this service over one shared engine, with [`PROJECT_ARN`]
/// arranged in the build-time store a submission's project reference resolves against.
fn service() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "data-automation-invoices");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(engine));
    router
}

/// One wire answer: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

/// Call one operation the way an SDK would, signing for `region`.
async fn call(router: &MultiServiceRouter, region: &str, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// Check `arn` against the modelled `InvocationArn` shape — the pattern
/// `^arn:aws(|-cn|-iso|-iso-[a-z]|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:data-automation-invocation/[a-zA-Z0-9-_]+$`
/// with `region` in its region field, and `@length(1..128)`. Checked structurally, so the
/// test needs no regex engine.
fn assert_invocation_arn(case: &str, arn: &str, region: &str) {
    let prefix = format!(
        "arn:aws:bedrock:{region}:{}:data-automation-invocation/",
        emulator_server::EMULATOR_ACCOUNT_ID
    );
    let Some(id) = arn.strip_prefix(prefix.as_str()) else {
        panic!("case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}");
    };
    assert!(
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "case {case:?}: the resource id must match [a-zA-Z0-9-_]+, got {id:?}"
    );
    assert!(
        INVOCATION_ARN_LENGTH.contains(&arn.len()),
        "case {case:?}: InvocationArn is @length({}..{}), got {} chars in {arn:?}",
        INVOCATION_ARN_LENGTH.start(),
        INVOCATION_ARN_LENGTH.end(),
        arn.len()
    );
}

/// The rows: what a request can sign for, and the region the minted ARN must carry.
fn rows() -> Vec<RegionCase> {
    let default = bda::state::DEFAULT_REGION.to_string();
    vec![
        (
            "a deployed region label",
            "us-west-2".to_string(),
            "us-west-2".to_string(),
        ),
        (
            "a longer label carrying a digit",
            "ap-southeast-4".to_string(),
            "ap-southeast-4".to_string(),
        ),
        ("a one-character label", "a".to_string(), "a".to_string()),
        (
            "a label at the published region bound",
            region_of_length(MAX_REGION_LABEL_LENGTH),
            region_of_length(MAX_REGION_LABEL_LENGTH),
        ),
        (
            "one character past the published region bound",
            region_of_length(MAX_REGION_LABEL_LENGTH + 1),
            default.clone(),
        ),
        (
            "a label long enough to push the minted ARN past its modelled length",
            region_of_length(39),
            default.clone(),
        ),
        (
            "a label far past every bound",
            "a".repeat(400),
            default.clone(),
        ),
        (
            "an upper-case label",
            "US-WEST-2".to_string(),
            default.clone(),
        ),
        (
            "a label outside the region charset",
            "us_west_2".to_string(),
            default,
        ),
    ]
}

#[tokio::test]
async fn a_minted_invocation_arn_is_readable_back_whatever_region_was_signed_for() {
    let router = service();

    let mut minted: Vec<String> = Vec::new();
    for (index, (case, signed, want_region)) in rows().into_iter().enumerate() {
        let output_s3_uri = format!("s3://out-bucket/{index}/");
        let body = json!({
            "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
            "outputConfiguration": {"s3Uri": output_s3_uri},
            "dataAutomationProfileArn": PROFILE_ARN,
            "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
        });
        let answer = call(&router, &signed, "InvokeDataAutomationAsync", body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from InvokeDataAutomationAsync, got {} / {}",
            answer.status, answer.body
        );
        let arn = answer.body["invocationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no invocationArn in {}", answer.body))
            .to_string();
        assert_invocation_arn(case, &arn, &want_region);
        minted.push(arn.clone());

        // The record the minted ARN names is readable back through the input member that
        // carries the SAME modelled bounds: a job whose status no caller can ever read is
        // a job that was accepted for nothing.
        let status = call(
            &router,
            &signed,
            "GetDataAutomationStatus",
            json!({"invocationArn": arn}),
        )
        .await;
        assert_eq!(
            status.status, 200,
            "case {case:?}: expected 200 from GetDataAutomationStatus for {arn:?}, got {} / {}",
            status.status, status.body
        );
        assert_eq!(
            status.body["outputConfiguration"]["s3Uri"].as_str(),
            Some(crate::support::job_metadata_uri(&output_s3_uri, &arn).as_str()),
            "case {case:?}: the reported outputConfiguration must be this record's own, got {}",
            status.body
        );

        // The tagging operations resolve the same ARN, so one record is addressable by
        // every operation that takes it rather than by some of them.
        let tags = call(
            &router,
            &signed,
            "ListTagsForResource",
            json!({"resourceARN": arn}),
        )
        .await;
        assert_eq!(
            tags.status, 200,
            "case {case:?}: expected 200 from ListTagsForResource for {arn:?}, got {} / {}",
            tags.status, tags.body
        );
    }

    let mut distinct = minted.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "each submission must mint its own invocationArn, got {minted:?}"
    );
}

#[test]
fn the_mint_bounds_the_region_segment_for_every_label_length() {
    let mut store =
        bda::state::ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::default();
    let id = store.mint_invocation_id(SystemTime::now());
    let default = bda::state::DEFAULT_REGION;

    // A sweep over every label length a caller could sign for, the one dimension that
    // decides whether the minted ARN still fits the member it is returned in.
    for length in 1..=400 {
        let candidate = region_of_length(length);
        let case = format!("a region label of {length} characters");
        let arn = bda::state::invocation_arn(&candidate, &id);
        let want_region = if length <= MAX_REGION_LABEL_LENGTH {
            candidate.as_str()
        } else {
            default
        };
        assert_invocation_arn(&case, &arn, want_region);
    }

    // The same bound holds for a label the charset rejects at any length.
    for candidate in ["US-WEST-2", "us_west_2", "us west 2", "us-west-2/", ""] {
        let case = format!("the label {candidate:?}");
        let arn = bda::state::invocation_arn(candidate, &id);
        assert_invocation_arn(&case, &arn, default);
    }
}
