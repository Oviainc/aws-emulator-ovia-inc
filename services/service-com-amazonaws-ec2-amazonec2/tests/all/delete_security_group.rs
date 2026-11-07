// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two `DeleteSecurityGroup` rules the lifecycle suite leaves
//! untested: WHAT MAKES A GROUP UNDELETABLE, and what a `DryRun` of the delete answers.
//!
//! "If you attempt to delete a security group that is associated with an instance or
//! network interface, is referenced by another security group in the same VPC, or has a VPC
//! association, the operation fails with DependencyViolation" (`DeleteSecurityGroup`) — so
//! the delete removes the group only while NOTHING holds it, and an instance holds the
//! groups it was launched with until it has reached `terminated`. Beside that, `DryRun`
//! "checks whether you have the required permissions for the action, without actually
//! making the request" (`DeleteSecurityGroupRequest$DryRun`): the probe reports the outcome
//! a permitted caller would have got — `DryRunOperation` for a request that would have
//! succeeded, the request's own refusal for one that would not — and deletes nothing
//! either way.
//!
//! The rows of each table are distinct inputs along what those rules generalise over: how
//! many instances hold the group, WHICH instance state holds it (`pending`, `running`,
//! `shutting-down`, `terminated`), whether the instance holds this group or another one of
//! the same VPC, and which refusal a dry run has to report instead of its own. The
//! transport is the crate's shared [`super::ec2_query`] harness, the ec2Query wire the
//! service actually speaks.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for: the Region the default VPC is seeded for.
const REGION: &str = "us-east-1";

/// An AMI id a launch names. `RunInstances` only has to accept it — this suite asserts
/// nothing about the image, so the id the instance suite uses serves here too.
const AMI: &str = "ami-0123456789abcdef0";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span rather than a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    member(case, &body, "vpcId")
}

/// A security group of `vpc_id` named `group_name`, as its id.
async fn create_security_group(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    group_name: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSecurityGroup",
        &[
            param("VpcId", vpc_id),
            param("GroupName", group_name),
            param("GroupDescription", "a group the delete suite created"),
        ],
    )
    .await;
    member(case, &body, "groupId")
}

/// A `/24` inside the VPC block `vpc_cidr`, which is the subnet a launch of that VPC is
/// placed in.
fn subnet_cidr_in(vpc_cidr: &str) -> String {
    let mut octets = vpc_cidr.split('.');
    let first = octets.next().unwrap_or("10");
    let second = octets.next().unwrap_or("0");
    format!("{first}.{second}.1.0/24")
}

/// A subnet of `vpc_id` holding `cidr_block`, as its id.
///
/// A launch has to be placed in a subnet OF THE VPC its security groups belong to — a group
/// belongs to one VPC, and `RunInstances` refuses a group of another — so a suite that
/// launches with a group of a VPC it created gives the launch that VPC's subnet.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[param("VpcId", vpc_id), param("CidrBlock", cidr_block)],
    )
    .await;
    member(case, &body, "subnetId")
}

/// Launch `count` instances holding the security group `group_id`, in `subnet_id`, as their
/// instance ids.
async fn run_instances(
    case: &str,
    router: &MultiServiceRouter,
    group_id: &str,
    subnet_id: &str,
    count: u32,
) -> Vec<String> {
    let body = ok(
        case,
        router,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("MinCount", &count.to_string()),
            param("MaxCount", &count.to_string()),
            param("SubnetId", subnet_id),
            param("SecurityGroupId.1", group_id),
        ],
    )
    .await;
    let launched = elements(&body, "instanceId");
    assert_eq!(
        launched.len(),
        count as usize,
        "case {case:?}: expected {count} launched instances, got {launched:?}"
    );
    launched
}

/// The `instanceState/name` `DescribeInstances` reports for `instance_id`. Read through the
/// document rather than by finding a tag: an instance carries a `<name>` inside its
/// `instanceState` and inside each of its security groups.
async fn instance_state(case: &str, router: &MultiServiceRouter, instance_id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id]),
    )
    .await;
    let parsed = document(case, &body);
    let instance = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "instanceState").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no instance with a state in {body}"));
    child(instance, "instanceState")
        .and_then(|state| child_text(state, "name"))
        .unwrap_or_else(|| panic!("case {case:?}: no instanceState/name in {body}"))
}

