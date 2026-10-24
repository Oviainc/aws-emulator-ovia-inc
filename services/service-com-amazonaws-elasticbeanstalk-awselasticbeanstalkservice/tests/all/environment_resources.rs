// SPDX-License-Identifier: CC0-1.0

//! `DescribeEnvironmentResources`: the Amazon Web Services resources an
//! environment is made of, driven end to end over the service's awsQuery HTTP
//! surface.
//!
//! RULE: the operation reports the inventory the store holds for the named
//! environment — the resources its launch provisioned, which a rebuild replaces
//! and a completed termination leaves it with none of. Which KINDS of resource
//! an environment has follows what was launched: every live environment has one
//! Auto Scaling group, the Amazon EC2 launch template it launches from and one
//! instance per Auto Scaling minimum size; an environment that is load balanced
//! (`aws:elasticbeanstalk:environment` / `EnvironmentType`, whose documented
//! default is `LoadBalanced`) also has the triggers its group scales on, a
//! load-balanced WEB SERVER environment a load balancer, and a WORKER
//! environment the Amazon SQS queue its daemon reads.
//!
//! Each test is a TABLE over that class — the tier, the environment type, the
//! requested group size, which identifier names the environment — and every
//! assertion names its case and prints what the emulator actually answered.
//!
//! The harness is the one `lifecycle.rs` owns: the same `MultiServiceRouter` the
//! production binary mounts, spoken to with a form-urlencoded `Action=…` body.
//! Nothing here reaches into the store, so an inventory that is not persisted —
//! or one a second read would report differently — fails these tests.

use std::collections::BTreeSet;

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{account, Answer, Emulator, PAST_DUE, SOLUTION_STACK, TEST_REGION};

/// The application every environment below is launched under.
const APPLICATION: &str = "resources-app";

/// The `aws:elasticbeanstalk:environment` / `EnvironmentType` option, as a
/// `CreateEnvironment` option setting: namespace, option name and the
/// `SingleInstance` value ("launch one EC2 instance with no load balancer").
const SINGLE_INSTANCE: &[(&str, &str)] = &[
    (
        "OptionSettings.member.1.Namespace",
        "aws:elasticbeanstalk:environment",
    ),
    ("OptionSettings.member.1.OptionName", "EnvironmentType"),
    ("OptionSettings.member.1.Value", "SingleInstance"),
];

/// The same option carrying its documented DEFAULT value, stated explicitly, so
/// one row of the table asserts that naming it changes nothing.
const LOAD_BALANCED: &[(&str, &str)] = &[
    (
        "OptionSettings.member.1.Namespace",
        "aws:elasticbeanstalk:environment",
    ),
    ("OptionSettings.member.1.OptionName", "EnvironmentType"),
    ("OptionSettings.member.1.Value", "LoadBalanced"),
];

/// `aws:autoscaling:asg` / `MinSize` at 3 — "the minimum number of instances
/// you want in your Auto Scaling group".
const MIN_SIZE_3: &[(&str, &str)] = &[
    ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
    ("OptionSettings.member.1.OptionName", "MinSize"),
    ("OptionSettings.member.1.Value", "3"),
];

/// The same option below its own floor: an Auto Scaling group of no instances
/// is not an environment, so one instance is what such a request launches.
const MIN_SIZE_0: &[(&str, &str)] = &[
    ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
    ("OptionSettings.member.1.OptionName", "MinSize"),
    ("OptionSettings.member.1.Value", "0"),
];

/// The same option past the bound this store holds one environment's inventory
/// to ([`eb::state::MAX_ENVIRONMENT_INSTANCES`]).
const MIN_SIZE_HUGE: &[(&str, &str)] = &[
    ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
    ("OptionSettings.member.1.OptionName", "MinSize"),
    ("OptionSettings.member.1.Value", "10000"),
];

/// A single-instance environment that ALSO asks for a group of five: the
/// environment type is one instance, whatever the group size says.
const SINGLE_INSTANCE_WITH_MIN_SIZE_5: &[(&str, &str)] = &[
    (
        "OptionSettings.member.1.Namespace",
        "aws:elasticbeanstalk:environment",
    ),
    ("OptionSettings.member.1.OptionName", "EnvironmentType"),
    ("OptionSettings.member.1.Value", "SingleInstance"),
    ("OptionSettings.member.2.Namespace", "aws:autoscaling:asg"),
    ("OptionSettings.member.2.OptionName", "MinSize"),
    ("OptionSettings.member.2.Value", "5"),
];

