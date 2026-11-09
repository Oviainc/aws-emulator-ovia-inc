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
//! * a provider over an Auto Scaling group the caller ALREADY has is created `ACTIVE`,
//!   while one whose capacity Amazon ECS has to stand up is created `PROVISIONING` with
//!   its create in flight (what it settles into is
//!   [`crate::cluster_scoped_capacity_providers`]'s subject);
//! * the NAME identifies the provider and an Auto Scaling group backs exactly one, so a
//!   create that reuses either is refused and stores nothing;
//! * the create is `@idempotent`, so REPEATING it for the provider that is already stored
//!   — the same name over the same binding — reports that provider unchanged and writes
//!   nothing, while a repeat that would have to change the stored binding is refused;
//! * a name outside the documented character set is refused as a malformed parameter, and
//!   one carrying a prefix AWS reserves as an identifier that is not the caller's; and so
//!   is a request that names neither binding, or both.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front
//! door a client uses, so nothing reaches into the state struct and a response that is not
//! computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_auto_scaling_group, create_cluster, list, ok, refusal, refused, service, string,
    ACCOUNT, SIGNED_REGION,
};

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
    /// The `status` the create answers under: a provider over a group the caller already
    /// has is usable at once, while one whose capacity Amazon ECS has to stand up is
    /// still `PROVISIONING`.
    expect_status: &'static str,
    /// The `updateStatus` reported beside it — the create that is still running, and
    /// nothing at all when there was none to run.
    expect_update_status: Option<&'static str>,
}

/// An Auto Scaling group ARN of the account, as `autoScalingGroupProvider` takes it.
/// Shared with every suite that needs a provider over a group the caller already has.
pub(crate) fn group_arn(name: &str) -> String {
    format!(
        "arn:aws:autoscaling:{SIGNED_REGION}:{ACCOUNT}:autoScalingGroup:\
         11111111-2222-3333-4444-555555555555:autoScalingGroupName/{name}"
    )
}

/// A managed-instances provider configuration, with the members the model requires
/// (`infrastructureRoleArn`, `instanceLaunchTemplate.ec2InstanceProfileArn` and that
/// template's `networkConfiguration`). Shared with the daemon suite, which needs a
/// MANAGED_INSTANCES provider to create a daemon over.
pub(crate) fn managed_instances() -> Value {
    managed_instances_monitored(None)
}

/// The same configuration with the launch template's optional `monitoring` option set to
/// `monitoring` when a suite names one — the ONE place this payload is spelled, so every
/// suite creates the same provider.
pub(crate) fn managed_instances_monitored(monitoring: Option<&str>) -> Value {
    let mut configuration = json!({
        "infrastructureRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/ecsInfrastructureRole"),
        "instanceLaunchTemplate": {
            "ec2InstanceProfileArn": format!("arn:aws:iam::{ACCOUNT}:instance-profile/ecsInstanceRole"),
            "networkConfiguration": {
                "subnets": ["subnet-0123456789abcdef0"],
                "securityGroups": ["sg-0123456789abcdef0"],
            },
        },
    });
    if let Some(monitoring) = monitoring {
        configuration["instanceLaunchTemplate"]["monitoring"] = json!(monitoring);
    }
    configuration
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
    // Every Auto Scaling group a row below binds a provider onto, created through Auto
    // Scaling's own API first: a provider over a group the account does not hold is
    // refused.
    for group in ["asg-byarn", "asg-byname"] {
        create_auto_scaling_group(&router, group, "setup").await;
    }

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
            expect_type: Some("EC2_AUTOSCALING"),
            expect_group: || Some(group_arn("asg-byarn")),
            expect_tags: json!([{ "key": "team", "value": "platform" }]),
            expect_status: "ACTIVE",
            expect_update_status: None,
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
            expect_type: Some("EC2_AUTOSCALING"),
            expect_group: || Some("asg-byname".to_string()),
            expect_tags: json!([]),
            expect_status: "ACTIVE",
            expect_update_status: None,
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
            expect_status: "PROVISIONING",
            expect_update_status: Some("CREATE_IN_PROGRESS"),
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
            expect_status: "PROVISIONING",
            expect_update_status: Some("CREATE_IN_PROGRESS"),
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
                test.expect_status,
                "case {case:?}: unexpected status in {reported}"
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
            // The create itself is the only update in flight, and it did what it was
            // asked, so no reason is stated for it.
            assert_eq!(
                reported["updateStatus"].as_str(),
                test.expect_update_status,
                "case {case:?}: unexpected updateStatus in {reported}"
            );
            assert!(
                reported["updateStatusReason"].is_null(),
                "case {case:?}: a create that did what it was asked states no reason, got \
                 {reported}"
            );
        }

        // "If this field is omitted, tags aren't included in the response": the member is
        // still reported, carrying no tag.
        let without_tags = describe(&router, test.provider_name, false, case).await;
        assert_eq!(
            without_tags["tags"],
            json!([]),
            "case {case:?}: a provider read without include reports an empty tag list, got \
             {without_tags}"
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

    // Every provider the account holds is described when the request names none —
    // including the two Amazon ECS manages for every account in every Region (`FARGATE`
    // and `FARGATE_SPOT`), which nothing has to create.
    let case = "every provider";
    let body = ok(&router, "DescribeCapacityProviders", json!({}), case).await;
    let mut names: Vec<String> = list(&body, "capacityProviders", case)
        .iter()
        .map(|provider| string(provider, "name", case))
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "FARGATE",
            "FARGATE_SPOT",
            "byarn",
            "byclusterarn",
            "byname",
            "managed"
        ],
        "case {case:?}: expected every stored provider, got {body}"
    );
}

