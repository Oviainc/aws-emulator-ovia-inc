// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the option group DELETE path: `DeleteOptionGroup` against what
//! `CreateOptionGroup` persisted, and against what it no longer holds.
//!
//! Every call goes over the wire, through the same mounting helper and awsQuery form POST
//! the sibling suites use, so the assertions read the XML the emulator itself answered
//! with.
//!
//! The rule under test is the same one `global_clusters.rs` and `event_subscriptions.rs`
//! pin for their resources, on the delete side: a resource a create persisted is visible
//! to every later call that looks it up — so the delete of a group that exists is
//! accepted — and the modelled not-found fault is answered only for a name nothing
//! created, or one whose group a delete has already removed. The one documented refusal in
//! between is a group a DB instance is using, which is a state fault rather than a
//! not-found: "You can't delete an option group that is associated with a DB instance".
//!
//! The table spans the classes the rule generalises over: the engine and major-engine
//! version the group was created for, whether options were added to it before the delete,
//! the shape of its name, and the region it was created in.

use smithy_server_runtime::MultiServiceRouter;

use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state::STATUS_TRANSITION_DELAY;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, REGION};

/// A second region, so the regional scope of a delete is observable.
const OTHER_REGION: &str = "eu-west-1";

/// One row of the create/delete round trip.
struct DeletionCase {
    case: &'static str,
    name: &'static str,
    engine_name: &'static str,
    major_engine_version: &'static str,
    /// An option added through `ModifyOptionGroup` before the delete, if any: a group
    /// that holds options is deleted the same way an empty one is.
    option_name: Option<&'static str>,
}

/// The option groups the round trip creates, deletes and then looks up again.
///
/// The rows differ along what the rule generalises over: the engine (four entries of the
/// published `EngineName` list), the major engine version's form (`8.0`, `10.11`, `19`,
/// `15.00`), whether the group holds an option at delete time, and the name's shape.
const DELETION_CASES: &[DeletionCase] = &[
    DeletionCase {
        case: "an empty mysql group",
        name: "emu-del-mysql",
        engine_name: "mysql",
        major_engine_version: "8.0",
        option_name: None,
    },
    DeletionCase {
        case: "a mariadb group holding an option",
        name: "emu-del-mariadb",
        engine_name: "mariadb",
        major_engine_version: "10.11",
        option_name: Some("MARIADB_AUDIT_PLUGIN"),
    },
    DeletionCase {
        case: "an oracle group, hyphen-bearing name",
        name: "emu-del-oracle-se2",
        engine_name: "oracle-se2",
        major_engine_version: "19",
        option_name: None,
    },
    DeletionCase {
        case: "a sqlserver group, four-part major version",
        name: "emu-del-sqlserver",
        engine_name: "sqlserver-ex",
        major_engine_version: "15.00",
        option_name: None,
    },
];

/// `CreateOptionGroup` with its four required members, in `region`.
async fn create_group(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    engine_name: &str,
    major_engine_version: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateOptionGroup",
        &[
            ("OptionGroupName", name),
            ("EngineName", engine_name),
            ("MajorEngineVersion", major_engine_version),
            ("OptionGroupDescription", "deletion-rule-options"),
        ],
    )
    .await
}

#[tokio::test]
async fn deleting_a_persisted_group_is_accepted_and_only_then_is_it_not_found() {
    let router = emulator();

    for row in DELETION_CASES {
        expect_ok(
            row.case,
            create_group(
                &router,
                REGION,
                row.name,
                row.engine_name,
                row.major_engine_version,
            )
            .await,
        );
        if let Some(option_name) = row.option_name {
            expect_ok(
                row.case,
                call(
                    &router,
                    "ModifyOptionGroup",
                    &[
                        ("OptionGroupName", row.name),
                        (
                            "OptionsToInclude.OptionConfiguration.1.OptionName",
                            option_name,
                        ),
                    ],
                )
                .await,
            );
        }
        // The group the create persisted: the delete of it is ACCEPTED, which is the
        // defect this suite pins — a group that exists must never be answered with the
        // not-found fault.
        let deleted = expect_ok(
            row.case,
            call(&router, "DeleteOptionGroup", &[("OptionGroupName", row.name)]).await,
        );
        assert!(
            deleted.contains("<DeleteOptionGroupResponse"),
            "case {:?}: expected the delete's own empty-output envelope, got {deleted}",
            row.case
        );
        // Only now is the name not found — by the report and by a second delete alike,
        // which is what lets a caller tell "deleted" from "never existed".
        for (action, code) in [
            ("DescribeOptionGroups", "OptionGroupNotFoundFault"),
            ("DeleteOptionGroup", "OptionGroupNotFoundFault"),
        ] {
            expect_error(
                row.case,
                call(&router, action, &[("OptionGroupName", row.name)]).await,
                404,
                code,
            );
        }
    }
}