/// The worker tier, by the two members that name it ("For Standard-mode
/// EC2-based backend application with Amazon SQS – `Worker`", "For Worker tier –
/// `SQS/HTTP`").
const WORKER_TIER: &[(&str, &str)] = &[("Tier.Name", "Worker"), ("Tier.Type", "SQS/HTTP")];

/// The web server tier, named explicitly rather than defaulted.
const WEB_SERVER_TIER: &[(&str, &str)] = &[("Tier.Name", "WebServer"), ("Tier.Type", "Standard")];

/// One environment's reported inventory, read out of the response envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Inventory {
    environment_name: String,
    auto_scaling_groups: Vec<String>,
    instances: Vec<String>,
    launch_configurations: Vec<String>,
    launch_templates: Vec<String>,
    load_balancers: Vec<String>,
    triggers: Vec<String>,
    /// Each queue as `(Name, URL)`, the two members `Queue` carries.
    queues: Vec<(String, String)>,
    /// Whether the response carried a `Cluster` with an ARN in it.
    cluster: Option<String>,
}

impl Inventory {
    fn of(answer: &Answer) -> Self {
        let values = |path: &str| {
            answer
                .values_at(path)
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<String>>()
        };
        Self {
            environment_name: answer
                .value("EnvironmentResources/EnvironmentName")
                .unwrap_or_default()
                .to_string(),
            auto_scaling_groups: values("EnvironmentResources/AutoScalingGroups/member/Name"),
            instances: values("EnvironmentResources/Instances/member/Id"),
            launch_configurations: values("EnvironmentResources/LaunchConfigurations/member/Name"),
            launch_templates: values("EnvironmentResources/LaunchTemplates/member/Id"),
            load_balancers: values("EnvironmentResources/LoadBalancers/member/Name"),
            triggers: values("EnvironmentResources/Triggers/member/Name"),
            queues: answer
                .elements_at("EnvironmentResources/Queues/member")
                .iter()
                .map(|queue| {
                    (
                        queue.value("Name").unwrap_or_default().to_string(),
                        queue.value("URL").unwrap_or_default().to_string(),
                    )
                })
                .collect(),
            cluster: answer
                .value("EnvironmentResources/Cluster/ClusterArn")
                .map(str::to_string),
        }
    }

    /// Every resource identifier in this inventory, as one set — what a rebuild
    /// has to replace in full.
    fn identifiers(&self) -> BTreeSet<String> {
        self.auto_scaling_groups
            .iter()
            .chain(self.instances.iter())
            .chain(self.launch_templates.iter())
            .chain(self.load_balancers.iter())
            .chain(self.triggers.iter())
            .chain(self.queues.iter().map(|(name, _)| name))
            .cloned()
            .collect()
    }

    /// How many resources of each kind this inventory holds, in the order the
    /// fields above declare them: two inventories of one environment agree on
    /// this even when a rebuild has replaced every resource in it.
    fn shape(&self) -> [usize; 6] {
        [
            self.auto_scaling_groups.len(),
            self.instances.len(),
            self.launch_templates.len(),
            self.load_balancers.len(),
            self.triggers.len(),
            self.queues.len(),
        ]
    }
}

/// Create [`APPLICATION`], the application the environments are launched under.
async fn application(emulator: &Emulator) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "fixture: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );
}

/// Launch `environment` with `extra` parameters, answering its `EnvironmentId`.
async fn launch(
    emulator: &Emulator,
    environment: &str,
    extra: &[(&str, &str)],
    case: &str,
) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("ApplicationName", APPLICATION),
        ("EnvironmentName", environment),
        ("SolutionStackName", SOLUTION_STACK.as_str()),
    ];
    params.extend_from_slice(extra);
    let launched = emulator.call("CreateEnvironment", &params).await;
    assert_eq!(
        launched.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        launched.status, launched.body
    );
    launched.expect("EnvironmentId", case).to_string()
}

