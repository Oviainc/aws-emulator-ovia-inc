// SPDX-License-Identifier: CC0-1.0

//! The FORM of the invocation id this service mints, over the wire.
//!
//! A data automation job is named by a UUID, so the resource segment of the
//! `invocationArn` an accepted submission answers with — and the segment the job's output
//! lands under beneath the submitted `outputConfiguration.s3Uri` — is a UUID in its
//! canonical lower-case `8-4-4-4-12` hexadecimal form. A client (or a fixture) that parses
//! the id out of the ARN, or derives the job's output prefix from it, therefore sees the
//! same shape it sees against AWS.
//!
//! The rule is a property of the MINT, not of any one request, so every assertion below
//! runs over a table whose rows differ along everything a submission can carry — the input
//! asset, the output prefix (including a bucket root with no prefix at all), the extraction
//! target's stage, a repeated `clientToken`, tags, notification and encryption
//! configuration — and along the region the request signs for. The id is a function of none
//! of them.
//!
//! The form itself is spelled out here, in the test, which is where restating a layout is
//! the point: five hyphen-separated groups of 8-4-4-4-12 lower-case hexadecimal digits, the
//! version digit `4` and the variant digit one of `8`, `9`, `a`, `b`. The sweep over the
//! store's own mint adds the invariants the emulator owns: two mints never agree — not even
//! two at the identical instant — and consecutive ids are independent rather than differing
//! only in a trailing counter field.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

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

/// The resource prefix every invocation ARN carries, from the modelled `InvocationArn`
/// pattern (`smithy_query` shape `InvocationArn`): the id under test is the segment after
/// it.
const INVOCATION_RESOURCE_PREFIX: &str = "data-automation-invocation/";

/// One row: the case name, the region the request signs for, the submission body, and the
/// output prefix that submission names.
struct MintCase {
    label: &'static str,
    region: &'static str,
    body: Value,
    output_prefix: &'static str,
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

/// Why `id` is not a UUID in canonical text form, or `None` when it is.
///
/// Five hyphen-separated groups of 8-4-4-4-12 lower-case hexadecimal digits, the version
/// digit (the first digit of the third group) `4`, and the variant digit (the first of the
/// fourth group) one of `8`, `9`, `a`, `b`.
fn not_a_canonical_uuid(id: &str) -> Option<String> {
    let groups: Vec<&str> = id.split('-').collect();
    if !groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12]) {
        return Some(format!("not five groups of 8-4-4-4-12, got {groups:?}"));
    }
    if !groups.iter().all(|group| {
        group
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    }) {
        return Some("not lower-case hexadecimal throughout".to_string());
    }
    let version = groups[2].chars().next();
    if version != Some('4') {
        return Some(format!("version digit is {version:?}, not '4'"));
    }
    let variant = groups[3].chars().next();
    if !matches!(variant, Some('8' | '9' | 'a' | 'b')) {
        return Some(format!(
            "variant digit is {variant:?}, not one of '8', '9', 'a', 'b'"
        ));
    }
    None
}

/// Assert `id` is a canonical UUID, naming the case and the id when it is not.
fn assert_canonical_uuid(case: &str, id: &str) {
    assert!(
        not_a_canonical_uuid(id).is_none(),
        "case {case:?}: the minted invocation id must be a canonical UUID, got {id:?}: {}",
        not_a_canonical_uuid(id).unwrap_or_default()
    );
}

/// The id inside a minted invocation ARN, asserted to be addressed through the modelled
/// resource prefix.
fn id_of(case: &str, arn: &str) -> String {
    let (_, id) = arn
        .split_once(INVOCATION_RESOURCE_PREFIX)
        .unwrap_or_else(|| panic!("case {case:?}: no {INVOCATION_RESOURCE_PREFIX:?} in {arn:?}"));
    id.to_string()
}

