// SPDX-License-Identifier: CC0-1.0

//! The REACH of AWS FIS's built-in action catalog, read end to end over HTTP.
//!
//! The sibling `action_catalog` suite asserts what `GetAction` reports for an action; this
//! one asserts WHICH actions it reports at all. The rule under test:
//!
//! * FIS publishes the action catalog, so `GetAction` answers the catalogued action for
//!   every action id that catalog holds — not for a sample of them — and an id the catalog
//!   does not hold answers `ResourceNotFoundException` (HTTP 404);
//! * what it reports for each is the same contract the sibling suite asserts: the id
//!   echoed, a description, the action's parameter set with every parameter's `required`
//!   flag, the target resource types it supports keyed by the name an experiment template
//!   refers to them by, and an ARN naming the region the request was signed for with no
//!   account segment.
//!
//! SOURCE for the expected rows below: the AWS FIS actions reference
//! (<https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>), which
//! the `Action` shape's own model documentation points at. They are written out here rather
//! than imported from `src/state.rs`, so a wrong entry in that table fails this suite
//! instead of agreeing with it.
//!
//! The rows are the actions the catalog table did NOT hold while it was a subset of that
//! reference: they span nine AWS services, actions that take one parameter and actions that
//! take five, required-only and mixed parameter sets, and target resource types from
//! containers (`aws:ecs:task`, `aws:eks:pod`, `aws:eks:cluster`) to storage
//! (`aws:s3:bucket`, `aws:dynamodb:global-table`) and networking (`aws:ec2:route-table`,
//! `aws:ec2:transit-gateway`). Several of them are actions no issue cited.
//!
//! The HTTP harness below is local to this suite, as each sibling suite's is.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_fis_faultinjectionsimulator::state::ACTION_CATALOG;
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// A SigV4 credential scope naming this service, for `region`. The service field is what
/// routes the request to fis; the region field is what a reported ARN names.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fis/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `action` this answer carries.
    fn action(&self) -> &Value {
        &self.body["action"]
    }
}

async fn get_action(router: &MultiServiceRouter, id: &str, region: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("/actions/{id}"))
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(region))
        .header("content-type", "application/json")
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "GET /actions/{id} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One expected catalog row: the action's id, its parameters as (name, required) pairs and
/// its supported targets as (target name, resource type) pairs.
struct Row {
    id: &'static str,
    parameters: &'static [(&'static str, bool)],
    targets: &'static [(&'static str, &'static str)],
}

