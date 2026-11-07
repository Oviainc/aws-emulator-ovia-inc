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

/// Every operation this service's model binds, from
/// `smithy_query(query="list_operations",
/// model_path="services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/smithy-model/com-amazonaws-cloudtrail-cloudtrail-20131101.json")`
/// — count 60. The dispatch rule is per REQUEST, not per operation, so the whole bound
/// surface is the class it has to hold over.
const MODELLED_OPERATIONS: [&str; 60] = [
    "AddTags",
    "CancelQuery",
    "CreateChannel",
    "CreateDashboard",
    "CreateEventDataStore",
    "CreateTrail",
    "DeleteChannel",
    "DeleteDashboard",
    "DeleteEventDataStore",
    "DeleteResourcePolicy",
    "DeleteTrail",
    "DeregisterOrganizationDelegatedAdmin",
    "DescribeQuery",
    "DescribeTrails",
    "DisableFederation",
    "EnableFederation",
    "GenerateQuery",
    "GetChannel",
    "GetDashboard",
    "GetEventConfiguration",
    "GetEventDataStore",
    "GetEventSelectors",
    "GetImport",
    "GetInsightSelectors",
    "GetQueryResults",
    "GetResourcePolicy",
    "GetTrail",
    "GetTrailStatus",
    "ListChannels",
    "ListDashboards",
    "ListEventDataStores",
    "ListImportFailures",
    "ListImports",
    "ListInsightsData",
    "ListInsightsMetricData",
    "ListPublicKeys",
    "ListQueries",
    "ListTags",
    "ListTrails",
    "LookupEvents",
    "PutEventConfiguration",
    "PutEventSelectors",
    "PutInsightSelectors",
    "PutResourcePolicy",
    "RegisterOrganizationDelegatedAdmin",
    "RemoveTags",
    "RestoreEventDataStore",
    "SearchSampleQueries",
    "StartDashboardRefresh",
    "StartEventDataStoreIngestion",
    "StartImport",
    "StartLogging",
    "StartQuery",
    "StopEventDataStoreIngestion",
    "StopImport",
    "StopLogging",
    "UpdateChannel",
    "UpdateDashboard",
    "UpdateEventDataStore",
    "UpdateTrail",
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

    /// The wire error message, or `""`.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// Whether `operation` RAN, as opposed to being refused by the dispatcher before
    /// anything ran.
    ///
    /// Anything but a 501 came out of the operation itself — its output, a modelled
    /// failure, or the request walk refusing the input. A 501 has two possible authors,
    /// told apart by how each NAMES the operation rather than by its prose: the
    /// scaffold channel of an operation that is still a stub names it by its SHORT name
    /// (`operation AddTags is scaffolded but not implemented …`), while the dispatcher
    /// names the whole dotted routing key it could not resolve (`operation
    /// amazonaws.cloudtrail.v20131101.CloudTrail_20131101.AddTags is not implemented
    /// …`), where the operation is preceded by a dot. Both halves are required, so
    /// neither channel can be reworded into passing for the other.
    fn reached_the_operation(&self, operation: &str) -> bool {
        if self.status != 501 {
            return true;
        }
        let message = self.message();
        message.contains(operation) && !message.contains(&format!(".{operation}"))
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
    /// The shared engine, kept so a trail's log-delivery destination can be recorded in
    /// the one object store before a request names it. See [`Emulator::call_signed_or_not`].
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    /// Call the operation named by the whole `target`, with `body`, SIGNED unless
    /// `signed` is false.
    ///
    /// Unsigned is a case of its own, not a convenience: a request carrying no SigV4
    /// credential scope is routed by its `X-Amz-Target` alone, which is the other half
    /// of the rule under test.
    async fn call_signed_or_not(&self, target: &str, signed: bool, body: &Value) -> Reply {
        // A trail's log-delivery destination is a bucket of the account, resolved in the
        // one object store by `CreateTrail` and `UpdateTrail`, so the bucket a request
        // names is recorded there first, carrying the policy that lets CloudTrail deliver
        // into it — the `CreateBucket` plus `PutBucketPolicy` a client makes before it
        // points a trail anywhere. This suite's subject is the target header, not the
        // destination.
        if let Some(bucket) = body
            .get("S3BucketName")
            .and_then(Value::as_str)
            .filter(|bucket| !bucket.is_empty())
        {
            let objects = service_com_amazonaws_s3_amazons3::state::service_state(&*self.engine)
                .expect("the in-memory engine carries the object store");
            let mut store = objects.lock();
            let _ = store.create_bucket(bucket, REGION);
            if let Some(held) = store.bucket_mut(bucket) {
                held.policy = Some(super::emulator::cloudtrail_bucket_policy(bucket));
            }
        }
        let mut request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", target)
            .header("content-type", "application/x-amz-json-1.1");
        if signed {
            // The Region a request is served in comes from its SigV4 credential scope.
            request = request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/cloudtrail/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
                ),
            );
        }
        let request = request
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Reply { status, body }
    }

    /// Call the operation named by the whole `target`, with `body`.
    async fn call_target(&self, target: &str, body: &Value) -> Reply {
        self.call_signed_or_not(target, true, body).await
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
                // CloudTrail answers a not-found trail with 400, not the 404 its
                // model annotates; see `state::TRAIL_NOT_FOUND_STATUS`.
                400,
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

/// The rule is per REQUEST, not per operation: EVERY operation the model binds is
/// reached behind every prefix form, not just the trail lifecycle above.
///
/// Whatever the operation then answers — its output, a modelled failure, the request
/// walk refusing an empty body, or the scaffold's own 501 for one that is still a stub —
/// it is the operation answering, not the dispatcher refusing a routing key it could not
/// resolve. That refusal is what this issue reported for all 60.
#[tokio::test]
async fn every_modelled_operation_is_reached_behind_every_target_prefix_form() {
    for (form, prefix) in TARGET_PREFIXES {
        let emulator = Emulator::new();
        for operation in MODELLED_OPERATIONS {
            // An empty body: a required member missing is refused AFTER dispatch, by the
            // operation's own request walk, which is itself evidence the request reached
            // the operation.
            let reply = emulator.call(prefix, operation, &json!({})).await;
            assert!(
                reply.reached_the_operation(operation),
                "case {form}, {operation}: the dispatcher refused the request instead of \
                 running the operation, got {reply:?}"
            );
        }
    }
}

/// The prefix form changes neither the operation a request reaches nor the state it
/// computes from: a trail created behind one form reads back behind every other, out of
/// the one store.
#[tokio::test]
async fn one_store_lies_behind_every_target_prefix_form() {
    let emulator = Emulator::new();

    let mut expected: Vec<String> = Vec::new();
    for (form, prefix) in TARGET_PREFIXES {
        // One trail per writing form, named after it, so the reads below can tell which
        // form stored which record.
        let name = format!("ct-store-{}", expected.len());
        let trail_arn = format!("arn:aws:cloudtrail:{REGION}:{ACCOUNT}:trail/{name}");
        let created = emulator
            .call(
                prefix,
                "CreateTrail",
                &json!({"Name": name, "S3BucketName": "ct-store-bucket"}),
            )
            .await;
        let case = format!("{form}: CreateTrail");
        assert_eq!(
            created.expect_ok(&case).get("TrailARN"),
            Some(&json!(trail_arn)),
            "case {case}: expected the trail ARN for {name}, got {created:?}"
        );
        expected.push(trail_arn);

        // Read it back behind EVERY form, including the ones that did not write it.
        for (reading_form, reading_prefix) in TARGET_PREFIXES {
            let fetched = emulator
                .call(reading_prefix, "GetTrail", &json!({"Name": name}))
                .await;
            let case = format!("{form} written, read behind {reading_form}");
            let trail = fetched.expect_ok(&case)["Trail"].clone();
            assert_eq!(
                (trail.get("Name"), trail.get("S3BucketName")),
                (Some(&json!(name)), Some(&json!("ct-store-bucket"))),
                "case {case}: the trail read back is not the one created, got {trail:?}"
            );
        }
    }

    // One store behind all three forms: the enumeration reports every trail written.
    let listed = emulator.call(TARGET_PREFIXES[0].1, "ListTrails", &json!({})).await;
    let mut actual = arns(listed.expect_ok("ListTrails behind every form"), "Trails");
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "case ListTrails: expected the trails written behind every prefix form, got \
         {listed:?}"
    );
}

