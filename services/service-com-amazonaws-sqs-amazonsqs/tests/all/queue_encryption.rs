// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for server-side encryption of an SQS queue: which of the two
//! mutually exclusive options a queue is on and what it reports, and — for a queue on a
//! customer managed KMS key — which calls that key lets through.
//!
//! Driven over the wire through the same `MultiServiceRouter` the emulator's front doors
//! use. KMS is mounted BESIDE Amazon SQS on the same engine, so every key a case uses is
//! one KMS itself created, disabled, scheduled for deletion or holds a policy for; IAM
//! and STS are mounted with them, so a case that needs a principal OTHER than the
//! account's own credential makes one with their own operations (`CreateRole` +
//! `PutRolePolicy` + `AssumeRole`) and signs with the key STS minted for it. Nothing here
//! reaches into any store to plant a key, a policy or a decision.
//!
//! The tables span the classes the rules generalise over rather than one recorded
//! example:
//!
//! * the encryption a queue can be on (no attribute at all, the SQS-owned key enabled and
//!   disabled explicitly, a key id, a key ARN, an alias, an Amazon Web Services managed
//!   alias), and the ways a call moves a queue between the two options;
//! * the reasons a KMS key cannot serve a call (it resolves to nothing, it is disabled, it
//!   is pending deletion, it cannot encrypt at all, no policy grants the action, a policy
//!   denies the action, the grant is on another key) crossed with the two actions the two
//!   sides of the message plane perform on it (generating a data key for a send,
//!   decrypting one for a receive);
//! * where the grant lives: the key's own policy, and — for a key policy that delegates to
//!   IAM by naming the account root — the calling principal's identity-based policy;
//! * the published bounds of the data key reuse period, and one value past each.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{as_map, error_code, form_encode, xml_element, REGION};

// The sentence KMS itself words for a request addressing a resource through an ARN that
// names no account, asked of the KMS crate so a test cannot re-word it.
use ::service_com_amazonaws_kms_trentservice::state::unauthorized_account_message as kms_unauthorized_account_message;

/// The three attributes that describe a queue's server-side encryption, which is the
/// slice each projection row asserts over — so a row states the whole encryption the
/// queue reports, not just the attribute it set.
const ENCRYPTION_ATTRIBUTES: [&str; 3] = [
    "KmsMasterKeyId",
    "KmsDataKeyReusePeriodSeconds",
    "SqsManagedSseEnabled",
];

/// The access-key id standing for the account's own published credential: STS resolves an
/// id it never minted to the account's ambient identity, which holds no IAM policies and
/// is not refused by its own account's keys, exactly as account root credentials are not.
const ACCOUNT_KEY: &str = "AKIDTEST";