/// The published actions this suite asserts, by the reference named in the module docs.
fn rows() -> Vec<Row> {
    vec![
        // One required parameter, a DynamoDB global-table target.
        Row {
            id: "aws:dynamodb:global-table-pause-replication",
            parameters: &[("duration", true)],
            targets: &[("Tables", "aws:dynamodb:global-table")],
        },
        // Four parameters, three of them required, an IAM-role target.
        Row {
            id: "aws:ec2:api-error",
            parameters: &[
                ("duration", true),
                ("errorCode", true),
                ("operations", true),
                ("percentage", false),
            ],
            targets: &[("Roles", "aws:iam:role")],
        },
        // An ECS task action, stressing the task through the SSM agent.
        Row {
            id: "aws:ecs:task-cpu-stress",
            parameters: &[
                ("duration", true),
                ("installDependencies", false),
                ("percent", false),
                ("workers", false),
            ],
            targets: &[("Tasks", "aws:ecs:task")],
        },
        // A row no issue cited: the I/O sibling of the stress action above.
        Row {
            id: "aws:ecs:task-io-stress",
            parameters: &[
                ("duration", true),
                ("installDependencies", false),
                ("percent", false),
                ("workers", false),
            ],
            targets: &[("Tasks", "aws:ecs:task")],
        },
        // No `duration` at all: this action ends when the process dies.
        Row {
            id: "aws:ecs:task-kill-process",
            parameters: &[
                ("installDependencies", false),
                ("processName", true),
                ("signal", false),
            ],
            targets: &[("Tasks", "aws:ecs:task")],
        },
        // A row no issue cited: five parameters, four of them required.
        Row {
            id: "aws:ecs:task-network-blackhole-port",
            parameters: &[
                ("duration", true),
                ("installDependencies", false),
                ("port", true),
                ("protocol", true),
                ("trafficType", true),
            ],
            targets: &[("Tasks", "aws:ecs:task")],
        },
        // A row no issue cited.
        Row {
            id: "aws:ecs:task-network-latency",
            parameters: &[
                ("delayMilliseconds", false),
                ("duration", true),
                ("installDependencies", false),
                ("jitterMilliseconds", false),
                ("sources", true),
            ],
            targets: &[("Tasks", "aws:ecs:task")],
        },
        // A row no issue cited.
        Row {
            id: "aws:ecs:task-network-packet-loss",
            parameters: &[
                ("duration", true),
                ("installDependencies", false),
                ("lossPercent", false),
                ("sources", true),
            ],
            targets: &[("Tasks", "aws:ecs:task")],
        },
        // Five required parameters and a CLUSTER target, not a pod one.
        Row {
            id: "aws:eks:inject-kubernetes-custom-resource",
            parameters: &[
                ("kubernetesApiVersion", true),
                ("kubernetesKind", true),
                ("kubernetesNamespace", true),
                ("kubernetesSpec", true),
                ("maxDuration", true),
            ],
            targets: &[("Cluster", "aws:eks:cluster")],
        },
        Row {
            id: "aws:eks:pod-cpu-stress",
            parameters: &[
                ("duration", true),
                ("kubernetesServiceAccount", true),
                ("percent", false),
                ("workers", false),
            ],
            targets: &[("Pods", "aws:eks:pod")],
        },
        // Two parameters, no duration: the pod is deleted rather than held in a fault.
        Row {
            id: "aws:eks:pod-delete",
            parameters: &[("gracePeriodSeconds", false), ("kubernetesServiceAccount", true)],
            targets: &[("Pods", "aws:eks:pod")],
        },
        // A row no issue cited.
        Row {
            id: "aws:eks:pod-io-stress",
            parameters: &[
                ("duration", true),
                ("kubernetesServiceAccount", true),
                ("percent", false),
                ("workers", false),
            ],
            targets: &[("Pods", "aws:eks:pod")],
        },
        // A row no issue cited.
        Row {
            id: "aws:eks:pod-memory-stress",
            parameters: &[
                ("duration", true),
                ("kubernetesServiceAccount", true),
                ("percent", false),
                ("workers", false),
            ],
            targets: &[("Pods", "aws:eks:pod")],
        },
        // A row no issue cited.
        Row {
            id: "aws:eks:pod-network-blackhole-port",
            parameters: &[
                ("duration", true),
                ("kubernetesServiceAccount", true),
                ("port", true),
                ("protocol", true),
                ("trafficType", true),
            ],
            targets: &[("Pods", "aws:eks:pod")],
        },
        // A row no issue cited.
        Row {
            id: "aws:eks:pod-network-latency",
            parameters: &[
                ("delayMilliseconds", false),
                ("duration", true),
                ("jitterMilliseconds", false),
                ("kubernetesServiceAccount", true),
                ("sources", true),
            ],
            targets: &[("Pods", "aws:eks:pod")],
        },
        // A row no issue cited.
        Row {
            id: "aws:eks:pod-network-packet-loss",
            parameters: &[
                ("duration", true),
                ("kubernetesServiceAccount", true),
                ("lossPercent", false),
                ("sources", true),
            ],
            targets: &[("Pods", "aws:eks:pod")],
        },
        // One required parameter, an ElastiCache replication-group target.
        Row {
            id: "aws:elasticache:replicationgroup-interrupt-az-power",
            parameters: &[("duration", true)],
            targets: &[("ReplicationGroups", "aws:elasticache:replicationgroup")],
        },
        // A Lambda action: four parameters, two of them required.
        Row {
            id: "aws:lambda:invocation-add-delay",
            parameters: &[
                ("duration", true),
                ("invocationPercentage", false),
                ("preventExecution", false),
                ("startupDelayMilliseconds", true),
            ],
            targets: &[("Functions", "aws:lambda:function")],
        },
        Row {
            id: "aws:lambda:invocation-error",
            parameters: &[
                ("duration", true),
                ("invocationPercentage", false),
                ("preventExecution", false),
            ],
            targets: &[("Functions", "aws:lambda:function")],
        },
        // A row no issue cited.
        Row {
            id: "aws:lambda:invocation-http-integration-response",
            parameters: &[
                ("contentTypeHeader", false),
                ("duration", true),
                ("invocationPercentage", false),
                ("preventExecution", false),
                ("statusCode", true),
            ],
            targets: &[("Functions", "aws:lambda:function")],
        },
        // A networking action whose target is a route table.
        Row {
            id: "aws:network:route-table-disrupt-connectivity",
            parameters: &[("duration", true), ("service", true)],
            targets: &[("RouteTables", "aws:ec2:route-table")],
        },
        // A networking action whose target is a transit gateway.
        Row {
            id: "aws:network:transit-gateway-disrupt-cross-region-connectivity",
            parameters: &[("duration", true), ("region", true)],
            targets: &[("TransitGateways", "aws:ec2:transit-gateway")],
        },
        // One required parameter, an S3 bucket target.
        Row {
            id: "aws:s3:bucket-pause-replication",
            parameters: &[("duration", true)],
            targets: &[("Buckets", "aws:s3:bucket")],
        },
    ]
}

