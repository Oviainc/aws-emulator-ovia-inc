// SPDX-License-Identifier: CC0-1.0

//! What `PublishMetrics` does with the environment health metrics it is given, end to end
//! over HTTP through the router this service mounts in production.
//!
//! The rule every case here asserts: a published datum is appended to the metric series its
//! metric name TOGETHER with its dimension set identifies on the named environment's own
//! record, as the statistics of the observations it states — one `Value` being a single
//! observation that is its own sum, minimum and maximum, a `StatisticValues` set carrying its
//! own — so the series a later read aggregates is everything ever published to it; and a
//! request whose datum cannot describe observations of a metric, or whose environment this
//! Region does not hold or cannot take a change, is refused with HTTP 400
//! `ValidationException` and publishes NOTHING.
//!
//! The rows differ along what that rule generalises over: which form a datum states its
//! observations in, how many datums one request carries, how many dimensions they carry and
//! in which order, whether a unit travels with them, where the instants fall (before the
//! epoch, out of order, repeated), how large the numbers are (zero, negative, the published
//! magnitude bound and one past it), and which member of which datum of the batch breaks a
//! rule.
//!
//! `PublishMetrics` is modelled internal, and no operation of this service reports what it
//! stored, so a published series is read back through the shared engine's own state — the
//! same store the handler wrote, reached exactly as the router hands it to every handler.
//! `GetEnvironment` is asserted on too, so a publish is shown to leave the control plane's
//! view of the environment alone.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// The region every request below is signed for unless it says otherwise.
const REGION: &str = "us-east-1";

/// Another Region, for the scoping case: an environment belongs to the Region it was created
/// in, so a publish signed for this one resolves none of the environments created above.
const OTHER_REGION: &str = "us-west-2";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// A signed-looking request header so the router resolves this service by its SigV4
/// credential scope, as an SDK request does. Signature verification is off in this harness.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/airflow/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The modelled error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator, assembled as the router assembles it: one shared engine behind one mounted
/// service. The engine is kept so a suite can read the store the handlers wrote.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router, engine }
    }

    async fn send(&self, method: &str, uri: &str, region: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.us-east-1.amazonaws.com")
            .header("authorization", authorization(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body().to_vec();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "expected a JSON body, got {error}: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer { status, body }
    }

    /// One environment, carrying the members a create requires and nothing else.
    async fn create(&self, case: &str, name: &str) {
        let answer = self
            .send(
                "PUT",
                &format!("/environments/{name}"),
                REGION,
                Some(json!({
                    "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-execution"),
                    "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket-unique-name",
                    "DagS3Path": "dags",
                    "NetworkConfiguration": {
                        "SubnetIds": ["subnet-aaa111", "subnet-bbb222"],
                        "SecurityGroupIds": ["sg-111"],
                    },
                })),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the fixture create of {name} must succeed, got {} {}",
            answer.status, answer.body
        );
    }

    async fn delete(&self, case: &str, name: &str) {
        let answer = self
            .send("DELETE", &format!("/environments/{name}"), REGION, None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the delete of {name} must succeed, got {} {}",
            answer.status, answer.body
        );
    }

    /// `PublishMetrics` against `name`, signed for `region`.
    async fn publish_from(&self, region: &str, name: &str, datums: &[Value]) -> Answer {
        self.send(
            "POST",
            &format!("/metrics/environments/{name}"),
            region,
            Some(json!({"MetricData": datums})),
        )
        .await
    }

    /// `PublishMetrics` against `name`, signed for [`REGION`].
    async fn publish(&self, name: &str, datums: &[Value]) -> Answer {
        self.publish_from(REGION, name, datums).await
    }

    /// The `Environment` a successful `GetEnvironment` reports.
    async fn environment(&self, case: &str, name: &str) -> Value {
        let answer = self
            .send("GET", &format!("/environments/{name}"), REGION, None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected GetEnvironment 200, got {} {}",
            answer.status, answer.body
        );
        answer
            .body
            .get("Environment")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: GetEnvironment answered without Environment"))
    }

    /// One stored metric series of `name`'s record, reached through the shared engine — the
    /// store the handler wrote.
    fn series(
        &self,
        name: &str,
        metric: &str,
        dimensions: &[(String, String)],
    ) -> Option<state::MetricSeries> {
        let store = state::service_state(&*self.engine).expect("the shared store");
        let store = store.lock();
        store
            .environment(REGION, name)?
            .metrics
            .series_of(metric, &dimension_map(dimensions))
            .cloned()
    }

    /// How many series `name`'s record holds, and how many datapoints across them.
    fn stored_counts(&self, name: &str) -> (usize, usize) {
        let store = state::service_state(&*self.engine).expect("the shared store");
        let store = store.lock();
        match store.environment(REGION, name) {
            Some(record) => (record.metrics.series.len(), record.metrics.datapoint_count()),
            None => (0, 0),
        }
    }
}