/// An emulator holding Amazon SQS, KMS, IAM and STS on one shared engine.
///
/// The shared harness mounts Amazon SQS alone, which is right for every suite that asks
/// nothing of another service; a queue on a customer managed key is resolved in KMS, and
/// the principal a call speaks for is resolved through STS and IAM.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One Amazon SQS call, signed with `access_key_id` in the credential scope.
    async fn sqs(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("sqs.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AmazonSQS.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/sqs/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One Amazon SQS call, as the account's own credential, that must succeed.
    async fn sqs_ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.sqs(ACCOUNT_KEY, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One KMS call, as the account's own credential. KMS is an awsJson1_1 service reached
    /// by `X-Amz-Target: TrentService.<Operation>`.
    async fn kms(&self, operation: &str, body: Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("kms.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TrentService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/kms/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "case KMS {operation} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One awsQuery call to IAM or STS.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create an IAM role with `policy` as its inline policy (when one is given), assume
    /// it through STS, and answer the session's access key id — the id a call made BY that
    /// role signs with.
    async fn assumed_role_key(&self, role_name: &str, policy: Option<&str>) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{}:root", ::engine::emulator_account_id())},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the role {role_name:?} must succeed, got {status} {body}"
        );
        // Every role here is a CALLER of Amazon SQS as well as a principal of the key: a
        // send or a receive is authorized against the queue's policies too, so each role
        // carries the service permission its call needs, beside whatever the KEY grants it
        // — which is what this suite varies.
        let service_permission = json!({
            "Version": "2012-10-17",
            "Statement": [{"Effect": "Allow", "Action": "sqs:*", "Resource": "*"}],
        })
        .to_string();
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.to_string()),
                    ("PolicyName", "sqs".to_string()),
                    ("PolicyDocument", service_permission),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "putting the service permission of {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", policy.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "putting the inline policy of {role_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!(
                            "arn:aws:iam::{}:role/{role_name}",
                            ::engine::emulator_account_id()
                        ),
                    ),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "assuming {role_name:?} must succeed, got {status} {body}"
        );
        xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {body}"))
    }

    /// Create an IAM user with `inline` as its inline policy and `attached` as the
    /// document of a customer-managed policy attached to it (each when given), and answer
    /// the access key id of a long-term key minted for it — the id a call made BY that
    /// user signs with.
    async fn user_key(
        &self,
        user_name: &str,
        inline: Option<&str>,
        attached: Option<&str>,
    ) -> String {
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the user {user_name:?} must succeed, got {status} {body}"
        );
        // The user is a CALLER of Amazon SQS as well as a principal of the key, exactly as
        // every role here is: it carries the service permission its call needs beside
        // whatever the KEY grants it ([`Self::assumed_role_key`]).
        let service_permission = json!({
            "Version": "2012-10-17",
            "Statement": [{"Effect": "Allow", "Action": "sqs:*", "Resource": "*"}],
        })
        .to_string();
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "PutUserPolicy",
                &[
                    ("UserName", user_name.to_string()),
                    ("PolicyName", "sqs".to_string()),
                    ("PolicyDocument", service_permission),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "putting the service permission of {user_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = inline {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "PutUserPolicy",
                    &[
                        ("UserName", user_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", policy.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "putting the inline policy of {user_name:?} must succeed, got {status} {body}"
            );
        }
        if let Some(document) = attached {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "CreatePolicy",
                    &[
                        ("PolicyName", format!("{user_name}-managed")),
                        ("PolicyDocument", document.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "creating the managed policy of {user_name:?} must succeed, got {status} {body}"
            );
            let arn = xml_element(&body, "Arn")
                .unwrap_or_else(|| panic!("CreatePolicy must answer the policy's Arn, got {body}"));
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "AttachUserPolicy",
                    &[
                        ("UserName", user_name.to_string()),
                        ("PolicyArn", arn.clone()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "attaching {arn} to {user_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        assert_eq!(
            status, 200,
            "minting a key for {user_name:?} must succeed, got {status} {body}"
        );
        xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey must answer an AccessKeyId, got {body}"))
    }

    /// Create a KMS key from `request` and answer `(key id, key ARN)`.
    async fn create_key(&self, request: Value) -> (String, String) {
        let created = self.kms("CreateKey", request).await;
        let metadata = &created["KeyMetadata"];
        (
            metadata["KeyId"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateKey reports a KeyId, got {created}"))
                .to_string(),
            metadata["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateKey reports an Arn, got {created}"))
                .to_string(),
        )
    }

    /// Create a queue carrying `attributes`, as the account's own credential, and answer
    /// its url.
    async fn queue(&self, name: &str, attributes: &Value) -> String {
        let mut body = json!({"QueueName": name});
        if attributes.as_object().is_some_and(|map| !map.is_empty()) {
            body["Attributes"] = attributes.clone();
        }
        let created = self.sqs_ok("CreateQueue", body).await;
        created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case create {name}: no QueueUrl in {created}"))
            .to_string()
    }

    /// Every attribute the queue at `url` has a value for.
    async fn attributes(&self, url: &str) -> BTreeMap<String, String> {
        let reported = self
            .sqs_ok(
                "GetQueueAttributes",
                json!({"QueueUrl": url, "AttributeNames": ["All"]}),
            )
            .await;
        as_map(&reported["Attributes"])
    }

    /// Send one message as `access_key_id` and answer `(status, error code)` — the code is
    /// the harness's `<no __type>` for an accepted send.
    async fn send(&self, access_key_id: &str, url: &str, body: &str) -> (u16, String) {
        let (status, parsed) = self
            .sqs(
                access_key_id,
                "SendMessage",
                &json!({"QueueUrl": url, "MessageBody": body}),
            )
            .await;
        (status, error_code(&parsed))
    }

    /// Receive as `access_key_id` and answer `(status, error code, bodies handed over)`.
    async fn receive(&self, access_key_id: &str, url: &str) -> (u16, String, Vec<String>) {
        let (status, parsed) = self
            .sqs(
                access_key_id,
                "ReceiveMessage",
                &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
            )
            .await;
        let bodies = parsed
            .get("Messages")
            .and_then(Value::as_array)
            .map(|messages| {
                messages
                    .iter()
                    .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        (status, error_code(&parsed), bodies)
    }
}

/// A key policy granting `actions` to every principal: a document that grants the listed
/// actions on the key directly, to whoever asks, and nothing else.
fn key_policy_allowing(actions: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "grant",
            "Effect": "Allow",
            "Principal": "*",
            "Action": actions,
            "Resource": "*",
        }],
    })
    .to_string()
}

/// A key policy granting every KMS action to every principal and denying `denied`, so an
/// explicit deny is what refuses the call rather than the absence of a grant.
fn key_policy_denying(denied: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "grant",
                "Effect": "Allow",
                "Principal": "*",
                "Action": "kms:*",
                "Resource": "*",
            },
            {
                "Sid": "refuse",
                "Effect": "Deny",
                "Principal": "*",
                "Action": denied,
                "Resource": "*",
            },
        ],
    })
    .to_string()
}

/// An identity-based policy allowing `actions` on `resource`, which is what a key policy
/// that delegates to IAM leaves the decision to.
fn identity_policy_allowing(actions: &[&str], resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": actions,
            "Resource": resource,
        }],
    })
    .to_string()
}

/// An identity-based policy allowing every KMS action and denying `denied`.
fn identity_policy_denying(denied: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "kms:*", "Resource": "*"},
            {"Effect": "Deny", "Action": denied, "Resource": "*"},
        ],
    })
    .to_string()
}

/// One row of the encryption-projection table: the attributes a queue is created with,
/// what a later `SetQueueAttributes` changes, and the whole encryption the queue must
/// then report.
struct ProjectionRow {
    case: &'static str,
    created_with: Value,
    then_set: Option<Value>,
    reports: Value,
}

