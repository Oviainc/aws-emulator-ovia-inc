// SPDX-License-Identifier: CC0-1.0

//! The managed scaling policy attached to an EMR cluster, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, so these are the same bytes an SDK client sends. Nothing reaches
//! into the service's state struct: a policy is only ever read back through
//! `GetManagedScalingPolicy`, which is what makes these tests evidence that the response
//! is computed from persisted state.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `com.amazonaws.emr#ComputeLimitsUnitType`, every value the model declares
/// (`smithy_query(query="shape", shape="ComputeLimitsUnitType")`).
const UNIT_TYPES: [&str; 3] = ["InstanceFleetUnits", "Instances", "VCPU"];

/// `com.amazonaws.emr#ScalingStrategy`, every value the model declares
/// (`smithy_query(query="shape", shape="ScalingStrategy")`).
const SCALING_STRATEGIES: [&str; 2] = ["DEFAULT", "ADVANCED"];

/// The published bounds of `ManagedScalingPolicy$UtilizationPerformanceIndex`
/// (`range { min: 1, max: 100 }`, Smithy model).
const UTILIZATION_INDEX_BOUNDS: [i64; 2] = [1, 100];

static NULL: Value = Value::Null;

/// One emulator: a router carrying only this service, over one shared engine.
struct Emr {
    router: MultiServiceRouter,
}

impl Emr {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "elasticmapreduce.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("ElasticMapReduce.{operation}"))
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

    async fn ok(&self, case: &str, operation: &str, body: Value) -> Value {
        let (status, payload) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {payload}"
        );
        payload
    }

    /// `RunJobFlow`, answering the minted cluster id.
    async fn run_job_flow(&self, case: &str, request: Value) -> String {
        let out = self.ok(case, "RunJobFlow", request).await;
        out["JobFlowId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: RunJobFlow must answer a JobFlowId, got {out}")
            })
            .to_string()
    }

    /// The policy `GetManagedScalingPolicy` reports for `cluster_id` — `Value::Null`
    /// when the cluster carries none.
    async fn policy(&self, case: &str, cluster_id: &str) -> Value {
        let out = self
            .ok(case, "GetManagedScalingPolicy", json!({ "ClusterId": cluster_id }))
            .await;
        out.get("ManagedScalingPolicy").cloned().unwrap_or(NULL.clone())
    }
}

/// The smallest request that satisfies `RunJobFlow`'s required members.
fn minimal_cluster(name: &str) -> Value {
    json!({
        "Name": name,
        "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
        "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
    })
}

/// A refusal: HTTP 400 with the error code `ValidationException`, carrying a message
/// that names `must_name` (the member or value the request got wrong) when one is given.
fn assert_refused(case: &str, status: u16, payload: &Value, must_name: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    let code = payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        code.contains("ValidationException"),
        "case {case:?}: expected error code ValidationException, got {code:?} in {payload}"
    );
    let message = crate::input_refusals::error_message(payload);
    assert!(
        !message.is_empty(),
        "case {case:?}: the refusal must say what was wrong, got {payload}"
    );
    assert!(
        must_name.is_empty() || message.contains(must_name),
        "case {case:?}: the refusal must name {must_name:?}, got {message:?}"
    );
}

