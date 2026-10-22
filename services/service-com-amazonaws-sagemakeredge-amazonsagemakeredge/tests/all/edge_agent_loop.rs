// SPDX-License-Identifier: CC0-1.0

//! The SageMaker Edge Manager agent loop, end to end over the wire.
//!
//! Every assertion here drives `restJson1` requests through the same
//! `MultiServiceRouter` the production entrypoint serves, so routing,
//! deserialization, constraint validation and serialization are all exercised; state is
//! read back through `state::service_state`, the accessor the handlers use.
//!
//! The rule under test: each of the three operations answers from the device and fleet
//! records this service persists — the registration document and TTL recorded for THAT
//! device, the deployments seeded for THAT fleet minus the ones THAT device has
//! acknowledged — and the heartbeat, whose output has no members, is observable only
//! through the record it writes. Each table row is one input of the class, not the one
//! case the issue cited.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_sagemakeredge_amazonsagemakeredge::ops::types::{
    Checksum, Definition,
};
use service_com_amazonaws_sagemakeredge_amazonsagemakeredge::state::{
    self, Deployment, NOT_REGISTERED, REGISTERED,
};
use service_com_amazonaws_sagemakeredge_amazonsagemakeredge::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for; the device ARN in a registration document must carry
/// the region the request was signed for, not a hardcoded one.
const REGION: &str = "us-west-2";

/// An engine and a router carrying only this service.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The test signs with a syntactically valid but unverifiable scope: routing reads the
    // scope, so the header must be there, but no authenticator may check its signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// POST one operation and return its status and parsed body (`Null` for an empty body).
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://edge.sagemaker.{REGION}.amazonaws.com/{operation}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/sagemaker/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|_| {
            json!({ "__unparsed": String::from_utf8_lossy(response.body()).to_string() })
        })
    };
    (status, parsed)
}

/// A `Definition` as a fleet's deployment carries it.
fn definition(model_handle: &str, s3_url: &str, checksum_sum: &str, model_state: &str) -> Definition {
    Definition {
        model_handle: Some(model_handle.to_string()),
        s3_url: Some(s3_url.to_string()),
        checksum: Some(Checksum {
            // `ChecksumType` accepts only SHA1 (CHECKSUM_TYPE_VALUES in ops/types).
            r#type: Some("SHA1".to_string()),
            sum: Some(checksum_sum.to_string()),
        }),
        state: Some(model_state.to_string()),
    }
}

