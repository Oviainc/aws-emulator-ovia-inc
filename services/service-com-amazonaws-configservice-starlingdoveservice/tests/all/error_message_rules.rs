// SPDX-License-Identifier: CC0-1.0

//! How Config WORDS a refusal, on the wire.
//!
//! An error message whose production rule is known is part of the contract, and these
//! cases pin the rules rather than the one recorded instance of each:
//!
//! 1. a precondition failure carries a FIXED message naming the precondition that
//!    failed, so two operations sharing one error class do not collapse into one
//!    sentence;
//! 2. a not-found refusal whose message is BUILT FROM THE REQUEST quotes the name the
//!    caller sent, and the same class words the same way in every operation that
//!    raises it — while a class whose message is fixed carries that one text wherever
//!    it is raised, including the ARN lookups;
//! 3. a not-found refusal follows the SHAPE of the member that named the resource: a
//!    list-shaped lookup reports the plural form, a single-name member the singular
//!    form quoting that name;
//! 4. a limit refusal states the service's own limit and nothing of the environment,
//!    so the same cause reads identically in every Region;
//! 5. a request-validation refusal advertises a `@pattern` without the model's
//!    whole-value anchors, names a member the input wraps by the name the model
//!    declares it with, and withholds the value of a member Config does not echo.
//!
//! Every case drives the emulator the way a client does, through the same
//! `MultiServiceRouter` the production router builds, reusing the `Emulator` harness
//! `config_setup_and_rule_loop` already provides.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// A delivery channel body naming a bucket; its value is never asserted on.
fn channel(name: &str) -> Value {
    json!({ "DeliveryChannel": { "name": name, "s3BucketName": "config-bucket" } })
}

/// Call `operation`, require a 400 with the error code `code`, and return the message
/// the service worded the refusal with.
async fn refusal(emulator: &Emulator, operation: &str, body: &Value, code: &str) -> String {
    let (status, parsed) = emulator.call(operation, body).await;
    assert_eq!(
        status, 400,
        "{operation}: expected HTTP 400, got {status} with body {parsed}"
    );
    let actual = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("<no __type in {parsed}>"));
    assert_eq!(
        actual, code,
        "{operation}: expected the error code {code}, got {actual:?} in {parsed}"
    );
    parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{operation}: expected a message in {parsed}"))
        .to_string()
}

