// SPDX-License-Identifier: CC0-1.0

//! `SetInstanceProtection` over the awsQuery wire.
//!
//! The rule under test: an instance-scoped update of an Auto Scaling group is ONE unit
//! of work over the instances THAT GROUP holds — every instance id the request names
//! must be one the group reports, so a call naming an id of another group, or of no
//! group at all, updates NONE of them and is refused `ValidationError` naming the
//! instances that are not part of the group, while a call whose every id belongs to the
//! group writes the setting on exactly those instances and leaves the rest as they
//! were. Past the operation's own documented per-call bound on instance ids the call is
//! refused with the modelled `LimitExceededFault` instead, and a group the account does
//! not hold is refused like every other group-scoped write of this service.
//!
//! The tables below span what that rule generalises over — how many instances the group
//! holds, how many the request names, where in the request a foreign id sits, whether
//! that id belongs to another group or to no instance at all, whether an id repeats,
//! which value is written and whether it replaces an earlier one, and the documented
//! per-call bound — and every row reads the instances back through
//! `DescribeAutoScalingGroups` (the implemented reader of a group's instances), so no
//! row can pass on an accepted-but-unstored change. One row goes further and scales the
//! group in, because what the stored flag MEANS is that the instance is not chosen for
//! termination.

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{borrowed, new_engine, p, query, router_with, tag_value, tag_values,
                     xml_unescape};

/// The launch source every group here is created from — a launch TEMPLATE, because no
/// account can create a launch configuration (see
/// `launch_configurations_are_unavailable.rs`).
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// The zone every group here places into, so each group has somewhere to launch the
/// instances this suite protects.
const ZONE: (&str, &str) = ("AvailabilityZones.member.1", "us-west-2a");

/// The most instance ids ONE call may name, from the model's own documentation for
/// `SetInstanceProtection`: "If you exceed your maximum limit of instance IDs, which is
/// 50 per Auto Scaling group, the call fails."
const MAX_INSTANCE_IDS_PER_CALL: usize = 50;

/// What a group reports for one of its instances.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedInstance {
    instance_id: String,
    protected_from_scale_in: bool,
    lifecycle_state: String,
}

/// Create a group holding `capacity` instances and answer the instance ids it reports,
/// in the order the group holds them.
async fn create_group(router: &MultiServiceRouter, name: &str, capacity: usize) -> Vec<String> {
    let capacity = capacity.to_string();
    let params = vec![
        p("AutoScalingGroupName", name),
        p(LAUNCH_SOURCE.0, LAUNCH_SOURCE.1),
        p("MinSize", "0"),
        p("MaxSize", capacity.as_str()),
        p("DesiredCapacity", capacity.as_str()),
        p(ZONE.0, ZONE.1),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "creating {name:?} with capacity {capacity} must succeed, got {status} with {body:?}"
    );
    let reported = reported_instances(router, name).await;
    assert_eq!(
        reported.len(),
        capacity.parse::<usize>().expect("a capacity is a count"),
        "the group {name:?} must hold the capacity it was created with, got {reported:?}"
    );
    reported
        .into_iter()
        .map(|instance| instance.instance_id)
        .collect()
}

/// The instances the group reports through the implemented reader of its own
/// `Instances`, in the order it holds them.
async fn reported_instances(router: &MultiServiceRouter, name: &str) -> Vec<ReportedInstance> {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the read-back of {name:?} must succeed, got {status} with {body:?}"
    );
    // The instance members live inside the group's own `Instances` element; the
    // response carries other `<member>` lists (the zones, the termination policies), so
    // the slice is narrowed to that element first.
    let Some((_, rest)) = body.split_once("<Instances>") else {
        return Vec::new();
    };
    let inner = rest
        .split_once("</Instances>")
        .map_or(rest, |(inner, _)| inner);
    tag_values(inner, "member")
        .iter()
        .map(|member| ReportedInstance {
            instance_id: tag_value(member, "InstanceId"),
            protected_from_scale_in: tag_value(member, "ProtectedFromScaleIn") == "true",
            lifecycle_state: tag_value(member, "LifecycleState"),
        })
        .collect()
}

/// Post one `SetInstanceProtection` and answer its status, wire error code and message.
async fn set_protection(
    router: &MultiServiceRouter,
    name: &str,
    instance_ids: &[String],
    protected: bool,
) -> (u16, String, String) {
    let mut params = vec![
        p("AutoScalingGroupName", name),
        p(
            "ProtectedFromScaleIn",
            if protected { "true" } else { "false" },
        ),
    ];
    for (index, instance_id) in instance_ids.iter().enumerate() {
        params.push(p(
            format!("InstanceIds.member.{}", index + 1).as_str(),
            instance_id.clone(),
        ));
    }
    let (status, body) = query(router, "SetInstanceProtection", &borrowed(&params)).await;
    (
        status,
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
    )
}