/// The `Deployments` list of a `GetDeployments` response as `(name, definition handles)`.
fn reported_deployments(body: &Value) -> Vec<(String, Vec<String>)> {
    body["Deployments"]
        .as_array()
        .unwrap_or_else(|| panic!("Deployments must be a list, got {body}"))
        .iter()
        .map(|deployment| {
            let handles = deployment["Definitions"]
                .as_array()
                .map(|definitions| {
                    definitions
                        .iter()
                        .map(|d| d["ModelHandle"].as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            (
                deployment["DeploymentName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                handles,
            )
        })
        .collect()
}

/// `GetDeployments` for one device, as `(name, definition handles)` per deployment.
async fn outstanding(
    router: &MultiServiceRouter,
    fleet: &str,
    device: &str,
) -> Vec<(String, Vec<String>)> {
    let (status, body) = call(
        router,
        "GetDeployments",
        &json!({ "DeviceFleetName": fleet, "DeviceName": device }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {fleet}/{device}: expected 200, got {status} {body}"
    );
    reported_deployments(&body)
}

/// A minimal heartbeat body for `(fleet, device)`.
fn heartbeat(fleet: &str, device: &str, agent_version: &str) -> Value {
    json!({
        "DeviceFleetName": fleet,
        "DeviceName": device,
        "AgentVersion": agent_version,
    })
}

/// `GetDeviceRegistration` answers the document and TTL recorded for the device asked
/// about — per device, per fleet, byte-identical on a second read — and reports a device
/// it does not hold as not registered without recording anything.
#[tokio::test]
async fn get_device_registration_reports_the_record_of_the_device_asked_about() {
    let (engine, router) = harness();

    // Rows differ along fleet, the form and length of the device name (hyphens,
    // underscores, the shortest legal name), the document recorded, and the TTL.
    let rows: [(&str, &str, &str, &str); 4] = [
        ("fleet-a", "edge-device-1", r#"{"Status":"REGISTERED","Slot":"a1"}"#, "300"),
        ("fleet-a", "edge_device_2", r#"{"Status":"REGISTERED","Slot":"a2"}"#, "60"),
        // The same device name in another fleet is a DIFFERENT device.
        ("fleet-b", "edge-device-1", r#"{"Status":"REGISTERED","Slot":"b1"}"#, "1"),
        ("f", "d", r#"{"Status":"REGISTERED","Slot":"min"}"#, "86400"),
    ];

    {
        let held = state::service_state(&*engine).expect("this service's state");
        let mut held = held.lock();
        for (fleet, device, registration, cache_ttl) in rows {
            held.register_device(fleet, device, registration, cache_ttl)
                .unwrap_or_else(|e| panic!("case {fleet}/{device}: recording the device: {e}"));
        }
    }

    for (fleet, device, registration, cache_ttl) in rows {
        let case = format!("{fleet}/{device}");
        let body = json!({ "DeviceFleetName": fleet, "DeviceName": device });
        let (status, first) = call(&router, "GetDeviceRegistration", &body).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {first}");
        assert_eq!(
            first["DeviceRegistration"].as_str(),
            Some(registration),
            "case {case}: expected the recorded document, got {:?}",
            first["DeviceRegistration"]
        );
        assert_eq!(
            first["CacheTTL"].as_str(),
            Some(cache_ttl),
            "case {case}: expected the recorded TTL, got {:?}",
            first["CacheTTL"]
        );

        // Reading is not generating: the second read answers the same bytes.
        let (status, second) = call(&router, "GetDeviceRegistration", &body).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {second}");
        assert_eq!(
            first, second,
            "case {case}: expected a second read to answer identically, got {second}"
        );
    }

    // A device the service does not hold is reported as not registered, and the read
    // registers nothing: the fleet keeps exactly the devices that were recorded.
    for (fleet, device) in [("fleet-a", "edge-device-9"), ("fleet-z", "edge-device-1")] {
        let case = format!("{fleet}/{device}");
        let (status, body) = call(
            &router,
            "GetDeviceRegistration",
            &json!({ "DeviceFleetName": fleet, "DeviceName": device }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");
        let document: Value = serde_json::from_str(
            body["DeviceRegistration"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: DeviceRegistration must be a string, got {body}")),
        )
        .unwrap_or_else(|e| panic!("case {case}: DeviceRegistration must be a document: {e}"));
        assert_eq!(
            document["Status"].as_str(),
            Some(NOT_REGISTERED),
            "case {case}: expected {NOT_REGISTERED}, got {document}"
        );
        assert_eq!(
            document["DeviceName"].as_str(),
            Some(device),
            "case {case}: expected the device asked about, got {document}"
        );
        assert!(
            body.get("CacheTTL").is_none_or(Value::is_null),
            "case {case}: expected no CacheTTL where no record is held, got {body}"
        );

        let held = state::service_state(&*engine).expect("this service's state");
        let held = held.lock();
        assert!(
            held.device(fleet, device).is_none(),
            "case {case}: expected the report to record nothing, got a device record"
        );
    }

    let held = state::service_state(&*engine).expect("this service's state");
    let held = held.lock();
    assert_eq!(
        held.fleet("fleet-a").map(|fleet| fleet.devices.len()),
        Some(2),
        "expected fleet-a to keep exactly its two recorded devices"
    );
    assert!(
        held.fleet("fleet-z").is_none(),
        "expected no fleet to be created by a read"
    );
}

/// `SendHeartbeat` persists what the agent reported — agent version, heartbeat instant,
/// the reported models and the reported agent metrics — and records the device on first
/// contact with a registration document `GetDeviceRegistration` then reports unchanged.
#[tokio::test]
async fn send_heartbeat_persists_what_the_agent_reported() {
    let (engine, router) = harness();
    let (fleet, device) = ("fleet-a", "edge-device-1");

    struct Case {
        case: &'static str,
        agent_version: &'static str,
        /// (ModelName, ModelVersion)
        models: &'static [(&'static str, &'static str)],
        /// (MetricName, Value)
        metrics: &'static [(&'static str, f64)],
    }

    // Rows differ along which optional members are present and how many entries each
    // list carries — including a later heartbeat that reports FEWER models than the one
    // before it, which is the device reporting its current state.
    let cases = [
        Case { case: "required members only", agent_version: "1.20.0", models: &[], metrics: &[] },
        Case {
            case: "one model, one metric",
            agent_version: "1.20.1",
            models: &[("mnist", "1.0")],
            metrics: &[("cpu-usage", 12.5)],
        },
        Case {
            case: "three models, two metrics",
            agent_version: "1.21.0",
            models: &[("mnist", "1.0"), ("resnet50", "2.3"), ("yolo", "0.9")],
            metrics: &[("cpu-usage", 90.25), ("mem-usage", 0.0)],
        },
        Case {
            case: "metrics only, after models were reported",
            agent_version: "2.0.0",
            models: &[],
            metrics: &[("cpu-usage", 7.0)],
        },
    ];

    let mut previous_heartbeat = None;
    let mut first_registration = None;
    for Case {
        case,
        agent_version,
        models,
        metrics,
    } in cases
    {
        let mut body = heartbeat(fleet, device, agent_version);
        if !models.is_empty() {
            body["Models"] = Value::Array(
                models
                    .iter()
                    .map(|(name, version)| json!({ "ModelName": name, "ModelVersion": version }))
                    .collect(),
            );
        }
        if !metrics.is_empty() {
            body["AgentMetrics"] = Value::Array(
                metrics
                    .iter()
                    .map(|(name, value)| {
                        json!({ "MetricName": name, "Value": value, "Dimension": "device/cpu" })
                    })
                    .collect(),
            );
        }

        let (status, response) = call(&router, "SendHeartbeat", &body).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {response}");

        let held = state::service_state(&*engine).expect("this service's state");
        let held = held.lock();
        let record = held
            .device(fleet, device)
            .unwrap_or_else(|| panic!("case {case}: expected the heartbeat to record the device"));
        assert_eq!(
            record.agent_version.as_deref(),
            Some(agent_version),
            "case {case}: expected the reported agent version, got {:?}",
            record.agent_version
        );
        let reported_models: Vec<(&str, &str)> = record
            .models
            .iter()
            .map(|model| {
                (
                    model.model_name.as_deref().unwrap_or_default(),
                    model.model_version.as_deref().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            reported_models, models,
            "case {case}: expected the reported models, got {reported_models:?}"
        );
        let reported_metrics: Vec<(&str, f64)> = record
            .agent_metrics
            .iter()
            .map(|metric| {
                (
                    metric.metric_name.as_deref().unwrap_or_default(),
                    metric.value.unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            reported_metrics, metrics,
            "case {case}: expected the reported metrics, got {reported_metrics:?}"
        );

        let heartbeat_at = record
            .last_heartbeat_at
            .unwrap_or_else(|| panic!("case {case}: expected the heartbeat instant to be recorded"));
        if let Some(previous) = previous_heartbeat {
            assert!(
                heartbeat_at >= previous,
                "case {case}: expected the heartbeat instant to advance, got {heartbeat_at:?}"
            );
        }
        previous_heartbeat = Some(heartbeat_at);

        // The registration document is minted once, when the record is created.
        let registration = record.registration.clone();
        match &first_registration {
            None => first_registration = Some(registration),
            Some(first) => assert_eq!(
                &registration, first,
                "case {case}: expected the registration document to be minted once"
            ),
        }
    }

    // Round trip: the record the heartbeat created is what GetDeviceRegistration reports.
    let (status, body) = call(
        &router,
        "GetDeviceRegistration",
        &json!({ "DeviceFleetName": fleet, "DeviceName": device }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {body}");
    let document: Value = serde_json::from_str(
        body["DeviceRegistration"]
            .as_str()
            .unwrap_or_else(|| panic!("DeviceRegistration must be a string, got {body}")),
    )
    .expect("DeviceRegistration must be a document");
    assert_eq!(
        document["Status"].as_str(),
        Some(REGISTERED),
        "expected the heartbeating device to be reported registered, got {document}"
    );
    assert_eq!(
        document["DeviceArn"].as_str(),
        Some(
            format!(
                "arn:aws:sagemaker:{REGION}:{}:device-fleet/{fleet}/device/{device}",
                engine::EMULATOR_ACCOUNT_ID
            )
            .as_str()
        ),
        "expected the device ARN of the signed region, got {document}"
    );
    assert_eq!(
        body["CacheTTL"].as_str(),
        Some(state::DEFAULT_CACHE_TTL_SECONDS),
        "expected the recorded cache TTL, got {body}"
    );
    assert_eq!(
        body["DeviceRegistration"].as_str(),
        first_registration.as_deref(),
        "expected the document recorded at first contact, got {body}"
    );
}

/// `GetDeployments` reports exactly the deployments seeded for the device's fleet that
/// the device has not acknowledged yet: never a fabricated one, never another fleet's,
/// and never one this device already reported a result for.
#[tokio::test]
async fn get_deployments_reports_the_outstanding_deployments_of_the_device() {
    let (engine, router) = harness();
    let fleet = "fleet-a";

    // Rows differ along how many definitions a deployment carries (two, none, one), its
    // type and failure-handling policy, and the model state each definition asks for.
    // The enum values come from DEPLOYMENT_TYPE_VALUES / FAILURE_HANDLING_POLICY_VALUES /
    // MODEL_STATE_VALUES in src/ops/types/shared_types.rs.
    {
        let seeds: Vec<(&str, Deployment)> = vec![
            (
                fleet,
                Deployment {
                    deployment_name: "deploy-1".to_string(),
                    deployment_type: Some("Model".to_string()),
                    failure_handling_policy: Some("ROLLBACK_ON_FAILURE".to_string()),
                    definitions: vec![
                        definition("mnist", "s3://bucket/mnist.tar.gz", "9a0364b9e99bb480", "DEPLOY"),
                        definition("yolo", "s3://bucket/yolo.tar.gz", "0bee89b07a248e27", "UNDEPLOY"),
                    ],
                },
            ),
            (
                fleet,
                Deployment {
                    deployment_name: "deploy-2".to_string(),
                    deployment_type: Some("Model".to_string()),
                    failure_handling_policy: Some("DO_NOTHING".to_string()),
                    definitions: Vec::new(),
                },
            ),
            (
                fleet,
                Deployment {
                    deployment_name: "deploy-3".to_string(),
                    deployment_type: None,
                    failure_handling_policy: None,
                    definitions: vec![definition(
                        "resnet50",
                        "s3://bucket/resnet50.tar.gz",
                        "b1946ac92492d234",
                        "DEPLOY",
                    )],
                },
            ),
            // Another fleet's deployment must never leak into this fleet's answer.
            (
                "fleet-b",
                Deployment {
                    deployment_name: "other-fleet-deploy".to_string(),
                    ..Deployment::default()
                },
            ),
        ];

        let held = state::service_state(&*engine).expect("this service's state");
        let mut held = held.lock();
        for (seed_fleet, deployment) in seeds {
            let name = deployment.deployment_name.clone();
            held.seed_deployment(seed_fleet, deployment)
                .unwrap_or_else(|e| panic!("case {seed_fleet}/{name}: seeding: {e}"));
        }
    }

    // Two devices of the same fleet, each recorded by its own heartbeat.
    for device in ["edge-device-1", "edge-device-2"] {
        let (status, body) = call(&router, "SendHeartbeat", &heartbeat(fleet, device, "1.20.0")).await;
        assert_eq!(status, 200, "case {device}: expected 200, got {status} {body}");
    }

    // A third device recorded through the control-plane seam instead of a heartbeat: how
    // the device record was created must not change what GetDeployments reports.
    {
        let held = state::service_state(&*engine).expect("this service's state");
        let mut held = held.lock();
        held.register_device(
            fleet,
            "edge-device-3",
            r#"{"Status":"REGISTERED","Slot":"a3"}"#,
            "300",
        )
        .expect("recording a device through the control-plane seam");
    }

    let all = vec![
        ("deploy-1".to_string(), vec!["mnist".to_string(), "yolo".to_string()]),
        ("deploy-2".to_string(), Vec::new()),
        ("deploy-3".to_string(), vec!["resnet50".to_string()]),
    ];
    assert_eq!(
        outstanding(&router, fleet, "edge-device-1").await,
        all,
        "case fleet-a/edge-device-1: expected every seeded deployment, in seeded order, with its stored definitions"
    );
    assert_eq!(
        outstanding(&router, fleet, "edge-device-3").await,
        all,
        "case fleet-a/edge-device-3 (registered through the seam, never heartbeated): expected every seeded deployment"
    );

    // A device the service does not hold, and a fleet nothing was seeded into, get an
    // empty list rather than a fabricated deployment.
    for (case_fleet, case_device) in [("fleet-a", "edge-device-9"), ("fleet-z", "edge-device-1")] {
        let reported = outstanding(&router, case_fleet, case_device).await;
        assert!(
            reported.is_empty(),
            "case {case_fleet}/{case_device}: expected no deployments, got {reported:?}"
        );
    }

    // Acknowledging one deployment withdraws exactly that one, for exactly that device.
    let acknowledge = |deployment_name: &str| {
        json!({
            "DeviceFleetName": fleet,
            "DeviceName": "edge-device-1",
            "AgentVersion": "1.20.0",
            "DeploymentResult": {
                "DeploymentName": deployment_name,
                "DeploymentStatus": "SUCCESS",
                "DeploymentStatusMessage": "deployed",
                "DeploymentStartTime": 1_767_225_600,
                "DeploymentEndTime": 1_767_225_660,
                "DeploymentModels": [
                    { "ModelHandle": "mnist", "ModelName": "mnist", "ModelVersion": "1.0", "DesiredState": "DEPLOY", "State": "DEPLOY", "Status": "SUCCESS" }
                ]
            }
        })
    };

    let (status, body) = call(&router, "SendHeartbeat", &acknowledge("deploy-2")).await;
    assert_eq!(status, 200, "case acknowledge deploy-2: expected 200, got {status} {body}");
    assert_eq!(
        outstanding(&router, fleet, "edge-device-1").await,
        vec![all[0].clone(), all[2].clone()],
        "case acknowledge deploy-2: expected the acknowledged deployment to be withdrawn"
    );
    assert_eq!(
        outstanding(&router, fleet, "edge-device-2").await,
        all,
        "case acknowledge deploy-2: expected another device of the fleet to still owe every deployment"
    );

    for name in ["deploy-1", "deploy-3"] {
        let (status, body) = call(&router, "SendHeartbeat", &acknowledge(name)).await;
        assert_eq!(status, 200, "case acknowledge {name}: expected 200, got {status} {body}");
    }
    assert!(
        outstanding(&router, fleet, "edge-device-1").await.is_empty(),
        "case acknowledge every deployment: expected nothing outstanding"
    );

    // The recorded results are the agent's, member for member.
    let held = state::service_state(&*engine).expect("this service's state");
    let held = held.lock();
    let record = held
        .device(fleet, "edge-device-1")
        .expect("the heartbeating device is recorded");
    let result = record
        .deployment_results
        .get("deploy-2")
        .expect("the acknowledgement of deploy-2 is recorded");
    assert_eq!(
        result.status.as_deref(),
        Some("SUCCESS"),
        "expected the reported status, got {:?}",
        result.status
    );
    assert_eq!(
        result.start_time.map(|t| t.secs()),
        Some(1_767_225_600),
        "expected the reported start time, got {:?}",
        result.start_time
    );
    assert_eq!(
        result.models.len(),
        1,
        "expected the reported deployment models, got {:?}",
        result.models
    );
}

/// A `DeploymentResult` that names no deployment cannot acknowledge one, so the request
/// is refused — 400 `ValidationException`, not a 501 and not a silent partial write.
#[tokio::test]
async fn a_deployment_result_naming_no_deployment_is_refused_and_writes_nothing() {
    let (engine, router) = harness();
    let (fleet, device) = ("fleet-a", "edge-device-1");

    let (status, body) = call(&router, "SendHeartbeat", &heartbeat(fleet, device, "1.20.0")).await;
    assert_eq!(status, 200, "the first heartbeat: expected 200, got {status} {body}");

    // Rows: the member absent, and present but empty-ish — both unplaceable.
    let cases = [
        ("DeploymentName absent", json!({ "DeploymentStatus": "SUCCESS" })),
        (
            "DeploymentName empty",
            json!({ "DeploymentName": "", "DeploymentStatus": "FAIL" }),
        ),
    ];
    for (case, deployment_result) in cases {
        let mut request = heartbeat(fleet, device, "9.9.9");
        request["DeploymentResult"] = deployment_result;
        let (status, body) = call(&router, "SendHeartbeat", &request).await;
        assert_eq!(status, 400, "case {case}: expected 400, got {status} {body}");

        let held = state::service_state(&*engine).expect("this service's state");
        let held = held.lock();
        let record = held.device(fleet, device).expect("the device is recorded");
        assert_eq!(
            record.agent_version.as_deref(),
            Some("1.20.0"),
            "case {case}: expected the refused heartbeat to write nothing, got {:?}",
            record.agent_version
        );
        assert!(
            record.deployment_results.is_empty(),
            "case {case}: expected no acknowledgement to be recorded, got {:?}",
            record.deployment_results
        );
    }
}

/// Request-keyed state is bounded: once the store holds its bound of device fleets, a
/// heartbeat naming a further fleet is refused rather than growing the store, and the
/// bound is not consumed by heartbeats of fleets already held.
#[tokio::test]
async fn request_keyed_state_stops_growing_at_its_bound() {
    let (engine, router) = harness();

    // Fill the fleet set: one fleet per heartbeat, each with one device.
    for index in 0..state::MAX_FLEETS {
        let body = heartbeat(&format!("fleet-{index}"), "edge-device-1", "1.20.0");
        let (status, response) = call(&router, "SendHeartbeat", &body).await;
        assert_eq!(
            status, 200,
            "case fleet-{index}: expected 200, got {status} {response}"
        );
    }

    // A fleet already held still accepts heartbeats — the bound caps growth, not traffic.
    let (status, response) = call(
        &router,
        "SendHeartbeat",
        &heartbeat("fleet-0", "edge-device-2", "1.20.0"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a held fleet: expected 200, got {status} {response}"
    );

    // One fleet too many is refused, and nothing is recorded for it.
    let (status, response) = call(
        &router,
        "SendHeartbeat",
        &heartbeat("fleet-over-the-bound", "edge-device-1", "1.20.0"),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one fleet past the bound: expected 400, got {status} {response}"
    );
    assert!(
        response.to_string().contains("bound of"),
        "case one fleet past the bound: expected the modelled error to name the bound, got {response}"
    );

    let held = state::service_state(&*engine).expect("this service's state");
    let held = held.lock();
    assert!(
        held.fleet("fleet-over-the-bound").is_none(),
        "case one fleet past the bound: expected the refusal to record nothing"
    );
    assert_eq!(
        held.fleet("fleet-0").map(|fleet| fleet.devices.len()),
        Some(2),
        "case a held fleet: expected both of its devices to be recorded"
    );
}