/// A precondition failure names the precondition that failed, not the class: the two
/// operations that share `NoAvailableConfigurationRecorderException` word it
/// differently, and the delivery-channel precondition of
/// `StartConfigurationRecorder` has its own sentence too.
#[tokio::test]
async fn a_precondition_failure_names_the_precondition_that_failed() {
    // (case, setup, operation, body, error code, message).
    for (case, needs_recorder, operation, body, code, expected) in [
        (
            "a rule with no recorder",
            false,
            "PutConfigRule",
            json!({
                "ConfigRule": {
                    "ConfigRuleName": "rule-one",
                    "Source": { "Owner": "AWS", "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED" },
                }
            }),
            "NoAvailableConfigurationRecorderException",
            "You must create a configuration recorder before you can create or update a \
             Config rule.",
        ),
        (
            "a delivery channel with no recorder",
            false,
            "PutDeliveryChannel",
            channel("default"),
            "NoAvailableConfigurationRecorderException",
            "Configuration recorder is not available to put delivery channel.",
        ),
        (
            "starting a recorder with no delivery channel",
            true,
            "StartConfigurationRecorder",
            json!({ "ConfigurationRecorderName": "default" }),
            "NoAvailableDeliveryChannelException",
            "Delivery channel is not available to start configuration recorder.",
        ),
    ] {
        let emulator = Emulator::new();
        if needs_recorder {
            emulator
                .ok(
                    "PutConfigurationRecorder",
                    &recorder_request(json!({ "roleARN": ROLE_ARN })),
                )
                .await;
        }
        let message = refusal(&emulator, operation, &body, code).await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// A `NoSuchDeliveryChannelException` quotes the channel name the request carried,
/// wherever it is raised: the delete path, the describe path and the status path all
/// word it the same way.
#[tokio::test]
async fn an_absent_delivery_channel_is_quoted_by_the_name_the_request_sent() {
    // Names differing in form — the default Config assigns, a hyphenated name, one
    // with digits and underscores, and a single character (the `ChannelName` length
    // minimum, from `smithy_query(query="shape", shape="ChannelName")`: 1..256).
    for name in ["default", "sim-probe-channel", "channel_42", "c"] {
        let emulator = Emulator::new();
        let expected = format!("Cannot find delivery channel with specified name '{name}'.");

        let deleted = refusal(
            &emulator,
            "DeleteDeliveryChannel",
            &json!({ "DeliveryChannelName": name }),
            "NoSuchDeliveryChannelException",
        )
        .await;
        assert_eq!(
            deleted, expected,
            "case {name:?} (delete): expected {expected:?}, got {deleted:?}"
        );

        let described = refusal(
            &emulator,
            "DescribeDeliveryChannels",
            &json!({ "DeliveryChannelNames": [name] }),
            "NoSuchDeliveryChannelException",
        )
        .await;
        assert_eq!(
            described, expected,
            "case {name:?} (describe): expected {expected:?}, got {described:?}"
        );

        let status = refusal(
            &emulator,
            "DescribeDeliveryChannelStatus",
            &json!({ "DeliveryChannelNames": [name] }),
            "NoSuchDeliveryChannelException",
        )
        .await;
        assert_eq!(
            status, expected,
            "case {name:?} (describe status): expected {expected:?}, got {status:?}"
        );
    }
}

/// Deleting the account's only delivery channel while its recorder is running quotes
/// the channel name and says why the delete failed.
#[tokio::test]
async fn a_blocked_delivery_channel_delete_quotes_the_channel_it_refused() {
    for name in ["default", "sim-probe-channel", "c"] {
        let emulator = Emulator::new();
        emulator
            .ok(
                "PutConfigurationRecorder",
                &recorder_request(json!({ "roleARN": ROLE_ARN })),
            )
            .await;
        emulator.ok("PutDeliveryChannel", &channel(name)).await;
        emulator
            .ok(
                "StartConfigurationRecorder",
                &json!({ "ConfigurationRecorderName": "default" }),
            )
            .await;

        let message = refusal(
            &emulator,
            "DeleteDeliveryChannel",
            &json!({ "DeliveryChannelName": name }),
            "LastDeliveryChannelDeleteFailedException",
        )
        .await;
        let expected = format!(
            "Failed to delete last specified delivery channel with name '{name}', because \
             there is a running configuration recorder."
        );
        assert_eq!(
            message, expected,
            "case {name:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// `NoSuchConfigurationRecorderException` carries ONE fixed text: the message quotes
/// nothing from the request, so every operation that fails to find a recorder — by
/// name or by ARN — answers with it.
#[tokio::test]
async fn an_absent_configuration_recorder_carries_the_fixed_message() {
    const EXPECTED: &str = "The name of the configuration recorder you entered cannot be \
                            found. Verify the configuration recorder name, and try again.";

    // (case, operation, body) — every operation that looks a recorder up, including
    // the two that can look one up by ARN.
    for (case, operation, body) in [
        (
            "start by name",
            "StartConfigurationRecorder",
            json!({ "ConfigurationRecorderName": "absent-recorder" }),
        ),
        (
            "stop by name",
            "StopConfigurationRecorder",
            json!({ "ConfigurationRecorderName": "absent-recorder" }),
        ),
        (
            "delete by name",
            "DeleteConfigurationRecorder",
            json!({ "ConfigurationRecorderName": "absent-recorder" }),
        ),
        (
            "describe by name",
            "DescribeConfigurationRecorders",
            json!({ "ConfigurationRecorderNames": ["absent-recorder"] }),
        ),
        (
            "describe status by name",
            "DescribeConfigurationRecorderStatus",
            json!({ "ConfigurationRecorderNames": ["absent-recorder"] }),
        ),
        (
            "describe by ARN",
            "DescribeConfigurationRecorders",
            json!({ "Arn": "arn:aws:config:us-east-1:000000000000:configuration-recorder/x/y" }),
        ),
        (
            "describe status by ARN",
            "DescribeConfigurationRecorderStatus",
            json!({ "Arn": "arn:aws:config:us-east-1:000000000000:configuration-recorder/x/y" }),
        ),
    ] {
        let emulator = Emulator::new();
        let message = refusal(
            &emulator,
            operation,
            &body,
            "NoSuchConfigurationRecorderException",
        )
        .await;
        assert_eq!(
            message, EXPECTED,
            "case {case:?}: expected {EXPECTED:?}, got {message:?}"
        );
    }
}

/// `NoSuchConfigRuleException` follows the SHAPE of the member that named the rule: a
/// list-shaped lookup reports the plural form and quotes no name, a single-name member
/// the singular form quoting the name it carried.
#[tokio::test]
async fn an_absent_config_rule_is_worded_for_the_member_that_named_it() {
    const PLURAL: &str = "One or more ConfigRules provided in the request are invalid. \
                          Please check the configRule names.";

    for name in ["absent-rule", "probe_rule_two", "r"] {
        let emulator = Emulator::new();

        // `DescribeConfigRules$ConfigRuleNames` is a list (0..25 of `ConfigRuleName`).
        let listed = refusal(
            &emulator,
            "DescribeConfigRules",
            &json!({ "ConfigRuleNames": [name] }),
            "NoSuchConfigRuleException",
        )
        .await;
        assert_eq!(
            listed, PLURAL,
            "case {name:?} (list member): expected {PLURAL:?}, got {listed:?}"
        );

        // `DescribeComplianceByConfigRule$ConfigRuleNames` is the same list-shaped
        // member, so the same class words itself the same way.
        let compliance = refusal(
            &emulator,
            "DescribeComplianceByConfigRule",
            &json!({ "ConfigRuleNames": [name] }),
            "NoSuchConfigRuleException",
        )
        .await;
        assert_eq!(
            compliance, PLURAL,
            "case {name:?} (second list member): expected {PLURAL:?}, got {compliance:?}"
        );

        // `DeleteConfigRule$ConfigRuleName` names exactly one rule.
        let single = refusal(
            &emulator,
            "DeleteConfigRule",
            &json!({ "ConfigRuleName": name }),
            "NoSuchConfigRuleException",
        )
        .await;
        let expected = format!(
            "The ConfigRule '{name}' provided in the request is invalid. Please check the \
             configRule name."
        );
        assert_eq!(
            single, expected,
            "case {name:?} (single member): expected {expected:?}, got {single:?}"
        );
    }
}

/// The recorder limit refusal states the service's own limit and nothing of the
/// environment: the same cause reads identically in every Region.
#[tokio::test]
async fn the_recorder_limit_refusal_states_the_limit_and_not_the_region() {
    const EXPECTED: &str = "Failed to put configuration recorder  because you have reached \
                            the limit for the maximum number of customer managed \
                            configuration records: (1)";

    for region in ["us-east-1", "us-west-2", "eu-west-1", "ap-southeast-2"] {
        let emulator = Emulator::in_region(region);
        emulator
            .ok(
                "PutConfigurationRecorder",
                &recorder_request(json!({ "name": "default", "roleARN": ROLE_ARN })),
            )
            .await;

        let message = refusal(
            &emulator,
            "PutConfigurationRecorder",
            &recorder_request(json!({ "name": "second", "roleARN": ROLE_ARN })),
            "MaxNumberOfConfigurationRecordersExceededException",
        )
        .await;
        assert_eq!(
            message, EXPECTED,
            "case {region:?}: expected {EXPECTED:?}, got {message:?}"
        );
        assert!(
            !message.contains(region),
            "case {region:?}: the message must not name the Region, got {message:?}"
        );
    }
}

/// A request-validation refusal advertises the `@pattern` a value must satisfy without
/// the whole-value anchors the model spells it with, and echoes the value the caller
/// submitted.
#[tokio::test]
async fn a_validation_refusal_advertises_the_pattern_without_its_anchors() {
    // (case, operation, body, the clause the service states). The patterns come from
    // `smithy_query(query="shape", shape="AccountId")` (`^\d{12}$`) and
    // `smithy_query(query="shape", shape="QueryName")` (`^[a-zA-Z0-9-_]+$`).
    for (case, operation, body, expected) in [
        (
            "an account id that is not twelve digits",
            "DeletePendingAggregationRequest",
            json!({ "RequesterAccountId": "not-an-acct1", "RequesterAwsRegion": "us-east-1" }),
            "1 validation error detected: Value 'not-an-acct1' at 'requesterAccountId' \
             failed to satisfy constraint: Member must satisfy regular expression pattern: \
             \\d{12}",
        ),
        (
            "an account id that is too short",
            "DeletePendingAggregationRequest",
            json!({ "RequesterAccountId": "12345", "RequesterAwsRegion": "eu-west-1" }),
            "1 validation error detected: Value '12345' at 'requesterAccountId' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: \\d{12}",
        ),
        (
            "an aggregation request missing its Region",
            "DeletePendingAggregationRequest",
            json!({ "RequesterAccountId": "000000000000" }),
            "1 validation error detected: Value null at 'requesterAwsRegion' failed to \
             satisfy constraint: Member must not be null",
        ),
    ] {
        let emulator = Emulator::new();
        let message = refusal(&emulator, operation, &body, "ValidationException").await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// The value submitted at a member Config does not echo back is withheld: the clause
/// states the position and the constraint alone, wherever that member travels.
#[tokio::test]
async fn a_withheld_members_value_is_not_echoed_into_the_clause() {
    const EXPECTED: &str = "1 validation error detected: Value at 'servicePrincipal' failed \
                            to satisfy constraint: Member must satisfy regular expression \
                            pattern: [\\w+=,.@-]+";

    // Every operation whose input carries `ServicePrincipal`
    // (`smithy_query(query="shape", shape="ServicePrincipal")`: `@pattern`
    // `^[\w+=,.@-]+$`, `@length(1, 128)`), including the three the issue did not cite.
    for (case, operation, body) in [
        (
            "the third-party service-linked recorder",
            "PutThirdPartyServiceLinkedConfigurationRecorder",
            json!({
                "ServicePrincipal": "not a valid principal!!",
                "ConnectorArn": "arn:aws:config:us-east-1:000000000000:connector/c-1",
                "ScopeConfiguration": { "scopeType": "tenant", "allRegions": true },
            }),
        ),
        (
            "the service-linked recorder",
            "PutServiceLinkedConfigurationRecorder",
            json!({ "ServicePrincipal": "not a valid principal!!" }),
        ),
        (
            "the service-linked recorder delete",
            "DeleteServiceLinkedConfigurationRecorder",
            json!({ "ServicePrincipal": "not a valid principal!!" }),
        ),
        (
            "the recorder describe filter",
            "DescribeConfigurationRecorders",
            json!({ "ServicePrincipal": "not a valid principal!!" }),
        ),
        (
            "the recorder status describe filter",
            "DescribeConfigurationRecorderStatus",
            json!({ "ServicePrincipal": "not a valid principal!!" }),
        ),
    ] {
        let emulator = Emulator::new();
        let message = refusal(&emulator, operation, &body, "ValidationException").await;
        assert_eq!(
            message, EXPECTED,
            "case {case:?}: expected {EXPECTED:?}, got {message:?}"
        );
    }
}

/// A member the input carries inside a WRAPPER structure is named by the name the
/// model declares it with — not by the path through the wrapper — with the model's
/// `@pattern` printed as the model declares it.
#[tokio::test]
async fn a_wrapped_members_refusal_names_the_member_the_model_declares() {
    // (case, the StoredQuery the request carried, the clause the service states).
    // `StoredQuery`'s members and their constraints come from
    // `smithy_query(query="shape", shape="StoredQuery")`: `QueryName` 1..64 /
    // `^[a-zA-Z0-9-_]+$`, `QueryId` 1..36 / `^\S+$`.
    let long_name = "q".repeat(65);
    for (case, stored_query, expected) in [
        (
            "a query name with spaces",
            json!({ "QueryName": "invalid query name" }),
            "1 validation error detected: Value 'invalid query name' at 'QueryName' \
             failed to satisfy constraint: Member must satisfy regular expression pattern: \
             ^[a-zA-Z0-9-_]+$"
                .to_string(),
        ),
        (
            "a query name over its length maximum",
            json!({ "QueryName": long_name.clone() }),
            format!(
                "1 validation error detected: Value '{long_name}' at 'QueryName' failed to \
                 satisfy constraint: Member must have length less than or equal to 64"
            ),
        ),
        (
            "a query id with a space",
            json!({ "QueryName": "valid-name", "QueryId": "a b" }),
            "1 validation error detected: Value 'a b' at 'QueryId' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: ^\\S+$"
                .to_string(),
        ),
        (
            "an absent query name",
            json!({ "QueryId": "q-1" }),
            "1 validation error detected: Value null at 'QueryName' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
    ] {
        let emulator = Emulator::new();
        let message = refusal(
            &emulator,
            "PutStoredQuery",
            &json!({ "StoredQuery": stored_query }),
            "ValidationException",
        )
        .await;
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}
