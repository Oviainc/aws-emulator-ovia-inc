// SPDX-License-Identifier: CC0-1.0

//! Container reservations: what a container definition's numeric resource members may
//! name, and what a registration answers when one of them names something else.
//!
//! The rule under test, asserted over a table that spans the class rather than over one
//! request: a container definition's numeric resource reservations are COUNTS of a
//! resource — `cpu` is "The number of `cpu` units reserved for the container", `memory`
//! "The amount (in MiB) of memory to present to the container" and `memoryReservation`
//! "The soft limit (in MiB) of memory to reserve for the container" — so a registration
//! whose definition names a value the unit does not admit (a count of CPU units below
//! none, an amount of memory at or below none) is refused with the modelled
//! invalid-parameter error naming that member and that container, and nothing of the
//! registration reaches state: no revision is minted and the family's revision counter
//! does not move.
//!
//! Both registrations that carry container definitions are driven: the plain
//! `RegisterTaskDefinition` and the daemon `RegisterDaemonTaskDefinition`.
//!
//! Every case drives the emulator through the shared wire harness in [`crate::ecs_wire`],
//! the same front door a client uses, so nothing reaches into the state struct and a
//! response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{count, list, ok, refusal, service, string, ACCOUNT, SIGNED_REGION};

/// The smallest number of CPU units a container may reserve, from this crate's own
/// statement of the `cpu` member's range.
const MIN_CPU_UNITS: i32 =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MIN_CONTAINER_CPU_UNITS;

/// The smallest amount of memory, in MiB, a container may name.
const MIN_MEMORY_MIB: i32 =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MIN_CONTAINER_MEMORY_MIB;

/// The image every container here starts from; irrelevant to the rule, so one value.
const IMAGE: &str = "public.ecr.aws/amazonlinux/amazonlinux:latest";

/// The largest CPU share the `cpu` member's own doc names as valid to the Linux kernel
/// ("the maximum valid CPU share value that the Linux kernel allows is 262144"), used as
/// the far end of the accepted range.
const MAX_DOCUMENTED_CPU_SHARE: i32 = 262_144;

/// A task-level `memory`, carried by every plain registration here so that the
/// container-level "memory or memoryReservation is required" rule is satisfied
/// independently of the reservation values a row is about.
const TASK_MEMORY: &str = "512";

/// One row of the out-of-range table: the reservation members a container definition
/// carries, the member that is out of range and so the one the refusal must name, and the
/// container's own name, which the refusal must quote.
struct OutOfRange {
    case: &'static str,
    name: &'static str,
    member: &'static str,
    reservations: Value,
}

/// The container definition a row describes, as the wire carries it.
fn container(name: &str, reservations: &Value) -> Value {
    let mut definition = json!({ "name": name, "image": IMAGE });
    for (member, value) in reservations.as_object().expect("an object of members") {
        definition[member] = value.clone();
    }
    definition
}

/// The rows: every numeric reservation member, each just past its bound and well past it,
/// on container names that differ in length and in the characters they use — the recorded
/// case is one row of this table.
fn out_of_range_rows() -> Vec<OutOfRange> {
    vec![
        OutOfRange {
            case: "a CPU count one below the smallest one",
            name: "agent",
            member: "cpu",
            reservations: json!({ "cpu": MIN_CPU_UNITS - 1, "memory": 128 }),
        },
        OutOfRange {
            case: "a CPU count far below the smallest one",
            name: "log-router",
            member: "cpu",
            reservations: json!({ "cpu": -1024, "memory": 128 }),
        },
        OutOfRange {
            case: "the smallest CPU count an int32 can spell",
            name: "a",
            member: "cpu",
            reservations: json!({ "cpu": i32::MIN, "memory": 128 }),
        },
        OutOfRange {
            case: "a negative CPU count with no memory beside it",
            name: "metrics_agent_0",
            member: "cpu",
            reservations: json!({ "cpu": -1 }),
        },
        OutOfRange {
            case: "a memory amount one below the smallest one",
            name: "Sidecar",
            member: "memory",
            reservations: json!({ "memory": MIN_MEMORY_MIB - 1 }),
        },
        OutOfRange {
            case: "a negative memory amount",
            name: "fluent-bit",
            member: "memory",
            reservations: json!({ "cpu": 128, "memory": -128 }),
        },
        OutOfRange {
            case: "a soft memory limit one below the smallest amount",
            name: "Proxy1",
            member: "memoryReservation",
            reservations: json!({ "memoryReservation": MIN_MEMORY_MIB - 1 }),
        },
        OutOfRange {
            case: "a negative soft memory limit beside a valid memory amount",
            name: "x-ray-daemon",
            member: "memoryReservation",
            reservations: json!({ "memory": 256, "memoryReservation": -64 }),
        },
    ]
}

