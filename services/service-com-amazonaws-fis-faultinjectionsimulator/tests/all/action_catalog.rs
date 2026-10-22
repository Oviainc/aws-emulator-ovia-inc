// SPDX-License-Identifier: CC0-1.0

//! AWS FIS's built-in action catalog, read end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler and serialization — the same path a client takes. The
//! rule under test is the one FIS's action surface states:
//!
//! * an ACTION is service-owned, so `GetAction` reports the catalogued action the
//!   requested id names — that id echoed, a description, the action's parameter set with
//!   each parameter's `required` flag, and the target resource types it supports keyed by
//!   the name an experiment template refers to them by — under an ARN that names the
//!   region the request was signed for and carries NO account segment, because the action
//!   belongs to the service rather than to an account;
//! * the catalog is constant: no call creates, changes or removes an entry, so repeating
//!   a read reports the same action, and an id the catalog does not hold (including a
//!   differently cased or truncated one) answers `ResourceNotFoundException` (HTTP 404)
//!   rather than a fabricated action;
//! * an id outside `ActionId`'s modelled `@length(min: 0, max: 128)` is refused with
//!   `ValidationException` (HTTP 400) before any of that.
//!
//! SOURCE for the expected rows below: the AWS FIS actions reference
//! (<https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>), which
//! the `Action` shape's own model documentation points at. They are written out here
//! rather than imported from `src/state.rs` so a wrong entry in that table fails this
//! suite instead of agreeing with it.
//!
//! The HTTP harness below is local to this suite: the ones in the sibling suites are
//! private to them, and a test harness is not shared state.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `ActionId`'s published ceiling (`@length(min: 0, max: 128)`), spelled out here rather
/// than imported from the crate so a wrong bound fails this suite.
const ACTION_ID_CEILING: usize = 128;

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

/// One expected catalog row: the action's id, its parameters as (name, required) pairs
/// and its supported targets as (target name, resource type) pairs.
struct Row {
    id: &'static str,
    parameters: &'static [(&'static str, bool)],
    targets: &'static [(&'static str, &'static str)],
}

/// The rows the rule is asserted over.
///
/// They differ along everything the rule generalises over: how many parameters an action
/// takes (none, one, two, four, five), whether those parameters are all required, all
/// optional or mixed, how many targets it supports (none, one) and which resource type
/// those targets name, and which AWS service the action's id belongs to. The recorded
/// case the issue was filed from is ONE of these rows.
fn rows() -> Vec<Row> {
    vec![
        // No parameters at all, one target.
        Row {
            id: "aws:ec2:terminate-instances",
            parameters: &[],
            targets: &[("Instances", "aws:ec2:instance")],
        },
        // No parameters, a target of another service's resource type.
        Row {
            id: "aws:rds:failover-db-cluster",
            parameters: &[],
            targets: &[("Clusters", "aws:rds:cluster")],
        },
        // Parameters, no target.
        Row {
            id: "aws:fis:wait",
            parameters: &[("duration", true)],
            targets: &[],
        },
        // Two required parameters, no target.
        Row {
            id: "aws:cloudwatch:assert-alarm-state",
            parameters: &[("alarmArns", true), ("alarmStates", true)],
            targets: &[],
        },
        // Optional parameters only, one target.
        Row {
            id: "aws:ec2:stop-instances",
            parameters: &[
                ("completeIfInstancesTerminated", false),
                ("startInstancesAfterDuration", false),
            ],
            targets: &[("Instances", "aws:ec2:instance")],
        },
        // Required and optional parameters mixed, an IAM-role target.
        Row {
            id: "aws:fis:inject-api-throttle-error",
            parameters: &[
                ("duration", true),
                ("operations", true),
                ("percentage", false),
                ("service", true),
            ],
            targets: &[("Roles", "aws:iam:role")],
        },
        // The same parameter set on the sibling fault, so the catalog is not one action
        // deep.
        Row {
            id: "aws:fis:inject-api-internal-error",
            parameters: &[
                ("duration", true),
                ("operations", true),
                ("percentage", false),
                ("service", true),
            ],
            targets: &[("Roles", "aws:iam:role")],
        },
        // Four parameters and a target.
        Row {
            id: "aws:ssm:send-command",
            parameters: &[
                ("documentArn", true),
                ("documentParameters", false),
                ("documentVersion", false),
                ("duration", true),
            ],
            targets: &[("Instances", "aws:ec2:instance")],
        },
        // Five parameters, no target.
        Row {
            id: "aws:ssm:start-automation-execution",
            parameters: &[
                ("documentArn", true),
                ("documentParameters", false),
                ("documentVersion", false),
                ("maxConcurrency", false),
                ("maxErrors", false),
            ],
            targets: &[],
        },
        // A single required parameter expressed as a percentage, with a target.
        Row {
            id: "aws:eks:terminate-nodegroup-instances",
            parameters: &[("instanceTerminationPercentage", true)],
            targets: &[("Nodegroups", "aws:eks:nodegroup")],
        },
    ]
}

