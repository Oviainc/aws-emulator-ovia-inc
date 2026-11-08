// SPDX-License-Identifier: CC0-1.0

//! The DELIVERY GRANT a destination must carry before an export may be created against it,
//! over the awsJson1_1 wire with Amazon S3 beside this service on one engine.
//!
//! The rule under test: an operation that accepts an export definition evaluates the
//! destination bucket's own POLICY for a grant of the object write to this service's
//! principal, and refuses the call with HTTP 400 `ValidationException` carrying
//! [`BUCKET_PERMISSION_VALIDATION_FAILED`] when the policy does not grant it — persisting
//! nothing. The precondition is evaluated PER CALL, so the identical call refused against an
//! unpolicied bucket succeeds as soon as the policy is put on that same bucket.
//!
//! Why a client cares: the service writes the export into the bucket in its OWN name on the
//! cadence the definition states. A create that succeeded against a bucket that will not
//! accept the write would leave the client holding an ARN for an export that silently never
//! delivers, with no error branch ever taken and nothing to retry.
//!
//! It is asserted over a TABLE whose rows differ along what the rule generalises over: what
//! the policy grants (the action, the principal, the resource), what it conditions the grant
//! on (nothing, the account and the export ARN, an ARN in another Region), what form the
//! grant takes (a service principal, a wildcard principal, a wildcard action), whether a
//! `Deny` sits beside it and what that `Deny` is conditioned on (including the transport the
//! delivery itself is made over), which prefix of the bucket the export is delivered under,
//! and the Region the call is signed for — never along the identity of a bucket.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    BUCKET_PERMISSION_VALIDATION_FAILED, DELIVERY_ACTION, DELIVERY_SERVICE_PRINCIPAL,
};

use crate::export_lifecycle::{error_code, PINNED_REGION};

/// A SigV4 `Authorization` header in the `s3` scope: the credential scope is how the router
/// picks the service a request is for, so a request to the object-storage front door carries
/// one naming `s3`. Nothing here is verified — the harness turns request authentication off,
/// exactly as an unauthenticated emulator runs.
const SIGNED_FOR_S3: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTS3/20261021/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The access-key id every call below presents: an id neither STS nor IAM holds, which is
/// the account's own published credential and so is authorized (see
/// `tests/all/front_door_authorization.rs`). This suite is about the DESTINATION's policy,
/// not about the caller's.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// This emulator: this service, Amazon S3 and AWS Billing over ONE shared engine, so the
/// policy this service reads is the document S3's own `PutBucketPolicy` wrote, the bucket it
/// resolves is one S3's own `CreateBucket` made, and the billing view a definition names is
/// one AWS Billing itself reports.
///
/// `pub(crate)` so the suites that assert on the delivery ITSELF
/// (`tests/all/export_file_delivery.rs`) and on resolving the resources a definition names in
/// other services (`tests/all/referenced_resource_resolution.rs`) drive the same services
/// over the same engine through this ONE harness instead of growing a second copy of it.
pub(crate) struct Deliveries {
    router: MultiServiceRouter,
    /// The engine every service here is mounted on, so a suite can read the object store
    /// back and move a stored due instant the service itself reads.
    engine: Arc<engine::InMemoryEngine>,
}