/// Assert the whole rule for one row, read in `region`.
fn assert_row(row: &Row, region: &str, answer: &Answer) {
    let case = row.id;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetAction for a published action, got {} with {:?}",
        answer.status, answer.body
    );

    let action = answer.action();
    assert_eq!(
        action["id"].as_str(),
        Some(row.id),
        "case {case:?}: expected the reported action id to be {:?}, got {:?}",
        row.id,
        action["id"]
    );
    assert_eq!(
        action["arn"].as_str().unwrap_or_default(),
        format!("arn:aws:fis:{region}::action/{}", row.id),
        "case {case:?}: expected the action arn to name {region} with no account segment, got {:?}",
        action["arn"]
    );
    assert!(
        action["description"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "case {case:?}: expected a non-empty description, got {:?}",
        action["description"]
    );
    assert!(
        action["tags"]
            .as_object()
            .is_some_and(serde_json::Map::is_empty),
        "case {case:?}: expected no tags on a service-owned action, got {:?}",
        action["tags"]
    );

    let parameters = action["parameters"].as_object().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected parameters to be a map, got {:?}",
            action["parameters"]
        )
    });
    let mut reported: Vec<&str> = parameters.keys().map(String::as_str).collect();
    reported.sort_unstable();
    let mut expected: Vec<&str> = row.parameters.iter().map(|(name, _)| *name).collect();
    expected.sort_unstable();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected exactly the parameters {expected:?}, got {reported:?}"
    );
    for (name, required) in row.parameters {
        let parameter = &parameters[*name];
        assert_eq!(
            parameter["required"].as_bool(),
            Some(*required),
            "case {case:?}: expected parameter {name:?} required to be {required}, got {:?}",
            parameter["required"]
        );
        assert!(
            parameter["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "case {case:?}: expected parameter {name:?} to carry a description, got {:?}",
            parameter["description"]
        );
    }

    let targets = action["targets"].as_object().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected targets to be a map, got {:?}",
            action["targets"]
        )
    });
    assert_eq!(
        targets.len(),
        row.targets.len(),
        "case {case:?}: expected {} target(s), got {:?}",
        row.targets.len(),
        action["targets"]
    );
    for (name, resource_type) in row.targets {
        assert_eq!(
            targets[*name]["resourceType"].as_str(),
            Some(*resource_type),
            "case {case:?}: expected target {name:?} to name resource type {resource_type:?}, \
             got {:?}",
            targets[*name]
        );
    }
}

