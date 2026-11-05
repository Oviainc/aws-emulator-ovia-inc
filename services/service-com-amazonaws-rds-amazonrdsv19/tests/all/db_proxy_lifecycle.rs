// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyDBProxyTargetGroup` and `DeleteDBProxy`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it, and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML the
//! emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The two rules under test, each asserted over a TABLE spanning the class it generalises
//! over rather than over one request:
//!
//! * a modification of a proxy's target group carries the settings a caller wants CHANGED,
//!   so each one it states is applied to the group and each one it leaves out keeps the
//!   value the group already holds — except `MaxIdleConnectionsPercent`, documented as a
//!   default derived from the pool size, which follows the `MaxConnectionsPercent` the call
//!   puts in force — a value outside the constraints
//!   `ConnectionPoolConfiguration` publishes is refused and leaves the group exactly as it
//!   was, and a group the request does not resolve — an unknown proxy, any name but the one
//!   group a proxy has — is the fault the operation models for it;
//! * a delete is ACCEPTED rather than instantaneous: the proxy reports `deleting` from the
//!   answer onwards and this service takes it away a moment later, after which every read
//!   of the name is the proxy's own not-found fault, while a proxy no delete resolves and a
//!   proxy in a state no delete acts on are the two faults that operation models — and a
//!   delete touches no other proxy.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_PROXY_STATUS_AVAILABLE, DB_PROXY_STATUS_CREATING, DB_PROXY_STATUS_DELETING,
    DEFAULT_CONNECTION_BORROW_TIMEOUT, DEFAULT_DB_PROXY_TARGET_GROUP,
    DEFAULT_MAX_CONNECTIONS_PERCENT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, text, wait_for_settle};
use crate::db_proxy_target_groups::create_proxy;

/// One row of the modification table: the parameters the call carries beyond the proxy and
/// the group, and the settings the group must report afterwards.
struct ModifyCase {
    case: &'static str,
    /// The `ConnectionPoolConfig` members the request states.
    sent: &'static [(&'static str, &'static str)],
    /// The whole of the group's connection pool settings afterwards — every member,
    /// whether this row stated it or not, so a row proves both what it changed and what it
    /// left alone.
    reported: &'static [(&'static str, &'static str)],
}

/// One row of the refusal table: a request the operation refuses, and the failure it
/// answers.
struct RefusalCase {
    case: &'static str,
    /// The whole request beyond `Action`, so a row can name a different proxy or group.
    params: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
}

/// `ModifyDBProxyTargetGroup` for the one target group of `proxy`, with `sent` as its
/// `ConnectionPoolConfig`.
async fn modify_target_group(
    router: &MultiServiceRouter,
    case: &str,
    proxy: &str,
    sent: &[(&str, &str)],
) -> String {
    let mut params: Vec<(String, String)> = vec![
        ("DBProxyName".to_string(), proxy.to_string()),
        (
            "TargetGroupName".to_string(),
            DEFAULT_DB_PROXY_TARGET_GROUP.to_string(),
        ),
    ];
    for (member, value) in sent {
        params.push((
            format!("ConnectionPoolConfig.{member}"),
            (*value).to_string(),
        ));
    }
    expect_ok(
        case,
        call(router, "ModifyDBProxyTargetGroup", &params).await,
    )
}

/// `DescribeDBProxyTargetGroups` for `proxy`, which is what reports the group a
/// modification changed.
async fn read_target_group(router: &MultiServiceRouter, case: &str, proxy: &str) -> String {
    expect_ok(
        case,
        call(
            router,
            "DescribeDBProxyTargetGroups",
            &[("DBProxyName", proxy)],
        )
        .await,
    )
}

/// `DescribeDBProxies` for `proxy`, answered with the status and the body.
async fn describe_proxy(router: &MultiServiceRouter, proxy: &str) -> (u16, String) {
    call(router, "DescribeDBProxies", &[("DBProxyName", proxy)]).await
}

