// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the DMS migration-task slice this crate implements:
//! `CreateReplicationInstance`, `CreateEndpoint`, `CreateReplicationTask`,
//! `DescribeReplicationInstances`, `DescribeEndpoints`, `DescribeReplicationTasks`,
//! `StartReplicationTask` and `DeleteReplicationTask`.
//!
//! Every case drives the real wire path — `POST /` with
//! `X-Amz-Target: AmazonDMSv20160101.<Operation>` through the multi-service router — so
//! what is asserted is what a client sees, body and HTTP status included.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_databasemigrationservice_amazondmsv20160101::state::{
    MAX_RECORDS_MAX, MAX_RECORDS_MIN, RESOURCE_IDENTIFIER_MAX_LEN, TRANSITION_DELAY,
};
use service_com_amazonaws_databasemigrationservice_amazondmsv20160101::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The region every case signs for.
pub(crate) const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix of this service, which is its Smithy service shape name.
const TARGET_PREFIX: &str = "AmazonDMSv20160101";

/// The account segment every ARN this emulator mints carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// A router carrying this service — and, through [`Emulator::with_network`], the owners of
/// the resources it resolves — over one shared engine.
///
/// Shared with the other suites in this target, so every case drives the same wire
/// path.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// This service mounted beside the OWNERS of the resources it resolves — Amazon EC2,
    /// which holds the account's subnets and the Region's Availability Zones, and IAM,
    /// which holds the roles an endpoint asks DMS to assume — on ONE shared engine, plus
    /// an SDK client for each over the router's own in-process callback: the very seam
    /// this service's own resolution reaches them through.
    ///
    /// Nothing in a suite using this reaches into either peer's store: a subnet exists
    /// only because EC2 answered for it, and a role only because IAM did.
    pub(crate) fn with_network() -> (Self, ::aws_sdk_ec2::Client, ::aws_sdk_iam::Client) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        let ec2 = ::aws_sdk_ec2::Client::from_conf(
            ::aws_sdk_ec2::Config::builder()
                .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
                .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
                .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
                .credentials_provider(callback.aws_credentials_provider())
                .http_client(callback.http_client())
                .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        );
        let iam = ::aws_sdk_iam::Client::from_conf(
            ::aws_sdk_iam::Config::builder()
                .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
                .region(::aws_sdk_iam::config::Region::new(REGION.to_string()))
                .endpoint_url(format!("https://iam.{REGION}.amazonaws.com"))
                .credentials_provider(callback.aws_credentials_provider())
                .http_client(callback.http_client())
                .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        );
        (Self { router, engine }, ec2, iam)
    }

    /// The engine the service is mounted on, so a suite can seed a record for a
    /// resource whose own create operation is still a stub.
    pub(crate) fn engine(&self) -> &engine::InMemoryEngine {
        &self.engine
    }

    /// Call `operation` with `body`, signed for [`REGION`] so the service resolves the
    /// request's region the way it does in production (the SigV4 credential scope).
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/dms/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Call `operation`, asserting a 200 and returning the response body.
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {response}"
        );
        response
    }

    /// A replication instance that has finished provisioning, and its ARN.
    pub(crate) async fn available_instance(&self, identifier: &str) -> String {
        let created = self
            .ok(
                identifier,
                "CreateReplicationInstance",
                &json!({
                    "ReplicationInstanceIdentifier": identifier,
                    "ReplicationInstanceClass": "dms.t3.medium",
                }),
            )
            .await;
        wait_for_transition();
        text(&created, "/ReplicationInstance/ReplicationInstanceArn")
    }

    /// An endpoint of `endpoint_type`, and its ARN.
    pub(crate) async fn endpoint(
        &self,
        identifier: &str,
        endpoint_type: &str,
        engine_name: &str,
    ) -> String {
        let created = self
            .ok(
                identifier,
                "CreateEndpoint",
                &json!({
                    "EndpointIdentifier": identifier,
                    "EndpointType": endpoint_type,
                    "EngineName": engine_name,
                }),
            )
            .await;
        text(&created, "/Endpoint/EndpointArn")
    }
}

/// The `__type` an awsJson1_1 error body carries.
pub(crate) fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The string at `pointer` in `value`, or a marker naming its absence.
pub(crate) fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or("<missing>")
        .to_string()
}

