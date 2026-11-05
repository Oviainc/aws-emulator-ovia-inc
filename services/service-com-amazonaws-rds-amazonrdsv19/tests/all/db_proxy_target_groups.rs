// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDBProxyTargetGroups`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it, and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML the
//! emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test, asserted over a TABLE spanning the class it generalises over
//! rather than over one request: a report of a proxy's target groups computes the one
//! group that proxy was created with from the proxy's own stored record — its name, its
//! own ARN, that it is the proxy's default group, its readiness (which the group has as
//! soon as the proxy record exists, the group having no provisioning of its own), and the
//! connection pool settings documented as the defaults for the proxy's engine family —
//! while a proxy name that resolves to nothing, and any group name that is not the one
//! group a proxy has, are the two not-found faults the operation models; and the report
//! changes nothing a later caller sees.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_PROXY_STATUS_AVAILABLE, DB_PROXY_STATUS_CREATING, DEFAULT_CONNECTION_BORROW_TIMEOUT,
    DEFAULT_DB_PROXY_TARGET_GROUP, DEFAULT_MAX_CONNECTIONS_PERCENT,
    DEFAULT_MAX_CONNECTIONS_PERCENT_SQLSERVER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text,
    wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// An IAM role ARN of the form `CreateDBProxy$RoleArn` takes.
const PROXY_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/rds-proxy";

/// A subnet id of the published form (`subnet-` plus eight hexadecimal characters).
const SUBNET_ID: &str = "subnet-0a1b2c3d";

/// One row of the engine-family table: the proxy created, and the connection pool
/// settings its target group reports.
struct EngineFamilyCase {
    case: &'static str,
    proxy: &'static str,
    engine_family: &'static str,
    /// `ConnectionPoolConfigurationInfo$MaxConnectionsPercent` this engine family's
    /// target group starts with; `MaxIdleConnectionsPercent` is half of it.
    max_connections_percent: i32,
}

/// Every value the `EngineFamily` enum takes (`smithy_query(query="shape",
/// shape="EngineFamily")`: `MYSQL | POSTGRESQL | SQLSERVER`), with the
/// `MaxConnectionsPercent` default `ConnectionPoolConfiguration$MaxConnectionsPercent`
/// documents for each ("Default: 10 for RDS for Microsoft SQL Server, and 100 for all
/// other engines"). The rows differ along exactly what the defaults generalise over: the
/// engine family the proxy was created with.
const ENGINE_FAMILY_CASES: &[EngineFamilyCase] = &[
    EngineFamilyCase {
        case: "a MySQL proxy",
        proxy: "emu-tg-mysql",
        engine_family: "MYSQL",
        max_connections_percent: DEFAULT_MAX_CONNECTIONS_PERCENT,
    },
    EngineFamilyCase {
        case: "a PostgreSQL proxy",
        proxy: "emu-tg-postgres",
        engine_family: "POSTGRESQL",
        max_connections_percent: DEFAULT_MAX_CONNECTIONS_PERCENT,
    },
    EngineFamilyCase {
        case: "a SQL Server proxy",
        proxy: "emu-tg-sqlserver",
        engine_family: "SQLSERVER",
        max_connections_percent: DEFAULT_MAX_CONNECTIONS_PERCENT_SQLSERVER,
    },
];

/// `CreateDBProxy` with its required members, for a proxy of `engine_family`.
///
/// `pub(crate)` so a sibling suite of this one test target stands a proxy up the same way
/// rather than growing a second copy of this call.
pub(crate) async fn create_proxy(router: &MultiServiceRouter, name: &str, engine_family: &str) {
    expect_ok(
        &format!("create proxy {name}"),
        call(
            router,
            "CreateDBProxy",
            &[
                ("DBProxyName", name),
                ("EngineFamily", engine_family),
                ("RoleArn", PROXY_ROLE_ARN),
                ("VpcSubnetIds.member.1", SUBNET_ID),
            ],
        )
        .await,
    );
}

/// `DescribeDBProxyTargetGroups` for `proxy`, with `extra` beyond `DBProxyName`.
async fn describe_target_groups(
    router: &MultiServiceRouter,
    case: &str,
    proxy: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut params = vec![("DBProxyName", proxy)];
    params.extend_from_slice(extra);
    expect_ok(
        case,
        call(router, "DescribeDBProxyTargetGroups", &params).await,
    )
}

