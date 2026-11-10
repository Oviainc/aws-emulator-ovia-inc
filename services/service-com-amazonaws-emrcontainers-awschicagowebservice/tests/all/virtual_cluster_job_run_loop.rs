// SPDX-License-Identifier: CC0-1.0

//! The EMR on EKS virtual-cluster / job-run core loop, end to end.
//!
//! Every call here goes through the production path: this one service is mounted in the
//! real [`MultiServiceRouter`] and driven with restJson1 HTTP requests, so routing,
//! deserialization, constraint validation, the handler and serialization all run exactly
//! as they do for an AWS SDK client.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! 1. A create persists the resource and a describe answers from that record: every
//!    member the request carried comes back, and the ones it omitted stay absent.
//! 2. An `@idempotencyToken` replayed names the resource the first call minted.
//! 3. A status transition the clock owes is observable in both phases: the transient
//!    state right after the call, the settled state once the span has passed.
//! 4. A resource is resolved out of state before it is used, and a request naming one
//!    that is not there (or not usable) is refused with the modelled error.
//! 5. `@paginated` paging walks the stored set with no repeats and no gaps, and reports
//!    `nextToken` only while runs remain.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::ops::types::{
    JOB_RUN_STATE_VALUES, VIRTUAL_CLUSTER_STATE_VALUES,
};
use service_com_amazonaws_emrcontainers_awschicagowebservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for. Deliberately not the emulator's default,
/// so an ARN that carried a hardcoded region instead of the caller's would fail.
pub(crate) const REGION: &str = "us-west-2";

/// What one call answered.
///
/// `pub(crate)` because this suite's HTTP harness is the one the other suites in this
/// test binary drive the service through too (`tests/all.rs` compiles them all into one
/// crate), so there is a single request path under test rather than one copy per suite.
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The error shape restJson1 names in `x-amzn-errortype`.
    pub(crate) error_code: Option<String>,
    pub(crate) body: Value,
}

impl Answer {
    /// The body of a call that must have succeeded.
    pub(crate) fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            self.status, self.body
        );
        &self.body
    }

    /// Assert this call was refused with `code` at `status`.
    pub(crate) fn refused(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            self.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {}",
            self.status, self.body
        );
        assert_eq!(
            self.error_code.as_deref(),
            Some(code),
            "case {case:?}: expected error code {code:?}, got {:?} with body {}",
            self.error_code,
            self.body
        );
    }
}

/// The emulator under test.
pub(crate) struct Emr {
    router: MultiServiceRouter,
    /// The one shared engine the mounted service was constructed from, kept so a suite
    /// that has to stage a condition no request reaches can reach this service's store
    /// ([`Self::store`]).
    engine: Arc<engine::InMemoryEngine>,
}

