// SPDX-License-Identifier: CC0-1.0

//! `GetContainerLog` driven over the wire, through the same front door a client
//! uses: `POST /` with `X-Amz-Target: Lightsail_20161128.<Operation>` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test:
//!
//! * Reading a container log is a REPORT on the container service record the store
//!   holds and on the log events it holds for that container. For every container
//!   that is running (or previously ran) on the named service, over any interval
//!   and under any filter pattern, the answer is HTTP 200 carrying a `logEvents`
//!   list — empty, because this emulator runs no container runtime, so no deployed
//!   image was ever started and nothing wrote a log line — and no `nextPageToken`,
//!   since there is no page after one that displayed everything. The call writes
//!   nothing a later caller can see.
//! * The containers it answers about are the ones a deployment that BECAME ACTIVE
//!   declared: before the clock activates a service's first deployment, no
//!   container has run on it, and a service created without a deployment never has
//!   one. Every other container name — and every service name that names no
//!   container service in the region the request is made in — earns the service's
//!   `DoesNotExist` envelope (HTTP 400, wire error `NotFoundException`, body `code`
//!   `DoesNotExist`) rather than an empty report.
//! * An interval whose end does not follow its start, and a `pageToken` this log
//!   never reported as a `nextPageToken`, are refused with the modelled 400
//!   `InvalidInputException`.
//!
//! The harness is this suite's own (rather than `container_service_lifecycle`'s,
//! whose `Emulator` is private to that module).

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::MAX_CONTAINERS_PER_DEPLOYMENT;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The endpoint every call in this suite reaches the emulator on.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The lifecycle state a container service settles in once a deployment of it is
/// active, and the one it settles in with no deployment at all (the modelled
/// `ContainerServiceState` values, as `ContainerService$state` documents them).
const RUNNING: &str = "RUNNING";
/// See [`RUNNING`].
const READY: &str = "READY";

