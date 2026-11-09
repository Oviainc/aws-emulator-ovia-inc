// SPDX-License-Identifier: CC0-1.0

//! Where a user pool's log delivery destinations are resolved: in the services that OWN
//! them.
//!
//! The rule under test: every destination a `LogConfigurations` entry names is the ARN of
//! a resource of another service — a CloudWatch Logs log group, an Amazon Data Firehose
//! stream, an Amazon S3 bucket — and `SetLogDeliveryConfiguration` accepts the collection
//! only once the service that owns each destination reports holding what the ARN names.
//! A collection naming one that does not resolve is refused with HTTP 400
//! `InvalidParameterException` and the fixed sentence `Invalid destination in the input.`,
//! and the pool keeps the collection it already had. The gate is the owning service's
//! state AT THE MOMENT OF THE CALL, not a list written here: a destination that resolved
//! once stops resolving when the owning service no longer holds it.
//!
//! Holding the resource is necessary, not sufficient: a destination resolves only when it
//! also satisfies what its own member documents of the resource. `LogGroupArn` is the one
//! that documents any — "The log group must not be encrypted with Key Management Service
//! and must be in the same Amazon Web Services account as your user pool" — so a log group
//! of another account, and one CloudWatch Logs reports a KMS key for, are refused the same
//! way a group nothing holds is.
//!
//! The rows span what the rule generalises over rather than one recorded request: WHICH
//! destination member carries the ARN, WHICH event source is configured, whether the
//! resource was never created or was created and then deleted, how many entries the
//! collection carries, a member that names no resource at all, WHICH account the ARN
//! names, and whether the owning service reports the resource as encrypted.
//!
//! Every service is mounted on ONE shared engine, exactly as the assembled emulator mounts
//! them, so each destination is created through its own service's wire and resolved through
//! the same state that service serves its own reads from.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use crate::log_delivery_configuration::{
    call, error_code, error_message, ok, pool, read_back, user_notification,
};

/// The Region every request below is signed for, and the one each destination ARN names.
const REGION: &str = "us-east-1";

/// What Amazon Cognito answers for a destination it cannot resolve: fixed text, with
/// neither the pool nor the ARN interpolated into it.
const INVALID_DESTINATION: &str = "Invalid destination in the input.";

/// A log group CloudWatch Logs really holds, and one nothing ever created.
const HELD_LOG_GROUP: &str = "/aws/cognito/held-pool";
const ABSENT_LOG_GROUP: &str = "/aws/cognito/never-created";

/// A Firehose stream Amazon Data Firehose really holds, and one nothing ever created.
const HELD_STREAM: &str = "cognito-held-stream";
const ABSENT_STREAM: &str = "cognito-never-created-stream";

/// A bucket Amazon S3 really holds, and one nothing ever created.
const HELD_BUCKET: &str = "cognito-held-logs";
const ABSENT_BUCKET: &str = "cognito-never-created-logs";

/// The bucket the held Firehose stream delivers to: a stream is created against an Amazon
/// S3 destination of its own, which Firehose resolves the same way.
const STREAM_SINK_BUCKET: &str = "cognito-firehose-sink";

/// The role the held Firehose stream assumes. No IAM service is mounted here, so this
/// names the role form Firehose documents without asserting anything about IAM.
const STREAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/firehose-delivery";

/// One emulator with the cognito-idp service and the three services that own a log
/// delivery destination, all on one shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router.set_request_authentication(false);
    router
}