impl Emr {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // This suite signs nothing, so pin authentication off rather than inheriting the
        // process-wide switch.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// This service's store in that engine — the very one every handler above reads and
    /// writes.
    ///
    /// For a fixture that has to stage a state the API offers no call for;
    /// [`crate::job_run_completion`] stages compute that goes away behind a job run
    /// nobody cancelled. Everything a request can set up is set up through a request.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<state::ComAmazonawsEmrcontainersAwschicagowebserviceState> {
        state::service_state(&*self.engine).expect("the emr-containers store is the engine's")
    }

    pub(crate) async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        self.send_in(REGION, method, path, body).await
    }

    /// The same call, signed for `region`: the regional endpoint it is addressed to and
    /// the region of its SigV4 credential scope. `crate::region_isolation` drives more
    /// than one region through this one request path.
    pub(crate) async fn send_in(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Answer {
        let payload = match &body {
            Some(value) => serde_json::to_vec(value).expect("a JSON request body serializes"),
            None => Vec::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://emr-containers.{region}.amazonaws.com{path}"))
            .header("host", format!("emr-containers.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/emr-containers/aws4_request, SignedHeaders=host, Signature=abc"
                ),
            )
            .body(Bytes::from(payload))
            .expect("the test request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_code,
            body,
        }
    }

    async fn create_cluster(&self, body: Value) -> Answer {
        self.send("POST", "/virtualclusters", Some(body)).await
    }

    pub(crate) async fn describe_cluster(&self, id: &str) -> Answer {
        self.send("GET", &format!("/virtualclusters/{id}"), None).await
    }

    pub(crate) async fn delete_cluster(&self, id: &str) -> Answer {
        self.send("DELETE", &format!("/virtualclusters/{id}"), None)
            .await
    }

    pub(crate) async fn start_job(&self, cluster: &str, body: Value) -> Answer {
        self.send(
            "POST",
            &format!("/virtualclusters/{cluster}/jobruns"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn describe_job(&self, cluster: &str, id: &str) -> Answer {
        self.send(
            "GET",
            &format!("/virtualclusters/{cluster}/jobruns/{id}"),
            None,
        )
        .await
    }

    async fn cancel_job(&self, cluster: &str, id: &str) -> Answer {
        self.send(
            "DELETE",
            &format!("/virtualclusters/{cluster}/jobruns/{id}"),
            None,
        )
        .await
    }

    async fn list_jobs(&self, cluster: &str, query: &str) -> Answer {
        self.send(
            "GET",
            &format!("/virtualclusters/{cluster}/jobruns{query}"),
            None,
        )
        .await
    }

    /// Create one plain virtual cluster and return its id.
    pub(crate) async fn cluster(&self, name: &str, token: &str) -> String {
        let answer = self
            .create_cluster(json!({
                "name": name,
                "clientToken": token,
                "containerProvider": {"id": "eks-fixture", "type": "EKS"},
            }))
            .await;
        text(answer.ok(name), "/id", name)
    }
}

/// One `StartJobRun` body that a cluster can actually run: a submission naming no job
/// template has to carry `executionRoleArn`, `releaseLabel` and `jobDriver` itself, the
/// three members `JobTemplateData` marks `@required` (smithy_query: shape
/// `JobTemplateData`). The driver is the caller's, so one definition of an acceptable
/// submission serves every `JobDriver` member.
pub(crate) fn runnable_body(token: &str, driver: Value) -> Value {
    json!({
        "clientToken": token,
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
        "releaseLabel": "emr-6.10.0-latest",
        "jobDriver": driver,
    })
}

/// The `sparkSubmitJobDriver` member of the `JobDriver` union (smithy_query: shape
/// `JobDriver`), the driver the fixtures here submit.
pub(crate) fn spark_submit_driver() -> Value {
    json!({"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}})
}

/// Wait until a status transition the service owes has come due, with margin, so the
/// next call materialises it. The service stores the due instant and never sleeps; this
/// is the test's own clock.
fn wait_for_the_clock() {
    std::thread::sleep(state::TRANSITION_SPAN + Duration::from_millis(250));
}

pub(crate) fn field<'a>(value: &'a Value, pointer: &str, case: &str) -> &'a Value {
    value
        .pointer(pointer)
        .unwrap_or_else(|| panic!("case {case:?}: response carries no {pointer}: {value}"))
}

pub(crate) fn text(value: &Value, pointer: &str, case: &str) -> String {
    field(value, pointer, case)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {pointer} is not a string: {value}"))
        .to_string()
}

/// A `ResourceIdString` is `^[0-9a-z]+$`, length 1..64 (the model, via `OPERATIONS.md`).
pub(crate) fn assert_resource_id(case: &str, what: &str, id: &str) {
    let shaped = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase());
    assert!(
        shaped,
        "case {case:?}: {what} {id:?} is not a ResourceIdString (len 1..64, ^[0-9a-z]+$)"
    );
}

/// The millisecond instant a `createdAt` / `finishedAt` names.
///
/// restJson1 carries a timestamp either as an ISO-8601 string or as epoch seconds,
/// depending on the member's `@timestampFormat`; this accepts whichever the wire used, so
/// the assertion is about the INSTANT and not about the framework's rendering of it.
pub(crate) fn instant_millis(value: &Value, pointer: &str, case: &str) -> i64 {
    let found = field(value, pointer, case);
    if let Some(rendered) = found.as_str() {
        let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
            panic!("case {case:?}: {pointer} {rendered:?} is not an ISO-8601 instant: {error}")
        });
        state::millis_of(&parsed)
    } else if let Some(seconds) = found.as_f64() {
        state::millis_of(&DateTime::from_secs_f64(seconds))
    } else {
        panic!("case {case:?}: {pointer} is not a timestamp: {value}")
    }
}

/// An instant as a query-string value: ISO-8601 with its colons percent-encoded.
pub(crate) fn query_instant(millis: i64) -> String {
    state::date_time(millis)
        .fmt(Format::DateTime)
        .expect("a stored instant renders as date-time")
        .replace(':', "%3A")
}

/// One `CreateVirtualCluster` request, and what the describe of it must answer.
struct ClusterCase {
    label: &'static str,
    body: Value,
}

/// RULE 1 + the ARN rule: a created virtual cluster is read back out of state, with the
/// members the request carried and without the ones it omitted.
///
/// The rows differ along everything `CreateVirtualCluster` generalises over: which
/// optional members are present, the shape of `containerProvider.info`, the form of the
/// name, and how many tags there are.
#[tokio::test]
async fn a_created_virtual_cluster_is_described_from_state() {
    let emr = Emr::new();
    // `securityConfigurationId` is a REFERENCE to a security configuration this service
    // stores, so the row that carries one names a configuration that really exists (the
    // refusal of one that does not is `crate::virtual_cluster_references`' rule).
    let security_configuration = text(
        emr.send(
            "POST",
            "/securityconfigurations",
            Some(json!({
                "name": "vc-loop-config",
                "clientToken": "tok-vc-loop-config",
                "securityConfigurationData": {},
            })),
        )
        .await
        .ok("a security configuration for the cluster to reference"),
        "/id",
        "a security configuration for the cluster to reference",
    );
    let cases = vec![
        ClusterCase {
            label: "only the required members",
            body: json!({
                "name": "minimal",
                "clientToken": "tok-minimal",
                "containerProvider": {"id": "eks-minimal", "type": "EKS"},
            }),
        },
        ClusterCase {
            label: "tags, a namespace and sessionEnabled",
            body: json!({
                "name": "tagged.cluster",
                "clientToken": "tok-tagged",
                "containerProvider": {
                    "id": "eks-tagged",
                    "type": "EKS",
                    "info": {"eksInfo": {"namespace": "emr-jobs"}},
                },
                "tags": {"team": "data", "env": "test"},
                "sessionEnabled": true,
            }),
        },
        ClusterCase {
            label: "a node label, a scheduler configuration and a security configuration id",
            body: json!({
                "name": "deep/name_1#x",
                "clientToken": "tok-deep",
                "containerProvider": {
                    "id": "eks_deep-9",
                    "type": "EKS",
                    "info": {"eksInfo": {"namespace": "emr", "nodeLabel": "spark-nodes"}},
                },
                "tags": {"only": "one"},
                "sessionEnabled": false,
                "securityConfigurationId": security_configuration,
                "schedulerConfiguration": {"maxConcurrentJobRuns": 7, "maxInQueueJobRuns": 250},
            }),
        },
    ];

    let before = state::now_millis();
    let mut ids = BTreeSet::new();
    for case in &cases {
        let label = case.label;
        let created = emr.create_cluster(case.body.clone()).await;
        let created = created.ok(label).clone();
        let id = text(&created, "/id", label);
        assert_resource_id(label, "the minted virtual cluster id", &id);
        assert!(
            ids.insert(id.clone()),
            "case {label:?}: id {id:?} was minted twice"
        );

        let expected_arn = format!(
            "arn:aws:emr-containers:{REGION}:{}:/virtualclusters/{id}",
            engine::emulator_account_id()
        );
        assert_eq!(
            text(&created, "/arn", label),
            expected_arn,
            "case {label:?}: CreateVirtualCluster answered the wrong arn"
        );
        assert_eq!(
            created.get("name"),
            case.body.get("name"),
            "case {label:?}: CreateVirtualCluster answered the wrong name"
        );

        let described = emr.describe_cluster(&id).await;
        let cluster = field(described.ok(label), "/virtualCluster", label).clone();
        assert_eq!(
            text(&cluster, "/id", label),
            id,
            "case {label:?}: the described cluster is not the created one"
        );
        assert_eq!(
            text(&cluster, "/arn", label),
            expected_arn,
            "case {label:?}: the described arn is not the created one"
        );
        assert_eq!(
            text(&cluster, "/state", label),
            "RUNNING",
            "case {label:?}: a created virtual cluster must be RUNNING"
        );
        // Every member the request carried, read back out of the record; every member it
        // omitted still absent.
        for member in [
            "name",
            "containerProvider",
            "tags",
            "sessionEnabled",
            "securityConfigurationId",
            "schedulerConfiguration",
        ] {
            let expected = match (member, case.body.get(member)) {
                // `tags` is a map the service always reports, empty when none were given.
                ("tags", None) => Some(json!({})),
                (_, other) => other.cloned(),
            };
            assert_eq!(
                cluster.get(member).cloned(),
                expected,
                "case {label:?}: {member} did not round-trip through state"
            );
        }

        let created_at = instant_millis(&cluster, "/createdAt", label);
        let after = state::now_millis();
        assert!(
            created_at >= before - 1_000 && created_at <= after + 1_000,
            "case {label:?}: createdAt {created_at} is not within the window [{before}, {after}]"
        );

        // A read must not change what the next reader sees.
        let again = emr.describe_cluster(&id).await;
        assert_eq!(
            field(again.ok(label), "/virtualCluster", label),
            &cluster,
            "case {label:?}: describing a cluster twice answered two different clusters"
        );
    }

    assert_eq!(
        ids.len(),
        cases.len(),
        "every row must have minted its own cluster, got {ids:?}"
    );
}

/// RULE 2: `clientToken` is `@idempotencyToken`, so the same token replayed returns the
/// cluster the first call minted, and a different token mints a new one.
#[tokio::test]
async fn a_replayed_client_token_returns_the_first_virtual_cluster() {
    let emr = Emr::new();
    // Rows differ in the token's form and in whether the bodies beside it differ.
    let cases = [
        ("a plain token", "tok-1", json!({"tags": {"a": "1"}})),
        ("a token with punctuation", "tok/2.a-b_c", json!({})),
        ("a token at the 64-character bound", &"t".repeat(64)[..], json!({"sessionEnabled": true})),
    ];

    let mut ids = BTreeSet::new();
    for (label, token, extra) in &cases {
        let mut body = json!({
            "name": "idempotent",
            "clientToken": token,
            "containerProvider": {"id": "eks-idem", "type": "EKS"},
        });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }

        let first = emr.create_cluster(body.clone()).await;
        let first_id = text(first.ok(label), "/id", label);
        let replay = emr.create_cluster(body).await;
        let replay_id = text(replay.ok(label), "/id", label);
        assert_eq!(
            replay_id, first_id,
            "case {label:?}: replaying clientToken {token:?} minted a second cluster"
        );
        ids.insert(first_id);
    }

    assert_eq!(
        ids.len(),
        cases.len(),
        "each distinct clientToken must mint its own cluster, got {ids:?}"
    );
}