/// The inventory the emulator reports for the environment named by `params`.
async fn described(emulator: &Emulator, params: &[(&str, &str)], case: &str) -> Inventory {
    let answer = emulator
        .call("DescribeEnvironmentResources", params)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from DescribeEnvironmentResources, got {} and body {}",
        answer.status, answer.body
    );
    Inventory::of(&answer)
}

/// The inventory reported for `environment`, named by its name.
async fn described_by_name(emulator: &Emulator, environment: &str, case: &str) -> Inventory {
    described(emulator, &[("EnvironmentName", environment)], case).await
}

/// Whether `id` is `<prefix>-<17 lowercase hexadecimal characters>`, the form
/// Amazon EC2 issues an instance id and a launch template id in.
fn is_ec2_resource_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(&format!("{prefix}-")).is_some_and(|body| {
        body.len() == eb::state::EC2_RESOURCE_ID_BODY_LENGTH
            && body.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

/// Assert that `name` is the physical name of the `logical_id` resource of the
/// stack `environment_id`'s resources live in: the stack name, the logical id
/// and then a suffix identifying that one resource.
fn assert_stack_resource(name: &str, environment_id: &str, logical_id: &str, case: &str) {
    let prefix = format!(
        "{}-{logical_id}-",
        eb::state::environment_stack_name(environment_id)
    );
    assert!(
        name.starts_with(&prefix),
        "case {case:?}: expected a {logical_id} named {prefix}…, got {name:?}"
    );
    assert!(
        name.len() > prefix.len(),
        "case {case:?}: expected a suffix identifying the {logical_id}, got {name:?}"
    );
}

/// What one row of a table below launches: the environment's name and the
/// parameter sets it is created with, under the label its assertions name.
struct Launch {
    case: &'static str,
    environment: &'static str,
    extra: &'static [&'static [(&'static str, &'static str)]],
}

/// One row of the inventory table: what was launched, and how many resources of
/// each kind that environment is then made of.
struct Case {
    case: &'static str,
    environment: &'static str,
    extra: &'static [&'static [(&'static str, &'static str)]],
    instances: usize,
    load_balancers: usize,
    triggers: usize,
    queues: usize,
}

