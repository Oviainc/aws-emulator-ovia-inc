// SPDX-License-Identifier: CC0-1.0

//! What a SageMaker create does with the Amazon S3 location its resource's work would READ,
//! and with the IAM role that read is made as — driven end to end over the real wire path
//! (SageMaker, Amazon S3 and IAM mounted on ONE shared engine, as the production router
//! mounts every service), so what is asserted is what a client sees.
//!
//! THE RULE UNDER TEST: before a create accepts a resource whose work reads an Amazon S3
//! location the request names, SageMaker resolves that location AS the execution role — a
//! location Amazon S3 answers no object for, and one whose bucket policy withholds that read
//! from the role, backs no work at all, so the call is refused 400 `ValidationException` and
//! nothing is stored. Only what Amazon S3 and the stored documents ANSWERED refuses: a bucket
//! with the object present and carrying no policy, and one whose policy allows the read, are
//! both accepted.
//!
//! A second rule, about the role itself: a notebook instance is ACCEPTED for a role this
//! account's IAM holds nothing under and then FAILS — the clock settles it into the modelled
//! `Failed` status with the `FailureReason` naming that role, never `InService` — while a
//! role IAM HOLDS whose trust document does not admit SageMaker is refused up front.
//!
//! The tables span what each rule generalises over: which member of the request carries the
//! location (a primary container's `ModelDataUrl`, its `ModelDataSource`, a pipeline
//! container's, a transform job's input), the shape of the location (an object, a prefix, a
//! bucket that does not exist), the bucket policy's effect, principal, action and resource,
//! and — for the role — whether IAM holds it and what its trust document admits.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call below is signed for and every ARN is scoped to.
pub(crate) const REGION: &str = "us-east-1";

/// The account id the emulator mints its ARNs under.
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The service principal a SageMaker execution role's trust document admits.
const SERVICE_PRINCIPAL: &str = service_com_amazonaws_sagemaker_sagemaker::state::SERVICE_PRINCIPAL;

/// Long enough to pass `state::NOTEBOOK_INSTANCE_IN_SERVICE_AFTER` (300 ms), so the status a
/// notebook instance reports is the one the clock already owed.
const PAST_NOTEBOOK_SETTLE_DUE: Duration = Duration::from_millis(600);

/// The bucket the artifacts of most rows live in.
const BUCKET: &str = "sagemaker-artifacts";

/// The object key the artifact of most rows lives at.
const ARTIFACT_KEY: &str = "models/model.tar.gz";