/// One `StartJobRun` request.
struct JobCase {
    label: &'static str,
    body: Value,
}

/// RULE 2 for job runs: `StartJobRun`'s `clientToken` is `@idempotencyToken` scoped to
/// the virtual cluster, so the same token replayed against one cluster returns the run
/// the first call started, while the same token against another cluster is a different
/// submission and starts a run of its own.
#[tokio::test]
async fn a_replayed_job_run_client_token_is_scoped_to_its_virtual_cluster() {
    let emr = Emr::new();
    let first = emr.cluster("jobs-idem-1", "tok-jobs-idem-1").await;
    let second = emr.cluster("jobs-idem-2", "tok-jobs-idem-2").await;
    // Rows differ in the token's form and in what the body carries beside it.
    let cases = [
        ("a plain token", "job-idem-1", json!({})),
        (
            "a token with punctuation",
            "job/idem.2-a_b",
            json!({"name": "idem-job"}),
        ),
        (
            "a token at the 64-character bound",
            &"j".repeat(64)[..],
            json!({"releaseLabel": "emr-7.0.0-latest"}),
        ),
    ];

    let mut ids = BTreeSet::new();
    for (label, token, extra) in &cases {
        let mut body = json!({
            "clientToken": token,
            "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
            "releaseLabel": "emr-6.10.0-latest",
            "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}},
        });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }

        let started = emr.start_job(&first, body.clone()).await;
        let started_id = text(started.ok(label), "/id", label);
        let replay = emr.start_job(&first, body.clone()).await;
        assert_eq!(
            text(replay.ok(label), "/id", label),
            started_id,
            "case {label:?}: replaying clientToken {token:?} on one cluster started a second run"
        );

        let elsewhere = emr.start_job(&second, body).await;
        let elsewhere_id = text(elsewhere.ok(label), "/id", label);
        assert_ne!(
            elsewhere_id, started_id,
            "case {label:?}: the token is scoped per cluster, so another cluster must start its own run"
        );
        ids.insert(started_id);
        ids.insert(elsewhere_id);
    }

    assert_eq!(
        ids.len(),
        cases.len() * 2,
        "each (cluster, clientToken) pair must start its own run, got {ids:?}"
    );
}