#[tokio::test]
async fn a_target_group_modification_applies_what_it_states_and_keeps_the_rest() {
    let router = emulator();
    create_proxy(&router, "emu-mod-pool", "MYSQL").await;
    wait_for_settle();

    // The rows differ along exactly what the rule generalises over: WHICH members the
    // request states (one alone, two together, the integer members and the string one) and
    // which VALUES they carry — each published bound of each range among them
    // (`MaxConnectionsPercent` "Must be between 1 and 100", `ConnectionBorrowTimeout`
    // "Must be between 0 and 300", `MaxIdleConnectionsPercent` "Must be between 0 and the
    // value of MaxConnectionsPercent"). Each row's `reported` is the WHOLE group, so every
    // row also asserts the members it did not state.
    //
    // The rows run in order against one group: a modification is a change to the group as
    // it stands, so the state each row starts from is what the row before it left.
    let cases: &[ModifyCase] = &[
        ModifyCase {
            case: "the lowest ConnectionBorrowTimeout alone",
            sent: &[("ConnectionBorrowTimeout", "0")],
            reported: &[
                ("MaxConnectionsPercent", "100"),
                ("MaxIdleConnectionsPercent", "50"),
                ("ConnectionBorrowTimeout", "0"),
            ],
        },
        ModifyCase {
            case: "both pool sizes together",
            sent: &[
                ("MaxConnectionsPercent", "80"),
                ("MaxIdleConnectionsPercent", "20"),
            ],
            reported: &[
                ("MaxConnectionsPercent", "80"),
                ("MaxIdleConnectionsPercent", "20"),
                ("ConnectionBorrowTimeout", "0"),
            ],
        },
        ModifyCase {
            case: "the highest bound of each range",
            sent: &[
                ("MaxConnectionsPercent", "100"),
                ("ConnectionBorrowTimeout", "300"),
            ],
            reported: &[
                ("MaxConnectionsPercent", "100"),
                // The row states no idle percentage, so it takes its documented default
                // for the pool size this row puts in force: half of it.
                ("MaxIdleConnectionsPercent", "50"),
                ("ConnectionBorrowTimeout", "300"),
            ],
        },
        ModifyCase {
            case: "the lowest bound of each pool size",
            sent: &[
                ("MaxConnectionsPercent", "1"),
                ("MaxIdleConnectionsPercent", "0"),
            ],
            reported: &[
                ("MaxConnectionsPercent", "1"),
                ("MaxIdleConnectionsPercent", "0"),
                ("ConnectionBorrowTimeout", "300"),
            ],
        },
        ModifyCase {
            case: "an initialization query alone",
            sent: &[("InitQuery", "SET time_zone = '+00:00'")],
            reported: &[
                ("MaxConnectionsPercent", "1"),
                ("MaxIdleConnectionsPercent", "0"),
                ("ConnectionBorrowTimeout", "300"),
                ("InitQuery", "SET time_zone = '+00:00'"),
            ],
        },
    ];

    // The group starts with the settings documented as the defaults for the proxy's engine
    // family, which is what the first row is a change TO.
    let before = read_target_group(&router, "the group before any modification", "emu-mod-pool")
        .await;
    for (member, expected) in [
        (
            "MaxConnectionsPercent",
            DEFAULT_MAX_CONNECTIONS_PERCENT.to_string(),
        ),
        (
            "ConnectionBorrowTimeout",
            DEFAULT_CONNECTION_BORROW_TIMEOUT.to_string(),
        ),
    ] {
        let actual = text(&before, member);
        assert_eq!(
            actual, expected,
            "case \"before any modification\": expected {member} {expected:?}, got \
             {actual:?} in {before}"
        );
    }

    for row in cases {
        let answered = modify_target_group(&router, row.case, "emu-mod-pool", row.sent).await;
        // The modification ANSWERS with the group it changed, and a later report of the
        // group agrees with that answer: the change was persisted, not echoed.
        let reported = read_target_group(&router, row.case, "emu-mod-pool").await;
        for (member, expected) in row.reported {
            for (source, body) in [("the answer", &answered), ("the report", &reported)] {
                let actual = text(body, member);
                assert_eq!(
                    actual, *expected,
                    "case {:?}: expected {source} to carry {member} {expected:?}, got \
                     {actual:?} in {body}",
                    row.case
                );
            }
        }
        for (member, expected) in [
            ("DBProxyName", "emu-mod-pool"),
            ("TargetGroupName", DEFAULT_DB_PROXY_TARGET_GROUP),
            // The group reports the PROXY's status: it is created with the proxy and has
            // no provisioning of its own, so modifying it does not take it out of service.
            ("Status", DB_PROXY_STATUS_AVAILABLE),
        ] {
            let actual = text(&answered, member);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {member} {expected:?}, got {actual:?} in {answered}",
                row.case
            );
        }
    }

    let refusals: &[RefusalCase] = &[
        RefusalCase {
            case: "a MaxConnectionsPercent one below the lowest bound",
            params: &[
                ("DBProxyName", "emu-mod-pool"),
                ("TargetGroupName", "default"),
                ("ConnectionPoolConfig.MaxConnectionsPercent", "0"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a MaxConnectionsPercent one above the highest bound",
            params: &[
                ("DBProxyName", "emu-mod-pool"),
                ("TargetGroupName", "default"),
                ("ConnectionPoolConfig.MaxConnectionsPercent", "101"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a ConnectionBorrowTimeout one below the lowest bound",
            params: &[
                ("DBProxyName", "emu-mod-pool"),
                ("TargetGroupName", "default"),
                ("ConnectionPoolConfig.ConnectionBorrowTimeout", "-1"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a ConnectionBorrowTimeout one above the highest bound",
            params: &[
                ("DBProxyName", "emu-mod-pool"),
                ("TargetGroupName", "default"),
                ("ConnectionPoolConfig.ConnectionBorrowTimeout", "301"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a MaxIdleConnectionsPercent above the MaxConnectionsPercent in force",
            params: &[
                ("DBProxyName", "emu-mod-pool"),
                ("TargetGroupName", "default"),
                ("ConnectionPoolConfig.MaxIdleConnectionsPercent", "50"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a rename of the one group a proxy has",
            params: &[
                ("DBProxyName", "emu-mod-pool"),
                ("TargetGroupName", "default"),
                ("NewName", "emu-renamed-group"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a proxy name that resolves to nothing",
            params: &[
                ("DBProxyName", "emu-mod-nope"),
                ("TargetGroupName", "default"),
            ],
            status: 404,
            code: "DBProxyNotFoundFault",
        },
        RefusalCase {
            case: "a target group name that is not the one group the proxy has",
            params: &[
                ("DBProxyName", "emu-mod-pool"),
                ("TargetGroupName", "emu-other-group"),
            ],
            status: 404,
            code: "DBProxyTargetGroupNotFoundFault",
        },
    ];
    for row in refusals {
        expect_error(
            row.case,
            call(&router, "ModifyDBProxyTargetGroup", row.params).await,
            row.status,
            row.code,
        );
    }

    // Every refusal left the group exactly as the last accepted modification did.
    let after = read_target_group(&router, "the group after the refusals", "emu-mod-pool").await;
    for (member, expected) in cases[cases.len() - 1].reported {
        let actual = text(&after, member);
        assert_eq!(
            actual, *expected,
            "case \"after the refusals\": expected {member} {expected:?}, got {actual:?} in \
             {after}"
        );
    }
}

#[tokio::test]
async fn deleting_a_proxy_puts_it_in_deleting_and_then_takes_it_away() {
    let router = emulator();

    // A proxy name that resolves to nothing is the proxy's own not-found fault.
    expect_error(
        "an unknown proxy",
        call(&router, "DeleteDBProxy", &[("DBProxyName", "emu-del-nope")]).await,
        404,
        "DBProxyNotFoundFault",
    );

    // A proxy that has not finished coming up is in a state no delete acts on.
    create_proxy(&router, "emu-del-creating", "MYSQL").await;
    let creating = expect_error(
        "a proxy that is still coming up",
        call(
            &router,
            "DeleteDBProxy",
            &[("DBProxyName", "emu-del-creating")],
        )
        .await,
        400,
        "InvalidDBProxyStateFault",
    );
    assert!(
        creating.contains(DB_PROXY_STATUS_CREATING),
        "case \"a proxy that is still coming up\": expected the refusal to name the state \
         the proxy is in, got {creating}"
    );

    // Two proxies, so the delete of one is shown to leave the other alone.
    create_proxy(&router, "emu-del-target", "MYSQL").await;
    create_proxy(&router, "emu-del-bystander", "POSTGRESQL").await;
    wait_for_settle();

    let answered = expect_ok(
        "the delete of an available proxy",
        call(
            &router,
            "DeleteDBProxy",
            &[("DBProxyName", "emu-del-target")],
        )
        .await,
    );
    for (member, expected) in [
        ("DBProxyName", "emu-del-target"),
        // The delete is ACCEPTED, not instantaneous: "the proxy goes into `deleting`".
        ("Status", DB_PROXY_STATUS_DELETING),
    ] {
        let actual = text(&answered, member);
        assert_eq!(
            actual, expected,
            "case \"the delete of an available proxy\": expected {member} {expected:?}, got \
             {actual:?} in {answered}"
        );
    }

    // Before the removal is due the proxy is still there, in the state the delete put it
    // in — and a second delete is refused for exactly that reason.
    let (status, reported) = describe_proxy(&router, "emu-del-target").await;
    assert_eq!(
        status, 200,
        "a proxy whose delete was accepted is still reported until the removal is due, got \
         {status} {reported}"
    );
    assert_eq!(
        text(&reported, "Status"),
        DB_PROXY_STATUS_DELETING,
        "the proxy must report the state the delete put it in, got {reported}"
    );
    expect_error(
        "a second delete of the same proxy",
        call(
            &router,
            "DeleteDBProxy",
            &[("DBProxyName", "emu-del-target")],
        )
        .await,
        400,
        "InvalidDBProxyStateFault",
    );

    wait_for_settle();

    // Once the removal is due every read of the name is the proxy's own not-found fault —
    // the report of its target groups included, since the group goes with the proxy.
    for action in [
        "DescribeDBProxies",
        "DescribeDBProxyTargetGroups",
        "DescribeDBProxyTargets",
    ] {
        expect_error(
            action,
            call(&router, action, &[("DBProxyName", "emu-del-target")]).await,
            404,
            "DBProxyNotFoundFault",
        );
    }

    // The delete touched no other proxy.
    let (status, bystander) = describe_proxy(&router, "emu-del-bystander").await;
    assert_eq!(
        status, 200,
        "deleting one proxy must leave another alone, got {status} {bystander}"
    );
    assert_eq!(
        text(&bystander, "Status"),
        DB_PROXY_STATUS_AVAILABLE,
        "the other proxy must still be available, got {bystander}"
    );
}
