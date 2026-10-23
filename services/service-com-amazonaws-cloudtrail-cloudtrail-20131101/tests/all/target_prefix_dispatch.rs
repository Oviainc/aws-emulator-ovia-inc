// SPDX-License-Identifier: CC0-1.0

//! Dispatch by `X-Amz-Target`, over the prefix forms a client can write.
//!
//! The rule under test: an `awsJson1_1` target is `<targetPrefix>.<OperationName>`, and
//! a target prefix MAY itself contain dots — this service's is the fully qualified Coral
//! service name `com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101`, which is what
//! every AWS SDK sends — so the operation a request names is the segment after the LAST
//! dot of the target, and every modelled operation must dispatch to its handler however
//! many dots its prefix carries.
//!
//! Each case goes over the wire: an `awsJson1_1` `POST /` through the same
//! `MultiServiceRouter` the production router serves, so what is asserted is what a
//! client sees — including the HTTP status and the error code.

use serde_json::{json, Value};
use std::sync::Arc;

/// The Region every case calls in.
const REGION: &str = "us-west-2";

/// The account the emulator mints ARNs under (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The `X-Amz-Target` prefix forms this service answers, each `(case, prefix)`.
///
/// Source: the model's service shape id
/// (`smithy_query(query="service_info")` -> `CloudTrail_20131101`) is the last segment
/// of all three; the first row is the prefix an AWS SDK sends for this service, which
/// qualifies that shape name with its Coral namespace.
const TARGET_PREFIXES: [(&str, &str); 3] = [
    (
        "the fully qualified prefix an AWS SDK sends",
        "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.",
    ),
    ("the bare service shape name", "CloudTrail_20131101."),
    (
        "one namespace label before the shape name",
        "cloudtrail.CloudTrail_20131101.",
    ),
];

/// One answer from the emulator: the HTTP status and the parsed JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// The wire error code (`__type`), or `""` for a success. A `__type` may be
    /// namespaced (`com.amazonaws.cloudtrail#Foo`); the code clients branch on is the
    /// shape name.
    fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .map(|t| t.rsplit('#').next().unwrap_or(t))
            .unwrap_or_default()
    }

    /// Assert this reply is a success, and hand back its body.
    fn expect_ok(&self, case: &str) -> &Value {
        assert_eq!(
            (self.status, self.error_code()),
            (200, ""),
            "case {case}: expected HTTP 200, got {self:?}"
        );
        &self.body
    }

    /// Assert this reply is the modelled failure `code` with HTTP `status`.
    fn expect_error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code()),
            (status, code),
            "case {case}: expected HTTP {status} {code}, got {self:?}"
        );
    }
}

/// The emulator under test: one shared engine, one registered service, requests
/// dispatched through the production router.
struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call the operation named by the whole `target`, with `body`.
    async fn call_target(&self, target: &str, body: &Value) -> Reply {
        // The Region a request is served in comes from its SigV4 credential scope.
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/cloudtrail/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", target)
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Reply { status, body }
    }

    /// Call `operation` under `prefix`.
    async fn call(&self, prefix: &str, operation: &str, body: &Value) -> Reply {
        self.call_target(&format!("{prefix}{operation}"), body).await
    }
}