/// A queue is always on exactly one of the two server-side encryption options, and what
/// it reports follows from the option it is on: the SQS-owned key for a queue nothing took
/// off it, a KMS key when one is named, and never both at once.
#[tokio::test]
async fn a_queue_reports_the_one_encryption_option_it_is_on() {
    let emulator = Emulator::new();
    let (key_id, key_arn) = emulator.create_key(json!({})).await;

    let rows = vec![
        ProjectionRow {
            case: "a queue created with no encryption attribute at all",
            created_with: json!({}),
            then_set: None,
            reports: json!({"SqsManagedSseEnabled": "true"}),
        },
        ProjectionRow {
            case: "the SQS-owned key enabled explicitly",
            created_with: json!({"SqsManagedSseEnabled": "true"}),
            then_set: None,
            reports: json!({"SqsManagedSseEnabled": "true"}),
        },
        ProjectionRow {
            case: "the SQS-owned key disabled explicitly",
            created_with: json!({"SqsManagedSseEnabled": "false"}),
            then_set: None,
            reports: json!({"SqsManagedSseEnabled": "false"}),
        },
        ProjectionRow {
            case: "a key id, which carries the documented reuse period with it",
            created_with: json!({"KmsMasterKeyId": key_id.clone()}),
            then_set: None,
            reports: json!({
                "KmsMasterKeyId": key_id.clone(),
                "KmsDataKeyReusePeriodSeconds": "300",
                "SqsManagedSseEnabled": "false",
            }),
        },
        ProjectionRow {
            case: "a key ARN and an explicit reuse period",
            created_with: json!({
                "KmsMasterKeyId": key_arn.clone(),
                "KmsDataKeyReusePeriodSeconds": "60",
            }),
            then_set: None,
            reports: json!({
                "KmsMasterKeyId": key_arn.clone(),
                "KmsDataKeyReusePeriodSeconds": "60",
                "SqsManagedSseEnabled": "false",
            }),
        },
        ProjectionRow {
            case: "an alias, which is a key identifier like any other",
            created_with: json!({"KmsMasterKeyId": "alias/aws/sqs"}),
            then_set: None,
            reports: json!({
                "KmsMasterKeyId": "alias/aws/sqs",
                "KmsDataKeyReusePeriodSeconds": "300",
                "SqsManagedSseEnabled": "false",
            }),
        },
        ProjectionRow {
            case: "a reuse period on a queue that is on no KMS key",
            created_with: json!({"KmsDataKeyReusePeriodSeconds": "600"}),
            then_set: None,
            reports: json!({
                "KmsDataKeyReusePeriodSeconds": "600",
                "SqsManagedSseEnabled": "true",
            }),
        },
        ProjectionRow {
            case: "a KMS key set on a queue that was on the SQS-owned one",
            created_with: json!({}),
            then_set: Some(json!({"KmsMasterKeyId": key_arn.clone()})),
            reports: json!({
                "KmsMasterKeyId": key_arn.clone(),
                "KmsDataKeyReusePeriodSeconds": "300",
                "SqsManagedSseEnabled": "false",
            }),
        },
        ProjectionRow {
            case: "the KMS key CLEARED, which takes the queue off it and off its \
                   reuse period without putting it back on the SQS-owned key",
            created_with: json!({"KmsMasterKeyId": key_arn.clone()}),
            then_set: Some(json!({"KmsMasterKeyId": ""})),
            reports: json!({"SqsManagedSseEnabled": "false"}),
        },
        ProjectionRow {
            case: "the KMS key cleared on a queue that carried an explicit reuse period",
            created_with: json!({
                "KmsMasterKeyId": key_arn.clone(),
                "KmsDataKeyReusePeriodSeconds": "120",
            }),
            then_set: Some(json!({"KmsMasterKeyId": ""})),
            reports: json!({"SqsManagedSseEnabled": "false"}),
        },
        ProjectionRow {
            case: "the SQS-owned key enabled on a queue that was on a KMS key",
            created_with: json!({"KmsMasterKeyId": key_arn.clone()}),
            then_set: Some(json!({"SqsManagedSseEnabled": "true"})),
            reports: json!({"SqsManagedSseEnabled": "true"}),
        },
        ProjectionRow {
            case: "the reuse period changed on a queue already on a KMS key",
            created_with: json!({"KmsMasterKeyId": key_arn.clone()}),
            then_set: Some(json!({"KmsDataKeyReusePeriodSeconds": "86400"})),
            reports: json!({
                "KmsMasterKeyId": key_arn.clone(),
                "KmsDataKeyReusePeriodSeconds": "86400",
                "SqsManagedSseEnabled": "false",
            }),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let url = emulator
            .queue(&format!("encrypted-{index}"), &row.created_with)
            .await;
        if let Some(change) = &row.then_set {
            emulator
                .sqs_ok(
                    "SetQueueAttributes",
                    json!({"QueueUrl": url, "Attributes": change}),
                )
                .await;
        }
        let reported = emulator.attributes(&url).await;
        let encryption: BTreeMap<String, String> = reported
            .iter()
            .filter(|(name, _)| ENCRYPTION_ATTRIBUTES.contains(&name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        assert_eq!(
            encryption,
            as_map(&row.reports),
            "case {}: the queue's encryption read {encryption:?}",
            row.case
        );

        // Asking for the three attributes by name reports exactly the same values: the
        // projection is one reading, not one per request shape.
        let named = emulator
            .sqs_ok(
                "GetQueueAttributes",
                json!({"QueueUrl": url, "AttributeNames": ENCRYPTION_ATTRIBUTES}),
            )
            .await;
        assert_eq!(
            as_map(named.get("Attributes").unwrap_or(&Value::Null)),
            as_map(&row.reports),
            "case {}: naming the attributes reported {named}",
            row.case
        );
    }
}

/// Only one server-side encryption option is supported per queue, so a request that asks
/// for a KMS key and for the SQS-owned key at once is refused — at create and on an
/// existing queue — and changes nothing.
#[tokio::test]
async fn a_request_asking_for_both_encryption_options_is_refused() {
    let emulator = Emulator::new();
    let (_, key_arn) = emulator.create_key(json!({})).await;
    // (case, the attributes one request supplies, whether they can hold together)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a KMS key alone",
            json!({"KmsMasterKeyId": key_arn.clone()}),
            true,
        ),
        (
            "the SQS-owned key alone",
            json!({"SqsManagedSseEnabled": "true"}),
            true,
        ),
        (
            "a KMS key beside the SQS-owned key DISABLED, which is one option",
            json!({"KmsMasterKeyId": key_arn.clone(), "SqsManagedSseEnabled": "false"}),
            true,
        ),
        (
            "an empty KMS key beside the SQS-owned key, which is also one option",
            json!({"KmsMasterKeyId": "", "SqsManagedSseEnabled": "true"}),
            true,
        ),
        (
            "a KMS key and the SQS-owned key at once",
            json!({"KmsMasterKeyId": key_arn.clone(), "SqsManagedSseEnabled": "true"}),
            false,
        ),
    ];

    for (index, (case, attributes, holds)) in rows.iter().enumerate() {
        let mut body = json!({"QueueName": format!("both-options-{index}")});
        body["Attributes"] = attributes.clone();
        let (status, parsed) = emulator.sqs(ACCOUNT_KEY, "CreateQueue", &body).await;
        if *holds {
            assert_eq!(
                status, 200,
                "case {case} at create: expected HTTP 200, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidAttributeName"),
                "case {case} at create: expected a 400 InvalidAttributeName, got {status} \
                 with {parsed}"
            );
        }

        // The same pair is judged the same way on an existing queue, and a refused set
        // leaves the queue on the option it was already on.
        let url = emulator.queue(&format!("both-set-{index}"), &json!({})).await;
        let (status, parsed) = emulator
            .sqs(
                ACCOUNT_KEY,
                "SetQueueAttributes",
                &json!({"QueueUrl": url, "Attributes": attributes}),
            )
            .await;
        let reported = emulator.attributes(&url).await;
        if *holds {
            assert_eq!(
                status, 200,
                "case {case} on an existing queue: expected HTTP 200, got {status} with \
                 {parsed}"
            );
        } else {
            assert_eq!(
                (
                    status,
                    error_code(&parsed).as_str(),
                    reported.get("KmsMasterKeyId").map(String::as_str),
                    reported.get("SqsManagedSseEnabled").map(String::as_str),
                ),
                (400, "InvalidAttributeName", None, Some("true")),
                "case {case} on an existing queue: expected a 400 InvalidAttributeName \
                 leaving the queue on the SQS-owned key, got {status} with {parsed} and \
                 {reported:?}"
            );
        }
    }
}