/// The dimension set as the series key holds it: a map from dimension name to value.
fn dimension_map(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// A dimension list as a `MetricDatum.Dimensions` member states it, in the order given.
fn dimension_members(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(name, value)| json!({"Name": name, "Value": value}))
            .collect(),
    )
}

/// An owned dimension set from its pairs.
fn dims(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// A datum stating ONE observation, at `seconds` past the epoch.
fn value_datum(metric: &str, seconds: i64, value: f64) -> Value {
    json!({"MetricName": metric, "Timestamp": seconds, "Value": value})
}

/// A datum stating a SET of observations, at `seconds` past the epoch.
fn statistic_datum(metric: &str, seconds: i64, set: Value) -> Value {
    json!({"MetricName": metric, "Timestamp": seconds, "StatisticValues": set})
}

/// `datum` with `member` set to `value` — how a row adds a unit or a dimension list.
fn with(datum: Value, member: &str, value: Value) -> Value {
    let mut datum = datum;
    datum
        .as_object_mut()
        .expect("an object datum")
        .insert(member.to_string(), value);
    datum
}

/// The statistics of a set of observations, as the store holds them.
fn statistics(sample_count: i64, sum: f64, minimum: f64, maximum: f64) -> state::MetricStatistics {
    state::MetricStatistics {
        sample_count,
        sum,
        minimum,
        maximum,
    }
}

/// What one stored series must hold after a row's publish.
struct Expectation {
    metric: &'static str,
    dimensions: Vec<(String, String)>,
    /// The instants of the stored datapoints, in the order the series holds them.
    timestamps_millis: Vec<i64>,
    /// The units of those datapoints, in the same order.
    units: Vec<Option<&'static str>>,
    /// The statistics of the whole series, and its mean observation.
    statistics: state::MetricStatistics,
    average: f64,
}

/// One accepted publish: the datums it carries, and the series they must leave behind.
struct Accepted {
    case: &'static str,
    datums: Vec<Value>,
    expect: Vec<Expectation>,
}

/// One refused publish: the datums it carries, and the fragments its refusal must name — the
/// member path the boundary validator would use for that datum, and the rule it broke.
struct Refused {
    case: &'static str,
    datums: Vec<Value>,
    mentions: Vec<String>,
}

/// The magnitude bound a metric value stays inside, from
/// [`state::max_metric_value_magnitude`].
fn bound() -> f64 {
    state::max_metric_value_magnitude()
}

/// Every accepted publish this suite asserts the rule over.
fn accepted() -> Vec<Accepted> {
    vec![
        Accepted {
            case: "one value datum, no dimensions and no unit",
            datums: vec![value_datum("CpuUtilization", 1_700_000_000, 42.5)],
            expect: vec![Expectation {
                metric: "CpuUtilization",
                dimensions: Vec::new(),
                timestamps_millis: vec![1_700_000_000_000],
                units: vec![None],
                statistics: statistics(1, 42.5, 42.5, 42.5),
                average: 42.5,
            }],
        },
        Accepted {
            case: "a value datum carrying a unit and one dimension",
            datums: vec![with(
                with(
                    value_datum("QueuedTasks", 1_700_000_060, 7.0),
                    "Unit",
                    json!("Count"),
                ),
                "Dimensions",
                dimension_members(&[("Function", "Scheduler")]),
            )],
            expect: vec![Expectation {
                metric: "QueuedTasks",
                dimensions: dims(&[("Function", "Scheduler")]),
                timestamps_millis: vec![1_700_000_060_000],
                units: vec![Some("Count")],
                statistics: statistics(1, 7.0, 7.0, 7.0),
                average: 7.0,
            }],
        },
        Accepted {
            case: "a statistic set is stored as the set it summarises",
            datums: vec![statistic_datum(
                "TaskDuration",
                1_700_000_120,
                json!({"SampleCount": 4, "Sum": 10.0, "Minimum": 1.0, "Maximum": 5.0}),
            )],
            expect: vec![Expectation {
                metric: "TaskDuration",
                dimensions: Vec::new(),
                timestamps_millis: vec![1_700_000_120_000],
                units: vec![None],
                statistics: statistics(4, 10.0, 1.0, 5.0),
                average: 2.5,
            }],
        },
        Accepted {
            case: "two datums of one series aggregate rather than replace",
            datums: vec![
                value_datum("CpuUtilization", 1_700_000_000, 2.0),
                value_datum("CpuUtilization", 1_700_000_060, 4.0),
            ],
            expect: vec![Expectation {
                metric: "CpuUtilization",
                dimensions: Vec::new(),
                timestamps_millis: vec![1_700_000_000_000, 1_700_000_060_000],
                units: vec![None, None],
                statistics: statistics(2, 6.0, 2.0, 4.0),
                average: 3.0,
            }],
        },
        Accepted {
            case: "two observations at the SAME instant are both kept",
            datums: vec![
                value_datum("CpuUtilization", 1_700_000_000, 2.0),
                value_datum("CpuUtilization", 1_700_000_000, 6.0),
            ],
            expect: vec![Expectation {
                metric: "CpuUtilization",
                dimensions: Vec::new(),
                timestamps_millis: vec![1_700_000_000_000, 1_700_000_000_000],
                units: vec![None, None],
                statistics: statistics(2, 8.0, 2.0, 6.0),
                average: 4.0,
            }],
        },
        Accepted {
            case: "a value and a statistic set of one metric aggregate together",
            datums: vec![
                value_datum("TaskDuration", 1_700_000_000, 3.0),
                statistic_datum(
                    "TaskDuration",
                    1_700_000_060,
                    json!({"SampleCount": 2, "Sum": 9.0, "Minimum": 4.0, "Maximum": 5.0}),
                ),
            ],
            expect: vec![Expectation {
                metric: "TaskDuration",
                dimensions: Vec::new(),
                timestamps_millis: vec![1_700_000_000_000, 1_700_000_060_000],
                units: vec![None, None],
                statistics: statistics(3, 12.0, 3.0, 5.0),
                average: 4.0,
            }],
        },
        Accepted {
            case: "the same dimensions in another order are the same series",
            datums: vec![
                with(
                    value_datum("QueuedTasks", 1_700_000_000, 1.0),
                    "Dimensions",
                    dimension_members(&[("Function", "Worker"), ("Queue", "default")]),
                ),
                with(
                    value_datum("QueuedTasks", 1_700_000_060, 3.0),
                    "Dimensions",
                    dimension_members(&[("Queue", "default"), ("Function", "Worker")]),
                ),
            ],
            expect: vec![Expectation {
                metric: "QueuedTasks",
                dimensions: dims(&[("Function", "Worker"), ("Queue", "default")]),
                timestamps_millis: vec![1_700_000_000_000, 1_700_000_060_000],
                units: vec![None, None],
                statistics: statistics(2, 4.0, 1.0, 3.0),
                average: 2.0,
            }],
        },
        Accepted {
            case: "one metric name under two dimension sets is two series",
            datums: vec![
                with(
                    value_datum("QueuedTasks", 1_700_000_000, 1.0),
                    "Dimensions",
                    dimension_members(&[("Queue", "alpha")]),
                ),
                with(
                    value_datum("QueuedTasks", 1_700_000_000, 5.0),
                    "Dimensions",
                    dimension_members(&[("Queue", "beta")]),
                ),
            ],
            expect: vec![
                Expectation {
                    metric: "QueuedTasks",
                    dimensions: dims(&[("Queue", "alpha")]),
                    timestamps_millis: vec![1_700_000_000_000],
                    units: vec![None],
                    statistics: statistics(1, 1.0, 1.0, 1.0),
                    average: 1.0,
                },
                Expectation {
                    metric: "QueuedTasks",
                    dimensions: dims(&[("Queue", "beta")]),
                    timestamps_millis: vec![1_700_000_000_000],
                    units: vec![None],
                    statistics: statistics(1, 5.0, 5.0, 5.0),
                    average: 5.0,
                },
            ],
        },
        Accepted {
            case: "datums published out of order are held ascending by instant",
            datums: vec![
                value_datum("SchedulerHeartbeat", 1_700_000_180, 3.0),
                value_datum("SchedulerHeartbeat", 1_700_000_000, 1.0),
                value_datum("SchedulerHeartbeat", 1_700_000_060, 2.0),
            ],
            expect: vec![Expectation {
                metric: "SchedulerHeartbeat",
                dimensions: Vec::new(),
                timestamps_millis: vec![
                    1_700_000_000_000,
                    1_700_000_060_000,
                    1_700_000_180_000,
                ],
                units: vec![None, None, None],
                statistics: statistics(3, 6.0, 1.0, 3.0),
                average: 2.0,
            }],
        },
        Accepted {
            case: "an instant before the epoch, and zero as a value",
            datums: vec![value_datum("CpuUtilization", -86_400, 0.0)],
            expect: vec![Expectation {
                metric: "CpuUtilization",
                dimensions: Vec::new(),
                timestamps_millis: vec![-86_400_000],
                units: vec![None],
                statistics: statistics(1, 0.0, 0.0, 0.0),
                average: 0.0,
            }],
        },
        Accepted {
            case: "values at each end of the published magnitude bound",
            datums: vec![
                value_datum("MemoryUtilization", 1_700_000_000, -bound()),
                value_datum("MemoryUtilization", 1_700_000_060, bound()),
            ],
            expect: vec![Expectation {
                metric: "MemoryUtilization",
                dimensions: Vec::new(),
                timestamps_millis: vec![1_700_000_000_000, 1_700_000_060_000],
                units: vec![None, None],
                statistics: statistics(2, 0.0, -bound(), bound()),
                average: 0.0,
            }],
        },
        Accepted {
            case: "a datum carrying the maximum number of dimensions",
            datums: vec![with(
                value_datum("CpuUtilization", 1_700_000_000, 9.0),
                "Dimensions",
                Value::Array(
                    (1..=state::MAX_METRIC_DIMENSIONS)
                        .map(|position| json!({"Name": format!("D{position}"), "Value": "v"}))
                        .collect(),
                ),
            )],
            expect: vec![Expectation {
                metric: "CpuUtilization",
                dimensions: (1..=state::MAX_METRIC_DIMENSIONS)
                    .map(|position| (format!("D{position}"), "v".to_string()))
                    .collect(),
                timestamps_millis: vec![1_700_000_000_000],
                units: vec![None],
                statistics: statistics(1, 9.0, 9.0, 9.0),
                average: 9.0,
            }],
        },
        Accepted {
            case: "a batch of no datums publishes nothing",
            datums: Vec::new(),
            expect: Vec::new(),
        },
    ]
}

/// Every refused publish this suite asserts the rule over. Each names the member path of the
/// offending datum and the rule it broke.
fn refused() -> Vec<Refused> {
    let over_bound = bound() * 2.0;
    vec![
        Refused {
            case: "a datum stating both a value and a statistic set",
            datums: vec![with(
                value_datum("CpuUtilization", 1_700_000_000, 1.0),
                "StatisticValues",
                json!({"SampleCount": 1, "Sum": 1.0, "Minimum": 1.0, "Maximum": 1.0}),
            )],
            mentions: vec![
                "metricData.1.member.value".to_string(),
                "mutually exclusive".to_string(),
            ],
        },
        Refused {
            case: "a datum stating neither a value nor a statistic set",
            datums: vec![json!({"MetricName": "CpuUtilization", "Timestamp": 1_700_000_000})],
            mentions: vec!["either metricData.1.member.value".to_string()],
        },
        Refused {
            case: "a value larger than the published magnitude bound",
            datums: vec![value_datum("CpuUtilization", 1_700_000_000, over_bound)],
            mentions: vec![
                "metricData.1.member.value is not a metric value".to_string(),
                "2^360".to_string(),
            ],
        },
        Refused {
            case: "a statistic set whose sum is past that bound",
            datums: vec![statistic_datum(
                "TaskDuration",
                1_700_000_000,
                json!({"SampleCount": 2, "Sum": over_bound, "Minimum": 1.0, "Maximum": 2.0}),
            )],
            mentions: vec![
                "metricData.1.member.statisticValues.sum is not a metric value".to_string()
            ],
        },
        Refused {
            case: "a statistic set missing its sum",
            datums: vec![statistic_datum(
                "TaskDuration",
                1_700_000_000,
                json!({"SampleCount": 2, "Minimum": 1.0, "Maximum": 2.0}),
            )],
            mentions: vec![
                "metricData.1.member.statisticValues.sum must be specified".to_string()
            ],
        },
        Refused {
            case: "a statistic set missing its sample count",
            datums: vec![statistic_datum(
                "TaskDuration",
                1_700_000_000,
                json!({"Sum": 3.0, "Minimum": 1.0, "Maximum": 2.0}),
            )],
            mentions: vec![
                "metricData.1.member.statisticValues.sampleCount must be specified".to_string(),
            ],
        },
        Refused {
            case: "a statistic set counting no sample",
            datums: vec![statistic_datum(
                "TaskDuration",
                1_700_000_000,
                json!({"SampleCount": 0, "Sum": 0.0, "Minimum": 0.0, "Maximum": 0.0}),
            )],
            mentions: vec!["sampleCount must be greater than 0".to_string()],
        },
        Refused {
            case: "a statistic set counting a negative number of samples",
            datums: vec![statistic_datum(
                "TaskDuration",
                1_700_000_000,
                json!({"SampleCount": -2, "Sum": 4.0, "Minimum": 1.0, "Maximum": 3.0}),
            )],
            mentions: vec!["sampleCount must be greater than 0".to_string()],
        },
        Refused {
            case: "a statistic set whose minimum exceeds its maximum",
            datums: vec![statistic_datum(
                "TaskDuration",
                1_700_000_000,
                json!({"SampleCount": 2, "Sum": 4.0, "Minimum": 3.0, "Maximum": 1.0}),
            )],
            mentions: vec!["minimum must be less than or equal to".to_string()],
        },
        Refused {
            case: "a blank metric name",
            datums: vec![value_datum("   ", 1_700_000_000, 1.0)],
            mentions: vec!["metricData.1.member.metricName must not be blank".to_string()],
        },
        Refused {
            case: "a metric name one character past the length kept",
            datums: vec![value_datum(
                &"m".repeat(state::MAX_METRIC_LABEL_LENGTH + 1),
                1_700_000_000,
                1.0,
            )],
            mentions: vec![format!(
                "metricData.1.member.metricName must have length less than or equal to {}",
                state::MAX_METRIC_LABEL_LENGTH
            )],
        },
        Refused {
            case: "a dimension whose value is blank",
            datums: vec![with(
                value_datum("QueuedTasks", 1_700_000_000, 1.0),
                "Dimensions",
                dimension_members(&[("Queue", "")]),
            )],
            mentions: vec![
                "metricData.1.member.dimensions.1.member.value must not be blank".to_string(),
            ],
        },
        Refused {
            case: "a dimension whose name is blank, in second position",
            datums: vec![with(
                value_datum("QueuedTasks", 1_700_000_000, 1.0),
                "Dimensions",
                dimension_members(&[("Queue", "default"), (" ", "v")]),
            )],
            mentions: vec![
                "metricData.1.member.dimensions.2.member.name must not be blank".to_string(),
            ],
        },
        Refused {
            case: "one dimension past the maximum number of them",
            datums: vec![with(
                value_datum("CpuUtilization", 1_700_000_000, 1.0),
                "Dimensions",
                Value::Array(
                    (1..=state::MAX_METRIC_DIMENSIONS + 1)
                        .map(|position| json!({"Name": format!("D{position}"), "Value": "v"}))
                        .collect(),
                ),
            )],
            mentions: vec![format!(
                "metricData.1.member.dimensions must have length less than or equal to {}",
                state::MAX_METRIC_DIMENSIONS
            )],
        },
        Refused {
            case: "a datum naming one dimension twice",
            datums: vec![with(
                value_datum("QueuedTasks", 1_700_000_000, 1.0),
                "Dimensions",
                dimension_members(&[("Queue", "alpha"), ("Queue", "beta")]),
            )],
            mentions: vec!["names the dimension Queue more than once".to_string()],
        },
        Refused {
            case: "a unit outside the value set the model fixes",
            datums: vec![with(
                value_datum("CpuUtilization", 1_700_000_000, 1.0),
                "Unit",
                json!("Furlongs"),
            )],
            mentions: vec!["metricData.1.member.unit".to_string()],
        },
        Refused {
            // The whole batch is refused, so the GOOD datum ahead of the bad one is not
            // published either: the assertion below that nothing was stored is what pins it.
            case: "a batch whose SECOND datum breaks a rule",
            datums: vec![
                value_datum("CpuUtilization", 1_700_000_000, 1.0),
                json!({"MetricName": "QueuedTasks", "Timestamp": 1_700_000_060}),
            ],
            mentions: vec!["metricData.2.member".to_string()],
        },
    ]
}

/// `name` for the row at `index` of a table. `EnvironmentName` is `^[a-zA-Z][0-9a-zA-Z-_]*$`,
/// so a row is named from its table and its position rather than from its own prose.
fn env_name(table: &str, index: usize) -> String {
    format!("{table}Row{index}")
}

/// Assert one series holds exactly what a row expects of it.
fn assert_series(harness: &Harness, case: &str, name: &str, expect: &Expectation) {
    let stored = harness
        .series(name, expect.metric, &expect.dimensions)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: nothing was published for {} under {:?}",
                expect.metric, expect.dimensions
            )
        });
    assert_eq!(
        stored.metric_name, expect.metric,
        "case {case:?}: expected the series of {}, got {:?}",
        expect.metric, stored.metric_name
    );
    assert_eq!(
        stored.dimensions,
        dimension_map(&expect.dimensions),
        "case {case:?}: expected the dimensions {:?}, got {:?}",
        expect.dimensions,
        stored.dimensions
    );
    let instants: Vec<i64> = stored
        .datapoints
        .iter()
        .map(|datapoint| datapoint.timestamp_millis)
        .collect();
    assert_eq!(
        instants, expect.timestamps_millis,
        "case {case:?}: expected the datapoints at {:?}, got {instants:?}",
        expect.timestamps_millis
    );
    let units: Vec<Option<String>> = stored
        .datapoints
        .iter()
        .map(|datapoint| datapoint.unit.clone())
        .collect();
    let wanted: Vec<Option<String>> = expect
        .units
        .iter()
        .map(|unit| unit.map(str::to_string))
        .collect();
    assert_eq!(
        units, wanted,
        "case {case:?}: expected the units {wanted:?}, got {units:?}"
    );
    let aggregated = stored.statistics();
    assert_eq!(
        aggregated,
        Some(expect.statistics),
        "case {case:?}: expected the series statistics {:?}, got {aggregated:?}",
        expect.statistics
    );
    let average = aggregated.and_then(|statistics| statistics.average());
    assert_eq!(
        average,
        Some(expect.average),
        "case {case:?}: expected the mean observation {}, got {average:?}",
        expect.average
    );
}