/// An instant the intervals in this suite are built around, in epoch seconds — an
/// arbitrary fixed point, so a case's interval is the same on every run.
const WINDOW_START: i64 = 1_760_000_000;

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`, and the
        // `Authorization` header below carries only the credential SCOPE, which
        // is what names the region a request is made in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` in `region` with `body`, answering its HTTP status and
    /// parsed body.
    async fn call(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` in `region` and require `want_status`, answering the body.
    async fn ok(&self, region: &str, operation: &str, body: Value, want_status: u16) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, want_status,
            "case {operation} in {region}: expected HTTP {want_status}, got {status} {parsed:?}"
        );
        parsed
    }

    /// Create the container service `body` describes, answering the
    /// `containerService` the create reports. `CreateContainerService` answers
    /// HTTP 201 (its modelled success status).
    async fn create(&self, region: &str, body: Value) -> Value {
        self.ok(region, "CreateContainerService", body, 201).await["containerService"].clone()
    }

    /// The stored container service `name` in `region`, as `GetContainerServices`
    /// reports it.
    async fn service(&self, region: &str, name: &str) -> Value {
        self.ok(
            region,
            "GetContainerServices",
            json!({"serviceName": name}),
            200,
        )
        .await["containerServices"][0]
            .clone()
    }

    /// `GetContainerLog` for `container` on `service` in `region`, with the extra
    /// members `extra` carries merged into the request body.
    async fn log(&self, region: &str, service: &str, container: &str, extra: Value) -> (u16, Value) {
        let mut body = json!({"serviceName": service, "containerName": container});
        if let Some(members) = extra.as_object() {
            for (name, value) in members {
                body[name] = value.clone();
            }
        }
        self.call(region, "GetContainerLog", body).await
    }
}

/// The wire error code of a failure body: awsJson carries it in `__type`, either
/// bare or namespaced.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Assert that the answer is this service's `DoesNotExist` envelope: HTTP 400,
/// wire error `NotFoundException`, body `code` `DoesNotExist`, and a message.
fn assert_does_not_exist(case: &str, status: u16, refused: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(refused),
        "NotFoundException",
        "case {case}: expected the NotFoundException wire code, got {refused:?}"
    );
    assert_eq!(
        refused["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected the DoesNotExist body code, got {refused:?}"
    );
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case}: expected a message saying what does not exist, got {refused:?}"
    );
}

/// Assert that the answer is the modelled `InvalidInputException`: HTTP 400 and
/// that wire code, with a message, and NOT the `DoesNotExist` envelope.
fn assert_invalid_input(case: &str, status: u16, refused: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(refused),
        "InvalidInputException",
        "case {case}: expected the InvalidInputException wire code, got {refused:?}"
    );
    assert_ne!(
        refused["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: an invalid-input refusal is not a DoesNotExist, got {refused:?}"
    );
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case}: expected a message naming what was wrong, got {refused:?}"
    );
}

/// Assert that `answered` is an empty log report: a `logEvents` list that is
/// present and carries no event, and no `nextPageToken` at all.
fn assert_empty_log(case: &str, status: u16, answered: &Value) {
    assert_eq!(
        status, 200,
        "case {case}: expected HTTP 200, got {status} {answered:?}"
    );
    assert!(
        answered["logEvents"].is_array(),
        "case {case}: expected a logEvents list, got {answered:?}"
    );
    assert_eq!(
        answered["logEvents"],
        json!([]),
        "case {case}: nothing ran the container, so it wrote no log event, got {answered:?}"
    );
    assert!(
        answered["nextPageToken"].is_null(),
        "case {case}: a report that displayed everything has no next page, got {answered:?}"
    );
}

/// Poll `GetContainerServices` until the service `name` in `region` reports
/// `want`, answering the whole record. The transition is owed by the clock, not by
/// the poll.
async fn wait_for_state(emulator: &Emulator, region: &str, name: &str, want: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let service = emulator.service(region, name).await;
        if service["state"].as_str() == Some(want) {
            return service;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            service["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------

/// One container service to create and then read container logs from. The rows
/// differ along everything the rule generalises over: the region the requests are
/// made in, the length and shape of the service name (the shortest the modelled
/// `ContainerServiceName` pattern admits, a hyphenated one, one carrying digits),
/// how many containers the deployment declares, the form of their names, and
/// whether one of them is the deployment's public endpoint.
#[derive(Debug, Clone, Copy)]
struct ServiceRow {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    /// The containers the deployment declares. Empty means the service is created
    /// with no deployment at all, so nothing ever runs on it.
    containers: &'static [&'static str],
    /// Whether the deployment fronts its first container with a public endpoint.
    public_endpoint: bool,
}

/// The table of container services.
const SERVICE_ROWS: &[ServiceRow] = &[
    ServiceRow {
        case: "one container, fronted by a public endpoint",
        region: "us-east-1",
        name: "log-service",
        containers: &["web"],
        public_endpoint: true,
    },
    ServiceRow {
        case: "three containers and no public endpoint, on the shortest name the \
               modelled pattern admits",
        region: "us-west-2",
        name: "ab",
        containers: &["api", "sidecar", "worker"],
        public_endpoint: false,
    },
    ServiceRow {
        case: "a single-character container name on a name carrying digits",
        region: "eu-west-1",
        name: "logs-2026",
        containers: &["a"],
        public_endpoint: false,
    },
    ServiceRow {
        case: "two containers named after their images",
        region: "ap-northeast-1",
        name: "r",
        containers: &["nginx", "redis"],
        public_endpoint: true,
    },
    ServiceRow {
        case: "no deployment at all, so nothing ever ran",
        region: "us-east-2",
        name: "never-deployed",
        containers: &[],
        public_endpoint: false,
    },
];

/// The extra request members a log read may carry. The rows differ along what the
/// rule generalises over: which ends of the interval are bounded, how wide and
/// where the interval is, and the FORM of the filter pattern — each of the five
/// the modelled `filterPattern` documentation lists ("To return all log events,
/// specify a filter pattern of \"\"", `-ERROR`, `ERROR`, `ERROR Exception`,
/// `?ERROR ?Exception`) — alone and together with an interval.
#[derive(Debug, Clone, Copy)]
struct ReadRow {
    case: &'static str,
    start_time: Option<i64>,
    end_time: Option<i64>,
    filter_pattern: Option<&'static str>,
}

/// The table of reads.
const READ_ROWS: &[ReadRow] = &[
    ReadRow {
        case: "no interval and no filter at all",
        start_time: None,
        end_time: None,
        filter_pattern: None,
    },
    ReadRow {
        case: "a bounded interval one second wide",
        start_time: Some(WINDOW_START),
        end_time: Some(WINDOW_START + 1),
        filter_pattern: None,
    },
    ReadRow {
        case: "an interval bounded at its start only",
        start_time: Some(WINDOW_START),
        end_time: None,
        filter_pattern: None,
    },
    ReadRow {
        case: "an interval bounded at its end only",
        start_time: None,
        end_time: Some(WINDOW_START),
        filter_pattern: None,
    },
    ReadRow {
        case: "an interval a day wide",
        start_time: Some(WINDOW_START),
        end_time: Some(WINDOW_START + 86_400),
        filter_pattern: None,
    },
    ReadRow {
        case: "an interval entirely in the future",
        start_time: Some(WINDOW_START + 86_400),
        end_time: Some(WINDOW_START + 172_800),
        filter_pattern: None,
    },
    ReadRow {
        case: "the empty filter pattern, which returns all log events",
        start_time: None,
        end_time: None,
        filter_pattern: Some(""),
    },
    ReadRow {
        case: "a filter pattern excluding a term",
        start_time: None,
        end_time: None,
        filter_pattern: Some("-ERROR"),
    },
    ReadRow {
        case: "a filter pattern requiring a term",
        start_time: None,
        end_time: None,
        filter_pattern: Some("ERROR"),
    },
    ReadRow {
        case: "a filter pattern requiring two terms",
        start_time: None,
        end_time: None,
        filter_pattern: Some("ERROR Exception"),
    },
    ReadRow {
        case: "a filter pattern accepting either term",
        start_time: None,
        end_time: None,
        filter_pattern: Some("?ERROR ?Exception"),
    },
    ReadRow {
        case: "a filter pattern and an interval together",
        start_time: Some(WINDOW_START - 86_400),
        end_time: Some(WINDOW_START),
        filter_pattern: Some("ERROR"),
    },
];

/// The extra members `row` asks for, as a request body fragment.
fn read_body(row: &ReadRow) -> Value {
    let mut extra = json!({});
    if let Some(start) = row.start_time {
        extra["startTime"] = json!(start);
    }
    if let Some(end) = row.end_time {
        extra["endTime"] = json!(end);
    }
    if let Some(pattern) = row.filter_pattern {
        extra["filterPattern"] = json!(pattern);
    }
    extra
}

/// The create body `row` describes.
fn create_body(row: &ServiceRow) -> Value {
    let mut body = json!({"serviceName": row.name, "power": "nano", "scale": 1});
    if row.containers.is_empty() {
        return body;
    }
    let mut containers = json!({});
    for container in row.containers {
        containers[*container] = json!({"image": "nginx:latest", "ports": {"80": "HTTP"}});
    }
    let mut deployment = json!({"containers": containers});
    if row.public_endpoint {
        deployment["publicEndpoint"] =
            json!({"containerName": row.containers[0], "containerPort": 80});
    }
    body["deployment"] = deployment;
    body
}

/// Create every row's container service on `emulator` and wait for each to settle.
async fn settle(emulator: &Emulator) {
    for row in SERVICE_ROWS {
        emulator.create(row.region, create_body(row)).await;
    }
    for row in SERVICE_ROWS {
        let want = if row.containers.is_empty() {
            READY
        } else {
            RUNNING
        };
        wait_for_state(emulator, row.region, row.name, want).await;
    }
}

// ---------------------------------------------------------------------------
// What a container that ran reports
// ---------------------------------------------------------------------------

/// RULE: a container of a deployment that became active is one this service can be
/// asked about, and its log reports the events the store holds for it — none, for
/// every interval and every filter pattern, because no container runtime started
/// the deployed image — with no next page token.
#[tokio::test]
async fn a_container_that_ran_reports_an_empty_log_for_every_interval_and_filter() {
    let emulator = Emulator::new();
    settle(&emulator).await;

    for row in SERVICE_ROWS {
        for container in row.containers {
            for read in READ_ROWS {
                let case = format!("{} / {container} / {}", row.case, read.case);
                let (status, answered) = emulator
                    .log(row.region, row.name, container, read_body(read))
                    .await;
                assert_empty_log(&case, status, &answered);
            }
        }
    }
}

/// RULE: the read is a report — it writes nothing a later caller can see, so the
/// container service record is unchanged by it and a second read answers the same.
#[tokio::test]
async fn reading_a_container_log_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    settle(&emulator).await;

    for row in SERVICE_ROWS {
        let before = emulator.service(row.region, row.name).await;
        let mut answers: Vec<Value> = Vec::new();
        for container in row.containers {
            for _ in 0..3 {
                let (status, answered) = emulator
                    .log(row.region, row.name, container, json!({}))
                    .await;
                assert_empty_log(&format!("{} / {container}", row.case), status, &answered);
                answers.push(answered);
            }
        }
        // Every repeat of the same read answered the same thing.
        assert!(
            answers.windows(2).all(|pair| pair[0] == pair[1]),
            "case {}: expected every repeated read to answer the same, got {answers:?}",
            row.case
        );
        let after = emulator.service(row.region, row.name).await;
        assert_eq!(
            before, after,
            "case {}: reading a container log must not change the stored record",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// What is refused
// ---------------------------------------------------------------------------

/// RULE: a container name no deployment of the service has launched names no log
/// on it, so it is the service's `DoesNotExist` refusal — whatever its form, and
/// including a container of ANOTHER service of the same store — and the message
/// names the container the request carried.
#[tokio::test]
async fn a_container_that_never_ran_on_the_service_is_refused() {
    let emulator = Emulator::new();
    settle(&emulator).await;

    for row in SERVICE_ROWS {
        // (case, container name) — a name nothing declared, a prefix and a suffix
        // of one that did, a differently-cased one, the empty name the modelled
        // `containerName` admits (it carries no length constraint), and every
        // container of every OTHER row, which ran somewhere else.
        let mut names: Vec<(String, String)> = vec![
            ("a name no deployment declared".to_string(), "absent".to_string()),
            ("the empty container name".to_string(), String::new()),
            (
                "a container name in the wrong case".to_string(),
                row.containers
                    .first()
                    .map_or_else(|| "WEB".to_string(), |first| first.to_uppercase()),
            ),
        ];
        if let Some(first) = row.containers.first() {
            names.push((
                "a prefix of a container that ran".to_string(),
                first[..first.len() - 1].to_string(),
            ));
            names.push((
                "a suffix added to a container that ran".to_string(),
                format!("{first}-2"),
            ));
        }
        for other in SERVICE_ROWS {
            if other.name == row.name {
                continue;
            }
            for container in other.containers {
                if row.containers.contains(container) {
                    continue;
                }
                names.push((
                    format!("a container that ran on {} instead", other.name),
                    (*container).to_string(),
                ));
            }
        }

        for (name_case, container) in names {
            let case = format!("{} / {name_case}", row.case);
            let (status, refused) = emulator
                .log(row.region, row.name, &container, json!({}))
                .await;
            assert_does_not_exist(&case, status, &refused);
            assert!(
                refused["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(&container)
                        && message.contains(row.name)),
                "case {case}: expected the message to name the container and the service \
                 the request carried, got {refused:?}"
            );
        }
    }
}

/// RULE: the containers a service can be asked about are the ones a deployment
/// that BECAME ACTIVE declared — so a service whose first deployment is still
/// waiting on the clock has none yet, and the same name is refused before
/// activation and answered after it.
#[tokio::test]
async fn a_container_of_a_deployment_that_has_not_activated_yet_is_refused() {
    let emulator = Emulator::new();
    let row = ServiceRow {
        case: "a deployment still waiting on the clock",
        region: "eu-central-1",
        name: "activating",
        containers: &["web", "sidecar"],
        public_endpoint: true,
    };
    let created = emulator.create(row.region, create_body(&row)).await;

    // Only assert the early refusal while the deployment really has not become
    // the current one yet: the activation is owed by the clock, and this read
    // must not race it.
    if created["currentDeployment"].is_null() {
        for container in row.containers {
            let case = format!("{} / {container} before activation", row.case);
            let (status, refused) = emulator
                .log(row.region, row.name, container, json!({}))
                .await;
            assert_does_not_exist(&case, status, &refused);
            assert!(
                refused["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("no container has run on it")),
                "case {case}: expected the message to say nothing has run on the service yet, \
                 got {refused:?}"
            );
        }
    }

    wait_for_state(&emulator, row.region, row.name, RUNNING).await;
    for container in row.containers {
        let case = format!("{} / {container} after activation", row.case);
        let (status, answered) = emulator
            .log(row.region, row.name, container, json!({}))
            .await;
        assert_empty_log(&case, status, &answered);
    }
}

/// RULE: a service created with no deployment never runs a container, so every
/// container name is refused with a message saying no deployment of it has become
/// active.
#[tokio::test]
async fn a_service_with_no_deployment_has_no_container_to_report() {
    let emulator = Emulator::new();
    let row = ServiceRow {
        case: "no deployment at all",
        region: "us-east-1",
        name: "bare",
        containers: &[],
        public_endpoint: false,
    };
    emulator.create(row.region, create_body(&row)).await;
    wait_for_state(&emulator, row.region, row.name, READY).await;

    for container in ["web", "api", "a"] {
        let case = format!("{} / {container}", row.case);
        let (status, refused) = emulator
            .log(row.region, row.name, container, json!({}))
            .await;
        assert_does_not_exist(&case, status, &refused);
        assert!(
            refused["message"]
                .as_str()
                .is_some_and(|message| message.contains("no container has run on it")),
            "case {case}: expected the message to say nothing has run on the service yet, \
             got {refused:?}"
        );
    }
}

/// RULE: the log belongs to a container service of ONE region, so a well-formed
/// service name that names no container service in the region the request is made
/// in — including one created in another region only — is the service's
/// `DoesNotExist` refusal, naming the identifier the request carried.
#[tokio::test]
async fn a_service_name_that_names_no_container_service_is_refused() {
    let emulator = Emulator::new();
    settle(&emulator).await;

    for row in SERVICE_ROWS {
        // The same name read from every OTHER region of the table names nothing.
        for other in SERVICE_ROWS {
            if other.region == row.region {
                continue;
            }
            let case = format!("{} read from {}", row.case, other.region);
            let container = row.containers.first().copied().unwrap_or("web");
            let (status, refused) = emulator.log(other.region, row.name, container, json!({})).await;
            assert_does_not_exist(&case, status, &refused);
            assert!(
                refused["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(row.name)),
                "case {case}: expected the message to name the service the request carried, \
                 got {refused:?}"
            );
        }
    }

    for (case, service) in [
        ("a name nothing created", "absent-service"),
        ("a name that is a prefix of one that exists", "log-servic"),
        ("the shortest name the modelled pattern admits", "zz"),
    ] {
        let (status, refused) = emulator.log("us-east-1", service, "web", json!({})).await;
        assert_does_not_exist(case, status, &refused);
    }
}

/// RULE: an interval whose end does not follow its start is no interval at all, so
/// it is refused with the modelled `InvalidInputException` — for a container that
/// ran and for one that never did, since the request is wrong either way.
#[tokio::test]
async fn an_interval_whose_end_does_not_follow_its_start_is_refused() {
    let emulator = Emulator::new();
    settle(&emulator).await;

    // (case, startTime, endTime) — the two instants equal, one second inverted,
    // a day inverted, and an inverted pair straddling the epoch.
    let rows: [(&str, i64, i64); 4] = [
        ("an interval of no width at all", WINDOW_START, WINDOW_START),
        ("an interval inverted by one second", WINDOW_START, WINDOW_START - 1),
        ("an interval inverted by a day", WINDOW_START, WINDOW_START - 86_400),
        ("an inverted interval straddling the epoch", 1, -1),
    ];

    for row in SERVICE_ROWS {
        let containers: Vec<&str> = row
            .containers
            .iter()
            .copied()
            .chain(std::iter::once("absent"))
            .collect();
        for container in containers {
            for (interval_case, start, end) in rows {
                let case = format!("{} / {container} / {interval_case}", row.case);
                let (status, refused) = emulator
                    .log(
                        row.region,
                        row.name,
                        container,
                        json!({"startTime": start, "endTime": end}),
                    )
                    .await;
                assert_invalid_input(&case, status, &refused);
                assert!(
                    refused["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("startTime")
                            && message.contains("endTime")),
                    "case {case}: expected the message to name both members, got {refused:?}"
                );
            }
        }
    }
}

/// RULE: a `pageToken` is the value a previous `GetContainerLog` request reported
/// as its `nextPageToken`. This log reports none — one page displays everything it
/// holds — so every token a caller can present names no page of it and is refused
/// with the modelled `InvalidInputException`, whatever its form. An EMPTY token is
/// not a token: it asks for the first page, exactly as an absent one does.
#[tokio::test]
async fn a_page_token_this_log_never_issued_is_refused() {
    let emulator = Emulator::new();
    settle(&emulator).await;

    // The forms a token can take: a decimal position, an opaque word, a base64-ish
    // blob, a very long one, and one carrying the container's own name.
    let long_token = "t".repeat(512);
    let tokens: [(&str, &str); 5] = [
        ("a decimal position", "1"),
        ("an opaque word", "next"),
        ("a base64-ish blob", "eyJuZXh0IjoxfQ=="),
        ("a very long token", long_token.as_str()),
        ("a token naming the container", "web:0"),
    ];

    for row in SERVICE_ROWS {
        for container in row.containers {
            for (token_case, token) in tokens {
                let case = format!("{} / {container} / {token_case}", row.case);
                let (status, refused) = emulator
                    .log(row.region, row.name, container, json!({"pageToken": token}))
                    .await;
                assert_invalid_input(&case, status, &refused);
                assert!(
                    refused["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("pageToken")),
                    "case {case}: expected the message to name the member, got {refused:?}"
                );
            }

            // The empty token is the first page, which is the whole log.
            let case = format!("{} / {container} / an empty page token", row.case);
            let (status, answered) = emulator
                .log(row.region, row.name, container, json!({"pageToken": ""}))
                .await;
            assert_empty_log(&case, status, &answered);
        }
    }
}

/// RULE (the table's own source): every service name in the table is inside the
/// modelled `ContainerServiceName` length (1..63), and every row that declares
/// containers declares at most the
/// [`MAX_CONTAINERS_PER_DEPLOYMENT`] one deployment accepts, so each row exercises
/// the handler rather than the boundary walk.
#[tokio::test]
async fn the_table_rows_are_inside_the_modelled_bounds() {
    for row in SERVICE_ROWS {
        assert!(
            (1..=63).contains(&row.name.len()),
            "case {}: service name {:?} is outside the modelled 1..63 length, got {}",
            row.case,
            row.name,
            row.name.len()
        );
        assert!(
            row.containers.len() <= MAX_CONTAINERS_PER_DEPLOYMENT,
            "case {}: expected at most the {MAX_CONTAINERS_PER_DEPLOYMENT} containers one \
             deployment accepts, got {}",
            row.case,
            row.containers.len()
        );
        assert!(
            !row.public_endpoint || !row.containers.is_empty(),
            "case {}: a public endpoint must front a container the deployment declares",
            row.case
        );
    }
}