/// The data key reuse period is an integer attribute with published bounds: a value
/// inside them is stored and reported, and a value past either bound is refused.
#[tokio::test]
async fn the_data_key_reuse_period_takes_only_values_inside_its_published_range() {
    let emulator = Emulator::new();
    let (_, key_arn) = emulator.create_key(json!({})).await;
    // The bounds the attribute's own documentation publishes: "an integer representing
    // seconds, between 60 seconds (1 minute) and 86,400 seconds (24 hours)", with one
    // value past each.
    let rows: [(&str, &str, bool); 5] = [
        ("the lower bound", "60", true),
        ("the documented default", "300", true),
        ("the upper bound", "86400", true),
        ("one second below the lower bound", "59", false),
        ("one second above the upper bound", "86401", false),
    ];

    for (index, (case, value, accepted)) in rows.iter().enumerate() {
        let name = format!("reuse-period-{index}");
        let url = emulator
            .queue(&name, &json!({"KmsMasterKeyId": key_arn}))
            .await;
        let (status, parsed) = emulator
            .sqs(
                ACCOUNT_KEY,
                "SetQueueAttributes",
                &json!({
                    "QueueUrl": url,
                    "Attributes": {"KmsDataKeyReusePeriodSeconds": value},
                }),
            )
            .await;
        let reported = emulator.attributes(&url).await;
        let read = reported
            .get("KmsDataKeyReusePeriodSeconds")
            .cloned()
            .unwrap_or_default();
        if *accepted {
            assert_eq!(
                (status, read.as_str()),
                (200, *value),
                "case {case}: the set answered {status} with {parsed} and the queue read \
                 {reported:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str(), read.as_str()),
                (400, "InvalidAttributeValue", "300"),
                "case {case}: expected a 400 InvalidAttributeValue leaving the documented \
                 default in place, got {status} with {parsed} and {reported:?}"
            );
        }

        // The same value is judged the same way at create time.
        let mut body = json!({"QueueName": format!("{name}-at-create")});
        body["Attributes"] = json!({
            "KmsMasterKeyId": key_arn,
            "KmsDataKeyReusePeriodSeconds": value,
        });
        let (create_status, created) = emulator.sqs(ACCOUNT_KEY, "CreateQueue", &body).await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            create_status, expected,
            "case {case}: the create answered {create_status} with {created}"
        );
    }
}

/// One row of a KMS gate table: the key a queue is on, and what the two sides of the
/// message plane are answered for it.
struct GateRow {
    case: &'static str,
    /// The `KmsMasterKeyId` the queue carries; `None` for a queue on the SQS-owned key.
    master_key_id: Option<String>,
    /// The access key id the send and the receive are signed with.
    access_key_id: String,
    /// The error code a send must be refused with, or `None` when it must be accepted.
    send_refused_with: Option<&'static str>,
    /// The error code a receive must be refused with, or `None` when it must be accepted.
    receive_refused_with: Option<&'static str>,
}