/// Assert the whole rule for one row, read in `region`.
fn assert_row(row: &Row, region: &str, answer: &Answer) {
    let case = row.id;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetAction, got {} with {:?}",
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
        "case {case:?}: expected the action arn to name {region} with no account segment, \
         got {:?}",
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
        action["tags"].as_object().is_some_and(serde_json::Map::is_empty),
        "case {case:?}: expected no tags on a service-owned action, got {:?}",
        action["tags"]
    );

    let parameters = action["parameters"].as_object().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected parameters to be a map, got {:?}",
            action["parameters"]
        )
    });
    let mut reported: Vec<&String> = parameters.keys().collect();
    reported.sort();
    let expected: Vec<&str> = row.parameters.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        reported,
        expected.iter().collect::<Vec<&&str>>(),
        "case {case:?}: expected exactly the parameters {expected:?}, got {:?}",
        parameters.keys().collect::<Vec<&String>>()
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
            "case {case:?}: expected target {name:?} to name resource type \
             {resource_type:?}, got {:?}",
            targets[*name]
        );
    }
}

#[tokio::test]
async fn get_action_reports_the_catalogued_action_for_every_row() {
    let router = emulator();
    for row in rows() {
        let answer = get_action(&router, row.id, FALLBACK_REGION).await;
        assert_row(&row, FALLBACK_REGION, &answer);
    }
}

#[tokio::test]
async fn an_action_arn_names_the_region_the_request_was_signed_for() {
    let router = emulator();
    // The signed region, an unusable scope that falls back, and two further regions: the
    // ARN is built from the request, never from one region.
    let regions = [
        ("eu-west-2", "eu-west-2"),
        ("ap-southeast-3", "ap-southeast-3"),
        ("us-gov-west-1", "us-gov-west-1"),
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
async fn reading_an_action_twice_reports_the_same_action() {
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
async fn an_id_the_catalog_does_not_hold_is_not_found() {
    let router = emulator();
    // Each case is a way of naming no action: a plausible id FIS does not publish, a
    // catalogued id with its case changed, one truncated, one with the service segment
    // dropped, an unqualified word, and an id at `ActionId`'s published ceiling.
    let unknown = [
        "aws:fis:sleep",
        "AWS:FIS:WAIT",
        "aws:ec2:stop-instance",
        "stop-instances",
        "wait",
        &"a".repeat(ACTION_ID_CEILING),
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
async fn an_id_past_the_published_ceiling_is_refused() {
    let router = emulator();
    let id = "a".repeat(ACTION_ID_CEILING + 1);
    let answer = get_action(&router, &id, FALLBACK_REGION).await;
    assert_eq!(
        answer.status,
        400,
        "case {:?}: expected HTTP 400 for an id past ActionId's ceiling of \
         {ACTION_ID_CEILING}, got {} with {:?}",
        format!("{ACTION_ID_CEILING}+1 chars"),
        answer.status,
        answer.body
    );
    assert!(
        answer.error_code().ends_with("ValidationException"),
        "case {:?}: expected ValidationException, got {:?}",
        format!("{ACTION_ID_CEILING}+1 chars"),
        answer.error_code()
    );
}