/// Policies spanning what the shape allows: every `UnitType`, both `ScalingStrategy`
/// values, both published bounds of `UtilizationPerformanceIndex`, the optional
/// per-market limits present and absent, a minimum equal to the maximum, and a policy
/// that carries no `ComputeLimits` at all (the member is optional).
fn policy_table() -> Vec<(&'static str, Value)> {
    let mut rows: Vec<(&'static str, Value)> = vec![
        (
            "instance-group units, minimum below maximum",
            json!({
                "ComputeLimits": {
                    "UnitType": UNIT_TYPES[1],
                    "MinimumCapacityUnits": 2,
                    "MaximumCapacityUnits": 10,
                },
            }),
        ),
        (
            "fleet units with both per-market ceilings",
            json!({
                "ComputeLimits": {
                    "UnitType": UNIT_TYPES[0],
                    "MinimumCapacityUnits": 8,
                    "MaximumCapacityUnits": 64,
                    "MaximumOnDemandCapacityUnits": 16,
                    "MaximumCoreCapacityUnits": 32,
                },
            }),
        ),
        (
            "vCPU units, minimum equal to maximum",
            json!({
                "ComputeLimits": {
                    "UnitType": UNIT_TYPES[2],
                    "MinimumCapacityUnits": 4,
                    "MaximumCapacityUnits": 4,
                },
            }),
        ),
        (
            "no compute limits, strategy only",
            json!({ "ScalingStrategy": SCALING_STRATEGIES[0] }),
        ),
    ];
    // Both published bounds of UtilizationPerformanceIndex, each with the ADVANCED
    // strategy the member documents ("an advanced scaling strategy").
    for index in UTILIZATION_INDEX_BOUNDS {
        rows.push((
            if index == UTILIZATION_INDEX_BOUNDS[0] {
                "advanced strategy at the minimum utilization index"
            } else {
                "advanced strategy at the maximum utilization index"
            },
            json!({
                "ComputeLimits": {
                    "UnitType": UNIT_TYPES[1],
                    "MinimumCapacityUnits": 1,
                    "MaximumCapacityUnits": 100,
                },
                "ScalingStrategy": SCALING_STRATEGIES[1],
                "UtilizationPerformanceIndex": index,
            }),
        ));
    }
    rows
}

/// Put then Get is the identity on every row of the table, and Put replaces rather than
/// merges: the policy a cluster reports is the LAST one attached to it.
#[tokio::test]
async fn put_then_get_reports_the_attached_policy() {
    let emr = Emr::new();
    let table = policy_table();

    for (case, policy) in &table {
        let cluster = emr.run_job_flow(case, minimal_cluster("scaled")).await;

        // A cluster created without a policy carries none — absent, not an empty shape.
        let before = emr.policy(case, &cluster).await;
        assert!(
            before.is_null(),
            "case {case:?}: a cluster with no policy attached must report none, got {before}"
        );

        emr.ok(
            case,
            "PutManagedScalingPolicy",
            json!({ "ClusterId": cluster, "ManagedScalingPolicy": policy }),
        )
        .await;

        let got = emr.policy(case, &cluster).await;
        assert_eq!(
            &got, policy,
            "case {case:?}: the reported policy must be the one attached, got {got}"
        );

        // Attaching a second time REPLACES: the members of the first policy that the
        // second omits are gone, rather than merged in.
        let replacement = &table[0].1;
        emr.ok(
            case,
            "PutManagedScalingPolicy",
            json!({ "ClusterId": cluster, "ManagedScalingPolicy": replacement }),
        )
        .await;
        let got = emr.policy(case, &cluster).await;
        assert_eq!(
            &got, replacement,
            "case {case:?}: a second Put must replace the attached policy, got {got}"
        );
    }
}

/// A policy attached by `RunJobFlow$ManagedScalingPolicy` is the same attachment, so the
/// cluster reports it without any `PutManagedScalingPolicy` call.
#[tokio::test]
async fn a_policy_attached_at_creation_is_reported() {
    let emr = Emr::new();

    for (case, policy) in policy_table() {
        let mut request = minimal_cluster("created with a policy");
        request["ManagedScalingPolicy"] = policy.clone();
        let cluster = emr.run_job_flow(case, request).await;

        let got = emr.policy(case, &cluster).await;
        assert_eq!(
            got, policy,
            "case {case:?}: RunJobFlow's policy must be reported for the new cluster, got {got}"
        );
    }
}