/// Whether `DescribeSecurityGroups` still reports the group `group_id`: the store's answer
/// to whether the delete happened, read back through the implemented describe rather than
/// inferred from the delete's own response.
async fn group_is_stored(case: &str, router: &MultiServiceRouter, group_id: &str) -> bool {
    let (status, body) = call(router, "DescribeSecurityGroups", &indexed("GroupId", &[group_id])).await;
    match status {
        200 => true,
        400 => {
            assert_eq!(
                error_code(&body),
                "InvalidGroup.NotFound",
                "case {case:?}: a group that is gone must be refused with \
                 InvalidGroup.NotFound, got {body}"
            );
            false
        }
        other => panic!("case {case:?}: unexpected HTTP {other} from the describe: {body}"),
    }
}

/// The security group ids `DescribeSecurityGroups` reports for `vpc_id`, read off each
/// group's `securityGroupArn` the way the lifecycle suite does: a `<groupId>` element also
/// appears inside a rule that references another group, while the ARN is one per reported
/// group and ends in that group's id.
async fn groups_of(case: &str, router: &MultiServiceRouter, vpc_id: &str) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeSecurityGroups",
        &filters(&[("vpc-id", &[vpc_id])]),
    )
    .await;
    elements(&body, "securityGroupArn")
        .iter()
        .filter_map(|arn| arn.rsplit('/').next().map(str::to_string))
        .collect()
}

/// What a launch does with the group under test, and how far the clock is let run.
struct Holder {
    /// How many instances the launch mints.
    instances: u32,
    /// Whether those instances were launched with the group under test, or with another
    /// group of the same VPC (which must not make this one undeletable).
    holds_the_group: bool,
    /// Whether the launch is given its `running` move before the delete is attempted.
    settle_launch: bool,
    /// Whether the instances are terminated before the delete is attempted.
    terminate: bool,
    /// Whether the termination is given its `terminated` move before the delete is
    /// attempted.
    settle_termination: bool,
    /// The instance states the row's assertions accept — one value where the clock has
    /// settled, and both sides of a move that may or may not have fallen due where it has
    /// deliberately not.
    states: &'static [&'static str],
}

/// The terminal instance state: the one state that holds nothing any more.
const TERMINATED: &str = "terminated";

/// The code a delete of a held group is refused with.
const DEPENDENCY_VIOLATION: &str = "DependencyViolation";

/// What the delete owes the row.
enum Expected {
    /// The row's instances hold the group in a state that is not terminal whichever side of
    /// a pending move they are on, so the refusal is pinned.
    Refused,
    /// Nothing holds the group, so the delete is pinned to succeed.
    Deleted,
    /// The row deliberately leaves a `terminated` move pending, so whichever outcome the
    /// delete gives is checked against the instance state for consistency — refused only
    /// while an instance that has not reached `terminated` holds the group — which is the
    /// rule itself rather than a timing a test may assume.
    FollowsTheState,
}

/// One row of the dependency table.
struct DependencyRow {
    case: &'static str,
    /// The VPC the row's group and instances live in — one per row, so no row can see
    /// another's resources.
    vpc_cidr: &'static str,
    holder: Holder,
    expected: Expected,
}