/// Every string at `member` across the list at `pointer`.
fn column(value: &Value, pointer: &str, member: &str) -> Vec<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record
                        .get(member)
                        .and_then(Value::as_str)
                        .unwrap_or("<missing>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Wait past a clock-owed transition's due instant.
fn wait_for_transition() {
    std::thread::sleep(TRANSITION_DELAY + Duration::from_millis(400));
}

/// A table-mappings document, which DMS takes as an opaque string.
fn table_mappings() -> String {
    json!({
        "rules": [{
            "rule-type": "selection",
            "rule-id": "1",
            "rule-name": "1",
            "object-locator": { "schema-name": "sales", "table-name": "%" },
            "rule-action": "include",
        }],
    })
    .to_string()
}

#[tokio::test]
async fn migration_task_slice_is_read_back_from_state() {
    let emulator = Emulator::new();

    // ---- CreateReplicationInstance -------------------------------------------------
    let created_instance = emulator
        .ok(
            "create instance",
            "CreateReplicationInstance",
            &json!({
                "ReplicationInstanceIdentifier": "My-Rep-Instance",
                "ReplicationInstanceClass": "dms.t3.medium",
                "AllocatedStorage": 100,
                "EngineVersion": "3.5.3",
                "MultiAZ": false,
                "PubliclyAccessible": false,
                "AvailabilityZone": "us-east-1d",
                "Tags": [{ "Key": "team", "Value": "migrations" }],
            }),
        )
        .await;
    let instance_arn = text(&created_instance, "/ReplicationInstance/ReplicationInstanceArn");
    let instance_prefix = format!("arn:aws:dms:{REGION}:{ACCOUNT}:rep:");
    assert!(
        instance_arn.starts_with(&instance_prefix),
        "case \"create instance\": expected an ARN under {instance_prefix}, got \
         {instance_arn:?}"
    );
    assert_eq!(
        text(
            &created_instance,
            "/ReplicationInstance/ReplicationInstanceIdentifier"
        ),
        "my-rep-instance",
        "case \"create instance\": expected the identifier stored lowercased, got \
         {created_instance}"
    );
    assert_eq!(
        text(
            &created_instance,
            "/ReplicationInstance/ReplicationInstanceStatus"
        ),
        "creating",
        "case \"create instance\": expected a freshly created instance to be creating, \
         got {created_instance}"
    );
    assert_eq!(
        created_instance
            .pointer("/ReplicationInstance/AllocatedStorage")
            .and_then(Value::as_i64),
        Some(100),
        "case \"create instance\": expected the requested storage, got {created_instance}"
    );
    assert!(
        created_instance
            .pointer("/ReplicationInstance/InstanceCreateTime")
            .is_some(),
        "case \"create instance\": expected an InstanceCreateTime, got {created_instance}"
    );

    // The describe answers from the same record the create wrote.
    let described = emulator
        .ok(
            "describe instance",
            "DescribeReplicationInstances",
            &json!({
                "Filters": [{
                    "Name": "replication-instance-id",
                    "Values": ["my-rep-instance"],
                }],
            }),
        )
        .await;
    assert_eq!(
        column(
            &described,
            "/ReplicationInstances",
            "ReplicationInstanceArn"
        ),
        vec![instance_arn.clone()],
        "case \"describe instance\": expected exactly the created instance, got {described}"
    );
    assert_eq!(
        text(&described, "/ReplicationInstances/0/EngineVersion"),
        "3.5.3",
        "case \"describe instance\": expected the requested engine version, got {described}"
    );
    assert_eq!(
        text(&described, "/ReplicationInstances/0/AvailabilityZone"),
        "us-east-1d",
        "case \"describe instance\": expected the requested AZ, got {described}"
    );

    // `creating` is not terminal: the clock owes `available`.
    wait_for_transition();
    let settled = emulator
        .ok(
            "instance settles",
            "DescribeReplicationInstances",
            &json!({}),
        )
        .await;
    assert_eq!(
        column(&settled, "/ReplicationInstances", "ReplicationInstanceStatus"),
        vec!["available".to_string()],
        "case \"instance settles\": expected the instance to reach available, got {settled}"
    );

    // ---- CreateEndpoint ------------------------------------------------------------
    let created_source = emulator
        .ok(
            "create source endpoint",
            "CreateEndpoint",
            &json!({
                "EndpointIdentifier": "src-ep",
                "EndpointType": "source",
                "EngineName": "mysql",
                "ServerName": "db1.example.internal",
                "Port": 3306,
                "DatabaseName": "sales",
                "Username": "migrator",
                "Password": "never-echoed",
            }),
        )
        .await;
    let source_arn = text(&created_source, "/Endpoint/EndpointArn");
    let endpoint_prefix = format!("arn:aws:dms:{REGION}:{ACCOUNT}:endpoint:");
    assert!(
        source_arn.starts_with(&endpoint_prefix),
        "case \"create source endpoint\": expected an ARN under {endpoint_prefix}, got \
         {source_arn:?}"
    );
    assert_eq!(
        text(&created_source, "/Endpoint/Status"),
        "active",
        "case \"create source endpoint\": expected an endpoint to be created active, got \
         {created_source}"
    );
    assert_eq!(
        text(&created_source, "/Endpoint/SslMode"),
        "none",
        "case \"create source endpoint\": expected the documented default SSL mode, got \
         {created_source}"
    );
    assert!(
        !created_source.to_string().contains("never-echoed"),
        "case \"create source endpoint\": the password must never be echoed, got \
         {created_source}"
    );

    let created_target = emulator
        .ok(
            "create target endpoint",
            "CreateEndpoint",
            &json!({
                "EndpointIdentifier": "tgt-ep",
                "EndpointType": "target",
                "EngineName": "postgres",
                "ServerName": "warehouse.example.internal",
                "Port": 5432,
                "SslMode": "require",
            }),
        )
        .await;
    let target_arn = text(&created_target, "/Endpoint/EndpointArn");
    assert_eq!(
        text(&created_target, "/Endpoint/SslMode"),
        "require",
        "case \"create target endpoint\": expected the requested SSL mode, got \
         {created_target}"
    );
    assert_ne!(
        source_arn, target_arn,
        "case \"create target endpoint\": two endpoints must not share an ARN"
    );

    let described_sources = emulator
        .ok(
            "describe source endpoints",
            "DescribeEndpoints",
            &json!({ "Filters": [{ "Name": "endpoint-type", "Values": ["source"] }] }),
        )
        .await;
    assert_eq!(
        column(&described_sources, "/Endpoints", "EndpointArn"),
        vec![source_arn.clone()],
        "case \"describe source endpoints\": expected only the source endpoint, got \
         {described_sources}"
    );
    assert_eq!(
        described_sources
            .pointer("/Endpoints/0/Port")
            .and_then(Value::as_i64),
        Some(3306),
        "case \"describe source endpoints\": expected the stored port, got \
         {described_sources}"
    );

    // ---- CreateReplicationTask -----------------------------------------------------
    let settings = json!({ "TargetMetadata": { "SupportLobs": true } }).to_string();
    let created_task = emulator
        .ok(
            "create task",
            "CreateReplicationTask",
            &json!({
                "ReplicationTaskIdentifier": "Nightly-Load",
                "SourceEndpointArn": source_arn,
                "TargetEndpointArn": target_arn,
                "ReplicationInstanceArn": instance_arn,
                "MigrationType": "full-load-and-cdc",
                "TableMappings": table_mappings(),
                "ReplicationTaskSettings": settings,
            }),
        )
        .await;
    let task_arn = text(&created_task, "/ReplicationTask/ReplicationTaskArn");
    let task_prefix = format!("arn:aws:dms:{REGION}:{ACCOUNT}:task:");
    assert!(
        task_arn.starts_with(&task_prefix),
        "case \"create task\": expected an ARN under {task_prefix}, got {task_arn:?}"
    );
    assert_eq!(
        text(&created_task, "/ReplicationTask/Status"),
        "creating",
        "case \"create task\": expected a freshly created task to be creating, got \
         {created_task}"
    );
    assert!(
        created_task
            .pointer("/ReplicationTask/ReplicationTaskCreationDate")
            .is_some(),
        "case \"create task\": expected a ReplicationTaskCreationDate, got {created_task}"
    );

    // A task that is still being created cannot be started.
    let (status, response) = emulator
        .call(
            "StartReplicationTask",
            &json!({
                "ReplicationTaskArn": task_arn,
                "StartReplicationTaskType": "start-replication",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).contains("InvalidResourceStateFault")),
        (400, true),
        "case \"start a creating task\": expected 400 InvalidResourceStateFault, got \
         {status} {response}"
    );

    wait_for_transition();
    let described_tasks = emulator
        .ok("describe tasks", "DescribeReplicationTasks", &json!({}))
        .await;
    assert_eq!(
        column(&described_tasks, "/ReplicationTasks", "ReplicationTaskArn"),
        vec![task_arn.clone()],
        "case \"describe tasks\": expected exactly the created task, got {described_tasks}"
    );
    for (member, expected) in [
        ("ReplicationTaskIdentifier", "Nightly-Load".to_string()),
        ("SourceEndpointArn", source_arn.clone()),
        ("TargetEndpointArn", target_arn.clone()),
        ("ReplicationInstanceArn", instance_arn.clone()),
        ("MigrationType", "full-load-and-cdc".to_string()),
        ("TableMappings", table_mappings()),
        ("ReplicationTaskSettings", settings.clone()),
        ("Status", "ready".to_string()),
    ] {
        assert_eq!(
            text(&described_tasks, &format!("/ReplicationTasks/0/{member}")),
            expected,
            "case \"describe tasks\": expected {member} to come back from state, got \
             {described_tasks}"
        );
    }

    // `WithoutSettings` drops the settings and nothing else.
    let without_settings = emulator
        .ok(
            "describe tasks without settings",
            "DescribeReplicationTasks",
            &json!({ "WithoutSettings": true }),
        )
        .await;
    assert!(
        without_settings
            .pointer("/ReplicationTasks/0/ReplicationTaskSettings")
            .is_none(),
        "case \"describe tasks without settings\": expected no ReplicationTaskSettings, \
         got {without_settings}"
    );
    assert_eq!(
        text(&without_settings, "/ReplicationTasks/0/TableMappings"),
        table_mappings(),
        "case \"describe tasks without settings\": expected the table mappings to stay, \
         got {without_settings}"
    );

    // ---- StartReplicationTask ------------------------------------------------------
    let started = emulator
        .ok(
            "start task",
            "StartReplicationTask",
            &json!({
                "ReplicationTaskArn": task_arn,
                "StartReplicationTaskType": "start-replication",
            }),
        )
        .await;
    assert_eq!(
        text(&started, "/ReplicationTask/Status"),
        "starting",
        "case \"start task\": expected the task to move to starting, got {started}"
    );
    assert!(
        started
            .pointer("/ReplicationTask/ReplicationTaskStartDate")
            .is_some(),
        "case \"start task\": expected a ReplicationTaskStartDate, got {started}"
    );

    let after_start = emulator
        .ok(
            "describe after start",
            "DescribeReplicationTasks",
            &json!({ "Filters": [{ "Name": "replication-task-arn", "Values": [task_arn] }] }),
        )
        .await;
    assert_eq!(
        column(&after_start, "/ReplicationTasks", "Status"),
        vec!["starting".to_string()],
        "case \"describe after start\": expected the persisted status to be starting, got \
         {after_start}"
    );

    wait_for_transition();
    let running = emulator
        .ok("task settles", "DescribeReplicationTasks", &json!({}))
        .await;
    assert_eq!(
        column(&running, "/ReplicationTasks", "Status"),
        vec!["running".to_string()],
        "case \"task settles\": expected the task to reach running, got {running}"
    );

    // A running task cannot be deleted: it has to be stopped first.
    let (status, response) = emulator
        .call(
            "DeleteReplicationTask",
            &json!({ "ReplicationTaskArn": task_arn }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).contains("InvalidResourceStateFault")),
        (400, true),
        "case \"delete a running task\": expected 400 InvalidResourceStateFault, got \
         {status} {response}"
    );

    // ---- DeleteReplicationTask -----------------------------------------------------
    let second = emulator
        .ok(
            "create second task",
            "CreateReplicationTask",
            &json!({
                "ReplicationTaskIdentifier": "backfill-load",
                "SourceEndpointArn": source_arn,
                "TargetEndpointArn": target_arn,
                "ReplicationInstanceArn": instance_arn,
                "MigrationType": "full-load",
                "TableMappings": table_mappings(),
            }),
        )
        .await;
    let second_arn = text(&second, "/ReplicationTask/ReplicationTaskArn");

    let deleted = emulator
        .ok(
            "delete second task",
            "DeleteReplicationTask",
            &json!({ "ReplicationTaskArn": second_arn }),
        )
        .await;
    assert_eq!(
        (
            text(&deleted, "/ReplicationTask/ReplicationTaskArn"),
            text(&deleted, "/ReplicationTask/Status")
        ),
        (second_arn.clone(), "deleting".to_string()),
        "case \"delete second task\": expected the deleted task to come back deleting, got \
         {deleted}"
    );

    wait_for_transition();
    let remaining = emulator
        .ok("describe after delete", "DescribeReplicationTasks", &json!({}))
        .await;
    assert_eq!(
        column(&remaining, "/ReplicationTasks", "ReplicationTaskArn"),
        vec![task_arn.clone()],
        "case \"describe after delete\": expected only the running task to remain, got \
         {remaining}"
    );

    // The identifier is free again, and deleting the same ARN twice finds nothing.
    let (status, response) = emulator
        .call(
            "DeleteReplicationTask",
            &json!({ "ReplicationTaskArn": second_arn }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).contains("ResourceNotFoundFault")),
        (400, true),
        "case \"delete twice\": expected 400 ResourceNotFoundFault, got {status} {response}"
    );
}