/// The table: what a submission can carry, and where its output goes. The minted id is a
/// function of none of it.
fn mint_cases() -> Vec<MintCase> {
    vec![
        MintCase {
            label: "the required members plus the project that says what to extract",
            region: "us-west-2",
            body: json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/plain/"},
                "dataAutomationProfileArn": PROFILE_ARN,
                "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
            }),
            output_prefix: "s3://out-bucket/plain/",
        },
        MintCase {
            label: "an output uri that is a bucket root, in another region",
            region: "eu-central-1",
            body: json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/scan.tiff"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket"},
                "dataAutomationProfileArn": PROFILE_ARN,
                "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
            }),
            output_prefix: "s3://out-bucket",
        },
        MintCase {
            label: "a deeply nested prefix, an explicit stage and three tags",
            region: "ap-southeast-2",
            body: json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/clip.mp4"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/deep/nested/prefix/"},
                "dataAutomationProfileArn": PROFILE_ARN,
                "dataAutomationConfiguration": {
                    "dataAutomationProjectArn": PROJECT_ARN,
                    "stage": "LIVE",
                },
                "tags": [
                    {"key": "team", "value": "search"},
                    {"key": "env", "value": "prod"},
                    {"key": "cost-center", "value": "4711"},
                ],
            }),
            output_prefix: "s3://out-bucket/deep/nested/prefix/",
        },
        MintCase {
            label: "an inline blueprint instead of a project, with encryption",
            region: "us-east-1",
            body: json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/invoice.png"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/blueprint/"},
                "dataAutomationProfileArn": PROFILE_ARN,
                "blueprints": [{
                    "blueprintArn": "arn:aws:bedrock:us-west-2:aws:blueprint/bedrock-data-automation-public-video",
                    "stage": "LIVE",
                }],
                "encryptionConfiguration": {"kmsKeyId": "alias/data-automation"},
            }),
            output_prefix: "s3://out-bucket/blueprint/",
        },
        MintCase {
            label: "a repeated clientToken, which deduplicates nothing",
            region: "us-west-2",
            body: json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/token/"},
                "dataAutomationProfileArn": PROFILE_ARN,
                "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
                "clientToken": "a-stable-token",
            }),
            output_prefix: "s3://out-bucket/token/",
        },
        MintCase {
            label: "the same clientToken and the same payload a second time",
            region: "us-west-2",
            body: json!({
                "inputConfiguration": {"s3Uri": "s3://in-bucket/assets/doc.pdf"},
                "outputConfiguration": {"s3Uri": "s3://out-bucket/token/"},
                "dataAutomationProfileArn": PROFILE_ARN,
                "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
                "clientToken": "a-stable-token",
            }),
            output_prefix: "s3://out-bucket/token/",
        },
    ]
}

#[tokio::test]
async fn every_submission_is_named_by_a_canonical_uuid_in_its_arn_and_its_output_prefix() {
    let router = service();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for MintCase {
        label,
        region,
        body,
        output_prefix,
    } in mint_cases()
    {
        let answer = call(&router, region, "InvokeDataAutomationAsync", body).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200 from InvokeDataAutomationAsync, got {} / {}",
            answer.status, answer.body
        );
        let arn = answer.body["invocationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: no invocationArn in {}", answer.body))
            .to_string();

        let id = id_of(label, &arn);
        assert_canonical_uuid(label, &id);
        assert!(
            minted.insert(id.clone()),
            "case {label:?}: the id {id:?} was already minted for another job"
        );

        // The SAME id is the segment this job's output lands under, so a client that
        // derives the output prefix from the ARN addresses the same location AWS does.
        let status = call(
            &router,
            region,
            "GetDataAutomationStatus",
            json!({"invocationArn": arn}),
        )
        .await;
        assert_eq!(
            status.status, 200,
            "case {label:?}: expected 200 from GetDataAutomationStatus for {arn:?}, got {} / {}",
            status.status, status.body
        );
        let reported = status.body["outputConfiguration"]["s3Uri"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            reported,
            format!("{}/{id}/job_metadata.json", output_prefix.trim_end_matches('/')),
            "case {label:?}: the reported output location must sit under this job's own UUID, \
             got {reported:?}"
        );
    }
}

#[test]
fn consecutive_mints_are_independent_canonical_uuids() {
    let mut store =
        bda::state::ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::default();

    // The instants a mint can be asked for: the epoch itself, a submission now, two
    // submissions landing on the IDENTICAL instant (which only the store's own mint count
    // separates), and instants far apart. The id's form is a function of none of them.
    let now = SystemTime::now();
    let instants: Vec<(String, SystemTime)> = vec![
        ("the unix epoch".to_string(), SystemTime::UNIX_EPOCH),
        ("one nanosecond past the epoch".to_string(), SystemTime::UNIX_EPOCH + Duration::from_nanos(1)),
        ("now".to_string(), now),
        ("the identical instant again".to_string(), now),
        ("the identical instant a third time".to_string(), now),
        ("a microsecond later".to_string(), now + Duration::from_micros(1)),
        ("an hour later".to_string(), now + Duration::from_secs(3_600)),
        ("a decade later".to_string(), now + Duration::from_secs(10 * 365 * 86_400)),
    ];

    let mut ids: Vec<String> = Vec::new();
    for (case, at) in &instants {
        let id = store.mint_invocation_id(*at);
        assert_canonical_uuid(case, &id);
        assert!(
            !ids.contains(&id),
            "case {case:?}: the id {id:?} was already minted for another job"
        );
        ids.push(id);
    }

    // Independent ids, not one id with a counter spliced into its tail: consecutive mints
    // differ in their leading group, the 32 bits an id built around a timestamp would
    // share between two mints a nanosecond apart.
    for pair in ids.windows(2) {
        let (first, second) = (&pair[0], &pair[1]);
        let leading = |id: &String| id.split('-').next().unwrap_or_default().to_string();
        assert_ne!(
            leading(first),
            leading(second),
            "consecutive mints must be independent UUIDs, got {first:?} then {second:?}"
        );
    }
}
