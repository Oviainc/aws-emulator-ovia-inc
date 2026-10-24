// SPDX-License-Identifier: CC0-1.0

//! The ECS CAPACITY PROVIDER control plane, driven end to end: `CreateCapacityProvider`
//! and `DescribeCapacityProviders`.
//!
//! The rules under test:
//!
//! * a capacity provider is a PERSISTED, account-scoped record — `CreateCapacityProvider`
//!   mints its ARN and stores the binding it was given (an Auto Scaling group, or capacity
//!   Amazon ECS manages), and `DescribeCapacityProviders` reports that stored record back:
//!   its name, status, ARN, binding, type and tags;
//! * the NAME identifies the provider and an Auto Scaling group backs exactly one, so a
//!   create that reuses either is refused and stores nothing;
//! * a name outside the documented character set, or carrying one of the prefixes AWS
//!   reserves, is refused; and so is a request that names neither binding, or both.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{create_cluster, list, ok, refusal, refused, service, string, ACCOUNT, SIGNED_REGION};

/// One row of the create table: how the provider is created, and what the stored record
/// must then report.
struct CreateCase {
    name: &'static str,
    provider_name: &'static str,
    /// The members the request carries beside `name`.
    request: fn() -> Value,
    /// The `cluster` the stored provider must report, if any.
    expect_cluster: Option<&'static str>,
    /// The `type` the stored provider must report, if any.
    expect_type: Option<&'static str>,
    /// The `autoScalingGroupProvider.autoScalingGroupArn` the stored provider must
    /// report, if any — the value AS GIVEN, since the service stores the reference the
    /// request carried rather than rewriting it.
    expect_group: fn() -> Option<String>,
    /// The tags the stored provider must report.
    expect_tags: Value,
}

/// An Auto Scaling group ARN of the account, as `autoScalingGroupProvider` takes it.
fn group_arn(name: &str) -> String {
    format!(
        "arn:aws:autoscaling:{SIGNED_REGION}:{ACCOUNT}:autoScalingGroup:\
         11111111-2222-3333-4444-555555555555:autoScalingGroupName/{name}"
    )
}

/// A managed-instances provider configuration, with the members the model requires
/// (`infrastructureRoleArn`, `instanceLaunchTemplate.ec2InstanceProfileArn` and that
/// template's `networkConfiguration`).
fn managed_instances() -> Value {
    json!({
        "infrastructureRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/ecsInfrastructureRole"),
        "instanceLaunchTemplate": {
            "ec2InstanceProfileArn": format!("arn:aws:iam::{ACCOUNT}:instance-profile/ecsInstanceRole"),
            "networkConfiguration": {
                "subnets": ["subnet-0123456789abcdef0"],
                "securityGroups": ["sg-0123456789abcdef0"],
            },
        },
    })
}

/// Describe one capacity provider by name and return it, with or without its tags.
async fn describe(
    router: &MultiServiceRouter,
    provider_name: &str,
    include_tags: bool,
    case: &str,
) -> Value {
    let mut request = json!({ "capacityProviders": [provider_name] });
    if include_tags {
        request["include"] = json!(["TAGS"]);
    }
    let body = ok(router, "DescribeCapacityProviders", request, case).await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: a stored capacity provider is described, not failed, got {body}"
    );
    let providers = list(&body, "capacityProviders", case);
    assert_eq!(
        providers.len(),
        1,
        "case {case:?}: expected exactly one described provider, got {body}"
    );
    providers[0].clone()
}