/// The message Amazon EC2 Auto Scaling refuses an instance-scoped update naming
/// instances the group does not hold with: fixed text plus the ids from the request that
/// are not part of the group, in request order.
fn not_in_group_message(instance_ids: &[String]) -> String {
    format!(
        "Trying to update Instances that are not part of the group: {}",
        instance_ids.join(", ")
    )
}

/// One accepted row: the case, how many instances the group holds, whether every
/// instance is protected first, the 1-based positions the request names (in request
/// order) and the value it writes.
struct AcceptedRow {
    case: &'static str,
    held: usize,
    protect_all_first: bool,
    named: &'static [usize],
    protected: bool,
}

/// Every row: the named instances carry the value the call wrote and only they changed,
/// read back off the group itself.
#[tokio::test]
async fn a_call_sets_the_protection_of_exactly_the_instances_it_names() {
    let rows = [
        AcceptedRow {
            case: "the only instance of the group, protected",
            held: 1,
            protect_all_first: false,
            named: &[1],
            protected: true,
        },
        AcceptedRow {
            case: "the middle instance of three, protected",
            held: 3,
            protect_all_first: false,
            named: &[2],
            protected: true,
        },
        AcceptedRow {
            case: "every instance of three at once, protected",
            held: 3,
            protect_all_first: false,
            named: &[1, 2, 3],
            protected: true,
        },
        AcceptedRow {
            case: "two of three named in reverse launch order, protected",
            held: 3,
            protect_all_first: false,
            named: &[3, 1],
            protected: true,
        },
        AcceptedRow {
            case: "the same instance named twice, protected",
            held: 2,
            protect_all_first: false,
            named: &[1, 1],
            protected: true,
        },
        AcceptedRow {
            case: "protection removed from one of three that had it",
            held: 3,
            protect_all_first: true,
            named: &[2],
            protected: false,
        },
        AcceptedRow {
            case: "protection removed from every instance that had it",
            held: 2,
            protect_all_first: true,
            named: &[1, 2],
            protected: false,
        },
        AcceptedRow {
            case: "protection written again on an instance that already has it",
            held: 2,
            protect_all_first: true,
            named: &[1],
            protected: true,
        },
        AcceptedRow {
            case: "protection cleared on an instance that never had it",
            held: 2,
            protect_all_first: false,
            named: &[2],
            protected: false,
        },
    ];

    let router = router_with(new_engine());

    for (row_index, row) in rows.iter().enumerate() {
        let name = format!("asg-accepted-{row_index}");
        let held = create_group(&router, &name, row.held).await;

        // A fresh group protects none of its instances
        // (`NewInstancesProtectedFromScaleIn` defaults to false), so the starting point
        // is read off the group rather than assumed.
        let before = reported_instances(&router, &name).await;
        assert!(
            before.iter().all(|instance| !instance.protected_from_scale_in),
            "case {:?}: a fresh group protects none of its instances, got {before:?}",
            row.case
        );

        if row.protect_all_first {
            let (status, code, message) = set_protection(&router, &name, &held, true).await;
            assert_eq!(
                status, 200,
                "case {:?}: protecting every instance first must succeed, got {status} with \
                 {code:?} {message:?}",
                row.case
            );
        }

        let requested: Vec<String> = row
            .named
            .iter()
            .map(|position| held[*position - 1].clone())
            .collect();
        let (status, code, message) =
            set_protection(&router, &name, &requested, row.protected).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {code:?} {message:?}",
            row.case
        );

        let actual = reported_instances(&router, &name).await;
        assert_eq!(
            actual.len(),
            held.len(),
            "case {:?}: the call changes no instance of the group, got {actual:?}",
            row.case
        );
        for instance in &actual {
            let expected = if requested.contains(&instance.instance_id) {
                row.protected
            } else {
                row.protect_all_first
            };
            assert_eq!(
                instance.protected_from_scale_in, expected,
                "case {:?}: expected {} protected from scale in: {expected}, got {instance:?}",
                row.case, instance.instance_id
            );
        }
    }
}