impl Deliveries {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test: routing, the stored policy and the
        // decision are.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        // AWS Billing holds the account's billing views, which is where a definition's
        // `BILLING_VIEW_ARN` table property is resolved: a definition naming none (as every
        // row of this suite does) asks it nothing.
        router.register_service(service_com_amazonaws_billing_awsbilling::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// The engine every service here is mounted on.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// One call to AWS BILLING, signed for the pinned Region, answering the status and the
    /// parsed body that service served.
    pub(crate) async fn billing(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AWSBilling.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20261021/{PINNED_REGION}/\
                     billing/aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
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

    /// One call to this service, signed for `region`.
    pub(crate) async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSBillingAndCostManagementDataExports.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20261021/{region}/\
                     bcm-data-exports/aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
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

    /// One call to this service, signed for the pinned Region.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(PINNED_REGION, operation, body).await
    }

    /// One call to this service, requiring HTTP 200.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One request to the object-storage front door, path-style (the emulator serves one
    /// endpoint per Region, so the bucket travels in the path), answering the status and the
    /// body Amazon S3 served.
    pub(crate) async fn s3_request(&self, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let host = format!("s3.{PINNED_REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("authorization", SIGNED_FOR_S3)
            .header("content-type", "application/octet-stream")
            .body(Bytes::copy_from_slice(body))
            .expect("a well-formed S3 request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().to_vec())
    }

    /// One request to the object-storage front door, answering its status alone.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> u16 {
        self.s3_request(method, path, body).await.0
    }

    /// `PUT /<bucket>` — `CreateBucket`.
    pub(crate) async fn create_bucket(&self, bucket: &str) {
        let status = self.s3("PUT", &format!("/{bucket}"), &[]).await;
        assert_eq!(
            status, 200,
            "bucket {bucket:?}: expected CreateBucket 200, got {status}"
        );
    }

    /// `PUT /<bucket>?policy` — `PutBucketPolicy`.
    pub(crate) async fn put_bucket_policy(&self, bucket: &str, policy: &Value) {
        let document = serde_json::to_vec(policy).expect("a policy document serializes");
        let status = self.s3("PUT", &format!("/{bucket}?policy"), &document).await;
        assert_eq!(
            status, 204,
            "bucket {bucket:?}: expected PutBucketPolicy 204, got {status}"
        );
    }

    /// A bucket of this account carrying `policy`, or carrying none for `None`.
    pub(crate) async fn bucket_with(&self, bucket: &str, policy: Option<&Value>) {
        self.create_bucket(bucket).await;
        if let Some(policy) = policy {
            self.put_bucket_policy(bucket, policy).await;
        }
    }
}

/// The ARN of the bucket's objects, as a bucket policy names them: a bucket is global to
/// the partition, so the ARN carries neither Region nor account.
pub(crate) fn objects_of(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}/*")
}

/// One bucket-policy statement, as a document.
pub(crate) fn policy(statement: Value) -> Value {
    json!({ "Version": "2012-10-17", "Statement": [statement] })
}

/// An `Allow` of `actions` to `principal` on `resources`, optionally conditioned.
pub(crate) fn grant(
    principal: Value,
    actions: Value,
    resources: Value,
    condition: Option<Value>,
) -> Value {
    let mut statement = json!({
        "Sid": "DataExportsDelivery",
        "Effect": "Allow",
        "Principal": principal,
        "Action": actions,
        "Resource": resources,
    });
    if let Some(condition) = condition {
        statement
            .as_object_mut()
            .expect("a statement is a JSON object")
            .insert("Condition".to_string(), condition);
    }
    statement
}

/// The condition the delivery grant AWS publishes for this service carries: the account the
/// delivery is made for and the exports it is made for, matched with `StringLike` because
/// the export's own ARN is not known when the policy is written.
///
/// `region` is the Region segment of the export ARNs the condition covers — this service's
/// pinned Region for a policy that is to match, since that is the Region every ARN it mints
/// carries.
pub(crate) fn source_condition(region: &str) -> Value {
    json!({
        "StringLike": {
            "aws:SourceAccount": ::engine::emulator_account_id(),
            "aws:SourceArn": format!(
                "arn:aws:bcm-data-exports:{region}:{}:export/*",
                ::engine::emulator_account_id()
            ),
        }
    })
}

/// The delivery grant AWS publishes for a data-export destination bucket.
pub(crate) fn published_grant(bucket: &str) -> Value {
    policy(grant(
        json!({ "Service": [DELIVERY_SERVICE_PRINCIPAL] }),
        json!([DELIVERY_ACTION]),
        json!([objects_of(bucket)]),
        Some(source_condition(PINNED_REGION)),
    ))
}

/// A document carrying SEVERAL statements, for a row that puts one beside the grant.
fn document(statements: Vec<Value>) -> Value {
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// The statement a hardened bucket carries beside its grants: a `Deny` of every request
/// whose `aws:SecureTransport` is `secure` — "false" in AWS's own published idiom, which
/// refuses everything that did not arrive over TLS, and "true" for the row that states the
/// clause the delivery's own transport DOES satisfy.
fn deny_on_secure_transport(bucket: &str, secure: &str) -> Value {
    json!({
        "Sid": "DenyUnencryptedTransport",
        "Effect": "Deny",
        "Principal": "*",
        "Action": "s3:*",
        "Resource": [format!("arn:aws:s3:::{bucket}"), objects_of(bucket)],
        "Condition": { "Bool": { "aws:SecureTransport": secure } },
    })
}

/// One row of the table.
struct Row {
    label: &'static str,
    /// The policy put on the destination bucket, built from the bucket's own name, or
    /// `None` for a bucket created without one.
    policy: fn(&str) -> Option<Value>,
    /// The `S3Prefix` the export is delivered under.
    prefix: &'static str,
    /// The Region the create is signed for, which no ARN this service mints follows.
    signed_for: &'static str,
    /// Whether the create is served.
    served: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "the delivery grant AWS publishes",
            policy: |bucket| Some(published_grant(bucket)),
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: true,
        },
        // The same grant, for a client configured somewhere else: the condition names the
        // ARNs of this service's pinned Region, which is what it mints whatever Region the
        // call was signed for.
        Row {
            label: "the published grant, asked for by a client in another Region",
            policy: |bucket| Some(published_grant(bucket)),
            prefix: "exports/daily",
            signed_for: "us-west-2",
            served: true,
        },
        // The published grant as AWS's own documentation spells it: ONE statement granting
        // both the report service and this one, with the `aws:SourceArn` clause listing the
        // export ARNs of both — so the clause holds on any of its values.
        Row {
            label: "the published grant naming several source ARNs",
            policy: |bucket| {
                Some(policy(grant(
                    json!({ "Service": ["billingreports.amazonaws.com", DELIVERY_SERVICE_PRINCIPAL] }),
                    json!([DELIVERY_ACTION, "s3:GetBucketPolicy"]),
                    json!([objects_of(bucket), format!("arn:aws:s3:::{bucket}")]),
                    Some(json!({
                        "StringLike": {
                            "aws:SourceAccount": ::engine::emulator_account_id(),
                            "aws:SourceArn": [
                                format!(
                                    "arn:aws:cur:{PINNED_REGION}:{}:definition/*",
                                    ::engine::emulator_account_id()
                                ),
                                format!(
                                    "arn:aws:bcm-data-exports:{PINNED_REGION}:{}:export/*",
                                    ::engine::emulator_account_id()
                                ),
                            ],
                        }
                    })),
                )))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: true,
        },
        // The grant with no condition at all: an unconditional grant covers every delivery.
        Row {
            label: "an unconditional grant of the write",
            policy: |bucket| {
                Some(policy(grant(
                    json!({ "Service": DELIVERY_SERVICE_PRINCIPAL }),
                    json!(DELIVERY_ACTION),
                    json!(objects_of(bucket)),
                    None,
                )))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: true,
        },
        // A wildcard ACTION: every S3 action on the bucket's objects includes the write.
        Row {
            label: "a wildcard action granted to this service",
            policy: |bucket| {
                Some(policy(grant(
                    json!({ "Service": DELIVERY_SERVICE_PRINCIPAL }),
                    json!("s3:*"),
                    json!(objects_of(bucket)),
                    None,
                )))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: true,
        },
        // Scoped to exactly the objects this export's prefix is delivered under.
        Row {
            label: "a grant scoped to the delivery prefix",
            policy: |bucket| {
                Some(policy(grant(
                    json!({ "Service": [DELIVERY_SERVICE_PRINCIPAL] }),
                    json!([DELIVERY_ACTION]),
                    json!([format!("arn:aws:s3:::{bucket}/exports/daily/*")]),
                    None,
                )))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: true,
        },
        // The same grant where the export is delivered at the root of the bucket.
        Row {
            label: "a bucket-wide grant and an empty prefix",
            policy: |bucket| Some(published_grant(bucket)),
            prefix: "",
            signed_for: PINNED_REGION,
            served: true,
        },
        // The published grant on a HARDENED bucket: the delivery is made over a secure
        // transport, so the statement refusing everything that was not refuses nothing.
        Row {
            label: "the published grant beside a deny of what did not arrive over TLS",
            policy: |bucket| {
                Some(document(vec![
                    grant(
                        json!({ "Service": [DELIVERY_SERVICE_PRINCIPAL] }),
                        json!([DELIVERY_ACTION]),
                        json!([objects_of(bucket)]),
                        Some(source_condition(PINNED_REGION)),
                    ),
                    deny_on_secure_transport(bucket, "false"),
                ]))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: true,
        },
        // The same statement stating the clause the delivery's transport DOES satisfy:
        // that deny is about this write, so it refuses it.
        Row {
            label: "the published grant beside a deny of what DID arrive over TLS",
            policy: |bucket| {
                Some(document(vec![
                    grant(
                        json!({ "Service": [DELIVERY_SERVICE_PRINCIPAL] }),
                        json!([DELIVERY_ACTION]),
                        json!([objects_of(bucket)]),
                        Some(source_condition(PINNED_REGION)),
                    ),
                    deny_on_secure_transport(bucket, "true"),
                ]))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: false,
        },
        // A bucket created WITHOUT a policy: the recorded refusal.
        Row {
            label: "a bucket with no policy at all",
            policy: |_| None,
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: false,
        },
        // A policy whose only statement DENIES the write: nothing allows it, and the deny
        // would win even if something did.
        Row {
            label: "a policy that only denies the write",
            policy: |bucket| {
                Some(policy(json!({
                    "Sid": "NoDataExports",
                    "Effect": "Deny",
                    "Principal": { "Service": [DELIVERY_SERVICE_PRINCIPAL] },
                    "Action": [DELIVERY_ACTION],
                    "Resource": [objects_of(bucket)],
                })))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: false,
        },
        // The write granted to ANOTHER service principal.
        Row {
            label: "the write granted to another service",
            policy: |bucket| {
                Some(policy(grant(
                    json!({ "Service": ["cur.amazonaws.com"] }),
                    json!([DELIVERY_ACTION]),
                    json!([objects_of(bucket)]),
                    None,
                )))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: false,
        },
        // A DIFFERENT action granted to this service: reading the bucket is not writing it.
        Row {
            label: "another action granted to this service",
            policy: |bucket| {
                Some(policy(grant(
                    json!({ "Service": [DELIVERY_SERVICE_PRINCIPAL] }),
                    json!(["s3:GetObject", "s3:ListBucket"]),
                    json!([objects_of(bucket)]),
                    None,
                )))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: false,
        },
        // The write granted on another bucket's objects.
        Row {
            label: "the write granted on another bucket",
            policy: |_| {
                Some(policy(grant(
                    json!({ "Service": [DELIVERY_SERVICE_PRINCIPAL] }),
                    json!([DELIVERY_ACTION]),
                    json!([objects_of("somebody-elses-bucket")]),
                    None,
                )))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: false,
        },
        // The write granted under a prefix this export is not delivered under.
        Row {
            label: "the write granted under another prefix",
            policy: |bucket| {
                Some(policy(grant(
                    json!({ "Service": [DELIVERY_SERVICE_PRINCIPAL] }),
                    json!([DELIVERY_ACTION]),
                    json!([format!("arn:aws:s3:::{bucket}/other/*")]),
                    None,
                )))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: false,
        },
        // The published grant conditioned on the export ARNs of ANOTHER Region: no ARN this
        // service mints is in that Region, so the condition never holds.
        Row {
            label: "the grant conditioned on another Region's export ARNs",
            policy: |bucket| {
                Some(policy(grant(
                    json!({ "Service": [DELIVERY_SERVICE_PRINCIPAL] }),
                    json!([DELIVERY_ACTION]),
                    json!([objects_of(bucket)]),
                    Some(source_condition("us-west-2")),
                )))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: false,
        },
        // The published grant conditioned on another account.
        Row {
            label: "the grant conditioned on another account",
            policy: |bucket| {
                Some(policy(grant(
                    json!({ "Service": [DELIVERY_SERVICE_PRINCIPAL] }),
                    json!([DELIVERY_ACTION]),
                    json!([objects_of(bucket)]),
                    Some(json!({ "StringEquals": { "aws:SourceAccount": "111122223333" } })),
                )))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: false,
        },
        // An explicit `Deny` beside the published grant: the deny wins.
        Row {
            label: "an explicit deny beside the published grant",
            policy: |bucket| {
                Some(json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        grant(
                            json!({ "Service": [DELIVERY_SERVICE_PRINCIPAL] }),
                            json!([DELIVERY_ACTION]),
                            json!([objects_of(bucket)]),
                            None,
                        ),
                        {
                            "Sid": "NoDataExports",
                            "Effect": "Deny",
                            "Principal": { "Service": [DELIVERY_SERVICE_PRINCIPAL] },
                            "Action": [DELIVERY_ACTION],
                            "Resource": [objects_of(bucket)],
                        },
                    ],
                }))
            },
            prefix: "exports/daily",
            signed_for: PINNED_REGION,
            served: false,
        },
    ]
}

/// A `CreateExport` request for `name`, delivered to `bucket` under `prefix`.
fn create_request(name: &str, bucket: &str, prefix: &str) -> Value {
    json!({
        "Export": {
            "Name": name,
            "DataQuery": {
                "QueryStatement": "SELECT identity_line_item_id FROM COST_AND_USAGE_REPORT",
            },
            "DestinationConfigurations": {
                "S3Destination": {
                    "S3Bucket": bucket,
                    "S3Prefix": prefix,
                    "S3Region": PINNED_REGION,
                    "S3OutputConfigurations": {
                        "OutputType": "CUSTOM",
                        "Format": "TEXT_OR_CSV",
                        "Compression": "GZIP",
                        "Overwrite": "CREATE_NEW_REPORT",
                    },
                },
            },
            "RefreshCadence": { "Frequency": "SYNCHRONOUS" },
        },
    })
}

/// Assert `body` is the refusal a missing delivery grant is answered with.
fn assert_refused(case: &str, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
    );
    let message = body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a Message: {body}"));
    assert_eq!(
        message, BUCKET_PERMISSION_VALIDATION_FAILED,
        "case {case:?}: expected the recorded sentence, got {message:?}"
    );
}

/// RULE: a create is served exactly when the destination bucket's policy grants this
/// service the delivery write, and a refused create persists nothing.
#[tokio::test]
async fn a_create_is_served_only_where_the_bucket_policy_grants_the_delivery() {
    let emulator = Deliveries::new();
    let mut served_arns: Vec<String> = Vec::new();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.label;
        let bucket = format!("delivery-grant-{index}");
        emulator
            .bucket_with(&bucket, (row.policy)(&bucket).as_ref())
            .await;
        let name = format!("delivery_grant_{index}");
        let (status, body) = emulator
            .call_in(
                row.signed_for,
                "CreateExport",
                &create_request(&name, &bucket, row.prefix),
            )
            .await;
        if row.served {
            assert_eq!(
                status, 200,
                "case {case:?}: a granted destination must be served, got {status} with {body}"
            );
            let arn = body["ExportArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: CreateExport returns an ExportArn"))
                .to_string();
            // The export really is there, with the destination it was created for.
            let fetched = emulator.ok("GetExport", &json!({ "ExportArn": arn })).await;
            assert_eq!(
                fetched["Export"]["DestinationConfigurations"]["S3Destination"]["S3Bucket"],
                json!(bucket),
                "case {case:?}: GetExport must report the destination it was created for, got {fetched}"
            );
            served_arns.push(arn);
        } else {
            assert_refused(case, status, &body);
        }
    }

    // Nothing a refused create asked for happened: exactly the granted rows' exports exist.
    let listed = emulator.ok("ListExports", &json!({})).await;
    let arns: Vec<&str> = listed["Exports"]
        .as_array()
        .unwrap_or_else(|| panic!("ListExports returns Exports: {listed}"))
        .iter()
        .filter_map(|reference| reference["ExportArn"].as_str())
        .collect();
    assert_eq!(
        arns.len(),
        served_arns.len(),
        "only the granted destinations' exports may exist, got {arns:?}"
    );
    for arn in &served_arns {
        assert!(
            arns.contains(&arn.as_str()),
            "a served create must be listed, got {arns:?}"
        );
    }
}

/// RULE, the other half: the precondition is evaluated PER CALL. The identical request
/// refused against an unpolicied bucket is served once the grant is put on that same
/// bucket, and refused again once it is taken away.
#[tokio::test]
async fn the_grant_is_evaluated_on_every_call() {
    let emulator = Deliveries::new();
    let bucket = "delivery-grant-positive-control";
    emulator.create_bucket(bucket).await;
    let request = create_request("positive_control", bucket, "exports/daily");

    let (status, body) = emulator.call("CreateExport", &request).await;
    assert_refused("before the policy is put", status, &body);

    emulator
        .put_bucket_policy(bucket, &published_grant(bucket))
        .await;
    let created = emulator.ok("CreateExport", &request).await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    // The grant is read again on the next call: with the policy replaced by one that grants
    // nothing, a second create against the same bucket is refused.
    emulator
        .put_bucket_policy(
            bucket,
            &policy(grant(
                json!({ "Service": ["cur.amazonaws.com"] }),
                json!([DELIVERY_ACTION]),
                json!([objects_of(bucket)]),
                None,
            )),
        )
        .await;
    let (status, body) = emulator
        .call(
            "CreateExport",
            &create_request("positive_control_two", bucket, "exports/daily"),
        )
        .await;
    assert_refused("after the grant is taken away", status, &body);

    // And the export created while the grant was in force is untouched by either refusal.
    let fetched = emulator.ok("GetExport", &json!({ "ExportArn": arn })).await;
    assert_eq!(
        fetched["Export"]["Name"],
        json!("positive_control"),
        "the export created under the grant must still be readable, got {fetched}"
    );
}

/// RULE applied to the OTHER operation that accepts a destination: an update's replacement
/// destination is judged the same way, and a refused update leaves the stored definition as
/// it was.
#[tokio::test]
async fn an_update_judges_its_replacement_destination() {
    let emulator = Deliveries::new();
    let granted = "delivery-grant-update-granted";
    let ungranted = "delivery-grant-update-ungranted";
    emulator
        .bucket_with(granted, Some(&published_grant(granted)))
        .await;
    emulator.bucket_with(ungranted, None).await;

    let created = emulator
        .ok(
            "CreateExport",
            &create_request("update_destination", granted, "exports/daily"),
        )
        .await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    let replacement = |bucket: &str| {
        let mut request = create_request("update_destination", bucket, "exports/weekly");
        let object = request
            .as_object_mut()
            .expect("the request is a JSON object");
        object.insert("ExportArn".to_string(), json!(arn));
        // "All export parameters must be specified in the UpdateExport request": a
        // replacement definition restates the table configuration of each table it reads.
        object["Export"]["DataQuery"]
            .as_object_mut()
            .expect("DataQuery is a JSON object")
            .insert(
                "TableConfigurations".to_string(),
                json!({ "COST_AND_USAGE_REPORT": {} }),
            );
        request
    };
    let (status, body) = emulator
        .call("UpdateExport", &replacement(ungranted))
        .await;
    assert_refused("an update onto an unpolicied bucket", status, &body);
    let fetched = emulator.ok("GetExport", &json!({ "ExportArn": arn })).await;
    assert_eq!(
        fetched["Export"]["DestinationConfigurations"]["S3Destination"]["S3Bucket"],
        json!(granted),
        "a refused update must leave the stored destination as it was, got {fetched}"
    );

    // The same update onto a destination that DOES grant the delivery is served.
    let updated = emulator.ok("UpdateExport", &replacement(granted)).await;
    assert_eq!(
        updated["ExportArn"],
        json!(arn),
        "a granted update must report the ARN it updated, got {updated}"
    );
    let fetched = emulator.ok("GetExport", &json!({ "ExportArn": arn })).await;
    assert_eq!(
        fetched["Export"]["DestinationConfigurations"]["S3Destination"]["S3Prefix"],
        json!("exports/weekly"),
        "a served update must store the replacement destination, got {fetched}"
    );
}

/// A destination the object store REPORTS it does not hold is refused, not accepted: with
/// Amazon S3 beside this service, a bucket that was never created is a bucket that service
/// answers about, and no grant can be read off a bucket that is not there.
///
/// The rule — every resource of another service a definition names is resolved against the
/// service that holds it — is tabled in `tests/all/referenced_resource_resolution.rs`. This
/// row stands here so the unpolicied-bucket refusals above cannot be mistaken for the answer
/// to a destination that does not exist at all.
#[tokio::test]
async fn a_destination_the_object_store_reports_it_does_not_hold_is_refused() {
    let emulator = Deliveries::new();

    let (status, body) = emulator
        .call(
            "CreateExport",
            &create_request("no_such_bucket", "delivery-grant-never-created", "exports/daily"),
        )
        .await;
    assert_refused("a destination the object store does not hold", status, &body);
}
