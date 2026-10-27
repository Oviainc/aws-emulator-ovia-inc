// SPDX-License-Identifier: CC0-1.0

//! Which resource TYPES the CodeDeploy resource-tagging operations act on,
//! driven over the wire.
//!
//! RULE under test: `TagResource`, `UntagResource` and `ListTagsForResource` act
//! only on the resource types CodeDeploy makes taggable — an application and a
//! deployment group — so a well-formed ARN whose resource part names any other
//! type is refused with HTTP 400 / `ArnNotSupportedException` before the named
//! resource is looked up at all, whether or not that resource exists; the
//! tag-writing operations word the refusal "The ARN: <arn> is not supported by
//! CodeDeploy" and the tag-reading one "Not a CodeDeploy ARN: <arn>", each
//! quoting the `ResourceArn` exactly as the request sent it.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle` — so the boundary validator,
//! this service's refusal wording and the awsJson1_1 envelope are exercised.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, seeded_group, Emulator};

use service_com_amazonaws_codedeploy_codedeploy_20141006::state;

/// The region the harness's unsigned calls are served in, which is the one the
/// service builds an ARN with when a request carries no credential scope.
const REGION: &str = "us-east-1";

/// One row of the resource-type table: the case name, the `ResourceArn` under
/// test, and whether these operations act on the resource type it names.
type ArnCase<'a> = (&'a str, String, bool);

/// A `Tags` list entry, as CodeDeploy's `Tag` shape names its members
/// (`smithy_query(query="shape",shape="Tag")`: `Key`, `Value`).
fn tag(key: &str, value: &str) -> Value {
    json!({"Key": key, "Value": value})
}

/// The `$.message` of a failure body: the sentence the refusal carries.
fn message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// The sentence the tag-WRITING operations answer an unsupported ARN with.
fn write_refusal(resource_arn: &str) -> String {
    format!("The ARN: {resource_arn} is not supported by CodeDeploy")
}

/// The sentence the tag-READING operation answers the same ARN with.
fn read_refusal(resource_arn: &str) -> String {
    format!("Not a CodeDeploy ARN: {resource_arn}")
}