#[tokio::test]
async fn a_created_capacity_provider_reads_back_through_describe() {
    let router = service();
    create_cluster(&router, "capacity", "setup").await;

    // The rows differ along what backs the provider (an Auto Scaling group named by ARN,
    // the same named by its bare name, or capacity Amazon ECS manages), whether managed
    // scaling is configured, whether the provider is scoped to a cluster, and whether it
    // carries tags.
    let cases = [
        CreateCase {
            name: "an Auto Scaling group named by ARN, with managed scaling",
            provider_name: "byarn",
            request: || {
                json!({
                    "autoScalingGroupProvider": {
                        "autoScalingGroupArn": group_arn("asg-byarn"),
                        "managedScaling": {
                            "status": "ENABLED",
                            "targetCapacity": 90,
                            "minimumScalingStepSize": 2,
                            "maximumScalingStepSize": 20,
                            "instanceWarmupPeriod": 120,
                        },
                        "managedTerminationProtection": "ENABLED",
                        "managedDraining": "ENABLED",
                    },
                    "tags": [{ "key": "team", "value": "platform" }],
                })
            },
            expect_cluster: None,
            expect_type: None,
            expect_group: || Some(group_arn("asg-byarn")),
            expect_tags: json!([{ "key": "team", "value": "platform" }]),
        },
        CreateCase {
            name: "an Auto Scaling group named by its bare name, no managed scaling",
            provider_name: "byname",
            request: || {
                json!({
                    "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-byname" },
                })
            },
            expect_cluster: None,
            expect_type: None,
            expect_group: || Some("asg-byname".to_string()),
            expect_tags: json!([]),
        },
        CreateCase {
            name: "capacity Amazon ECS manages, scoped to a cluster",
            provider_name: "managed",
            request: || {
                json!({
                    "cluster": "capacity",
                    "managedInstancesProvider": managed_instances(),
                    "tags": [
                        { "key": "env", "value": "test" },
                        { "key": "owner", "value": "ecs" },
                    ],
                })
            },
            expect_cluster: Some("capacity"),
            expect_type: Some("MANAGED_INSTANCES"),
            expect_group: || None,
            expect_tags: json!([
                { "key": "env", "value": "test" },
                { "key": "owner", "value": "ecs" },
            ]),
        },
        CreateCase {
            name: "a cluster named by its ARN scopes the provider to that cluster",
            provider_name: "byclusterarn",
            request: || {
                json!({
                    "cluster": format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/capacity"),
                    "managedInstancesProvider": managed_instances(),
                })
            },
            expect_cluster: Some("capacity"),
            expect_type: Some("MANAGED_INSTANCES"),
            expect_group: || None,
            expect_tags: json!([]),
        },
    ];

    for test in &cases {
        let case = test.name;
        let mut request = (test.request)();
        request["name"] = json!(test.provider_name);
        let created = ok(&router, "CreateCapacityProvider", request, case).await;
        let created = created["capacityProvider"].clone();
        let expect_group = (test.expect_group)();

        // Create and read must agree member for member: the answer is the stored record,
        // not the request.
        let read = describe(&router, test.provider_name, true, case).await;
        for reported in [&created, &read] {
            assert_eq!(
                string(reported, "capacityProviderArn", case),
                format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/{}",
                    test.provider_name
                ),
                "case {case:?}: unexpected capacityProviderArn in {reported}"
            );
            assert_eq!(
                string(reported, "name", case),
                test.provider_name,
                "case {case:?}: unexpected name in {reported}"
            );
            assert_eq!(
                string(reported, "status", case),
                "ACTIVE",
                "case {case:?}: a created provider is usable at once, got {reported}"
            );
            assert_eq!(
                reported["cluster"].as_str(),
                test.expect_cluster,
                "case {case:?}: unexpected cluster in {reported}"
            );
            assert_eq!(
                reported["type"].as_str(),
                test.expect_type,
                "case {case:?}: unexpected type in {reported}"
            );
            assert_eq!(
                reported["autoScalingGroupProvider"]["autoScalingGroupArn"].as_str(),
                expect_group.as_deref(),
                "case {case:?}: unexpected autoScalingGroupProvider in {reported}"
            );
            assert_eq!(
                list(reported, "tags", case),
                test.expect_tags
                    .as_array()
                    .expect("the expected tags are a list")
                    .clone(),
                "case {case:?}: unexpected tags in {reported}"
            );
            // Nothing is in flight for a provider that was just created.
            assert!(
                reported["updateStatus"].is_null()
                    && reported["updateStatusReason"].is_null(),
                "case {case:?}: a created provider reports no update in flight, got {reported}"
            );
        }

        // "If this field is omitted, tags aren't included in the response."
        let without_tags = describe(&router, test.provider_name, false, case).await;
        assert!(
            without_tags["tags"].is_null(),
            "case {case:?}: tags are reported only when include names TAGS, got {without_tags}"
        );
    }

    // The managed-scaling row's configuration is stored whole.
    let case = "managed scaling reads back";
    let scaling = describe(&router, "byarn", false, case).await;
    let scaling = &scaling["autoScalingGroupProvider"]["managedScaling"];
    assert_eq!(
        (
            scaling["status"].as_str(),
            scaling["targetCapacity"].as_i64(),
            scaling["minimumScalingStepSize"].as_i64(),
            scaling["maximumScalingStepSize"].as_i64(),
            scaling["instanceWarmupPeriod"].as_i64(),
        ),
        (Some("ENABLED"), Some(90), Some(2), Some(20), Some(120)),
        "case {case:?}: expected the stored managed scaling back, got {scaling}"
    );

    // Every provider the account holds is described when the request names none.
    let case = "every provider";
    let body = ok(&router, "DescribeCapacityProviders", json!({}), case).await;
    let mut names: Vec<String> = list(&body, "capacityProviders", case)
        .iter()
        .map(|provider| string(provider, "name", case))
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["byarn", "byclusterarn", "byname", "managed"],
        "case {case:?}: expected every stored provider, got {body}"
    );
}