/// Where a foreign instance id comes from.
#[derive(Debug, Clone, Copy)]
enum Foreign {
    /// An id of the same account's OTHER group.
    OtherGroup(usize),
    /// A well-formed instance id no instance of the account has.
    Unknown(&'static str),
}

/// One refused row: the case, how many instances the group holds, the request's ids (an
/// `Ok` position names an instance of the group, an `Err` one a foreign id) and the ids
/// the refusal must name, in request order.
struct RefusedRow {
    case: &'static str,
    held: usize,
    named: &'static [Result<usize, Foreign>],
    refused: &'static [Foreign],
}

/// Every row: HTTP 400, `ValidationError`, the refusal naming the instances that are not
/// part of the group, and NOTHING written — every instance of the group is still
/// unprotected, and the other group's instance is too, which is what makes the refusal
/// whole.
#[tokio::test]
async fn a_call_naming_an_instance_the_group_does_not_hold_is_refused_whole() {
    let rows = [
        RefusedRow {
            case: "an id no instance of the account has, alone",
            held: 2,
            named: &[Err(Foreign::Unknown("i-0fff0000000000001"))],
            refused: &[Foreign::Unknown("i-0fff0000000000001")],
        },
        RefusedRow {
            case: "an instance of another group of the account, alone",
            held: 2,
            named: &[Err(Foreign::OtherGroup(1))],
            refused: &[Foreign::OtherGroup(1)],
        },
        RefusedRow {
            case: "an instance of the group followed by one of another group",
            held: 2,
            named: &[Ok(1), Err(Foreign::OtherGroup(2))],
            refused: &[Foreign::OtherGroup(2)],
        },
        RefusedRow {
            case: "a foreign id ahead of an instance of the group",
            held: 2,
            named: &[Err(Foreign::Unknown("i-0fff0000000000002")), Ok(2)],
            refused: &[Foreign::Unknown("i-0fff0000000000002")],
        },
        RefusedRow {
            case: "two foreign ids mixed between two instances of the group",
            held: 2,
            named: &[
                Ok(1),
                Err(Foreign::OtherGroup(1)),
                Ok(2),
                Err(Foreign::Unknown("i-0fff0000000000003")),
            ],
            refused: &[
                Foreign::OtherGroup(1),
                Foreign::Unknown("i-0fff0000000000003"),
            ],
        },
        RefusedRow {
            case: "the same foreign id named twice",
            held: 1,
            named: &[
                Err(Foreign::Unknown("i-0fff0000000000004")),
                Err(Foreign::Unknown("i-0fff0000000000004")),
            ],
            refused: &[
                Foreign::Unknown("i-0fff0000000000004"),
                Foreign::Unknown("i-0fff0000000000004"),
            ],
        },
        RefusedRow {
            case: "an instance of another group, for a group that holds none",
            held: 0,
            named: &[Err(Foreign::OtherGroup(1))],
            refused: &[Foreign::OtherGroup(1)],
        },
    ];

    for (row_index, row) in rows.iter().enumerate() {
        // One emulator per row: the other group's instances are the foreign ids of this
        // row alone.
        let router = router_with(new_engine());
        let other = create_group(&router, "asg-other", 2).await;
        let name = format!("asg-refused-{row_index}");
        let held = create_group(&router, &name, row.held).await;

        let id_of = |foreign: &Foreign| match foreign {
            Foreign::OtherGroup(position) => other[*position - 1].clone(),
            Foreign::Unknown(id) => (*id).to_string(),
        };
        let requested: Vec<String> = row
            .named
            .iter()
            .map(|entry| match entry {
                Ok(position) => held[*position - 1].clone(),
                Err(foreign) => id_of(foreign),
            })
            .collect();
        let refused: Vec<String> = row.refused.iter().map(id_of).collect();

        let (status, code, message) = set_protection(&router, &name, &requested, true).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {code:?} {message:?}",
            row.case
        );
        assert_eq!(
            code, "ValidationError",
            "case {:?}: expected the ValidationError code, got {code:?} {message:?}",
            row.case
        );
        let expected = not_in_group_message(&refused);
        assert_eq!(
            message, expected,
            "case {:?}: expected the message {expected:?}, got {message:?}",
            row.case
        );

        for (group, instances) in [(name.as_str(), &held), ("asg-other", &other)] {
            let actual = reported_instances(&router, group).await;
            assert_eq!(
                actual.len(),
                instances.len(),
                "case {:?}: a refused call leaves {group:?} holding its instances, got {actual:?}",
                row.case
            );
            assert!(
                actual.iter().all(|instance| !instance.protected_from_scale_in),
                "case {:?}: a refused call protects no instance of {group:?}, got {actual:?}",
                row.case
            );
        }
    }
}