/// RULE: `DeleteSecurityGroup` removes the group only while nothing holds it — an instance
/// that has not reached `terminated` holds every group it was launched with, so the delete
/// is refused with `DependencyViolation` and the group stays stored; once nothing holds it,
/// the same delete removes it.
///
/// The rows differ in how many instances hold the group, in which state holds it, and in
/// whether the instance holds this group at all.
#[tokio::test]
async fn delete_security_group_refuses_while_an_unterminated_instance_holds_the_group() {
    let router = emulator();
    let rows = [
        DependencyRow {
            case: "one instance that has not left pending",
            vpc_cidr: "10.60.0.0/16",
            holder: Holder {
                instances: 1,
                holds_the_group: true,
                settle_launch: false,
                terminate: false,
                settle_termination: false,
                // The launch's `running` move may or may not have fallen due by the time
                // the delete lands; neither state is terminal, so both refuse.
                states: &["pending", "running"],
            },
            expected: Expected::Refused,
        },
        DependencyRow {
            case: "one running instance",
            vpc_cidr: "10.61.0.0/16",
            holder: Holder {
                instances: 1,
                holds_the_group: true,
                settle_launch: true,
                terminate: false,
                settle_termination: false,
                states: &["running"],
            },
            expected: Expected::Refused,
        },
        DependencyRow {
            case: "three running instances",
            vpc_cidr: "10.62.0.0/16",
            holder: Holder {
                instances: 3,
                holds_the_group: true,
                settle_launch: true,
                terminate: false,
                settle_termination: false,
                states: &["running"],
            },
            expected: Expected::Refused,
        },
        DependencyRow {
            case: "an instance still shutting down",
            vpc_cidr: "10.63.0.0/16",
            holder: Holder {
                instances: 1,
                holds_the_group: true,
                settle_launch: true,
                terminate: true,
                settle_termination: false,
                // The `terminated` move is deliberately not due yet; an instance on its way
                // out still holds its groups.
                states: &["shutting-down", "terminated"],
            },
            expected: Expected::FollowsTheState,
        },
        DependencyRow {
            case: "an instance that has reached terminated",
            vpc_cidr: "10.64.0.0/16",
            holder: Holder {
                instances: 1,
                holds_the_group: true,
                settle_launch: true,
                terminate: true,
                settle_termination: true,
                states: &["terminated"],
            },
            expected: Expected::Deleted,
        },
        DependencyRow {
            case: "a running instance holding another group of the same VPC",
            vpc_cidr: "10.65.0.0/16",
            holder: Holder {
                instances: 2,
                holds_the_group: false,
                settle_launch: true,
                terminate: false,
                settle_termination: false,
                states: &["running"],
            },
            expected: Expected::Deleted,
        },
    ];

    for row in rows {
        let case = row.case;
        let vpc_id = create_vpc(case, &router, row.vpc_cidr).await;
        let group_id = create_security_group(case, &router, &vpc_id, "delete-dependencies").await;
        let launched_group = if row.holder.holds_the_group {
            group_id.clone()
        } else {
            create_security_group(case, &router, &vpc_id, "another-group").await
        };

        let instances = run_instances(
            case,
            &router,
            &launched_group,
            &create_subnet(case, &router, &vpc_id, &subnet_cidr_in(row.vpc_cidr)).await,
            row.holder.instances,
        )
        .await;
        if row.holder.settle_launch {
            past_the_transition().await;
        }
        if row.holder.terminate {
            let named: Vec<&str> = instances.iter().map(String::as_str).collect();
            ok(
                case,
                &router,
                "TerminateInstances",
                &indexed("InstanceId", &named),
            )
            .await;
            if row.holder.settle_termination {
                past_the_transition().await;
            }
        }

        // The row's premise: the instances really are in the state the row says they are.
        let mut observed: Vec<String> = Vec::new();
        for instance_id in &instances {
            let state = instance_state(case, &router, instance_id).await;
            assert!(
                row.holder.states.contains(&state.as_str()),
                "case {case:?}: expected {instance_id} in one of {:?}, got {state:?}",
                row.holder.states
            );
            observed.push(state);
        }
        let (status, body) =
            call(&router, "DeleteSecurityGroup", &[param("GroupId", &group_id)]).await;
        let refused = match status {
            200 => false,
            400 => true,
            other => panic!("case {case:?}: unexpected HTTP {other} from the delete: {body}"),
        };
        match row.expected {
            Expected::Refused => assert!(
                refused,
                "case {case:?}: a group held by {observed:?} must be refused, got {status} \
                 with body {body}"
            ),
            Expected::Deleted => assert!(
                !refused,
                "case {case:?}: a group held by nothing ({observed:?}) must be deletable, \
                 got {status} with body {body}"
            ),
            // An instance state only ever advances towards `terminated`, so the two
            // outcomes are checked against the reading that cannot have moved under them:
            // a refusal means the call saw a holder that had not terminated, so the state
            // read BEFORE it cannot have been terminal either; a success means the call saw
            // no holder at all, so every instance must read `terminated` NOW. Neither side
            // assumes where inside `TRANSITION_DELAY` the delete landed.
            Expected::FollowsTheState => {
                if refused {
                    assert!(
                        observed.iter().any(|state| state != TERMINATED),
                        "case {case:?}: the delete was refused, so the states read before it \
                         cannot all be {TERMINATED:?}, got {observed:?}"
                    );
                } else {
                    for instance_id in &instances {
                        let state = instance_state(case, &router, instance_id).await;
                        assert_eq!(
                            state, TERMINATED,
                            "case {case:?}: the delete succeeded, so {instance_id} must have \
                             reached {TERMINATED:?}, got {state:?}"
                        );
                    }
                }
            }
        }

        if refused {
            let actual = error_code(&body);
            assert_eq!(
                actual, DEPENDENCY_VIOLATION,
                "case {case:?}: expected {DEPENDENCY_VIOLATION}, got {actual:?} in body {body}"
            );
            assert!(
                group_is_stored(case, &router, &group_id).await,
                "case {case:?}: a refused delete must leave {group_id} stored"
            );
        } else {
            let reported = member(case, &body, "return");
            assert_eq!(
                reported, "true",
                "case {case:?}: expected the delete to report true, got {reported:?}"
            );
            let reported_id = member(case, &body, "groupId");
            assert_eq!(
                reported_id, group_id,
                "case {case:?}: expected the deleted group {group_id:?}, got {reported_id:?}"
            );
            assert!(
                !group_is_stored(case, &router, &group_id).await,
                "case {case:?}: {group_id} must be gone after the delete"
            );
        }
    }
}