#[tokio::test]
async fn get_action_reports_every_published_action() {
    let router = emulator();
    for row in rows() {
        let answer = get_action(&router, row.id, FALLBACK_REGION).await;
        assert_row(&row, FALLBACK_REGION, &answer);
    }
}

#[tokio::test]
async fn a_published_action_is_reported_in_the_region_the_request_was_signed_for() {
    let router = emulator();
    // The signed region, two further partitions' regions, and an unusable scope that falls
    // back: the ARN is built from the request, never from one region.
    let regions = [
        ("eu-central-1", "eu-central-1"),
        ("ap-south-2", "ap-south-2"),
        ("us-gov-east-1", "us-gov-east-1"),
        ("not a region", FALLBACK_REGION),
    ];
    for (signed, expected) in regions {
        for row in rows() {
            let answer = get_action(&router, row.id, signed).await;
            assert_row(&row, expected, &answer);
        }
    }
}

#[tokio::test]
async fn reading_a_published_action_twice_reports_the_same_action() {
    let router = emulator();
    for row in rows() {
        let first = get_action(&router, row.id, FALLBACK_REGION).await;
        let second = get_action(&router, row.id, FALLBACK_REGION).await;
        assert_eq!(
            first.body, second.body,
            "case {:?}: expected a repeated read to report the same action, got {:?} then {:?}",
            row.id, first.body, second.body
        );
    }
}

#[tokio::test]
async fn an_id_the_published_catalog_does_not_hold_is_still_not_found() {
    let router = emulator();
    // A catalogued id is not a prefix rule: each of these is a near miss of a row above —
    // a plausible verb FIS does not publish, a cased id, a pluralised one, a reversed one
    // and a suffixed one — and names no action.
    let unknown = [
        "aws:eks:pod-kill",
        "AWS:EKS:POD-DELETE",
        "aws:eks:pod-deletes",
        "aws:lambda:invocation-delay",
        "aws:s3:bucket-resume-replication",
        "aws:dynamodb:table-pause-replication",
        "aws:eks:pod-delete:v2",
    ];
    for id in unknown {
        let answer = get_action(&router, id, FALLBACK_REGION).await;
        assert_eq!(
            answer.status, 404,
            "case {id:?}: expected HTTP 404 for an id the catalog does not hold, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code().ends_with("ResourceNotFoundException"),
            "case {id:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
    }
}

#[tokio::test]
async fn the_catalog_holds_each_action_once_and_in_id_order() {
    // An invariant of the table itself rather than an expected value: the catalog is what
    // makes `GetAction` and `ListActions` agree, so one id resolves to one action and the
    // table stays in the order `ListActions` enumerates in.
    for pair in ACTION_CATALOG.windows(2) {
        assert!(
            pair[0].id < pair[1].id,
            "case {:?}: expected the catalog to be sorted by id with no duplicate, got {:?} \
             before {:?}",
            pair[0].id,
            pair[0].id,
            pair[1].id
        );
    }
    // Every row this suite asserts is one the table holds, and every id has the published
    // `aws:<service>:<action>` form.
    for entry in ACTION_CATALOG {
        let segments: Vec<&str> = entry.id.split(':').collect();
        assert!(
            segments.len() == 3 && segments[0] == "aws",
            "case {:?}: expected an id of the form aws:<service>:<action>, got {:?}",
            entry.id,
            segments
        );
        for target in entry.targets {
            let segments: Vec<&str> = target.resource_type.split(':').collect();
            assert!(
                segments.len() == 3 && segments[0] == "aws",
                "case {:?}: expected target {:?} to name a resource type of the form \
                 aws:<service>:<type>, got {:?}",
                entry.id,
                target.name,
                target.resource_type
            );
        }
    }
}
