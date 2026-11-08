// SPDX-License-Identifier: CC0-1.0

//! The cluster's step runner, end to end over the wire: what a submitted step settles in.
//!
//! THE RULE every case below asserts: a step submitted to a cluster that is up is
//! dispatched by the cluster's step runner — in submission order, at most
//! `StepConcurrencyLevel` at a time — and settles in the terminal `StepState` its work
//! produced, with the `StepTimeline` of the run: `COMPLETED` when the work succeeded and
//! `FAILED`, carrying the driver's own error, when it did not. The work of a
//! `command-runner.jar` step is the command its arguments are; a statement that command
//! submits whose only effect is READING names tables, and a table the cluster's Hive
//! metastore does not hold fails the step. A step whose work this emulator does not run —
//! one submitting no statement, one whose statement would leave state behind, or one on a
//! cluster whose metastore this emulator does not hold — settles in the same modelled
//! failure state carrying the reason that says its work was not run, never a success for
//! work nothing performed.
//!
//! Every case drives the real HTTP path — `POST /` with an `awsJson1_1` body — through the
//! router's `dispatch`, against one emulator carrying BOTH Amazon EMR and the Glue Data
//! Catalog over one shared engine, because the metastore a step resolves its tables in is
//! that catalog. Nothing reaches into either service's state struct: the catalog is
//! written through `CreateDatabase`/`CreateTable` and a step is only ever read back
//! through `DescribeStep` and `ListSteps`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_flow_lifecycle::{at, INSTANCE_PROFILE, SERVICE_ROLE};

/// How long a poll for a step's terminal state keeps asking. The runner owes a queued
/// step two clock-owed spans (500 ms each: `PENDING` -> `RUNNING` -> terminal), so this is
/// several times that and still leaves a failing case to fail fast.
const SETTLE_BUDGET: Duration = Duration::from_secs(6);

/// How long a poll waits between asks.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The Glue database the catalog holds in these cases.
const DATABASE: &str = "sim_catalog_db";

/// The table that database holds.
const REGISTERED_TABLE: &str = "orders";

/// A table name the catalog holds nothing under.
const UNREGISTERED_TABLE: &str = "orders_absent";

/// The property and value that make a cluster's Hive metastore the Glue Data Catalog, as
/// Amazon EMR documents the Hive metastore client factory for it (see
/// `crate::state::GLUE_METASTORE_FACTORY_CLASS`).
const METASTORE_PROPERTY: &str = "hive.metastore.client.factory.class";
const GLUE_FACTORY: &str = "com.amazonaws.glue.catalog.metastore.AWSGlueDataCatalogHiveClientFactory";

/// The step states a step never leaves again (`com.amazonaws.emr#StepState`).
const TERMINAL_STATES: [&str; 4] = ["COMPLETED", "CANCELLED", "FAILED", "INTERRUPTED"];

/// One emulator carrying Amazon EMR and the Glue Data Catalog over one shared engine.
struct Pairing {
    router: MultiServiceRouter,
}

