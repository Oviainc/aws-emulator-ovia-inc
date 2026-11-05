// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the lifecycles a client drives WITH A POLL LOOP: the DB instance
//! `CreateDBInstance` -> `ModifyDBInstance` -> `StopDBInstance` -> `StartDBInstance` ->
//! `DeleteDBInstance` flow, and the Aurora Limitless Database `CreateDBCluster` ->
//! `CreateDBShardGroup` -> `DeleteDBShardGroup` provision-and-teardown flow.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on. The assertions read the XML the emulator answers with, so a handler
//! that computed nothing could not pass them.
//!
//! The rule under test: a status transition this service owes a resource becomes due a
//! BOUNDED span after the call that started it and is materialised by the next call, so a
//! client that polls a REPORTING operation — making no mutating call of its own, which is
//! what every SDK waiter and provisioning script does — reaches the settled state of every
//! stage of the documented lifecycle in bounded time, and the call that was accepted
//! answers the transient status it stored (`creating`, `modifying`, `stopping`, `starting`,
//! `deleting`). The stage that was missing is `StartDBInstance`: it takes a `stopped`
//! instance through `starting` to `available`, retains the metadata the stop kept, and
//! refuses every other case as the model declares it. A `DBInstanceIdentifier` that names
//! no instance is the modelled 404 worded "DBInstance <id> not found." whichever operation
//! raises it.
//!
//! The tables below span what the rules generalise over: engine families an instance-level
//! start and stop apply to with their own instance classes and storage sizes, every stage
//! of the instance lifecycle and of the shard-group lifecycle, the statuses a start can
//! arrive in, each engine family the command does not apply to, a cluster member, and
//! shard groups created with different capacities and standby redundancy.

use std::time::{Duration, Instant};

use service_com_amazonaws_rds_amazonrdsv19::state::start_db_instance_state_message;
use smithy_server_runtime::MultiServiceRouter;
use tokio::time::sleep;

use crate::db_engine_versions::all_versions_of;
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, subtrees, text, REGION,
};

/// How long a poll loop is given before this suite calls a transition unobservable.
///
/// Generous next to the span a transition this service owes becomes due after, so a loaded
/// test machine cannot fail the suite, and still far short of a client's patience.
const POLL_DEADLINE: Duration = Duration::from_secs(20);

/// The gap between two polls of the same loop.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The account id every generated emulator presents, which the ARNs below are read against.
const ACCOUNT: &str = "000000000000";

/// One row of the DB instance lifecycle table.
struct InstanceCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    class: &'static str,
    /// The `AllocatedStorage` the create asks for.
    storage: i32,
    /// The `AllocatedStorage` the modification grows it to.
    grown_storage: i32,
}

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// Poll `read` until `settled` holds, answering the response it settled on and how many
/// polls that took; panics with the last answer when [`POLL_DEADLINE`] passes first.
///
/// This is the shape of the loop a real client writes, and the thing under test: `read`
/// makes a REPORTING call and nothing else, so only a transition this service's clock owes
/// can end the loop.
async fn poll_until<Read, Fut>(
    case: &str,
    what: &str,
    read: Read,
    settled: impl Fn(u16, &str) -> bool,
) -> (String, usize)
where
    Read: Fn() -> Fut,
    Fut: std::future::Future<Output = (u16, String)>,
{
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut polls = 0usize;
    let mut last = String::new();
    while Instant::now() < deadline {
        let (status, body) = read().await;
        polls += 1;
        if settled(status, &body) {
            return (body, polls);
        }
        last = format!("HTTP {status}: {body}");
        sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: {what} never settled in {POLL_DEADLINE:?} ({polls} polls); \
         last answer: {last}"
    )
}

/// The one `<DBInstance>` subtree a report carries for `identifier`.
fn the_instance(case: &str, body: &str) -> String {
    let reported = subtrees(body, "DBInstance");
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: exactly one DBInstance must be reported, got {body}"
    );
    reported.into_iter().next().expect("checked just above")
}