/// RULE: the reported inventory follows the environment's tier and its
/// configuration options — the Auto Scaling group and launch template every
/// environment has, one instance per Auto Scaling minimum size, the scaling
/// triggers and load balancer a load-balanced web server has, and the queue a
/// worker has.
#[tokio::test]
async fn the_reported_inventory_follows_the_tier_and_the_options() {
    // The number of scaling triggers comes from the service's own list of them,
    // so this table cannot drift from what it provisions.
    let all_triggers = eb::state::STACK_RESOURCE_TRIGGERS.len();
    let cases = [
        Case {
            case: "web server, load balanced by default",
            environment: "res-default",
            extra: &[],
            instances: 1,
            load_balancers: 1,
            triggers: all_triggers,
            queues: 0,
        },
        Case {
            case: "web server and load balanced, both named explicitly",
            environment: "res-explicit",
            extra: &[WEB_SERVER_TIER, LOAD_BALANCED],
            instances: 1,
            load_balancers: 1,
            triggers: all_triggers,
            queues: 0,
        },
        Case {
            case: "web server, single instance",
            environment: "res-single",
            extra: &[SINGLE_INSTANCE],
            instances: 1,
            load_balancers: 0,
            triggers: 0,
            queues: 0,
        },
        Case {
            case: "worker tier, load balanced by default",
            environment: "res-worker",
            extra: &[WORKER_TIER],
            instances: 1,
            load_balancers: 0,
            triggers: all_triggers,
            queues: 1,
        },
        Case {
            case: "worker tier, single instance",
            environment: "res-worker-single",
            extra: &[WORKER_TIER, SINGLE_INSTANCE],
            instances: 1,
            load_balancers: 0,
            triggers: 0,
            queues: 1,
        },
        Case {
            case: "an Auto Scaling minimum of three",
            environment: "res-three",
            extra: &[MIN_SIZE_3],
            instances: 3,
            load_balancers: 1,
            triggers: all_triggers,
            queues: 0,
        },
        Case {
            case: "an Auto Scaling minimum below one instance",
            environment: "res-zero",
            extra: &[MIN_SIZE_0],
            instances: 1,
            load_balancers: 1,
            triggers: all_triggers,
            queues: 0,
        },
        Case {
            case: "an Auto Scaling minimum past the bound this store holds",
            environment: "res-huge",
            extra: &[MIN_SIZE_HUGE],
            instances: eb::state::MAX_ENVIRONMENT_INSTANCES,
            load_balancers: 1,
            triggers: all_triggers,
            queues: 0,
        },
        Case {
            case: "a single instance that also asks for a group of five",
            environment: "res-single-five",
            extra: &[SINGLE_INSTANCE_WITH_MIN_SIZE_5],
            instances: 1,
            load_balancers: 0,
            triggers: 0,
            queues: 0,
        },
    ];

    let emulator = Emulator::new();
    application(&emulator).await;

    for row in &cases {
        let case = row.case;
        let extra: Vec<(&str, &str)> = row.extra.iter().flat_map(|set| set.iter()).copied().collect();
        let environment_id = launch(&emulator, row.environment, &extra, case).await;
        let inventory = described_by_name(&emulator, row.environment, case).await;

        assert_eq!(
            inventory.environment_name, row.environment,
            "case {case:?}: the inventory must name the environment it is of, got {inventory:?}"
        );
        // Every environment has exactly one Auto Scaling group, named inside
        // the stack its resources live in.
        assert_eq!(
            inventory.auto_scaling_groups.len(),
            1,
            "case {case:?}: expected one Auto Scaling group, got {:?}",
            inventory.auto_scaling_groups
        );
        assert_stack_resource(
            &inventory.auto_scaling_groups[0],
            &environment_id,
            eb::state::STACK_RESOURCE_AUTO_SCALING_GROUP,
            case,
        );
        // One launch template, reported by its Amazon EC2 id; and no launch
        // CONFIGURATION, which Amazon EC2 Auto Scaling no longer creates.
        assert_eq!(
            inventory.launch_templates.len(),
            1,
            "case {case:?}: expected one launch template, got {:?}",
            inventory.launch_templates
        );
        assert!(
            is_ec2_resource_id(
                &inventory.launch_templates[0],
                eb::state::EC2_LAUNCH_TEMPLATE_ID_PREFIX
            ),
            "case {case:?}: expected a launch template id, got {:?}",
            inventory.launch_templates[0]
        );
        assert!(
            inventory.launch_configurations.is_empty(),
            "case {case:?}: expected no launch configuration, got {:?}",
            inventory.launch_configurations
        );

        assert_eq!(
            inventory.instances.len(),
            row.instances,
            "case {case:?}: expected {} instance(s), got {:?}",
            row.instances,
            inventory.instances
        );
        for instance in &inventory.instances {
            assert!(
                is_ec2_resource_id(instance, eb::state::EC2_INSTANCE_ID_PREFIX),
                "case {case:?}: expected an Amazon EC2 instance id, got {instance:?}"
            );
        }
        assert_eq!(
            inventory.instances.iter().collect::<BTreeSet<_>>().len(),
            row.instances,
            "case {case:?}: every instance is a distinct one, got {:?}",
            inventory.instances
        );

        assert_eq!(
            inventory.load_balancers.len(),
            row.load_balancers,
            "case {case:?}: expected {} load balancer(s), got {:?}",
            row.load_balancers,
            inventory.load_balancers
        );
        for load_balancer in &inventory.load_balancers {
            assert_stack_resource(
                load_balancer,
                &environment_id,
                eb::state::STACK_RESOURCE_LOAD_BALANCER,
                case,
            );
        }

        assert_eq!(
            inventory.triggers.len(),
            row.triggers,
            "case {case:?}: expected {} trigger(s), got {:?}",
            row.triggers,
            inventory.triggers
        );
        for (trigger, logical_id) in inventory
            .triggers
            .iter()
            .zip(eb::state::STACK_RESOURCE_TRIGGERS)
        {
            assert_stack_resource(trigger, &environment_id, logical_id, case);
        }

        assert_eq!(
            inventory.queues.len(),
            row.queues,
            "case {case:?}: expected {} queue(s), got {:?}",
            row.queues,
            inventory.queues
        );
        for (name, url) in &inventory.queues {
            assert_stack_resource(
                name,
                &environment_id,
                eb::state::STACK_RESOURCE_WORKER_QUEUE,
                case,
            );
            // An Amazon SQS queue's URL:
            // `https://sqs.<region>.amazonaws.com/<account>/<queue name>`, in
            // the region the request was signed for.
            assert_eq!(
                url,
                &format!(
                    "https://sqs.{TEST_REGION}.amazonaws.com/{}/{name}",
                    account()
                ),
                "case {case:?}: unexpected queue URL, got {url:?}"
            );
        }

        // "This member is present only for environments in the Cluster tier",
        // which this service publishes no platform for.
        assert_eq!(
            inventory.cluster, None,
            "case {case:?}: expected no Cluster member, got {:?}",
            inventory.cluster
        );
    }
}