/// The refusal a container definition out of range earns: the member and the container's
/// own name, and nothing of the value or the bound.
fn expected_message(member: &str, name: &str) -> String {
    format!("Invalid '{member}' setting for container '{name}'.")
}

/// A family name no other row uses, derived from the row's position so a refused
/// registration can be shown to have stored nothing under it.
fn family(prefix: &str, index: usize) -> String {
    format!("{prefix}-reservation-{index}")
}

#[tokio::test]
async fn out_of_range_container_reservations_are_refused_by_both_registrations() {
    let router = service();

    for (index, row) in out_of_range_rows().into_iter().enumerate() {
        let definition = container(row.name, &row.reservations);
        let expected = expected_message(row.member, row.name);

        // The daemon registration: its recorded refusal for a reservation out of range is
        // the modelled invalid-parameter error.
        let daemon_family = family("daemon", index);
        let (code, message) = refusal(
            &router,
            "RegisterDaemonTaskDefinition",
            json!({
                "family": daemon_family,
                "containerDefinitions": [definition.clone()],
            }),
            row.case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {:?}: expected RegisterDaemonTaskDefinition to refuse with an \
             InvalidParameterException, got {code} with {message:?}",
            row.case
        );
        assert_eq!(
            message, expected,
            "case {:?}: expected the refusal to read {expected:?}, got {message:?}",
            row.case
        );

        // The plain registration: the same range and the same wording, reported in the
        // class this operation gives its container-definition refusals.
        let plain_family = family("plain", index);
        let (code, message) = refusal(
            &router,
            "RegisterTaskDefinition",
            json!({
                "family": plain_family,
                "memory": TASK_MEMORY,
                "containerDefinitions": [definition.clone()],
            }),
            row.case,
        )
        .await;
        assert_eq!(
            code, "ClientException",
            "case {:?}: expected RegisterTaskDefinition to refuse with a ClientException, \
             got {code} with {message:?}",
            row.case
        );
        assert_eq!(
            message, expected,
            "case {:?}: expected the refusal to read {expected:?}, got {message:?}",
            row.case
        );

        // A refused registration minted nothing, so neither family resolves to a
        // revision.
        for (operation, member, refused_family) in [
            (
                "DescribeDaemonTaskDefinition",
                "daemonTaskDefinition",
                &daemon_family,
            ),
            ("DescribeTaskDefinition", "taskDefinition", &plain_family),
        ] {
            let (code, message) = refusal(
                &router,
                operation,
                json!({ member: refused_family }),
                row.case,
            )
            .await;
            assert_eq!(
                code, "ClientException",
                "case {:?}: a refused registration stored nothing, so {operation} finds \
                 no revision of {refused_family:?}; got {code} with {message:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn the_offending_container_is_the_one_the_refusal_names() {
    let router = service();

    // The list is walked in order, so the refusal names whichever definition carries the
    // out-of-range value — not the first one in the request.
    let request = json!({
        "family": "daemon-second-container",
        "containerDefinitions": [
            container("first", &json!({ "cpu": 256, "memory": 128 })),
            container("second", &json!({ "cpu": -1, "memory": 128 })),
        ],
    });
    let case = "the second of two containers out of range";
    let (code, message) = refusal(&router, "RegisterDaemonTaskDefinition", request, case).await;
    let expected = expected_message("cpu", "second");
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected an InvalidParameterException, got {code} with {message:?}"
    );
    assert_eq!(
        message, expected,
        "case {case:?}: expected the refusal to read {expected:?}, got {message:?}"
    );

    // A definition whose FIRST member out of range is the CPU count is reported as a
    // `cpu` refusal even when a memory amount is out of range too: the members are
    // examined in one fixed order.
    let request = json!({
        "family": "daemon-two-members",
        "containerDefinitions": [container("both", &json!({ "cpu": -1, "memory": 0 }))],
    });
    let case = "a container with both a negative CPU count and no memory amount";
    let (code, message) = refusal(&router, "RegisterDaemonTaskDefinition", request, case).await;
    let expected = expected_message("cpu", "both");
    assert_eq!(
        code, "InvalidParameterException",
        "case {case:?}: expected an InvalidParameterException, got {code} with {message:?}"
    );
    assert_eq!(
        message, expected,
        "case {case:?}: expected the refusal to read {expected:?}, got {message:?}"
    );
}

/// One row of the accepted table: reservations inside the range, including each bound
/// itself, and the values a read of the stored revision must report.
struct InRange {
    case: &'static str,
    reservations: Value,
    expected_cpu: i64,
    expected_memory: i64,
}

fn in_range_rows() -> Vec<InRange> {
    vec![
        InRange {
            case: "no CPU count at all, so the member's modelled default",
            reservations: json!({ "memory": 128 }),
            expected_cpu: 0,
            expected_memory: 128,
        },
        InRange {
            case: "both bounds themselves: no CPU units and one MiB",
            reservations: json!({ "cpu": MIN_CPU_UNITS, "memory": MIN_MEMORY_MIB }),
            expected_cpu: i64::from(MIN_CPU_UNITS),
            expected_memory: i64::from(MIN_MEMORY_MIB),
        },
        InRange {
            case: "the smallest CPU share the kernel itself honours",
            reservations: json!({ "cpu": 2, "memory": 6 }),
            expected_cpu: 2,
            expected_memory: 6,
        },
        InRange {
            case: "the largest documented CPU share",
            reservations: json!({ "cpu": MAX_DOCUMENTED_CPU_SHARE, "memory": 1024 }),
            expected_cpu: i64::from(MAX_DOCUMENTED_CPU_SHARE),
            expected_memory: 1024,
        },
    ]
}

/// The one container of a stored revision, read back through the reader of that resource.
async fn stored_container(
    router: &MultiServiceRouter,
    operation: &str,
    member: &str,
    structure: &str,
    reference: &str,
    case: &str,
) -> Value {
    let body = ok(router, operation, json!({ member: reference }), case).await;
    let definitions = list(&body[structure], "containerDefinitions", case);
    assert_eq!(
        definitions.len(),
        1,
        "case {case:?}: expected the stored revision to describe one container, got \
         {definitions:?}"
    );
    definitions[0].clone()
}

#[tokio::test]
async fn in_range_container_reservations_are_stored_and_read_back() {
    let router = service();

    for (index, row) in in_range_rows().into_iter().enumerate() {
        let definition = container("agent", &row.reservations);

        let daemon_family = family("daemon-ok", index);
        let registered = ok(
            &router,
            "RegisterDaemonTaskDefinition",
            json!({
                "family": daemon_family,
                "containerDefinitions": [definition.clone()],
            }),
            row.case,
        )
        .await;
        let arn = string(&registered, "daemonTaskDefinitionArn", row.case);
        assert_eq!(
            arn,
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:daemon-task-definition/{daemon_family}:1"
            ),
            "case {:?}: expected the first revision of the family, got {arn:?}",
            row.case
        );
        let stored = stored_container(
            &router,
            "DescribeDaemonTaskDefinition",
            "daemonTaskDefinition",
            "daemonTaskDefinition",
            &daemon_family,
            row.case,
        )
        .await;
        assert_eq!(
            count(&stored, "cpu", row.case),
            row.expected_cpu,
            "case {:?}: expected the stored daemon container to reserve {} CPU units, got \
             {stored}",
            row.case,
            row.expected_cpu
        );
        assert_eq!(
            count(&stored, "memory", row.case),
            row.expected_memory,
            "case {:?}: expected the stored daemon container to carry {} MiB, got {stored}",
            row.case,
            row.expected_memory
        );

        let plain_family = family("plain-ok", index);
        ok(
            &router,
            "RegisterTaskDefinition",
            json!({
                "family": plain_family,
                "memory": TASK_MEMORY,
                "containerDefinitions": [definition.clone()],
            }),
            row.case,
        )
        .await;
        let stored = stored_container(
            &router,
            "DescribeTaskDefinition",
            "taskDefinition",
            "taskDefinition",
            &plain_family,
            row.case,
        )
        .await;
        assert_eq!(
            count(&stored, "cpu", row.case),
            row.expected_cpu,
            "case {:?}: expected the stored container to reserve {} CPU units, got {stored}",
            row.case,
            row.expected_cpu
        );
        assert_eq!(
            count(&stored, "memory", row.case),
            row.expected_memory,
            "case {:?}: expected the stored container to carry {} MiB, got {stored}",
            row.case,
            row.expected_memory
        );
    }
}

#[tokio::test]
async fn a_refused_registration_does_not_consume_a_revision() {
    let router = service();

    // Every refused registration names the SAME family, so if one of them moved the
    // family's revision counter the registration that follows them could not be revision
    // 1.
    let family = "daemon-revision-counter";
    for row in out_of_range_rows() {
        let definition = container(row.name, &row.reservations);
        let (code, _) = refusal(
            &router,
            "RegisterDaemonTaskDefinition",
            json!({
                "family": family,
                "containerDefinitions": [definition],
            }),
            row.case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {:?}: expected the registration to be refused, got {code}",
            row.case
        );
    }

    let case = "the first registration the family accepts";
    let body = ok(
        &router,
        "RegisterDaemonTaskDefinition",
        json!({
            "family": family,
            "containerDefinitions": [container("agent", &json!({ "memory": 128 }))],
        }),
        case,
    )
    .await;
    let arn = string(&body, "daemonTaskDefinitionArn", case);
    assert_eq!(
        arn,
        format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:daemon-task-definition/{family}:1"),
        "case {case:?}: the refused registrations consumed no revision, so the first \
         accepted one is revision 1; got {arn:?}"
    );
}
