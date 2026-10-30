// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the two request-level ARN-parameter refusals, which share
//! HTTP 400 and `InvalidParameterException` and are therefore told apart only by
//! their message.
//!
//! Two rules, each asserted over a table rather than one recorded request:
//!
//! * a value that cannot be parsed as an ARN is reported by the VALUE alone —
//!   the offending value, then fixed text — so the text is the same for every
//!   unparseable form and for every operation that takes an ARN list;
//! * an ARN whose Region field is not the Region the call was invoked in is
//!   reported by that ARN and the INVOKED OPERATION's name, so the same ARN gets
//!   a different message from `TagResources` than from `UntagResources`, and the
//!   endpoint's own Region is never named.
//!
//! Every test drives the real wire path — the router, `POST /` with an
//! `X-Amz-Target` and an awsJson1_1 body — so what is asserted is what an SDK
//! client sees.

use std::collections::BTreeMap;
use std::sync::Arc;

use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the requests below are signed for unless a case names another one,
/// and therefore the Region the endpoint serves.
const REGION: &str = "us-east-1";

/// An ARN of a service no owning crate resolves, so it needs no live resource
/// behind it: this service's own store is its tag home. The control ARN every
/// refused batch carries beside its bad value.
const UNOWNED: &str = "arn:aws:s3:::wording-control-bucket";

/// A router carrying only this service, over its own fresh engine, so each test
/// starts from an empty store and observes the same state across requests.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler behaviour, not SigV4: the `Authorization`
    // header below is present because it is what routes an awsJson request to
    // the `tagging` signing name, and because its credential scope is what
    // names the Region the endpoint serves.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation, signed for `region`, and return its HTTP status and
/// parsed JSON body.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/tagging/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: serde_json::Value = if response.body().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// [`call_in_region`] against the default [`REGION`] endpoint.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The error code an awsJson error body carries (`__type` minus its namespace).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("an error body with no __type: {body}"));
    raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
}

/// The message an error body carries, under either spelling of the member.
fn error_message(body: &serde_json::Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("an error body with no message: {body}"))
        .to_string()
}

/// The request body of one ARN-list operation over `arns`.
fn request_for(operation: &str, arns: &[&str]) -> serde_json::Value {
    match operation {
        "TagResources" => {
            serde_json::json!({ "ResourceARNList": arns, "Tags": { "Env": "prod" } })
        }
        "UntagResources" => serde_json::json!({ "ResourceARNList": arns, "TagKeys": ["Env"] }),
        "GetResources" => serde_json::json!({ "ResourceARNList": arns }),
        other => panic!("{other} takes no ResourceARNList"),
    }
}

/// The tag map a `GetResources` page reports for one ARN.
fn page_tags(body: &serde_json::Value, arn: &str) -> BTreeMap<String, String> {
    let mapping = body["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {body}"))
        .iter()
        .find(|mapping| mapping["ResourceARN"] == serde_json::json!(arn))
        .unwrap_or_else(|| panic!("{arn} is not in the page: {body}"));
    mapping["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a mapping with no Tags: {mapping}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().expect("Tag.Key").to_string(),
                tag["Value"].as_str().expect("Tag.Value").to_string(),
            )
        })
        .collect()
}