/// Drive `rows` end to end: each row's queue takes a send and then a receive, and each is
/// either accepted or refused with the code the row names — with nothing enqueued by a
/// refused send and no body handed over by a refused receive.
async fn drive(emulator: &Emulator, prefix: &str, rows: &[GateRow]) {
    for (index, row) in rows.iter().enumerate() {
        let attributes = match &row.master_key_id {
            Some(key) => json!({"KmsMasterKeyId": key}),
            None => json!({}),
        };
        let url = emulator
            .queue(&format!("{prefix}-{index}"), &attributes)
            .await;
        let body = format!("{prefix} case {index}");

        let (status, code) = emulator.send(&row.access_key_id, &url, &body).await;
        match row.send_refused_with {
            Some(expected) => {
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {}: the send had to be refused {expected}",
                    row.case
                );
                // A refused send enqueues nothing: the queue is left as it was.
                let reported = emulator.attributes(&url).await;
                assert_eq!(
                    reported
                        .get("ApproximateNumberOfMessages")
                        .map(String::as_str),
                    Some("0"),
                    "case {}: a refused send must store nothing, got {reported:?}",
                    row.case
                );
            }
            None => assert_eq!(
                status, 200,
                "case {}: the send had to be accepted, got {status} / {code}",
                row.case
            ),
        }

        let (status, code, bodies) = emulator.receive(&row.access_key_id, &url).await;
        match row.receive_refused_with {
            Some(expected) => assert_eq!(
                (status, code.as_str(), bodies.as_slice()),
                (400, expected, &[][..]),
                "case {}: the receive had to be refused {expected} and hand over nothing",
                row.case
            ),
            None => {
                assert_eq!(
                    status, 200,
                    "case {}: the receive had to be accepted, got {status} / {code}",
                    row.case
                );
                if row.send_refused_with.is_none() {
                    assert_eq!(
                        bodies.as_slice(),
                        &[body.clone()][..],
                        "case {}: the receive had to hand the sent body over",
                        row.case
                    );
                }
            }
        }
    }
}

/// A send generates a data key under the queue's KMS key and a receive decrypts one, so
/// each is refused — with the KMS error of the refusal's class — when the key itself
/// cannot serve it: it resolves to nothing, or it is in no state to encrypt anything.
#[tokio::test]
async fn a_queue_whose_kms_key_cannot_serve_a_call_refuses_it() {
    let emulator = Emulator::new();
    let (_, usable) = emulator.create_key(json!({})).await;
    let (disabled_id, disabled) = emulator.create_key(json!({})).await;
    emulator
        .kms("DisableKey", json!({"KeyId": disabled_id}))
        .await;
    let (deleting_id, deleting) = emulator.create_key(json!({})).await;
    emulator
        .kms(
            "ScheduleKeyDeletion",
            json!({"KeyId": deleting_id, "PendingWindowInDays": 7}),
        )
        .await;
    let (_, signing) = emulator
        .create_key(json!({"KeyUsage": "SIGN_VERIFY", "KeySpec": "RSA_2048"}))
        .await;

    let rows = vec![
        GateRow {
            case: "the SQS-owned key, which asks nothing of KMS",
            master_key_id: None,
            access_key_id: ACCOUNT_KEY.to_string(),
            send_refused_with: None,
            receive_refused_with: None,
        },
        GateRow {
            case: "an Amazon Web Services managed key, whose policy AWS manages",
            master_key_id: Some("alias/aws/sqs".to_string()),
            access_key_id: ACCOUNT_KEY.to_string(),
            send_refused_with: None,
            receive_refused_with: None,
        },
        GateRow {
            case: "a usable key of this account, asked for by the account itself",
            master_key_id: Some(usable),
            access_key_id: ACCOUNT_KEY.to_string(),
            send_refused_with: None,
            receive_refused_with: None,
        },
        GateRow {
            case: "a disabled key",
            master_key_id: Some(disabled),
            access_key_id: ACCOUNT_KEY.to_string(),
            send_refused_with: Some("KmsDisabled"),
            receive_refused_with: Some("KmsDisabled"),
        },
        GateRow {
            case: "a key awaiting deletion",
            master_key_id: Some(deleting),
            access_key_id: ACCOUNT_KEY.to_string(),
            send_refused_with: Some("KmsInvalidState"),
            receive_refused_with: Some("KmsInvalidState"),
        },
        GateRow {
            case: "a key that signs rather than encrypts",
            master_key_id: Some(signing),
            access_key_id: ACCOUNT_KEY.to_string(),
            send_refused_with: Some("KmsInvalidKeyUsage"),
            receive_refused_with: Some("KmsInvalidKeyUsage"),
        },
        GateRow {
            case: "a key id no key answers to",
            master_key_id: Some("1234abcd-12ab-34cd-56ef-1234567890ab".to_string()),
            access_key_id: ACCOUNT_KEY.to_string(),
            send_refused_with: Some("KmsNotFound"),
            receive_refused_with: Some("KmsNotFound"),
        },
        GateRow {
            case: "an alias no key answers to",
            master_key_id: Some("alias/no-such-key".to_string()),
            access_key_id: ACCOUNT_KEY.to_string(),
            send_refused_with: Some("KmsNotFound"),
            receive_refused_with: Some("KmsNotFound"),
        },
        // A KMS ARN whose account segment is not an account id is refused for want of
        // permission before the resource it names is looked up, so neither form of it
        // reaches a key at all — not the key form and not the alias form.
        GateRow {
            case: "a key ARN whose account segment is not an account id",
            master_key_id: Some(format!(
                "arn:aws:kms:{REGION}:not-an-account:key/\
                 1234abcd-12ab-34cd-56ef-1234567890ab"
            )),
            access_key_id: ACCOUNT_KEY.to_string(),
            send_refused_with: Some("KmsAccessDenied"),
            receive_refused_with: Some("KmsAccessDenied"),
        },
        GateRow {
            case: "an alias ARN whose account segment is not an account id",
            master_key_id: Some(format!("arn:aws:kms:{REGION}:not-an-account:alias/aws/sqs")),
            access_key_id: ACCOUNT_KEY.to_string(),
            send_refused_with: Some("KmsAccessDenied"),
            receive_refused_with: Some("KmsAccessDenied"),
        },
    ];

    drive(&emulator, "keyless", &rows).await;
}