/// RULE: reading the inventory changes nothing, and a rebuild — which "deletes
/// and recreates all of the Amazon Web Services resources for a specified
/// environment" — replaces every resource in it while leaving the KINDS the
/// environment is made of, and the stack they live in, as they were.
#[tokio::test]
async fn a_rebuild_replaces_every_resource_and_a_read_replaces_none() {
    // Rows differ along the kinds of resource the environment holds: a
    // load-balanced web server (load balancer, triggers, three instances) and a
    // worker (a queue instead of the load balancer).
    let rows = [
        Launch {
            case: "load-balanced web server",
            environment: "rebuild-web",
            extra: &[MIN_SIZE_3],
        },
        Launch {
            case: "worker",
            environment: "rebuild-worker",
            extra: &[WORKER_TIER],
        },
        Launch {
            case: "single instance",
            environment: "rebuild-single",
            extra: &[SINGLE_INSTANCE],
        },
    ];

    let emulator = Emulator::new();
    application(&emulator).await;

    for Launch {
        case,
        environment,
        extra,
    } in rows
    {
        let extra: Vec<(&str, &str)> = extra.iter().flat_map(|set| set.iter()).copied().collect();
        let environment_id = launch(&emulator, environment, &extra, case).await;
        // The launch the clock owes, so the environment is `Ready` — the one
        // status a rebuild is accepted from.
        std::thread::sleep(PAST_DUE);

        let before = described_by_name(&emulator, environment, case).await;
        let again = described_by_name(&emulator, environment, case).await;
        assert_eq!(
            before, again,
            "case {case:?}: a second read reports the same inventory, got {again:?}"
        );

        let rebuilt = emulator
            .call("RebuildEnvironment", &[("EnvironmentName", environment)])
            .await;
        assert_eq!(
            rebuilt.status, 200,
            "case {case:?}: expected 200 from RebuildEnvironment, got {} and body {}",
            rebuilt.status, rebuilt.body
        );

        let after = described_by_name(&emulator, environment, case).await;
        assert_eq!(
            after.shape(),
            before.shape(),
            "case {case:?}: a rebuild recreates the same kinds of resource, got {after:?} from {before:?}"
        );
        let kept: Vec<String> = after
            .identifiers()
            .intersection(&before.identifiers())
            .cloned()
            .collect();
        assert!(
            kept.is_empty(),
            "case {case:?}: a rebuild replaces every resource, but these survived it: {kept:?}"
        );
        // The environment itself is the same one, so its resources still live
        // in the stack its id names.
        assert_stack_resource(
            &after.auto_scaling_groups[0],
            &environment_id,
            eb::state::STACK_RESOURCE_AUTO_SCALING_GROUP,
            case,
        );

        // The rebuild's completion is a status change, not another rebuild: the
        // resources it created are the ones the environment is left with.
        std::thread::sleep(PAST_DUE);
        let settled = described_by_name(&emulator, environment, case).await;
        assert_eq!(
            settled, after,
            "case {case:?}: the completion the clock owed must not replace the resources again, got {settled:?}"
        );
    }
}