#[tokio::test]
async fn a_capacity_provider_request_that_cannot_be_satisfied_is_refused() {
    let router = service();
    create_cluster(&router, "capacity", "setup").await;
    ok(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "taken",
            "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-taken") },
        }),
        "setup",
    )
    .await;

    // One row per rule the handler enforces, each with the modelled error it answers.
    // The name rows span the reserved prefixes (`CreateCapacityProvider$name`: "The name
    // can't be prefixed with \"aws\", \"ecs\", or \"fargate\"") in both spellings, the
    // character set and the documented length.
    let cases: [(&str, Value, &str); 12] = [
        (
            "a name prefixed with aws",
            json!({
                "name": "aws-provider",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "InvalidParameterException",
        ),
        (
            "a name prefixed with ecs",
            json!({
                "name": "ecsprovider",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "InvalidParameterException",
        ),
        (
            "a name prefixed with fargate",
            json!({
                "name": "fargate_spot_clone",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "InvalidParameterException",
        ),
        (
            "a reserved prefix in upper case",
            json!({
                "name": "AWSProvider",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "InvalidParameterException",
        ),
        (
            "a name outside the documented character set",
            json!({
                "name": "not a provider name",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "InvalidParameterException",
        ),
        (
            "a name longer than 255 characters",
            json!({
                "name": "p".repeat(256),
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "InvalidParameterException",
        ),
        (
            "neither an Auto Scaling group nor managed instances",
            json!({ "name": "nothing" }),
            "InvalidParameterException",
        ),
        (
            "both an Auto Scaling group and managed instances",
            json!({
                "name": "both",
                "cluster": "capacity",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-both" },
                "managedInstancesProvider": managed_instances(),
            }),
            "InvalidParameterException",
        ),
        (
            "managed instances with no cluster to scope them to",
            json!({
                "name": "unscoped",
                "managedInstancesProvider": managed_instances(),
            }),
            "InvalidParameterException",
        ),
        (
            "a cluster that does not exist",
            json!({
                "name": "ghostcluster",
                "cluster": "nosuchcluster",
                "managedInstancesProvider": managed_instances(),
            }),
            "ClusterNotFoundException",
        ),
        (
            "a name the account already holds",
            json!({
                "name": "taken",
                "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-other") },
            }),
            "ClientException",
        ),
        (
            "an Auto Scaling group another provider already backs, named by its bare name",
            json!({
                "name": "second",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-taken" },
            }),
            "ClientException",
        ),
    ];

    for (case, request, expected) in cases {
        refused(&router, "CreateCapacityProvider", request, expected, case).await;
    }

    // Nothing above was stored: a refused create creates nothing.
    let case = "only the provider that was accepted exists";
    let body = ok(&router, "DescribeCapacityProviders", json!({}), case).await;
    let names: Vec<String> = list(&body, "capacityProviders", case)
        .iter()
        .map(|provider| string(provider, "name", case))
        .collect();
    assert_eq!(
        names,
        vec!["taken".to_string()],
        "case {case:?}: a refused create must store nothing, got {body}"
    );

    // The group-reuse refusal names the provider that already backs the group, so the
    // caller can find it.
    let case = "the reuse refusal names the holder";
    let (code, message) = refusal(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "third",
            "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-taken") },
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "ClientException",
        "case {case:?}: expected a ClientException, got {code:?} with {message:?}"
    );
    assert!(
        message.contains("taken"),
        "case {case:?}: expected the refusal to name the provider holding the group, \
         got {message:?}"
    );
}

#[tokio::test]
async fn describe_capacity_providers_filters_pages_and_reports_failures() {
    let router = service();
    create_cluster(&router, "scoped", "setup").await;
    create_cluster(&router, "other", "setup").await;

    // Twelve providers here plus the cluster-scoped one below, so the documented page
    // size of 10 is crossed.
    let names: Vec<String> = (0..12).map(|index| format!("provider{index:02}")).collect();
    for name in &names {
        ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": name,
                "autoScalingGroupProvider": { "autoScalingGroupArn": format!("asg-{name}") },
            }),
            "setup",
        )
        .await;
    }
    // One provider scoped to `scoped` by construction, and one merely ASSOCIATED with it.
    ok(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "scopedmanaged",
            "cluster": "scoped",
            "managedInstancesProvider": managed_instances(),
        }),
        "setup",
    )
    .await;
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": "scoped",
            "capacityProviders": ["provider00"],
            "defaultCapacityProviderStrategy": [
                { "capacityProvider": "provider00", "weight": 1 },
            ],
        }),
        "setup",
    )
    .await;

    // Paging walks every provider exactly once, in name order, whatever page size the
    // request asks for: the rows differ along that size, and the unpaged default.
    let page_sizes: [Option<i64>; 4] = [None, Some(1), Some(5), Some(10)];
    for size in page_sizes {
        let case = &format!("paging {size:?} at a time");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({});
            if let Some(size) = size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let body = ok(&router, "DescribeCapacityProviders", request, case).await;
            let page = list(&body, "capacityProviders", case);
            let expected_page = size.unwrap_or(10);
            assert!(
                page.len() as i64 <= expected_page,
                "case {case:?}: a page carries at most {expected_page} providers, got {body}"
            );
            seen.extend(page.iter().map(|provider| string(provider, "name", case)));
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut expected: Vec<String> = names.clone();
        expected.push("scopedmanaged".to_string());
        expected.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every provider exactly once in name order"
        );
    }

    // "This value can be between 1 and 10."
    for (case, max_results) in [("zero", 0), ("eleven", 11)] {
        refused(
            &router,
            "DescribeCapacityProviders",
            json!({ "maxResults": max_results }),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // "only capacity providers associated with this cluster are returned, including
    // Amazon ECS Managed Instances capacity providers".
    let case = "the providers of one cluster";
    let body = ok(
        &router,
        "DescribeCapacityProviders",
        json!({ "cluster": "scoped" }),
        case,
    )
    .await;
    let mut scoped: Vec<String> = list(&body, "capacityProviders", case)
        .iter()
        .map(|provider| string(provider, "name", case))
        .collect();
    scoped.sort();
    assert_eq!(
        scoped,
        vec!["provider00".to_string(), "scopedmanaged".to_string()],
        "case {case:?}: expected the associated and the cluster-scoped provider, got {body}"
    );
    let case = "a cluster with no providers";
    let body = ok(
        &router,
        "DescribeCapacityProviders",
        json!({ "cluster": "other" }),
        case,
    )
    .await;
    assert!(
        list(&body, "capacityProviders", case).is_empty(),
        "case {case:?}: expected no provider for a cluster holding none, got {body}"
    );

    // A name that matches nothing is a FAILURE entry beside the results, not a refusal.
    let case = "a mix of known and unknown names";
    let body = ok(
        &router,
        "DescribeCapacityProviders",
        json!({
            "capacityProviders": [
                "provider00",
                format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/provider01"),
                "ghost",
            ],
        }),
        case,
    )
    .await;
    let found: Vec<String> = list(&body, "capacityProviders", case)
        .iter()
        .map(|provider| string(provider, "name", case))
        .collect();
    assert_eq!(
        found,
        vec!["provider00".to_string(), "provider01".to_string()],
        "case {case:?}: a provider named by ARN is the same provider, got {body}"
    );
    assert_eq!(
        list(&body, "failures", case),
        vec![json!({
            "arn": format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:capacity-provider/ghost"),
            "reason": "MISSING",
        })],
        "case {case:?}: unexpected failures in {body}"
    );

    // "Up to 100 capacity providers can be described in an action."
    let overlong: Vec<String> = (0..101).map(|index| format!("name{index}")).collect();
    refused(
        &router,
        "DescribeCapacityProviders",
        json!({ "capacityProviders": overlong }),
        "InvalidParameterException",
        "more than a hundred providers in one describe",
    )
    .await;
}