/// A `KmsMasterKeyId` that has the shape of a KMS key ARN but whose account segment is not
/// an account id names no account at all, so no account's permission can cover the key it
/// addresses: KMS refuses such a request for want of permission BEFORE resolving the
/// resource, and Amazon SQS relays that authorization failure — not a missing-key refusal —
/// to both sides of the message plane.
///
/// The account segment is judged by form, so the table spans the ways a segment can fail to
/// be one (`AccountIdType` is `@length(12, 12)` with `@pattern(^[0-9]{12}$)`) rather than
/// one example, and the sentence asserted is the one KMS itself words for the operation the
/// call performs.
#[tokio::test]
async fn a_queue_key_arn_whose_account_segment_names_no_account_relays_the_denial() {
    let emulator = Emulator::new();
    // A key that DOES exist, so what the rows below differ in is only the account segment
    // of the ARN they address it through.
    let (key_id, arn) = emulator.create_key(json!({})).await;

    // (case, the account segment the ARN carries)
    let segments: Vec<(&'static str, String)> = vec![
        ("a word", "not-an-account".to_string()),
        ("one digit short", "0".repeat(11)),
        ("one digit long", "0".repeat(13)),
        ("twelve characters, one of them a letter", format!("{}a", "0".repeat(11))),
        ("a wildcard", "*".to_string()),
        ("whitespace", " ".repeat(12)),
    ];

    let rows: Vec<GateRow> = segments
        .iter()
        .map(|(case, account)| GateRow {
            case,
            master_key_id: Some(format!("arn:aws:kms:{REGION}:{account}:key/{key_id}")),
            access_key_id: ACCOUNT_KEY.to_string(),
            // Not `KmsNotFound`: the key was never looked up.
            send_refused_with: Some("KmsAccessDenied"),
            receive_refused_with: Some("KmsAccessDenied"),
        })
        .collect();
    drive(&emulator, "accountless", &rows).await;

    // The relayed sentence is KMS's own for the KMS operation the call performs — a send
    // generates a data key, a receive decrypts one — naming the identifier the request
    // carried. Asked of KMS rather than re-worded here, so the two cannot drift apart.
    let addressed = format!("arn:aws:kms:{REGION}:not-an-account:key/{key_id}");
    let url = emulator
        .queue("accountless-message", &json!({"KmsMasterKeyId": addressed}))
        .await;
    for (operation, body, kms_operation) in [
        (
            "SendMessage",
            json!({"QueueUrl": url, "MessageBody": "sealed"}),
            "GenerateDataKey",
        ),
        (
            "ReceiveMessage",
            json!({"QueueUrl": url, "MaxNumberOfMessages": 1}),
            "Decrypt",
        ),
    ] {
        let (status, refused) = emulator.sqs(ACCOUNT_KEY, operation, &body).await;
        let expected = kms_unauthorized_account_message(kms_operation, &addressed);
        let message = refused
            .get("message")
            .or_else(|| refused.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            status, 400,
            "case {operation}: expected the relayed denial, got {status} with {refused}"
        );
        // The relay is KMS's sentence followed by the metadata block an AWS error carries,
        // whose request id is minted per call — so the sentence is the prefix, and the
        // block is checked for the code it reports rather than compared whole.
        assert!(
            message.starts_with(&expected),
            "case {operation}: expected the message to open with {expected:?}, got {message:?}"
        );
        assert!(
            message.contains("AccessDeniedException"),
            "case {operation}: expected the relayed KMS error code, got {message:?}"
        );
    }

    // The same key, addressed through an ARN that DOES name an account, is the account's
    // own key again: the denial is a property of the identifier, not of the key.
    let url = emulator
        .queue("accounted-message", &json!({"KmsMasterKeyId": arn.clone()}))
        .await;
    let (status, code) = emulator.send(ACCOUNT_KEY, &url, "sealed").await;
    assert_eq!(
        (status, code.as_str()),
        (200, "<no __type>"),
        "the key's own ARN ({arn}) must serve the send"
    );
}