impl Pairing {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(engine.clone()),
        );
        router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    async fn call(&self, service: &str, target: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{service}.us-east-1.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDSTEPRUNNER/20261105/us-east-1/\
                     {service}/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    async fn emr(&self, operation: &str, body: Value) -> Value {
        let (status, payload) = self
            .call("elasticmapreduce", "ElasticMapReduce", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "{operation} must answer 200, got {status} with {payload}"
        );
        payload
    }

    async fn glue(&self, operation: &str, body: Value) -> Value {
        let (status, payload) = self.call("glue", "AWSGlue", operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} must answer 200, got {status} with {payload}"
        );
        payload
    }

    /// Register `DATABASE` with one table over an S3 location, the way a client prepares a
    /// Glue-metastore cluster's catalog.
    async fn register_catalog(&self) {
        self.glue(
            "CreateDatabase",
            json!({ "DatabaseInput": { "Name": DATABASE } }),
        )
        .await;
        self.glue(
            "CreateTable",
            json!({
                "DatabaseName": DATABASE,
                "TableInput": {
                    "Name": REGISTERED_TABLE,
                    "StorageDescriptor": {
                        "Location": "s3://sim-catalog/orders/",
                        "Columns": [{ "Name": "id", "Type": "bigint" }],
                    },
                },
            }),
        )
        .await;
    }

    /// A Glue-metastore cluster that stays up when it has no steps to run, with
    /// `concurrency` steps running at a time.
    async fn glue_metastore_cluster(&self, name: &str, concurrency: i32) -> String {
        self.cluster(
            name,
            concurrency,
            json!([{
                "Classification": "hive-site",
                "Properties": { METASTORE_PROPERTY: GLUE_FACTORY },
            }]),
        )
        .await
    }

    /// The same, with the configurations the caller gives.
    async fn cluster(&self, name: &str, concurrency: i32, configurations: Value) -> String {
        let out = self
            .emr(
                "RunJobFlow",
                json!({
                    "Name": name,
                    "ReleaseLabel": "emr-6.15.0",
                    "Applications": [{ "Name": "Spark" }, { "Name": "Hive" }],
                    "JobFlowRole": INSTANCE_PROFILE,
                    "ServiceRole": SERVICE_ROLE,
                    "StepConcurrencyLevel": concurrency,
                    "Configurations": configurations,
                    "Instances": {
                        "InstanceCount": 2,
                        "MasterInstanceType": "m5.xlarge",
                        "SlaveInstanceType": "m5.xlarge",
                        "KeepJobFlowAliveWhenNoSteps": true,
                    },
                }),
            )
            .await;
        let cluster_id = out["JobFlowId"]
            .as_str()
            .unwrap_or_else(|| panic!("RunJobFlow must answer a JobFlowId, got {out}"))
            .to_string();
        // A step runs on a cluster that is UP, so every case waits for the cluster the
        // way a client does.
        self.await_live_cluster(&cluster_id).await;
        cluster_id
    }

    /// Poll `DescribeCluster` until the cluster is in a state that runs steps.
    async fn await_live_cluster(&self, cluster_id: &str) {
        let deadline = std::time::Instant::now() + SETTLE_BUDGET;
        loop {
            let out = self
                .emr("DescribeCluster", json!({ "ClusterId": cluster_id }))
                .await;
            let state = at(&out, &["Cluster", "Status", "State"])
                .as_str()
                .unwrap_or_default()
                .to_string();
            if state == "RUNNING" || state == "WAITING" {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the cluster must reach a state that runs steps, last saw {state:?} in {out}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// `AddJobFlowSteps`, answering the minted step ids in submission order.
    async fn add_steps(&self, cluster_id: &str, steps: Vec<Value>) -> Vec<String> {
        let out = self
            .emr(
                "AddJobFlowSteps",
                json!({ "JobFlowId": cluster_id, "Steps": steps }),
            )
            .await;
        out["StepIds"]
            .as_array()
            .unwrap_or_else(|| panic!("AddJobFlowSteps must answer StepIds, got {out}"))
            .iter()
            .map(|id| {
                id.as_str()
                    .unwrap_or_else(|| panic!("a StepId is a string, got {out}"))
                    .to_string()
            })
            .collect()
    }

    async fn describe_step(&self, cluster_id: &str, step_id: &str) -> Value {
        self.emr(
            "DescribeStep",
            json!({ "ClusterId": cluster_id, "StepId": step_id }),
        )
        .await["Step"]
            .clone()
    }

    /// Poll `DescribeStep` until the step is terminal, answering the step as it settled.
    async fn await_terminal_step(&self, case: &str, cluster_id: &str, step_id: &str) -> Value {
        let deadline = std::time::Instant::now() + SETTLE_BUDGET;
        loop {
            let step = self.describe_step(cluster_id, step_id).await;
            let state = at(&step, &["Status", "State"])
                .as_str()
                .unwrap_or_default()
                .to_string();
            if TERMINAL_STATES.contains(&state.as_str()) {
                return step;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {case:?}: the step must reach a terminal state, last saw \
                 {state:?} in {step}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// A `command-runner.jar` step whose command is `args`.
fn command_step(name: &str, args: Value) -> Value {
    json!({
        "Name": name,
        "ActionOnFailure": "CONTINUE",
        "HadoopJarStep": { "Jar": "command-runner.jar", "Args": args },
    })
}

/// A step the runner runs: a SQL client of the cluster, given one statement.
fn sql_step(name: &str, client: &str, sql: &str) -> Value {
    command_step(name, json!([client, "-e", sql]))
}

/// The seconds of a step timeline member, or `None` when it is absent.
fn instant(step: &Value, member: &str) -> Option<f64> {
    at(step, &["Status", "Timeline", member]).as_f64()
}

/// THE RULE, over the statement forms a step's command can submit: the step settles in
/// `COMPLETED` when every table its statements name is registered in the cluster's Glue
/// Data Catalog metastore, and in `FAILED` naming the table when one is not.
#[tokio::test]
async fn glue_metastore_steps_settle_on_what_the_catalog_holds() {
    let pairing = Pairing::new();
    pairing.register_catalog().await;
    // Concurrency so every row of the table runs at once: the sequencing rule has its own
    // case below.
    let cluster_id = pairing.glue_metastore_cluster("catalog steps", 25).await;

    // Rows differ along: the statement form (projection, aggregate, join, insert, one
    // naming no table at all), how the name is qualified (`<database>.<table>`, bare, bare
    // after a `USE`), its casing, which SQL client of the cluster the command names, and
    // whether the command arrives as the step's own argument list or as one argument
    // holding a whole shell command line.
    let qualified_registered = format!("{DATABASE}.{REGISTERED_TABLE}");
    let qualified_unregistered = format!("{DATABASE}.{UNREGISTERED_TABLE}");
    let cases: Vec<(&str, Value, &str, Option<String>)> = vec![
        (
            "a projection of a registered table",
            sql_step(
                "select registered",
                "spark-sql",
                &format!("SELECT id FROM {qualified_registered}"),
            ),
            "COMPLETED",
            None,
        ),
        (
            "an aggregate over an unregistered table",
            sql_step(
                "count unregistered",
                "spark-sql",
                &format!("SELECT COUNT(*) FROM {qualified_unregistered}"),
            ),
            "FAILED",
            Some(qualified_unregistered.clone()),
        ),
        (
            "the registered table named in another casing",
            sql_step(
                "select upper case",
                "spark-sql",
                &format!(
                    "SELECT id FROM {}.{} WHERE id > 1",
                    DATABASE.to_uppercase(),
                    REGISTERED_TABLE.to_uppercase()
                ),
            ),
            "COMPLETED",
            None,
        ),
        (
            "a database the catalog does not hold",
            sql_step(
                "select unknown database",
                "spark-sql",
                &format!("SELECT id FROM other_db.{REGISTERED_TABLE}"),
            ),
            "FAILED",
            Some(format!("other_db.{REGISTERED_TABLE}")),
        ),
        (
            "a join of a registered and an unregistered table",
            sql_step(
                "join",
                "spark-sql",
                &format!(
                    "SELECT l.id FROM {qualified_registered} l JOIN {qualified_unregistered} r \
                     ON l.id = r.id"
                ),
            ),
            "FAILED",
            Some(qualified_unregistered.clone()),
        ),
        (
            "an insert reading an unregistered table",
            sql_step(
                "insert from unregistered",
                "hive",
                &format!(
                    "INSERT INTO TABLE {qualified_registered} SELECT id FROM \
                     {qualified_unregistered}"
                ),
            ),
            "FAILED",
            // An insert leaves state behind, which this emulator does not apply, so the
            // run reports that its work was not run rather than resolving a table.
            Some("No application was started".to_string()),
        ),
        (
            "a statement naming no table at all",
            sql_step("select constant", "spark-sql", "SELECT 1"),
            "COMPLETED",
            None,
        ),
        (
            "a bare name after a USE of the registered database",
            sql_step(
                "use then select",
                "spark-sql",
                &format!("USE {DATABASE}; SELECT id FROM {REGISTERED_TABLE}"),
            ),
            "COMPLETED",
            None,
        ),
        (
            "a bare name with no USE, which is the default database",
            sql_step(
                "bare name",
                "spark-sql",
                &format!("SELECT id FROM {REGISTERED_TABLE}"),
            ),
            "FAILED",
            Some(format!("default.{REGISTERED_TABLE}")),
        ),
        (
            "a whole shell command line in one argument",
            command_step(
                "shell wrapped",
                json!([
                    "bash",
                    "-c",
                    format!("spark-sql -e 'SELECT id FROM {qualified_registered}' > /tmp/out.txt")
                ]),
            ),
            "COMPLETED",
            None,
        ),
        (
            "a shell command line reading an unregistered table",
            command_step(
                "shell wrapped unregistered",
                json!([
                    "bash",
                    "-c",
                    format!("spark-sql -e \"SELECT id FROM {qualified_unregistered}\"")
                ]),
            ),
            "FAILED",
            Some(qualified_unregistered.clone()),
        ),
    ];

    let step_ids = pairing
        .add_steps(
            &cluster_id,
            cases.iter().map(|(_, step, _, _)| step.clone()).collect(),
        )
        .await;
    assert_eq!(
        step_ids.len(),
        cases.len(),
        "AddJobFlowSteps must answer one id per submitted step, got {step_ids:?}"
    );

    for ((case, _, expected_state, expected_table), step_id) in cases.iter().zip(&step_ids) {
        let step = pairing.await_terminal_step(case, &cluster_id, step_id).await;
        let state = at(&step, &["Status", "State"]).as_str();
        assert_eq!(
            state,
            Some(*expected_state),
            "case {case:?}: expected {expected_state}, got {state:?} in {step}"
        );

        // The run has a timeline: it started after it was created and ended after it
        // started, and both instants are reported.
        let created = instant(&step, "CreationDateTime")
            .unwrap_or_else(|| panic!("case {case:?}: a step records its creation, got {step}"));
        let started = instant(&step, "StartDateTime")
            .unwrap_or_else(|| panic!("case {case:?}: a run records its start, got {step}"));
        let ended = instant(&step, "EndDateTime")
            .unwrap_or_else(|| panic!("case {case:?}: a settled run records its end, got {step}"));
        assert!(
            created <= started && started <= ended,
            "case {case:?}: expected CreationDateTime <= StartDateTime <= EndDateTime, got \
             {created} / {started} / {ended}"
        );

        match expected_table {
            // "The details for the step failure including reason, message, and log file
            // path where the root cause was identified" (`StepStatus$FailureDetails` doc):
            // the driver's error names the table the metastore holds nothing under, or
            // says the work was not run.
            Some(fragment) => {
                let message = at(&step, &["Status", "FailureDetails", "Message"])
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    message.contains(fragment),
                    "case {case:?}: the failure must carry {fragment:?}, got {message:?} in \
                     {step}"
                );
            }
            None => assert!(
                at(&step, &["Status", "FailureDetails"]).is_null(),
                "case {case:?}: a run that succeeded details no failure, got {step}"
            ),
        }

        // The ListSteps summary of the same step agrees with DescribeStep.
        let listed = pairing
            .emr(
                "ListSteps",
                json!({ "ClusterId": &cluster_id, "StepIds": [step_id] }),
            )
            .await;
        assert_eq!(
            listed["Steps"][0]["Status"], step["Status"],
            "case {case:?}: the ListSteps status must agree with DescribeStep, got {listed}"
        );
    }
}

/// THE RULE's sequencing half: "Amazon EMR executes each step in the order listed"
/// (`AddJobFlowSteps` doc), so at `StepConcurrencyLevel` 1 a step's run starts only once
/// the run before it has ended.
#[tokio::test]
async fn steps_run_in_submission_order_one_at_a_time() {
    let pairing = Pairing::new();
    pairing.register_catalog().await;
    let cluster_id = pairing.glue_metastore_cluster("sequenced steps", 1).await;

    let submitted = vec![
        sql_step(
            "first",
            "spark-sql",
            &format!("SELECT id FROM {DATABASE}.{REGISTERED_TABLE}"),
        ),
        sql_step(
            "second",
            "spark-sql",
            &format!("SELECT COUNT(*) FROM {DATABASE}.{UNREGISTERED_TABLE}"),
        ),
        sql_step(
            "third",
            "spark-sql",
            &format!("SELECT id FROM {DATABASE}.{REGISTERED_TABLE} WHERE id < 10"),
        ),
    ];
    let expected = ["COMPLETED", "FAILED", "COMPLETED"];
    let step_ids = pairing.add_steps(&cluster_id, submitted).await;

    let mut previous_end: Option<f64> = None;
    for (index, step_id) in step_ids.iter().enumerate() {
        let case = format!("step {index}");
        let step = pairing.await_terminal_step(&case, &cluster_id, step_id).await;
        assert_eq!(
            at(&step, &["Status", "State"]).as_str(),
            Some(expected[index]),
            "case {case:?}: expected {}, got {step}",
            expected[index]
        );
        let started = instant(&step, "StartDateTime")
            .unwrap_or_else(|| panic!("case {case:?}: a run records its start, got {step}"));
        let ended = instant(&step, "EndDateTime")
            .unwrap_or_else(|| panic!("case {case:?}: a settled run records its end, got {step}"));
        if let Some(previous_end) = previous_end {
            assert!(
                started >= previous_end,
                "case {case:?}: one step at a time, so this run must start at or after \
                 {previous_end}, got {started}"
            );
        }
        previous_end = Some(ended);
    }
}

/// THE RULE's metastore half: a cluster's Hive metastore is the Glue Data Catalog when its
/// configuration selects the Glue metastore client factory, wherever in the configuration
/// tree that property sits and whichever engine's configuration carries it — a Hadoop
/// property reaches Spark under a `spark.hadoop.` prefix and is the same property.
#[tokio::test]
async fn the_glue_metastore_is_selected_wherever_the_property_sits() {
    let pairing = Pairing::new();
    pairing.register_catalog().await;

    // Rows differ along WHERE the property sits: the Hive configuration, Spark's own copy
    // of it, a nested configuration, and Spark's pass-through prefix for a Hadoop
    // property.
    let cases: Vec<(&str, Value)> = vec![
        (
            "the hive-site classification",
            json!([{
                "Classification": "hive-site",
                "Properties": { METASTORE_PROPERTY: GLUE_FACTORY },
            }]),
        ),
        (
            "the spark-hive-site classification",
            json!([{
                "Classification": "spark-hive-site",
                "Properties": { METASTORE_PROPERTY: GLUE_FACTORY },
            }]),
        ),
        (
            "a nested configuration",
            json!([{
                "Classification": "spark",
                "Properties": { "maximizeResourceAllocation": "true" },
                "Configurations": [{
                    "Classification": "spark-hive-site",
                    "Properties": { METASTORE_PROPERTY: GLUE_FACTORY },
                }],
            }]),
        ),
        (
            "Spark's pass-through prefix for a Hadoop property",
            json!([{
                "Classification": "spark-defaults",
                "Properties": { format!("spark.hadoop.{METASTORE_PROPERTY}"): GLUE_FACTORY },
            }]),
        ),
    ];

    for (case, configurations) in &cases {
        let cluster_id = pairing.cluster(case, 1, configurations.clone()).await;
        let step_ids = pairing
            .add_steps(
                &cluster_id,
                vec![sql_step(
                    "count unregistered",
                    "spark-sql",
                    &format!("SELECT COUNT(*) FROM {DATABASE}.{UNREGISTERED_TABLE}"),
                )],
            )
            .await;
        let step = pairing
            .await_terminal_step(case, &cluster_id, &step_ids[0])
            .await;
        assert_eq!(
            at(&step, &["Status", "State"]).as_str(),
            Some("FAILED"),
            "case {case:?}: the metastore was selected, so the unregistered table fails the \
             step, got {step}"
        );
    }
}

/// THE RULE's other half: a step whose work this emulator does not run still settles — in
/// the modelled failure state, carrying the reason that says its work was not run, rather
/// than a success for work nothing performed or a queue position nothing ever leaves.
#[tokio::test]
async fn a_step_whose_work_is_not_run_fails_saying_so() {
    let pairing = Pairing::new();
    pairing.register_catalog().await;
    let on_glue = pairing.glue_metastore_cluster("unrunnable work", 25).await;

    // Rows differ along WHY the work is not run: the command submits no statement, the
    // step runs a jar of its own rather than a command, and the command's statement is
    // against a metastore this emulator does not hold (no configuration at all, and a
    // configuration naming another metastore client).
    let select_registered = format!("SELECT id FROM {DATABASE}.{REGISTERED_TABLE}");
    let no_metastore = pairing
        .cluster("no metastore configuration", 25, json!([]))
        .await;
    let other_metastore = pairing
        .cluster(
            "another metastore client",
            25,
            json!([{
                "Classification": "hive-site",
                "Properties": {
                    METASTORE_PROPERTY: "org.apache.hadoop.hive.metastore.HiveMetaStoreClient",
                },
            }]),
        )
        .await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a command carrying no statement",
            &on_glue,
            command_step("state pusher", json!(["state-pusher-script"])),
        ),
        (
            "a jar of the step's own",
            &on_glue,
            json!({
                "Name": "own jar",
                "HadoopJarStep": {
                    "Jar": "s3://jars/analytics.jar",
                    "MainClass": "com.example.Analytics",
                    "Args": [&select_registered],
                },
            }),
        ),
        (
            "a statement on a cluster with no metastore configuration",
            &no_metastore,
            sql_step("select on local metastore", "spark-sql", &select_registered),
        ),
        (
            "a statement on a cluster naming another metastore client",
            &other_metastore,
            sql_step("select on other metastore", "spark-sql", &select_registered),
        ),
    ];

    for (case, cluster_id, submitted) in &cases {
        let step_ids = pairing.add_steps(cluster_id, vec![submitted.clone()]).await;
        let step = pairing
            .await_terminal_step(case, cluster_id, &step_ids[0])
            .await;
        assert_eq!(
            at(&step, &["Status", "State"]).as_str(),
            Some("FAILED"),
            "case {case:?}: work this emulator does not run fails the step, got {step}"
        );
        let message = at(&step, &["Status", "FailureDetails", "Message"])
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains("No application was started"),
            "case {case:?}: the failure must say no application ran, got {message:?} in {step}"
        );
        // A run that did not resolve a table names none: the reason is about the work, not
        // about the catalog.
        assert!(
            !message.contains("Table or view not found"),
            "case {case:?}: no table was resolved, so none may be named, got {message:?}"
        );
    }
}

/// A statement whose effect this emulator does not apply does not report success, and
/// leaves the catalog exactly as it was: the reviewer's own examples — an insert, a drop
/// and a create — settle FAILED saying the work was not run, and Glue's own read still
/// answers what it answered before.
#[tokio::test]
async fn a_statement_that_would_change_the_catalog_changes_nothing_and_reports_no_success() {
    let pairing = Pairing::new();
    pairing.register_catalog().await;
    let cluster_id = pairing.glue_metastore_cluster("writing steps", 25).await;

    // Rows differ along the effect the statement would have: insert rows, drop the table,
    // define a new one from a query.
    let cases: Vec<(&str, String)> = vec![
        (
            "an insert into the registered table",
            format!("INSERT INTO TABLE {DATABASE}.{REGISTERED_TABLE} VALUES (1)"),
        ),
        (
            "a drop of the registered table",
            format!("DROP TABLE {DATABASE}.{REGISTERED_TABLE}"),
        ),
        (
            "a create from a query over the registered table",
            format!(
                "CREATE TABLE {DATABASE}.orders_copy AS SELECT id FROM \
                 {DATABASE}.{REGISTERED_TABLE}"
            ),
        ),
    ];

    for (case, statement) in &cases {
        let step_ids = pairing
            .add_steps(&cluster_id, vec![sql_step(case, "spark-sql", statement)])
            .await;
        let step = pairing
            .await_terminal_step(case, &cluster_id, &step_ids[0])
            .await;
        assert_eq!(
            at(&step, &["Status", "State"]).as_str(),
            Some("FAILED"),
            "case {case:?}: an effect this emulator does not apply is no success, got {step}"
        );
        let message = at(&step, &["Status", "FailureDetails", "Message"])
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains("No application was started"),
            "case {case:?}: the failure must say no application ran, got {message:?}"
        );

        // The catalog is unchanged: the table the statement would have dropped or written
        // is still there, with the one column it was registered with, and the table a
        // CREATE would have defined is not.
        let (status, held) = pairing
            .call(
                "glue",
                "AWSGlue",
                "GetTable",
                json!({ "DatabaseName": DATABASE, "Name": REGISTERED_TABLE }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the registered table must still be there, got {status} with {held}"
        );
        assert_eq!(
            at(&held, &["Table", "StorageDescriptor", "Columns"])
                .as_array()
                .map(Vec::len),
            Some(1),
            "case {case:?}: the registered table is unchanged, got {held}"
        );
        let (created, body) = pairing
            .call(
                "glue",
                "AWSGlue",
                "GetTable",
                json!({ "DatabaseName": DATABASE, "Name": "orders_copy" }),
            )
            .await;
        assert_eq!(
            created, 400,
            "case {case:?}: no table was defined, so the catalog holds none, got {created} \
             with {body}"
        );
    }
}

/// A step a caller cancels before its run comes due reaches `CANCELLED`, not the state its
/// work would have produced: "you can cancel steps that are in a `PENDING` or `RUNNING`
/// state" (`CancelSteps` doc).
#[tokio::test]
async fn a_cancelled_step_settles_cancelled_rather_than_running() {
    let pairing = Pairing::new();
    pairing.register_catalog().await;
    let cluster_id = pairing.glue_metastore_cluster("cancelled step", 1).await;
    let step_ids = pairing
        .add_steps(
            &cluster_id,
            vec![sql_step(
                "to cancel",
                "spark-sql",
                &format!("SELECT id FROM {DATABASE}.{REGISTERED_TABLE}"),
            )],
        )
        .await;
    pairing
        .emr(
            "CancelSteps",
            json!({ "ClusterId": &cluster_id, "StepIds": [&step_ids[0]] }),
        )
        .await;
    let step = pairing
        .await_terminal_step("cancelled", &cluster_id, &step_ids[0])
        .await;
    assert_eq!(
        at(&step, &["Status", "State"]).as_str(),
        Some("CANCELLED"),
        "a cancelled step settles CANCELLED, got {step}"
    );
    // The cancel decides the end, so the step neither COMPLETES on the answer its work
    // would have given nor reports a failure.
    assert!(
        instant(&step, "EndDateTime").is_some(),
        "a cancelled run records when it ended, got {step}"
    );
    assert!(
        at(&step, &["Status", "FailureDetails"]).is_null(),
        "a cancelled step did not fail, so it details no failure, got {step}"
    );
}
