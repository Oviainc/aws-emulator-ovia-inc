// SPDX-License-Identifier: CC0-1.0

//! How `UpdateContainerInstancesState` NAMES a container instance it could not move,
//! driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: the operation answers HTTP 200 and reports every entry it could
//! not resolve to an instance it can move through `failures` (`UpdateContainerInstancesState`
//! output members `containerInstances`/`failures`, each entry a `Failure` with
//! `arn`/`reason`/`detail`), with `reason` `MISSING` and `arn` the container-instance ARN
//! of the INSTANCE ID the entry named and nothing else:
//! `arn:aws:ecs:<region>:<account>:container-instance/<instance-id>`. The cluster segment
//! a stored instance's ARN carries names the cluster it is REGISTERED IN, which an
//! instance this call did not move is not, so it never appears in the resource part —
//! whichever of the member's two forms ("A list of up to 10 container instance IDs or full
//! ARN entries") the entry took.
//!
//! The rows vary the entry along what the rule generalises over: its FORM (bare id, an ARN
//! qualified by the cluster of the request, an ARN qualified by another cluster, an ARN
//! already carrying no cluster), the id's shape (the 32 hexadecimal digits and the
//! 36-character hyphenated form, the two lengths a container-instance id has), how many
//! entries are sent at once, whether a resolvable instance is sent beside them, and WHY
//! the entry resolves to nothing (nothing of that id at all, an instance registered into
//! another cluster, an instance of this cluster that was deregistered). Both statuses this
//! action governs are driven over the whole table. The recorded divergence — a
//! cluster-qualified ARN answered with the cluster still in it — is ONE row.
//!
//! The contrast the same suite pins: `DescribeContainerInstances` is a READ, and its
//! failure entry echoes the identifier the request sent, so the two operations answer the
//! same identifier differently.

use serde_json::json;

use crate::ecs_wire::{
    create_cluster, list, ok, register_instance, service, string, ACCOUNT, SIGNED_REGION,
};

/// The cluster every request below names.
const CLUSTER: &str = "drainingdemo";

/// Another cluster, for an entry that names an instance registered elsewhere.
const OTHER_CLUSTER: &str = "drainingelsewhere";

/// The `Failure$reason` for an identifier that names no instance this call can move, from
/// the API failure reasons: "MISSING — the described resource could not be found".
const MISSING: &str = "MISSING";

/// A container-instance ARN in the signed region and the caller's account, under the
/// resource path `path`.
fn container_instance_arn(path: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container-instance/{path}")
}

/// The ARN the rule requires a failure to name `instance_id` by: the id ALONE in the
/// resource part.
fn unqualified(instance_id: &str) -> String {
    container_instance_arn(instance_id)
}

/// The instance id a container-instance ARN ends in.
fn arn_id(arn: &str) -> &str {
    arn.rsplit('/')
        .next()
        .expect("a container-instance ARN ends in its id")
}