/// Under every target-prefix form, the whole trail lifecycle answers its modelled 200
/// response — never the unmodelled 501 `NotImplemented` of a request that named no
/// operation. One fresh emulator per row, so the rows cannot see each other's trails.
#[tokio::test]
async fn every_target_prefix_form_serves_the_trail_lifecycle() {
    for (form, prefix) in TARGET_PREFIXES {
        let emulator = Emulator::new();
        let name = "ct-dispatch-trail";
        let trail_arn = format!("arn:aws:cloudtrail:{REGION}:{ACCOUNT}:trail/{name}");

        // An empty account enumerates as a present, empty list — not an error.
        let listed = emulator.call(prefix, "ListTrails", &json!({})).await;
        let case = format!("{form}: ListTrails on an empty account");
        assert_eq!(
            listed.expect_ok(&case).get("Trails"),
            Some(&json!([])),
            "case {case}: expected Trails = [], got {listed:?}"
        );

        // An unknown trail name is a successful empty list, not a refusal.
        let described = emulator
            .call(
                prefix,
                "DescribeTrails",
                &json!({"trailNameList": ["ct-dispatch-ghost"]}),
            )
            .await;
        let case = format!("{form}: DescribeTrails for an unknown name");
        assert_eq!(
            described.expect_ok(&case).get("trailList"),
            Some(&json!([])),
            "case {case}: expected trailList = [], got {described:?}"
        );

        // CreateTrail reports the trail it stored.
        let created = emulator
            .call(
                prefix,
                "CreateTrail",
                &json!({"Name": name, "S3BucketName": "ct-dispatch-bucket"}),
            )
            .await;
        let case = format!("{form}: CreateTrail");
        let created = created.expect_ok(&case).clone();
        for (member, want) in [
            ("Name", json!(name)),
            ("S3BucketName", json!("ct-dispatch-bucket")),
            ("TrailARN", json!(trail_arn)),
            // The documented boolean defaults of a trail created with the required
            // members only.
            ("IsMultiRegionTrail", json!(false)),
            ("IsOrganizationTrail", json!(false)),
            ("LogFileValidationEnabled", json!(false)),
            ("IncludeGlobalServiceEvents", json!(true)),
            ("RecursiveLogging", json!(true)),
        ] {
            assert_eq!(
                created.get(member),
                Some(&want),
                "case {case}: expected {member} = {want}, got {created:?}"
            );
        }

        // Read-after-write: the stored trail comes back through GetTrail.
        let fetched = emulator.call(prefix, "GetTrail", &json!({"Name": name})).await;
        let case = format!("{form}: GetTrail after CreateTrail");
        let trail = fetched.expect_ok(&case)["Trail"].clone();
        for (member, want) in [
            ("Name", json!(name)),
            ("TrailARN", json!(trail_arn)),
            ("S3BucketName", json!("ct-dispatch-bucket")),
            ("HomeRegion", json!(REGION)),
        ] {
            assert_eq!(
                trail.get(member),
                Some(&want),
                "case {case}: expected Trail.{member} = {want}, got {trail:?}"
            );
        }

        // ...and through the two enumerations.
        let listed = emulator.call(prefix, "ListTrails", &json!({})).await;
        let case = format!("{form}: ListTrails after CreateTrail");
        assert_eq!(
            arns(listed.expect_ok(&case), "Trails"),
            vec![trail_arn.clone()],
            "case {case}: expected the created trail, got {listed:?}"
        );
        let described = emulator.call(prefix, "DescribeTrails", &json!({})).await;
        let case = format!("{form}: DescribeTrails after CreateTrail");
        assert_eq!(
            arns(described.expect_ok(&case), "trailList"),
            vec![trail_arn.clone()],
            "case {case}: expected the created trail, got {described:?}"
        );

        // DeleteTrail answers an empty body and removes the trail.
        let deleted = emulator
            .call(prefix, "DeleteTrail", &json!({"Name": name}))
            .await;
        let case = format!("{form}: DeleteTrail");
        assert_eq!(
            deleted.expect_ok(&case),
            &json!({}),
            "case {case}: expected an empty DeleteTrailResponse, got {deleted:?}"
        );
        emulator
            .call(prefix, "GetTrail", &json!({"Name": name}))
            .await
            .expect_error(
                &format!("{form}: GetTrail after DeleteTrail"),
                404,
                "TrailNotFoundException",
            );
        let listed = emulator.call(prefix, "ListTrails", &json!({})).await;
        let case = format!("{form}: ListTrails after DeleteTrail");
        assert_eq!(
            listed.expect_ok(&case).get("Trails"),
            Some(&json!([])),
            "case {case}: expected Trails = [], got {listed:?}"
        );
    }
}

/// Resolving the operation from the last dot does not invent handlers: a target that
/// names an operation this model does not bind, or that names another service, is NOT
/// served a 200.
#[tokio::test]
async fn a_target_that_names_no_operation_of_this_service_is_not_served() {
    let emulator = Emulator::new();
    // (case, whole target)
    let rows = [
        (
            "an unbound operation under this service's SDK prefix",
            "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.NoSuchOperation",
        ),
        (
            "an unbound operation under the bare shape name",
            "CloudTrail_20131101.NoSuchOperation",
        ),
        (
            "a prefix that only ends in this shape name's characters",
            "NotCloudTrail_20131101.NoSuchOperation",
        ),
        ("a target with no operation segment", "CloudTrail_20131101."),
    ];

    for (case, target) in rows {
        let reply = emulator.call_target(target, &json!({})).await;
        // An operation the emulator does not serve is the framework's own refusal
        // (501 `NotImplemented`), never a 200 from a handler it picked by accident.
        assert_eq!(
            (reply.status, reply.error_code()),
            (501, "NotImplemented"),
            "case {case:?}: expected target {target:?} to be refused as unimplemented, \
             got {reply:?}"
        );
    }
}

/// The trail ARNs a reply's `Trails` / `trailList` carries, in order.
fn arns(list: &Value, member: &str) -> Vec<String> {
    list.get(member)
        .and_then(Value::as_array)
        .map(|trails| {
            trails
                .iter()
                .map(|trail| {
                    trail
                        .get("TrailARN")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}