/// `DescribeDBInstances` for one instance, as the HTTP status and the body.
async fn read_instance(router: &MultiServiceRouter, identifier: &str) -> (u16, String) {
    call(
        router,
        "DescribeDBInstances",
        &params!["DBInstanceIdentifier" => identifier],
    )
    .await
}

/// Poll `DescribeDBInstances` until the instance reports `status`, answering the reported
/// `<DBInstance>` and how many polls that took.
async fn poll_instance_until(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    status: &str,
) -> (String, usize) {
    let (body, polls) = poll_until(
        case,
        &format!("{identifier} reaching {status}"),
        || read_instance(router, identifier),
        |http_status, body| {
            http_status == 200
                && optional_text(&the_instance(case, body), "DBInstanceStatus").as_deref()
                    == Some(status)
        },
    )
    .await;
    (the_instance(case, &body), polls)
}

/// The settled `<DBInstance>` a poll loop reached right after the call that owed the
/// transition, asserting that the transition was OWED: the first poll of the loop still
/// reported the transient status, so the settled one was not answered instantaneously.
async fn poll_owed_instance_transition(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    status: &str,
) -> String {
    let (instance, polls) = poll_instance_until(router, case, identifier, status).await;
    assert!(
        polls >= 2,
        "case {case:?}: reaching {status} is a transition this service OWES, so the first \
         poll after the call must still report the transient status; it reported \
         {status} after {polls} poll(s)"
    );
    instance
}

/// The `DBInstanceStatus` a mutating call answered with, out of its own response.
fn answered_status(case: &str, body: &str) -> String {
    text(&the_instance(case, body), "DBInstanceStatus")
}

