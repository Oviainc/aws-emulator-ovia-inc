// SPDX-License-Identifier: CC0-1.0

//! What a constraint refusal of this service QUOTES: the value the caller submitted.
//!
//! The rule: a refusal of a modelled `@length` on a LIST member states the list the
//! request carried as the clause's value — `[<entry>, …]`, each entry of structure type
//! rendered over this service's own model types as
//! `<ShapeName>(<member>=<value>, …)` with every member the shape declares named, in
//! declaration order, and one the request left unset stated as `null` — rather than a
//! count of its entries. The member path and the bound stay the model's.
//!
//! The rows span what the rule generalises over: which operation and which list member
//! (two of them, with different element shapes), which bound was broken (a maximum and a
//! minimum), how many entries the list carried, which members of an entry were set, and
//! the empty list. The recorded case — three otherwise well-formed log configurations
//! against a maximum of two — is one row of it.
//!
//! The harness is the one `log_delivery_configuration.rs` builds for the same surface,
//! reached through the test crate root rather than copied.

use serde_json::{json, Value};

use super::log_delivery_configuration::{
    call, error_code, error_message, pool, router, user_notification, LOG_GROUP_ARN,
};

/// How a member the request left unset is stated inside a rendered structure.
const NULL: &str = "null";

/// A second log group, so a row can carry two entries that differ in their destination.
const OTHER_LOG_GROUP_ARN: &str =
    "arn:aws:logs:us-east-1:000000000000:log-group:/aws/cognito/other";

/// An Amazon Data Firehose stream a user pool exports its activity logs to.
const STREAM_ARN: &str = "arn:aws:firehose:us-east-1:000000000000:deliverystream/cognito";

/// An Amazon S3 bucket a user pool exports its activity logs to.
const BUCKET_ARN: &str = "arn:aws:s3:::cognito-user-pool-logs";

