// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ModifyDBProxyTargetGroup` across a proxy's lifecycle.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it, and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML the
//! emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test, asserted over a TABLE spanning the class it generalises over
//! rather than over one request: a proxy's one target group is created WITH the proxy and
//! has no provisioning of its own, so it is ready — `available` — and takes a connection
//! pool modification as soon as the proxy record exists, whatever provisioning the PROXY
//! itself still owes; the modelled state fault is reserved for a proxy on its way out.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_PROXY_STATUS_AVAILABLE, DB_PROXY_STATUS_CREATING, DB_PROXY_STATUS_DELETING,
    DEFAULT_DB_PROXY_TARGET_GROUP, DEFAULT_MAX_CONNECTIONS_PERCENT,
    DEFAULT_MAX_CONNECTIONS_PERCENT_SQLSERVER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, text, wait_for_settle, REGION};
use crate::db_proxy_target_groups::create_proxy;

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// One row of the lifecycle table: when in the proxy's own lifecycle the modification
/// arrives, the engine family the proxy was created with, and the settings the call states.
struct LifecycleCase {
    case: &'static str,
    proxy: &'static str,
    /// A value of the `EngineFamily` enum (`smithy_query(query="shape",
    /// shape="EngineFamily")`: `MYSQL | POSTGRESQL | SQLSERVER`).
    engine_family: &'static str,
    /// Whether the row waits for the proxy's own pending transition to settle before
    /// modifying the group. A row that does NOT wait modifies the group of a proxy that is
    /// still `creating` — the moment the group's readiness is independent of.
    wait_for_the_proxy: bool,
    /// `ConnectionPoolConfigurationInfo$MaxConnectionsPercent` this engine family's group
    /// starts with ("Default: 10 for RDS for Microsoft SQL Server, and 100 for all other
    /// engines"); `MaxIdleConnectionsPercent` starts at half of it.
    default_max_connections_percent: i32,
    /// `ConnectionPoolConfiguration$MaxConnectionsPercent` the request states.
    sent_max_connections_percent: i32,
    /// `ConnectionPoolConfiguration$ConnectionBorrowTimeout` the request states.
    sent_connection_borrow_timeout: i32,
}

/// The rows differ along exactly what the rule generalises over: the moment in the proxy's
/// lifecycle at which the modification arrives, the engine family whose documented pool
/// defaults the change is merged over, and the values the request carries — each published
/// bound of `MaxConnectionsPercent` ("Must be between 1 and 100") and of
/// `ConnectionBorrowTimeout` ("Must be between 0 and 300") among them, and the value
/// `MaxIdleConnectionsPercent`'s own doc works through as its example ("if
/// MaxConnectionsPercent is 80, then the default value of MaxIdleConnectionsPercent is
/// 40").
const LIFECYCLE_CASES: &[LifecycleCase] = &[
    LifecycleCase {
        case: "a MySQL proxy modified straight after the create",
        proxy: "emu-ready-mysql",
        engine_family: "MYSQL",
        wait_for_the_proxy: false,
        default_max_connections_percent: DEFAULT_MAX_CONNECTIONS_PERCENT,
        sent_max_connections_percent: 50,
        sent_connection_borrow_timeout: 120,
    },
    LifecycleCase {
        case: "a PostgreSQL proxy modified straight after the create, at the lowest bounds",
        proxy: "emu-ready-postgres",
        engine_family: "POSTGRESQL",
        wait_for_the_proxy: false,
        default_max_connections_percent: DEFAULT_MAX_CONNECTIONS_PERCENT,
        sent_max_connections_percent: 1,
        sent_connection_borrow_timeout: 0,
    },
    LifecycleCase {
        case: "a SQL Server proxy modified straight after the create, at the highest bounds",
        proxy: "emu-ready-sqlserver",
        engine_family: "SQLSERVER",
        wait_for_the_proxy: false,
        default_max_connections_percent: DEFAULT_MAX_CONNECTIONS_PERCENT_SQLSERVER,
        sent_max_connections_percent: 100,
        sent_connection_borrow_timeout: 300,
    },
    LifecycleCase {
        case: "a MySQL proxy modified once it has come up on its own",
        proxy: "emu-ready-settled",
        engine_family: "MYSQL",
        wait_for_the_proxy: true,
        default_max_connections_percent: DEFAULT_MAX_CONNECTIONS_PERCENT,
        sent_max_connections_percent: 80,
        sent_connection_borrow_timeout: 7,
    },
];

/// `ModifyDBProxyTargetGroup` for the one target group of `proxy`, answered with the HTTP
/// status and the body.
async fn modify_target_group(
    router: &MultiServiceRouter,
    proxy: &str,
    max_connections_percent: i32,
    connection_borrow_timeout: i32,
) -> (u16, String) {
    call(
        router,
        "ModifyDBProxyTargetGroup",
        &[
            ("DBProxyName", proxy.to_string()),
            (
                "TargetGroupName",
                DEFAULT_DB_PROXY_TARGET_GROUP.to_string(),
            ),
            (
                "ConnectionPoolConfig.MaxConnectionsPercent",
                max_connections_percent.to_string(),
            ),
            (
                "ConnectionPoolConfig.ConnectionBorrowTimeout",
                connection_borrow_timeout.to_string(),
            ),
        ],
    )
    .await
}

/// `DescribeDBProxyTargetGroups` for `proxy` — what reports the group back.
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

/// `DescribeDBProxies` for `proxy`.
async fn read_proxy(router: &MultiServiceRouter, case: &str, proxy: &str) -> String {
    expect_ok(
        case,
        call(router, "DescribeDBProxies", &[("DBProxyName", proxy)]).await,
    )
}