/// RULE 1 for job runs: what `StartJobRun` submitted is what `DescribeJobRun` answers,
/// reconstructed from the stored record.
///
/// The rows differ along the job driver's union member, the presence of a name, tags,
/// configuration overrides and a retry policy, and the shape of the driver's own lists.
#[tokio::test]
async fn a_started_job_run_is_described_from_state() {
    let emr = Emr::new();
    let cluster = emr.cluster("jobs", "tok-jobs").await;
    let cases = vec![
        JobCase {
            label: "a bare spark-submit driver",
            body: json!({
                "clientToken": "job-tok-1",
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}},
            }),
        },
        JobCase {
            label: "spark-submit with arguments, parameters, tags and a retry policy",
            body: json!({
                "name": "demo-job",
                "clientToken": "job-tok-2",
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {
                    "entryPoint": "s3://bucket/main.py",
                    "entryPointArguments": ["--input", "s3://bucket/in", "--rows", "10"],
                    "sparkSubmitParameters": "--conf spark.executor.instances=2",
                }},
                "configurationOverrides": {
                    "applicationConfiguration": [{
                        "classification": "spark-defaults",
                        "properties": {"spark.executor.memory": "2G"},
                    }],
                    "monitoringConfiguration": {
                        "s3MonitoringConfiguration": {"logUri": "s3://bucket/logs"},
                    },
                },
                "tags": {"owner": "analytics", "cost-centre": "42"},
                "retryPolicyConfiguration": {"maxAttempts": 3},
            }),
        },
        JobCase {
            label: "a spark-sql driver",
            body: json!({
                "name": "sql_job.1",
                "clientToken": "job-tok-3",
                "executionRoleArn": "arn:aws:iam::000000000000:role/sql/EMRJobRole",
                "releaseLabel": "emr-7.0.0-latest",
                "jobDriver": {"sparkSqlJobDriver": {
                    "entryPoint": "s3://bucket/query.sql",
                    "sparkSqlParameters": "-e select 1",
                }},
            }),
        },
        JobCase {
            label: "a spark-submit driver with an empty argument list",
            body: json!({
                "clientToken": "job-tok-4",
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                "releaseLabel": "emr-6.15.0",
                "jobDriver": {"sparkSubmitJobDriver": {
                    "entryPoint": "s3://bucket/empty.py",
                    "entryPointArguments": [],
                }},
            }),
        },
    ];

    let before = state::now_millis();
    let mut ids = BTreeSet::new();
    for case in &cases {
        let label = case.label;
        let started = emr.start_job(&cluster, case.body.clone()).await;
        let started = started.ok(label).clone();
        let id = text(&started, "/id", label);
        assert_resource_id(label, "the minted job run id", &id);
        assert!(
            ids.insert(id.clone()),
            "case {label:?}: job run id {id:?} was minted twice"
        );

        let expected_arn = format!(
            "arn:aws:emr-containers:{REGION}:{}:/virtualclusters/{cluster}/jobruns/{id}",
            engine::emulator_account_id()
        );
        assert_eq!(
            text(&started, "/arn", label),
            expected_arn,
            "case {label:?}: StartJobRun answered the wrong arn"
        );
        assert_eq!(
            text(&started, "/virtualClusterId", label),
            cluster,
            "case {label:?}: StartJobRun answered the wrong virtualClusterId"
        );
        assert_eq!(
            started.get("name"),
            case.body.get("name"),
            "case {label:?}: StartJobRun answered the wrong name"
        );

        let described = emr.describe_job(&cluster, &id).await;
        let job_run = field(described.ok(label), "/jobRun", label).clone();
        assert_eq!(
            text(&job_run, "/id", label),
            id,
            "case {label:?}: the described run is not the started one"
        );
        assert_eq!(
            text(&job_run, "/arn", label),
            expected_arn,
            "case {label:?}: the described arn is not the started one"
        );
        assert_eq!(
            text(&job_run, "/virtualClusterId", label),
            cluster,
            "case {label:?}: the described run names the wrong virtual cluster"
        );
        assert_eq!(
            text(&job_run, "/state", label),
            "PENDING",
            "case {label:?}: a submitted job run is accepted PENDING"
        );
        assert!(
            job_run.get("finishedAt").is_none(),
            "case {label:?}: a running job run has no finishedAt: {job_run}"
        );
        for member in [
            "name",
            "clientToken",
            "executionRoleArn",
            "releaseLabel",
            "jobDriver",
            "configurationOverrides",
            "tags",
            "retryPolicyConfiguration",
        ] {
            let expected = match (member, case.body.get(member)) {
                ("tags", None) => Some(json!({})),
                (_, other) => other.cloned(),
            };
            assert_eq!(
                job_run.get(member).cloned(),
                expected,
                "case {label:?}: {member} did not round-trip through state"
            );
        }

        let created_at = instant_millis(&job_run, "/createdAt", label);
        let after = state::now_millis();
        assert!(
            created_at >= before - 1_000 && created_at <= after + 1_000,
            "case {label:?}: createdAt {created_at} is not within the window [{before}, {after}]"
        );
    }
}