/// The `(arn, reason)` pair of every failure in a response, in order.
fn failures(body: &serde_json::Value, case: &str) -> Vec<(String, String)> {
    list(body, "failures", case)
        .iter()
        .map(|failure| {
            (
                string(failure, "arn", case),
                string(failure, "reason", case),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_state_change_names_an_unresolvable_instance_by_its_id_alone() {
    let router = service();
    create_cluster(&router, CLUSTER, "setup").await;
    create_cluster(&router, OTHER_CLUSTER, "setup").await;
    let stored = register_instance(&router, CLUSTER, "i-0a0a0a0a0a0a0a0a0", "setup").await;
    let elsewhere =
        register_instance(&router, OTHER_CLUSTER, "i-0b0b0b0b0b0b0b0b0", "setup").await;
    let elsewhere_id = arn_id(&elsewhere).to_string();

    // An instance of THIS cluster that is no longer registered in it: a deregistered one
    // "transitions to an INACTIVE status", so it is no instance of the cluster to move.
    let retired = register_instance(&router, CLUSTER, "i-0c0c0c0c0c0c0c0c0", "setup").await;
    ok(
        &router,
        "DeregisterContainerInstance",
        json!({ "cluster": CLUSTER, "containerInstance": retired.clone() }),
        "setup",
    )
    .await;
    let retired_id = arn_id(&retired).to_string();

    // The two lengths a container-instance id has: the 32 hexadecimal digits this store
    // mints and the 36 characters the hyphenated UUID form spells.
    let uuid_form = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string();
    let other_uuid_form = "11111111-2222-3333-4444-555555555555".to_string();

    // Rows: the case, the entries the request names, the failure ARNs expected back IN THE
    // REQUEST'S ORDER, and how many instances the call is expected to report as moved.
    let rows: Vec<(&str, Vec<String>, Vec<String>, usize)> = vec![
        (
            "a bare 32-hex id that names nothing",
            vec!["f".repeat(32)],
            vec![unqualified(&"f".repeat(32))],
            0,
        ),
        (
            "a full ARN qualified by the cluster of the request",
            vec![container_instance_arn(&format!(
                "{CLUSTER}/{}",
                "7".repeat(32)
            ))],
            vec![unqualified(&"7".repeat(32))],
            0,
        ),
        (
            "a full ARN qualified by another cluster",
            vec![container_instance_arn(&format!(
                "{OTHER_CLUSTER}/{}",
                "6".repeat(32)
            ))],
            vec![unqualified(&"6".repeat(32))],
            0,
        ),
        (
            "a full ARN already carrying no cluster",
            vec![unqualified(&"8".repeat(32))],
            vec![unqualified(&"8".repeat(32))],
            0,
        ),
        (
            "a bare id in the hyphenated uuid form",
            vec![uuid_form.clone()],
            vec![unqualified(&uuid_form)],
            0,
        ),
        (
            "the hyphenated uuid form inside a cluster-qualified ARN",
            vec![container_instance_arn(&format!(
                "{CLUSTER}/{other_uuid_form}"
            ))],
            vec![unqualified(&other_uuid_form)],
            0,
        ),
        (
            "three unknown entries at once, one per form",
            vec![
                "a".repeat(32),
                container_instance_arn(&format!("{CLUSTER}/{}", "b".repeat(32))),
                container_instance_arn(&format!("{OTHER_CLUSTER}/{}", "c".repeat(32))),
            ],
            vec![
                unqualified(&"a".repeat(32)),
                unqualified(&"b".repeat(32)),
                unqualified(&"c".repeat(32)),
            ],
            0,
        ),
        (
            "the registered instance beside an unknown cluster-qualified ARN",
            vec![
                stored.clone(),
                container_instance_arn(&format!("{CLUSTER}/{}", "d".repeat(32))),
            ],
            vec![unqualified(&"d".repeat(32))],
            1,
        ),
        (
            "an instance registered into another cluster, by its full ARN",
            vec![elsewhere.clone()],
            vec![unqualified(&elsewhere_id)],
            0,
        ),
        (
            "an instance registered into another cluster, by its bare id",
            vec![elsewhere_id.clone()],
            vec![unqualified(&elsewhere_id)],
            0,
        ),
        (
            "a deregistered instance of this cluster, by its own ARN",
            vec![retired.clone()],
            vec![unqualified(&retired_id)],
            0,
        ),
    ];

    // "The only valid values for this action are ACTIVE and DRAINING": the rule holds for
    // either state the call asks for.
    for status in ["DRAINING", "ACTIVE"] {
        for (row, named, expected, moved) in &rows {
            let case = format!("{row}, moving to {status}");
            let body = ok(
                &router,
                "UpdateContainerInstancesState",
                json!({
                    "cluster": CLUSTER,
                    "containerInstances": named,
                    "status": status,
                }),
                &case,
            )
            .await;
            let reported = failures(&body, &case);
            let want: Vec<(String, String)> = expected
                .iter()
                .map(|arn| (arn.clone(), MISSING.to_string()))
                .collect();
            assert_eq!(
                reported, want,
                "case {case:?}: expected the failures {want:?}, got {reported:?} in {body}"
            );
            // The resource part is the instance id and nothing else, so no cluster name
            // reaches it however the entry spelled the instance.
            for (arn, _) in &reported {
                assert!(
                    !arn.contains(CLUSTER) && !arn.contains(OTHER_CLUSTER),
                    "case {case:?}: expected no cluster segment in the failure ARN, got \
                     {arn:?}"
                );
            }
            assert_eq!(
                list(&body, "containerInstances", &case).len(),
                *moved,
                "case {case:?}: expected {moved} instance(s) reported as moved, got {body}"
            );
        }

        // The instance that IS registered here moved, and the move reached the store: the
        // failures above are about entries this call could not resolve, not about a write
        // that was skipped.
        let case = format!("the registered instance reads back {status}");
        let read = ok(
            &router,
            "DescribeContainerInstances",
            json!({ "cluster": CLUSTER, "containerInstances": [stored.clone()] }),
            &case,
        )
        .await;
        assert_eq!(
            string(&list(&read, "containerInstances", &case)[0], "status", &case),
            status,
            "case {case:?}: expected the stored instance to report {status}, got {read}"
        );
    }
}

#[tokio::test]
async fn the_read_echoes_the_identifier_the_state_change_normalises() {
    let router = service();
    create_cluster(&router, CLUSTER, "setup").await;
    create_cluster(&router, OTHER_CLUSTER, "setup").await;

    // Rows: the case, the entry, the ARN the STATE CHANGE names it by (the id alone) and
    // the ARN the READ echoes (the identifier as sent — the value itself when it is
    // already an ARN, otherwise the id alone).
    let qualified = container_instance_arn(&format!("{CLUSTER}/{}", "4".repeat(32)));
    let foreign = container_instance_arn(&format!("{OTHER_CLUSTER}/{}", "5".repeat(32)));
    let rows: Vec<(&str, String, String, String)> = vec![
        (
            "a cluster-qualified ARN",
            qualified.clone(),
            unqualified(&"4".repeat(32)),
            qualified.clone(),
        ),
        (
            "an ARN qualified by another cluster",
            foreign.clone(),
            unqualified(&"5".repeat(32)),
            foreign.clone(),
        ),
        (
            // A bare id carries no cluster to begin with, so both answer alike.
            "a bare id",
            "3".repeat(32),
            unqualified(&"3".repeat(32)),
            unqualified(&"3".repeat(32)),
        ),
    ];

    for (case, named, updated_arn, described_arn) in &rows {
        let update = ok(
            &router,
            "UpdateContainerInstancesState",
            json!({
                "cluster": CLUSTER,
                "containerInstances": [named],
                "status": "DRAINING",
            }),
            case,
        )
        .await;
        assert_eq!(
            failures(&update, case),
            vec![(updated_arn.clone(), MISSING.to_string())],
            "case {case:?}: expected the state change to name {updated_arn:?}, got {update}"
        );

        let describe = ok(
            &router,
            "DescribeContainerInstances",
            json!({ "cluster": CLUSTER, "containerInstances": [named] }),
            case,
        )
        .await;
        assert_eq!(
            failures(&describe, case),
            vec![(described_arn.clone(), MISSING.to_string())],
            "case {case:?}: expected the read to echo {described_arn:?}, got {describe}"
        );
    }
}