/// A producer needs the data key generation grant on the queue's key and a consumer the
/// decryption grant, so a principal holding one, the other or neither is served exactly
/// the calls its grants cover — whether the grant lives in the key's own policy or, for a
/// key policy that delegates to the account, in the principal's identity policy.
#[tokio::test]
async fn a_queue_on_a_kms_key_serves_each_caller_only_the_calls_its_grants_cover() {
    let emulator = Emulator::new();
    let (_, both) = emulator
        .create_key(json!({"Policy": key_policy_allowing(&["kms:GenerateDataKey", "kms:Decrypt"])}))
        .await;
    let (_, send_only) = emulator
        .create_key(json!({"Policy": key_policy_allowing(&["kms:GenerateDataKey"])}))
        .await;
    let (_, receive_only) = emulator
        .create_key(json!({"Policy": key_policy_allowing(&["kms:Decrypt"])}))
        .await;
    let (_, neither) = emulator
        .create_key(json!({"Policy": key_policy_allowing(&["kms:DescribeKey"])}))
        .await;
    let (_, decrypt_denied) = emulator
        .create_key(json!({"Policy": key_policy_denying("kms:Decrypt")}))
        .await;
    // A key left to the DEFAULT key policy, which grants the account root and so hands the
    // decision to the calling principal's own identity policies.
    let (_, delegating) = emulator.create_key(json!({})).await;
    let (_, other_key) = emulator.create_key(json!({})).await;

    // Every row below is driven as a ROLE: the account's own published credential holds no
    // IAM policies and is not refused by its own account's keys, so it cannot show which
    // grant a call needs.
    let unprivileged = emulator.assumed_role_key("no-kms-policy", None).await;
    let full = emulator
        .assumed_role_key(
            "every-kms-action",
            Some(&identity_policy_allowing(&["kms:*"], "*")),
        )
        .await;
    let producer = emulator
        .assumed_role_key(
            "data-key-only",
            Some(&identity_policy_allowing(&["kms:GenerateDataKey"], "*")),
        )
        .await;
    let consumer = emulator
        .assumed_role_key(
            "decrypt-only",
            Some(&identity_policy_allowing(&["kms:Decrypt"], "*")),
        )
        .await;
    let on_another_key = emulator
        .assumed_role_key(
            "granted-elsewhere",
            Some(&identity_policy_allowing(
                &["kms:GenerateDataKey", "kms:Decrypt"],
                &other_key,
            )),
        )
        .await;
    let denied_decrypt = emulator
        .assumed_role_key(
            "decrypt-denied",
            Some(&identity_policy_denying("kms:Decrypt")),
        )
        .await;
    // The other kind of principal the IAM store resolves: a user signing with a long-term
    // key, whose grant may live in an inline policy or in a managed policy attached to it.
    let user_inline = emulator
        .user_key(
            "inline-consumer",
            Some(&identity_policy_allowing(&["kms:Decrypt"], "*")),
            None,
        )
        .await;
    let user_attached = emulator
        .user_key(
            "attached-operator",
            None,
            Some(&identity_policy_allowing(
                &["kms:GenerateDataKey", "kms:Decrypt"],
                "*",
            )),
        )
        .await;

    let rows = vec![
        GateRow {
            case: "a key policy granting both actions to whoever asks",
            master_key_id: Some(both),
            access_key_id: unprivileged.clone(),
            send_refused_with: None,
            receive_refused_with: None,
        },
        GateRow {
            case: "a key policy granting only the data key generation a producer needs",
            master_key_id: Some(send_only),
            access_key_id: unprivileged.clone(),
            send_refused_with: None,
            receive_refused_with: Some("KmsAccessDenied"),
        },
        GateRow {
            case: "a key policy granting only the decryption a consumer needs",
            master_key_id: Some(receive_only),
            access_key_id: unprivileged.clone(),
            send_refused_with: Some("KmsAccessDenied"),
            receive_refused_with: None,
        },
        GateRow {
            case: "a key policy granting neither action",
            master_key_id: Some(neither),
            access_key_id: unprivileged.clone(),
            send_refused_with: Some("KmsAccessDenied"),
            receive_refused_with: Some("KmsAccessDenied"),
        },
        GateRow {
            case: "a key policy granting everything and DENYING the decryption",
            master_key_id: Some(decrypt_denied),
            access_key_id: unprivileged.clone(),
            send_refused_with: None,
            receive_refused_with: Some("KmsAccessDenied"),
        },
        GateRow {
            case: "a delegating key policy and a principal with no policy at all",
            master_key_id: Some(delegating.clone()),
            access_key_id: unprivileged,
            send_refused_with: Some("KmsAccessDenied"),
            receive_refused_with: Some("KmsAccessDenied"),
        },
        GateRow {
            case: "a delegating key policy and a principal allowed every KMS action",
            master_key_id: Some(delegating.clone()),
            access_key_id: full,
            send_refused_with: None,
            receive_refused_with: None,
        },
        GateRow {
            case: "a delegating key policy and a principal allowed only the producer's \
                   action",
            master_key_id: Some(delegating.clone()),
            access_key_id: producer,
            send_refused_with: None,
            receive_refused_with: Some("KmsAccessDenied"),
        },
        GateRow {
            case: "a delegating key policy and a principal allowed only the consumer's \
                   action",
            master_key_id: Some(delegating.clone()),
            access_key_id: consumer,
            send_refused_with: Some("KmsAccessDenied"),
            receive_refused_with: None,
        },
        GateRow {
            case: "a delegating key policy and a principal whose grant names ANOTHER key",
            master_key_id: Some(delegating.clone()),
            access_key_id: on_another_key,
            send_refused_with: Some("KmsAccessDenied"),
            receive_refused_with: Some("KmsAccessDenied"),
        },
        GateRow {
            case: "a delegating key policy and a principal whose policy denies the \
                   decryption",
            master_key_id: Some(delegating.clone()),
            access_key_id: denied_decrypt,
            send_refused_with: None,
            receive_refused_with: Some("KmsAccessDenied"),
        },
        GateRow {
            case: "a delegating key policy and an IAM user whose INLINE policy allows only \
                   the consumer's action",
            master_key_id: Some(delegating.clone()),
            access_key_id: user_inline,
            send_refused_with: Some("KmsAccessDenied"),
            receive_refused_with: None,
        },
        GateRow {
            case: "a delegating key policy and an IAM user whose grant lives in an \
                   ATTACHED managed policy",
            master_key_id: Some(delegating),
            access_key_id: user_attached,
            send_refused_with: None,
            receive_refused_with: None,
        },
    ];

    drive(&emulator, "granted", &rows).await;
}