#[tokio::test]
async fn a_polling_client_reaches_every_settled_state_of_the_db_instance_lifecycle() {
    // The rows differ along what the rule generalises over: the engine families an
    // instance-level start and stop apply to, each with its own instance class, and
    // storage sizes that grow by different amounts.
    let table: &[InstanceCase] = &[
        InstanceCase {
            case: "postgres on a burstable class, storage grown by five",
            identifier: "emu-life-postgres",
            engine: "postgres",
            class: "db.t3.micro",
            storage: 20,
            grown_storage: 25,
        },
        InstanceCase {
            case: "mysql on a standard class, storage grown by twenty",
            identifier: "emu-life-mysql",
            engine: "mysql",
            class: "db.m5.large",
            storage: 100,
            grown_storage: 120,
        },
        InstanceCase {
            case: "mariadb, storage grown to more than twice its size",
            identifier: "emu-life-mariadb",
            engine: "mariadb",
            class: "db.t3.small",
            storage: 20,
            grown_storage: 50,
        },
        InstanceCase {
            case: "sqlserver-ex, a licensed engine",
            identifier: "emu-life-sqlserver",
            engine: "sqlserver-ex",
            class: "db.t3.medium",
            storage: 20,
            grown_storage: 30,
        },
        InstanceCase {
            case: "oracle-se2, another licensed engine",
            identifier: "emu-life-oracle",
            engine: "oracle-se2",
            class: "db.m5.xlarge",
            storage: 20,
            grown_storage: 40,
        },
    ];

    for row in table {
        // Each row gets its own emulator, so one row's transitions cannot settle for
        // another.
        let router = emulator();
        let case = row.case;

        // 1. The create is accepted and answers the transient status it stored.
        let created = expect_ok(
            case,
            call(
                &router,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => row.identifier,
                    "Engine" => row.engine,
                    "DBInstanceClass" => row.class,
                    "AllocatedStorage" => row.storage,
                    "MasterUsername" => "emuadmin",
                    "BackupRetentionPeriod" => 0,
                ],
            )
            .await,
        );
        let status = answered_status(case, &created);
        assert_eq!(
            status, "creating",
            "case {case:?}: an accepted create must answer `creating`, got {status:?}"
        );

        // 2. A poll loop — no mutating call in it — reaches the settled instance, and
        //    what it reports there is what the create persisted.
        let available =
            poll_owed_instance_transition(&router, case, row.identifier, "available").await;
        assert_eq!(
            optional_text(&available, "AllocatedStorage"),
            Some(row.storage.to_string()),
            "case {case:?}: the settled instance must report the storage the create asked \
             for, got {:?}",
            optional_text(&available, "AllocatedStorage")
        );
        assert_eq!(
            optional_text(&available, "DBInstanceArn"),
            Some(format!(
                "arn:aws:rds:{REGION}:{ACCOUNT}:db:{}",
                row.identifier
            )),
            "case {case:?}: the settled instance must report its own ARN, got {:?}",
            optional_text(&available, "DBInstanceArn")
        );
        let endpoint = optional_text(&available, "Address");
        assert!(
            endpoint.as_deref().is_some_and(|address| {
                address.starts_with(row.identifier)
                    || address
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|host| host.is_loopback())
            }),
            "case {case:?}: the settled instance must report the endpoint a client \
             connects to — its own engine listener's host where this emulator serves its \
             engine, else the DNS name of its endpoint — got {endpoint:?}"
        );

        // 3. A modification: accepted as `modifying`, and the poll loop reaches the
        //    settled instance carrying the change.
        let modified = expect_ok(
            case,
            call(
                &router,
                "ModifyDBInstance",
                &params![
                    "DBInstanceIdentifier" => row.identifier,
                    "AllocatedStorage" => row.grown_storage,
                    "ApplyImmediately" => true,
                ],
            )
            .await,
        );
        let status = answered_status(case, &modified);
        assert_eq!(
            status, "modifying",
            "case {case:?}: an accepted modification must answer `modifying`, got {status:?}"
        );
        let available =
            poll_owed_instance_transition(&router, case, row.identifier, "available").await;
        assert_eq!(
            optional_text(&available, "AllocatedStorage"),
            Some(row.grown_storage.to_string()),
            "case {case:?}: the settled instance must report the grown storage, got {:?}",
            optional_text(&available, "AllocatedStorage")
        );

        // 4. The stop: accepted as `stopping`, settling on `stopped`.
        let stopped_answer = expect_ok(
            case,
            call(
                &router,
                "StopDBInstance",
                &params!["DBInstanceIdentifier" => row.identifier],
            )
            .await,
        );
        let status = answered_status(case, &stopped_answer);
        assert_eq!(
            status, "stopping",
            "case {case:?}: an accepted stop must answer `stopping`, got {status:?}"
        );
        let stopped = poll_owed_instance_transition(&router, case, row.identifier, "stopped").await;

        // 5. The start: accepted as `starting`, settling back on `available`, with every
        //    setting the stop retained still reported.
        let started_answer = expect_ok(
            case,
            call(
                &router,
                "StartDBInstance",
                &params!["DBInstanceIdentifier" => row.identifier],
            )
            .await,
        );
        let status = answered_status(case, &started_answer);
        assert_eq!(
            status, "starting",
            "case {case:?}: an accepted start must answer `starting`, got {status:?}"
        );
        let restarted =
            poll_owed_instance_transition(&router, case, row.identifier, "available").await;
        for member in [
            "Address",
            "DBInstanceArn",
            "DbiResourceId",
            "DBInstanceClass",
            "Engine",
            "AllocatedStorage",
            "MasterUsername",
            "InstanceCreateTime",
            "DBParameterGroupName",
            "OptionGroupName",
        ] {
            let (was, now) = (
                optional_text(&stopped, member),
                optional_text(&restarted, member),
            );
            assert_eq!(
                was, now,
                "case {case:?}: a start must retain {member}, was {was:?} and is now {now:?}"
            );
        }

        // 6. The delete: accepted as `deleting`, and the poll loop reaches the modelled
        //    not-found, worded the way this service words that fault.
        let deleted_answer = expect_ok(
            case,
            call(
                &router,
                "DeleteDBInstance",
                &params![
                    "DBInstanceIdentifier" => row.identifier,
                    "SkipFinalSnapshot" => true,
                ],
            )
            .await,
        );
        let status = answered_status(case, &deleted_answer);
        assert_eq!(
            status, "deleting",
            "case {case:?}: an accepted delete must answer `deleting`, got {status:?}"
        );
        let (gone, polls) = poll_until(
            case,
            &format!("{} leaving the store", row.identifier),
            || read_instance(&router, row.identifier),
            |http_status, _| http_status == 404,
        )
        .await;
        assert!(
            polls >= 2,
            "case {case:?}: the removal is a transition this service OWES, so the first poll \
             after the delete must still report the instance; it was gone after {polls} \
             poll(s)"
        );
        assert_eq!(
            text(&gone, "Code"),
            "DBInstanceNotFound",
            "case {case:?}: the read of a deleted instance must be DBInstanceNotFound, \
             got {gone}"
        );
        assert_eq!(
            text(&gone, "Message"),
            format!("DBInstance {} not found.", row.identifier),
            "case {case:?}: got {:?}",
            text(&gone, "Message")
        );
    }
}