/// RULE 3: the states a job run moves through are the clock's, so both phases are
/// observable — the transient one right after the call and the settled one after the
/// span — and `finishedAt` appears exactly when the run reaches a terminal state.
#[tokio::test]
async fn a_job_run_reaches_running_and_then_cancelled_on_the_clock() {
    let emr = Emr::new();
    let case = "pending -> running -> cancel_pending -> cancelled";
    let cluster = emr.cluster("lifecycle", "tok-lifecycle").await;
    let started = emr
        .start_job(
            &cluster,
            json!({
                "name": "lifecycle-job",
                "clientToken": "job-lifecycle",
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}},
            }),
        )
        .await;
    let id = text(started.ok(case), "/id", case);

    let described = emr.describe_job(&cluster, &id).await;
    assert_eq!(
        text(field(described.ok(case), "/jobRun", case), "/state", case),
        "PENDING",
        "case {case:?}: the transient state right after StartJobRun must be observable"
    );

    wait_for_the_clock();
    let described = emr.describe_job(&cluster, &id).await;
    let running = field(described.ok(case), "/jobRun", case).clone();
    assert_eq!(
        text(&running, "/state", case),
        "RUNNING",
        "case {case:?}: the clock owed this run RUNNING"
    );
    assert!(
        running.get("finishedAt").is_none(),
        "case {case:?}: a RUNNING job run has no finishedAt: {running}"
    );
    let created_at = instant_millis(&running, "/createdAt", case);

    let cancelled = emr.cancel_job(&cluster, &id).await;
    let cancelled = cancelled.ok(case).clone();
    assert_eq!(
        text(&cancelled, "/id", case),
        id,
        "case {case:?}: CancelJobRun answered the wrong id"
    );
    assert_eq!(
        text(&cancelled, "/virtualClusterId", case),
        cluster,
        "case {case:?}: CancelJobRun answered the wrong virtualClusterId"
    );

    let described = emr.describe_job(&cluster, &id).await;
    assert_eq!(
        text(field(described.ok(case), "/jobRun", case), "/state", case),
        "CANCEL_PENDING",
        "case {case:?}: the transient state right after CancelJobRun must be observable"
    );

    wait_for_the_clock();
    let described = emr.describe_job(&cluster, &id).await;
    let settled = field(described.ok(case), "/jobRun", case).clone();
    assert_eq!(
        text(&settled, "/state", case),
        "CANCELLED",
        "case {case:?}: the clock owed this run CANCELLED"
    );
    let finished_at = instant_millis(&settled, "/finishedAt", case);
    assert!(
        finished_at >= created_at,
        "case {case:?}: finishedAt {finished_at} precedes createdAt {created_at}"
    );

    // Reading a settled run twice answers the same run: nothing is consumed by a read.
    let again = emr.describe_job(&cluster, &id).await;
    assert_eq!(
        field(again.ok(case), "/jobRun", case),
        &settled,
        "case {case:?}: describing a cancelled run twice answered two different runs"
    );

    // A run that has already finished cannot be cancelled again.
    emr.cancel_job(&cluster, &id)
        .await
        .refused("cancelling a finished run", 400, "ValidationException");
}

/// RULE 4: every operation resolves its resource out of state first, and refuses a
/// request that names one which is not there (or, for `StartJobRun`, one that cannot
/// take work) with the error its model declares.
#[tokio::test]
async fn an_unresolvable_resource_is_refused_with_the_modelled_error() {
    let emr = Emr::new();
    let cluster = emr.cluster("resolve", "tok-resolve").await;
    let other = emr.cluster("resolve2", "tok-resolve2").await;
    let started = emr
        .start_job(
            &cluster,
            runnable_body("job-resolve", spark_submit_driver()),
        )
        .await;
    let job = text(started.ok("fixture"), "/id", "fixture");

    let body = json!({"clientToken": "job-nowhere"});
    // StartJobRun: the virtual cluster must exist and be RUNNING.
    for (label, target) in [
        ("a cluster id that was never created", "doesnotexist1"),
        ("a cluster id of the right shape but no record", "0zzzz9"),
    ] {
        emr.start_job(target, body.clone())
            .await
            .refused(label, 400, "ResourceNotFoundException");
    }
    // StartJobRun: a job template id is resolved out of state too, so one naming no
    // stored template is not found; and template parameters have no meaning without one.
    emr.start_job(
        &cluster,
        json!({"clientToken": "job-template", "jobTemplateId": "tmpl123"}),
    )
    .await
    .refused(
        "a job template id naming no stored template",
        400,
        "ResourceNotFoundException",
    );
    emr.start_job(
        &cluster,
        json!({"clientToken": "job-template-params", "jobTemplateParameters": {"rows": "10"}}),
    )
    .await
    .refused(
        "job template parameters with no template",
        400,
        "ValidationException",
    );

    // DescribeJobRun: the run must exist under THAT cluster.
    for (label, target_cluster, target_job) in [
        ("an unknown job run", cluster.as_str(), "doesnotexist1"),
        ("a job run under the wrong cluster", other.as_str(), job.as_str()),
        ("an unknown cluster", "doesnotexist1", job.as_str()),
    ] {
        emr.describe_job(target_cluster, target_job)
            .await
            .refused(label, 400, "ResourceNotFoundException");
    }

    // DescribeVirtualCluster models not-found; Delete and Cancel do not, so they refuse
    // an unresolvable id as an invalid request.
    emr.describe_cluster("doesnotexist1")
        .await
        .refused("describing an unknown cluster", 400, "ResourceNotFoundException");
    emr.delete_cluster("doesnotexist1")
        .await
        .refused("deleting an unknown cluster", 400, "ValidationException");
    emr.cancel_job(&cluster, "doesnotexist1")
        .await
        .refused("cancelling an unknown run", 400, "ValidationException");
    emr.cancel_job(&other, &job)
        .await
        .refused("cancelling a run under the wrong cluster", 400, "ValidationException");
    // ListJobRuns resolves no resource at all: the cluster id SCOPES its collection, so an
    // id naming no cluster selects no run and the listing answers an empty page
    // (`crate::cluster_scoped_listing_scope` holds that rule over both cluster-scoped
    // listings).
    let case = "listing an unknown cluster";
    let listed = emr.list_jobs("doesnotexist1", "").await;
    assert_eq!(
        listed.ok(case).pointer("/jobRuns").and_then(Value::as_array),
        Some(&Vec::new()),
        "case {case:?}: expected an empty page, got {}",
        listed.body
    );
}