#[tokio::test]
async fn a_published_datum_lands_in_the_series_its_name_and_dimensions_identify() {
    let harness = Harness::new();
    for (index, row) in accepted().iter().enumerate() {
        let case = row.case;
        let name = env_name("Accepted", index);
        harness.create(case, &name).await;

        let answer = harness.publish(&name, &row.datums).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected PublishMetrics 200, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body,
            json!({}),
            "case {case:?}: PublishMetrics answers the modelled empty output, got {}",
            answer.body
        );

        for expect in &row.expect {
            assert_series(&harness, case, &name, expect);
        }
        let (series, datapoints) = harness.stored_counts(&name);
        assert_eq!(
            series,
            row.expect.len(),
            "case {case:?}: expected {} series, got {series}",
            row.expect.len()
        );
        let published: usize = row
            .expect
            .iter()
            .map(|expect| expect.timestamps_millis.len())
            .sum();
        assert_eq!(
            datapoints, published,
            "case {case:?}: expected {published} datapoints in all, got {datapoints}"
        );
    }
}

#[tokio::test]
async fn a_datum_that_describes_no_observation_refuses_the_whole_request() {
    let harness = Harness::new();
    for (index, row) in refused().iter().enumerate() {
        let case = row.case;
        let name = env_name("Refused", index);
        harness.create(case, &name).await;

        // The control: a datum the rules accept is published to this very environment, so no
        // assertion below could pass against a service that refused everything.
        let served = harness
            .publish(&name, &[value_datum("Control", 1_700_000_000, 1.0)])
            .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: the control publish must be served, got {} {}",
            served.status, served.body
        );
        let before = harness.stored_counts(&name);

        let answer = harness.publish(&name, &row.datums).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {}",
            answer.body
        );
        let message = answer.message();
        for fragment in &row.mentions {
            assert!(
                message.contains(fragment.as_str()),
                "case {case:?}: the refusal must name {fragment:?}, got {message:?}"
            );
        }
        assert_eq!(
            harness.stored_counts(&name),
            before,
            "case {case:?}: a refused publish must store nothing, got {:?}",
            harness.stored_counts(&name)
        );
    }
}

