// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the gate a customer managed key's KEY POLICY puts on associating it
//! with a CloudWatch Logs resource.
//!
//! CloudWatch Logs and KMS are mounted on ONE shared engine — the same assembly
//! `core/router` performs — so the key policy the gate reads is one KMS itself holds, and a
//! create that is accepted or refused here is decided by a document no part of this suite
//! handed to CloudWatch Logs.
//!
//! The rule under test: a `kmsKeyId` may be associated with a resource only when the KEY's
//! own key policy lets CloudWatch Logs use the key for THAT resource — the grant is scoped by
//! the encryption-context key CloudWatch Logs passes the encrypted resource's ARN in — and an
//! association the policy does not cover is refused with `AccessDeniedException` and `The
//! specified KMS key does not exist or is not allowed to be used with Arn '<arn>'`, leaving
//! nothing created or changed.
//!
//! The table spans the classes the rule generalises over: which resource the policy names,
//! whether it names it exactly or by a wildcard, whether it grants the global or the Regional
//! service principal, and whether it grants CloudWatch Logs anything at all. Each row is
//! driven through both operations that take a `kmsKeyId` for a log group — `CreateLogGroup`
//! and `AssociateKmsKey`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for, which is the Region the ARNs carry.
const REGION: &str = "eu-west-2";

/// The error code an association the key policy does not cover is refused with.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// CloudWatch Logs and KMS on one shared engine, reached over the wire.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope is still carried, because it is what names the Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_kms_trentservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One awsJson1_1 call to the service signing as `service` under `prefix`.
    async fn call(&self, service: &str, prefix: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One CloudWatch Logs call: its status and body.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call("logs", "Logs_20140328", operation, body).await
    }

    /// One CloudWatch Logs call that must succeed.
    async fn logs_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.logs(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// Create a KMS key holding `policy`, answering its ARN — the key the association names.
    async fn created_key(&self, case: &str, policy: &str) -> String {
        let (status, parsed) = self
            .call(
                "kms",
                "TrentService",
                "CreateKey",
                &json!({"Policy": policy, "BypassPolicyLockoutSafetyCheck": true}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateKey must succeed, got {status} {parsed}"
        );
        parsed["KeyMetadata"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a created key carries an Arn, got {parsed}"))
            .to_string()
    }

    /// Whether CloudWatch Logs holds a log group under `name`.
    async fn holds_group(&self, case: &str, name: &str) -> bool {
        let page = self
            .logs_ok(
                case,
                "DescribeLogGroups",
                &json!({"logGroupIdentifiers": [name]}),
            )
            .await;
        page["logGroups"]
            .as_array()
            .is_some_and(|groups| !groups.is_empty())
    }

    /// The `kmsKeyId` CloudWatch Logs reports for `name`, or `None` when it reports none.
    async fn reported_key(&self, case: &str, name: &str) -> Option<String> {
        let page = self
            .logs_ok(
                case,
                "DescribeLogGroups",
                &json!({"logGroupIdentifiers": [name]}),
            )
            .await;
        page["logGroups"][0]
            .get("kmsKeyId")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// A log group's ARN in the form a key policy's encryption-context condition names it by —
/// `LogGroup$logGroupArn`, which carries no trailing `:*`.
fn log_group_arn(name: &str) -> String {
    format!(
        "arn:aws:logs:{REGION}:{}:log-group:{name}",
        engine::emulator_account_id()
    )
}

/// The statement every key policy carries so the key stays manageable by its account, as
/// KMS's default key policy words it.
fn account_statement() -> Value {
    json!({
        "Sid": "Enable IAM User Permissions",
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
        "Action": "kms:*",
        "Resource": "*",
    })
}

/// A key policy granting `principal` the CloudWatch Logs encryption operations, scoped by
/// `condition` when one is given.
fn key_policy(principal: &str, condition: Option<Value>) -> String {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": {"Service": principal},
        "Action": [
            "kms:Encrypt*",
            "kms:Decrypt*",
            "kms:ReEncrypt*",
            "kms:GenerateDataKey*",
            "kms:Describe*",
        ],
        "Resource": "*",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({
        "Version": "2012-10-17",
        "Statement": [account_statement(), statement],
    })
    .to_string()
}

/// A key policy that grants CloudWatch Logs nothing: the account statement alone, which is
/// what a key created with the default policy carries.
fn account_only_policy() -> String {
    json!({"Version": "2012-10-17", "Statement": [account_statement()]}).to_string()
}

/// One row of the table: a key policy, and the group it may be used for.
struct Row {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The key policy the key is created with, built from the group the policy names.
    policy: fn(&str) -> String,
    /// Whether the policy covers the group the association names.
    covers: bool,
}

/// The group a row's policy names, and the group it is then used for.
const NAMED_GROUP: &str = "scope/named";
const OTHER_GROUP: &str = "scope/other";

/// The rows, in the order they are driven.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the policy names this very log group",
            policy: |group| {
                key_policy(
                    "logs.amazonaws.com",
                    Some(json!({"ArnEquals": {"kms:EncryptionContext:aws:logs:arn": log_group_arn(group)}})),
                )
            },
            covers: true,
        },
        Row {
            case: "the policy names another log group",
            policy: |_| {
                key_policy(
                    "logs.amazonaws.com",
                    Some(json!({"ArnEquals": {"kms:EncryptionContext:aws:logs:arn": log_group_arn(NAMED_GROUP)}})),
                )
            },
            covers: false,
        },
        Row {
            case: "the policy names every log group of the account by wildcard",
            policy: |_| {
                key_policy(
                    "logs.amazonaws.com",
                    Some(json!({"ArnLike": {"kms:EncryptionContext:aws:logs:arn": log_group_arn("*")}})),
                )
            },
            covers: true,
        },
        Row {
            case: "the policy grants the Regional service principal unconditionally",
            policy: |_| key_policy(&format!("logs.{REGION}.amazonaws.com"), None),
            covers: true,
        },
        Row {
            case: "the policy grants CloudWatch Logs nothing",
            policy: |_| account_only_policy(),
            covers: false,
        },
        Row {
            case: "the policy denies CloudWatch Logs explicitly",
            policy: |_| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        account_statement(),
                        {
                            "Effect": "Deny",
                            "Principal": {"Service": "logs.amazonaws.com"},
                            "Action": "kms:*",
                            "Resource": "*",
                        },
                    ],
                })
                .to_string()
            },
            covers: false,
        },
    ]
}

