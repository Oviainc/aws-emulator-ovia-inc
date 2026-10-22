// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `DeleteEndpointConfig`.
//!
//! RULE: `DeleteEndpointConfig` removes exactly the named endpoint-configuration record
//! from state and nothing else — the operation's model doc says it "deletes only the
//! specified configuration. It does not delete endpoints created using the
//! configuration." So after it: `DescribeEndpointConfig` refuses that name, every other
//! configuration, every model and every endpoint created from the deleted configuration
//! is unchanged, the name is free to create again, and a name nothing created is refused
//! (this operation models no errors, so that refusal is the common
//! HTTP 400 `ValidationException`, as for the sibling deletes).
//!
//! The cases assert that rule over a TABLE whose rows differ along the axes it
//! generalises over: the configuration's name shape, the region the caller signed for,
//! and the configuration's own shape (instance-backed, serverless, and the modelled
//! maximum variant count).
//!
//! The harness (`Emulator` and the body helpers) is the one
//! `tests/all/inference_deployment_lifecycle.rs` already defines, so these cases go over
//! the same wire through the same `MultiServiceRouter` as every other suite here.

use serde_json::{json, Value};

use super::inference_deployment_lifecycle::{
    error_code, message, one_variant_config, primary_container, str_field, Emulator, ACCOUNT,
};

/// The model every configuration below hosts. `CreateEndpointConfig` refuses a variant
/// naming a model that does not exist, so each case creates it first.
const MODEL: &str = "cfg-del-model";

/// An emulator holding [`MODEL`] and nothing else.
async fn emulator_with_model() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": MODEL, "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
    emulator
}

/// A serverless configuration: `ProductionVariant.ServerlessConfig` instead of an
/// instance count, with `MemorySizeInMB` / `MaxConcurrency` inside their modelled ranges
/// (1024..6144 and 1..200).
fn serverless_config(name: &str) -> Value {
    json!({
        "EndpointConfigName": name,
        "ProductionVariants": [{
            "VariantName": "Serverless",
            "ModelName": MODEL,
            "ServerlessConfig": { "MemorySizeInMB": 2048, "MaxConcurrency": 5 },
        }],
        "Tags": [{ "Key": "team", "Value": "inference" }],
    })
}

/// A configuration carrying the modelled maximum of ten variants
/// (`ProductionVariants` is len 1..10) plus optional members.
fn ten_variant_config(name: &str) -> Value {
    let variants: Vec<Value> = (0..10)
        .map(|index| {
            json!({
                "VariantName": format!("v{index}"),
                "ModelName": MODEL,
                "InitialInstanceCount": 1,
                "InstanceType": "ml.m5.large",
                "InitialVariantWeight": 0.5,
            })
        })
        .collect();
    json!({
        "EndpointConfigName": name,
        "ProductionVariants": variants,
        "KmsKeyId": "alias/ExampleAlias",
        "EnableNetworkIsolation": true,
    })
}

/// RULE: `DeleteEndpointConfig` removes exactly the named configuration — the reader
/// refuses it afterwards, a second delete of the same name is refused, a sibling
/// configuration survives, and the freed name can be created again.
#[tokio::test]
async fn delete_endpoint_config_removes_exactly_the_named_configuration() {
    // `EndpointConfigName` is modelled len 0..63 with pattern
    // ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$, so this is a name at the published maximum.
    let longest: String = format!("c{}", "9".repeat(62));

    // Rows: name shape x signed region x configuration shape.
    let cases: Vec<(String, &str, Value)> = vec![
        ("c".to_string(), "us-east-1", one_variant_config("c", MODEL)),
        (
            "cfg-9-b".to_string(),
            "eu-west-3",
            serverless_config("cfg-9-b"),
        ),
        (
            longest.clone(),
            "ap-southeast-2",
            ten_variant_config(&longest),
        ),
        ("99".to_string(), "us-west-2", one_variant_config("99", MODEL)),
    ];

    for (name, region, body) in cases {
        let emulator = emulator_with_model().await;
        // A sibling configuration the delete must leave alone.
        emulator
            .ok(
                "CreateEndpointConfig",
                &one_variant_config("survivor", MODEL),
            )
            .await;

        let (create_status, created) = emulator
            .call_in(region, "CreateEndpointConfig", &body)
            .await;
        assert_eq!(
            create_status, 200,
            "case {name:?} in {region}: expected the create to succeed, got \
             {create_status} {created}"
        );
        let arn = str_field(&created, "EndpointConfigArn");

        let (delete_status, deleted) = emulator
            .call_in(
                region,
                "DeleteEndpointConfig",
                &json!({ "EndpointConfigName": name }),
            )
            .await;
        assert_eq!(
            delete_status, 200,
            "case {name:?} in {region}: expected HTTP 200 from the delete, got \
             {delete_status} {deleted}"
        );
        assert_eq!(
            deleted,
            json!({}),
            "case {name:?}: the modelled output has no members, got {deleted:?}"
        );

        // The configuration is gone: its reader refuses the name, naming the ARN.
        let (read_status, read_body) = emulator
            .call_in(
                region,
                "DescribeEndpointConfig",
                &json!({ "EndpointConfigName": name }),
            )
            .await;
        assert_eq!(
            read_status, 400,
            "case {name:?}: the deleted configuration must be gone, got \
             {read_status} {read_body}"
        );
        assert_eq!(
            error_code(&read_body),
            "ValidationException",
            "case {name:?}: error code after the delete, got {read_body:?}"
        );
        assert!(
            message(&read_body).contains(&format!("{ACCOUNT}:endpoint-config/{name}")),
            "case {name:?}: the refusal must name the configuration it could not find, \
             got {read_body:?}"
        );

        // A second delete is refused the same way: the entry really went.
        let (repeat_status, repeat_body) = emulator
            .call_in(
                region,
                "DeleteEndpointConfig",
                &json!({ "EndpointConfigName": name }),
            )
            .await;
        assert_eq!(
            repeat_status, 400,
            "case {name:?}: deleting twice must be refused, got {repeat_status} {repeat_body}"
        );
        assert_eq!(
            error_code(&repeat_body),
            "ValidationException",
            "case {name:?}: error code for the second delete, got {repeat_body:?}"
        );
        assert!(
            message(&repeat_body).contains(&format!("{ACCOUNT}:endpoint-config/{name}")),
            "case {name:?}: the second delete must name the configuration it could not \
             find, got {repeat_body:?}"
        );

        // Only that entry went: the sibling configuration and the model are untouched.
        let survivor = emulator
            .ok(
                "DescribeEndpointConfig",
                &json!({ "EndpointConfigName": "survivor" }),
            )
            .await;
        assert_eq!(
            str_field(&survivor, "EndpointConfigName"),
            "survivor",
            "case {name:?}: the sibling configuration must survive, got {survivor:?}"
        );
        let model = emulator
            .ok("DescribeModel", &json!({ "ModelName": MODEL }))
            .await;
        assert_eq!(
            str_field(&model, "ModelName"),
            MODEL,
            "case {name:?}: the hosted model must survive, got {model:?}"
        );

        // The name is released, so the same create succeeds again and mints the same ARN.
        let (recreate_status, recreated) = emulator
            .call_in(region, "CreateEndpointConfig", &body)
            .await;
        assert_eq!(
            recreate_status, 200,
            "case {name:?}: the freed name must be creatable again, got \
             {recreate_status} {recreated}"
        );
        assert_eq!(
            str_field(&recreated, "EndpointConfigArn"),
            arn,
            "case {name:?}: the re-created configuration keeps the same ARN, got \
             {recreated:?}"
        );
        let replayed = emulator
            .call_in(
                region,
                "DescribeEndpointConfig",
                &json!({ "EndpointConfigName": name }),
            )
            .await;
        assert_eq!(
            replayed.0, 200,
            "case {name:?}: the re-created configuration must be readable, got \
             {} {}",
            replayed.0, replayed.1
        );
        assert_eq!(
            replayed.1.get("ProductionVariants"),
            body.get("ProductionVariants"),
            "case {name:?}: the re-created configuration replays its variants, got {:?}",
            replayed.1
        );
    }
}

