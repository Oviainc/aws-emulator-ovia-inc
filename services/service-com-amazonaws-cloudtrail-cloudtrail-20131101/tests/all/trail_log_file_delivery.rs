// SPDX-License-Identifier: CC0-1.0

//! What a LOGGING trail delivers into its bucket, end to end.
//!
//! Two services are mounted on ONE engine, because the behaviour spans them: CloudTrail
//! serves the calls and delivers the log files, and the object store is where they land.
//! CloudTrail is driven over the wire, the way a client drives it; the bucket is created,
//! policied, listed and read through Amazon S3's OWN API over the same in-process callback a
//! handler uses — so a delivered log file counts only if the object store itself answers
//! with it.
//!
//! The rule under test: while a trail is logging, every call CloudTrail records for the
//! account that the trail's selectors admit is ALSO delivered into that trail's bucket as a
//! gzipped JSON object, under the documented key layout
//! (`<S3KeyPrefix>/AWSLogs/<account-id>/CloudTrail/<region>/<YYYY>/<MM>/<DD>/` and a file
//! named `<account-id>_CloudTrail_<region>_<YYYYMMDDTHHmm>Z_<unique>.json.gz`), whose
//! document holds those records under `Records`. A trail that is not logging, or whose
//! selectors do not admit the call, has nothing delivered for it.

use std::io::Read;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::emulator::{cloudtrail_bucket_policy, TARGET_PREFIX};

/// The Region every call below is signed for, which is the Region the trails are homed in
/// and the Region their log files are delivered under. The bucket is created in it with no
/// location constraint, which is the one Region an absent constraint names.
const REGION: &str = "us-east-1";

/// The first segment of a log-file key, under whatever prefix the trail carries.
const LOG_FILE_ROOT_SEGMENT: &str = "AWSLogs";

/// The segment naming the service whose files a prefix holds, which is also the middle
/// field of a log file's own name.
const LOG_FILE_SERVICE_SEGMENT: &str = "CloudTrail";

/// What a log file's name ends with.
const LOG_FILE_SUFFIX: &str = ".json.gz";

/// The member a log file's document holds its records in.
const RECORDS_MEMBER: &str = "Records";

/// This service's `eventSource` on every record it writes.
const EVENT_SOURCE: &str = "cloudtrail.amazonaws.com";

/// CloudTrail and the object store over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_s3_amazons3::EmulatedService::new(Arc::clone(&engine)),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// One CloudTrail call (`awsJson1_1`), answering the HTTP status and the parsed body.
    async fn cloudtrail(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/cloudtrail/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("cloudtrail.{REGION}.amazonaws.com"))
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One CloudTrail call that has to be served.
    async fn served(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.cloudtrail(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// The Amazon S3 client, over the same in-process callback the delivery uses — built by
    /// the same shared constructor, so the test's transport is the delivery's by
    /// construction.
    fn s3(&self) -> aws_sdk_s3::Client {
        service_common::s3_client::s3_client(&self.callback, REGION)
    }

    /// Create `bucket` and attach the policy that lets CloudTrail deliver into it — the
    /// `CreateBucket` and `PutBucketPolicy` a client makes before pointing a trail at it.
    async fn log_bucket(&self, bucket: &str) {
        self.s3()
            .create_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {bucket} must succeed, got {error:?}"));
        self.s3()
            .put_bucket_policy()
            .bucket(bucket)
            .policy(cloudtrail_bucket_policy(bucket))
            .send()
            .await
            .unwrap_or_else(|error| panic!("policying {bucket} must succeed, got {error:?}"));
    }

    /// Every object key in `bucket`, as Amazon S3 lists them.
    async fn keys(&self, bucket: &str) -> Vec<String> {
        let listed = self
            .s3()
            .list_objects_v2()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("listing {bucket} must succeed, got {error:?}"));
        let mut keys: Vec<String> = listed
            .contents()
            .iter()
            .filter_map(|object| object.key().map(str::to_string))
            .collect();
        keys.sort();
        keys
    }

    /// The records one delivered log file holds, read through `GetObject` and gunzipped.
    async fn records(&self, bucket: &str, key: &str) -> Vec<Value> {
        let object = self
            .s3()
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .unwrap_or_else(|error| panic!("reading {key} must succeed, got {error:?}"));
        let body = object
            .body
            .collect()
            .await
            .unwrap_or_else(|error| panic!("reading {key}'s body must succeed, got {error:?}"))
            .into_bytes();
        let mut document = String::new();
        flate2::read::GzDecoder::new(body.as_ref())
            .read_to_string(&mut document)
            .unwrap_or_else(|error| panic!("{key} must be a gzip stream, got {error}"));
        let parsed: Value = serde_json::from_str(&document)
            .unwrap_or_else(|error| panic!("{key} must gunzip to JSON, got {error}"));
        parsed
            .get(RECORDS_MEMBER)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("{key} must hold {RECORDS_MEMBER}, got {parsed}"))
    }
}