#[tokio::test]
async fn an_arn_that_names_no_task_is_refused_as_not_found() {
    let emulator = Emulator::new();
    let source_arn = emulator.endpoint("src-ep", "source", "mysql").await;

    // Every ARN here is one the store does not hold: a well-formed task ARN nobody
    // created, the same resource id in another region, an ARN of another resource type,
    // and a string that is not an ARN at all.
    let cases = [
        (
            "well-formed but unknown",
            format!("arn:aws:dms:{REGION}:{ACCOUNT}:task:ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        ),
        (
            "another region",
            format!("arn:aws:dms:eu-west-1:{ACCOUNT}:task:ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        ),
        ("an endpoint ARN", source_arn),
        ("not an ARN", "nightly-load".to_string()),
    ];

    for (case, arn) in cases {
        for (operation, body) in [
            (
                "StartReplicationTask",
                json!({
                    "ReplicationTaskArn": arn,
                    "StartReplicationTaskType": "start-replication",
                }),
            ),
            (
                "DeleteReplicationTask",
                json!({ "ReplicationTaskArn": arn }),
            ),
        ] {
            let (status, response) = emulator.call(operation, &body).await;
            assert_eq!(
                (status, error_code(&response).contains("ResourceNotFoundFault")),
                (400, true),
                "case {case:?} on {operation}: expected 400 ResourceNotFoundFault, got \
                 {status} {response}"
            );
        }
    }
}

#[tokio::test]
async fn a_task_can_only_be_created_against_resources_that_exist() {
    let emulator = Emulator::new();
    let instance_arn = emulator.available_instance("task-deps").await;
    let source_arn = emulator.endpoint("src-ep", "source", "mysql").await;
    let target_arn = emulator.endpoint("tgt-ep", "target", "postgres").await;
    let unknown = format!("arn:aws:dms:{REGION}:{ACCOUNT}:endpoint:NOSUCHENDPOINT0000000000AA");
    let unknown_instance = format!("arn:aws:dms:{REGION}:{ACCOUNT}:rep:NOSUCHINSTANCE0000000000AA");

    // Each row breaks exactly one of the three ARN resolutions.
    let cases = [
        (
            "unknown source endpoint",
            unknown.clone(),
            target_arn.clone(),
            instance_arn.clone(),
            "ResourceNotFoundFault",
        ),
        (
            "unknown target endpoint",
            source_arn.clone(),
            unknown.clone(),
            instance_arn.clone(),
            "ResourceNotFoundFault",
        ),
        (
            "unknown replication instance",
            source_arn.clone(),
            target_arn.clone(),
            unknown_instance,
            "ResourceNotFoundFault",
        ),
        (
            "source and target swapped",
            target_arn.clone(),
            source_arn.clone(),
            instance_arn.clone(),
            "ValidationException",
        ),
    ];

    for (case, source, target, instance, expected) in cases {
        let (status, response) = emulator
            .call(
                "CreateReplicationTask",
                &json!({
                    "ReplicationTaskIdentifier": "nightly-load",
                    "SourceEndpointArn": source,
                    "TargetEndpointArn": target,
                    "ReplicationInstanceArn": instance,
                    "MigrationType": "full-load",
                    "TableMappings": table_mappings(),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).contains(expected)),
            (400, true),
            "case {case:?}: expected 400 {expected}, got {status} {response}"
        );
    }

    // A task cannot be created on an instance that is still provisioning.
    let creating = emulator
        .ok(
            "instance still creating",
            "CreateReplicationInstance",
            &json!({
                "ReplicationInstanceIdentifier": "still-creating",
                "ReplicationInstanceClass": "dms.t3.medium",
            }),
        )
        .await;
    let creating_arn = text(&creating, "/ReplicationInstance/ReplicationInstanceArn");
    let (status, response) = emulator
        .call(
            "CreateReplicationTask",
            &json!({
                "ReplicationTaskIdentifier": "nightly-load",
                "SourceEndpointArn": source_arn,
                "TargetEndpointArn": target_arn,
                "ReplicationInstanceArn": creating_arn,
                "MigrationType": "full-load",
                "TableMappings": table_mappings(),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).contains("InvalidResourceStateFault")),
        (400, true),
        "case \"instance still creating\": expected 400 InvalidResourceStateFault, got \
         {status} {response}"
    );
}

#[tokio::test]
async fn a_duplicate_identifier_is_refused_on_every_create() {
    let emulator = Emulator::new();
    let instance_arn = emulator.available_instance("rep-dup").await;
    let source_arn = emulator.endpoint("src-dup", "source", "mysql").await;
    let target_arn = emulator.endpoint("tgt-dup", "target", "postgres").await;
    emulator
        .ok(
            "first task",
            "CreateReplicationTask",
            &json!({
                "ReplicationTaskIdentifier": "task-dup",
                "SourceEndpointArn": source_arn,
                "TargetEndpointArn": target_arn,
                "ReplicationInstanceArn": instance_arn,
                "MigrationType": "cdc",
                "TableMappings": table_mappings(),
            }),
        )
        .await;

    // One row per resource kind this slice creates; the second create of the same
    // identifier is refused, and the first resource keeps its ARN.
    let cases = [
        (
            "replication instance",
            "CreateReplicationInstance",
            json!({
                "ReplicationInstanceIdentifier": "REP-DUP",
                "ReplicationInstanceClass": "dms.t3.large",
            }),
        ),
        (
            "endpoint",
            "CreateEndpoint",
            json!({
                "EndpointIdentifier": "src-dup",
                "EndpointType": "source",
                "EngineName": "oracle",
            }),
        ),
        (
            "replication task",
            "CreateReplicationTask",
            json!({
                "ReplicationTaskIdentifier": "task-dup",
                "SourceEndpointArn": source_arn,
                "TargetEndpointArn": target_arn,
                "ReplicationInstanceArn": instance_arn,
                "MigrationType": "cdc",
                "TableMappings": table_mappings(),
            }),
        ),
    ];

    for (case, operation, body) in cases {
        let (status, response) = emulator.call(operation, &body).await;
        assert_eq!(
            (
                status,
                error_code(&response).contains("ResourceAlreadyExistsFault")
            ),
            (400, true),
            "case {case:?}: expected 400 ResourceAlreadyExistsFault, got {status} {response}"
        );
    }

    // Nothing was added by the refused creates.
    for (pointer, operation) in [
        ("/ReplicationInstances", "DescribeReplicationInstances"),
        ("/Endpoints", "DescribeEndpoints"),
        ("/ReplicationTasks", "DescribeReplicationTasks"),
    ] {
        let described = emulator.ok("after duplicates", operation, &json!({})).await;
        let count = described
            .pointer(pointer)
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let expected = usize::from(operation == "DescribeEndpoints") + 1;
        assert_eq!(
            count, expected,
            "case \"after duplicates\": expected {operation} to report {expected} \
             resources, got {described}"
        );
    }
}

/// One identifier, and whether the documented identifier rule accepts it.
struct IdentifierCase {
    name: &'static str,
    identifier: String,
    valid: bool,
    /// Whether the row tests a length bound, which only the replication-instance and
    /// replication-task identifiers publish.
    length_bound: bool,
}

#[tokio::test]
async fn an_identifier_must_satisfy_the_documented_character_rule() {
    // The rule the `ReplicationInstanceIdentifier`, `EndpointIdentifier` and
    // `ReplicationTaskIdentifier` docs publish: ASCII letters, digits and hyphens only,
    // a letter first, no trailing hyphen and no two consecutive hyphens.
    let cases = vec![
        IdentifierCase {
            name: "lowercase word",
            identifier: "myrepinstance".to_string(),
            valid: true,
            length_bound: false,
        },
        IdentifierCase {
            name: "hyphenated",
            identifier: "sales-to-warehouse".to_string(),
            valid: true,
            length_bound: false,
        },
        IdentifierCase {
            name: "digits inside",
            identifier: "db2zos-7".to_string(),
            valid: true,
            length_bound: false,
        },
        IdentifierCase {
            name: "mixed case",
            identifier: "NightlyLoad".to_string(),
            valid: true,
            length_bound: false,
        },
        IdentifierCase {
            name: "single letter",
            identifier: "a".to_string(),
            valid: true,
            length_bound: false,
        },
        IdentifierCase {
            name: "empty",
            identifier: String::new(),
            valid: false,
            length_bound: false,
        },
        IdentifierCase {
            name: "leading digit",
            identifier: "9lives".to_string(),
            valid: false,
            length_bound: false,
        },
        IdentifierCase {
            name: "leading hyphen",
            identifier: "-lead".to_string(),
            valid: false,
            length_bound: false,
        },
        IdentifierCase {
            name: "trailing hyphen",
            identifier: "trail-".to_string(),
            valid: false,
            length_bound: false,
        },
        IdentifierCase {
            name: "two consecutive hyphens",
            identifier: "two--hyphens".to_string(),
            valid: false,
            length_bound: false,
        },
        IdentifierCase {
            name: "underscore",
            identifier: "has_underscore".to_string(),
            valid: false,
            length_bound: false,
        },
        IdentifierCase {
            name: "dot",
            identifier: "has.dot".to_string(),
            valid: false,
            length_bound: false,
        },
        IdentifierCase {
            name: "non-ascii letter",
            identifier: "caf\u{e9}".to_string(),
            valid: false,
            length_bound: false,
        },
        // The published bound, and one character past it: "Must contain 1-63
        // alphanumeric characters or hyphens" for a replication instance.
        IdentifierCase {
            name: "63 characters",
            identifier: format!("a{}", "b".repeat(62)),
            valid: true,
            length_bound: true,
        },
        IdentifierCase {
            name: "64 characters",
            identifier: format!("a{}", "b".repeat(63)),
            valid: false,
            length_bound: true,
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        let (status, response) = emulator
            .call(
                "CreateReplicationInstance",
                &json!({
                    "ReplicationInstanceIdentifier": case.identifier,
                    "ReplicationInstanceClass": "dms.t3.medium",
                }),
            )
            .await;
        if case.valid {
            assert_eq!(
                status, 200,
                "case {:?}: expected CreateReplicationInstance to accept {:?}, got {status} \
                 {response}",
                case.name, case.identifier
            );
            assert_eq!(
                text(&response, "/ReplicationInstance/ReplicationInstanceIdentifier"),
                case.identifier.to_lowercase(),
                "case {:?}: expected the identifier stored lowercased, got {response}",
                case.name
            );
        } else {
            assert_eq!(
                (status, error_code(&response).contains("ValidationException")),
                (400, true),
                "case {:?}: expected CreateReplicationInstance to refuse {:?} with 400 \
                 ValidationException, got {status} {response}",
                case.name,
                case.identifier
            );
        }

        // The same rule governs an endpoint identifier, whose doc publishes no length
        // bound, so the length rows are skipped there.
        if case.length_bound {
            continue;
        }
        let (status, response) = emulator
            .call(
                "CreateEndpoint",
                &json!({
                    "EndpointIdentifier": case.identifier,
                    "EndpointType": "source",
                    "EngineName": "mysql",
                }),
            )
            .await;
        let expected_status = if case.valid { 200 } else { 400 };
        assert_eq!(
            status, expected_status,
            "case {:?}: expected CreateEndpoint to answer {expected_status} for {:?}, got \
             {status} {response}",
            case.name, case.identifier
        );
    }
}

#[tokio::test]
async fn a_resource_identifier_names_the_last_arn_field() {
    let emulator = Emulator::new();

    // "The value for this parameter can have up to 31 characters" — the published
    // bound, and one character past it.
    let at_bound = format!("Example-App{}", "A".repeat(RESOURCE_IDENTIFIER_MAX_LEN - 11));
    let past_bound = format!("{at_bound}A");
    assert_eq!(
        at_bound.chars().count(),
        RESOURCE_IDENTIFIER_MAX_LEN,
        "the at-bound case must be exactly {RESOURCE_IDENTIFIER_MAX_LEN} characters"
    );

    let created = emulator
        .ok(
            "resource identifier at the bound",
            "CreateReplicationInstance",
            &json!({
                "ReplicationInstanceIdentifier": "named-suffix",
                "ReplicationInstanceClass": "dms.t3.medium",
                "ResourceIdentifier": at_bound,
            }),
        )
        .await;
    assert_eq!(
        text(&created, "/ReplicationInstance/ReplicationInstanceArn"),
        format!("arn:aws:dms:{REGION}:{ACCOUNT}:rep:{at_bound}"),
        "case \"resource identifier at the bound\": expected the ARN to end in the \
         requested resource identifier, got {created}"
    );

    for (case, operation, body) in [
        (
            "one character past the bound",
            "CreateReplicationInstance",
            json!({
                "ReplicationInstanceIdentifier": "too-long-suffix",
                "ReplicationInstanceClass": "dms.t3.medium",
                "ResourceIdentifier": past_bound,
            }),
        ),
        (
            "trailing hyphen",
            "CreateEndpoint",
            json!({
                "EndpointIdentifier": "bad-suffix-ep",
                "EndpointType": "source",
                "EngineName": "mysql",
                "ResourceIdentifier": "Example-App-",
            }),
        ),
    ] {
        let (status, response) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&response).contains("ValidationException")),
            (400, true),
            "case {case:?}: expected 400 ValidationException, got {status} {response}"
        );
    }
}

#[tokio::test]
async fn max_records_is_bounded_and_a_marker_pages_the_scan() {
    let emulator = Emulator::new();

    // The range every paginated DMS describe publishes for `MaxRecords`: "Default: 100"
    // and "Constraints: Minimum 20, maximum 100". Each published bound is tested, and
    // one value past each.
    let bounds = [
        ("no MaxRecords", None, 200),
        ("one below the minimum", Some(MAX_RECORDS_MIN - 1), 400),
        ("the published minimum", Some(MAX_RECORDS_MIN), 200),
        ("the published maximum", Some(MAX_RECORDS_MAX), 200),
        ("one above the maximum", Some(MAX_RECORDS_MAX + 1), 400),
        ("zero", Some(0), 400),
        ("negative", Some(-1), 400),
    ];
    for (case, max_records, expected) in bounds {
        for operation in [
            "DescribeReplicationInstances",
            "DescribeEndpoints",
            "DescribeReplicationTasks",
        ] {
            let body = match max_records {
                Some(max_records) => json!({ "MaxRecords": max_records }),
                None => json!({}),
            };
            let (status, response) = emulator.call(operation, &body).await;
            assert_eq!(
                status, expected,
                "case {case:?} on {operation}: expected {expected}, got {status} {response}"
            );
            if expected == 400 {
                assert!(
                    error_code(&response).contains("ValidationException"),
                    "case {case:?} on {operation}: expected a ValidationException, got \
                     {response}"
                );
            }
        }
    }

    // One more endpoint than a minimum-sized page holds, so the scan has to resume.
    let page = usize::try_from(MAX_RECORDS_MIN).expect("a positive page size");
    let total = page + 5;
    let mut created = Vec::new();
    for index in 0..total {
        created.push(
            emulator
                .endpoint(&format!("page-ep-{index}"), "source", "mysql")
                .await,
        );
    }

    let first = emulator
        .ok(
            "first page",
            "DescribeEndpoints",
            &json!({ "MaxRecords": MAX_RECORDS_MIN }),
        )
        .await;
    let first_arns = column(&first, "/Endpoints", "EndpointArn");
    assert_eq!(
        first_arns.len(),
        page,
        "case \"first page\": expected {page} endpoints, got {first}"
    );
    let marker = first
        .get("Marker")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !marker.is_empty(),
        "case \"first page\": expected a Marker when records remain, got {first}"
    );
    assert!(
        !marker.contains(':'),
        "case \"first page\": expected an opaque Marker, got {marker:?}"
    );

    let second = emulator
        .ok(
            "second page",
            "DescribeEndpoints",
            &json!({ "MaxRecords": MAX_RECORDS_MIN, "Marker": marker }),
        )
        .await;
    let second_arns = column(&second, "/Endpoints", "EndpointArn");
    assert_eq!(
        second_arns.len(),
        total - page,
        "case \"second page\": expected the remaining {} endpoints, got {second}",
        total - page
    );
    assert!(
        second.get("Marker").is_none(),
        "case \"second page\": expected no Marker on the final page, got {second}"
    );

    let paged: Vec<String> = first_arns.into_iter().chain(second_arns).collect();
    assert_eq!(
        paged, created,
        "case \"paged scan\": expected the pages to cover every endpoint exactly once, \
         in creation order, got {paged:?}"
    );

    // A single page the size of the whole scan carries no marker.
    let whole = emulator
        .ok(
            "one page for the whole scan",
            "DescribeEndpoints",
            &json!({ "MaxRecords": MAX_RECORDS_MAX }),
        )
        .await;
    assert_eq!(
        column(&whole, "/Endpoints", "EndpointArn").len(),
        total,
        "case \"one page for the whole scan\": expected every endpoint, got {whole}"
    );
    assert!(
        whole.get("Marker").is_none(),
        "case \"one page for the whole scan\": expected no Marker, got {whole}"
    );

    // A marker this emulator did not mint is refused rather than silently ignored.
    for (case, marker) in [
        ("not hex", "page-ep-0"),
        ("hex that names nothing", "6e6f742d616e2d61726e"),
    ] {
        let (status, response) = emulator
            .call(
                "DescribeEndpoints",
                &json!({ "MaxRecords": MAX_RECORDS_MIN, "Marker": marker }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).contains("ValidationException")),
            (400, true),
            "case {case:?}: expected 400 ValidationException, got {status} {response}"
        );
    }
}