/// Which group a dry-run row names.
enum Target {
    /// A group of the row's VPC that nothing holds.
    Free,
    /// A group a running instance holds.
    Held,
    /// The default group the VPC came with.
    Default,
    /// An id no group of the account has.
    Unknown,
}

/// One row of the dry-run table.
struct DryRunRow {
    case: &'static str,
    vpc_cidr: &'static str,
    target: Target,
    status: u16,
    code: &'static str,
}

/// RULE: a `DryRun` delete performs nothing and reports the outcome a permitted caller
/// would have got — `DryRunOperation` (HTTP 412) when the delete would have succeeded, and
/// the delete's OWN refusal when it would have been refused — leaving every group stored
/// either way.
///
/// The rows differ in which group the probe names, and so in which answer the probe owes:
/// one nothing holds, one an instance holds, the VPC's default group, and an id no group
/// has.
#[tokio::test]
async fn delete_security_group_dry_run_reports_the_outcome_and_deletes_nothing() {
    let router = emulator();
    let rows = [
        DryRunRow {
            case: "a group nothing holds",
            vpc_cidr: "10.70.0.0/16",
            target: Target::Free,
            status: 412,
            code: "DryRunOperation",
        },
        DryRunRow {
            case: "a group a running instance holds",
            vpc_cidr: "10.71.0.0/16",
            target: Target::Held,
            status: 400,
            code: "DependencyViolation",
        },
        DryRunRow {
            case: "the default group of its VPC",
            vpc_cidr: "10.72.0.0/16",
            target: Target::Default,
            status: 400,
            code: "CannotDelete",
        },
        DryRunRow {
            case: "an id no group has",
            vpc_cidr: "10.73.0.0/16",
            target: Target::Unknown,
            status: 400,
            code: "InvalidGroup.NotFound",
        },
    ];

    for row in rows {
        let case = row.case;
        let vpc_id = create_vpc(case, &router, row.vpc_cidr).await;
        let group_id = match row.target {
            Target::Free => create_security_group(case, &router, &vpc_id, "dry-run-free").await,
            Target::Held => {
                let held =
                    create_security_group(case, &router, &vpc_id, "dry-run-held").await;
                let subnet_id =
                    create_subnet(case, &router, &vpc_id, &subnet_cidr_in(row.vpc_cidr)).await;
                run_instances(case, &router, &held, &subnet_id, 1).await;
                past_the_transition().await;
                held
            }
            Target::Default => groups_of(case, &router, &vpc_id)
                .await
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: the VPC must hold its default group"))
                .clone(),
            Target::Unknown => "sg-00000000000000000".to_string(),
        };

        let (status, body) = call(
            &router,
            "DeleteSecurityGroup",
            &[param("GroupId", &group_id), param("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.status
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, row.code,
            "case {case:?}: expected {}, got {actual:?} in body {body}",
            row.code
        );

        // Whatever the probe answered, the group it named is still there — an unknown id
        // being the one row where there was never a group to keep.
        let stored = group_is_stored(case, &router, &group_id).await;
        match row.target {
            Target::Unknown => assert!(
                !stored,
                "case {case:?}: {group_id} must still name no group after the probe"
            ),
            _ => assert!(
                stored,
                "case {case:?}: a dry run must leave {group_id} stored"
            ),
        }
    }

    // And the probe does not spend the delete: a fresh group that is dry-run first is
    // still deletable for real afterwards.
    let case = "the real delete after a dry run";
    let vpc_id = create_vpc(case, &router, "10.74.0.0/16").await;
    let group_id = create_security_group(case, &router, &vpc_id, "dry-run-then-delete").await;
    let (status, body) = call(
        &router,
        "DeleteSecurityGroup",
        &[param("GroupId", &group_id), param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412 from the probe, got {status} with body {body}"
    );
    let body = ok(
        case,
        &router,
        "DeleteSecurityGroup",
        &[param("GroupId", &group_id)],
    )
    .await;
    let reported = member(case, &body, "return");
    assert_eq!(
        reported, "true",
        "case {case:?}: expected the delete to report true, got {reported:?}"
    );
    assert!(
        !group_is_stored(case, &router, &group_id).await,
        "case {case:?}: {group_id} must be gone after the real delete"
    );
}

/// RULE: a delete names its group by `GroupId` or by `GroupName` — "you can specify either
/// the security group name or the security group ID"
/// (`DeleteSecurityGroupRequest$GroupName`), a NAME naming a group of the DEFAULT VPC only
/// ("for security groups in a nondefault VPC, you must specify the security group ID") —
/// so a request naming neither is a `MissingParameter`, an id no group of the account is
/// held under is `InvalidGroup.NotFound` whether or not a name accompanies it, and the two
/// members together have to name the SAME group.
///
/// The rows differ along which members the request carries and how they disagree; the last
/// one is the request that resolves, deleting a default-VPC group by name alone.
#[tokio::test]
async fn a_delete_names_its_group_by_id_or_by_name() {
    let router = emulator();

    // One group of the DEFAULT VPC, which is the VPC a name resolves in: created without a
    // `VpcId`, the way a caller that names no VPC creates one.
    let case = "the seed";
    let body = ok(
        case,
        &router,
        "CreateSecurityGroup",
        &[
            param("GroupName", "named-delete"),
            param("GroupDescription", "a group the delete suite names by name"),
        ],
    )
    .await;
    let group_id = member(case, &body, "groupId");

    // An id no group of the account has, in the long id form so it is well formed.
    const UNKNOWN_ID: &str = "sg-0fedcba987654321f";

    let refusals: [(&str, Vec<Param>, u16, &str); 4] = [
        (
            "neither member",
            Vec::new(),
            400,
            "MissingParameter",
        ),
        (
            "an unknown id on its own",
            vec![param("GroupId", UNKNOWN_ID)],
            400,
            "InvalidGroup.NotFound",
        ),
        (
            "an unknown id beside a name a group does answer to",
            vec![param("GroupId", UNKNOWN_ID), param("GroupName", "named-delete")],
            400,
            "InvalidGroup.NotFound",
        ),
        (
            "a held id beside a name that group is not called",
            vec![param("GroupId", &group_id), param("GroupName", "another-name")],
            400,
            "InvalidParameterCombination",
        ),
    ];

    for (case, params, status, code) in refusals {
        let (actual_status, body) = call(&router, "DeleteSecurityGroup", &params).await;
        assert_eq!(
            actual_status, status,
            "case {case:?}: expected HTTP {status}, got {actual_status} with body {body}"
        );
        let actual = error_code(&body);
        assert_eq!(
            actual, code,
            "case {case:?}: expected {code}, got {actual:?} in body {body}"
        );
        assert!(
            group_is_stored(case, &router, &group_id).await,
            "case {case:?}: a refused delete must leave {group_id} stored"
        );
    }

    // The request that resolves: the name alone, which the default VPC's group answers to,
    // and the delete reports the id that name resolved to.
    let case = "the name of a default-VPC group";
    let body = ok(
        case,
        &router,
        "DeleteSecurityGroup",
        &[param("GroupName", "named-delete")],
    )
    .await;
    let reported = member(case, &body, "groupId");
    assert_eq!(
        reported, group_id,
        "case {case:?}: expected the delete to report {group_id}, got {reported:?}"
    );
    assert!(
        !group_is_stored(case, &router, &group_id).await,
        "case {case:?}: {group_id} must be gone after the delete by name"
    );
}