/// RULE: deleting a configuration deletes no endpoint — every endpoint created from it
/// keeps the snapshot it took at `CreateEndpoint` — while a later `CreateEndpoint`
/// naming the deleted configuration is refused, because the configuration is no longer
/// in state.
#[tokio::test]
async fn deleting_a_configuration_leaves_the_endpoints_created_from_it() {
    // Rows: configurations of different shapes, each with a different number of
    // endpoints created from it.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "inst-cfg",
            one_variant_config("inst-cfg", MODEL),
            vec!["ep-one"],
        ),
        (
            "srvless-cfg",
            serverless_config("srvless-cfg"),
            vec!["ep-two", "ep-three"],
        ),
        (
            "wide-cfg",
            ten_variant_config("wide-cfg"),
            vec!["ep-four", "ep-five", "ep-six"],
        ),
    ];

    for (config, config_body, endpoints) in cases {
        let emulator = emulator_with_model().await;
        emulator.ok("CreateEndpointConfig", &config_body).await;
        for endpoint in &endpoints {
            emulator
                .ok(
                    "CreateEndpoint",
                    &json!({ "EndpointName": endpoint, "EndpointConfigName": config }),
                )
                .await;
        }

        let mut before: Vec<(&str, Value)> = Vec::new();
        for endpoint in &endpoints {
            before.push((
                endpoint,
                emulator
                    .ok("DescribeEndpoint", &json!({ "EndpointName": endpoint }))
                    .await,
            ));
        }

        emulator
            .ok(
                "DeleteEndpointConfig",
                &json!({ "EndpointConfigName": config }),
            )
            .await;

        // Every endpoint survives with the snapshot it took at create. `EndpointStatus`
        // and `LastModifiedTime` are left out: the clock owes the endpoint its
        // Creating -> InService transition independently of this delete.
        for (endpoint, snapshot) in &before {
            let after = emulator
                .ok("DescribeEndpoint", &json!({ "EndpointName": endpoint }))
                .await;
            for member in [
                "EndpointName",
                "EndpointArn",
                "EndpointConfigName",
                "CreationTime",
                "ProductionVariants",
            ] {
                assert_eq!(
                    after.get(member),
                    snapshot.get(member),
                    "case {config}/{endpoint}: {member} must be unchanged by deleting the \
                     configuration, got {after:?}"
                );
            }
        }

        // The configuration itself is gone, so a new endpoint naming it is refused.
        let (status, parsed) = emulator
            .call(
                "CreateEndpoint",
                &json!({ "EndpointName": "ep-after", "EndpointConfigName": config }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {config}: an endpoint naming the deleted configuration must be refused, \
             got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {config}: error code for the dangling reference, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&format!("{ACCOUNT}:endpoint-config/{config}")),
            "case {config}: the refusal must name the configuration it could not find, \
             got {parsed:?}"
        );

        // Once the configuration exists again, the endpoint create succeeds.
        emulator.ok("CreateEndpointConfig", &config_body).await;
        emulator
            .ok(
                "CreateEndpoint",
                &json!({ "EndpointName": "ep-after", "EndpointConfigName": config }),
            )
            .await;
    }
}