/// The account every trail and bucket below belongs to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// Assert `key` is the documented log-file layout under `key_prefix`, and answer the minute
/// stamp its name carries.
///
/// Checked field by field rather than against a key this test built the same way the
/// delivery does: the segments under the trail's own prefix, the day directories, and the
/// name's own five fields — the last of which is the unique field, which a reader cannot
/// predict and only has to be there.
fn assert_log_file_layout(case: &str, key: &str, key_prefix: Option<&str>) -> String {
    let prefix = key_prefix
        .map(|prefix| prefix.trim_matches('/'))
        .filter(|prefix| !prefix.is_empty())
        .map(|prefix| format!("{prefix}/"))
        .unwrap_or_default();
    let under = key.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected a key under {prefix:?}, got {key:?}")
    });
    let segments: Vec<&str> = under.split('/').collect();
    let [root, account_segment, service, region, year, month, day, name] = segments.as_slice()
    else {
        panic!(
            "case {case:?}: expected the eight segments of the layout, got {:?} in {key:?}",
            segments.len()
        )
    };
    assert_eq!(
        (*root, *account_segment, *service, *region),
        (
            LOG_FILE_ROOT_SEGMENT,
            account().as_str(),
            LOG_FILE_SERVICE_SEGMENT,
            REGION
        ),
        "case {case:?}: expected the documented prefix, got {key:?}"
    );
    for (field, width) in [(year, 4), (month, 2), (day, 2)] {
        assert!(
            field.len() == width && field.chars().all(|digit| digit.is_ascii_digit()),
            "case {case:?}: expected a {width}-digit date segment, got {field:?} in {key:?}"
        );
    }

    let name = name
        .strip_suffix(LOG_FILE_SUFFIX)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {LOG_FILE_SUFFIX} name, got {key:?}"));
    let fields: Vec<&str> = name.split('_').collect();
    let [named_account, named_service, named_region, stamp, unique] = fields.as_slice() else {
        panic!("case {case:?}: expected the five fields of a log file's name, got {name:?}")
    };
    assert_eq!(
        (*named_account, *named_service, *named_region),
        (account().as_str(), LOG_FILE_SERVICE_SEGMENT, REGION),
        "case {case:?}: expected the name to carry the account, the service and the Region, \
         got {name:?}"
    );
    // `YYYYMMDDTHHmmZ`: the minute the records belong to, which has to be the day the key
    // was filed under.
    let shaped = stamp.len() == 14
        && stamp.as_bytes()[8] == b'T'
        && stamp.ends_with('Z')
        && stamp[..8].chars().all(|digit| digit.is_ascii_digit())
        && stamp[9..13].chars().all(|digit| digit.is_ascii_digit());
    assert!(
        shaped,
        "case {case:?}: expected a YYYYMMDDTHHmmZ stamp, got {stamp:?} in {key:?}"
    );
    assert_eq!(
        &stamp[..8],
        format!("{year}{month}{day}").as_str(),
        "case {case:?}: expected the stamp's day to be the key's day, got {key:?}"
    );
    assert!(
        !unique.is_empty() && unique.chars().all(|character| character.is_ascii_alphanumeric()),
        "case {case:?}: expected a unique field, got {unique:?} in {key:?}"
    );
    (*stamp).to_string()
}