/// The documented per-call bound: a call naming the maximum is served, and one naming a
/// single instance id more is refused with the operation's modelled `LimitExceededFault`
/// — before anything is written, so the group is untouched.
#[tokio::test]
async fn a_call_past_the_documented_per_call_maximum_is_refused() {
    let router = router_with(new_engine());
    let held = create_group(&router, "asg-bound", MAX_INSTANCE_IDS_PER_CALL).await;

    // One past the bound: the group's own instances plus one more id.
    let mut past_bound = held.clone();
    past_bound.push("i-0fff0000000000009".to_string());
    let (status, code, message) = set_protection(&router, "asg-bound", &past_bound, true).await;
    assert_eq!(
        status, 400,
        "a call naming {} instance ids must be refused, got {status} with {code:?} {message:?}",
        past_bound.len()
    );
    // `LimitExceededFault` carries the awsQuery error code `LimitExceeded`
    // (smithy_query(query="operation",operation="SetInstanceProtection")).
    assert_eq!(
        code, "LimitExceeded",
        "expected the LimitExceeded code past the bound, got {code:?} {message:?}"
    );
    let expected =
        format!("Trying to update too many Instances at once. The limit is {MAX_INSTANCE_IDS_PER_CALL}");
    assert_eq!(
        message, expected,
        "expected the message {expected:?} past the bound, got {message:?}"
    );
    let actual = reported_instances(&router, "asg-bound").await;
    assert!(
        actual.iter().all(|instance| !instance.protected_from_scale_in),
        "a call refused past the bound protects nothing, got {actual:?}"
    );

    // AT the bound: served, and every instance the call named carries the setting.
    let (status, code, message) = set_protection(&router, "asg-bound", &held, true).await;
    assert_eq!(
        status, 200,
        "a call naming {MAX_INSTANCE_IDS_PER_CALL} instance ids must succeed, got {status} with \
         {code:?} {message:?}"
    );
    let actual = reported_instances(&router, "asg-bound").await;
    assert!(
        actual.iter().all(|instance| instance.protected_from_scale_in),
        "expected every instance of the group protected, got {actual:?}"
    );
}

/// A call naming a group the account does not hold is refused like every other
/// group-scoped WRITE of this service: the fixed sentence whose detail field is
/// unfilled, so no part of the request appears in it.
#[tokio::test]
async fn a_call_against_an_absent_group_is_refused() {
    let router = router_with(new_engine());
    let held = create_group(&router, "asg-present", 1).await;

    for absent in ["asg-absent", "ASG-Present", "asg-present-but-longer"] {
        let (status, code, message) = set_protection(&router, absent, &held, true).await;
        assert_eq!(
            status, 400,
            "case {absent:?}: expected HTTP 400, got {status} with {code:?} {message:?}"
        );
        assert_eq!(
            code, "ValidationError",
            "case {absent:?}: expected the ValidationError code, got {code:?} {message:?}"
        );
        assert_eq!(
            message, "AutoScalingGroup name not found - null",
            "case {absent:?}: expected the no-detail absent-group sentence, got {message:?}"
        );
    }

    let actual = reported_instances(&router, "asg-present").await;
    assert!(
        actual.iter().all(|instance| !instance.protected_from_scale_in),
        "the refusals above wrote nothing on the group that does exist, got {actual:?}"
    );
}

/// What the stored flag MEANS: an instance protected from scale-in is not chosen when
/// the group scales in, and one whose protection was cleared is chosen again.
#[tokio::test]
async fn a_protected_instance_is_not_chosen_by_a_scale_in() {
    let router = router_with(new_engine());
    let held = create_group(&router, "asg-scale-in", 2).await;
    // The most recently launched instance is the one a scale-in would take first, so
    // protecting it is what makes the assertion about the protection and not the order.
    let protected = held[1].clone();
    let unprotected = held[0].clone();

    let (status, code, message) =
        set_protection(&router, "asg-scale-in", std::slice::from_ref(&protected), true).await;
    assert_eq!(
        status, 200,
        "protecting {protected} must succeed, got {status} with {code:?} {message:?}"
    );

    let (status, body) = query(
        &router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", "asg-scale-in"),
            ("DesiredCapacity", "1"),
        ],
    )
    .await;
    assert_eq!(status, 200, "scaling in expected 200, got {status}: {body}");

    let actual = reported_instances(&router, "asg-scale-in").await;
    let protected_state = actual
        .iter()
        .find(|instance| instance.instance_id == protected)
        .unwrap_or_else(|| panic!("the group must still report {protected}, got {actual:?}"));
    assert_ne!(
        protected_state.lifecycle_state, "Terminating",
        "a protected instance is not chosen by a scale-in, got {protected_state:?}"
    );
    let unprotected_state = actual
        .iter()
        .find(|instance| instance.instance_id == unprotected)
        .unwrap_or_else(|| panic!("the group must still report {unprotected}, got {actual:?}"));
    assert_eq!(
        unprotected_state.lifecycle_state, "Terminating",
        "the unprotected instance is the one the scale-in takes, got {unprotected_state:?}"
    );
}