/// The envelope and clause a refusal of ONE broken constraint states.
fn one_validation_error(value: &str, path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// A list as a clause states it.
fn rendered_list(entries: &[String]) -> String {
    format!("[{}]", entries.join(", "))
}

/// A `CloudWatchLogsConfigurationType` as a clause states it. Its one declared member is
/// `LogGroupArn` (`smithy_query(query="shape",shape="CloudWatchLogsConfigurationType")`).
fn rendered_cloud_watch(log_group_arn: &str) -> String {
    format!("CloudWatchLogsConfigurationType(logGroupArn={log_group_arn})")
}

/// An `S3ConfigurationType` as a clause states it: one declared member, `BucketArn`.
fn rendered_s3(bucket_arn: &str) -> String {
    format!("S3ConfigurationType(bucketArn={bucket_arn})")
}

/// A `FirehoseConfigurationType` as a clause states it: one declared member, `StreamArn`.
fn rendered_firehose(stream_arn: &str) -> String {
    format!("FirehoseConfigurationType(streamArn={stream_arn})")
}

/// A `LogConfigurationType` as a clause states it.
///
/// `LogConfigurationType` declares `LogLevel`, `EventSource`,
/// `CloudWatchLogsConfiguration`, `S3Configuration` and `FirehoseConfiguration`, in that
/// order (`smithy_query(query="shape",shape="LogConfigurationType")`), so that is the
/// order a clause names them in; a destination the request left out is `null`.
fn rendered_log_configuration(
    log_level: &str,
    event_source: &str,
    cloud_watch: Option<String>,
    s3: Option<String>,
    firehose: Option<String>,
) -> String {
    format!(
        "LogConfigurationType(logLevel={log_level}, eventSource={event_source}, \
         cloudWatchLogsConfiguration={}, s3Configuration={}, firehoseConfiguration={})",
        cloud_watch.unwrap_or_else(|| NULL.to_string()),
        s3.unwrap_or_else(|| NULL.to_string()),
        firehose.unwrap_or_else(|| NULL.to_string()),
    )
}

/// A `SchemaAttributeType` as a clause states it.
///
/// It declares `Name`, `AttributeDataType`, `DeveloperOnlyAttribute`, `Mutable`,
/// `Required`, `NumberAttributeConstraints` and `StringAttributeConstraints`, in that
/// order (`smithy_query(query="shape",shape="SchemaAttributeType")`).
fn rendered_schema_attribute(name: &str, data_type: &str, mutable: Option<bool>) -> String {
    format!(
        "SchemaAttributeType(name={name}, attributeDataType={data_type}, \
         developerOnlyAttribute=null, mutable={}, required=null, \
         numberAttributeConstraints=null, stringAttributeConstraints=null)",
        mutable.map_or_else(|| NULL.to_string(), |set| set.to_string()),
    )
}

/// A `userAuthEvents` entry delivering to `destination`, the request half of a row.
fn user_auth_events(destination: Value) -> Value {
    let mut entry = json!({ "LogLevel": "INFO", "EventSource": "userAuthEvents" });
    if let (Some(target), Some(members)) = (entry.as_object_mut(), destination.as_object()) {
        for (name, value) in members {
            target.insert(name.clone(), value.clone());
        }
    }
    entry
}

/// One custom attribute, the request half of a row: `AttributeDataType` is one of the
/// modelled `AttributeDataType` values
/// (`smithy_query(query="shape",shape="AttributeDataType")`).
fn custom_attribute(name: &str, data_type: &str, mutable: Option<bool>) -> Value {
    let mut attribute = json!({ "Name": name, "AttributeDataType": data_type });
    if let (Some(target), Some(mutable)) = (attribute.as_object_mut(), mutable) {
        target.insert("Mutable".to_string(), json!(mutable));
    }
    attribute
}

/// One row: the request a list member's `@length` refuses, and the clause it earns.
struct Row {
    case: &'static str,
    operation: &'static str,
    /// The request body, bar `UserPoolId`, which the created pool supplies.
    body: Value,
    expected_message: String,
}

#[tokio::test]
async fn a_broken_list_bound_quotes_the_list_the_request_carried() {
    let router = router();
    // A Plus pool, so no row is refused for its feature plan before the bound is
    // reached.
    let user_pool_id = pool(&router, "a Plus pool", Some("PLUS")).await;

    // The clause's requirement halves, as the framework words every `@length`:
    // `LogConfigurationListType` is `@length` 0..2 and `CustomAttributesListType` is
    // 1..25 (`smithy_query(query="shape",shape="LogConfigurationListType")` and
    // `…shape="CustomAttributesListType"`).
    let at_most_two = "Member must have length less than or equal to 2";
    let at_most_twenty_five = "Member must have length less than or equal to 25";
    let at_least_one = "Member must have length greater than or equal to 1";

    let three_identical: Vec<Value> = (0..3).map(|_| user_notification(LOG_GROUP_ARN)).collect();
    let three_identical_rendered: Vec<String> = (0..3)
        .map(|_| {
            rendered_log_configuration(
                "ERROR",
                "userNotification",
                Some(rendered_cloud_watch(LOG_GROUP_ARN)),
                None,
                None,
            )
        })
        .collect();

    let twenty_six: Vec<Value> = (0..26)
        .map(|index| custom_attribute(&format!("attr{index}"), "String", Some(index % 2 == 0)))
        .collect();
    let twenty_six_rendered: Vec<String> = (0..26)
        .map(|index| {
            rendered_schema_attribute(&format!("attr{index}"), "String", Some(index % 2 == 0))
        })
        .collect();

    let rows = vec![
        Row {
            case: "three otherwise well-formed log configurations against a maximum of two",
            operation: "SetLogDeliveryConfiguration",
            body: json!({ "LogConfigurations": three_identical }),
            expected_message: one_validation_error(
                &rendered_list(&three_identical_rendered),
                "logConfigurations",
                at_most_two,
            ),
        },
        Row {
            case: "four log configurations, each naming a different destination",
            operation: "SetLogDeliveryConfiguration",
            body: json!({
                "LogConfigurations": [
                    user_notification(LOG_GROUP_ARN),
                    user_auth_events(json!({ "S3Configuration": { "BucketArn": BUCKET_ARN } })),
                    user_auth_events(
                        json!({ "FirehoseConfiguration": { "StreamArn": STREAM_ARN } })
                    ),
                    user_notification(OTHER_LOG_GROUP_ARN),
                ],
            }),
            expected_message: one_validation_error(
                &rendered_list(&[
                    rendered_log_configuration(
                        "ERROR",
                        "userNotification",
                        Some(rendered_cloud_watch(LOG_GROUP_ARN)),
                        None,
                        None,
                    ),
                    rendered_log_configuration(
                        "INFO",
                        "userAuthEvents",
                        None,
                        Some(rendered_s3(BUCKET_ARN)),
                        None,
                    ),
                    rendered_log_configuration(
                        "INFO",
                        "userAuthEvents",
                        None,
                        None,
                        Some(rendered_firehose(STREAM_ARN)),
                    ),
                    rendered_log_configuration(
                        "ERROR",
                        "userNotification",
                        Some(rendered_cloud_watch(OTHER_LOG_GROUP_ARN)),
                        None,
                        None,
                    ),
                ]),
                "logConfigurations",
                at_most_two,
            ),
        },
        Row {
            case: "three log configurations, one of them naming no destination at all",
            operation: "SetLogDeliveryConfiguration",
            body: json!({
                "LogConfigurations": [
                    user_notification(LOG_GROUP_ARN),
                    { "LogLevel": "ERROR", "EventSource": "userNotification" },
                    user_notification(OTHER_LOG_GROUP_ARN),
                ],
            }),
            expected_message: one_validation_error(
                &rendered_list(&[
                    rendered_log_configuration(
                        "ERROR",
                        "userNotification",
                        Some(rendered_cloud_watch(LOG_GROUP_ARN)),
                        None,
                        None,
                    ),
                    rendered_log_configuration("ERROR", "userNotification", None, None, None),
                    rendered_log_configuration(
                        "ERROR",
                        "userNotification",
                        Some(rendered_cloud_watch(OTHER_LOG_GROUP_ARN)),
                        None,
                        None,
                    ),
                ]),
                "logConfigurations",
                at_most_two,
            ),
        },
        Row {
            case: "twenty-six custom attributes against a maximum of twenty-five",
            operation: "AddCustomAttributes",
            body: json!({ "CustomAttributes": twenty_six }),
            expected_message: one_validation_error(
                &rendered_list(&twenty_six_rendered),
                "customAttributes",
                at_most_twenty_five,
            ),
        },
        Row {
            case: "no custom attributes at all against a minimum of one",
            operation: "AddCustomAttributes",
            body: json!({ "CustomAttributes": [] }),
            expected_message: one_validation_error(
                &rendered_list(&[]),
                "customAttributes",
                at_least_one,
            ),
        },
    ];

    for row in rows {
        let mut body = row.body.clone();
        body["UserPoolId"] = json!(user_pool_id);
        let (status, payload) = call(&router, row.operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {:?}: expected a 400 InvalidParameterException, got {status} with {payload}",
            row.case
        );
        let message = error_message(&payload);
        assert_eq!(
            message, row.expected_message,
            "case {:?}: expected {:?}, got {message:?}",
            row.case, row.expected_message
        );
    }
}

#[tokio::test]
async fn a_scalar_constraint_keeps_the_frameworks_own_clause() {
    let router = router();

    // The rule is about the half of a clause the framework has no value for: a
    // COLLECTION, which its walk can only summarise. A SCALAR member's clause already
    // states the bytes the caller sent, so it is left exactly as it was — asserted here
    // so the collection rendering stays an addition rather than a replacement.
    // `UserPoolIdType` is `@length` 1..55 with the pattern `^[\w-]+_[0-9a-zA-Z]+$`
    // (`smithy_query(query="shape",shape="UserPoolIdType")`).
    let over_long = format!("us-east-1_{}", "a".repeat(60));
    let (status, payload) = call(
        &router,
        "GetLogDeliveryConfiguration",
        &json!({ "UserPoolId": over_long }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case \"a user pool id past its length\": expected a 400 InvalidParameterException, \
         got {status} with {payload}"
    );
    let expected = one_validation_error(
        &over_long,
        "userPoolId",
        "Member must have length less than or equal to 55",
    );
    let message = error_message(&payload);
    assert_eq!(
        message, expected,
        "case \"a user pool id past its length\": expected {expected:?}, got {message:?}"
    );
}