#[tokio::test]
async fn a_capacity_provider_request_that_cannot_be_satisfied_is_refused() {
    let router = service();
    create_cluster(&router, "capacity", "setup").await;
    // The groups the rows below bind onto, so each row is refused for the rule it is about
    // rather than for a group the account does not hold.
    for group in ["asg-taken", "asg-other"] {
        create_auto_scaling_group(&router, group, "setup").await;
    }
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
    // character set and the documented length. The name is the provider's IDENTIFIER, so
    // every value outside that one rule earns the `ClientException` modelled for an
    // identifier that isn't valid; the rows about other members earn the
    // malformed-parameter refusal.
    let cases: [(&str, Value, &str); 12] = [
        (
            "a name prefixed with aws",
            json!({
                "name": "aws-provider",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "ClientException",
        ),
        (
            "a name prefixed with ecs",
            json!({
                "name": "ecsprovider",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "ClientException",
        ),
        (
            "a name prefixed with fargate",
            json!({
                "name": "fargate_spot_clone",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "ClientException",
        ),
        (
            "a reserved prefix in upper case",
            json!({
                "name": "AWSProvider",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "ClientException",
        ),
        (
            "a name outside the documented character set",
            json!({
                "name": "not a provider name",
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "ClientException",
        ),
        (
            "a name longer than 255 characters",
            json!({
                "name": "p".repeat(256),
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-1" },
            }),
            "ClientException",
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

    // Nothing above was stored: a refused create creates nothing. The account's two
    // ECS-managed providers are there whether or not anybody created anything.
    let case = "only the provider that was accepted exists";
    let body = ok(&router, "DescribeCapacityProviders", json!({}), case).await;
    let names: Vec<String> = list(&body, "capacityProviders", case)
        .iter()
        .map(|provider| string(provider, "name", case))
        .collect();
    assert_eq!(
        names,
        vec![
            "FARGATE".to_string(),
            "FARGATE_SPOT".to_string(),
            "taken".to_string()
        ],
        "case {case:?}: a refused create must store nothing, got {body}"
    );

    // Every refusal of the NAME carries the one sentence that states the whole name rule,
    // whichever half of it the value broke: the rows differ along which half that is.
    let name_rule = "The specified capacity provider name is invalid. Up to 255 characters \
                     are allowed, including letters (upper and lowercase), numbers, \
                     underscores, and hyphens. The name cannot be prefixed with \"aws\", \
                     \"ecs\", or \"fargate\". Specify a valid name and try again.";
    let name_rows: [(&str, String); 3] = [
        ("a reserved prefix", "ecs-mine".to_string()),
        ("a character the member does not allow", "not a name".to_string()),
        ("a name past the documented length", "p".repeat(256)),
    ];
    for (case, provider_name) in name_rows {
        let (code, message) = refusal(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": provider_name,
                "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-unused" },
            }),
            case,
        )
        .await;
        assert_eq!(
            code, "ClientException",
            "case {case:?}: the name is the provider's identifier, got {code:?} with \
             {message:?}"
        );
        assert_eq!(
            message, name_rule,
            "case {case:?}: unexpected refusal message"
        );
    }

    // The group-reuse refusal names the MEMBER at fault and what a retry has to carry:
    // the Auto Scaling group ARN has to be one no other provider is using.
    let case = "the reuse refusal names the member at fault";
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
    assert_eq!(
        message,
        "The specified Auto Scaling group ARN is already being used by another capacity \
         provider. Specify a unique Auto Scaling group ARN and try again.",
        "case {case:?}: unexpected refusal message"
    );
}

/// One row of the repeat table: how the provider is first created, how the create is
/// REPEATED, and whether that repeat is the same provider (an idempotent no-op) or a
/// request that conflicts with what is stored.
struct RepeatCase {
    name: &'static str,
    /// The members the first create carries beside `name`.
    create: fn() -> Value,
    /// The members the repeat carries beside `name`.
    repeat: fn() -> Value,
    /// Whether the repeat asks for the provider that is already stored.
    idempotent: bool,
}

#[tokio::test]
async fn repeating_a_capacity_provider_create_reports_the_stored_provider() {
    let router = service();
    create_cluster(&router, "capacity", "setup").await;
    create_cluster(&router, "elsewhere", "setup").await;
    // Every group a row below binds onto, including the one a repeat tries to move a
    // provider to: a group the account does not hold is refused before the repeat rule is
    // ever reached.
    for group in [
        "asg-r0", "asg-r1", "asg-r2", "asg-r4", "asg-r5", "asg-other5", "asg-r6", "asg-r7",
    ] {
        create_auto_scaling_group(&router, group, "setup").await;
    }

    // `CreateCapacityProvider` carries the `@idempotent` trait
    // (`smithy_query(query="operation",operation="CreateCapacityProvider")` reports
    // `idempotent: true`), so the rows below span the repeats a caller makes: the same
    // binding spelled the same way and spelled the other way, each kind of binding, a
    // repeat differing only in metadata, and the repeats that would have to CHANGE the
    // stored binding.
    let cases = [
        RepeatCase {
            name: "the same Auto Scaling group, named the same way",
            create: || json!({ "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-r0") } }),
            repeat: || json!({ "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-r0") } }),
            idempotent: true,
        },
        RepeatCase {
            name: "the same Auto Scaling group, named by its bare name on the repeat",
            create: || json!({ "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-r1") } }),
            repeat: || json!({ "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-r1" } }),
            idempotent: true,
        },
        RepeatCase {
            name: "the same group with the same managed scaling configuration",
            create: || {
                json!({
                    "autoScalingGroupProvider": {
                        "autoScalingGroupArn": group_arn("asg-r2"),
                        "managedScaling": {
                            "status": "ENABLED",
                            "targetCapacity": 75,
                            "minimumScalingStepSize": 1,
                            "maximumScalingStepSize": 10,
                            "instanceWarmupPeriod": 60,
                        },
                        "managedTerminationProtection": "DISABLED",
                        "managedDraining": "ENABLED",
                    },
                })
            },
            repeat: || {
                json!({
                    "autoScalingGroupProvider": {
                        "autoScalingGroupArn": group_arn("asg-r2"),
                        "managedScaling": {
                            "status": "ENABLED",
                            "targetCapacity": 75,
                            "minimumScalingStepSize": 1,
                            "maximumScalingStepSize": 10,
                            "instanceWarmupPeriod": 60,
                        },
                        "managedTerminationProtection": "DISABLED",
                        "managedDraining": "ENABLED",
                    },
                })
            },
            idempotent: true,
        },
        RepeatCase {
            name: "the same managed-instances configuration in the same cluster",
            create: || json!({ "cluster": "capacity", "managedInstancesProvider": managed_instances() }),
            repeat: || json!({ "cluster": "capacity", "managedInstancesProvider": managed_instances() }),
            idempotent: true,
        },
        RepeatCase {
            name: "the same binding with other tags, which are not what it binds onto",
            create: || {
                json!({
                    "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-r4") },
                    "tags": [{ "key": "team", "value": "platform" }],
                })
            },
            repeat: || {
                json!({
                    "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-r4") },
                    "tags": [{ "key": "team", "value": "other" }],
                })
            },
            idempotent: true,
        },
        RepeatCase {
            name: "a different Auto Scaling group under the name already taken",
            create: || json!({ "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-r5") } }),
            repeat: || json!({ "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-other5") } }),
            idempotent: false,
        },
        RepeatCase {
            name: "the same group with managed scaling turned on by the repeat",
            create: || json!({ "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-r6") } }),
            repeat: || {
                json!({
                    "autoScalingGroupProvider": {
                        "autoScalingGroupArn": group_arn("asg-r6"),
                        "managedScaling": { "status": "ENABLED", "targetCapacity": 100 },
                    },
                })
            },
            idempotent: false,
        },
        RepeatCase {
            name: "managed instances under a name an Auto Scaling group holds",
            create: || json!({ "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-r7") } }),
            repeat: || json!({ "cluster": "capacity", "managedInstancesProvider": managed_instances() }),
            idempotent: false,
        },
        RepeatCase {
            name: "the same managed-instances configuration scoped to another cluster",
            create: || json!({ "cluster": "capacity", "managedInstancesProvider": managed_instances() }),
            repeat: || json!({ "cluster": "elsewhere", "managedInstancesProvider": managed_instances() }),
            idempotent: false,
        },
    ];

    for (index, test) in cases.iter().enumerate() {
        let case = test.name;
        let provider_name = format!("repeat{index}");

        let mut create = (test.create)();
        create["name"] = json!(provider_name);
        let created = ok(&router, "CreateCapacityProvider", create, case).await;
        let created = created["capacityProvider"].clone();
        let stored = describe(&router, &provider_name, true, case).await;

        let mut repeat = (test.repeat)();
        repeat["name"] = json!(provider_name);
        if test.idempotent {
            let again = ok(&router, "CreateCapacityProvider", repeat, case).await;
            assert_eq!(
                again["capacityProvider"], created,
                "case {case:?}: a repeat of the same create reports the stored provider \
                 unchanged, got {again}"
            );
        } else {
            let (code, message) = refusal(&router, "CreateCapacityProvider", repeat, case).await;
            assert_eq!(
                code, "ClientException",
                "case {case:?}: expected a ClientException, got {code:?} with {message:?}"
            );
            assert_eq!(
                message,
                "The specified capacity provider already exists. To change the \
                 configuration of an existing capacity provider, update the capacity \
                 provider.",
                "case {case:?}: unexpected refusal message"
            );
        }

        // Either way the stored record is the one the FIRST create wrote: an idempotent
        // repeat writes nothing, and a refused one changes nothing.
        assert_eq!(
            describe(&router, &provider_name, true, case).await,
            stored,
            "case {case:?}: the stored provider must be untouched by the repeat"
        );
    }

    // The name is what is repeated: the same binding under a DIFFERENT name is a second
    // provider on one Auto Scaling group, which is still refused.
    let case = "the same group under another name";
    let (code, message) = refusal(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "renamed",
            "autoScalingGroupProvider": { "autoScalingGroupArn": group_arn("asg-r0") },
        }),
        case,
    )
    .await;
    assert_eq!(
        code, "ClientException",
        "case {case:?}: expected a ClientException, got {code:?} with {message:?}"
    );
    assert_eq!(
        message,
        "The specified Auto Scaling group ARN is already being used by another capacity \
         provider. Specify a unique Auto Scaling group ARN and try again.",
        "case {case:?}: a repeat under another name is a second provider on one group"
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
        let group = format!("asg-{name}");
        create_auto_scaling_group(&router, &group, "setup").await;
        ok(
            &router,
            "CreateCapacityProvider",
            json!({
                "name": name,
                "autoScalingGroupProvider": { "autoScalingGroupArn": group },
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
        // The providers Amazon ECS manages for the account are walked with the rest.
        expected.push("FARGATE".to_string());
        expected.push("FARGATE_SPOT".to_string());
        expected.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every provider exactly once in name order"
        );
    }

    // "This value can be between 1 and 10. If this parameter is not used, then
    // DescribeCapacityProviders returns up to 10 results": a page size below the range asks
    // for no page at all and is refused, while one above it is served the page this read
    // serves.
    refused(
        &router,
        "DescribeCapacityProviders",
        json!({ "maxResults": 0 }),
        "InvalidParameterException",
        "zero",
    )
    .await;
    for (case, max_results) in [("eleven", 11), ("a hundred", 100)] {
        let body = ok(
            &router,
            "DescribeCapacityProviders",
            json!({ "maxResults": max_results }),
            case,
        )
        .await;
        assert_eq!(
            list(&body, "capacityProviders", case).len(),
            10,
            "case {case:?}: a page above the range is served the page this read serves, got \
             {body}"
        );
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
            "arn": "ghost",
            "reason": "The specified capacity provider does not exist. Specify a valid name \
                       or ARN and try again.",
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
