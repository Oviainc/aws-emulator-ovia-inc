// SPDX-License-Identifier: CC0-1.0

//! `SetLogDeliveryConfiguration` and `GetLogDeliveryConfiguration` over the wire,
//! driven through the router exactly as an SDK drives them (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a user pool holds ONE log delivery configuration, the
//! `LogConfigurations` collection a `SetLogDeliveryConfiguration` call last stored,
//! and every reader renders that stored collection — empty for a pool nothing has
//! configured logging for, because
//! `LogDeliveryConfigurationType.LogConfigurations` is a modelled required member.
//! A set REPLACES the collection and is accepted only when every entry is one the
//! pool can deliver: `LogConfigurationType` pairs `userNotification` with
//! `CloudWatchLogsConfiguration`, and `userAuthEvents` with one of the three
//! destinations and the Plus feature plan, each entry naming exactly one
//! destination and each source configured at most once. The `LogLevel` is the
//! caller's choice among the modelled values. A request that fails any of that is
//! refused and leaves the stored collection exactly as it was.
//!
//! Each table row names the case it asserts and every assertion reports the actual
//! value.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A CloudWatch Logs log group a user pool exports its logs to.
pub const LOG_GROUP_ARN: &str = "arn:aws:logs:us-east-1:000000000000:log-group:/aws/cognito/pool";

/// A second log group, so a case can change the stored destination.
const OTHER_LOG_GROUP_ARN: &str =
    "arn:aws:logs:us-east-1:000000000000:log-group:/aws/cognito/other";

/// An Amazon Data Firehose stream a user pool exports its activity logs to.
const STREAM_ARN: &str = "arn:aws:firehose:us-east-1:000000000000:deliverystream/cognito";

/// An Amazon S3 bucket a user pool exports its activity logs to. `S3ArnType`'s
/// pattern carries no account id.
const BUCKET_ARN: &str = "arn:aws:s3:::cognito-user-pool-logs";

/// One emulator with just this service mounted.
pub fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
pub fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` of an error body, under either casing the protocol admits.
pub fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// A pool in `tier` (or in the default plan when `tier` is `None`), and its
/// minted id.
pub async fn pool(router: &MultiServiceRouter, case: &str, tier: Option<&str>) -> String {
    let mut request = json!({ "PoolName": "log-delivery-pool" });
    if let (Some(target), Some(tier)) = (request.as_object_mut(), tier) {
        target.insert("UserPoolTier".to_string(), json!(tier));
    }
    let created = ok(router, "CreateUserPool", &request).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted pool id: {created}"))
        .to_string()
}

/// The `LogDeliveryConfiguration` `GetLogDeliveryConfiguration` reports for
/// `user_pool_id`.
pub async fn read_back(router: &MultiServiceRouter, case: &str, user_pool_id: &str) -> Value {
    let payload = ok(
        router,
        "GetLogDeliveryConfiguration",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    let configuration = payload.get("LogDeliveryConfiguration").cloned();
    configuration
        .unwrap_or_else(|| panic!("case {case:?}: the read must carry a configuration: {payload}"))
}

/// A `userNotification` entry delivering to the CloudWatch Logs group `arn`.
pub fn user_notification(arn: &str) -> Value {
    json!({
        "LogLevel": "ERROR",
        "EventSource": "userNotification",
        "CloudWatchLogsConfiguration": { "LogGroupArn": arn },
    })
}

/// A `userAuthEvents` entry delivering to `destination`.
fn user_auth_events(destination: Value) -> Value {
    let mut entry = json!({ "LogLevel": "INFO", "EventSource": "userAuthEvents" });
    if let (Some(target), Some(members)) = (entry.as_object_mut(), destination.as_object()) {
        for (name, value) in members {
            target.insert(name.clone(), value.clone());
        }
    }
    entry
}

/// What a request-validation refusal says: the fixed sentence, the condition and
/// the event sources that failed it, as recorded for a collection Amazon Cognito
/// will not accept.
fn request_validation_message(clause: &str, event_sources: &[&str]) -> String {
    format!(
        "Request validation Failed. Following event sources in request {clause}: [{}].",
        event_sources.join(", ")
    )
}

#[tokio::test]
async fn a_pool_with_no_logging_configured_reports_an_empty_collection() {
    let router = router();

    // Every feature plan, because an unconfigured pool reports the same empty
    // collection whatever its plan: `UserPoolTierType` is `LITE | ESSENTIALS |
    // PLUS` (`smithy_query(query="shape",shape="UserPoolTierType")`), plus the
    // pool that names no plan and carries the documented default.
    for tier in [Some("LITE"), Some("ESSENTIALS"), Some("PLUS"), None] {
        let case = format!("a fresh pool in {tier:?}");
        let user_pool_id = pool(&router, &case, tier).await;
        let configuration = read_back(&router, &case, &user_pool_id).await;
        assert_eq!(
            configuration,
            json!({ "UserPoolId": user_pool_id, "LogConfigurations": [] }),
            "case {case:?}: an unconfigured pool reports itself with an empty \
             collection, got {configuration}"
        );

        // The read is a read: a second one answers the same thing.
        let again = read_back(&router, &case, &user_pool_id).await;
        assert_eq!(
            again, configuration,
            "case {case:?}: reading twice must answer the same configuration, got {again}"
        );
    }
}

