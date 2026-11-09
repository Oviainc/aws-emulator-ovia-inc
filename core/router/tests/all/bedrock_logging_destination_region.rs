// SPDX-License-Identifier: CC0-1.0

//! Amazon Bedrock only installs a model invocation logging configuration whose Amazon S3
//! destinations it could deliver into.
//!
//! RULE: model invocation logging delivers its records from the Region the configuration is
//! installed in, so a destination bucket that lives in ANOTHER Region is not a destination
//! the configuration could ever be served from: such a configuration is refused as input —
//! the modelled `ValidationException` (HTTP 400) — and installs nothing, while a
//! configuration whose buckets are in the caller's own Region is installed and read back.
//! Both buckets a configuration can name are judged, since Amazon Bedrock delivers into
//! both: `loggingConfig.s3Config` and the `largeDataDeliveryS3Config` of the CloudWatch
//! Logs destination.
//!
//! Where a bucket LIVES is a question the object store answers, over the in-process
//! callback, so a single-service router has no Amazon S3 to ask: that is why this suite
//! lives here rather than in the service's own crate.
//!
//! The rows span what the rule generalises over: which destination carries the bucket, how
//! many destinations the configuration names, which Region the bucket is in (the caller's
//! own, one other Region, and another one again), whether the configuration also carries a
//! key prefix and the data-delivery switches, a bucket the object store does not hold at
//! all — about which nothing is established, so the configuration is admitted rather than
//! refused on a guess — and such a bucket BESIDE a cross-Region one, which the check still
//! goes on to judge.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every Bedrock call here is signed for, and so the Region whose logging the
/// configurations configure.
const REGION: &str = "us-east-1";

/// The one Region a bucket carries no location constraint for, which is this suite's own
/// (`GetBucketLocationOutput$LocationConstraint`: "Buckets in Region us-east-1 have a
/// LocationConstraint of null").
const UNCONSTRAINED_REGION: &str = "us-east-1";

/// Two other Regions a destination bucket can live in.
const ELSEWHERE: &str = "eu-west-1";
const FAR_AWAY: &str = "ap-southeast-2";

/// S3's signing name, for the bucket creates this suite stages.
const S3: &str = "s3";

/// The URI the three model-invocation-logging operations are bound to.
const LOGGING_URI: &str = "/logging/modelinvocations";

/// A role ARN of this account, for the CloudWatch Logs destination whose `roleArn` is
/// `@required`. No IAM is mounted here, so nothing is established about the role and the
/// pass-role step admits it — which is what leaves the bucket's Region as the one thing
/// under test.
const LOGGING_ROLE: &str = "arn:aws:iam::000000000000:role/bedrock-logging";

/// The Bedrock control plane and Amazon S3 over ONE shared engine, as `router/src/lib.rs`
/// mounts them, so the control plane's in-process callback reaches this very object store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header scoped to `service` in `region`, which is what routes the
/// request and what the services read the caller's Region from.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// `PUT /<bucket>` — S3's `CreateBucket`, in `region`: a create is signed for the Region the
/// bucket is to live in and names it as the location constraint.
///
/// `us-east-1` is the one Region a create names NO constraint for ("Buckets in Region
/// us-east-1 have a LocationConstraint of null"), which is why that Region is not one of
/// `BucketLocationConstraint`'s values and why S3 refuses a create that states it.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str, region: &str) {
    let body = if region == UNCONSTRAINED_REGION {
        String::new()
    } else {
        format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{region}</LocationConstraint></CreateBucketConfiguration>"
        )
    };
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{bucket}"))
        .header("host", format!("{S3}.{region}.amazonaws.com"))
        .header("content-type", "application/xml")
        .header("authorization", authorization(S3, region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "setup: CreateBucket({bucket:?}) in {region} must succeed, got {} with {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
}

/// One restJson1 call to the Bedrock control plane: the status, the wire error code and the
/// answered document.
async fn bedrock(
    router: &MultiServiceRouter,
    method: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(LOGGING_URI)
        .header("host", format!("bedrock.{REGION}.amazonaws.com"))
        .header("authorization", authorization("bedrock", REGION))
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(':')
                .next()
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, code, parsed)
}

/// One row: the configuration to install, and whether this Region's logging can deliver
/// into the destinations it names.
struct Row {
    case: &'static str,
    /// `(bucket, region)` for every bucket the object store is to hold first.
    buckets: Vec<(&'static str, &'static str)>,
    config: Value,
    /// `true` where the put must be installed, `false` where it must be refused.
    installed: bool,
}

