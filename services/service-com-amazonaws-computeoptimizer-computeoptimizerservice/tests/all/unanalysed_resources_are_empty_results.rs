// SPDX-License-Identifier: CC0-1.0

//! A resource Compute Optimizer holds no recommendation for is reported as an EMPTY RESULT,
//! never as an absent or failed resource — driven end to end over HTTP.
//!
//! The rule under test, for every recommendation reader of this service at once: a request
//! naming a resource this service holds no recommendation for is answered HTTP 200 with that
//! resource simply absent from the result — the recommendation list carries no row for it,
//! the `errors` list carries no entry for it (that list reports the resources an analysis
//! REJECTED as unsupported, not the ones it has not reached), and a projected-metrics reader
//! reports the empty list of projections rather than a 404. Compute Optimizer produces a
//! recommendation only once it has accumulated enough metric history for a resource, so
//! "enrolled, nothing analysed yet" is the steady state of a freshly enrolled account and is
//! a successful empty answer, not a per-resource failure a client would alert on.
//!
//! The rows span what the rule generalises over: which reader answers, which resource type
//! the identifier names, the ARN form it is written in (another Region's, another resource
//! kind's, an identifier nothing ever recorded), how many identifiers one request names, and
//! whether the account's analysis is due at all.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::ANALYSIS_DELAY;
use smithy_server_runtime::MultiServiceRouter;
use std::time::Duration;

use crate::harness::{call, emulator_with_analysed_services, opt_in, ACCOUNT, REGION};

/// The first data point the projected-metrics rows ask for, in epoch seconds, and the
/// granularity of their window: one hourly data point.
const WINDOW_START: i64 = 1_761_000_000;
/// See [`WINDOW_START`].
const PERIOD: i64 = 3_600;

/// How a reader names the resources a request asks about, and what it reports them under.
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// The reader takes a LIST of identifiers and reports a list of recommendations beside
    /// the `errors` list: `(the input member, the output member)`.
    Many(&'static str, &'static str),
    /// The reader takes ONE identifier and reports the projections of its recommendation
    /// options: `(the input member)`.
    One(&'static str),
}

