// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DB (instance) parameter group DELETE path:
//! `DeleteDBParameterGroup` against what `CreateDBParameterGroup` and
//! `ModifyDBParameterGroup` persisted, and against what this service no longer holds.
//!
//! Every call goes over the wire, through the same mounting helper and awsQuery form POST
//! the sibling suites use, so the assertions read the XML the emulator itself answered
//! with.
//!
//! The rule under test is the option group suite's rule for the group kind this operation
//! owns: a group a create persisted is visible to every later call that looks it up — so
//! the delete of a group that exists is accepted, and the group, with every parameter a
//! caller set on it, is gone afterwards — while the modelled not-found fault is answered
//! only for a name nothing created or one a delete has already removed. The two documented
//! refusals in between are `DBParameterGroupName`'s own constraints: "Can't be associated
//! with any DB instances" and "You can't delete a default DB parameter group", each the
//! state fault rather than a not-found.
//!
//! The tables span the classes the rule generalises over: the shape and case of the
//! group's name, the engine family it was created for, whether parameters were written into
//! it before the delete, the case the instance that holds it spells its name in, and the
//! region the group was created in.

use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state::STATUS_TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, REGION};

/// A second region, so the regional scope of a delete is observable.
const OTHER_REGION: &str = "eu-west-1";

/// The `awsQueryError` code of `DBParameterGroupNotFoundFault`.
const NOT_FOUND: &str = "DBParameterGroupNotFound";

/// The `awsQueryError` code of `InvalidDBParameterGroupStateFault`.
const INVALID_STATE: &str = "InvalidDBParameterGroupState";

/// One row of the create/delete round trip.
struct DeletionCase {
    case: &'static str,
    /// The name the create asks for; the store keys it lowercased.
    requested_name: &'static str,
    family: &'static str,
    /// A parameter written through `ModifyDBParameterGroup` before the delete, if any: a
    /// group that carries settings is deleted the same way an empty one is.
    setting: Option<(&'static str, &'static str)>,
}

/// The groups the round trip creates, deletes and then looks up again.
///
/// The rows differ along what the rule generalises over: the name's case and length, the
/// engine family (one per parameter vocabulary this front door publishes a catalog for,
/// plus one it publishes none for), and whether the group carries a caller's parameter at
/// delete time.
const DELETION_CASES: &[DeletionCase] = &[
    DeletionCase {
        case: "an empty mysql group",
        requested_name: "emu-del-pg-mysql",
        family: "mysql8.0",
        setting: None,
    },
    DeletionCase {
        case: "a postgres group carrying a parameter a caller set",
        requested_name: "emu-del-pg-postgres",
        family: "postgres16",
        setting: Some(("autovacuum", "0")),
    },
    DeletionCase {
        case: "a MIXED-CASE name, stored lowercased, carrying a parameter",
        requested_name: "Emu-Del-PG-Aurora",
        family: "aurora-mysql8.0",
        setting: Some(("max_connections", "120")),
    },
    DeletionCase {
        case: "a one-character name on a family with no published catalog",
        requested_name: "d",
        family: "db2-se",
        setting: None,
    },
];

/// `CreateDBParameterGroup` with its three required members, in `region`.
async fn create_group(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    family: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateDBParameterGroup",
        &[
            ("DBParameterGroupName", name),
            ("DBParameterGroupFamily", family),
            ("Description", "deletion-rule-parameters"),
        ],
    )
    .await
}

/// `DeleteDBParameterGroup` naming one group, in [`REGION`].
async fn delete_group(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "DeleteDBParameterGroup",
        &[("DBParameterGroupName", name)],
    )
    .await
}

/// `CreateDBInstance` placed in the parameter group `group`.
async fn create_instance_in(
    router: &MultiServiceRouter,
    identifier: &str,
    group: &str,
) -> (u16, String) {
    call(
        router,
        "CreateDBInstance",
        &[
            ("DBInstanceIdentifier", identifier),
            ("Engine", "mysql"),
            ("DBInstanceClass", "db.m5.large"),
            ("DBParameterGroupName", group),
        ],
    )
    .await
}

/// Wait past the instant a status change this service accepted becomes due.
async fn wait_for_settle() {
    tokio::time::sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200)).await;
}

