// SPDX-License-Identifier: CC0-1.0

//! The CLASS and the WORDING of the refusals this service answers on the paths where
//! what the caller sent names nothing Config can act on, end to end over the wire.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — reusing the
//! `Emulator` harness `config_setup_and_rule_loop` defines.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded case:
//!
//! 1. a value that NAMES no resource type — neither one the modelled `ResourceType`
//!    value set carries nor a CloudFormation registry type name — is refused with
//!    Config's own fixed sentence about a resource type it does not support, which
//!    enumerates neither the value sent nor the types Config supports, wherever a member
//!    carrying a resource type sits;
//! 2. a `ServicePrincipal` Config can resolve no service-linked configuration recorder
//!    from is `ValidationException` worded about the service principal, while a
//!    principal whose recorder Config COULD resolve and this account does not hold is
//!    `NoSuchConfigurationRecorderException`;
//! 3. a request that names a Config rule this account does not hold, to evaluate it,
//!    is refused naming the one name that could not be resolved;
//! 4. a resource definition that is not valid JSON is refused with Config's fixed
//!    sentence about the JSON it is not;
//! 5. a named delivery channel this account does not hold is refused rather than
//!    reported as an empty status set;
//! 6. a configuration-history read refuses about the resource key it was given, not
//!    about the recorders the account holds.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state;

use crate::config_setup_and_rule_loop::{
    managed_rule, recorder_request, with_recorder_and_channel, Emulator, ROLE_ARN,
};

/// The sentence Config answers a resource type it does not support with, spelled out
/// here rather than read from the service's own constant so this suite pins the text
/// instead of restating it.
const UNSUPPORTED_RESOURCE_TYPE: &str =
    "The resource type you specified is not supported by AWS Config. Verify the resource \
     type, and try again.";

/// The sentence Config answers a `ServicePrincipal` it can resolve no service-linked
/// configuration recorder from with, spelled out for the same reason.
const UNRESOLVABLE_PRINCIPAL: &str =
    "The service principal of the configuration recorder you entered cannot be found. \
     Verify the configuration recorder service principal, and try again.";

/// The sentence Config answers a resource definition that is not valid JSON with,
/// spelled out for the same reason.
const CONFIGURATION_NOT_JSON: &str =
    "The provided resource configuration is not a valid JSON. Provide a valid JSON and \
     try again.";

/// Resource types every table that needs a SUPPORTED one names. Entries of
/// `com.amazonaws.configservice#ResourceType`, from `smithy_query(query="shape",
/// shape="ResourceType")`.
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`S3_BUCKET`].
const EC2_VOLUME: &str = "AWS::EC2::Volume";

/// The only published `EvaluationMode` of a resource evaluation
/// (`StartResourceEvaluationRequest$EvaluationMode`).
const PROACTIVE: &str = "PROACTIVE";

/// Call `operation`, require a 400 carrying the error code `code`, and answer the
/// message the service worded the refusal with.
async fn refusal(case: &str, emulator: &Emulator, operation: &str, body: &Value, code: &str) -> String {
    let (status, parsed) = emulator.call(operation, body).await;
    let actual = parsed["__type"]
        .as_str()
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
        .unwrap_or("<none>");
    assert_eq!(
        (status, actual),
        (400, code),
        "case {case:?}: expected HTTP 400 {code} from {operation}, got {status} {actual:?} \
         in {parsed}"
    );
    parsed["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {parsed}"))
        .to_string()
}