#[tokio::test]
async fn a_set_collection_is_what_every_reader_renders() {
    let router = router();

    // The rows differ along what the rule generalises over: which source is
    // configured, which LEVEL it is configured with, which destination it is
    // exported to, how many entries the collection carries, and the empty
    // collection.
    let rows: Vec<(&str, Value)> = vec![
        ("the empty collection", json!([])),
        (
            "one userNotification entry",
            json!([user_notification(LOG_GROUP_ARN)]),
        ),
        (
            "userNotification against another log group",
            json!([user_notification(OTHER_LOG_GROUP_ARN)]),
        ),
        // Each source takes either modelled `LogLevel`
        // (`smithy_query(query="shape",shape="LogLevel")`: ERROR | INFO): the
        // level a source's documentation recommends is a recommendation, not a
        // restriction, and the pool reports back the level the request named.
        (
            "userNotification at the INFO level",
            json!([{
                "LogLevel": "INFO",
                "EventSource": "userNotification",
                "CloudWatchLogsConfiguration": { "LogGroupArn": LOG_GROUP_ARN },
            }]),
        ),
        (
            "userAuthEvents at the ERROR level",
            json!([{
                "LogLevel": "ERROR",
                "EventSource": "userAuthEvents",
                "S3Configuration": { "BucketArn": BUCKET_ARN },
            }]),
        ),
        (
            "userAuthEvents to CloudWatch Logs",
            json!([user_auth_events(
                json!({ "CloudWatchLogsConfiguration": { "LogGroupArn": LOG_GROUP_ARN } })
            )]),
        ),
        (
            "userAuthEvents to Firehose",
            json!([user_auth_events(
                json!({ "FirehoseConfiguration": { "StreamArn": STREAM_ARN } })
            )]),
        ),
        (
            "userAuthEvents to Amazon S3",
            json!([user_auth_events(
                json!({ "S3Configuration": { "BucketArn": BUCKET_ARN } })
            )]),
        ),
        (
            "both event sources at once",
            json!([
                user_notification(LOG_GROUP_ARN),
                user_auth_events(json!({ "S3Configuration": { "BucketArn": BUCKET_ARN } })),
            ]),
        ),
    ];

    // A Plus pool, so every row's source is included in the plan.
    let user_pool_id = pool(&router, "a Plus pool", Some("PLUS")).await;

    for (case, log_configurations) in rows {
        let answered = ok(
            &router,
            "SetLogDeliveryConfiguration",
            &json!({
                "UserPoolId": user_pool_id,
                "LogConfigurations": log_configurations,
            }),
        )
        .await;
        let expected = json!({
            "UserPoolId": user_pool_id,
            "LogConfigurations": log_configurations,
        });
        assert_eq!(
            answered.get("LogDeliveryConfiguration"),
            Some(&expected),
            "case {case:?}: the set must answer the configuration it stored, got {answered}"
        );

        // A SET replaces the collection, so the reader renders exactly this row —
        // including the empty collection, which stays an empty collection rather
        // than becoming an absent member.
        let configuration = read_back(&router, case, &user_pool_id).await;
        assert_eq!(
            configuration, expected,
            "case {case:?}: the read must render the stored collection, got {configuration}"
        );
    }
}