/// One reader of the account's analysis, with the identifiers to ask it about.
#[derive(Debug, Clone)]
struct Reader {
    operation: &'static str,
    shape: Shape,
    /// Identifiers this service holds no recommendation for, each a well-formed ARN of the
    /// one account this emulator answers for.
    unheld: Vec<(&'static str, String)>,
}

/// The ARN of one resource of the caller's own account in `region`.
fn arn(service: &str, region: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{ACCOUNT}:{resource}")
}

/// Every recommendation reader that answers about named resources, each with identifiers of
/// its own resource type that nothing has analysed.
///
/// The identifier forms differ per reader along what the rule generalises over: a resource
/// nothing ever recorded, the same resource named at ANOTHER Region's endpoint (which this
/// endpoint analyses nothing of), and — for the readers whose member takes any resource ARN —
/// another resource kind entirely.
fn readers() -> Vec<Reader> {
    vec![
        Reader {
            operation: "GetEC2InstanceRecommendations",
            shape: Shape::Many("instanceArns", "instanceRecommendations"),
            unheld: vec![
                (
                    "an instance nothing analysed",
                    arn("ec2", REGION, "instance/i-0fedcba9876543210"),
                ),
                (
                    "an instance of another Region",
                    arn("ec2", "eu-west-1", "instance/i-0123456789abcdef0"),
                ),
            ],
        },
        Reader {
            operation: "GetAutoScalingGroupRecommendations",
            shape: Shape::Many(
                "autoScalingGroupArns",
                "autoScalingGroupRecommendations",
            ),
            unheld: vec![
                (
                    "a group nothing analysed",
                    arn(
                        "autoscaling",
                        REGION,
                        "autoScalingGroup:00000000-0000-0000-0000-000000000000:\
                         autoScalingGroupName/co-probe-asg",
                    ),
                ),
                (
                    "a group of another Region",
                    arn(
                        "autoscaling",
                        "ap-southeast-2",
                        "autoScalingGroup:00000000-0000-0000-0000-000000000000:\
                         autoScalingGroupName/co-probe-asg",
                    ),
                ),
            ],
        },
        Reader {
            operation: "GetECSServiceRecommendations",
            shape: Shape::Many("serviceArns", "ecsServiceRecommendations"),
            unheld: vec![
                (
                    "a service nothing analysed",
                    arn("ecs", REGION, "service/co-probe-cluster/co-probe-service"),
                ),
                (
                    "a service of another Region",
                    arn("ecs", "eu-central-1", "service/co-probe-cluster/co-probe-service"),
                ),
            ],
        },
        Reader {
            operation: "GetIdleRecommendations",
            shape: Shape::Many("resourceArns", "idleRecommendations"),
            unheld: vec![
                (
                    "a volume nothing analysed",
                    arn("ec2", REGION, "volume/vol-0123456789abcdef0"),
                ),
                (
                    "a WorkSpace nothing analysed",
                    arn("workspaces", REGION, "workspace/ws-0123456789"),
                ),
                (
                    "a resource of another Region",
                    arn("elasticache", "us-west-2", "cluster:co-probe-cache"),
                ),
            ],
        },
        Reader {
            operation: "GetLicenseRecommendations",
            shape: Shape::Many("resourceArns", "licenseRecommendations"),
            unheld: vec![
                (
                    "an instance nothing analysed",
                    arn("ec2", REGION, "instance/i-0123456789abcdef0"),
                ),
                (
                    "an instance of another Region",
                    arn("ec2", "eu-west-1", "instance/i-0fedcba9876543210"),
                ),
            ],
        },
        Reader {
            operation: "GetRDSDatabaseRecommendations",
            shape: Shape::Many("resourceArns", "rdsDBRecommendations"),
            unheld: vec![
                (
                    "a database nothing analysed",
                    arn("rds", REGION, "db:co-probe-db"),
                ),
                (
                    "a cluster nothing analysed",
                    arn("rds", REGION, "cluster:co-probe-cluster"),
                ),
                (
                    "a database of another Region",
                    arn("rds", "eu-west-1", "db:co-probe-db"),
                ),
            ],
        },
        Reader {
            operation: "GetEC2RecommendationProjectedMetrics",
            shape: Shape::One("instanceArn"),
            unheld: vec![
                (
                    "an instance nothing analysed",
                    arn("ec2", REGION, "instance/i-0fedcba9876543210"),
                ),
                (
                    "an instance of another Region",
                    arn("ec2", "eu-west-1", "instance/i-0123456789abcdef0"),
                ),
            ],
        },
        Reader {
            operation: "GetECSServiceRecommendationProjectedMetrics",
            shape: Shape::One("serviceArn"),
            unheld: vec![
                (
                    "a service nothing analysed",
                    arn("ecs", REGION, "service/co-probe-cluster/co-probe-service"),
                ),
                (
                    "a service of another Region",
                    arn("ecs", "eu-central-1", "service/co-probe-cluster/co-probe-service"),
                ),
            ],
        },
        Reader {
            operation: "GetRDSDatabaseRecommendationProjectedMetrics",
            shape: Shape::One("resourceArn"),
            unheld: vec![
                (
                    "a database nothing analysed",
                    arn("rds", REGION, "db:co-probe-db"),
                ),
                (
                    "a cluster nothing analysed",
                    arn("rds", REGION, "cluster:co-probe-cluster"),
                ),
            ],
        },
    ]
}

/// The request one reader is driven with: the identifiers under the member it names them in,
/// plus the window a projected-metrics reader needs.
fn request(reader: &Reader, identifiers: &[String]) -> Value {
    match reader.shape {
        Shape::Many(member, _) => json!({ member: identifiers }),
        Shape::One(member) => json!({
            member: identifiers.first().cloned().unwrap_or_default(),
            "stat": "Average",
            "period": PERIOD,
            "startTime": WINDOW_START,
            "endTime": WINDOW_START + PERIOD,
        }),
    }
}

/// The list a response carries under `member`, as a list — the empty list for a member the
/// response omits, so an omitted list and an empty one read the same here.
fn items(case: &str, member: &str, body: &Value) -> Vec<Value> {
    if body[member].is_null() {
        return Vec::new();
    }
    body[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} to be a list, got {body}"))
        .clone()
}

/// Drive one reader with `identifiers` and assert the whole rule: HTTP 200, nothing reported
/// for an identifier with no recommendation, and NO absence signal of any kind.
async fn assert_empty_result(
    router: &MultiServiceRouter,
    case: &str,
    reader: &Reader,
    identifiers: &[String],
) {
    let body = request(reader, identifiers);
    let (status, response) = call(router, reader.operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {body} must be answered with 200, got {status}: {response}"
    );
    match reader.shape {
        Shape::Many(_, reported) => {
            assert_eq!(
                items(case, reported, &response),
                Vec::<Value>::new(),
                "case {case:?}: no identifier has a recommendation, so {reported} is empty, \
                 got {response}"
            );
            assert_eq!(
                items(case, "errors", &response),
                Vec::<Value>::new(),
                "case {case:?}: a resource with no recommendation yet is no error of the \
                 request, so errors is empty, got {response}"
            );
        }
        Shape::One(_) => assert_eq!(
            response["recommendedOptionProjectedMetrics"],
            json!([]),
            "case {case:?}: a resource with no recommendation has the empty list of \
             projections, got {response}"
        ),
    }
}

/// Every reader, every identifier form: one identifier at a time, and then every identifier
/// of that reader in one request — an empty result either way, whatever the request names.
#[tokio::test]
async fn a_resource_with_no_recommendation_is_an_empty_result_not_an_absence_signal() {
    let router = emulator_with_analysed_services();
    opt_in(&router).await;
    // The account's own analysis comes due one span after the enrollment record was written;
    // these rows ask about resources that analysis does not hold, so they must read the same
    // whether or not it is due.
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;

    for reader in readers() {
        for (name, identifier) in &reader.unheld {
            let case = format!("{}/{name}", reader.operation);
            assert_empty_result(
                &router,
                &case,
                &reader,
                std::slice::from_ref(identifier),
            )
            .await;
        }

        // The same identifier twice, and every identifier of this reader at once: the answer
        // is the same empty result, so no count of requested identifiers produces an entry.
        let repeated = vec![reader.unheld[0].1.clone(), reader.unheld[0].1.clone()];
        let case = format!("{}/the same identifier twice", reader.operation);
        assert_empty_result(&router, &case, &reader, &repeated).await;

        let every: Vec<String> = reader
            .unheld
            .iter()
            .map(|(_, identifier)| identifier.clone())
            .collect();
        let case = format!("{}/every unheld identifier at once", reader.operation);
        assert_empty_result(&router, &case, &reader, &every).await;
    }
}

/// The state of a FRESHLY enrolled account: the analysis its enrollment started is not due
/// yet, so every reader reports an empty result for every resource — the same answer, with no
/// absence signal, whether the request names identifiers or none at all.
#[tokio::test]
async fn a_freshly_enrolled_account_reads_empty_results_from_every_reader() {
    let router = emulator_with_analysed_services();
    opt_in(&router).await;

    for reader in readers() {
        let identifiers: Vec<String> = reader
            .unheld
            .iter()
            .map(|(_, identifier)| identifier.clone())
            .collect();
        let case = format!("{}/just enrolled", reader.operation);
        assert_empty_result(&router, &case, &reader, &identifiers).await;

        // A reader of a list of resources is also asked about NOTHING in particular: an
        // account with nothing analysed reports the empty list, and still no error.
        if let Shape::Many(_, _) = reader.shape {
            let case = format!("{}/just enrolled, no identifier named", reader.operation);
            assert_empty_result(&router, &case, &reader, &[]).await;
        }
    }
}