#[tokio::test]
async fn a_resource_type_config_does_not_support_is_refused_with_its_own_sentence() {
    // Rows differ along the value sent: a name that is no resource type at all, one
    // carrying an empty segment, and one whose segments are shorter than a registry type
    // name's. None of them NAMES a resource type, in the modelled set or in the
    // CloudFormation registry's form, so every member that carries one refuses them.
    let unsupported = ["not-a-resource-type", "AWS::::Account", "A::B::C"];

    for value in unsupported {
        let rows: [(&str, &str, Value); 5] = [
            (
                "a resource-type list",
                "AssociateResourceTypes",
                json!({
                    "ConfigurationRecorderArn": state::recorder_arn("us-east-1", "default", "cr-1"),
                    "ResourceTypes": [value],
                }),
            ),
            (
                "a resource-type list, second element",
                "DisassociateResourceTypes",
                json!({
                    "ConfigurationRecorderArn": state::recorder_arn("us-east-1", "default", "cr-1"),
                    "ResourceTypes": [S3_BUCKET, value],
                }),
            ),
            (
                "a single resource-type member",
                "GetResourceConfigHistory",
                json!({ "resourceType": value, "resourceId": "r-1" }),
            ),
            (
                "the discovery read's resource type",
                "ListDiscoveredResources",
                json!({ "resourceType": value }),
            ),
            (
                "the aggregate discovery read's resource type",
                "ListAggregateDiscoveredResources",
                json!({ "ConfigurationAggregatorName": "an-aggregator", "ResourceType": value }),
            ),
        ];

        for (position, operation, body) in rows {
            let case = format!("{position} carrying {value:?}");
            let emulator = Emulator::new();
            let message = refusal(&case, &emulator, operation, &body, "ValidationException").await;
            assert_eq!(
                message, UNSUPPORTED_RESOURCE_TYPE,
                "case {case:?}: expected {UNSUPPORTED_RESOURCE_TYPE:?}, got {message:?}"
            );
            assert!(
                !message.contains(value) && !message.contains(S3_BUCKET),
                "case {case:?}: the refusal enumerates neither the value nor the supported \
                 types, got {message:?}"
            );
        }
    }

    // A type-shaped name of nothing is a CloudFormation registry TYPE NAME, which is
    // what a custom resource is recorded under: the members that RECORD resource types
    // still refuse it (Config records only the types it supports), while a member that
    // only ASKS about a type name takes it — covered end to end in
    // `registry_resource_type_reads`.
    for value in ["AWS::NotAReal::ResourceType", "aws::s3::bucket"] {
        for (position, operation, body) in [
            (
                "a resource-type list",
                "AssociateResourceTypes",
                json!({
                    "ConfigurationRecorderArn": state::recorder_arn("us-east-1", "default", "cr-1"),
                    "ResourceTypes": [value],
                }),
            ),
            (
                "a single resource-type member",
                "GetResourceConfigHistory",
                json!({ "resourceType": value, "resourceId": "r-1" }),
            ),
        ] {
            let case = format!("{position} carrying {value:?}");
            let emulator = Emulator::new();
            let message = refusal(&case, &emulator, operation, &body, "ValidationException").await;
            assert_eq!(
                message, UNSUPPORTED_RESOURCE_TYPE,
                "case {case:?}: expected {UNSUPPORTED_RESOURCE_TYPE:?}, got {message:?}"
            );
        }
    }

    // The control: a SUPPORTED type reaches the handler, which answers about the
    // request's own subject instead — never with the sentence above.
    let emulator = Emulator::new();
    for (case, operation, body) in [
        (
            "a supported type on the association path",
            "AssociateResourceTypes",
            json!({
                "ConfigurationRecorderArn": state::recorder_arn("us-east-1", "default", "cr-1"),
                "ResourceTypes": [S3_BUCKET],
            }),
        ),
        (
            "a supported type on the history read",
            "GetResourceConfigHistory",
            json!({ "resourceType": S3_BUCKET, "resourceId": "r-1" }),
        ),
    ] {
        let (status, parsed) = emulator.call(operation, &body).await;
        assert_ne!(
            parsed["message"].as_str(),
            Some(UNSUPPORTED_RESOURCE_TYPE),
            "case {case:?}: a supported type is not refused as unsupported, got {status} \
             {parsed}"
        );
    }
}