/// RULE 5: `ListJobRuns` pages the runs this cluster actually holds, newest first, with
/// no repeats and no gaps at any page size, and reports `nextToken` only while runs
/// remain.
#[tokio::test]
async fn list_job_runs_pages_the_stored_runs_newest_first() {
    let emr = Emr::new();
    let cluster = emr.cluster("paging", "tok-paging").await;
    let mut created = Vec::new();
    for index in 0..5 {
        let started = emr
            .start_job(
                &cluster,
                json!({
                    "name": format!("job-{index}"),
                    "clientToken": format!("job-page-{index}"),
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.10.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}},
                }),
            )
            .await;
        created.push(text(started.ok("fixture"), "/id", "fixture"));
    }

    let whole = emr.list_jobs(&cluster, "").await;
    let whole = whole.ok("the whole list").clone();
    assert!(
        whole.get("nextToken").is_none(),
        "the whole list has no next page: {whole}"
    );
    let order: Vec<String> = field(&whole, "/jobRuns", "the whole list")
        .as_array()
        .expect("jobRuns is a list")
        .iter()
        .map(|run| text(run, "/id", "the whole list"))
        .collect();
    let newest_first: Vec<String> = created.iter().rev().cloned().collect();
    assert_eq!(
        order, newest_first,
        "the whole list must be newest first, got {order:?}"
    );

    // Rows differ in the page size, spanning one item per page up to more than the whole
    // set.
    for page_size in [1_usize, 2, 3, 5, 6] {
        let label = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let page = emr.list_jobs(&cluster, &query).await;
            let page = page.ok(&label).clone();
            let ids: Vec<String> = field(&page, "/jobRuns", &label)
                .as_array()
                .expect("jobRuns is a list")
                .iter()
                .map(|run| text(run, "/id", &label))
                .collect();
            let remaining = order.len() - walked.len();
            assert_eq!(
                ids.len(),
                page_size.min(remaining),
                "case {label:?}: page {pages} returned {} runs, expected {}",
                ids.len(),
                page_size.min(remaining)
            );
            walked.extend(ids);
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                token.is_some(),
                walked.len() < order.len(),
                "case {label:?}: nextToken {token:?} after {} of {} runs",
                walked.len(),
                order.len()
            );
            pages += 1;
            assert!(
                pages <= order.len() + 1,
                "case {label:?}: paging did not terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, order,
            "case {label:?}: paging must walk the whole list once, in order"
        );
    }

    // A page of fewer than one run is not a page, and a token that names no run of the
    // result set is not a cursor into it.
    emr.list_jobs(&cluster, "?maxResults=0")
        .await
        .refused("maxResults=0", 400, "ValidationException");
    emr.list_jobs(&cluster, "?nextToken=notatoken")
        .await
        .refused("a bogus nextToken", 400, "ValidationException");

    // A token a client already holds stays usable when the run it pointed at LEAVES the
    // result set between pages: the cursor names a place in the order, not a row. Here
    // the third run is cancelled after the first page, so it drops out of a
    // `states=RUNNING` listing, and resuming reports the runs after it rather than
    // refusing the token.
    let label = "a token across a state change";
    wait_for_the_clock();
    let first_page = emr.list_jobs(&cluster, "?states=RUNNING&maxResults=2").await;
    let first_page = first_page.ok(label).clone();
    let seen: Vec<String> = field(&first_page, "/jobRuns", label)
        .as_array()
        .expect("jobRuns is a list")
        .iter()
        .map(|run| text(run, "/id", label))
        .collect();
    assert_eq!(
        seen,
        order[0..2].to_vec(),
        "case {label:?}: the first page must be the two newest RUNNING runs"
    );
    let token = text(&first_page, "/nextToken", label);
    emr.cancel_job(&cluster, &order[2])
        .await
        .ok("cancel the run the cursor points at");
    let resumed = emr
        .list_jobs(
            &cluster,
            &format!("?states=RUNNING&maxResults=2&nextToken={token}"),
        )
        .await;
    let resumed = resumed.ok(label).clone();
    let rest: Vec<String> = field(&resumed, "/jobRuns", label)
        .as_array()
        .expect("jobRuns is a list")
        .iter()
        .map(|run| text(run, "/id", label))
        .collect();
    assert_eq!(
        rest,
        order[3..5].to_vec(),
        "case {label:?}: resuming must report the runs after the cursor, with no repeats"
    );
    assert!(
        resumed.get("nextToken").is_none(),
        "case {label:?}: no runs remain after this page: {resumed}"
    );

    // A cursor is scoped to the listing it came from: carried to another cluster's
    // pagination it is not a cursor into that one, and neither is a token this service
    // never issued.
    let other = emr.cluster("paging-other", "tok-paging-other").await;
    emr.list_jobs(&other, &format!("?nextToken={token}"))
        .await
        .refused(
            "a token from another cluster's listing",
            400,
            "ValidationException",
        );
    emr.list_jobs(&cluster, "?nextToken=1-1")
        .await
        .refused("a fabricated numeric token", 400, "ValidationException");
}