#[tokio::test]
async fn a_group_a_db_instance_is_using_is_refused_with_the_state_fault_then_released() {
    let router = emulator();

    expect_ok(
        "the option group",
        create_group(&router, REGION, "emu-del-inuse", "mysql", "8.0").await,
    );
    expect_ok(
        "the DB instance using it",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-del-inuse-instance"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
                ("OptionGroupName", "emu-del-inuse"),
            ],
        )
        .await,
    );
    // A group in use is refused with the STATE fault, not the not-found: the group exists,
    // and the caller has to release it first.
    let refused = expect_error(
        "the group while an instance uses it",
        call(
            &router,
            "DeleteOptionGroup",
            &[("OptionGroupName", "emu-del-inuse")],
        )
        .await,
        400,
        "InvalidOptionGroupStateFault",
    );
    // The sentence is worded from the REQUEST alone — the group the delete addressed,
    // single-quoted — and does not report which instance holds it.
    let message = text(&refused, "Message");
    assert_eq!(
        message, "The option group 'emu-del-inuse' cannot be deleted because it is in use.",
        "expected the in-use sentence for the requested group, got {message:?}"
    );
    assert!(
        !message.contains("emu-del-inuse-instance"),
        "expected the refusal not to name the instance using the group, got {message:?}"
    );
    // The refused delete left the group exactly where it was.
    let described = expect_ok(
        "the group after the refusal",
        call(
            &router,
            "DescribeOptionGroups",
            &[("OptionGroupName", "emu-del-inuse")],
        )
        .await,
    );
    assert_eq!(
        text(&described, "OptionGroupName"),
        "emu-del-inuse",
        "expected the refused delete to change nothing, got {described}"
    );

    // Once the instance is gone the same delete is accepted — the teardown order a
    // caller that created both has to follow. The delete is ACCEPTED at once and the
    // instance leaves the store when the removal the clock owes falls due, so the group
    // is released then rather than before.
    expect_ok(
        "the instance delete",
        call(
            &router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-del-inuse-instance"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
    expect_ok(
        "the group once nothing uses it",
        call(
            &router,
            "DeleteOptionGroup",
            &[("OptionGroupName", "emu-del-inuse")],
        )
        .await,
    );
}

#[tokio::test]
async fn a_delete_reaches_only_the_region_it_was_signed_for() {
    let router = emulator();

    // One name, a group of that name in each of two regions: the delete signed for one
    // region must not reach the other's.
    for region in [REGION, OTHER_REGION] {
        expect_ok(
            "the regional group",
            create_group(&router, region, "emu-del-regional", "postgres", "16").await,
        );
    }
    expect_ok(
        "the delete in us-west-2",
        call_in(
            &router,
            REGION,
            "DeleteOptionGroup",
            &[("OptionGroupName", "emu-del-regional")],
        )
        .await,
    );
    expect_error(
        "the deleted group in us-west-2",
        call(
            &router,
            "DescribeOptionGroups",
            &[("OptionGroupName", "emu-del-regional")],
        )
        .await,
        404,
        "OptionGroupNotFoundFault",
    );
    let other = expect_ok(
        "the other region's group of the same name",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeOptionGroups",
            &[("OptionGroupName", "emu-del-regional")],
        )
        .await,
    );
    assert_eq!(
        text(&other, "OptionGroupName"),
        "emu-del-regional",
        "expected the other region's group to be untouched, got {other}"
    );
}