/// RULE: a termination leaves the environment with no resources — but only once
/// the clock has completed it; while the environment is still shutting down it
/// is made of what it was running on.
#[tokio::test]
async fn a_terminated_environment_is_made_of_nothing() {
    // Rows differ along the kinds of resource there are to delete.
    let rows = [
        Launch {
            case: "web server",
            environment: "terminate-web",
            extra: &[],
        },
        Launch {
            case: "worker",
            environment: "terminate-worker",
            extra: &[WORKER_TIER],
        },
    ];

    let emulator = Emulator::new();
    application(&emulator).await;

    for Launch {
        case,
        environment,
        extra,
    } in rows
    {
        let extra: Vec<(&str, &str)> = extra.iter().flat_map(|set| set.iter()).copied().collect();
        launch(&emulator, environment, &extra, case).await;
        std::thread::sleep(PAST_DUE);
        let running = described_by_name(&emulator, environment, case).await;
        assert!(
            !running.identifiers().is_empty(),
            "case {case:?}: a running environment is made of something, got {running:?}"
        );

        let terminated = emulator
            .call("TerminateEnvironment", &[("EnvironmentName", environment)])
            .await;
        assert_eq!(
            terminated.status, 200,
            "case {case:?}: expected 200 from TerminateEnvironment, got {} and body {}",
            terminated.status, terminated.body
        );
        let shutting_down = described_by_name(&emulator, environment, case).await;
        assert_eq!(
            shutting_down, running,
            "case {case:?}: an environment still shutting down reports what it runs on, got {shutting_down:?}"
        );

        std::thread::sleep(PAST_DUE);
        let gone = described_by_name(&emulator, environment, case).await;
        assert_eq!(
            gone.environment_name, environment,
            "case {case:?}: the inventory still names the environment, got {gone:?}"
        );
        assert_eq!(
            gone.shape(),
            [0; 6],
            "case {case:?}: a terminated environment is made of nothing, got {gone:?}"
        );
    }
}

/// RULE: the environment is selected by `EnvironmentId`, by `EnvironmentName` or
/// by both — "you must specify either this or an EnvironmentName, or both. If
/// you do not specify either, Elastic Beanstalk returns MissingRequiredParameter
/// error" — and an identifier that names no environment is refused rather than
/// answered with an empty inventory.
#[tokio::test]
async fn the_environment_is_named_by_id_by_name_or_by_both() {
    let emulator = Emulator::new();
    application(&emulator).await;
    let case = "selection";
    let environment = "select-env";
    let environment_id = launch(&emulator, environment, WORKER_TIER, case).await;

    let by_name = described_by_name(&emulator, environment, case).await;
    let by_id = described(
        &emulator,
        &[("EnvironmentId", environment_id.as_str())],
        case,
    )
    .await;
    let by_both = described(
        &emulator,
        &[
            ("EnvironmentId", environment_id.as_str()),
            ("EnvironmentName", environment),
        ],
        case,
    )
    .await;
    assert_eq!(
        by_id, by_name,
        "case {case:?}: naming the environment by id reports the same inventory, got {by_id:?}"
    );
    assert_eq!(
        by_both, by_name,
        "case {case:?}: naming it by both reports the same inventory, got {by_both:?}"
    );

    // Neither identifier: the condition the operation's own documentation names.
    let neither = emulator.call("DescribeEnvironmentResources", &[]).await;
    assert_eq!(
        neither.error_code(),
        "MissingRequiredParameter",
        "case \"neither identifier\": expected MissingRequiredParameter, got {}",
        neither.body
    );

    // An identifier that names nothing, by each of the two members, with the
    // value the caller sent quoted back in the refusal.
    let refusals: [(&str, &str, &str, &str); 2] = [
        (
            "unknown name",
            "EnvironmentName",
            "no-such-env",
            "No Environment found for EnvironmentName = 'no-such-env'.",
        ),
        (
            "unknown id",
            "EnvironmentId",
            "e-0000000000000",
            "No Environment found for EnvironmentId = 'e-0000000000000'.",
        ),
    ];
    for (row, member, value, message) in refusals {
        let refused = emulator
            .call("DescribeEnvironmentResources", &[(member, value)])
            .await;
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {row:?}: expected InvalidParameterValue, got {}",
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            message,
            "case {row:?}: unexpected refusal message in {}",
            refused.body
        );
    }
}