/// A modelled failure reaches the client as ITSELF behind the dotted prefix: the
/// negative classes this service owns are no longer swallowed by the dispatcher's
/// unmodelled 501 `NotImplemented`.
#[tokio::test]
async fn modelled_failures_survive_the_dotted_target_prefix() {
    let sdk_prefix = TARGET_PREFIXES[0].1;
    let emulator = Emulator::new();
    emulator
        .call(
            sdk_prefix,
            "CreateTrail",
            &json!({"Name": "ct-taken", "S3BucketName": "ct-fail-bucket"}),
        )
        .await
        .expect_ok("CreateTrail");

    // (case, operation, body, expected status, expected error code)
    let rows: [(&str, &str, Value, u16, &str); 4] = [
        (
            "a name already taken",
            "CreateTrail",
            json!({"Name": "ct-taken", "S3BucketName": "other-bucket"}),
            400,
            "TrailAlreadyExistsException",
        ),
        (
            "a name the model's rules refuse",
            "CreateTrail",
            json!({"Name": "my--namespace", "S3BucketName": "ct-fail-bucket"}),
            400,
            "InvalidTrailNameException",
        ),
        (
            // CloudTrail answers a not-found trail with 400, not the 404 its model
            // annotates; see `state::TRAIL_NOT_FOUND_STATUS`.
            "an unknown trail",
            "GetTrail",
            json!({"Name": "ct-absent"}),
            400,
            "TrailNotFoundException",
        ),
        (
            "an unknown trail's status",
            "GetTrailStatus",
            json!({"Name": "ct-absent"}),
            400,
            "TrailNotFoundException",
        ),
    ];

    for (case, operation, body, status, code) in rows {
        emulator
            .call(sdk_prefix, operation, &body)
            .await
            .expect_error(case, status, code);
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

/// A request whose ONLY routable signal is its `X-Amz-Target` reaches this service.
///
/// With no credential scope the framework's routing key is the target's FIRST segment —
/// `com` for the prefix an SDK sends, which names no service — so such a request would
/// otherwise reach nobody and answer `404 UnknownService`. The service a target names is
/// the segment before the operation, which is the same rule the operation is read by.
#[tokio::test]
async fn an_unsigned_target_naming_this_service_still_reaches_it() {
    for (form, prefix) in TARGET_PREFIXES {
        let emulator = Emulator::new();
        let created = emulator
            .call_signed_or_not(
                &format!("{prefix}CreateTrail"),
                false,
                &json!({"Name": "ct-unsigned", "S3BucketName": "ct-unsigned-bucket"}),
            )
            .await;
        let case = format!("{form}, unsigned");
        assert_eq!(
            created.expect_ok(&case).get("Name"),
            Some(&json!("ct-unsigned")),
            "case {case}: expected the created trail, got {created:?}"
        );
    }

    // A target naming ANOTHER service is not claimed by this one, signed or not.
    let emulator = Emulator::new();
    for (case, signed) in [("unsigned", false), ("signed for cloudtrail", true)] {
        let other = emulator
            .call_signed_or_not("DynamoDB_20120810.CreateTable", signed, &json!({}))
            .await;
        assert_ne!(
            other.status, 200,
            "case another service's target, {case}: this service must not serve it, got \
             {other:?}"
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