#[tokio::test]
async fn metrics_are_scoped_to_one_environment_in_one_region() {
    let harness = Harness::new();
    let case = "scoping";
    for name in ["ScopedOne", "ScopedTwo"] {
        harness.create(case, name).await;
    }

    assert_eq!(
        harness
            .publish("ScopedOne", &[value_datum("CpuUtilization", 1_700_000_000, 1.0)])
            .await
            .status,
        200,
        "case {case:?}: the publish to ScopedOne must be served"
    );

    assert_eq!(
        harness.stored_counts("ScopedOne"),
        (1, 1),
        "case {case:?}: ScopedOne must hold the one datapoint published to it, got {:?}",
        harness.stored_counts("ScopedOne")
    );
    assert_eq!(
        harness.stored_counts("ScopedTwo"),
        (0, 0),
        "case {case:?}: a publish to one environment must not reach another, got {:?}",
        harness.stored_counts("ScopedTwo")
    );

    // An environment lives in the Region it was created in: the same name signed for another
    // Region resolves to nothing, and a publish there is refused as a name that is not there.
    let elsewhere = harness
        .publish_from(
            OTHER_REGION,
            "ScopedOne",
            &[value_datum("CpuUtilization", 1_700_000_000, 1.0)],
        )
        .await;
    assert_eq!(
        elsewhere.status, 400,
        "case {case:?}: expected HTTP 400 from another Region, got {} {}",
        elsewhere.status, elsewhere.body
    );
    assert_eq!(
        elsewhere.message(),
        state::environment_not_found_message("ScopedOne"),
        "case {case:?}: the refusal must name the environment the request asked for, got {:?}",
        elsewhere.message()
    );

    // A name no Region holds is refused the same way.
    let unknown = harness
        .publish("NeverCreated", &[value_datum("CpuUtilization", 1_700_000_000, 1.0)])
        .await;
    assert_eq!(
        unknown.status, 400,
        "case {case:?}: expected HTTP 400 for an unknown environment, got {} {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.message(),
        state::environment_not_found_message("NeverCreated"),
        "case {case:?}: the refusal must name the environment asked for, got {:?}",
        unknown.message()
    );
}