/// Remove takes the policy off, and removing again changes nothing.
#[tokio::test]
async fn remove_detaches_the_policy_and_is_idempotent() {
    let emr = Emr::new();

    for (case, policy) in policy_table() {
        let cluster = emr.run_job_flow(case, minimal_cluster("to be unscaled")).await;
        emr.ok(
            case,
            "PutManagedScalingPolicy",
            json!({ "ClusterId": cluster, "ManagedScalingPolicy": policy }),
        )
        .await;

        for attempt in 1..=2 {
            emr.ok(
                case,
                "RemoveManagedScalingPolicy",
                json!({ "ClusterId": cluster }),
            )
            .await;
            let got = emr.policy(case, &cluster).await;
            assert!(
                got.is_null(),
                "case {case:?}: removal {attempt} must leave no policy, got {got}"
            );
        }

        // And the cluster takes a policy again afterwards.
        emr.ok(
            case,
            "PutManagedScalingPolicy",
            json!({ "ClusterId": cluster, "ManagedScalingPolicy": policy }),
        )
        .await;
        let got = emr.policy(case, &cluster).await;
        assert_eq!(
            got, policy,
            "case {case:?}: a cluster must take a policy again after removal, got {got}"
        );
    }
}

/// Reading a policy never changes what a later caller sees, and reading one cluster's
/// policy says nothing about another's.
#[tokio::test]
async fn get_is_a_read_and_clusters_are_independent() {
    let emr = Emr::new();
    let table = policy_table();

    let mut clusters = Vec::new();
    for (case, policy) in &table {
        let cluster = emr.run_job_flow(case, minimal_cluster("one of many")).await;
        emr.ok(
            case,
            "PutManagedScalingPolicy",
            json!({ "ClusterId": cluster, "ManagedScalingPolicy": policy }),
        )
        .await;
        clusters.push(cluster);
    }
    // A cluster that never took a policy keeps reporting none, however often it is read.
    let bare = emr.run_job_flow("bare cluster", minimal_cluster("bare")).await;

    for round in 1..=3 {
        for ((case, policy), cluster) in table.iter().zip(&clusters) {
            let got = emr.policy(case, cluster).await;
            assert_eq!(
                &got, policy,
                "case {case:?}: read {round} must report the same policy, got {got}"
            );
        }
        let got = emr.policy("bare cluster", &bare).await;
        assert!(
            got.is_null(),
            "case \"bare cluster\": read {round} must report no policy, got {got}"
        );
    }
}

/// Every managed-scaling-policy operation REFUSES a cluster the service holds no record
/// for, with HTTP 400 `ValidationException` — never a 501, and never an empty success —
/// and each one answers the sentence its own surface words: the reporting operation
/// reports the cluster id it was given, while the two mutators answer the legacy
/// job-flow surface's constant sentence, which names no id.
#[tokio::test]
async fn unknown_clusters_are_refused_by_every_operation() {
    let emr = Emr::new();
    // A real cluster exists, so the refusals are about the id and not about an empty
    // store.
    let live = emr
        .run_job_flow("live cluster", minimal_cluster("live"))
        .await;

    let unknown = [
        ("well-formed but absent id", format!("{live}ZZ")),
        ("another absent job-flow id", "j-3ABCDEFGHIJKL".to_string()),
        ("an id in no EMR form at all", "not-a-cluster".to_string()),
        ("an empty id", String::new()),
    ];

    for (label, cluster_id) in unknown {
        // The reporting operation reports the id; the mutators answer the constant.
        let job_flow_sentence = "Specified job flow ID not valid.".to_string();
        let requests = [
            (
                "GetManagedScalingPolicy",
                json!({ "ClusterId": cluster_id }),
                format!("Cluster id '{cluster_id}' is not valid."),
            ),
            (
                "PutManagedScalingPolicy",
                json!({
                    "ClusterId": cluster_id,
                    "ManagedScalingPolicy": policy_table()[0].1,
                }),
                job_flow_sentence.clone(),
            ),
            (
                "RemoveManagedScalingPolicy",
                json!({ "ClusterId": cluster_id }),
                job_flow_sentence,
            ),
        ];
        for (operation, body, expected) in requests {
            let case = format!("{operation} with {label}");
            let (status, payload) = emr.call(operation, body).await;
            assert_refused(&case, status, &payload, "");
            assert_eq!(
                crate::input_refusals::error_message(&payload),
                expected,
                "case {case:?}: expected {expected:?}, got {payload}"
            );
        }
    }

    // The refusals left the live cluster alone.
    let got = emr.policy("live cluster", &live).await;
    assert!(
        got.is_null(),
        "case \"live cluster\": a refused request must not attach a policy, got {got}"
    );
}