/// Every unparseable form is reported the same way: the offending VALUE first,
/// then the fixed text, with neither the member it arrived in nor the operation
/// named — so the identical message comes back from each operation that takes an
/// ARN list.
///
/// The rows differ along WHY the value is not an ARN (no `arn` scheme, too few
/// fields, an empty required field, surrounding whitespace, a bare prefix,
/// non-ASCII text); none of them is the value the issue cited, which is one more
/// row of the same class.
#[tokio::test]
async fn an_unparseable_arn_list_value_is_reported_by_the_value_and_fixed_text() {
    let router = emulator();

    let malformed: &[(&str, &str)] = &[
        ("no arn scheme", "wording-not-an-arn"),
        ("another scheme", "urn:aws:s3:::bucket-a"),
        ("too few fields", "arn:aws:sqs:us-east-1"),
        ("the bare prefix", "arn"),
        ("an empty resource", "arn:aws:s3:::"),
        ("an empty partition", "arn::s3:::bucket-a"),
        ("an empty service", "arn:aws::::bucket-a"),
        ("a leading space", " arn:aws:s3:::bucket-a"),
        ("an ARN-ish sentence", "this is not arn:aws anything"),
        ("non-ASCII text", "arn\u{e9}:aws:s3:::bucket-a"),
    ];

    for (case, value) in malformed {
        // The ONE template: the value, then AWS's fixed text.
        let expected = format!("{value} is not a valid AmazonResourceName (ARN)");
        for operation in ["TagResources", "UntagResources", "GetResources"] {
            let (status, body) =
                call(&router, operation, request_for(operation, &[UNOWNED, value])).await;
            assert_eq!(
                status, 400,
                "case {case} ({value:?})/{operation}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterException",
                "case {case} ({value:?})/{operation}: got {body}"
            );
            let actual = error_message(&body);
            assert_eq!(
                actual, expected,
                "case {case} ({value:?})/{operation}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// An ARN in another Region is reported by that ARN and the name of the
/// operation it was sent to — not by the endpoint's Region, which the message
/// never states.
///
/// The rows differ along the endpoint's Region, the ARN's Region, the service
/// namespace and the resource form (bare name, `type/name`, `type:name`), and
/// each row is sent to both ARN-list writers, so the operation slot varies too.
/// No row's endpoint Region appears anywhere in its ARN, so a message naming the
/// endpoint cannot pass by coincidence.
#[tokio::test]
async fn a_cross_region_arn_is_reported_by_the_arn_and_the_invoked_operation() {
    let router = emulator();

    let rows: &[(&str, &str, &str)] = &[
        (
            "a queue of the next Region west",
            REGION,
            "arn:aws:sqs:us-west-2:000000000000:wording-far-queue",
        ),
        (
            "a queue seen from a us-west-2 endpoint",
            "us-west-2",
            "arn:aws:sqs:us-east-1:000000000000:wording-far-queue",
        ),
        (
            "a table in another partition's geography",
            "eu-central-1",
            "arn:aws:dynamodb:ap-southeast-2:000000000000:table/wording-far-table",
        ),
        (
            "an instance addressed with a type/id resource",
            "eu-west-1",
            "arn:aws:ec2:ca-central-1:000000000000:instance/i-0abc",
        ),
        (
            "a log group addressed with a type:name resource",
            "ap-northeast-1",
            "arn:aws:logs:sa-east-1:000000000000:log-group:wording-far-group",
        ),
    ];

    for (case, endpoint_region, arn) in rows {
        let mut messages: Vec<String> = Vec::new();
        for operation in ["TagResources", "UntagResources"] {
            let (status, body) = call_in_region(
                &router,
                endpoint_region,
                operation,
                request_for(operation, &[UNOWNED, arn]),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}/{operation}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterException",
                "case {case}/{operation}: got {body}"
            );
            // The template's two variable parts: the ARN as the request wrote
            // it, and the invoked operation.
            let expected = format!(
                "Region in the ARN {arn} does not match with the region in which {operation} \
                 API is invoked"
            );
            let actual = error_message(&body);
            assert_eq!(
                actual, expected,
                "case {case}/{operation}: expected {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(endpoint_region),
                "case {case}/{operation}: the message must not name the endpoint's Region \
                 {endpoint_region}, got {actual:?}"
            );
            messages.push(actual);
        }
        assert_ne!(
            messages[0], messages[1],
            "case {case}: the message names the invoked operation, so the two operations \
             cannot answer the same text, got {messages:?}"
        );
    }
}

/// The two refusals are distinguishable by message alone: they share HTTP 400
/// and `InvalidParameterException`, so a client that tells a malformed value
/// from an out-of-Region one has only the text to go on.
#[tokio::test]
async fn the_two_refusals_differ_only_in_their_message() {
    let router = emulator();

    let malformed = "wording-not-an-arn";
    let cross_region = "arn:aws:sqs:us-west-2:000000000000:wording-far-queue";

    let (malformed_status, malformed_body) = call(
        &router,
        "TagResources",
        request_for("TagResources", &[UNOWNED, malformed]),
    )
    .await;
    let (region_status, region_body) = call(
        &router,
        "TagResources",
        request_for("TagResources", &[UNOWNED, cross_region]),
    )
    .await;

    assert_eq!(
        (malformed_status, error_code(&malformed_body)),
        (region_status, error_code(&region_body)),
        "case both refusals: expected the same status and code, got {malformed_body} \
         and {region_body}"
    );
    assert_ne!(
        error_message(&malformed_body),
        error_message(&region_body),
        "case both refusals: the message is the only discriminator, got {:?} for both",
        error_message(&malformed_body)
    );
    assert!(
        error_message(&malformed_body).starts_with(malformed),
        "case malformed: the message leads with the offending value, got {:?}",
        error_message(&malformed_body)
    );
    assert!(
        error_message(&region_body).contains(cross_region),
        "case cross-Region: the message names the offending ARN, got {:?}",
        error_message(&region_body)
    );
}

/// The wording change refuses nothing new: an ARN this endpoint serves — its own
/// Region, or no Region at all (a global resource) — is still tagged, and a
/// refused batch still leaves the ARNs beside the bad value untouched.
#[tokio::test]
async fn an_arn_this_endpoint_serves_is_still_tagged_and_a_refusal_writes_nothing() {
    let router = emulator();

    // One ARN per accepted form: a global resource (empty Region segment) and a
    // regional one carrying exactly the endpoint's Region.
    let served: &[(&str, &str)] = &[
        ("a global resource", UNOWNED),
        (
            "a resource of the endpoint's own Region",
            "arn:aws:dynamodb:us-east-1:000000000000:table/wording-near-table",
        ),
    ];
    for (case, arn) in served {
        let (status, body) =
            call(&router, "TagResources", request_for("TagResources", &[arn])).await;
        assert_eq!(
            status, 200,
            "case {case} ({arn}): expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            body["FailedResourcesMap"],
            serde_json::json!({}),
            "case {case} ({arn}): expected no failed resource, got {body}"
        );
    }

    // Now a refused batch over the same ARNs, which must write nothing.
    for bad in [
        "wording-not-an-arn",
        "arn:aws:sqs:us-west-2:000000000000:wording-far-queue",
    ] {
        let arns: Vec<&str> = served
            .iter()
            .map(|(_, arn)| *arn)
            .chain(std::iter::once(bad))
            .collect();
        let (status, body) = call(
            &router,
            "UntagResources",
            request_for("UntagResources", &arns),
        )
        .await;
        assert_eq!(
            status, 400,
            "case refused batch with {bad:?}: expected HTTP 400, got {status} with {body}"
        );
    }

    for (case, arn) in served {
        let (status, body) =
            call(&router, "GetResources", request_for("GetResources", &[arn])).await;
        assert_eq!(
            status, 200,
            "case {case} ({arn}): read-back expected HTTP 200, got {status} with {body}"
        );
        let tags = page_tags(&body, arn);
        assert_eq!(
            tags,
            [("Env".to_string(), "prod".to_string())]
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
            "case {case} ({arn}): a refused batch must leave the tag written earlier in \
             place, got {tags:?}"
        );
    }
}