#[tokio::test]
async fn every_documented_filter_narrows_the_describe() {
    let emulator = Emulator::new();
    let instance_arn = emulator.available_instance("filter-rep").await;
    let other_instance_arn = emulator.available_instance("other-rep").await;
    let source_arn = emulator.endpoint("filter-src", "source", "mysql").await;
    let target_arn = emulator.endpoint("filter-tgt", "target", "postgres").await;
    let task = emulator
        .ok(
            "filter task",
            "CreateReplicationTask",
            &json!({
                "ReplicationTaskIdentifier": "filter-task",
                "SourceEndpointArn": source_arn,
                "TargetEndpointArn": target_arn,
                "ReplicationInstanceArn": instance_arn,
                "MigrationType": "cdc",
                "TableMappings": table_mappings(),
            }),
        )
        .await;
    let task_arn = text(&task, "/ReplicationTask/ReplicationTaskArn");

    // Every filter name the three `Filters` docs publish, each with a value that
    // matches and one that does not.
    let cases: Vec<(&str, &str, &str, String, Vec<String>)> = vec![
        (
            "DescribeReplicationInstances",
            "/ReplicationInstances",
            "replication-instance-arn",
            instance_arn.clone(),
            vec![instance_arn.clone()],
        ),
        (
            "DescribeReplicationInstances",
            "/ReplicationInstances",
            "replication-instance-id",
            "other-rep".to_string(),
            vec![other_instance_arn.clone()],
        ),
        (
            "DescribeReplicationInstances",
            "/ReplicationInstances",
            "replication-instance-class",
            "dms.t3.medium".to_string(),
            vec![instance_arn.clone(), other_instance_arn.clone()],
        ),
        (
            "DescribeReplicationInstances",
            "/ReplicationInstances",
            "replication-instance-class",
            "dms.c4.xlarge".to_string(),
            Vec::new(),
        ),
        (
            "DescribeReplicationInstances",
            "/ReplicationInstances",
            "engine-version",
            "0.0.0".to_string(),
            Vec::new(),
        ),
        (
            "DescribeEndpoints",
            "/Endpoints",
            "endpoint-arn",
            target_arn.clone(),
            vec![target_arn.clone()],
        ),
        (
            "DescribeEndpoints",
            "/Endpoints",
            "endpoint-type",
            "target".to_string(),
            vec![target_arn.clone()],
        ),
        (
            "DescribeEndpoints",
            "/Endpoints",
            "endpoint-id",
            "filter-src".to_string(),
            vec![source_arn.clone()],
        ),
        (
            "DescribeEndpoints",
            "/Endpoints",
            "engine-name",
            "mysql".to_string(),
            vec![source_arn.clone()],
        ),
        (
            "DescribeEndpoints",
            "/Endpoints",
            "engine-name",
            "sqlserver".to_string(),
            Vec::new(),
        ),
        (
            "DescribeReplicationTasks",
            "/ReplicationTasks",
            "replication-task-arn",
            task_arn.clone(),
            vec![task_arn.clone()],
        ),
        (
            "DescribeReplicationTasks",
            "/ReplicationTasks",
            "replication-task-id",
            "filter-task".to_string(),
            vec![task_arn.clone()],
        ),
        (
            "DescribeReplicationTasks",
            "/ReplicationTasks",
            "migration-type",
            "cdc".to_string(),
            vec![task_arn.clone()],
        ),
        (
            "DescribeReplicationTasks",
            "/ReplicationTasks",
            "migration-type",
            "full-load".to_string(),
            Vec::new(),
        ),
        (
            "DescribeReplicationTasks",
            "/ReplicationTasks",
            "endpoint-arn",
            target_arn.clone(),
            vec![task_arn.clone()],
        ),
        (
            "DescribeReplicationTasks",
            "/ReplicationTasks",
            "replication-instance-arn",
            other_instance_arn.clone(),
            Vec::new(),
        ),
    ];

    for (operation, pointer, name, value, expected) in cases {
        let member = match operation {
            "DescribeReplicationInstances" => "ReplicationInstanceArn",
            "DescribeEndpoints" => "EndpointArn",
            _ => "ReplicationTaskArn",
        };
        // DMS raises its one modelled error rather than answering an empty list when
        // the filters narrow a describe to nothing. The two describes that do so each
        // have their own fixed message.
        let no_match_message = match operation {
            "DescribeReplicationInstances" => Some("No instances found matching provided filters"),
            "DescribeEndpoints" => Some("No Endpoints found matching provided filters"),
            _ => None,
        };
        if expected.is_empty() {
            if let Some(message) = no_match_message {
                let (status, response) = emulator
                    .call(
                        operation,
                        &json!({ "Filters": [{ "Name": name, "Values": [value] }] }),
                    )
                    .await;
                assert_eq!(
                    (
                        status,
                        error_code(&response),
                        text(&response, "/message").as_str()
                    ),
                    (400, "ResourceNotFoundFault".to_string(), message),
                    "case {name:?}={value:?} on {operation}: expected 400 ResourceNotFoundFault \
                     {message:?}, got {status} {response}"
                );
                continue;
            }
        }
        let described = emulator
            .ok(
                &format!("{name}={value}"),
                operation,
                &json!({ "Filters": [{ "Name": name, "Values": [value] }] }),
            )
            .await;
        assert_eq!(
            column(&described, pointer, member),
            expected,
            "case {name:?}={value:?} on {operation}: expected {expected:?}, got {described}"
        );
    }

    // A filter name no doc publishes is refused, rather than ignored — which would
    // answer with resources the caller asked to exclude.
    for operation in [
        "DescribeReplicationInstances",
        "DescribeEndpoints",
        "DescribeReplicationTasks",
    ] {
        let (status, response) = emulator
            .call(
                operation,
                &json!({ "Filters": [{ "Name": "endpoint-colour", "Values": ["blue"] }] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).contains("ValidationException")),
            (400, true),
            "case \"undocumented filter\" on {operation}: expected 400 ValidationException, \
             got {status} {response}"
        );
    }
}