/// `MinimumCapacityUnits` is the lower boundary and `MaximumCapacityUnits` the upper
/// one, so a minimum above the maximum is refused — wherever a policy is attached — and
/// nothing is stored.
#[tokio::test]
async fn compute_limits_with_an_inverted_range_are_refused() {
    let emr = Emr::new();

    let inverted: Vec<(&str, Value)> = UNIT_TYPES
        .iter()
        .map(|unit_type| {
            (
                *unit_type,
                json!({
                    "ComputeLimits": {
                        "UnitType": unit_type,
                        "MinimumCapacityUnits": 12,
                        "MaximumCapacityUnits": 3,
                    },
                }),
            )
        })
        .collect();

    for (unit_type, policy) in &inverted {
        let case = format!("PutManagedScalingPolicy with inverted {unit_type} limits");
        let cluster = emr.run_job_flow(&case, minimal_cluster("inverted")).await;
        let (status, payload) = emr
            .call(
                "PutManagedScalingPolicy",
                json!({ "ClusterId": cluster, "ManagedScalingPolicy": policy }),
            )
            .await;
        assert_refused(&case, status, &payload, "MinimumCapacityUnits");

        // Refused means not stored.
        let got = emr.policy(&case, &cluster).await;
        assert!(
            got.is_null(),
            "case {case:?}: a refused policy must not be attached, got {got}"
        );

        // The same rule at the other place a policy is attached.
        let case = format!("RunJobFlow with inverted {unit_type} limits");
        let mut request = minimal_cluster("inverted at creation");
        request["ManagedScalingPolicy"] = policy.clone();
        let (status, payload) = emr.call("RunJobFlow", request).await;
        assert_refused(&case, status, &payload, "MinimumCapacityUnits");
    }
}

/// `UtilizationPerformanceIndex` carries `range { min: 1, max: 100 }`, so a value past
/// either published bound is refused at the boundary while both bounds are accepted.
#[tokio::test]
async fn utilization_performance_index_is_held_to_its_range() {
    let emr = Emr::new();
    let cluster = emr
        .run_job_flow("indexed cluster", minimal_cluster("indexed"))
        .await;

    for index in UTILIZATION_INDEX_BOUNDS {
        let case = format!("UtilizationPerformanceIndex at the bound {index}");
        let policy = json!({
            "ScalingStrategy": SCALING_STRATEGIES[1],
            "UtilizationPerformanceIndex": index,
        });
        emr.ok(
            &case,
            "PutManagedScalingPolicy",
            json!({ "ClusterId": cluster, "ManagedScalingPolicy": policy }),
        )
        .await;
        let got = emr.policy(&case, &cluster).await;
        assert_eq!(
            got, policy,
            "case {case:?}: a bound value must be accepted and reported, got {got}"
        );
    }

    for index in [UTILIZATION_INDEX_BOUNDS[0] - 1, UTILIZATION_INDEX_BOUNDS[1] + 1] {
        let case = format!("UtilizationPerformanceIndex past the bound: {index}");
        let (status, payload) = emr
            .call(
                "PutManagedScalingPolicy",
                json!({
                    "ClusterId": cluster,
                    "ManagedScalingPolicy": {
                        "ScalingStrategy": SCALING_STRATEGIES[1],
                        "UtilizationPerformanceIndex": index,
                    },
                }),
            )
            .await;
        // The boundary validator names the member on the wire path
        // (`managedScalingPolicy.utilizationPerformanceIndex`).
        assert_refused(&case, status, &payload, "utilizationPerformanceIndex");
    }
}