#[tokio::test]
async fn a_refused_set_leaves_the_stored_collection_unchanged() {
    let router = router();

    // Every row is a collection the pool cannot deliver, with the condition the
    // refusal names. `LogConfigurationType.LogLevel`: `userNotification` is
    // exported to `CloudWatchLogsConfiguration`; `userAuthEvents` to one of the
    // three destinations.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a userNotification entry with no destination",
            json!([{ "LogLevel": "ERROR", "EventSource": "userNotification" }]),
            request_validation_message("have no destination", &["userNotification"]),
        ),
        (
            "a userAuthEvents entry with no destination",
            json!([{ "LogLevel": "INFO", "EventSource": "userAuthEvents" }]),
            request_validation_message("have no destination", &["userAuthEvents"]),
        ),
        (
            "both entries without a destination",
            json!([
                { "LogLevel": "ERROR", "EventSource": "userNotification" },
                { "LogLevel": "INFO", "EventSource": "userAuthEvents" },
            ]),
            request_validation_message(
                "have no destination",
                &["userNotification", "userAuthEvents"],
            ),
        ),
        (
            "userNotification exported to Amazon S3",
            json!([{
                "LogLevel": "ERROR",
                "EventSource": "userNotification",
                "S3Configuration": { "BucketArn": BUCKET_ARN },
            }]),
            request_validation_message("have an unsupported destination", &["userNotification"]),
        ),
        (
            "userNotification exported to Firehose",
            json!([{
                "LogLevel": "ERROR",
                "EventSource": "userNotification",
                "FirehoseConfiguration": { "StreamArn": STREAM_ARN },
            }]),
            request_validation_message("have an unsupported destination", &["userNotification"]),
        ),
        (
            "userAuthEvents exported to two destinations at once",
            json!([{
                "LogLevel": "INFO",
                "EventSource": "userAuthEvents",
                "S3Configuration": { "BucketArn": BUCKET_ARN },
                "FirehoseConfiguration": { "StreamArn": STREAM_ARN },
            }]),
            request_validation_message("have more than one destination", &["userAuthEvents"]),
        ),
        (
            "one event source configured twice",
            json!([
                user_notification(LOG_GROUP_ARN),
                user_notification(OTHER_LOG_GROUP_ARN),
            ]),
            request_validation_message("are configured more than once", &["userNotification"]),
        ),
    ];

    // The collection every refusal has to leave alone: a Plus pool with one
    // stored entry per source.
    let stored = json!([
        user_notification(LOG_GROUP_ARN),
        user_auth_events(json!({ "FirehoseConfiguration": { "StreamArn": STREAM_ARN } })),
    ]);
    let user_pool_id = pool(&router, "a Plus pool", Some("PLUS")).await;
    ok(
        &router,
        "SetLogDeliveryConfiguration",
        &json!({ "UserPoolId": user_pool_id, "LogConfigurations": stored }),
    )
    .await;
    let before = read_back(&router, "the stored collection", &user_pool_id).await;

    for (case, log_configurations, expected_message) in rows {
        let (status, payload) = call(
            &router,
            "SetLogDeliveryConfiguration",
            &json!({
                "UserPoolId": user_pool_id,
                "LogConfigurations": log_configurations,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );

        // The refusal is judged before anything is written, so the pool still
        // reports the collection it held.
        let after = read_back(&router, case, &user_pool_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused set must leave the stored collection unchanged, \
             got {after}"
        );
    }
}

#[tokio::test]
async fn activity_logs_need_the_plan_that_includes_them() {
    let router = router();

    // `LogConfigurationType.EventSource`: `userAuthEvents` carries "info-level
    // logs about threat-protection user activity in user pools with the Plus
    // feature plan", so every other plan — named or defaulted — excludes it,
    // while `userNotification` is included in all of them.
    let plans = [
        ("LITE", Some("LITE"), "LITE"),
        ("ESSENTIALS", Some("ESSENTIALS"), "ESSENTIALS"),
        ("PLUS", Some("PLUS"), "PLUS"),
        ("the default plan", None, "ESSENTIALS"),
    ];

    for (plan_case, requested, effective) in plans {
        let user_pool_id = pool(&router, plan_case, requested).await;

        // Notification logs are included in every plan.
        let case = format!("userNotification on a pool in {plan_case}");
        let answered = ok(
            &router,
            "SetLogDeliveryConfiguration",
            &json!({
                "UserPoolId": user_pool_id,
                "LogConfigurations": [user_notification(LOG_GROUP_ARN)],
            }),
        )
        .await;
        assert_eq!(
            answered["LogDeliveryConfiguration"]["LogConfigurations"],
            json!([user_notification(LOG_GROUP_ARN)]),
            "case {case:?}: notification logs are included in every plan, got {answered}"
        );
        let before = read_back(&router, &case, &user_pool_id).await;

        let case = format!("userAuthEvents on a pool in {plan_case}");
        let (status, payload) = call(
            &router,
            "SetLogDeliveryConfiguration",
            &json!({
                "UserPoolId": user_pool_id,
                "LogConfigurations": [user_auth_events(
                    json!({ "CloudWatchLogsConfiguration": { "LogGroupArn": LOG_GROUP_ARN } })
                )],
            }),
        )
        .await;

        if effective == "PLUS" {
            assert_eq!(
                status, 200,
                "case {case:?}: the Plus plan includes activity logs, got {status} with {payload}"
            );
            // The accepted path is a write like any other: the pool reports the
            // activity-logs entry that replaced the notification one.
            let after = read_back(&router, &case, &user_pool_id).await;
            assert_eq!(
                after["LogConfigurations"],
                json!([user_auth_events(
                    json!({ "CloudWatchLogsConfiguration": { "LogGroupArn": LOG_GROUP_ARN } })
                )]),
                "case {case:?}: the accepted set must be what the pool reports, got {after}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&payload)),
            (400, "FeatureUnavailableInTierException"),
            "case {case:?}: expected a 400 FeatureUnavailableInTierException, got {status} \
             with {payload}"
        );
        let message = error_message(&payload);
        let expected = format!(
            "The following feature is not available for the {effective} pricing tier \
             configured: Log Streaming"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // A plan refusal writes nothing either.
        let after = read_back(&router, &case, &user_pool_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused set must leave the stored collection unchanged, got {after}"
        );
    }
}

#[tokio::test]
async fn a_pool_that_does_not_exist_is_a_400_on_both_operations() {
    let router = router();

    // Ids satisfying `UserPoolIdType`'s pattern `^[\w-]+_[0-9a-zA-Z]+$` that no
    // create ever minted, so the lookup is what fails rather than the model's
    // constraint.
    for absent in ["us-east-1_absentpool", "eu-west-2_0000000000"] {
        for operation in [
            "GetLogDeliveryConfiguration",
            "SetLogDeliveryConfiguration",
        ] {
            let case = format!("{operation} against {absent}");
            let mut request = json!({ "UserPoolId": absent });
            if operation == "SetLogDeliveryConfiguration" {
                request["LogConfigurations"] = json!([user_notification(LOG_GROUP_ARN)]);
            }
            let (status, payload) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected a 400 ResourceNotFoundException, got {status} \
                 with {payload}"
            );
            let message = error_message(&payload);
            // Each operation words its own pool lookup: the setter answers
            // Amazon Cognito's sentence for it, which joins `UserPool with ID: `
            // to `does not exist.` with no separator, and the reader answers its
            // own.
            let expected = if operation == "SetLogDeliveryConfiguration" {
                format!("UserPool with ID: {absent}does not exist.")
            } else {
                format!("User pool {absent} does not exist.")
            };
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn each_pool_holds_its_own_logging_configuration() {
    let router = router();

    // Two pools, so a set against one is not a set against the other.
    let first = pool(&router, "the first pool", Some("PLUS")).await;
    let second = pool(&router, "the second pool", Some("PLUS")).await;

    ok(
        &router,
        "SetLogDeliveryConfiguration",
        &json!({
            "UserPoolId": first,
            "LogConfigurations": [user_notification(LOG_GROUP_ARN)],
        }),
    )
    .await;

    let configuration = read_back(&router, "the second pool", &second).await;
    assert_eq!(
        configuration,
        json!({ "UserPoolId": second, "LogConfigurations": [] }),
        "case {:?}: a set against another pool must not configure this one, got {configuration}",
        "the second pool after the first is configured"
    );

    let configuration = read_back(&router, "the first pool", &first).await;
    assert_eq!(
        configuration,
        json!({
            "UserPoolId": first,
            "LogConfigurations": [user_notification(LOG_GROUP_ARN)],
        }),
        "case {:?}: the configured pool reports its own collection, got {configuration}",
        "the first pool"
    );
}