/// The grant is read at the instant of the call: a message a consumer could read stops
/// being handed over as soon as the key stops permitting the decryption, and is handed
/// over again once it permits it — the refusal never consumes the message, and the body is
/// never handed to a caller without the grant.
#[tokio::test]
async fn withdrawing_the_decrypt_grant_withholds_a_body_the_consumer_could_once_read() {
    let emulator = Emulator::new();
    let (key_id, key_arn) = emulator
        .create_key(json!({"Policy": key_policy_allowing(&["kms:GenerateDataKey", "kms:Decrypt"])}))
        .await;
    let caller = emulator.assumed_role_key("grant-watcher", None).await;
    let url = emulator
        .queue("grant-withdrawn", &json!({"KmsMasterKeyId": key_arn}))
        .await;

    let (status, code) = emulator.send(&caller, &url, "readable-while-granted").await;
    assert_eq!(
        (status, code.as_str()),
        (200, "<no __type>"),
        "the send under a key granting both actions had to be accepted"
    );
    let (_, _, bodies) = emulator.receive(&caller, &url).await;
    assert_eq!(
        bodies.as_slice(),
        &["readable-while-granted".to_string()][..],
        "the body had to come over while the key permitted the decryption"
    );

    // The consumer loses the decrypt grant. The message above is still in the queue — the
    // receive only made it invisible for its visibility timeout — so the queue is emptied
    // and refilled under the narrowed policy.
    emulator.sqs_ok("PurgeQueue", json!({"QueueUrl": url})).await;
    emulator
        .kms(
            "PutKeyPolicy",
            json!({
                "KeyId": key_id,
                "PolicyName": "default",
                "Policy": key_policy_allowing(&["kms:GenerateDataKey"]),
            }),
        )
        .await;
    let (status, code) = emulator.send(&caller, &url, "withheld-without-decrypt").await;
    assert_eq!(
        (status, code.as_str()),
        (200, "<no __type>"),
        "the producer still holds the data key grant, so its send had to be accepted"
    );
    let (status, code, bodies) = emulator.receive(&caller, &url).await;
    assert_eq!(
        (status, code.as_str(), bodies.as_slice()),
        (400, "KmsAccessDenied", &[][..]),
        "a consumer without the decrypt grant must never be handed the body"
    );

    // The grant comes back, and the message that was never handed over is still there.
    emulator
        .kms(
            "PutKeyPolicy",
            json!({
                "KeyId": key_id,
                "PolicyName": "default",
                "Policy": key_policy_allowing(&["kms:GenerateDataKey", "kms:Decrypt"]),
            }),
        )
        .await;
    let (_, _, bodies) = emulator.receive(&caller, &url).await;
    assert_eq!(
        bodies.as_slice(),
        &["withheld-without-decrypt".to_string()][..],
        "the refused receive must not have consumed the message"
    );
}

/// A batch send is gated on the same key as a single send: the key belongs to the queue,
/// so a caller without the grant has the whole request refused and nothing is enqueued.
#[tokio::test]
async fn a_batch_send_is_gated_on_the_queues_key_as_a_single_send_is() {
    let emulator = Emulator::new();
    let (_, granted) = emulator
        .create_key(json!({"Policy": key_policy_allowing(&["kms:GenerateDataKey"])}))
        .await;
    let (_, ungranted) = emulator
        .create_key(json!({"Policy": key_policy_allowing(&["kms:DescribeKey"])}))
        .await;
    let caller = emulator.assumed_role_key("batch-sender", None).await;
    let entries = json!([
        {"Id": "one", "MessageBody": "first"},
        {"Id": "two", "MessageBody": "second"},
    ]);
    // (case, the key the queue is on, the expected status, the expected error code)
    let rows: [(&str, &str, u16, &str); 2] = [
        (
            "a key granting the data key generation",
            &granted,
            200,
            "<no __type>",
        ),
        (
            "a key granting neither action",
            &ungranted,
            400,
            "KmsAccessDenied",
        ),
    ];

    for (index, (case, key, status, code)) in rows.iter().enumerate() {
        let url = emulator
            .queue(
                &format!("batch-gated-{index}"),
                &json!({"KmsMasterKeyId": key}),
            )
            .await;
        let (reached, parsed) = emulator
            .sqs(
                &caller,
                "SendMessageBatch",
                &json!({"QueueUrl": url, "Entries": entries}),
            )
            .await;
        assert_eq!(
            (reached, error_code(&parsed).as_str()),
            (*status, *code),
            "case {case}: the batch answered {reached} with {parsed}"
        );
        let reported = emulator.attributes(&url).await;
        let expected_count = if *status == 200 { "2" } else { "0" };
        assert_eq!(
            reported
                .get("ApproximateNumberOfMessages")
                .map(String::as_str),
            Some(expected_count),
            "case {case}: the queue holds the wrong number of messages, got {reported:?}"
        );
    }
}

/// A queue whose `KmsMasterKeyId` is a KMS ARN whose account segment is not an account id
/// names no key this endpoint can decide the call against: KMS refuses such an identifier
/// for want of permission, BEFORE the resource it names is looked up, so both calls the
/// queue's key gates are refused with the KMS access refusal and nothing is enqueued.
///
/// The rows differ in the form of the ARN — a key ARN and an alias ARN — and in what
/// stands where the account id belongs.
#[tokio::test]
async fn a_kms_arn_whose_account_segment_is_not_an_account_id_refuses_both_gated_calls() {
    let emulator = Emulator::new();
    let caller = emulator.assumed_role_key("account-less-arn", None).await;
    // (case, the `KmsMasterKeyId` the queue carries)
    let rows: [(&str, &str); 3] = [
        (
            "a key ARN whose account segment is a name",
            "arn:aws:kms:us-east-1:my-account:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        ),
        (
            "a key ARN whose account segment is too short to be an account id",
            "arn:aws:kms:us-east-1:12345:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        ),
        (
            "an alias ARN whose account segment is a name",
            "arn:aws:kms:us-east-1:tenant-a:alias/aws/sqs",
        ),
    ];

    for (index, (case, master_key_id)) in rows.iter().enumerate() {
        let url = emulator
            .queue(
                &format!("account-less-{index}"),
                &json!({"KmsMasterKeyId": master_key_id}),
            )
            .await;
        let (status, code) = emulator.send(&caller, &url, "never-sealed").await;
        assert_eq!(
            (status, code.as_str()),
            (400, "KmsAccessDenied"),
            "case {case}: the send had to be refused as the key refusal KMS answers"
        );
        let (status, code, bodies) = emulator.receive(&caller, &url).await;
        assert_eq!(
            (status, code.as_str(), bodies.as_slice()),
            (400, "KmsAccessDenied", &[][..]),
            "case {case}: the receive had to be refused the same way and hand over nothing"
        );
        let reported = emulator.attributes(&url).await;
        assert_eq!(
            reported
                .get("ApproximateNumberOfMessages")
                .map(String::as_str),
            Some("0"),
            "case {case}: a refused send must enqueue nothing, got {reported:?}"
        );
    }
}