#[tokio::test]
async fn deleting_a_persisted_group_is_accepted_and_only_then_is_it_not_found() {
    let router = emulator();

    for row in DELETION_CASES {
        expect_ok(
            row.case,
            create_group(&router, REGION, row.requested_name, row.family).await,
        );
        if let Some((parameter, value)) = row.setting {
            expect_ok(
                row.case,
                call(
                    &router,
                    "ModifyDBParameterGroup",
                    &[
                        ("DBParameterGroupName", row.requested_name),
                        ("Parameters.Parameter.1.ParameterName", parameter),
                        ("Parameters.Parameter.1.ParameterValue", value),
                    ],
                )
                .await,
            );
        }
        // The group the create persisted: the delete of it is ACCEPTED, whatever the name's
        // case — a group that exists must never be answered with the not-found fault.
        let deleted = expect_ok(row.case, delete_group(&router, row.requested_name).await);
        assert!(
            deleted.contains("<DeleteDBParameterGroupResponse"),
            "case {:?}: expected the delete's own empty-output envelope, got {deleted}",
            row.case
        );
        // Only now is the name not found — by the group report, by the parameter report and
        // by a second delete alike, which is what lets a caller tell "deleted" from "never
        // existed", and says the parameters went with the group rather than outliving it.
        for action in [
            "DescribeDBParameterGroups",
            "DescribeDBParameters",
            "DeleteDBParameterGroup",
        ] {
            expect_error(
                row.case,
                call(
                    &router,
                    action,
                    &[("DBParameterGroupName", row.requested_name)],
                )
                .await,
                404,
                NOT_FOUND,
            );
        }
    }
}

#[tokio::test]
async fn a_group_a_db_instance_is_using_is_refused_with_the_state_fault_then_released() {
    // The rows differ in the CASE the instance spells the group's name in: the group is
    // keyed lowercased and an instance carries the name its create was given, so both
    // spellings name the one group and both must hold it.
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "the instance names the group as it was created",
            "emu-del-pg-inuse",
            "emu-del-pg-inuse",
            "emu-del-pg-inuse-instance",
        ),
        (
            "the instance names the group in another case",
            "emu-del-pg-cased",
            "EMU-Del-PG-Cased",
            "emu-del-pg-cased-instance",
        ),
    ];

    for &(case, group, named_by_instance, instance) in cases {
        let router = emulator();
        expect_ok(case, create_group(&router, REGION, group, "mysql8.0").await);
        expect_ok(
            case,
            create_instance_in(&router, instance, named_by_instance).await,
        );

        // A group in use is refused with the STATE fault, not the not-found: the group
        // exists, and the caller has to release it first.
        let refused = expect_error(case, delete_group(&router, group).await, 400, INVALID_STATE);
        assert!(
            text(&refused, "Message").contains(group),
            "case {case:?}: expected the refusal to name the group, got {refused}"
        );
        // The refused delete left the group exactly where it was.
        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeDBParameterGroups",
                &[("DBParameterGroupName", group)],
            )
            .await,
        );
        assert_eq!(
            text(&described, "DBParameterGroupName"),
            group.to_ascii_lowercase(),
            "case {case:?}: expected the refused delete to change nothing, got {described}"
        );

        // Once the instance is gone the same delete is accepted — the teardown order a
        // caller that created both has to follow. The delete is ACCEPTED at once and the
        // instance leaves the store when the removal the clock owes falls due, so the group
        // is released then rather than before.
        expect_ok(
            case,
            call(
                &router,
                "DeleteDBInstance",
                &[
                    ("DBInstanceIdentifier", instance),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await,
        );
        wait_for_settle().await;
        expect_ok(case, delete_group(&router, group).await);
    }
}

#[tokio::test]
async fn a_delete_reaches_only_the_region_it_was_signed_for() {
    let router = emulator();

    // One name, a group of that name in each of two regions: the delete signed for one
    // region must not reach the other's.
    for region in [REGION, OTHER_REGION] {
        expect_ok(
            "the regional group",
            create_group(&router, region, "emu-del-pg-regional", "mysql8.0").await,
        );
    }
    expect_ok(
        "the delete in the signed-for region",
        delete_group(&router, "emu-del-pg-regional").await,
    );
    expect_error(
        "the deleted group in the signed-for region",
        call(
            &router,
            "DescribeDBParameterGroups",
            &[("DBParameterGroupName", "emu-del-pg-regional")],
        )
        .await,
        404,
        NOT_FOUND,
    );
    let other = expect_ok(
        "the other region's group of the same name",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBParameterGroups",
            &[("DBParameterGroupName", "emu-del-pg-regional")],
        )
        .await,
    );
    assert_eq!(
        text(&other, "DBParameterGroupName"),
        "emu-del-pg-regional",
        "expected the other region's group to be untouched, got {other}"
    );
}