/// One row of the DB shard group table: the capacity and redundancy a Limitless shard
/// group is provisioned with.
struct ShardGroupCase {
    case: &'static str,
    cluster: &'static str,
    identifier: &'static str,
    max_acu: f64,
    min_acu: Option<f64>,
    compute_redundancy: Option<i32>,
}

#[tokio::test]
async fn a_polling_client_provisions_and_tears_down_a_limitless_shard_group() {
    // The rows differ along what the rule generalises over: a shard group with nothing but
    // the required capacity, one with a minimum capacity and a standby data access shard,
    // and one with two standbys and a fractional capacity.
    let table: &[ShardGroupCase] = &[
        ShardGroupCase {
            case: "nothing but the maximum capacity",
            cluster: "emu-life-limitless-a",
            identifier: "emu-life-shard-a",
            max_acu: 16.0,
            min_acu: None,
            compute_redundancy: None,
        },
        ShardGroupCase {
            case: "a minimum capacity and one standby data access shard",
            cluster: "emu-life-limitless-b",
            identifier: "emu-life-shard-b",
            max_acu: 1000.0,
            min_acu: Some(24.0),
            compute_redundancy: Some(1),
        },
        ShardGroupCase {
            case: "two standby data access shards and a fractional capacity",
            cluster: "emu-life-limitless-c",
            identifier: "emu-life-shard-c",
            max_acu: 12.5,
            min_acu: Some(0.5),
            compute_redundancy: Some(2),
        },
    ];

    for row in table {
        let router = emulator();
        let case = row.case;

        // The engine version comes out of the catalogue this service publishes — the
        // Aurora PostgreSQL version that reports `SupportsLimitlessDatabase` — exactly as
        // a client picks one, rather than being written here.
        let limitless = all_versions_of(&router, "aurora-postgresql")
            .await
            .into_iter()
            .find(|version| {
                optional_text(&version.scalars, "SupportsLimitlessDatabase").as_deref()
                    == Some("true")
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the catalogue must publish an Aurora PostgreSQL version \
                     that supports Aurora Limitless Database"
                )
            });

        // 1. The Limitless cluster: accepted as `creating`, reached as `available` by a
        //    poll loop that makes no mutating call.
        let created = expect_ok(
            case,
            call(
                &router,
                "CreateDBCluster",
                &params![
                    "DBClusterIdentifier" => row.cluster,
                    "Engine" => "aurora-postgresql",
                    "EngineVersion" => limitless.engine_version,
                    "ClusterScalabilityType" => "limitless",
                    "StorageType" => "aurora-iopt1",
                    "MasterUsername" => "emuadmin",
                ],
            )
            .await,
        );
        let status = text(&created, "Status");
        assert_eq!(
            status, "creating",
            "case {case:?}: an accepted cluster create must answer `creating`, got {status:?}"
        );
        let cluster_request = params!["DBClusterIdentifier" => row.cluster];
        let read_cluster = || call(&router, "DescribeDBClusters", &cluster_request);
        let (cluster, polls) = poll_until(
            case,
            &format!("{} reaching available", row.cluster),
            read_cluster,
            |http_status, body| {
                http_status == 200 && optional_text(body, "Status").as_deref() == Some("available")
            },
        )
        .await;
        assert!(
            polls >= 2,
            "case {case:?}: the cluster's transition to available is OWED, so the first poll \
             after the create must still report `creating`; it was available after {polls} \
             poll(s)"
        );
        assert_eq!(
            optional_text(&cluster, "ClusterScalabilityType").as_deref(),
            Some("limitless"),
            "case {case:?}: the settled cluster must report the scalability type it was \
             created with, got {cluster}"
        );

        // 2. The shard group: accepted as `creating`, reached as `available`.
        let mut request: Vec<(&str, String)> = params![
            "DBShardGroupIdentifier" => row.identifier,
            "DBClusterIdentifier" => row.cluster,
            "MaxACU" => row.max_acu,
        ]
        .to_vec();
        if let Some(min_acu) = row.min_acu {
            request.push(("MinACU", min_acu.to_string()));
        }
        if let Some(compute_redundancy) = row.compute_redundancy {
            request.push(("ComputeRedundancy", compute_redundancy.to_string()));
        }
        let created = expect_ok(case, call(&router, "CreateDBShardGroup", &request).await);
        let status = text(&created, "Status");
        assert_eq!(
            status, "creating",
            "case {case:?}: an accepted shard group create must answer `creating`, got \
             {status:?}"
        );
        let shard_group_request = params!["DBShardGroupIdentifier" => row.identifier];
        let read_shard_group = || call(&router, "DescribeDBShardGroups", &shard_group_request);
        let (shard_group, polls) = poll_until(
            case,
            &format!("{} reaching available", row.identifier),
            read_shard_group,
            |http_status, body| {
                http_status == 200 && optional_text(body, "Status").as_deref() == Some("available")
            },
        )
        .await;
        assert!(
            polls >= 2,
            "case {case:?}: the shard group's transition to available is OWED, so the first \
             poll after the create must still report `creating`; it was available after \
             {polls} poll(s)"
        );
        assert_eq!(
            optional_text(&shard_group, "DBClusterIdentifier").as_deref(),
            Some(row.cluster),
            "case {case:?}: the settled shard group must name the cluster it was created \
             for, got {shard_group}"
        );
        assert_eq!(
            optional_text(&shard_group, "MaxACU"),
            Some(service_common::query_decimal_floats::decimal_double_text(
                row.max_acu
            )),
            "case {case:?}: the settled shard group must report its maximum capacity, got \
             {:?}",
            optional_text(&shard_group, "MaxACU")
        );

        // 3. The teardown: accepted as `deleting`, and the report answers the modelled
        //    not-found once the delete has run its course.
        let deleted = expect_ok(
            case,
            call(
                &router,
                "DeleteDBShardGroup",
                &params!["DBShardGroupIdentifier" => row.identifier],
            )
            .await,
        );
        let status = text(&deleted, "Status");
        assert_eq!(
            status, "deleting",
            "case {case:?}: an accepted shard group delete must answer `deleting`, got \
             {status:?}"
        );
        let (gone, polls) = poll_until(
            case,
            &format!("{} leaving the store", row.identifier),
            read_shard_group,
            |http_status, _| http_status == 404,
        )
        .await;
        assert!(
            polls >= 2,
            "case {case:?}: the shard group's removal is OWED, so the first poll after the \
             delete must still report it; it was gone after {polls} poll(s)"
        );
        assert_eq!(
            text(&gone, "Code"),
            "DBShardGroupNotFound",
            "case {case:?}: the read of a deleted shard group must be DBShardGroupNotFound, \
             got {gone}"
        );
        assert_eq!(
            text(&gone, "Message"),
            format!("DBShardGroup {} not found.", row.identifier),
            "case {case:?}: got {:?}",
            text(&gone, "Message")
        );
        // And the list the flow is about is empty again.
        let listed = expect_ok(
            case,
            call(
                &router,
                "DescribeDBShardGroups",
                &[] as &[(&str, String)],
            )
            .await,
        );
        assert!(
            subtrees(&listed, "DBShardGroup").is_empty(),
            "case {case:?}: the teardown must leave no shard group listed, got {listed}"
        );
    }
}