#[tokio::test]
async fn each_proxy_reports_the_one_target_group_it_was_created_with() {
    let router = emulator();
    for row in ENGINE_FAMILY_CASES {
        create_proxy(&router, row.proxy, row.engine_family).await;
    }

    // The group is created WITH the proxy and has no provisioning of its own, so it is
    // ready — `available` — as soon as the proxy record exists, even while the proxy
    // itself is still coming up.
    let creating = describe_target_groups(
        &router,
        "the target group of a proxy that is still coming up",
        ENGINE_FAMILY_CASES[0].proxy,
        &[],
    )
    .await;
    assert_eq!(
        text(&creating, "Status"),
        DB_PROXY_STATUS_AVAILABLE,
        "case \"a proxy that is still coming up\": expected the target group to report \
         {DB_PROXY_STATUS_AVAILABLE}, got {:?} in {creating}",
        text(&creating, "Status")
    );
    let proxy_coming_up = expect_ok(
        "the proxy that is still coming up",
        call(
            &router,
            "DescribeDBProxies",
            &[("DBProxyName", ENGINE_FAMILY_CASES[0].proxy)],
        )
        .await,
    );
    assert_eq!(
        text(&proxy_coming_up, "Status"),
        DB_PROXY_STATUS_CREATING,
        "case \"a proxy that is still coming up\": the PROXY is the one that is still \
         coming up, got {proxy_coming_up}"
    );
    wait_for_settle();

    for row in ENGINE_FAMILY_CASES {
        let body = describe_target_groups(&router, row.case, row.proxy, &[]).await;
        // The modelled `TargetGroupList` is a plain list, so each `DBProxyTargetGroup` is
        // a `<member>` of `<TargetGroups>` on the wire.
        let groups = subtrees(&body, "member");
        assert_eq!(
            groups.len(),
            1,
            "case {:?}: expected exactly one target group, got {} in {body}",
            row.case,
            groups.len()
        );
        for (element, expected) in [
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
                row.max_connections_percent.to_string(),
            ),
            // "The default value is half of the value of MaxConnectionsPercent."
            (
                "MaxIdleConnectionsPercent",
                (row.max_connections_percent / 2).to_string(),
            ),
            (
                "ConnectionBorrowTimeout",
                DEFAULT_CONNECTION_BORROW_TIMEOUT.to_string(),
            ),
        ] {
            let actual = text(&body, element);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {element} {expected:?}, got {actual:?} in {body}",
                row.case
            );
        }
        assert!(
            !text(&body, "CreatedDate").is_empty(),
            "case {:?}: expected the group to report when it was created, got {body}",
            row.case
        );
        assert_eq!(
            optional_text(&body, "Marker"),
            None,
            "case {:?}: a proxy's one target group fits in one page, so no Marker is due: \
             {body}",
            row.case
        );

        // Naming the group the proxy HAS reports that same group.
        let named = describe_target_groups(
            &router,
            row.case,
            row.proxy,
            &[("TargetGroupName", DEFAULT_DB_PROXY_TARGET_GROUP)],
        )
        .await;
        assert_eq!(
            subtrees(&named, "member"),
            groups,
            "case {:?}: naming the proxy's own target group reports the same group: {named}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_target_group_report_refuses_what_names_none_and_changes_nothing() {
    let router = emulator();

    // A proxy name that resolves to nothing is the proxy's own not-found fault — before
    // any proxy exists, and in a region that holds none of that name.
    expect_error(
        "an unknown proxy",
        call(
            &router,
            "DescribeDBProxyTargetGroups",
            &[("DBProxyName", "emu-tg-nope")],
        )
        .await,
        404,
        "DBProxyNotFoundFault",
    );
    create_proxy(&router, "emu-tg-faults", "MYSQL").await;
    wait_for_settle();
    expect_error(
        "a proxy of that name in another region",
        call_in(
            &router,
            "us-east-1",
            "DescribeDBProxyTargetGroups",
            &[("DBProxyName", "emu-tg-faults")],
        )
        .await,
        404,
        "DBProxyNotFoundFault",
    );

    // Any name but the one group a proxy has names none of its target groups — including
    // one that differs from it only in case.
    for group_name in ["emu-other-group", "Default", "defaultgroup"] {
        expect_error(
            &format!("a target group named {group_name}"),
            call(
                &router,
                "DescribeDBProxyTargetGroups",
                &[
                    ("DBProxyName", "emu-tg-faults"),
                    ("TargetGroupName", group_name),
                ],
            )
            .await,
            404,
            "DBProxyTargetGroupNotFoundFault",
        );
    }

    // `Filters` "is not currently supported", so a request that carries one is refused
    // rather than served a list the filter was meant to narrow.
    expect_error(
        "a filter this operation serves none of",
        call(
            &router,
            "DescribeDBProxyTargetGroups",
            &[
                ("DBProxyName", "emu-tg-faults"),
                ("Filters.Filter.1.Name", "target-group-name"),
                ("Filters.Filter.1.Values.Value.1", "default"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    // Both published `MaxRecords` bounds are served; one step past either is a value the
    // parameter does not accept.
    for (case, value) in [("the lowest", "20"), ("the highest", "100")] {
        let body = describe_target_groups(
            &router,
            &format!("{case} MaxRecords"),
            "emu-tg-faults",
            &[("MaxRecords", value)],
        )
        .await;
        assert_eq!(
            subtrees(&body, "member").len(),
            1,
            "case {case:?}: the one target group fits in the page, got {body}"
        );
    }
    for (case, value) in [("one below the lowest", "19"), ("one above the highest", "101")] {
        let body = expect_error(
            &format!("{case} MaxRecords"),
            call(
                &router,
                "DescribeDBProxyTargetGroups",
                &[("DBProxyName", "emu-tg-faults"), ("MaxRecords", value)],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&body, "Message");
        assert!(
            message.to_lowercase().contains("maxrecords"),
            "case {case:?}: expected the refusal to name MaxRecords, got {message:?}"
        );
    }

    // A `Marker` no prior page issued is refused rather than resynchronised.
    expect_error(
        "a marker no page issued",
        call(
            &router,
            "DescribeDBProxyTargetGroups",
            &[
                ("DBProxyName", "emu-tg-faults"),
                ("Marker", "emu-not-a-marker"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );

    // The report changed nothing a later caller sees: the group reads back the same, and
    // the proxy is still the one the create stored.
    let first = describe_target_groups(&router, "the first report", "emu-tg-faults", &[]).await;
    let again = describe_target_groups(&router, "the second report", "emu-tg-faults", &[]).await;
    assert_eq!(
        subtrees(&again, "member"),
        subtrees(&first, "member"),
        "a report changes nothing a later caller sees: {first} then {again}"
    );
    let proxy = expect_ok(
        "the proxy after the reports",
        call(
            &router,
            "DescribeDBProxies",
            &[("DBProxyName", "emu-tg-faults")],
        )
        .await,
    );
    assert_eq!(
        text(&proxy, "Status"),
        DB_PROXY_STATUS_AVAILABLE,
        "a report leaves the proxy's status where it was: {proxy}"
    );
}