#[tokio::test]
async fn a_service_principal_that_resolves_no_recorder_is_refused_about_the_principal() {
    let supported = state::SERVICE_LINKED_PRINCIPALS[0];

    // Rows differ along the service the principal names: services Config links no
    // configuration recorder to at all, and the service it links one to only through a
    // CONNECTOR. None of them resolves a recorder, however well formed it is.
    for principal in [
        "backup.amazonaws.com",
        "ec2.amazonaws.com",
        "securityhub.amazonaws.com",
        "telemetry.observabilityadmin.amazonaws.com",
    ] {
        let emulator = Emulator::new();
        let case = format!("deleting the recorder of {principal:?}");
        let message = refusal(
            &case,
            &emulator,
            "DeleteServiceLinkedConfigurationRecorder",
            &json!({ "ServicePrincipal": principal }),
            "ValidationException",
        )
        .await;
        assert_eq!(
            message, UNRESOLVABLE_PRINCIPAL,
            "case {case:?}: expected {UNRESOLVABLE_PRINCIPAL:?}, got {message:?}"
        );
        assert!(
            !message.contains(principal),
            "case {case:?}: the refusal quotes nothing of the value, got {message:?}"
        );
    }

    // A principal whose recorder Config COULD resolve — a service it links one to on
    // request — is the recorder this account does not hold instead, which names the
    // account and the principal it looked for.
    for principal in state::SERVICE_LINKED_PRINCIPALS {
        let emulator = Emulator::new();
        let case = format!("deleting the absent recorder of {principal:?}");
        let message = refusal(
            &case,
            &emulator,
            "DeleteServiceLinkedConfigurationRecorder",
            &json!({ "ServicePrincipal": principal }),
            "NoSuchConfigurationRecorderException",
        )
        .await;
        assert!(
            message.contains(principal),
            "case {case:?}: the refusal names the principal it looked for, got {message:?}"
        );
    }

    // A third-party recorder is linked to its connector, so with no connector behind
    // the ARN the principal resolves no recorder either, and the refusal is the same
    // sentence. Rows differ along the connector ARN sent: another account's, another
    // Region's, and one of this account that was never created.
    for (case, connector_arn) in [
        (
            "a connector of another account",
            "arn:aws:config:us-east-1:999999999999:connector/azure-tenant".to_string(),
        ),
        (
            "a connector of another Region",
            state::connector_arn("eu-west-1", "azure-tenant-elsewhere"),
        ),
        (
            "a connector that was never created",
            state::connector_arn("us-east-1", "azure-tenant-never-created"),
        ),
    ] {
        let emulator = Emulator::new();
        let message = refusal(
            case,
            &emulator,
            "PutThirdPartyServiceLinkedConfigurationRecorder",
            &json!({
                "ServicePrincipal": state::THIRD_PARTY_SERVICE_LINKED_PRINCIPALS[0],
                "ConnectorArn": connector_arn,
                "ScopeConfiguration": { "scopeType": "tenant", "allRegions": true },
            }),
            "ValidationException",
        )
        .await;
        assert_eq!(
            message, UNRESOLVABLE_PRINCIPAL,
            "case {case:?}: expected {UNRESOLVABLE_PRINCIPAL:?}, got {message:?}"
        );
        assert!(
            !message.contains(&connector_arn),
            "case {case:?}: the refusal quotes nothing of the value, got {message:?}"
        );
    }

    // The control: the supported principal's own recorder is created and then deleted,
    // so the refusals above are about the principal the request carried.
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutServiceLinkedConfigurationRecorder",
            &json!({ "ServicePrincipal": supported }),
        )
        .await;
    let deleted = emulator
        .ok(
            "DeleteServiceLinkedConfigurationRecorder",
            &json!({ "ServicePrincipal": supported }),
        )
        .await;
    assert_eq!(
        deleted["Name"],
        json!(state::service_linked_recorder_name(supported)
            .expect("a supported principal names a recorder")),
        "case \"the supported principal\": expected the recorder to be deleted, got {deleted}"
    );
}

