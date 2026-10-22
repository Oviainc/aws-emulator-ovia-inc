// SPDX-License-Identifier: CC0-1.0

//! The emulator the suites beside this file drive, and the request helpers they share.
//!
//! Application Auto Scaling does not stand alone: a scalable target is a capacity dimension
//! of a resource that lives in ANOTHER service, and a target-tracking policy's alarms live
//! in CloudWatch. So the emulator built here holds the three services over ONE engine —
//! exactly what `core/router` assembles — and every call goes through the production
//! `MultiServiceRouter::dispatch`, so what the suites assert is what a signing AWS client
//! would see.
#![allow(dead_code)]

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every test's SigV4 credential scope names; every minted ARN is scoped to it.
pub const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
pub const ACCOUNT: &str = "000000000000";

/// One emulator over one shared engine.
pub struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// Application Auto Scaling, DynamoDB and CloudWatch on one engine, with each name in
    /// `tables` created as a PROVISIONED DynamoDB table of one read and one write unit.
    ///
    /// A scalable target is a capacity dimension OF A RESOURCE, so registering one resolves
    /// that resource in the service that owns it: a suite whose rows register a `dynamodb`
    /// target names the tables those rows need here, rather than relying on a list another
    /// suite maintains.
    pub async fn with_tables(tables: &[&str]) -> Self {
        let emulator = Self::empty();
        for table in tables {
            emulator.create_provisioned_table(table, 1, 1).await;
        }
        emulator
    }

    /// The same emulator with no resource created yet, for a suite whose rows create the
    /// resources they need (or deliberately do not).
    pub fn empty() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suites drive unsigned requests; the `Authorization` header is still what
        // routes each request and names its region, as a signed client's would.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// Call an Application Auto Scaling operation.
    pub async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.dispatch(
            "application-autoscaling",
            "application/x-amz-json-1.1",
            &format!("AnyScaleFrontendService.{operation}"),
            body,
        )
        .await
    }

    /// Call a DynamoDB operation.
    pub async fn call_dynamodb(&self, operation: &str, body: Value) -> (u16, Value) {
        self.dispatch(
            "dynamodb",
            "application/x-amz-json-1.0",
            &format!("DynamoDB_20120810.{operation}"),
            body,
        )
        .await
    }

    /// Call a CloudWatch operation.
    pub async fn call_cloudwatch(&self, operation: &str, body: Value) -> (u16, Value) {
        self.dispatch(
            "monitoring",
            "application/x-amz-json-1.0",
            &format!("GraniteServiceVersion20100801.{operation}"),
            body,
        )
        .await
    }

    /// Dispatch one request, returning the HTTP status and the parsed response.
    async fn dispatch(
        &self,
        signing_name: &str,
        content_type: &str,
        target: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", content_type)
            .header("x-amz-target", target)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{signing_name}\
                     /aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a PROVISIONED DynamoDB table with `read` and `write` capacity units.
    pub async fn create_provisioned_table(&self, name: &str, read: i64, write: i64) {
        self.create_table(
            name,
            json!({"BillingMode": "PROVISIONED",
                   "ProvisionedThroughput": {"ReadCapacityUnits": read, "WriteCapacityUnits": write}}),
        )
        .await;
    }

    /// Create a PAY_PER_REQUEST (on-demand) DynamoDB table, which has no provisioned
    /// capacity at all.
    pub async fn create_on_demand_table(&self, name: &str) {
        self.create_table(name, json!({"BillingMode": "PAY_PER_REQUEST"}))
            .await;
    }

    /// Create a PROVISIONED table carrying one global secondary index on `index_key`.
    pub async fn create_table_with_index(
        &self,
        name: &str,
        index: &str,
        index_key: &str,
        read: i64,
        write: i64,
    ) {
        self.create_table(
            name,
            json!({
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": read, "WriteCapacityUnits": write},
                "AttributeDefinitions": [
                    {"AttributeName": "pk", "AttributeType": "S"},
                    {"AttributeName": index_key, "AttributeType": "S"},
                ],
                "GlobalSecondaryIndexes": [{
                    "IndexName": index,
                    "KeySchema": [{"AttributeName": index_key, "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": read, "WriteCapacityUnits": write,
                    },
                }],
            }),
        )
        .await;
    }

    /// Create a table from `overrides` merged over the one-attribute default.
    async fn create_table(&self, name: &str, overrides: Value) {
        let mut request = json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        });
        for (member, value) in overrides.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        let (status, body) = self.call_dynamodb("CreateTable", request).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateTable to answer 200, got {status} {body:?}"
        );
    }

    /// Register a scalable target on `dimension`, creating the resource it names where this
    /// emulator owns the service that resource lives in, and answer the three identity
    /// members of the target so a caller can address it.
    ///
    /// The resource id is the form that dimension's resource type documents
    /// ([`resource_id_for`]), so one call works for every modelled dimension.
    pub async fn given_target(&self, dimension: &str, min: i64, max: i64) -> Value {
        let namespace = dimension.split(':').next().unwrap_or_default();
        let resource_id = resource_id_for(dimension);
        if namespace == "dynamodb" {
            let rest = resource_id.trim_start_matches("table/");
            match rest.split_once("/index/") {
                Some((table, index)) => {
                    if !self.table_exists(table).await {
                        self.create_table_with_index(table, index, "status", 4, 4).await;
                    }
                }
                None => {
                    if !self.table_exists(rest).await {
                        self.create_provisioned_table(rest, 4, 4).await;
                    }
                }
            }
        }
        let target = json!({
            "ServiceNamespace": namespace,
            "ResourceId": resource_id,
            "ScalableDimension": dimension,
        });
        let mut registration = target.clone();
        registration["MinCapacity"] = json!(min);
        registration["MaxCapacity"] = json!(max);
        let (status, body) = self.call("RegisterScalableTarget", registration).await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
        );
        target
    }

    /// Whether the DynamoDB store already holds a table by that name.
    async fn table_exists(&self, name: &str) -> bool {
        let (status, _) = self
            .call_dynamodb("DescribeTable", json!({"TableName": name}))
            .await;
        status == 200
    }

    /// Every alarm CloudWatch holds under the `TargetTracking` prefix, by alarm name.
    pub async fn target_tracking_alarms(&self) -> Vec<Value> {
        let (status, body) = self
            .call_cloudwatch(
                "DescribeAlarms",
                json!({"AlarmNamePrefix": ALARM_NAME_PREFIX, "MaxRecords": 100}),
            )
            .await;
        assert_eq!(
            status, 200,
            "expected DescribeAlarms to answer 200, got {status} {body:?}"
        );
        let mut alarms = body["MetricAlarms"].as_array().cloned().unwrap_or_default();
        alarms.sort_by_key(|alarm| alarm["AlarmName"].as_str().unwrap_or_default().to_string());
        alarms
    }
}

