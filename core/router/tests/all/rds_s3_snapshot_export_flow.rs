// SPDX-License-Identifier: CC0-1.0

//! The documented Amazon RDS SNAPSHOT-EXPORT-TO-S3 flow, end to end across the services it
//! spans.
//!
//! RULE: every call of the export flow answers in bounded time and settles into the state
//! the next step waits for. A client creates the destination (an S3 bucket, a KMS key and an
//! IAM role the export service principal can assume), provisions the source (a DB subnet
//! group over two Availability Zones, an Aurora DB cluster and a `db.serverless` member
//! instance), seeds the cluster through the RDS Data API — in the dialect of the cluster's
//! own engine, idempotently, as a client that retries its seeding step writes it — snapshots
//! it, starts the export and polls it to a terminal status; and then the exported objects are
//! in the bucket under `<S3Prefix>/<ExportTaskIdentifier>/`. No step of that sequence may
//! hang or fail to settle: a client has nothing to branch on if it does.
//!
//! Only a cross-service suite can pin this: the flow is served by seven services over ONE
//! engine (EC2 for the network the subnet group is over, Amazon RDS for the control plane,
//! the RDS Data API for the data plane, Secrets Manager for the credential that call reads,
//! KMS and IAM for the destination's key and role, and Amazon S3 for the objects), exactly
//! as `core/router/src/lib.rs` mounts them.
//!
//! Every call goes over the wire, on a router with request authentication off as every suite
//! in this crate runs it. Assembling the whole router costs stack (see `boot_serve.rs`), so
//! it is built — and the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{element, form_encode, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call below is signed for.
const REGION: &str = "us-west-2";

/// The destination bucket the export writes into.
const BUCKET: &str = "rds-export-flow-destination";

/// The prefix the export writes under, which is where the objects are then looked for.
const PREFIX: &str = "exports";

/// The export task the flow starts.
const EXPORT_TASK: &str = "flow-export-task";

/// The cluster the flow provisions, and the database it carries.
const CLUSTER: &str = "flow-export-cluster";
/// See [`CLUSTER`].
const DATABASE: &str = "flowdata";
/// The table the flow seeds through the Data API.
const TABLE: &str = "export_rows";

/// The trust policy the export role carries: the document that lets Amazon RDS's export
/// service principal assume it.
const EXPORT_TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"export.rds.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The policy the export role carries, as the user guide's export role does: the object
/// writes and the bucket read an export of a snapshot to Amazon S3 performs.
const EXPORT_ROLE_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"#,
    r#"["s3:PutObject*","s3:GetObject*","s3:ListBucket","s3:DeleteObject*","#,
    r#""s3:GetBucketLocation"],"Resource":["#,
    r#""arn:aws:s3:::rds-export-flow-destination","#,
    r#""arn:aws:s3:::rds-export-flow-destination/*"]}]}"#
);

/// How long the flow waits for a transition the RDS control plane owes before reading the
/// resource back: well over this emulator's own span, and nothing here sleeps for longer.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(600);

/// How many times the flow reads an export task while waiting for the terminal status it
/// polls for: the task walks two spans (`STARTING` -> `IN_PROGRESS` -> `COMPLETE`), and a
/// few more reads than that leave room on a loaded machine without waiting on a task that
/// is never going to settle.
const TERMINAL_STATUS_READS: usize = 6;

/// What one call answered.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// Assert the call was served, and hand back its body.
    fn served(self, case: &str) -> String {
        assert_eq!(
            self.status, 200,
            "case {case:?}: the call must be served, got HTTP {} with {}",
            self.status, self.body
        );
        self.body
    }
}

/// One call of `method` to the service `signing_name` at `path`, with `body`.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: String,
) -> Answer {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header(
            // The signature is never checked: this router runs with authentication off.
            // What the scope carries is the Region and the service a handler reads.
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body: String::from_utf8_lossy(&body).to_string(),
    }
}

/// One awsQuery call to the service `signing_name`, as an SDK client sends one.
fn query(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, String)],
) -> Answer {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        runtime,
        signing_name,
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        body,
    )
}

/// One awsQuery call to Amazon RDS.
fn rds(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, String)],
) -> Answer {
    query(router, runtime, "rds", "2014-10-31", action, params)
}