#[tokio::test]
async fn publishing_leaves_the_environment_the_control_plane_reports_alone() {
    let harness = Harness::new();
    let case = "control plane";
    let name = "ReportedEnv";
    harness.create(case, name).await;

    let before = harness.environment(case, name).await;
    assert_eq!(
        harness
            .publish(
                name,
                &[with(
                    value_datum("CpuUtilization", 1_700_000_000, 11.0),
                    "Unit",
                    json!("Percent"),
                )]
            )
            .await
            .status,
        200,
        "case {case:?}: the publish must be served"
    );
    let after = harness.environment(case, name).await;

    // Only the status moves, and only because the clock owes the create its transition; what
    // a publish wrote is not part of what the control plane reports.
    let strip = |environment: &Value| {
        let mut environment = environment.clone();
        if let Some(object) = environment.as_object_mut() {
            object.remove("Status");
        }
        environment
    };
    assert_eq!(
        strip(&after),
        strip(&before),
        "case {case:?}: a publish must not change the reported environment, got {after}"
    );
    assert_eq!(
        harness.stored_counts(name),
        (1, 1),
        "case {case:?}: the datapoint must still be stored, got {:?}",
        harness.stored_counts(name)
    );
}

#[tokio::test]
async fn a_series_keeps_its_most_recent_datapoints() {
    let harness = Harness::new();
    let case = "retention";
    let name = "RetainingEnv";
    harness.create(case, name).await;

    // Five datums past the window, so what ages out is the oldest five.
    let overflow = 5;
    let total = state::MAX_METRIC_DATAPOINTS_PER_SERIES + overflow;
    let datums: Vec<Value> = (0..total)
        .map(|position| {
            value_datum(
                "CpuUtilization",
                1_700_000_000 + position as i64,
                position as f64,
            )
        })
        .collect();
    let answer = harness.publish(name, &datums).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected PublishMetrics 200, got {} {}",
        answer.status, answer.body
    );

    let stored = harness
        .series(name, "CpuUtilization", &[])
        .unwrap_or_else(|| panic!("case {case:?}: nothing was published"));
    assert_eq!(
        stored.datapoints.len(),
        state::MAX_METRIC_DATAPOINTS_PER_SERIES,
        "case {case:?}: expected the series to hold {}, got {}",
        state::MAX_METRIC_DATAPOINTS_PER_SERIES,
        stored.datapoints.len()
    );
    let first = stored.datapoints.first().expect("a datapoint").clone();
    assert_eq!(
        first.timestamp_millis,
        (1_700_000_000 + overflow as i64) * 1_000,
        "case {case:?}: the oldest datapoints must be the ones dropped, got {first:?}"
    );
    let aggregated = stored.statistics().expect("a non-empty series");
    assert_eq!(
        aggregated.sample_count,
        state::MAX_METRIC_DATAPOINTS_PER_SERIES as i64,
        "case {case:?}: every held datapoint counts one observation, got {aggregated:?}"
    );
    assert_eq!(
        aggregated.minimum, overflow as f64,
        "case {case:?}: the smallest observation still held is {overflow}, got {aggregated:?}"
    );
    assert_eq!(
        aggregated.maximum,
        (total - 1) as f64,
        "case {case:?}: the largest observation is the last published, got {aggregated:?}"
    );
}