/// The `(key, value)` pairs `ListTagsForResource` reports for `resource_arn`,
/// requiring the read to succeed.
async fn tags_of(emulator: &Emulator, resource_arn: &str) -> Vec<(String, String)> {
    let listed = emulator
        .ok("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await;
    listed["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["Key"].as_str().unwrap_or_default().to_string(),
                entry["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Create a `Server` deployment configuration under `name`.
async fn create_config(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "CreateDeploymentConfig",
            json!({
                "deploymentConfigName": name,
                "computePlatform": "Server",
                "minimumHealthyHosts": {"type": "HOST_COUNT", "value": 1},
            }),
        )
        .await;
}

/// RULE: the resource TYPE an ARN names decides whether these operations act on
/// it — an application and a deployment group are tagged, every other type is
/// refused with `ArnNotSupportedException` in each operation's own wording, and
/// the existence of the named resource never enters into it.
#[tokio::test]
async fn only_application_and_deployment_group_arns_carry_tags() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateApplication", json!({"applicationName": "typed-app"}))
        .await;
    let group = seeded_group(&emulator, "typed-group-app", "Server").await;
    create_config(&emulator, "typed.config-1").await;

    // Rows differ along the resource TYPE the ARN's resource part names, whether
    // the named resource exists (created, predefined, absent), the identifier it
    // carries, and the partition / region / account segments that are no part of
    // a resource's identity. The two taggable types are the positive controls.
    let cases: &[ArnCase<'_>] = &[
        ("an application", state::application_arn(REGION, "typed-app"), true),
        (
            "a deployment group",
            state::deployment_group_arn(REGION, "typed-group-app", &group),
            true,
        ),
        (
            "a created deployment configuration",
            state::deployment_config_arn(REGION, "typed.config-1"),
            false,
        ),
        (
            "a predefined deployment configuration",
            state::deployment_config_arn(REGION, "CodeDeployDefault.OneAtATime"),
            false,
        ),
        (
            "a deployment configuration that does not exist",
            state::deployment_config_arn(REGION, "absent-config"),
            false,
        ),
        (
            "a deployment configuration ARN carrying no name",
            "arn:aws:codedeploy:us-east-1:000000000000:deploymentconfig:".to_string(),
            false,
        ),
        (
            "a deployment configuration ARN in another account",
            "arn:aws:codedeploy:us-east-1:111122223333:deploymentconfig:typed.config-1".to_string(),
            false,
        ),
        (
            "a deployment configuration ARN in another partition and region",
            "arn:aws-cn:codedeploy:cn-north-1:000000000000:deploymentconfig:typed.config-1"
                .to_string(),
            false,
        ),
        (
            "a deployment",
            "arn:aws:codedeploy:us-east-1:000000000000:deployment:d-ABCDEFGHI".to_string(),
            false,
        ),
        (
            "an on-premises instance",
            state::instance_arn(REGION, "host-01", "registration"),
            false,
        ),
        (
            "another service's resource",
            "arn:aws:iam::000000000000:role/CodeDeployRole".to_string(),
            false,
        ),
    ];

    for (case, resource_arn, supported) in cases {
        // The three operations that take a `ResourceArn`, each with a request
        // body it would otherwise serve, and the refusal sentence IT answers.
        let calls: [(&str, Value, String); 3] = [
            (
                "TagResource",
                json!({"ResourceArn": resource_arn, "Tags": [tag("Team", "probe")]}),
                write_refusal(resource_arn),
            ),
            (
                "ListTagsForResource",
                json!({"ResourceArn": resource_arn}),
                read_refusal(resource_arn),
            ),
            (
                "UntagResource",
                json!({"ResourceArn": resource_arn, "TagKeys": ["Team"]}),
                write_refusal(resource_arn),
            ),
        ];

        for (operation, request, refusal) in calls {
            let (status, body) = emulator.call(operation, request).await;
            if *supported {
                assert_eq!(
                    status, 200,
                    "case {case:?}: {operation} acts on this resource type, got {body:?}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} refuses this resource type, got {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ArnNotSupportedException",
                "case {case:?}: expected ArnNotSupportedException from {operation}, got {body:?}"
            );
            let actual = message(case, &body);
            assert_eq!(
                actual, refusal,
                "case {case:?}: expected {refusal:?} from {operation}, got {actual:?}"
            );
        }
    }

    // The positive controls really round-tripped: the tag the loop applied was
    // read back, and the removal that followed left the resource untagged.
    for (case, resource_arn, supported) in cases {
        if !supported {
            continue;
        }
        let reported = tags_of(&emulator, resource_arn).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: the taggable resource was tagged and untagged again, got {reported:?}"
        );
    }
}

/// RULE: a refused tagging call writes nothing — the resource whose ARN was
/// refused reads back exactly as it did before, and it reports no tags of its
/// own through the operation that DOES report it.
#[tokio::test]
async fn a_refused_tagging_call_leaves_the_resource_untouched() {
    let emulator = Emulator::new();
    create_config(&emulator, "untouched-config").await;
    let resource_arn = state::deployment_config_arn(REGION, "untouched-config");

    let before = emulator
        .ok(
            "GetDeploymentConfig",
            json!({"deploymentConfigName": "untouched-config"}),
        )
        .await;
    let listed_before = emulator.ok("ListDeploymentConfigs", json!({})).await;

    // Rows differ along the call made against the refused ARN: writing a tag,
    // removing one, and reading them.
    let attempts: &[(&str, Value)] = &[
        (
            "TagResource",
            json!({"ResourceArn": resource_arn, "Tags": [tag("Team", "probe-config")]}),
        ),
        (
            "UntagResource",
            json!({"ResourceArn": resource_arn, "TagKeys": ["Team"]}),
        ),
        (
            "ListTagsForResource",
            json!({"ResourceArn": resource_arn}),
        ),
    ];
    for (operation, request) in attempts {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: the refusal is a 400 client error, got {body:?}"
        );
    }

    let after = emulator
        .ok(
            "GetDeploymentConfig",
            json!({"deploymentConfigName": "untouched-config"}),
        )
        .await;
    assert_eq!(
        after, before,
        "case \"the configuration survives the refusals\": expected {before:?}, got {after:?}"
    );
    let listed_after = emulator.ok("ListDeploymentConfigs", json!({})).await;
    assert_eq!(
        listed_after, listed_before,
        "case \"the listing survives the refusals\": expected {listed_before:?}, got \
         {listed_after:?}"
    );

    // The taggable resources of the same account are untouched too: an
    // application created after the refusals still tags and reads back.
    emulator
        .ok("CreateApplication", json!({"applicationName": "still-taggable"}))
        .await;
    let application_arn = state::application_arn(REGION, "still-taggable");
    emulator
        .ok(
            "TagResource",
            json!({"ResourceArn": application_arn, "Tags": [tag("Team", "probe-control")]}),
        )
        .await;
    let reported = tags_of(&emulator, &application_arn).await;
    assert_eq!(
        reported,
        vec![("Team".to_string(), "probe-control".to_string())],
        "case \"a taggable resource still round-trips\": got {reported:?}"
    );
}
