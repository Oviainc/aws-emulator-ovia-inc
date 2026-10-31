// SPDX-License-Identifier: CC0-1.0

//! `CreateCustomPlugin`'s `location.s3Location.bucketArn` and what it has to NAME, end to
//! end over HTTP with the Amazon S3 service mounted beside kafkaconnect on one engine.
//!
//! The rule under test: a member documented as carrying the ARN of an S3 BUCKET
//! (`S3Location$bucketArn`, "The Amazon Resource Name (ARN) of an S3 bucket" —
//! `smithy_query(query="shape", shape="S3Location")`) is satisfied only by an ARN whose
//! resource field is a bucket name; an `s3` ARN whose resource names an object under a
//! bucket (`…:::bucket/key`) or a sub-resource such as an access point
//! (`…:accesspoint/name`) addresses no bucket, so it is refused as incorrect input with
//! the service's `Invalid parameter bucketArn: Invalid ARN.` (`BadRequestException`, HTTP
//! 400) and NOTHING is stored. Being a bucket ARN is also what lets the create resolve the
//! plugin's file at all, so an accepted row's plugin reads back with the `fileDescription`
//! the service computed from the artifact's bytes — the verification is never skipped for
//! an address this service cannot address a bucket by.
//!
//! The table's rows differ along what the rule generalises over: which part of the ARN
//! makes it a bucket ARN or not (the partition, the vendor field, the shape of the resource
//! field: a bare name, a name with a key under it, a nested key, a trailing separator, a
//! `/`-form and a `:`-form sub-resource, an empty resource), whether the region and account
//! fields are filled, and whether the object the value points at really exists in the store.
//!
//! The refusals are asserted twice over: with the object store mounted and with no object
//! store at all. The member is decided from the REQUEST, so neither answer may rest on what
//! a store said about the object.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN carries. No
/// authenticator is registered, so the signature is never checked; the scope is what routes
/// the request and names the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The same scope naming `s3`, which is what routes a request to the object store.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in both scopes above.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id, for the rows whose ARN fills the account field.
const ACCOUNT: &str = "000000000000";

/// The bucket most rows name, and the key the artifact is staged under in every staged
/// bucket.
const BUCKET: &str = "kafkaconnect-plugin-artifacts";

/// A second bucket, whose name carries the dots and digits S3's bucket naming rules allow.
const DOTTED_BUCKET: &str = "plugin.bucket-2024";

/// A third bucket, named at the longest S3's bucket naming rules allow: 63 characters
/// (asserted in [`given_staged_artifacts`], so the figure cannot drift).
const LONGEST_BUCKET: &str =
    "plugin-bucket-plugin-bucket-plugin-bucket-plugin-bucket-xxxxxxx";

/// The longest a bucket name may be under S3's bucket naming rules, in characters.
const MAX_BUCKET_NAME: usize = 63;

/// The key the artifact is staged under.
const FILE_KEY: &str = "plugin.zip";

/// The bytes staged under [`FILE_KEY`] in every staged bucket. Nothing here reads the
/// artifact's content.
const ARTIFACT: &[u8] = b"PK\x03\x04 emulator custom plugin artifact";

/// The message the service refuses an ARN-typed member with
/// (`crate::state::INVALID_ARN`, in `Invalid parameter <member>: <reason>` form).
const INVALID_BUCKET_ARN: &str = "Invalid parameter bucketArn: Invalid ARN.";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// A response header's value, or `""` when the response carries none.
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or_else(|error| {
                panic!(
                    "a JSON response body: {error}; got {:?}",
                    String::from_utf8_lossy(&self.body)
                )
            })
        }
    }
}