#[tokio::test]
async fn a_batch_past_the_series_ceiling_publishes_nothing() {
    let harness = Harness::new();
    let case = "series ceiling";
    let name = "CeilingEnv";
    harness.create(case, name).await;

    // One datum per series, filling the environment to the ceiling exactly. The last two
    // datums share a series, so the batch opens one fewer series than it carries datums —
    // which is what the ceiling is judged against.
    let filling: Vec<Value> = (0..state::MAX_METRIC_SERIES)
        .map(|position| {
            with(
                value_datum("CpuUtilization", 1_700_000_000, position as f64),
                "Dimensions",
                dimension_members(&[("Slot", &position.to_string())]),
            )
        })
        .chain(std::iter::once(with(
            value_datum("CpuUtilization", 1_700_000_060, 1.0),
            "Dimensions",
            dimension_members(&[("Slot", "0")]),
        )))
        .collect();
    let answer = harness.publish(name, &filling).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a batch filling the ceiling exactly must be served, got {} {}",
        answer.status, answer.body
    );
    let before = harness.stored_counts(name);
    assert_eq!(
        before,
        (state::MAX_METRIC_SERIES, state::MAX_METRIC_SERIES + 1),
        "case {case:?}: expected the ceiling filled with one series carrying two datapoints, \
         got {before:?}"
    );

    // One series past it, in a batch whose FIRST datum would have been accepted: the whole
    // batch is refused, so neither datum is stored.
    let refused = harness
        .publish(
            name,
            &[
                with(
                    value_datum("CpuUtilization", 1_700_000_120, 2.0),
                    "Dimensions",
                    dimension_members(&[("Slot", "0")]),
                ),
                with(
                    value_datum("CpuUtilization", 1_700_000_120, 3.0),
                    "Dimensions",
                    dimension_members(&[("Slot", "past-the-ceiling")]),
                ),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400 past the ceiling, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {}",
        refused.body
    );
    assert_eq!(
        refused.message(),
        format!(
            "this emulator holds at most {} metric series for one environment",
            state::MAX_METRIC_SERIES
        ),
        "case {case:?}: the refusal must name the ceiling, got {:?}",
        refused.message()
    );
    assert_eq!(
        harness.stored_counts(name),
        before,
        "case {case:?}: the refused batch must store nothing, got {:?}",
        harness.stored_counts(name)
    );

    // A datapoint for a series the environment ALREADY holds is still published: the ceiling
    // is on how many series there are, not on how many datapoints they hold.
    let served = harness
        .publish(
            name,
            &[with(
                value_datum("CpuUtilization", 1_700_000_180, 4.0),
                "Dimensions",
                dimension_members(&[("Slot", "1")]),
            )],
        )
        .await;
    assert_eq!(
        served.status, 200,
        "case {case:?}: a datapoint of a series already held must be served, got {} {}",
        served.status, served.body
    );
    assert_eq!(
        harness.stored_counts(name),
        (before.0, before.1 + 1),
        "case {case:?}: expected one more datapoint and no new series, got {:?}",
        harness.stored_counts(name)
    );
}