/// `CreateLogGroup` creates the group exactly when the key policy covers it, and the refusal
/// states the group's own ARN and leaves no group behind.
#[tokio::test]
async fn create_log_group_is_accepted_only_when_the_key_policy_covers_the_group() {
    for row in rows() {
        let emulator = Emulator::new();
        let policy = (row.policy)(OTHER_GROUP);
        let key = emulator.created_key(row.case, &policy).await;

        let (status, parsed) = emulator
            .logs(
                "CreateLogGroup",
                &json!({"logGroupName": OTHER_GROUP, "kmsKeyId": key}),
            )
            .await;

        if row.covers {
            assert_eq!(
                status, 200,
                "case {:?}: the key policy covers the group, got {status} {parsed}",
                row.case
            );
            assert_eq!(
                emulator.reported_key(row.case, OTHER_GROUP).await,
                Some(key.clone()),
                "case {:?}: the group reports the key it was created with",
                row.case
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {:?}: expected the create to be refused, got {status} {parsed}",
            row.case
        );
        assert_eq!(
            crate::harness::error_code(&parsed),
            ACCESS_DENIED,
            "case {:?}: expected {ACCESS_DENIED}, got {parsed}",
            row.case
        );
        let expected = format!(
            "The specified KMS key does not exist or is not allowed to be used with Arn '{}'",
            log_group_arn(OTHER_GROUP)
        );
        assert_eq!(
            parsed.get("message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {:?}: expected {expected:?}, got {parsed}",
            row.case
        );
        assert!(
            !emulator.holds_group(row.case, OTHER_GROUP).await,
            "case {:?}: a refused create leaves no log group behind",
            row.case
        );
    }
}

/// `AssociateKmsKey` records the key exactly when the key policy covers the group, and a
/// refused association leaves the group's own `kmsKeyId` as it was.
#[tokio::test]
async fn associate_kms_key_is_accepted_only_when_the_key_policy_covers_the_group() {
    for row in rows() {
        let emulator = Emulator::new();
        let policy = (row.policy)(OTHER_GROUP);
        let key = emulator.created_key(row.case, &policy).await;
        emulator
            .logs_ok(
                row.case,
                "CreateLogGroup",
                &json!({"logGroupName": OTHER_GROUP}),
            )
            .await;

        let (status, parsed) = emulator
            .logs(
                "AssociateKmsKey",
                &json!({"logGroupName": OTHER_GROUP, "kmsKeyId": key}),
            )
            .await;

        if row.covers {
            assert_eq!(
                status, 200,
                "case {:?}: the key policy covers the group, got {status} {parsed}",
                row.case
            );
            assert_eq!(
                emulator.reported_key(row.case, OTHER_GROUP).await,
                Some(key.clone()),
                "case {:?}: the group reports the associated key",
                row.case
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {:?}: expected the association to be refused, got {status} {parsed}",
            row.case
        );
        assert_eq!(
            crate::harness::error_code(&parsed),
            ACCESS_DENIED,
            "case {:?}: expected {ACCESS_DENIED}, got {parsed}",
            row.case
        );
        assert_eq!(
            emulator.reported_key(row.case, OTHER_GROUP).await,
            None,
            "case {:?}: a refused association records no key",
            row.case
        );
    }
}

/// The group the policy DOES name is served by the same key: the gate refuses the resource
/// the grant does not cover and nothing else.
#[tokio::test]
async fn the_group_the_policy_names_is_created_with_the_same_key() {
    let emulator = Emulator::new();
    let case = "a key scoped to one log group";
    let policy = key_policy(
        "logs.amazonaws.com",
        Some(json!({
            "ArnEquals": {"kms:EncryptionContext:aws:logs:arn": log_group_arn(NAMED_GROUP)}
        })),
    );
    let key = emulator.created_key(case, &policy).await;

    emulator
        .logs_ok(
            case,
            "CreateLogGroup",
            &json!({"logGroupName": NAMED_GROUP, "kmsKeyId": key}),
        )
        .await;
    assert_eq!(
        emulator.reported_key(case, NAMED_GROUP).await,
        Some(key.clone()),
        "case {case:?}: the named group holds the key"
    );

    // The group the policy names serves its events while encrypted under that key.
    emulator
        .logs_ok(
            case,
            "CreateLogStream",
            &json!({"logGroupName": NAMED_GROUP, "logStreamName": "stream"}),
        )
        .await;
    let timestamp = crate::harness::now_ms() - 60 * 1_000;
    emulator
        .logs_ok(
            case,
            "PutLogEvents",
            &json!({
                "logGroupName": NAMED_GROUP,
                "logStreamName": "stream",
                "logEvents": [{"timestamp": timestamp, "message": "encrypted under the key"}],
            }),
        )
        .await;
    let page = emulator
        .logs_ok(
            case,
            "GetLogEvents",
            &json!({"logGroupName": NAMED_GROUP, "logStreamName": "stream"}),
        )
        .await;
    assert_eq!(
        page["events"][0]["message"].as_str(),
        Some("encrypted under the key"),
        "case {case:?}: the named group serves its events, got {page}"
    );

    // The other group, which the same policy does not name, is refused.
    let (status, parsed) = emulator
        .logs(
            "CreateLogGroup",
            &json!({"logGroupName": OTHER_GROUP, "kmsKeyId": key}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the group the policy does not name is refused, got {status} {parsed}"
    );
    assert!(
        !emulator.holds_group(case, OTHER_GROUP).await,
        "case {case:?}: the refused group was not created"
    );
}