/// Every `eventName` the log files in `bucket` carry between them, with every record
/// checked for the fields a consumer reads it by.
async fn delivered_event_names(
    emulator: &Emulator,
    case: &str,
    bucket: &str,
    key_prefix: Option<&str>,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for key in emulator.keys(bucket).await {
        assert_log_file_layout(case, &key, key_prefix);
        for record in emulator.records(bucket, &key).await {
            let field = |name: &str| {
                record
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            assert_eq!(
                (field("eventSource").as_str(), field("awsRegion").as_str()),
                (EVENT_SOURCE, REGION),
                "case {case:?}: expected a CloudTrail record of this Region, got {record}"
            );
            assert_eq!(
                field("recipientAccountId"),
                account(),
                "case {case:?}: expected the account's own record, got {record}"
            );
            assert!(
                !field("eventID").is_empty() && !field("eventTime").is_empty(),
                "case {case:?}: expected a record naming itself and its instant, got {record}"
            );
            names.push(field("eventName"));
        }
    }
    names.sort();
    names
}

/// RULE: while a trail is logging, each call CloudTrail records lands in that trail's
/// bucket as a gzipped log file under the documented layout.
///
/// The rows differ along the trail's `S3KeyPrefix` — absent, one segment, a nested prefix,
/// and one written with separators around it — which is what the layout is relative to.
#[tokio::test]
async fn a_logging_trail_delivers_each_recorded_call_as_a_gzipped_log_file() {
    // (case, the S3KeyPrefix the trail carries)
    let rows: [(&str, Option<&str>); 4] = [
        ("no key prefix", None),
        ("a one-segment prefix", Some("audit")),
        ("a nested prefix", Some("logs/prod")),
        ("a prefix written with separators around it", Some("/edge/")),
    ];

    for (index, (case, key_prefix)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let bucket = format!("delivery-logs-{index}");
        let trail = format!("delivery-trail-{index}");
        emulator.log_bucket(&bucket).await;

        let mut create = json!({"Name": trail, "S3BucketName": bucket});
        if let Some(prefix) = key_prefix {
            create["S3KeyPrefix"] = json!(prefix);
        }
        emulator.served("CreateTrail", &create).await;
        // Nothing is logged yet, so nothing is delivered for the create itself — and the
        // trail reports no delivery, because its bucket holds no log file.
        let before = emulator.keys(&bucket).await;
        assert!(
            before.is_empty(),
            "case {case:?}: a trail that is not logging delivers nothing, got {before:?}"
        );
        let quiet = emulator
            .served("GetTrailStatus", &json!({"Name": trail}))
            .await;
        assert_eq!(
            quiet.get("LatestDeliveryTime"),
            None,
            "case {case:?}: expected no delivery reported before one was made, got {quiet}"
        );

        emulator.served("StartLogging", &json!({"Name": trail})).await;
        let status = emulator
            .served("GetTrailStatus", &json!({"Name": trail}))
            .await;
        assert_eq!(
            status.get("IsLogging"),
            Some(&json!(true)),
            "case {case:?}: expected the trail to report itself logging, got {status}"
        );
        // The call that started the logging was itself delivered, so the trail now reports
        // the delivery its bucket holds — and the deprecated string twins of that instant
        // report the same one.
        let delivered_at = status.get("LatestDeliveryTime").and_then(Value::as_f64);
        assert!(
            delivered_at.is_some(),
            "case {case:?}: expected the delivery the bucket holds to be reported, got {status}"
        );
        for member in ["LatestDeliveryAttemptTime", "LatestDeliveryAttemptSucceeded"] {
            let reported = status.get(member).and_then(Value::as_str).unwrap_or_default();
            // `YYYY-MM-DDTHH:MM:SSZ`: whole seconds, which is the form these string
            // members carry the instant in.
            let shaped = reported.len() == 20
                && reported.as_bytes().get(10) == Some(&b'T')
                && reported.ends_with('Z')
                && !reported.contains('.');
            assert!(
                shaped,
                "case {case:?}: expected {member} to carry the delivery's instant at \
                 whole-second resolution, got {status}"
            );
        }

        let names = delivered_event_names(&emulator, case, &bucket, key_prefix).await;
        // The two calls served while the trail was logging: the one that started it, and
        // the read that followed.
        for expected in ["GetTrailStatus", "StartLogging"] {
            assert!(
                names.iter().any(|name| name == expected),
                "case {case:?}: expected a delivered record of {expected}, got {names:?}"
            );
        }
    }
}

/// RULE's other side: a trail delivers only the calls it LOGS — nothing before it starts,
/// nothing after it stops, nothing its selectors exclude, and nothing outside the
/// read/write class they name.
#[tokio::test]
async fn nothing_is_delivered_unless_the_trail_logs_the_call() {
    // A trail that was never started, and one that was started and stopped again.
    for (case, stop_after_start) in [("never started", false), ("started and stopped", true)] {
        let emulator = Emulator::new();
        let bucket = format!("quiet-logs-{}", stop_after_start as u8);
        let trail = "quiet-trail";
        emulator.log_bucket(&bucket).await;
        emulator
            .served(
                "CreateTrail",
                &json!({"Name": trail, "S3BucketName": bucket}),
            )
            .await;
        if stop_after_start {
            emulator.served("StartLogging", &json!({"Name": trail})).await;
            emulator.served("StopLogging", &json!({"Name": trail})).await;
        }
        let delivered_while_logging = match stop_after_start {
            // What the trail delivered while it WAS logging is kept; the rows below are
            // about what follows the stop.
            true => emulator.keys(&bucket).await,
            false => Vec::new(),
        };
        emulator
            .served("GetTrailStatus", &json!({"Name": trail}))
            .await;
        emulator
            .served(
                "PutEventSelectors",
                &json!({
                    "TrailName": trail,
                    "EventSelectors": [{
                        "ReadWriteType": "All",
                        "IncludeManagementEvents": true,
                    }],
                }),
            )
            .await;
        let after = emulator.keys(&bucket).await;
        assert_eq!(
            after, delivered_while_logging,
            "case {case:?}: expected no further log file, got {after:?}"
        );
    }

    // A trail whose selectors EXCLUDE management events: it logs data events for the
    // resources it names, and this account's calls are management events.
    let emulator = Emulator::new();
    let bucket = "excluded-logs";
    let trail = "excluded-trail";
    emulator.log_bucket(bucket).await;
    emulator
        .served(
            "CreateTrail",
            &json!({"Name": trail, "S3BucketName": bucket}),
        )
        .await;
    emulator
        .served(
            "PutEventSelectors",
            &json!({
                "TrailName": trail,
                "EventSelectors": [{
                    "ReadWriteType": "All",
                    "IncludeManagementEvents": false,
                    "DataResources": [{
                        "Type": "AWS::S3::Object",
                        "Values": [format!("arn:aws:s3:::{bucket}/")],
                    }],
                }],
            }),
        )
        .await;
    emulator.served("StartLogging", &json!({"Name": trail})).await;
    emulator
        .served("GetTrailStatus", &json!({"Name": trail}))
        .await;
    let excluded = emulator.keys(bucket).await;
    assert!(
        excluded.is_empty(),
        "case management events excluded: expected no log file, got {excluded:?}"
    );

    // A trail logging the READ-ONLY class only: a read of the account is delivered, a write
    // is not.
    let emulator = Emulator::new();
    let bucket = "read-only-logs";
    let trail = "read-only-trail";
    emulator.log_bucket(bucket).await;
    emulator
        .served(
            "CreateTrail",
            &json!({"Name": trail, "S3BucketName": bucket}),
        )
        .await;
    emulator
        .served(
            "PutEventSelectors",
            &json!({
                "TrailName": trail,
                "EventSelectors": [{"ReadWriteType": "ReadOnly", "IncludeManagementEvents": true}],
            }),
        )
        .await;
    emulator.served("StartLogging", &json!({"Name": trail})).await;
    // A WRITE while the trail logs the read-only class only: re-applying the very selectors
    // it already carries, so what the trail logs does not change under the assertion.
    emulator
        .served(
            "PutEventSelectors",
            &json!({
                "TrailName": trail,
                "EventSelectors": [{"ReadWriteType": "ReadOnly", "IncludeManagementEvents": true}],
            }),
        )
        .await;
    let after_write = emulator.keys(bucket).await;
    assert!(
        after_write.is_empty(),
        "case a ReadOnly trail and a write call: expected no log file, got {after_write:?}"
    );
    emulator
        .served("GetTrailStatus", &json!({"Name": trail}))
        .await;
    let names = delivered_event_names(
        &emulator,
        "a ReadOnly trail and a read call",
        bucket,
        None,
    )
    .await;
    assert_eq!(
        names,
        vec!["GetTrailStatus".to_string()],
        "case a ReadOnly trail and a read call: expected only the read delivered, got {names:?}"
    );
}