/// The numbers a metric value may carry, judged directly on the rule the handler applies to
/// a `Value` and to every member of a statistic set alike.
///
/// Two of these cannot travel over restJson1 at all — JSON has no literal for NaN or an
/// infinity — so the rule's finite half is asserted here rather than through a request, while
/// the magnitude half is asserted both here and over HTTP by the tables above.
#[test]
fn the_numbers_a_metric_value_may_carry() {
    let magnitude = state::max_metric_value_magnitude();
    for (case, value, publishable) in [
        ("zero", 0.0, true),
        ("a whole number", 42.0, true),
        ("a negative fraction", -0.5, true),
        ("the smallest positive number", f64::MIN_POSITIVE, true),
        ("the published magnitude bound", magnitude, true),
        ("the bound, negated", -magnitude, true),
        ("just past the bound", magnitude * 2.0, false),
        ("just past the bound, negated", -magnitude * 2.0, false),
        ("the largest finite number", f64::MAX, false),
        ("positive infinity", f64::INFINITY, false),
        ("negative infinity", f64::NEG_INFINITY, false),
        ("not a number", f64::NAN, false),
    ] {
        assert_eq!(
            state::is_publishable_metric_value(value),
            publishable,
            "case {case:?}: expected publishable={publishable} for {value:?}"
        );
    }
}

#[tokio::test]
async fn an_environment_being_torn_down_publishes_nothing() {
    let harness = Harness::new();
    let case = "teardown";
    let name = "TornDownEnv";
    harness.create(case, name).await;
    assert_eq!(
        harness
            .publish(name, &[value_datum("CpuUtilization", 1_700_000_000, 1.0)])
            .await
            .status,
        200,
        "case {case:?}: the publish before the teardown must be served"
    );
    let before = harness.stored_counts(name);

    harness.delete(case, name).await;
    let refused = harness
        .publish(name, &[value_datum("CpuUtilization", 1_700_000_060, 2.0)])
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400 while the teardown runs, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {}",
        refused.body
    );
    assert_eq!(
        refused.message(),
        state::change_not_allowed_message(name, state::STATUS_DELETING),
        "case {case:?}: the refusal must name the environment and its status, got {:?}",
        refused.message()
    );
    assert_eq!(
        harness.stored_counts(name),
        before,
        "case {case:?}: the refused publish must store nothing, got {:?}",
        harness.stored_counts(name)
    );
}