/// RULE 5's filters: each one keeps exactly the stored runs it names.
#[tokio::test]
async fn list_job_runs_honours_every_filter() {
    let emr = Emr::new();
    let cluster = emr.cluster("filters", "tok-filters").await;
    let before_everything = state::now_millis() - 1_000;
    let mut created = Vec::new();
    for index in 0..4 {
        let started = emr
            .start_job(
                &cluster,
                json!({
                    "name": format!("filter-{index}"),
                    "clientToken": format!("job-filter-{index}"),
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.10.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}},
                }),
            )
            .await;
        created.push(text(started.ok("fixture"), "/id", "fixture"));
    }
    let after_everything = state::now_millis() + 1_000;

    // One run is cancelled, so the state filter has more than one state to choose from.
    emr.cancel_job(&cluster, &created[0])
        .await
        .ok("cancel the first run");
    wait_for_the_clock();

    // Rows differ along which filter is applied and whether the bound keeps all, some or
    // none of the stored runs.
    let cases: Vec<(&'static str, String, usize)> = vec![
        ("no filter", String::new(), 4),
        ("an exact name", "?name=filter-2".to_string(), 1),
        ("a name nothing carries", "?name=filter-99".to_string(), 0),
        ("one state", "?states=RUNNING".to_string(), 3),
        ("another state", "?states=CANCELLED".to_string(), 1),
        (
            "two states",
            "?states=RUNNING&states=CANCELLED".to_string(),
            4,
        ),
        ("a state nothing is in", "?states=COMPLETED".to_string(), 0),
        (
            "createdAfter before every run",
            format!("?createdAfter={}", query_instant(before_everything)),
            4,
        ),
        (
            "createdAfter after every run",
            format!("?createdAfter={}", query_instant(after_everything)),
            0,
        ),
        (
            "createdBefore after every run",
            format!("?createdBefore={}", query_instant(after_everything)),
            4,
        ),
        (
            "createdBefore before every run",
            format!("?createdBefore={}", query_instant(before_everything)),
            0,
        ),
        (
            "a window around every run",
            format!(
                "?createdAfter={}&createdBefore={}",
                query_instant(before_everything),
                query_instant(after_everything)
            ),
            4,
        ),
        (
            "a name and a state together",
            "?name=filter-0&states=CANCELLED".to_string(),
            1,
        ),
        (
            "a name and a state that disagree",
            "?name=filter-0&states=RUNNING".to_string(),
            0,
        ),
    ];

    for (label, query, expected) in cases {
        let answer = emr.list_jobs(&cluster, &query).await;
        let body = answer.ok(label).clone();
        let got = field(&body, "/jobRuns", label)
            .as_array()
            .expect("jobRuns is a list")
            .len();
        assert_eq!(
            got, expected,
            "case {label:?}: query {query:?} returned {got} runs, expected {expected}"
        );
    }
}

/// RULE 3 + RULE 4 for the cluster: deleting it starts a teardown the clock finishes,
/// the record stays describable throughout, and no job run can start on it again.
#[tokio::test]
async fn a_deleted_virtual_cluster_stays_describable_and_takes_no_more_work() {
    let emr = Emr::new();
    let case = "running -> terminating -> terminated";
    let cluster = emr.cluster("teardown", "tok-teardown").await;
    let started = emr
        .start_job(
            &cluster,
            runnable_body("job-teardown", spark_submit_driver()),
        )
        .await;
    let job = text(started.ok(case), "/id", case);

    let deleted = emr.delete_cluster(&cluster).await;
    assert_eq!(
        text(deleted.ok(case), "/id", case),
        cluster,
        "case {case:?}: DeleteVirtualCluster answered the wrong id"
    );

    let described = emr.describe_cluster(&cluster).await;
    assert_eq!(
        text(
            field(described.ok(case), "/virtualCluster", case),
            "/state",
            case
        ),
        "TERMINATING",
        "case {case:?}: the transient state right after DeleteVirtualCluster must be observable"
    );
    emr.start_job(&cluster, json!({"clientToken": "job-after-delete"}))
        .await
        .refused(
            "starting a job on a terminating cluster",
            400,
            "ResourceNotFoundException",
        );

    wait_for_the_clock();
    let described = emr.describe_cluster(&cluster).await;
    let settled = field(described.ok(case), "/virtualCluster", case).clone();
    assert_eq!(
        text(&settled, "/state", case),
        "TERMINATED",
        "case {case:?}: the clock owed this cluster TERMINATED"
    );
    assert_eq!(
        text(&settled, "/id", case),
        cluster,
        "case {case:?}: a terminated cluster stays describable under its own id"
    );
    emr.start_job(&cluster, json!({"clientToken": "job-after-terminated"}))
        .await
        .refused(
            "starting a job on a terminated cluster",
            400,
            "ResourceNotFoundException",
        );
    emr.delete_cluster(&cluster)
        .await
        .refused("deleting a terminated cluster", 400, "ValidationException");

    // The job runs the cluster held are still listed and still describable.
    let listed = emr.list_jobs(&cluster, "").await;
    let ids: Vec<String> = field(listed.ok(case), "/jobRuns", case)
        .as_array()
        .expect("jobRuns is a list")
        .iter()
        .map(|run| text(run, "/id", case))
        .collect();
    assert_eq!(
        ids,
        vec![job.clone()],
        "case {case:?}: a terminated cluster still lists the runs it held"
    );
}