/// The prefix Application Auto Scaling gives every alarm it mints, which is what lets a
/// client find them with `DescribeAlarms`.
pub const ALARM_NAME_PREFIX: &str = "TargetTracking";

/// The resource-type suffix a namespace's service-linked role name carries.
///
/// Source: "Service-linked roles for Application Auto Scaling" in the Application Auto
/// Scaling User Guide, which publishes one role per supported service, each named
/// `AWSServiceRoleForApplicationAutoScaling_<suffix>`. Spelled out here rather than read
/// from the emulator's own table, so an assertion is against the PUBLISHED name.
pub fn service_linked_role_suffix(namespace: &str) -> &'static str {
    match namespace {
        "appstream" => "AppStreamFleet",
        "cassandra" => "CassandraTable",
        "comprehend" => "ComprehendEndpoint",
        "custom-resource" => "CustomResource",
        "dynamodb" => "DynamoDBTable",
        "ec2" => "EC2SpotFleetRequest",
        "ecs" => "ECSService",
        "elasticache" => "ElastiCacheRG",
        "elasticmapreduce" => "EMRInstanceGroup",
        "kafka" => "KafkaCluster",
        "lambda" => "LambdaConcurrency",
        "neptune" => "NeptuneCluster",
        "rds" => "RDSCluster",
        "sagemaker" => "SageMakerEndpoint",
        "workspaces" => "WorkSpacesPool",
        other => panic!("case {other:?}: no service-linked role is published for it"),
    }
}