#[tokio::test]
async fn a_target_group_is_ready_and_modifiable_as_soon_as_its_proxy_exists() {
    let router = emulator();

    for row in LIFECYCLE_CASES {
        create_proxy(&router, row.proxy, row.engine_family).await;
        if row.wait_for_the_proxy {
            wait_for_settle();
        }

        // The group the modification is a change TO: ready, and holding the settings
        // documented as the defaults for the proxy's engine family.
        let before = read_target_group(&router, row.case, row.proxy).await;
        let created_date = text(&before, "CreatedDate");
        for (member, expected) in [
            ("Status", DB_PROXY_STATUS_AVAILABLE.to_string()),
            (
                "MaxConnectionsPercent",
                row.default_max_connections_percent.to_string(),
            ),
            (
                "MaxIdleConnectionsPercent",
                (row.default_max_connections_percent / 2).to_string(),
            ),
        ] {
            let actual = text(&before, member);
            assert_eq!(
                actual, expected,
                "case {:?}: expected the group before the modification to report {member} \
                 {expected:?}, got {actual:?} in {before}",
                row.case
            );
        }

        // The modification is ACCEPTED whatever the proxy itself still owes, and answers
        // with the group it changed.
        let answered = expect_ok(
            row.case,
            modify_target_group(
                &router,
                row.proxy,
                row.sent_max_connections_percent,
                row.sent_connection_borrow_timeout,
            )
            .await,
        );
        let expected_members = [
            ("DBProxyName", row.proxy.to_string()),
            ("TargetGroupName", DEFAULT_DB_PROXY_TARGET_GROUP.to_string()),
            (
                "TargetGroupArn",
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:target-group:{}", row.proxy),
            ),
            // "Because each proxy is currently associated with a single target group,
            // currently this setting is always true" (`DBProxyTargetGroup$IsDefault`).
            ("IsDefault", "true".to_string()),
            ("Status", DB_PROXY_STATUS_AVAILABLE.to_string()),
            (
                "MaxConnectionsPercent",
                row.sent_max_connections_percent.to_string(),
            ),
            (
                "ConnectionBorrowTimeout",
                row.sent_connection_borrow_timeout.to_string(),
            ),
            // The request stated no idle percentage, so that percentage takes its
            // documented default for the pool size now in force: "half of the value of
            // MaxConnectionsPercent".
            (
                "MaxIdleConnectionsPercent",
                (row.sent_max_connections_percent / 2).to_string(),
            ),
            // When the group was created does not change when it is modified.
            ("CreatedDate", created_date.clone()),
        ];
        // The answer and a later report agree: the change was persisted, not echoed.
        let reported = read_target_group(&router, row.case, row.proxy).await;
        for (member, expected) in &expected_members {
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
        let updated_date = text(&answered, "UpdatedDate");
        assert_ne!(
            updated_date, created_date,
            "case {:?}: expected the modification to refresh UpdatedDate past CreatedDate \
             {created_date:?}, got {updated_date:?} in {answered}",
            row.case
        );

        // The modification acted on the GROUP: it moved the proxy's own lifecycle nowhere,
        // so a proxy that was still coming up still is, and one that had come up still is.
        let proxy = read_proxy(&router, row.case, row.proxy).await;
        let expected_status = if row.wait_for_the_proxy {
            DB_PROXY_STATUS_AVAILABLE
        } else {
            DB_PROXY_STATUS_CREATING
        };
        let actual = text(&proxy, "Status");
        assert_eq!(
            actual, expected_status,
            "case {:?}: expected the proxy's own status to stay {expected_status:?}, got \
             {actual:?} in {proxy}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_proxy_on_its_way_out_has_a_target_group_no_modification_acts_on() {
    let router = emulator();
    create_proxy(&router, "emu-ready-leaving", "MYSQL").await;
    wait_for_settle();

    // A delete is ACCEPTED rather than instantaneous, so the proxy — and the group it
    // takes with it — is on its way out from the answer onwards.
    let deleted = expect_ok(
        "the delete of the proxy",
        call(
            &router,
            "DeleteDBProxy",
            &[("DBProxyName", "emu-ready-leaving")],
        )
        .await,
    );
    assert_eq!(
        text(&deleted, "Status"),
        DB_PROXY_STATUS_DELETING,
        "case \"the delete of the proxy\": expected the proxy to report \
         {DB_PROXY_STATUS_DELETING}, got {deleted}"
    );

    let refusal = expect_error(
        "the target group of a proxy on its way out",
        modify_target_group(&router, "emu-ready-leaving", 50, 120).await,
        400,
        "InvalidDBProxyStateFault",
    );
    for expected in ["emu-ready-leaving", DB_PROXY_STATUS_DELETING] {
        assert!(
            refusal.contains(expected),
            "case \"the target group of a proxy on its way out\": expected the refusal to \
             name {expected:?}, got {refusal}"
        );
    }
    // The group of a proxy on its way out reports the state the proxy is leaving in: it
    // has no provisioning of its own, and it is going with the proxy.
    let reported = read_target_group(
        &router,
        "the group of a proxy on its way out",
        "emu-ready-leaving",
    )
    .await;
    let actual = text(&reported, "Status");
    assert_eq!(
        actual, DB_PROXY_STATUS_DELETING,
        "case \"the group of a proxy on its way out\": expected Status \
         {DB_PROXY_STATUS_DELETING:?}, got {actual:?} in {reported}"
    );
}