/// The states this slice writes are members of the model's own enums, so no response
/// ever carries a state no `JobRunState` / `VirtualClusterState` client would accept.
///
/// Source: the generated `JOB_RUN_STATE_VALUES` and `VIRTUAL_CLUSTER_STATE_VALUES`
/// catalogs in `ops::types::shared_types`.
#[test]
fn every_state_this_slice_writes_is_a_modelled_one() {
    for state_value in [
        state::JOB_PENDING,
        state::JOB_SUBMITTED,
        state::JOB_RUNNING,
        state::JOB_CANCEL_PENDING,
        state::JOB_CANCELLED,
        state::JOB_FAILED,
        state::JOB_COMPLETED,
    ] {
        assert!(
            JOB_RUN_STATE_VALUES.contains(&state_value),
            "case {state_value:?}: not a JobRunState; the model allows {JOB_RUN_STATE_VALUES:?}"
        );
    }
    for state_value in [
        state::CLUSTER_RUNNING,
        state::CLUSTER_TERMINATING,
        state::CLUSTER_TERMINATED,
    ] {
        assert!(
            VIRTUAL_CLUSTER_STATE_VALUES.contains(&state_value),
            "case {state_value:?}: not a VirtualClusterState; the model allows {VIRTUAL_CLUSTER_STATE_VALUES:?}"
        );
    }
    for terminal in state::JOB_TERMINAL_STATES {
        assert!(
            JOB_RUN_STATE_VALUES.contains(terminal),
            "case {terminal:?}: a terminal state must also be a modelled JobRunState"
        );
    }
    assert!(
        !state::job_state_is_terminal(state::JOB_RUNNING),
        "case {:?}: a running job run has not finished",
        state::JOB_RUNNING
    );
}

/// The store is CAPPED, so a caller cannot grow it by creating resources in a loop: once
/// every slot holds a live cluster the create is refused with the modelled error, and
/// retiring one frees a slot that the next create reclaims.
#[tokio::test]
async fn the_virtual_cluster_store_is_capped_and_reclaims_retired_clusters() {
    let emr = Emr::new();
    let case = "the retention cap";
    let mut ids = Vec::new();
    for index in 0..state::MAX_VIRTUAL_CLUSTERS {
        let answer = emr
            .create_cluster(json!({
                "name": format!("capped-{index}"),
                "clientToken": format!("cap-tok-{index}"),
                "containerProvider": {"id": "eks-cap", "type": "EKS"},
            }))
            .await;
        ids.push(text(answer.ok(case), "/id", case));
    }
    assert_eq!(
        ids.len(),
        state::MAX_VIRTUAL_CLUSTERS,
        "case {case:?}: every cluster up to the cap must be created"
    );

    // Every slot holds a RUNNING cluster, so there is nothing to reclaim.
    emr.create_cluster(json!({
        "name": "one-too-many",
        "clientToken": "cap-tok-over",
        "containerProvider": {"id": "eks-cap", "type": "EKS"},
    }))
    .await
    .refused("a create past the cap", 400, "ValidationException");

    // Retire one, together with the job run it holds: a cluster slot must not be held
    // hostage by a run that has already finished. The delete starts a teardown the clock
    // finishes, and the next create reclaims the slot and the finished run with it.
    let started = emr
        .start_job(
            &ids[0],
            runnable_body("cap-job", spark_submit_driver()),
        )
        .await;
    let job = text(started.ok(case), "/id", case);
    emr.cancel_job(&ids[0], &job).await.ok(case);
    wait_for_the_clock();
    emr.delete_cluster(&ids[0]).await.ok(case);
    wait_for_the_clock();
    let answer = emr
        .create_cluster(json!({
            "name": "after-reclaim",
            "clientToken": "cap-tok-after",
            "containerProvider": {"id": "eks-cap", "type": "EKS"},
        }))
        .await;
    let fresh = text(answer.ok("a create after a cluster retired"), "/id", case);
    assert_resource_id(case, "the id minted into a reclaimed slot", &fresh);
    assert!(
        !ids.contains(&fresh),
        "case {case:?}: the reclaimed slot must hold a freshly minted id, got {fresh:?}"
    );

    // The reclaimed cluster and its finished run are gone; every live one is untouched.
    emr.describe_cluster(&ids[0]).await.refused(
        "the reclaimed cluster",
        400,
        "ResourceNotFoundException",
    );
    emr.describe_job(&ids[0], &job).await.refused(
        "the run reclaimed with its cluster",
        400,
        "ResourceNotFoundException",
    );
    emr.describe_cluster(&ids[1]).await.ok("a live cluster");
    emr.describe_cluster(&fresh).await.ok("the new cluster");
}