/// A `ResourceId` of the form the resource type in `dimension` documents.
///
/// One row per resource type the model's own `ResourceId` documentation gives an example
/// for (`smithy_query(query="doc",shape="com.amazonaws.applicationautoscaling#RegisterScalableTargetRequest",member="ResourceId")`),
/// so a suite that walks the `ScalableDimension` enum can name a resource for every value
/// of it instead of carrying its own table.
pub fn resource_id_for(dimension: &str) -> String {
    match dimension {
        "appstream:fleet:DesiredCapacity" => "fleet/sample-fleet".to_string(),
        "cassandra:table:ReadCapacityUnits" | "cassandra:table:WriteCapacityUnits" => {
            "keyspace/events/table/clicks".to_string()
        }
        "comprehend:document-classifier-endpoint:DesiredInferenceUnits" => format!(
            "arn:aws:comprehend:{REGION}:{ACCOUNT}:document-classifier-endpoint/classifier"
        ),
        "comprehend:entity-recognizer-endpoint:DesiredInferenceUnits" => format!(
            "arn:aws:comprehend:{REGION}:{ACCOUNT}:entity-recognizer-endpoint/recognizer"
        ),
        "custom-resource:ResourceType:Property" => "custom-resource/my-resource".to_string(),
        "dynamodb:table:ReadCapacityUnits" | "dynamodb:table:WriteCapacityUnits" => {
            "table/catalog".to_string()
        }
        "dynamodb:index:ReadCapacityUnits" | "dynamodb:index:WriteCapacityUnits" => {
            "table/catalog-indexed/index/by-status".to_string()
        }
        "ec2:spot-fleet-request:TargetCapacity" => {
            "spot-fleet-request/sfr-73fbd2ce-aa30-494c-8788-1cee4EXAMPLE".to_string()
        }
        "ecs:service:DesiredCount" => "service/prod-cluster/web-app".to_string(),
        "elasticache:cache-cluster:Nodes" => "cache-cluster/sessions".to_string(),
        "elasticache:replication-group:NodeGroups"
        | "elasticache:replication-group:Replicas" => "replication-group/sessions".to_string(),
        "elasticmapreduce:instancegroup:InstanceCount" => {
            "instancegroup/j-2EEZNYKUA1NTV/ig-1791Y4E1L8YI0".to_string()
        }
        "kafka:broker-storage:VolumeSize" => format!(
            "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/events/abcd1234-1234-abcd-5678-cdef0123ab01-2"
        ),
        "lambda:function:ProvisionedConcurrency" => "function:ingest:green".to_string(),
        "neptune:cluster:ReadReplicaCount" => "cluster:graphdb".to_string(),
        "rds:cluster:ReadReplicaCount" => "cluster:orders-db".to_string(),
        "sagemaker:inference-component:DesiredCopyCount" => {
            "inference-component/ranker".to_string()
        }
        "sagemaker:variant:DesiredInstanceCount"
        | "sagemaker:variant:DesiredProvisionedConcurrency" => {
            "endpoint/rank/variant/blue".to_string()
        }
        "workspaces:workspacespool:DesiredUserSessions" => {
            "workspacespool/wspool-9f8e7d".to_string()
        }
        other => panic!("case {other:?}: this harness names no resource for it"),
    }
}

/// Which of a policy's alarms an alarm name is: the segment between the resource id and the
/// opaque id, as `TargetTracking-<resource id>-<kind>-<id>`.
pub fn alarm_kind(alarm: &Value) -> String {
    let name = alarm["AlarmName"].as_str().unwrap_or_default();
    let mut segments = name.rsplitn(3, '-');
    segments.next();
    segments.next().unwrap_or_default().to_string()
}

/// The kinds of the alarms in `alarms`, sorted, as `DescribeAlarms` reported them.
pub fn alarm_kinds(alarms: &[Value]) -> Vec<String> {
    let mut kinds: Vec<String> = alarms.iter().map(alarm_kind).collect();
    kinds.sort();
    kinds
}

/// The `__type` discriminator an awsJson error body carries, without its namespace.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// The `Message` an error body carries, whichever casing the protocol rendered it in.
pub fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