/// The bucket a refusal must name: the first destination of the row that is NOT in the
/// caller's own Region, which is the destination the service refuses on.
fn refused_bucket(row: &Row) -> &'static str {
    row.buckets
        .iter()
        .find(|(_, region)| *region != REGION)
        .map(|(bucket, _)| *bucket)
        .unwrap_or_default()
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an object destination in the caller's own Region",
            buckets: vec![("logs-home-one", REGION)],
            config: json!({"s3Config": {"bucketName": "logs-home-one"}}),
            installed: true,
        },
        Row {
            case: "the same destination with a key prefix and the delivery switches",
            buckets: vec![("logs-home-two", REGION)],
            config: json!({
                "s3Config": {"bucketName": "logs-home-two", "keyPrefix": "bedrock/"},
                "textDataDeliveryEnabled": true,
                "imageDataDeliveryEnabled": false,
                "embeddingDataDeliveryEnabled": false,
            }),
            installed: true,
        },
        Row {
            case: "an object destination in another Region",
            buckets: vec![("logs-elsewhere", ELSEWHERE)],
            config: json!({"s3Config": {"bucketName": "logs-elsewhere"}}),
            installed: false,
        },
        Row {
            case: "an object destination in another Region again",
            buckets: vec![("logs-far-away", FAR_AWAY)],
            config: json!({"s3Config": {"bucketName": "logs-far-away", "keyPrefix": "p/"}}),
            installed: false,
        },
        Row {
            case: "a large-data-delivery bucket in another Region, beside a log group",
            buckets: vec![("logs-large-elsewhere", ELSEWHERE)],
            config: json!({"cloudWatchConfig": {
                "logGroupName": "/aws/bedrock/modelinvocations",
                "roleArn": LOGGING_ROLE,
                "largeDataDeliveryS3Config": {"bucketName": "logs-large-elsewhere"},
            }}),
            installed: false,
        },
        Row {
            case: "both destinations in the caller's own Region",
            buckets: vec![("logs-home-three", REGION), ("logs-home-large", REGION)],
            config: json!({
                "s3Config": {"bucketName": "logs-home-three"},
                "cloudWatchConfig": {
                    "logGroupName": "/aws/bedrock/modelinvocations",
                    "roleArn": LOGGING_ROLE,
                    "largeDataDeliveryS3Config": {"bucketName": "logs-home-large"},
                },
            }),
            installed: true,
        },
        Row {
            case: "an object destination here and a large-data one elsewhere",
            buckets: vec![("logs-home-four", REGION), ("logs-large-far", FAR_AWAY)],
            config: json!({
                "s3Config": {"bucketName": "logs-home-four"},
                "cloudWatchConfig": {
                    "logGroupName": "/aws/bedrock/modelinvocations",
                    "roleArn": LOGGING_ROLE,
                    "largeDataDeliveryS3Config": {"bucketName": "logs-large-far"},
                },
            }),
            installed: false,
        },
        Row {
            case: "a bucket the object store holds no record of",
            buckets: Vec::new(),
            config: json!({"s3Config": {"bucketName": "logs-nothing-holds"}}),
            installed: true,
        },
        // The object destination is one nothing is established about, so the check goes ON
        // to the large-data destination beside it rather than ending there.
        Row {
            case: "an unknown object bucket and a large-data bucket in another Region",
            buckets: vec![("logs-large-elsewhere-two", ELSEWHERE)],
            config: json!({
                "s3Config": {"bucketName": "logs-nothing-holds-either"},
                "cloudWatchConfig": {
                    "logGroupName": "/aws/bedrock/modelinvocations",
                    "roleArn": LOGGING_ROLE,
                    "largeDataDeliveryS3Config": {"bucketName": "logs-large-elsewhere-two"},
                },
            }),
            installed: false,
        },
        Row {
            case: "a CloudWatch Logs destination naming no bucket at all",
            buckets: Vec::new(),
            config: json!({"cloudWatchConfig": {
                "logGroupName": "/aws/bedrock/modelinvocations",
                "roleArn": LOGGING_ROLE,
            }}),
            installed: true,
        },
    ]
}

#[tokio::test]
async fn a_logging_destination_outside_the_callers_region_is_refused_and_installs_nothing() {
    for row in rows() {
        let router = emulator();
        let case = row.case;
        for (bucket, region) in &row.buckets {
            create_bucket(&router, bucket, region).await;
        }

        let (status, code, answered) = bedrock(
            &router,
            "PUT",
            Some(json!({"loggingConfig": row.config.clone()})),
        )
        .await;

        if row.installed {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the configuration to be installed, got {status} \
                 {code} with {answered}"
            );
            let (status, _, reported) = bedrock(&router, "GET", None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the installed configuration to be read back, got \
                 {status} with {reported}"
            );
            assert_eq!(
                reported["loggingConfig"], row.config,
                "case {case:?}: expected the configuration in force to be the one submitted, \
                 got {reported}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected the cross-Region destination to be refused, got \
             {status} {code} with {answered}"
        );
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?} with {answered}"
        );
        let message = answered["message"].as_str().unwrap_or_default();
        let bucket = refused_bucket(&row);
        assert!(
            message.contains(bucket),
            "case {case:?}: expected the refusal to name the bucket it refused ({bucket}), \
             got {message:?}"
        );

        // A refused put installs NOTHING: an independent read reports no configuration at
        // all, so no operator can believe a trail exists.
        let (status, _, reported) = bedrock(&router, "GET", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer, got {status} with {reported}"
        );
        assert!(
            reported
                .get("loggingConfig")
                .is_none_or(Value::is_null),
            "case {case:?}: expected a refused put to install nothing, got {reported}"
        );
    }
}