/// A SigV4 credential scope naming `service`: it is what routes a request to that service.
/// The key id and signature are not credentials — this harness turns request authentication
/// off, so nothing is verified.
pub(crate) fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// SageMaker, Amazon S3 and IAM on ONE shared engine, the way the router mounts every
/// service.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One SageMaker call, as a client makes it.
    pub(crate) async fn sagemaker(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header("authorization", authorization("sagemaker"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One SageMaker call that must be served.
    pub(crate) async fn sagemaker_ok(&self, case: &str, target: &str, body: &Value) -> Value {
        let (status, answered) = self.sagemaker(target, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {target}, got {status} with {answered}"
        );
        answered
    }

    /// One raw path-style request to the emulator's own Amazon S3 front door.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().to_vec())
    }

    /// `PUT /<bucket>` — `CreateBucket`.
    async fn given_bucket(&self, bucket: &str) {
        let (status, body) = self.s3("PUT", &format!("/{bucket}"), &[]).await;
        assert_eq!(
            status,
            200,
            "the bucket {bucket} must be created, got {status} {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// `PUT /<bucket>/<key>` — `PutObject`.
    async fn given_object(&self, bucket: &str, key: &str, body: &str) {
        let (status, answered) = self
            .s3("PUT", &format!("/{bucket}/{key}"), body.as_bytes())
            .await;
        assert_eq!(
            status,
            200,
            "the object {key} must be stored, got {status} {}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// `PUT /<bucket>?policy` — `PutBucketPolicy`, with `document` as the payload.
    async fn given_bucket_policy(&self, bucket: &str, document: &Value) {
        let (status, answered) = self
            .s3(
                "PUT",
                &format!("/{bucket}?policy"),
                document.to_string().as_bytes(),
            )
            .await;
        assert!(
            (200..300).contains(&status),
            "the policy of {bucket} must be stored, got {status} {}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// One IAM call (`awsQuery`), as a client makes it.
    async fn iam(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization("iam"))
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Hold `role_name` in IAM through its own `CreateRole`, with `trust` as its trust
    /// document, and answer the role's ARN.
    pub(crate) async fn given_role(&self, role_name: &str, trust: &str) -> String {
        let (status, body) = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", role_name),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "the role {role_name} must be created, got {status} {body}"
        );
        role_arn(role_name)
    }
}

/// The ARN a role of this account carries.
pub(crate) fn role_arn(role_name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{role_name}")
}

/// Percent-encode one `application/x-www-form-urlencoded` component with the maintained
/// RFC 3986 encoder, leaving only the unreserved characters literal.
fn form_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A trust document admitting `principal` to assume the role.
pub(crate) fn trust_for(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// A container naming the model's inference image and, when the row gives one, the location
/// its model data lives at.
fn container(model_data: Option<&str>) -> Value {
    let mut container = json!({
        "Image": format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/inference:1"),
    });
    if let Some(url) = model_data {
        container["ModelDataUrl"] = json!(url);
    }
    container
}

/// A bucket policy with one statement, built from the parts the rows vary.
fn bucket_policy(effect: &str, principal: Value, action: Value, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": principal,
            "Action": action,
            "Resource": resource,
        }],
    })
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The error message an answer carries, whichever casing the envelope spells it with.
pub(crate) fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A top-level string member of an answer.
pub(crate) fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE: `CreateModel` is refused, with nothing stored, when a container's model data names
/// an Amazon S3 location that holds no object — whichever member carries it and whichever
/// container of the model it belongs to; a location that holds one is registered.
#[tokio::test]
async fn a_model_whose_artifact_names_no_object_is_refused() {
    let emulator = Emulator::new();
    let role = emulator
        .given_role("artifact-reader", &trust_for(SERVICE_PRINCIPAL))
        .await;
    emulator.given_bucket(BUCKET).await;
    emulator
        .given_object(BUCKET, ARTIFACT_KEY, "a model archive")
        .await;
    emulator
        .given_object(BUCKET, "prefixed/one/model.tar.gz", "a model archive")
        .await;

    let held = format!("s3://{BUCKET}/{ARTIFACT_KEY}");
    let held_prefix = format!("s3://{BUCKET}/prefixed/");
    let absent_key = format!("s3://{BUCKET}/missing/model.tar.gz");
    let absent_prefix = format!("s3://{BUCKET}/missing/");
    let absent_bucket = "s3://sagemaker-no-such-bucket/model.tar.gz".to_string();

    // (case, the members laid over the create, the location the refusal must name — `None`
    // for a row the create accepts)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a primary container naming the object the bucket holds",
            json!({ "PrimaryContainer": container(Some(&held)) }),
            None,
        ),
        (
            "a primary container naming a prefix objects live under",
            json!({ "PrimaryContainer": container(Some(&held_prefix)) }),
            None,
        ),
        (
            "a primary container naming no model data at all",
            json!({ "PrimaryContainer": container(None) }),
            None,
        ),
        (
            "a model data source naming the object the bucket holds",
            json!({
                "PrimaryContainer": {
                    "Image": format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/inference:1"),
                    "ModelDataSource": {
                        "S3DataSource": {
                            "S3Uri": held,
                            "S3DataType": "S3Object",
                            "CompressionType": "Gzip",
                        },
                    },
                },
            }),
            None,
        ),
        (
            "a primary container naming a key nothing is stored under",
            json!({ "PrimaryContainer": container(Some(&absent_key)) }),
            Some(&absent_key),
        ),
        (
            "a primary container naming a prefix nothing lives under",
            json!({ "PrimaryContainer": container(Some(&absent_prefix)) }),
            Some(&absent_prefix),
        ),
        (
            "a primary container naming a bucket that does not exist",
            json!({ "PrimaryContainer": container(Some(&absent_bucket)) }),
            Some(&absent_bucket),
        ),
        (
            "a model data source naming a key nothing is stored under",
            json!({
                "PrimaryContainer": {
                    "Image": format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/inference:1"),
                    "ModelDataSource": {
                        "S3DataSource": {
                            "S3Uri": absent_key,
                            "S3DataType": "S3Object",
                            "CompressionType": "Gzip",
                        },
                    },
                },
            }),
            Some(&absent_key),
        ),
        (
            "a pipeline container naming a key nothing is stored under",
            json!({
                "Containers": [container(Some(&held)), container(Some(&absent_key))],
            }),
            Some(&absent_key),
        ),
    ];

    for (index, (case, members, refused_uri)) in rows.into_iter().enumerate() {
        let name = format!("artifactmodel{index}");
        let mut body = json!({ "ModelName": name, "ExecutionRoleArn": role });
        for (member, value) in members.as_object().expect("the row's members are an object") {
            body[member] = value.clone();
        }
        let (status, answered) = emulator.sagemaker("CreateModel", &body).await;
        let (read_status, read) = emulator
            .sagemaker("DescribeModel", &json!({ "ModelName": name }))
            .await;

        let Some(refused_uri) = refused_uri else {
            assert_eq!(
                status, 200,
                "case {case:?}: a readable artifact must be accepted, got {status} with \
                 {answered}"
            );
            assert_eq!(
                read_status, 200,
                "case {case:?}: the accepted create must store the model, got {read_status} \
                 with {read}"
            );
            continue;
        };
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: an unreadable artifact must be refused, got {status} with \
             {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("Could not find model data at {refused_uri}."),
            "case {case:?}: the refusal must name the location the model data is missing at, \
             got {:?}",
            message(&answered)
        );
        assert_eq!(
            read_status, 400,
            "case {case:?}: a refused create must store nothing, got {read_status} with {read}"
        );
    }
}

/// RULE: where the artifact's bucket carries a policy, that policy governs the read SageMaker
/// makes under the execution role: a policy that explicitly DENIES it refuses the create
/// whatever the role's own policies allow, and so does one that grants the role nothing —
/// while a policy that allows the read, and a bucket with no policy at all, refuse nothing.
#[tokio::test]
async fn a_bucket_policy_that_withholds_the_read_refuses_the_model() {
    let role = role_arn("artifact-reader");
    let object = format!("arn:aws:s3:::{BUCKET}/{ARTIFACT_KEY}");
    let every_object = format!("arn:aws:s3:::{BUCKET}/*");

    // (case, the policy on the bucket, whether the create is refused)
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        ("no policy at all", None, false),
        (
            "an allow for the role",
            Some(bucket_policy(
                "Allow",
                json!({ "AWS": role }),
                json!("s3:GetObject"),
                &every_object,
            )),
            false,
        ),
        (
            "a deny of the object for the role",
            Some(bucket_policy(
                "Deny",
                json!({ "AWS": role }),
                json!("s3:GetObject"),
                &object,
            )),
            true,
        ),
        (
            "a deny of every object for the role",
            Some(bucket_policy(
                "Deny",
                json!({ "AWS": role }),
                json!("s3:*"),
                &every_object,
            )),
            true,
        ),
        (
            "a deny for the whole account",
            Some(bucket_policy(
                "Deny",
                json!({ "AWS": ACCOUNT }),
                json!("s3:GetObject"),
                &every_object,
            )),
            true,
        ),
        (
            "a deny for every principal",
            Some(bucket_policy(
                "Deny",
                json!("*"),
                json!("s3:GetObject"),
                &every_object,
            )),
            true,
        ),
        (
            "a deny of another action",
            Some(bucket_policy(
                "Deny",
                json!({ "AWS": role }),
                json!("s3:PutObject"),
                &every_object,
            )),
            true,
        ),
        (
            "a policy that allows another principal only",
            Some(bucket_policy(
                "Allow",
                json!({ "AWS": role_arn("someone-else") }),
                json!("s3:GetObject"),
                &every_object,
            )),
            true,
        ),
        (
            "an allow scoped to another object",
            Some(bucket_policy(
                "Allow",
                json!({ "AWS": role }),
                json!("s3:GetObject"),
                &format!("arn:aws:s3:::{BUCKET}/elsewhere/*"),
            )),
            true,
        ),
        (
            "an allow for the SageMaker service principal",
            Some(bucket_policy(
                "Allow",
                json!({ "Service": SERVICE_PRINCIPAL }),
                json!("s3:GetObject"),
                &every_object,
            )),
            false,
        ),
        (
            "an allow for the whole account",
            Some(bucket_policy(
                "Allow",
                json!({ "AWS": ACCOUNT }),
                json!("s3:*"),
                &every_object,
            )),
            false,
        ),
        (
            "an allow of the object alone",
            Some(bucket_policy(
                "Allow",
                json!({ "AWS": role }),
                json!("s3:GetObject"),
                &object,
            )),
            false,
        ),
    ];

    for (index, (case, policy, refused)) in rows.into_iter().enumerate() {
        // One emulator per row: a bucket policy is a property of the bucket, so each row
        // stages its own.
        let emulator = Emulator::new();
        emulator
            .given_role("artifact-reader", &trust_for(SERVICE_PRINCIPAL))
            .await;
        // A bucket policy's `Principal.AWS` is resolved against IAM when it is stored, so
        // the role a row denies the read to has to be one the account holds.
        emulator
            .given_role("someone-else", &trust_for(SERVICE_PRINCIPAL))
            .await;
        emulator.given_bucket(BUCKET).await;
        emulator
            .given_object(BUCKET, ARTIFACT_KEY, "a model archive")
            .await;
        if let Some(document) = &policy {
            emulator.given_bucket_policy(BUCKET, document).await;
        }

        let name = format!("policymodel{index}");
        let body = json!({
            "ModelName": name,
            "ExecutionRoleArn": role,
            "PrimaryContainer": container(Some(&format!("s3://{BUCKET}/{ARTIFACT_KEY}"))),
        });
        let (status, answered) = emulator.sagemaker("CreateModel", &body).await;
        let (read_status, read) = emulator
            .sagemaker("DescribeModel", &json!({ "ModelName": name }))
            .await;

        if refused {
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ValidationException"),
                "case {case:?}: a policy that withholds the read must refuse the create, got \
                 {status} with {answered}"
            );
            assert_eq!(
                message(&answered),
                format!(
                    "Could not access model data at s3://{BUCKET}/{ARTIFACT_KEY}. Please \
                     ensure that the role \"{role}\" exists and that its trust relationship \
                     policy allows the action \"sts:AssumeRole\" for the service principal \
                     \"{SERVICE_PRINCIPAL}\". Also ensure that the role has \"s3:GetObject\" \
                     permissions and that the object is located in {REGION}."
                ),
                "case {case:?}: the refusal must name the artifact, the role and the Region, \
                 got {:?}",
                message(&answered)
            );
            assert_eq!(
                read_status, 400,
                "case {case:?}: a refused create must store nothing, got {read_status} with \
                 {read}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: nothing denies the read, so the create must be accepted, got \
             {status} with {answered}"
        );
        assert_eq!(
            read_status, 200,
            "case {case:?}: the accepted create must store the model, got {read_status} with \
             {read}"
        );
    }
}

/// RULE: `CreateTransformJob` is refused, with nothing stored, when the input location its
/// `TransformInput` names holds no object — the job would have nothing to transform — and
/// accepted when it holds one.
#[tokio::test]
async fn a_transform_job_whose_input_holds_no_object_is_refused() {
    let emulator = Emulator::new();
    let role = emulator
        .given_role("transform-reader", &trust_for(SERVICE_PRINCIPAL))
        .await;
    emulator.given_bucket(BUCKET).await;
    emulator
        .given_object(BUCKET, "input/one.csv", "1,2\n")
        .await;
    emulator
        .given_object(BUCKET, "single.csv", "1,2\n")
        .await;
    emulator
        .sagemaker_ok(
            "the model the jobs run",
            "CreateModel",
            &json!({
                "ModelName": "transformmodel",
                "ExecutionRoleArn": role,
                "PrimaryContainer": container(None),
            }),
        )
        .await;

    // (case, the input S3 URI, its S3DataType, whether the input holds an object)
    let rows: Vec<(&str, String, &str, bool)> = vec![
        (
            "a prefix objects live under",
            format!("s3://{BUCKET}/input/"),
            "S3Prefix",
            true,
        ),
        (
            "an object the bucket holds",
            format!("s3://{BUCKET}/single.csv"),
            "S3Prefix",
            true,
        ),
        (
            "a prefix with zero objects under it",
            format!("s3://{BUCKET}/empty-prefix/"),
            "S3Prefix",
            false,
        ),
        (
            "a key nothing is stored under",
            format!("s3://{BUCKET}/missing.csv"),
            "S3Prefix",
            false,
        ),
        (
            "a bucket that does not exist",
            "s3://sagemaker-no-such-input/data/".to_string(),
            "S3Prefix",
            false,
        ),
    ];

    for (index, (case, uri, data_type, readable)) in rows.into_iter().enumerate() {
        let name = format!("transformjob{index}");
        let body = json!({
            "TransformJobName": name,
            "ModelName": "transformmodel",
            "TransformInput": {
                "DataSource": { "S3DataSource": { "S3DataType": data_type, "S3Uri": uri } },
            },
            "TransformOutput": { "S3OutputPath": format!("s3://{BUCKET}/out/") },
            "TransformResources": { "InstanceType": "ml.m5.large", "InstanceCount": 1 },
        });
        let (status, answered) = emulator.sagemaker("CreateTransformJob", &body).await;
        let (read_status, read) = emulator
            .sagemaker("DescribeTransformJob", &json!({ "TransformJobName": name }))
            .await;

        if readable {
            assert_eq!(
                status, 200,
                "case {case:?}: an input that holds an object must be accepted, got {status} \
                 with {answered}"
            );
            assert_eq!(
                read_status, 200,
                "case {case:?}: the accepted create must store the job, got {read_status} with \
                 {read}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: an input holding no object must be refused, got {status} with \
             {answered}"
        );
        assert!(
            message(&answered).starts_with("No S3 objects found under S3 URL")
                && message(&answered).contains(&role),
            "case {case:?}: the refusal must name the location and the role, got {:?}",
            message(&answered)
        );
        assert_eq!(
            read_status, 400,
            "case {case:?}: a refused create must store nothing, got {read_status} with {read}"
        );
    }
}

/// RULE: a notebook instance created against a role this account's IAM holds NOTHING under
/// is accepted and then FAILS — it settles into the modelled `Failed` status carrying the
/// `FailureReason` that names the role, and never reaches `InService` — while a role IAM
/// HOLDS whose trust document does not admit SageMaker is refused up front, and one that does
/// comes up.
#[tokio::test]
async fn a_notebook_instance_whose_role_cannot_be_assumed_fails_instead_of_coming_up() {
    let emulator = Emulator::new();
    emulator
        .given_role("notebook-exec", &trust_for(SERVICE_PRINCIPAL))
        .await;
    emulator
        .given_role("notebook-lambda-only", &trust_for("lambda.amazonaws.com"))
        .await;

    // (case, the role the request names, the status the instance settles into — `None` when
    // the create itself is refused)
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        (
            "a role that trusts SageMaker",
            role_arn("notebook-exec"),
            Some("InService"),
        ),
        (
            "a role IAM holds nothing under",
            role_arn("does-not-exist"),
            Some("Failed"),
        ),
        (
            "a role of another account",
            "arn:aws:iam::123456789012:role/notebook-exec".to_string(),
            Some("InService"),
        ),
        (
            "a role that trusts another service only",
            role_arn("notebook-lambda-only"),
            None,
        ),
    ];

    for (index, (case, role, settled)) in rows.into_iter().enumerate() {
        let name = format!("notebook{index}");
        let body = json!({
            "NotebookInstanceName": name,
            "InstanceType": "ml.t3.medium",
            "RoleArn": role,
        });
        let (status, answered) = emulator.sagemaker("CreateNotebookInstance", &body).await;
        let Some(settled) = settled else {
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ValidationException"),
                "case {case:?}: a held role SageMaker cannot assume must be refused, got \
                 {status} with {answered}"
            );
            let (read_status, read) = emulator
                .sagemaker(
                    "DescribeNotebookInstance",
                    &json!({ "NotebookInstanceName": name }),
                )
                .await;
            assert_eq!(
                read_status, 400,
                "case {case:?}: a refused create must store nothing, got {read_status} with \
                 {read}"
            );
            continue;
        };
        assert_eq!(
            status, 200,
            "case {case:?}: the create must be accepted, got {status} with {answered}"
        );
        // Before the clock is due the instance is still provisioning, whichever status it is
        // owed: the span is never zero.
        let pending = emulator
            .sagemaker_ok(
                case,
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": name }),
            )
            .await;
        assert_eq!(
            str_field(&pending, "NotebookInstanceStatus"),
            "Pending",
            "case {case:?}: a fresh instance must report Pending, got {pending}"
        );

        tokio::time::sleep(PAST_NOTEBOOK_SETTLE_DUE).await;
        let read = emulator
            .sagemaker_ok(
                case,
                "DescribeNotebookInstance",
                &json!({ "NotebookInstanceName": name }),
            )
            .await;
        assert_eq!(
            str_field(&read, "NotebookInstanceStatus"),
            settled,
            "case {case:?}: expected the instance to settle {settled}, got {read}"
        );
        if settled == "Failed" {
            assert_eq!(
                str_field(&read, "FailureReason"),
                format!("Unable to assume role with RoleArn {role}"),
                "case {case:?}: a failed instance must say why, got {read}"
            );
            assert!(
                read.get("Url").is_none() && read.get("NetworkInterfaceId").is_none(),
                "case {case:?}: an instance that never came up has no endpoint to report, \
                 got {read}"
            );
        } else {
            assert!(
                read.get("FailureReason").is_none(),
                "case {case:?}: an instance that failed at nothing carries no FailureReason, \
                 got {read}"
            );
        }
    }
}