#[tokio::test]
async fn an_evaluation_of_a_rule_this_account_does_not_hold_names_that_rule() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for held in ["rule-one", "rule-two"] {
        emulator.ok("PutConfigRule", &managed_rule(held)).await;
    }

    // Rows differ along how many names the request carries and WHERE the absent one
    // sits in the list: alone, first, last, and among rules this account does hold.
    // Whichever it is, the refusal names the one name that resolved no rule.
    let cases: [(&str, Vec<&str>, &str); 5] = [
        ("the only name", vec!["absent-rule"], "absent-rule"),
        (
            "the first of two",
            vec!["no_such_rule", "rule-one"],
            "no_such_rule",
        ),
        (
            "the last of three",
            vec!["rule-one", "rule-two", "r"],
            "r",
        ),
        (
            "between two rules the account holds",
            vec!["rule-one", "absent-rule", "rule-two"],
            "absent-rule",
        ),
        (
            "every name absent",
            vec!["absent-one", "absent-two"],
            "absent-one",
        ),
    ];

    for (case, names, offending) in cases {
        let message = refusal(
            case,
            &emulator,
            "StartConfigRulesEvaluation",
            &json!({ "ConfigRuleNames": names }),
            "NoSuchConfigRuleException",
        )
        .await;
        let expected = format!(
            "The ConfigRule '{offending}' provided in the request is invalid. Please check \
             the configRule name."
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The control: the rules this account does hold are still evaluated.
    emulator
        .ok(
            "StartConfigRulesEvaluation",
            &json!({ "ConfigRuleNames": ["rule-one", "rule-two"] }),
        )
        .await;
}

#[tokio::test]
async fn a_resource_configuration_that_is_not_valid_json_is_refused_as_json() {
    // Rows differ along the form of the body sent: JSON of the wrong kind, truncated
    // JSON, JSON with a trailing comma, and text that is no JSON at all.
    for (case, configuration) in [
        ("not JSON at all", "BucketName=bucket"),
        ("a truncated object", r#"{"BucketName": "bucket""#),
        ("a trailing comma", r#"{"BucketName": "bucket",}"#),
        ("a JSON array", r#"[{"BucketName":"bucket"}]"#),
        ("a JSON string", r#""BucketName""#),
        ("a JSON number", "42"),
        ("a bare word", "null-ish"),
    ] {
        let emulator = Emulator::new();
        let message = refusal(
            case,
            &emulator,
            "StartResourceEvaluation",
            &json!({
                "EvaluationMode": PROACTIVE,
                "ResourceDetails": {
                    "ResourceId": "r-1",
                    "ResourceType": S3_BUCKET,
                    "ResourceConfiguration": configuration,
                },
            }),
            "InvalidParameterValueException",
        )
        .await;
        assert_eq!(
            message, CONFIGURATION_NOT_JSON,
            "case {case:?}: expected {CONFIGURATION_NOT_JSON:?}, got {message:?}"
        );
        assert!(
            !message.contains(configuration),
            "case {case:?}: the refusal quotes nothing of the body sent, got {message:?}"
        );
    }

    // The control: the resource definition the schema type describes — a JSON object of
    // the resource's properties — starts an evaluation.
    let emulator = Emulator::new();
    let started = emulator
        .ok(
            "StartResourceEvaluation",
            &json!({
                "EvaluationMode": PROACTIVE,
                "ResourceDetails": {
                    "ResourceId": "r-1",
                    "ResourceType": S3_BUCKET,
                    "ResourceConfiguration": json!({ "BucketName": "bucket" }).to_string(),
                },
            }),
        )
        .await;
    assert!(
        started["ResourceEvaluationId"].as_str().is_some(),
        "case \"a resource definition\": expected an evaluation id, got {started}"
    );
}

#[tokio::test]
async fn a_delivery_channel_status_read_refuses_a_name_this_account_does_not_hold() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;

    // Rows differ along which names the request carries: one absent name, an absent
    // name beside the channel this account holds, and the same name twice.
    for (case, names, offending) in [
        ("one absent name", vec!["absent-channel"], "absent-channel"),
        (
            "an absent name beside the stored one",
            vec!["default", "absent-channel"],
            "absent-channel",
        ),
        (
            "a name of another account's form",
            vec!["sim2real-probe-config-channel"],
            "sim2real-probe-config-channel",
        ),
    ] {
        let message = refusal(
            case,
            &emulator,
            "DescribeDeliveryChannelStatus",
            &json!({ "DeliveryChannelNames": names }),
            "NoSuchDeliveryChannelException",
        )
        .await;
        let expected = state::no_such_delivery_channel_message(offending);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The control: the channel this account does hold is still reported, so the
    // refusals above are about the name asked for.
    let reported = emulator
        .ok(
            "DescribeDeliveryChannelStatus",
            &json!({ "DeliveryChannelNames": ["default"] }),
        )
        .await;
    assert_eq!(
        reported["DeliveryChannelsStatus"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "case \"the stored channel\": expected its status, got {reported}"
    );
}

#[tokio::test]
async fn a_history_read_refuses_about_the_resource_key_and_not_about_the_recorders() {
    // Rows differ along the recorders the account holds when the read is made: none at
    // all, a service-linked recorder, and the account's customer managed recorder. The
    // inventory outlives them all, so every row answers about the key asked for.
    for (case, setup) in [
        ("no configuration recorder", Vec::new()),
        (
            "a service-linked recorder only",
            vec![(
                "PutServiceLinkedConfigurationRecorder",
                json!({ "ServicePrincipal": state::SERVICE_LINKED_PRINCIPALS[0] }),
            )],
        ),
        (
            "a customer managed recorder",
            vec![(
                "PutConfigurationRecorder",
                recorder_request(json!({ "roleARN": ROLE_ARN })),
            )],
        ),
    ] {
        let emulator = Emulator::new();
        for (operation, body) in &setup {
            emulator.ok(operation, body).await;
        }

        for (key_case, resource_type, resource_id) in [
            ("an id nothing was recorded for", EC2_VOLUME, "vol-00000000000000404"),
            ("a type nothing was recorded for", S3_BUCKET, "a-bucket"),
        ] {
            let row = format!("{key_case} with {case}");
            let message = refusal(
                &row,
                &emulator,
                "GetResourceConfigHistory",
                &json!({ "resourceType": resource_type, "resourceId": resource_id }),
                "ResourceNotDiscoveredException",
            )
            .await;
            let expected = format!(
                "Resource {resource_id} of resourceType:{resource_type} is unknown or has \
                 not been discovered"
            );
            assert_eq!(
                message, expected,
                "case {row:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