/// One emulator: kafkaconnect and Amazon S3 on a single shared engine behind the
/// production router, so the create's file resolution has a store to ask.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One kafkaconnect request (`restJson1`, the routes in `OPERATIONS.md`).
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> Answer {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("host", format!("kafkaconnect.{REGION}.amazonaws.com"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    answer(emulator, request).await
}

/// One path-style request to the emulator's S3 front door.
async fn s3(emulator: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    answer(emulator, request).await
}

/// Dispatch one request and read back its status, headers and body.
async fn answer(emulator: &MultiServiceRouter, request: http::Request<bytes::Bytes>) -> Answer {
    let (parts, body) = emulator.dispatch(request).await.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Stage the artifact in every bucket the rows name: `PUT /<bucket>` then
/// `PUT /<bucket>/<key>` for each.
///
/// Each create names its `LocationConstraint`, because a create that names none can only
/// mean us-east-1 and this suite signs for [`REGION`].
async fn given_staged_artifacts(emulator: &MultiServiceRouter) {
    assert_eq!(
        LONGEST_BUCKET.len(),
        MAX_BUCKET_NAME,
        "the longest-name row must be exactly {MAX_BUCKET_NAME} characters, got {}",
        LONGEST_BUCKET.len()
    );
    for bucket in [BUCKET, DOTTED_BUCKET, LONGEST_BUCKET] {
        let payload = format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        );
        let created = s3(emulator, "PUT", &format!("/{bucket}"), payload.as_bytes()).await;
        assert_eq!(
            created.status,
            200,
            "the bucket {bucket:?} must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let staged = s3(emulator, "PUT", &format!("/{bucket}/{FILE_KEY}"), ARTIFACT).await;
        assert_eq!(
            staged.status,
            200,
            "the artifact {bucket}/{FILE_KEY} must be stored, got {} with body {:?}",
            staged.status,
            String::from_utf8_lossy(&staged.body)
        );
    }
}

/// The `CreateCustomPlugin` body naming `bucket_arn` as the plugin file's bucket. Every row
/// addresses the one staged key, so the only thing a row varies is the `bucketArn`.
fn plugin_request(name: &str, bucket_arn: &str) -> Value {
    json!({
        "name": name,
        "contentType": "ZIP",
        "location": {"s3Location": {"bucketArn": bucket_arn, "fileKey": FILE_KEY}},
    })
}

/// A member of a JSON object as a string, for an assertion message.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS SDK
/// encodes a label.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The hex-encoded MD5 digest of `bytes`, computed in the test from the bytes it staged.
fn md5_hex(bytes: &[u8]) -> String {
    use md5::Digest as _;

    hex::encode(md5::Md5::digest(bytes))
}

/// Every `name` `ListCustomPlugins` reports.
async fn listed_names(emulator: &MultiServiceRouter) -> BTreeSet<String> {
    let answered = call(emulator, "GET", "/v1/custom-plugins", Value::Null).await;
    let listed = answered.json();
    assert_eq!(
        answered.status, 200,
        "ListCustomPlugins must succeed, got {} {listed}",
        answered.status
    );
    listed["customPlugins"]
        .as_array()
        .unwrap_or_else(|| panic!("a customPlugins array in {listed}"))
        .iter()
        .map(|summary| text(summary, "name"))
        .collect()
}

/// One row: what it varies, the plugin name it would take, the `bucketArn` it sends, and
/// the bucket that value names when that bucket is one this suite can stage in the store.
struct Row {
    case: &'static str,
    name: &'static str,
    bucket_arn: String,
    /// The bucket this row's `bucketArn` names, when that name is one the suite stages in
    /// the object store: the rows of [`bucket_arns`] carry `None` where the bucket name is
    /// outside S3's own naming rules (shorter than 3 characters), which the member's rule
    /// says nothing about, and every row of [`values_that_name_no_bucket`] carries `None`
    /// because its value names no bucket to stage in the first place.
    bucket: Option<&'static str>,
}

/// The rows whose `bucketArn` names a BUCKET: the three partitions AWS publishes (`aws`,
/// `aws-cn`, `aws-us-gov`), and resource fields spanning what a bucket name may hold — one
/// character, dots and digits, and the longest name S3's bucket naming rules allow
/// ([`MAX_BUCKET_NAME`] characters).
fn bucket_arns() -> Vec<Row> {
    vec![
        Row {
            case: "the commercial partition",
            name: "commercial-partition",
            bucket_arn: format!("arn:aws:s3:::{BUCKET}"),
            bucket: Some(BUCKET),
        },
        Row {
            case: "the China partition",
            name: "china-partition",
            bucket_arn: format!("arn:aws-cn:s3:::{BUCKET}"),
            bucket: Some(BUCKET),
        },
        Row {
            case: "the GovCloud partition",
            name: "govcloud-partition",
            bucket_arn: format!("arn:aws-us-gov:s3:::{BUCKET}"),
            bucket: Some(BUCKET),
        },
        Row {
            case: "a one-character bucket name",
            name: "shortest-bucket-name",
            bucket_arn: "arn:aws:s3:::b".to_string(),
            bucket: None,
        },
        Row {
            case: "a bucket name carrying dots and digits",
            name: "dotted-bucket-name",
            bucket_arn: format!("arn:aws:s3:::{DOTTED_BUCKET}"),
            bucket: Some(DOTTED_BUCKET),
        },
        Row {
            case: "a bucket name at the longest S3 allows",
            name: "longest-bucket-name",
            bucket_arn: format!("arn:aws:s3:::{LONGEST_BUCKET}"),
            bucket: Some(LONGEST_BUCKET),
        },
    ]
}

/// The rows whose `bucketArn` does NOT name a bucket: an object under a bucket (including
/// the staged object itself, so the refusal cannot be mistaken for a verdict about whether
/// the file is there), a nested key, a trailing separator, an access point in both the `/`
/// and the `:` form, an access-point object, a job, an empty resource field, a value that is
/// not an ARN at all, and an ARN of another service.
fn values_that_name_no_bucket() -> Vec<Row> {
    vec![
        Row {
            case: "an object ARN whose object really is staged",
            name: "staged-object-arn",
            bucket_arn: format!("arn:aws:s3:::{BUCKET}/{FILE_KEY}"),
            bucket: None,
        },
        Row {
            case: "an object ARN under a nested prefix",
            name: "nested-object-arn",
            bucket_arn: "arn:aws:s3:::plugin-bucket/connectors/debezium/plugin-1.0.0.jar"
                .to_string(),
            bucket: None,
        },
        Row {
            case: "a bucket ARN with a trailing separator",
            name: "trailing-separator",
            bucket_arn: "arn:aws:s3:::plugin-bucket/".to_string(),
            bucket: None,
        },
        Row {
            case: "an access point, region and account filled",
            name: "access-point",
            bucket_arn: format!("arn:aws:s3:{REGION}:{ACCOUNT}:accesspoint/plugins"),
            bucket: None,
        },
        Row {
            case: "an access point in the colon form",
            name: "access-point-colon-form",
            bucket_arn: format!("arn:aws:s3:{REGION}:{ACCOUNT}:accesspoint:plugins"),
            bucket: None,
        },
        Row {
            case: "an object reached through an access point",
            name: "access-point-object",
            bucket_arn: format!(
                "arn:aws:s3:{REGION}:{ACCOUNT}:accesspoint/plugins/object/{FILE_KEY}"
            ),
            bucket: None,
        },
        Row {
            case: "a batch job, not a bucket",
            name: "batch-job",
            bucket_arn: format!(
                "arn:aws:s3:{REGION}:{ACCOUNT}:job/00000000-0000-0000-0000-000000000000"
            ),
            bucket: None,
        },
        Row {
            case: "an empty resource field",
            name: "empty-resource",
            bucket_arn: "arn:aws:s3:::".to_string(),
            bucket: None,
        },
        Row {
            case: "a bare bucket name, not an ARN",
            name: "bare-bucket-name",
            bucket_arn: BUCKET.to_string(),
            bucket: None,
        },
        Row {
            case: "an ARN of another service",
            name: "another-service",
            bucket_arn: format!("arn:aws:sqs:{REGION}:{ACCOUNT}:plugin-bucket"),
            bucket: None,
        },
    ]
}

/// Every row whose value names no bucket, refused against one emulator.
///
/// `where_it_ran` names the setup in each assertion: the rule is decided from the MEMBER,
/// so the answer is the same whether or not there is an object store to ask and whether or
/// not the object the value points at is really there.
async fn every_value_naming_no_bucket_is_refused(
    emulator: &MultiServiceRouter,
    where_it_ran: &str,
) {
    for row in values_that_name_no_bucket() {
        let case = format!("{} {}", row.case, where_it_ran);
        let answered = call(
            emulator,
            "POST",
            "/v1/custom-plugins",
            plugin_request(row.name, &row.bucket_arn),
        )
        .await;
        let refused = answered.json();
        assert_eq!(
            answered.status, 400,
            "case {case:?}: expected HTTP 400 for the bucketArn {:?}, got {} {refused}",
            row.bucket_arn, answered.status
        );
        assert!(
            answered
                .header("x-amzn-errortype")
                .contains("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            answered.header("x-amzn-errortype")
        );
        let message = text(&refused, "message");
        assert_eq!(
            message, INVALID_BUCKET_ARN,
            "case {case:?}: expected the message {INVALID_BUCKET_ARN:?}, got {message:?}"
        );

        // Nothing was minted, so the name is still free and no later read can report it.
        let names = listed_names(emulator).await;
        assert!(
            !names.contains(row.name),
            "case {case:?}: a refused create must leave the store untouched, got {names:?}"
        );
    }

    assert!(
        listed_names(emulator).await.is_empty(),
        "no refused create may leave a plugin behind {where_it_ran}"
    );
}

#[tokio::test]
async fn a_bucket_arn_naming_no_bucket_is_refused_and_nothing_is_stored() {
    let emulator = emulator();
    given_staged_artifacts(&emulator).await;
    every_value_naming_no_bucket_is_refused(&emulator, "with the object store mounted").await;

    // And with no object store to ask: the member is judged from the request, so nothing
    // here rests on what a store answered — with no S3 mounted at all, these values are
    // still refused rather than stored with no file description.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut alone = MultiServiceRouter::new();
    alone.register_service(EmulatedService::new(engine));
    every_value_naming_no_bucket_is_refused(&alone, "with no object store to ask").await;
}

#[tokio::test]
async fn a_bucket_arn_naming_a_bucket_addresses_that_bucket() {
    let emulator = emulator();
    given_staged_artifacts(&emulator).await;

    for row in bucket_arns() {
        let answered = call(
            &emulator,
            "POST",
            "/v1/custom-plugins",
            plugin_request(row.name, &row.bucket_arn),
        )
        .await;
        let created = answered.json();

        // A row whose bucket this suite could stage resolves its file; a row naming a
        // bucket outside S3's own naming rules names a bucket no store holds, so the
        // store's answer refuses it on the OBJECT (`s3Location`) — never on the member,
        // which named a bucket.
        let Some(bucket) = row.bucket else {
            assert_eq!(
                answered.status, 400,
                "case {:?}: a bucket no store holds must be refused on the object, got {} \
                 {created}",
                row.case, answered.status
            );
            let message = text(&created, "message");
            assert!(
                message.contains("s3Location") && !message.contains("bucketArn"),
                "case {:?}: the refusal must name the object, not the member, got \
                 {message:?}",
                row.case
            );
            continue;
        };

        assert_eq!(
            answered.status, 200,
            "case {:?}: expected HTTP 200 for the bucketArn {:?}, got {} {created}",
            row.case, row.bucket_arn, answered.status
        );
        let arn = text(&created, "customPluginArn");
        assert!(
            !arn.is_empty(),
            "case {:?}: a created plugin carries its ARN, got {created}",
            row.case
        );

        // Round-trip: the stored plugin reads back with the location it was created from.
        let read = call(
            &emulator,
            "GET",
            &format!("/v1/custom-plugins/{}", label(&arn)),
            Value::Null,
        )
        .await;
        let described = read.json();
        assert_eq!(
            read.status, 200,
            "case {:?}: DescribeCustomPlugin must answer for the stored plugin, got {} \
             {described}",
            row.case, read.status
        );
        let revision = &described["latestRevision"];
        let s3_location = &revision["location"]["s3Location"];
        assert_eq!(
            text(s3_location, "bucketArn"),
            row.bucket_arn,
            "case {:?}: the described bucketArn must be the accepted one, got {s3_location}",
            row.case
        );

        // The bucket the ARN names is the bucket the file was read from, so the revision
        // reports the digest and the size the service computed from the bytes staged in
        // `bucket` — the verification the member's rule is what enables.
        let file = &revision["fileDescription"];
        assert_eq!(
            text(file, "fileMd5"),
            md5_hex(ARTIFACT),
            "case {:?}: the revision must report the digest of the artifact staged in \
             {bucket:?}, got {revision}",
            row.case
        );
        assert_eq!(
            file["fileSize"].as_i64().unwrap_or_default(),
            ARTIFACT.len() as i64,
            "case {:?}: the revision must report the size of the artifact staged in \
             {bucket:?}, got {revision}",
            row.case
        );
    }

    let names = listed_names(&emulator).await;
    let expected: BTreeSet<String> = bucket_arns()
        .iter()
        .filter(|row| row.bucket.is_some())
        .map(|row| row.name.to_string())
        .collect();
    assert_eq!(
        names, expected,
        "ListCustomPlugins must report exactly the plugins whose file resolved, got {names:?}"
    );
}

#[tokio::test]
async fn a_name_a_refused_bucket_arn_took_is_still_free() {
    let emulator = emulator();
    given_staged_artifacts(&emulator).await;

    // The issue's own address: the staged object named as if it were the bucket.
    let object_arn = format!("arn:aws:s3:::{BUCKET}/{FILE_KEY}");
    let refused = call(
        &emulator,
        "POST",
        "/v1/custom-plugins",
        plugin_request("retried", &object_arn),
    )
    .await;
    assert_eq!(
        refused.status,
        400,
        "the object ARN {object_arn:?} must be refused, got {} {}",
        refused.status,
        refused.json()
    );

    // The same name with the bucket's own ARN is free, and its file resolves.
    let accepted = call(
        &emulator,
        "POST",
        "/v1/custom-plugins",
        plugin_request("retried", &format!("arn:aws:s3:::{BUCKET}")),
    )
    .await;
    let created = accepted.json();
    assert_eq!(
        accepted.status, 200,
        "a retry with the bucket's own ARN must be accepted, got {} {created}",
        accepted.status
    );
    let described = call(
        &emulator,
        "GET",
        &format!(
            "/v1/custom-plugins/{}",
            label(&text(&created, "customPluginArn"))
        ),
        Value::Null,
    )
    .await
    .json();
    assert_eq!(
        text(&described["latestRevision"]["fileDescription"], "fileMd5"),
        md5_hex(ARTIFACT),
        "the retried plugin's file must have been taken on, got {described}"
    );
}