/// `[(name, value.to_string()), ..]` — the parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The export flow converges: every call answers, every state the flow waits for arrives,
/// and the exported objects are in the destination bucket.
#[test]
fn the_snapshot_export_to_s3_flow_converges() {
    std::thread::Builder::new()
        .name("rds-s3-export-flow".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            // `enable_all`, which is what the emulator's own boot path builds: a DB
            // instance create opens the instance's engine listener through the core, and
            // binding a socket needs the runtime's I/O driver.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // ---- the destination: a bucket, a key and a role ----
            dispatch(
                &router,
                &runtime,
                "s3",
                "PUT",
                &format!("/{BUCKET}"),
                &[],
                format!(
                    "<CreateBucketConfiguration><LocationConstraint>{REGION}\
                     </LocationConstraint></CreateBucketConfiguration>"
                ),
            )
            .served("create the destination bucket");
            let key = dispatch(
                &router,
                &runtime,
                "kms",
                "POST",
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.1"),
                    ("x-amz-target", "TrentService.CreateKey"),
                ],
                "{}".to_string(),
            )
            .served("create the export's KMS key");
            let key: Value = serde_json::from_str(&key).expect("KMS answers JSON");
            let kms_key_id = key["KeyMetadata"]["KeyId"]
                .as_str()
                .unwrap_or_else(|| panic!("KMS reports the key it minted: {key}"))
                .to_string();
            let role = query(
                &router,
                &runtime,
                "iam",
                "2010-05-08",
                "CreateRole",
                &params![
                    "RoleName" => "flow-export-role",
                    "AssumeRolePolicyDocument" => EXPORT_TRUST_POLICY,
                ],
            )
            .served("create the export role");
            let role_arn = element(&role, "Arn");
            assert!(
                role_arn.starts_with("arn:aws:iam::"),
                "IAM reports the role's ARN, got {role_arn:?} in {role}"
            );
            // The role's own policy: an export writes the objects under it, so it carries
            // the object writes the user guide's export role carries.
            query(
                &router,
                &runtime,
                "iam",
                "2010-05-08",
                "PutRolePolicy",
                &params![
                    "RoleName" => "flow-export-role",
                    "PolicyName" => "flow-export-to-s3",
                    "PolicyDocument" => EXPORT_ROLE_POLICY,
                ],
            )
            .served("attach the export role's policy");

            // ---- the network the DB subnet group is over ----
            let vpc = query(
                &router,
                &runtime,
                "ec2",
                "2016-11-15",
                "CreateVpc",
                &params!["CidrBlock" => "10.42.0.0/16"],
            )
            .served("create the VPC");
            let vpc_id = element(&vpc, "vpcId");
            let mut subnets = Vec::new();
            for (index, zone) in ["us-west-2a", "us-west-2b"].iter().enumerate() {
                let subnet = query(
                    &router,
                    &runtime,
                    "ec2",
                    "2016-11-15",
                    "CreateSubnet",
                    &params![
                        "VpcId" => vpc_id,
                        "CidrBlock" => format!("10.42.{index}.0/24"),
                        "AvailabilityZone" => zone,
                    ],
                )
                .served("create a subnet");
                subnets.push(element(&subnet, "subnetId"));
            }

            // ---- the source: a subnet group, an Aurora cluster and a member instance ----
            rds(
                &router,
                &runtime,
                "CreateDBSubnetGroup",
                &params![
                    "DBSubnetGroupName" => "flow-export-subnets",
                    "DBSubnetGroupDescription" => "the export flow's subnets",
                    "SubnetIds.SubnetIdentifier.1" => subnets[0],
                    "SubnetIds.SubnetIdentifier.2" => subnets[1],
                ],
            )
            .served("create the DB subnet group");
            let cluster = rds(
                &router,
                &runtime,
                "CreateDBCluster",
                &params![
                    "DBClusterIdentifier" => CLUSTER,
                    "Engine" => "aurora-mysql",
                    "MasterUsername" => "flowadmin",
                    "MasterUserPassword" => "flow-Secret-99",
                    "DatabaseName" => DATABASE,
                    "EnableHttpEndpoint" => "true",
                    "ServerlessV2ScalingConfiguration.MinCapacity" => "0.5",
                    "ServerlessV2ScalingConfiguration.MaxCapacity" => "2.0",
                    "StorageEncrypted" => "true",
                    "BackupRetentionPeriod" => "1",
                    "DBSubnetGroupName" => "flow-export-subnets",
                ],
            )
            .served("create the DB cluster");
            let cluster_arn = element(&cluster, "DBClusterArn");
            rds(
                &router,
                &runtime,
                "CreateDBInstance",
                &params![
                    "DBInstanceIdentifier" => "flow-export-instance",
                    "Engine" => "aurora-mysql",
                    "DBInstanceClass" => "db.serverless",
                    "DBClusterIdentifier" => CLUSTER,
                ],
            )
            .served("create the member DB instance");

            // The first state the flow waits for: the member instance is available.
            std::thread::sleep(SETTLE);
            let instance = rds(
                &router,
                &runtime,
                "DescribeDBInstances",
                &params!["DBInstanceIdentifier" => "flow-export-instance"],
            )
            .served("describe the member DB instance");
            assert_eq!(
                element(&instance, "DBInstanceStatus"),
                "available",
                "the member instance must settle to available, got {instance}"
            );

            // ---- the data: seeded through the Data API's HTTP endpoint ----
            // The credential the Data API reads the cluster's user from is a real secret of
            // this emulator's Secrets Manager, minted here and named by its own ARN — which
            // is what the Data API resolves the call against.
            let secret = dispatch(
                &router,
                &runtime,
                "secretsmanager",
                "POST",
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.1"),
                    ("x-amz-target", "secretsmanager.CreateSecret"),
                ],
                serde_json::json!({
                    "Name": "flow-export",
                    // An SDK generates this idempotency token itself; a raw request carries
                    // its own.
                    "ClientRequestToken": "7f1b0c8a-5d42-4a1e-9c3b-2e6f8a9d0b11",
                    "SecretString": "{\"username\":\"flowadmin\",\"password\":\"flow-Secret-99\"}",
                })
                .to_string(),
            )
            .served("create the cluster's credential secret");
            let secret: Value = serde_json::from_str(&secret).expect("Secrets Manager answers JSON");
            let secret_arn = secret["ARN"]
                .as_str()
                .unwrap_or_else(|| panic!("Secrets Manager reports the secret it minted: {secret}"))
                .to_string();
            // The seed is written the way a client of an Aurora MySQL cluster writes an
            // idempotent one, and run TWICE, as a client that retries its seeding step
            // does: the upsert stores the row and then updates the row it conflicts with.
            for statement in [
                format!("CREATE TABLE IF NOT EXISTS {TABLE} (id INT PRIMARY KEY, val VARCHAR(50))"),
                format!(
                    "INSERT INTO {TABLE} (id, val) VALUES (1, 'alpha') \
                     ON DUPLICATE KEY UPDATE val = VALUES(val)"
                ),
                format!(
                    "INSERT INTO {TABLE} (id, val) VALUES (1, 'alpha') \
                     ON DUPLICATE KEY UPDATE val = VALUES(val)"
                ),
            ] {
                let seeded = dispatch(
                    &router,
                    &runtime,
                    "rds-data",
                    "POST",
                    "/Execute",
                    &[("content-type", "application/json")],
                    serde_json::json!({
                        "resourceArn": cluster_arn,
                        "secretArn": secret_arn,
                        "database": DATABASE,
                        "sql": statement,
                    })
                    .to_string(),
                );
                seeded.served("seed the cluster through the Data API");
            }

            // ---- the snapshot the export reads ----
            rds(
                &router,
                &runtime,
                "CreateDBClusterSnapshot",
                &params![
                    "DBClusterSnapshotIdentifier" => "flow-export-snapshot",
                    "DBClusterIdentifier" => CLUSTER,
                ],
            )
            .served("snapshot the DB cluster");
            std::thread::sleep(SETTLE);
            let snapshot = rds(
                &router,
                &runtime,
                "DescribeDBClusterSnapshots",
                &params!["DBClusterSnapshotIdentifier" => "flow-export-snapshot"],
            )
            .served("describe the DB cluster snapshot");
            assert_eq!(
                element(&snapshot, "Status"),
                "available",
                "the cluster snapshot must settle to available, got {snapshot}"
            );
            let snapshot_arn = element(&snapshot, "DBClusterSnapshotArn");

            // ---- the export ----
            let started = rds(
                &router,
                &runtime,
                "StartExportTask",
                &params![
                    "ExportTaskIdentifier" => EXPORT_TASK,
                    "SourceArn" => snapshot_arn,
                    "S3BucketName" => BUCKET,
                    "S3Prefix" => PREFIX,
                    "IamRoleArn" => role_arn,
                    "KmsKeyId" => kms_key_id,
                ],
            )
            .served("start the export task");
            assert_eq!(
                element(&started, "Status"),
                "STARTING",
                "an export just accepted is STARTING, got {started}"
            );

            // The last state the flow waits for: a terminal export status, which a client
            // polls for and which is COMPLETE for an export that wrote its objects. The
            // task walks STARTING -> IN_PROGRESS -> COMPLETE, so the poll reads it until it
            // is terminal rather than once.
            let mut described = String::new();
            for _ in 0..TERMINAL_STATUS_READS {
                std::thread::sleep(SETTLE);
                described = rds(
                    &router,
                    &runtime,
                    "DescribeExportTasks",
                    &params!["ExportTaskIdentifier" => EXPORT_TASK],
                )
                .served("describe the export task");
                if ["COMPLETE", "FAILED", "CANCELED"].contains(&element(&described, "Status").as_str())
                {
                    break;
                }
            }
            assert_eq!(
                element(&described, "Status"),
                "COMPLETE",
                "the export must settle to COMPLETE, got {described}"
            );
            assert_eq!(
                opt_element(&described, "FailureCause"),
                None,
                "an export that completed carries no failure cause, got {described}"
            );

            // ---- and the objects are in the bucket ----
            let listed = dispatch(
                &router,
                &runtime,
                "s3",
                "GET",
                &format!("/{BUCKET}?list-type=2&prefix={PREFIX}%2F"),
                &[],
                String::new(),
            )
            .served("list the destination prefix");
            // The file Amazon RDS writes beside an export's data, naming what it exported:
            // `export_tables_info_<id>_from_1_to_<n>.json` under the export's own folder.
            let tables_info = format!("{PREFIX}/{EXPORT_TASK}/export_tables_info_{EXPORT_TASK}_");
            assert!(
                listed.contains(&tables_info),
                "the export writes the file that says what it exported, under \
                 {tables_info:?}, got {listed}"
            );
        })
        .expect("the sized thread starts")
        .join()
        .expect("the flow runs to its assertions");
}