/// Call one awsJson operation of `service`, whose `X-Amz-Target` prefix is `target_prefix`,
/// and return its status and response body.
async fn json_rpc(
    router: &MultiServiceRouter,
    service: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/\
                 aws4_request, SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Create the CloudWatch Logs log group `name`, through CloudWatch Logs' own wire.
async fn given_log_group(router: &MultiServiceRouter, name: &str) {
    let (status, payload) = json_rpc(
        router,
        "logs",
        "Logs_20140328",
        "CreateLogGroup",
        &json!({ "logGroupName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLogGroup({name}) must succeed, got {status} with {payload}"
    );
}

/// Delete the CloudWatch Logs log group `name`, so nothing holds it any more.
async fn delete_log_group(router: &MultiServiceRouter, name: &str) {
    let (status, payload) = json_rpc(
        router,
        "logs",
        "Logs_20140328",
        "DeleteLogGroup",
        &json!({ "logGroupName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteLogGroup({name}) must succeed, got {status} with {payload}"
    );
}

/// Create the Amazon S3 bucket `name`, through Amazon S3's own (REST) wire.
///
/// Path style, and with no `CreateBucketConfiguration`: an absent `LocationConstraint`
/// places the bucket in [`REGION`], which is the Region every request here is signed for.
async fn given_bucket(router: &MultiServiceRouter, name: &str) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{name}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/\
                 aws4_request, SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        )
        .body(Bytes::new())
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "CreateBucket({name}) must succeed, got {} with {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
}

/// Create the Amazon Data Firehose stream `name`, delivering to [`STREAM_SINK_BUCKET`].
async fn given_delivery_stream(router: &MultiServiceRouter, name: &str) {
    let (status, payload) = json_rpc(
        router,
        "firehose",
        "Firehose_20150804",
        "CreateDeliveryStream",
        &json!({
            "DeliveryStreamName": name,
            "ExtendedS3DestinationConfiguration": {
                "RoleARN": STREAM_ROLE_ARN,
                "BucketARN": format!("arn:aws:s3:::{STREAM_SINK_BUCKET}"),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateDeliveryStream({name}) must succeed, got {status} with {payload}"
    );
}

/// The `CloudWatchLogsConfiguration.LogGroupArn` of the log group `name`, in the account
/// this emulator serves — which is the account every pool created here is in.
fn log_group_arn(name: &str) -> String {
    log_group_arn_in_account(account(), name)
}

/// The `CloudWatchLogsConfiguration.LogGroupArn` of the log group `name` in `account`.
fn log_group_arn_in_account(account: &str, name: &str) -> String {
    format!("arn:aws:logs:{REGION}:{account}:log-group:{name}")
}

/// A KMS key ARN of the form CloudWatch Logs' `kmsKeyId` documents ("The Amazon Resource
/// Name (ARN) of the KMS key to use when encrypting log data"). No KMS service is mounted
/// here, so this asserts nothing about KMS: what the rows below turn on is only that
/// CloudWatch Logs REPORTS the group as encrypted.
fn kms_key_arn() -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:key/9cf5a1b2-0e4d-4c7a-9b3f-1d2e3f4a5b6c",
        account()
    )
}

/// Have CloudWatch Logs encrypt the log group `name` with `kms_key_id`, through its own
/// wire, so its own read reports the group's `kmsKeyId`.
async fn associate_kms_key(router: &MultiServiceRouter, name: &str, kms_key_id: &str) {
    let (status, payload) = json_rpc(
        router,
        "logs",
        "Logs_20140328",
        "AssociateKmsKey",
        &json!({ "logGroupName": name, "kmsKeyId": kms_key_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssociateKmsKey({name}) must succeed, got {status} with {payload}"
    );
}

/// Clear the KMS key from the log group `name`, so CloudWatch Logs stops reporting one.
async fn disassociate_kms_key(router: &MultiServiceRouter, name: &str) {
    let (status, payload) = json_rpc(
        router,
        "logs",
        "Logs_20140328",
        "DisassociateKmsKey",
        &json!({ "logGroupName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DisassociateKmsKey({name}) must succeed, got {status} with {payload}"
    );
}

/// Assert that setting `log_configurations` on `user_pool_id` is refused as a destination
/// the pool may not export to, and report `case` on every failure.
async fn refused_as_an_invalid_destination(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    log_configurations: &Value,
) {
    let (status, payload) = set(router, user_pool_id, log_configurations).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
    );
    let message = error_message(&payload);
    assert_eq!(
        message, INVALID_DESTINATION,
        "case {case:?}: expected {INVALID_DESTINATION:?}, got {message:?}"
    );
}

/// The `FirehoseConfiguration.StreamArn` of the delivery stream `name`.
fn stream_arn(name: &str) -> String {
    format!("arn:aws:firehose:{REGION}:{}:deliverystream/{name}", account())
}

/// The `S3Configuration.BucketArn` of the bucket `name`. `S3ArnType`'s pattern carries no
/// Region and no account.
fn bucket_arn(name: &str) -> String {
    format!("arn:aws:s3:::{name}")
}

/// The account every emulated ARN names.
fn account() -> &'static str {
    engine::EMULATOR_ACCOUNT_ID
}

/// A `userAuthEvents` entry delivering to `destination`, at the level that source's
/// documentation recommends.
fn user_auth_events(destination: Value) -> Value {
    let mut entry = json!({ "LogLevel": "INFO", "EventSource": "userAuthEvents" });
    if let (Some(target), Some(members)) = (entry.as_object_mut(), destination.as_object()) {
        for (name, value) in members {
            target.insert(name.clone(), value.clone());
        }
    }
    entry
}

/// Create every destination the accepted rows name, and the pool they are set on.
async fn given_destinations(router: &MultiServiceRouter) -> String {
    given_log_group(router, HELD_LOG_GROUP).await;
    given_bucket(router, HELD_BUCKET).await;
    given_bucket(router, STREAM_SINK_BUCKET).await;
    given_delivery_stream(router, HELD_STREAM).await;
    // A Plus pool, so the plan gate includes both event sources and what decides each row
    // is the destination.
    pool(router, "a Plus pool", Some("PLUS")).await
}

/// Set `log_configurations` on `user_pool_id` and return the status and body.
async fn set(router: &MultiServiceRouter, user_pool_id: &str, log_configurations: &Value) -> (u16, Value) {
    call(
        router,
        "SetLogDeliveryConfiguration",
        &json!({
            "UserPoolId": user_pool_id,
            "LogConfigurations": log_configurations,
        }),
    )
    .await
}

#[tokio::test]
async fn a_destination_is_accepted_only_where_the_service_that_owns_it_holds_it() {
    let router = router();
    let user_pool_id = given_destinations(&router).await;

    // (case, the collection, whether the owning services hold every destination in it)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("the empty collection, which names no destination", json!([]), true),
        (
            "userNotification to a log group CloudWatch Logs holds",
            json!([user_notification(&log_group_arn(HELD_LOG_GROUP))]),
            true,
        ),
        (
            "userNotification to a log group that was never created",
            json!([user_notification(&log_group_arn(ABSENT_LOG_GROUP))]),
            false,
        ),
        (
            "userAuthEvents to a log group CloudWatch Logs holds",
            json!([user_auth_events(json!({
                "CloudWatchLogsConfiguration": { "LogGroupArn": log_group_arn(HELD_LOG_GROUP) }
            }))]),
            true,
        ),
        (
            "userAuthEvents to a log group that was never created",
            json!([user_auth_events(json!({
                "CloudWatchLogsConfiguration": { "LogGroupArn": log_group_arn(ABSENT_LOG_GROUP) }
            }))]),
            false,
        ),
        (
            "userAuthEvents to a stream Amazon Data Firehose holds",
            json!([user_auth_events(json!({
                "FirehoseConfiguration": { "StreamArn": stream_arn(HELD_STREAM) }
            }))]),
            true,
        ),
        (
            "userAuthEvents to a stream that was never created",
            json!([user_auth_events(json!({
                "FirehoseConfiguration": { "StreamArn": stream_arn(ABSENT_STREAM) }
            }))]),
            false,
        ),
        (
            "userAuthEvents to a bucket Amazon S3 holds",
            json!([user_auth_events(json!({
                "S3Configuration": { "BucketArn": bucket_arn(HELD_BUCKET) }
            }))]),
            true,
        ),
        (
            "userAuthEvents to a bucket that was never created",
            json!([user_auth_events(json!({
                "S3Configuration": { "BucketArn": bucket_arn(ABSENT_BUCKET) }
            }))]),
            false,
        ),
        (
            "both event sources, every destination held",
            json!([
                user_notification(&log_group_arn(HELD_LOG_GROUP)),
                user_auth_events(json!({
                    "S3Configuration": { "BucketArn": bucket_arn(HELD_BUCKET) }
                })),
            ]),
            true,
        ),
        (
            "both event sources, one destination absent",
            json!([
                user_notification(&log_group_arn(HELD_LOG_GROUP)),
                user_auth_events(json!({
                    "FirehoseConfiguration": { "StreamArn": stream_arn(ABSENT_STREAM) }
                })),
            ]),
            false,
        ),
        (
            "a CloudWatch Logs destination naming no log group at all",
            json!([{
                "LogLevel": "ERROR",
                "EventSource": "userNotification",
                "CloudWatchLogsConfiguration": {},
            }]),
            false,
        ),
        (
            "a log group's ARN in the Firehose stream member",
            json!([user_auth_events(json!({
                "FirehoseConfiguration": { "StreamArn": log_group_arn(HELD_LOG_GROUP) }
            }))]),
            false,
        ),
    ];

    // What the pool reports, which only an ACCEPTED row changes.
    let mut stored = json!([]);
    for (case, log_configurations, resolvable) in rows {
        let (status, payload) = set(&router, &user_pool_id, &log_configurations).await;

        if resolvable {
            assert_eq!(
                status, 200,
                "case {case:?}: every destination here is held by its own service, \
                 got {status} with {payload}"
            );
            stored = log_configurations.clone();
        } else {
            assert_eq!(
                (status, error_code(&payload)),
                (400, "InvalidParameterException"),
                "case {case:?}: expected a 400 InvalidParameterException, got {status} \
                 with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, INVALID_DESTINATION,
                "case {case:?}: expected {INVALID_DESTINATION:?}, got {message:?}"
            );
        }

        // Accepted or refused, what the pool reports is the last collection it ACCEPTED:
        // the resolution runs before anything is written.
        let configuration = read_back(&router, case, &user_pool_id).await;
        assert_eq!(
            configuration,
            json!({ "UserPoolId": user_pool_id, "LogConfigurations": stored }),
            "case {case:?}: the pool must report the last accepted collection, \
             got {configuration}"
        );
    }
}

#[tokio::test]
async fn a_log_group_destination_must_be_in_the_pools_own_account() {
    let router = router();
    let user_pool_id = given_destinations(&router).await;

    // (case, the account the ARN names, whether the pool may export to it)
    //
    // The rule is `CloudWatchLogsConfiguration.LogGroupArn`: the log group "must be in the
    // same Amazon Web Services account as your user pool". Every pool created here is in
    // the one account this emulator serves, so an ARN naming any other account names a
    // group this pool may not export to — the log group of exactly that name is held, so
    // what each row turns on is the ACCOUNT and nothing else.
    let rows: [(&str, &str, bool); 4] = [
        ("the pool's own account", account(), true),
        ("another account", "123456789012", false),
        ("a third account", "111122223333", false),
        (
            "the pool's account with a digit changed",
            "000000000001",
            false,
        ),
    ];

    // What the pool reports, which only an ACCEPTED row changes.
    let mut stored = json!([]);
    for (case, arn_account, exportable) in rows {
        let log_configurations = json!([user_notification(&log_group_arn_in_account(
            arn_account,
            HELD_LOG_GROUP
        ))]);
        if exportable {
            let (status, payload) = set(&router, &user_pool_id, &log_configurations).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the pool's own account holds this log group, got {status} \
                 with {payload}"
            );
            stored = log_configurations.clone();
        } else {
            refused_as_an_invalid_destination(&router, case, &user_pool_id, &log_configurations)
                .await;
        }

        let configuration = read_back(&router, case, &user_pool_id).await;
        assert_eq!(
            configuration,
            json!({ "UserPoolId": user_pool_id, "LogConfigurations": stored }),
            "case {case:?}: the pool must report the last accepted collection, \
             got {configuration}"
        );
    }
}

#[tokio::test]
async fn a_log_group_the_owning_service_reports_as_encrypted_is_not_a_destination() {
    let router = router();
    let user_pool_id = given_destinations(&router).await;

    // A second log group, so the pool can be left pointing at one that stays acceptable.
    let rotated = "/aws/cognito/rotated-pool";
    given_log_group(&router, rotated).await;

    // The same group is acceptable while CloudWatch Logs reports no key for it.
    let encrypted_group = json!([user_notification(&log_group_arn(HELD_LOG_GROUP))]);
    ok(
        &router,
        "SetLogDeliveryConfiguration",
        &json!({ "UserPoolId": user_pool_id, "LogConfigurations": encrypted_group }),
    )
    .await;

    // Re-pointed at the second group, which is where the pool stays while the first is
    // refused.
    let plain_group = json!([user_notification(&log_group_arn(rotated))]);
    ok(
        &router,
        "SetLogDeliveryConfiguration",
        &json!({ "UserPoolId": user_pool_id, "LogConfigurations": plain_group }),
    )
    .await;

    associate_kms_key(&router, HELD_LOG_GROUP, &kms_key_arn()).await;

    // "The log group must not be encrypted with Key Management Service": the same request
    // that was accepted above now names a group the owning service reports a KMS key for.
    let case = "a log group CloudWatch Logs encrypts with a KMS key";
    refused_as_an_invalid_destination(&router, case, &user_pool_id, &encrypted_group).await;
    let configuration = read_back(&router, case, &user_pool_id).await;
    assert_eq!(
        configuration,
        json!({ "UserPoolId": user_pool_id, "LogConfigurations": plain_group }),
        "case {case:?}: the refused set must leave the pool on the group it was moved to, \
         got {configuration}"
    );

    // The gate is the owning service's state at the moment of the call, so clearing the key
    // makes the very same request acceptable again.
    disassociate_kms_key(&router, HELD_LOG_GROUP).await;
    let (status, payload) = set(&router, &user_pool_id, &encrypted_group).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the set to be accepted once the key is cleared, got {status} \
         with {payload}",
        "a log group whose KMS key was disassociated"
    );
    let configuration = read_back(&router, "after the key was cleared", &user_pool_id).await;
    assert_eq!(
        configuration,
        json!({ "UserPoolId": user_pool_id, "LogConfigurations": encrypted_group }),
        "case {:?}: the accepted set must be what the pool reports, got {configuration}",
        "a log group whose KMS key was disassociated"
    );
}

#[tokio::test]
async fn a_destination_the_owning_service_no_longer_holds_stops_resolving() {
    let router = router();
    let user_pool_id = given_destinations(&router).await;

    // A second log group, so the pool can be re-pointed at one that stays.
    let rotated = "/aws/cognito/rotated-pool";
    given_log_group(&router, rotated).await;

    // Accepted while CloudWatch Logs holds the group.
    let first = json!([user_notification(&log_group_arn(HELD_LOG_GROUP))]);
    ok(
        &router,
        "SetLogDeliveryConfiguration",
        &json!({ "UserPoolId": user_pool_id, "LogConfigurations": first }),
    )
    .await;

    // Re-pointed at the second group, which is where the pool stays.
    let second = json!([user_notification(&log_group_arn(rotated))]);
    ok(
        &router,
        "SetLogDeliveryConfiguration",
        &json!({ "UserPoolId": user_pool_id, "LogConfigurations": second }),
    )
    .await;

    delete_log_group(&router, HELD_LOG_GROUP).await;

    // The same ARN that was accepted above now names a group nothing holds, so the same
    // request is refused.
    let (status, payload) = set(&router, &user_pool_id, &first).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case {:?}: expected a 400 InvalidParameterException, got {status} with {payload}",
        "a destination deleted after it was accepted"
    );
    let message = error_message(&payload);
    assert_eq!(
        message, INVALID_DESTINATION,
        "case {:?}: expected {INVALID_DESTINATION:?}, got {message:?}",
        "a destination deleted after it was accepted"
    );

    let configuration = read_back(&router, "after the refused re-point", &user_pool_id).await;
    assert_eq!(
        configuration,
        json!({ "UserPoolId": user_pool_id, "LogConfigurations": second }),
        "case {:?}: the refused re-point must leave the pool on the group it was moved to, \
         got {configuration}",
        "a destination deleted after it was accepted"
    );
}