/// One row of the start-refusal table: an engine an instance-level start does not apply to.
struct EngineCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
}

#[tokio::test]
async fn a_start_is_accepted_only_for_a_stopped_instance_this_command_applies_to() {
    let router = emulator();

    // An identifier naming nothing is the modelled 404, worded the one way this service
    // words that fault. The identifiers vary so a row cannot pass on a value baked into
    // the handler.
    for identifier in ["emu-life-absent", "another-missing-instance", "x"] {
        let case = format!("StartDBInstance of the absent {identifier}");
        let body = expect_error(
            &case,
            call(
                &router,
                "StartDBInstance",
                &params!["DBInstanceIdentifier" => identifier],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
        assert_eq!(
            text(&body, "Message"),
            format!("DBInstance {identifier} not found."),
            "case {case:?}: got {:?}",
            text(&body, "Message")
        );
    }

    // An instance that is still being created is not stopped, so it cannot be started:
    // the status the next read reports is the one the start is judged against.
    expect_ok(
        "create the instance",
        call(
            &router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-life-start",
                "Engine" => "postgres",
                "DBInstanceClass" => "db.t3.micro",
                "AllocatedStorage" => 20,
                "MasterUsername" => "emuadmin",
            ],
        )
        .await,
    );
    let body = expect_error(
        "an instance still creating",
        call(
            &router,
            "StartDBInstance",
            &params!["DBInstanceIdentifier" => "emu-life-start"],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );
    let message = text(&body, "Message");
    assert_eq!(
        message,
        start_db_instance_state_message("emu-life-start"),
        "the refusal must answer the service's own sentence listing the startable statuses, \
         got {message:?}"
    );

    // Nor can one that is already running.
    poll_instance_until(&router, "the running instance", "emu-life-start", "available").await;
    let body = expect_error(
        "an available instance",
        call(
            &router,
            "StartDBInstance",
            &params!["DBInstanceIdentifier" => "emu-life-start"],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );
    let message = text(&body, "Message");
    assert_eq!(
        message,
        start_db_instance_state_message("emu-life-start"),
        "an instance that is already running must be refused in the same sentence, got \
         {message:?}"
    );

    // A stopped instance IS started, which is what makes the rows above refusals of the
    // status and not of the start itself — and a second start inside the starting window
    // is refused, because by then it is no longer stopped.
    expect_ok(
        "the stop",
        call(
            &router,
            "StopDBInstance",
            &params!["DBInstanceIdentifier" => "emu-life-start"],
        )
        .await,
    );
    poll_instance_until(&router, "the stopped instance", "emu-life-start", "stopped").await;
    expect_ok(
        "the first start",
        call(
            &router,
            "StartDBInstance",
            &params!["DBInstanceIdentifier" => "emu-life-start"],
        )
        .await,
    );
    let body = expect_error(
        "a second start inside the starting window",
        call(
            &router,
            "StartDBInstance",
            &params!["DBInstanceIdentifier" => "emu-life-start"],
        )
        .await,
        400,
        "InvalidDBInstanceState",
    );
    let message = text(&body, "Message");
    assert_eq!(
        message,
        start_db_instance_state_message("emu-life-start"),
        "a start that is already running must be refused in the same sentence, got \
         {message:?}"
    );

    // "The identifier or the name of the DB capacity reservation for the DB instance to
    // use after it starts": no operation of this emulator mints a DB capacity
    // reservation, so no name resolves to one and the modelled not-found fault is the
    // answer — never a start that quietly ignores the reservation the caller asked for.
    poll_instance_until(&router, "the started instance", "emu-life-start", "available").await;
    expect_ok(
        "the stop before the reservation rows",
        call(
            &router,
            "StopDBInstance",
            &params!["DBInstanceIdentifier" => "emu-life-start"],
        )
        .await,
    );
    poll_instance_until(
        &router,
        "the stopped instance again",
        "emu-life-start",
        "stopped",
    )
    .await;
    for reservation in ["dbcr-reservation", "my-reservation-name"] {
        let case = format!("a start naming the DB capacity reservation {reservation}");
        let body = expect_error(
            &case,
            call(
                &router,
                "StartDBInstance",
                &params![
                    "DBInstanceIdentifier" => "emu-life-start",
                    "DBCapacityReservationIdentifier" => reservation,
                ],
            )
            .await,
            404,
            "DBCapacityReservationNotFound",
        );
        assert_eq!(
            text(&body, "Message"),
            format!("DBCapacityReservation {reservation} not found."),
            "case {case:?}: got {:?}",
            text(&body, "Message")
        );
        let still = read_instance(&router, "emu-life-start").await;
        let reported = optional_text(&the_instance(&case, &still.1), "DBInstanceStatus");
        assert_eq!(
            reported.as_deref(),
            Some("stopped"),
            "case {case:?}: a refused start must leave the instance stopped, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_engines_an_instance_level_start_does_not_apply_to_are_refused() {
    // "This command doesn't apply to RDS Custom, Aurora MySQL, and Aurora PostgreSQL. For
    // Aurora DB clusters, use StartDBCluster instead." The engines are read off
    // `CreateDBInstanceMessage$Engine`'s own list: `aurora-mysql` and `aurora-postgresql`
    // are Aurora, and the `custom-` engines are RDS Custom.
    let table: &[EngineCase] = &[
        EngineCase {
            case: "Aurora MySQL",
            identifier: "emu-life-aurora-mysql",
            engine: "aurora-mysql",
        },
        EngineCase {
            case: "Aurora PostgreSQL",
            identifier: "emu-life-aurora-pg",
            engine: "aurora-postgresql",
        },
        EngineCase {
            case: "RDS Custom for Oracle",
            identifier: "emu-life-custom-oracle",
            engine: "custom-oracle-ee",
        },
        EngineCase {
            case: "RDS Custom for SQL Server",
            identifier: "emu-life-custom-sqlserver",
            engine: "custom-sqlserver-se",
        },
    ];

    let router = emulator();
    for row in table {
        expect_ok(
            row.case,
            call(
                &router,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => row.identifier,
                    "Engine" => row.engine,
                    "DBInstanceClass" => "db.m5.large",
                    "AllocatedStorage" => 20,
                    "MasterUsername" => "emuadmin",
                ],
            )
            .await,
        );
    }

    for row in table {
        poll_instance_until(&router, row.case, row.identifier, "available").await;
        let body = expect_error(
            row.case,
            call(
                &router,
                "StartDBInstance",
                &params!["DBInstanceIdentifier" => row.identifier],
            )
            .await,
            400,
            "InvalidDBInstanceState",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains(row.identifier) && message.contains(row.engine),
            "case {:?}: the refusal must name the instance and its engine, got {message:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_cluster_member_cannot_be_started_on_its_own() {
    let router = emulator();
    // A Multi-AZ DB cluster, with the settings `CreateDBCluster` requires for one, and an
    // instance that belongs to it.
    expect_ok(
        "create the cluster",
        call(
            &router,
            "CreateDBCluster",
            &params![
                "DBClusterIdentifier" => "emu-life-member-cluster",
                "Engine" => "mysql",
                "AllocatedStorage" => 100,
                "DBClusterInstanceClass" => "db.m6gd.xlarge",
                "StorageType" => "io1",
                "Iops" => 1000,
            ],
        )
        .await,
    );
    expect_ok(
        "create the member",
        call(
            &router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-life-member",
                "Engine" => "mysql",
                "DBInstanceClass" => "db.m5.large",
                "AllocatedStorage" => 20,
                "MasterUsername" => "emuadmin",
                "DBClusterIdentifier" => "emu-life-member-cluster",
            ],
        )
        .await,
    );
    poll_instance_until(&router, "the member", "emu-life-member", "available").await;

    let body = expect_error(
        "a member of a DB cluster",
        call(
            &router,
            "StartDBInstance",
            &params!["DBInstanceIdentifier" => "emu-life-member"],
        )
        .await,
        400,
        "InvalidDBClusterStateFault",
    );
    let message = text(&body, "Message");
    assert!(
        message.contains("emu-life-member") && message.contains("emu-life-member-cluster"),
        "the refusal must name the instance and its cluster, got {message:?}"
    );
}
