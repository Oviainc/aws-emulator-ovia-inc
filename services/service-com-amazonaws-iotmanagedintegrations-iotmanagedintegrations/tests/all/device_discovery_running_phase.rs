// SPDX-License-Identifier: CC0-1.0

//! The RUNNING phase of a device discovery job, as a POLLING client sees it:
//! `StartDeviceDiscovery` -> repeated `GetDeviceDiscovery` and `ListDeviceDiscoveries`.
//!
//! RULE: a device discovery job is ASYNCHRONOUS. `StartDeviceDiscovery` (POST
//! `/device-discoveries`, 201) answers with the job's id and start time only, and the job
//! reports the non-terminal `DeviceDiscoveryStatus` value `RUNNING`, with no `FinishedAt`,
//! until ITS OWN due instant — the span the clock owes that job, counted from that job's
//! own start (`state::DEVICE_DISCOVERY_DURATION`), long enough for a client to observe the
//! phase over several reads. From that instant on, and on every later read, the job reports
//! the terminal `SUCCEEDED` with the `FinishedAt` of that same instant. Every reader derives
//! the status from the clock, so `GetDeviceDiscovery` and the `ListDeviceDiscoveries`
//! `StatusFilter` always agree, and polling changes nothing a later caller sees: the status
//! and the finish time do not depend on how often, or whether, anybody read the job.
//!
//! The recorded unit this was derived from reads one job once, straight after starting it;
//! the cases below are the class that read is one row of. They differ along what the rule
//! generalises over: WHEN the read happens relative to the job's due instant (before it, on
//! every poll of a loop that runs across it, and after it), HOW MANY jobs are in flight and
//! how far apart they were started, how many times each job is read, which reader reports
//! the status (the get, and the list's `StatusFilter` both ways), and the `DiscoveryType`
//! the job runs — the model's whole value set
//! (`smithy_query(query="shape",shape="DiscoveryType")`).
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses and bodies asserted here are the ones a client sees.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::ops::types::DISCOVERY_TYPE_VALUES;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    DEVICE_DISCOVERY_DURATION, DEVICE_DISCOVERY_STATUS_RUNNING, DEVICE_DISCOVERY_STATUS_SUCCEEDED,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// How often the polling client reads: one sixth of the span the clock owes a job, so a
/// loop that waits for the job to finish reads it several times on the way.
const POLL_INTERVAL: Duration =
    Duration::from_nanos((DEVICE_DISCOVERY_DURATION.as_nanos() / 6) as u64);

/// The fewest `RUNNING` reads a poll loop at [`POLL_INTERVAL`] must collect before the job
/// finishes. A running phase one read can race is not a lifecycle a completion wait,
/// timeout or retry path can be written against.
const MIN_RUNNING_READS: usize = 3;

/// The most polls the loop takes before giving up, so a job that never leaves `RUNNING`
/// fails this test rather than hanging it.
const MAX_POLLS: usize = 40;

/// Comfortably past a due instant already reached, for the cases that read a finished job.
const PAST_THE_DUE_INSTANT: Duration = DEVICE_DISCOVERY_DURATION.saturating_add(
    Duration::from_millis(200),
);

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    fn seconds(&self, name: &str) -> Option<f64> {
        self.member(name).and_then(Value::as_f64)
    }

    /// The `Items` of a list answer.
    fn items(&self) -> Vec<Value> {
        self.member("Items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `Id` of every item a list reported.
    fn item_ids(&self) -> Vec<String> {
        self.items()
            .iter()
            .map(|item| {
                item.get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request, signed for [`REGION`], and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{REGION}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply { status, body }
}

/// `StartDeviceDiscovery` with `body`, asserting the modelled 201 and answering the
/// `DeviceDiscoveryId` it minted.
async fn start(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let started = call(router, "POST", "/device-discoveries", Some(body)).await;
    assert_eq!(
        started.status, 201,
        "case {case:?}: expected HTTP 201 from StartDeviceDiscovery, got {} with body {:?}",
        started.status, started.body
    );
    // A start reports the job's id and start time only: the run outlives the call, so there
    // is no status and no finish time to report yet.
    assert!(
        started.member("Status").is_none() && started.member("FinishedAt").is_none(),
        "case {case:?}: expected StartDeviceDiscovery to report neither a Status nor a \
         FinishedAt, got body {:?}",
        started.body
    );
    started
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected StartDeviceDiscovery to answer an Id, got body {:?}",
                started.body
            )
        })
        .to_string()
}

/// `GetDeviceDiscovery` for `id`, asserting the modelled 200.
async fn get(router: &MultiServiceRouter, case: &str, id: &str) -> Reply {
    let read = call(router, "GET", &format!("/device-discoveries/{id}"), None).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected HTTP 200 from GetDeviceDiscovery, got {} with body {:?}",
        read.status, read.body
    );
    read
}

/// `ListDeviceDiscoveries` narrowed to one `DeviceDiscoveryStatus`, asserting the modelled
/// 200.
async fn list_with_status(router: &MultiServiceRouter, case: &str, status: &str) -> Reply {
    let listed = call(
        router,
        "GET",
        &format!("/device-discoveries?StatusFilter={status}"),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected HTTP 200 from ListDeviceDiscoveries, got {} with body {:?}",
        listed.status, listed.body
    );
    listed
}

/// Create a managed thing to hang the discovered devices off, and answer its generated id.
async fn controller(router: &MultiServiceRouter, case: &str) -> String {
    let created = call(
        router,
        "POST",
        "/managed-things",
        Some(json!({
            "Role": "CONTROLLER",
            "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
            "AuthenticationMaterial": "zwave:qr:0123456789",
            "MetaData": {"ParentControllerId": "none"},
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: seeding a controller expected HTTP 201, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected CreateManagedThing to answer an Id, got body {:?}",
                created.body
            )
        })
        .to_string()
}

/// Create a connector destination and an account association against it, and answer the
/// `AccountAssociationId`: the surface a cloud-connected discovery searches.
async fn account_association(router: &MultiServiceRouter, case: &str) -> String {
    let connector = call(
        router,
        "POST",
        "/cloud-connectors",
        Some(json!({
            "Name": "partner cloud connector",
            "EndpointConfig": {"lambda": {"arn": "connector-handler"}},
        })),
    )
    .await;
    assert_eq!(
        connector.status, 201,
        "case {case:?}: seeding a cloud connector expected HTTP 201, got {} with body {:?}",
        connector.status, connector.body
    );
    let destination = call(
        router,
        "POST",
        "/connector-destinations",
        Some(json!({
            "CloudConnectorId": connector.string("Id").unwrap_or_default(),
            "AuthConfig": {},
            // Inside the `SecretsManagerArn` `@pattern` and the `SecretsManagerVersionId`
            // `@length` (32..64) the model declares.
            "SecretsManager": {
                "arn": format!(
                    "arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:destination-A1b2C3"
                ),
                "versionId": "a".repeat(32),
            },
        })),
    )
    .await;
    assert_eq!(
        destination.status, 201,
        "case {case:?}: seeding a connector destination expected HTTP 201, got {} with \
         body {:?}",
        destination.status, destination.body
    );
    let created = call(
        router,
        "POST",
        "/account-associations",
        Some(json!({"ConnectorDestinationId": destination.string("Id").unwrap_or_default()})),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateAccountAssociation, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("AccountAssociationId")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an AccountAssociationId, got body {:?}",
                created.body
            )
        })
        .to_string()
}

/// The surface member a discovery of `discovery_type` names, so one table can span every
/// `DiscoveryType` the model declares: the hub protocols and the controller rediscovery
/// name a controller, the cloud types name an account association.
fn surface(discovery_type: &str, controller: &str, association: &str) -> Value {
    match discovery_type {
        "CLOUD" | "CUSTOM" => json!({
            "DiscoveryType": discovery_type,
            "AccountAssociationId": association,
            "CustomProtocolDetail": {"Name": "custom"},
        }),
        _ => json!({
            "DiscoveryType": discovery_type,
            "ControllerIdentifier": controller,
        }),
    }
}

/// RULE: the running phase of a job outlives the call that started it by the span the clock
/// owes it, so a client polling `GetDeviceDiscovery` reads `RUNNING` several times before
/// the job reports a terminal status — and it never reports one before its due instant.
#[tokio::test]
async fn a_poll_loop_reads_running_several_times_before_the_job_finishes() {
    let router = emulator();
    let controller = controller(&router, "poll loop").await;

    // The due instant lies between these two: the handler stamped the job's start somewhere
    // inside the start call, so `before_start + span` is the earliest it can be and
    // `after_start + span` the latest.
    let before_start = Instant::now();
    let id = start(
        &router,
        "poll loop",
        surface("ZWAVE", &controller, "unused"),
    )
    .await;
    let after_start = Instant::now();

    let mut running_reads = 0usize;
    let mut finished_after: Option<Duration> = None;
    for poll in 0..MAX_POLLS {
        let case = format!("poll {poll}");
        let before_read = Instant::now();
        let read = get(&router, &case, &id).await;
        let after_read = Instant::now();
        match read.string("Status") {
            Some(DEVICE_DISCOVERY_STATUS_RUNNING) => {
                assert!(
                    before_read.saturating_duration_since(after_start)
                        < DEVICE_DISCOVERY_DURATION,
                    "case {case:?}: a job cannot still be RUNNING past its due instant; the \
                     read started {:?} after the job did, of a {DEVICE_DISCOVERY_DURATION:?} \
                     run",
                    before_read.saturating_duration_since(after_start)
                );
                assert!(
                    read.member("FinishedAt").is_none(),
                    "case {case:?}: a running job has not finished, got FinishedAt {:?}",
                    read.member("FinishedAt")
                );
                running_reads += 1;
            }
            Some(DEVICE_DISCOVERY_STATUS_SUCCEEDED) => {
                assert!(
                    after_read.duration_since(before_start) >= DEVICE_DISCOVERY_DURATION,
                    "case {case:?}: a job cannot finish before its due instant; it reported \
                     SUCCEEDED {:?} after the start call began, of a \
                     {DEVICE_DISCOVERY_DURATION:?} run",
                    after_read.duration_since(before_start)
                );
                assert!(
                    read.member("FinishedAt").is_some(),
                    "case {case:?}: a finished job reports when it finished, got body {:?}",
                    read.body
                );
                finished_after = Some(after_read.duration_since(before_start));
                break;
            }
            other => panic!(
                "case {case:?}: expected the modelled RUNNING or SUCCEEDED, got {other:?} in \
                 body {:?}",
                read.body
            ),
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    assert!(
        finished_after.is_some(),
        "case \"poll loop\": expected the job to finish within {MAX_POLLS} polls of \
         {POLL_INTERVAL:?}, it was still running after {:?}",
        before_start.elapsed()
    );
    assert!(
        running_reads >= MIN_RUNNING_READS,
        "case \"poll loop\": expected at least {MIN_RUNNING_READS} RUNNING reads from a loop \
         polling every {POLL_INTERVAL:?} across a {DEVICE_DISCOVERY_DURATION:?} run, got \
         {running_reads} (the job finished after {finished_after:?})"
    );
}

/// RULE: the span is owed to EACH job from its own start, and every reader derives the
/// status from the same clock — so a job started later is still running when an earlier one
/// has finished, and the `ListDeviceDiscoveries` `StatusFilter` reports exactly what
/// `GetDeviceDiscovery` reports for the same job.
#[tokio::test]
async fn each_job_leaves_running_at_its_own_due_instant() {
    let router = emulator();
    let controller = controller(&router, "two jobs").await;
    let association = account_association(&router, "two jobs").await;

    // The earlier job's run is over by the time the reads happen; the later job's is not.
    let earlier = start(
        &router,
        "earlier job",
        surface("ZIGBEE", &controller, &association),
    )
    .await;
    let earlier_after_start = Instant::now();
    tokio::time::sleep(DEVICE_DISCOVERY_DURATION * 7 / 10).await;
    let later_before_start = Instant::now();
    let later = start(
        &router,
        "later job",
        surface("CLOUD", &controller, &association),
    )
    .await;
    tokio::time::sleep(DEVICE_DISCOVERY_DURATION * 4 / 10).await;

    let before_reads = Instant::now();
    let earlier_read = get(&router, "earlier job", &earlier).await;
    let later_read = get(&router, "later job", &later).await;
    let running = list_with_status(&router, "StatusFilter RUNNING", DEVICE_DISCOVERY_STATUS_RUNNING)
        .await;
    let succeeded = list_with_status(
        &router,
        "StatusFilter SUCCEEDED",
        DEVICE_DISCOVERY_STATUS_SUCCEEDED,
    )
    .await;
    let after_reads = Instant::now();

    // The reads really did land in the window the cases describe — on a runner so slow that
    // they did not, the assertions below would say nothing.
    assert!(
        before_reads.duration_since(earlier_after_start) >= DEVICE_DISCOVERY_DURATION,
        "case \"earlier job\": the reads must land after its due instant, they started {:?} \
         after it was started, of a {DEVICE_DISCOVERY_DURATION:?} run",
        before_reads.duration_since(earlier_after_start)
    );
    assert!(
        after_reads.duration_since(later_before_start) < DEVICE_DISCOVERY_DURATION,
        "case \"later job\": the reads must land before its due instant, they ended {:?} \
         after it was started, of a {DEVICE_DISCOVERY_DURATION:?} run",
        after_reads.duration_since(later_before_start)
    );

    assert_eq!(
        earlier_read.string("Status"),
        Some(DEVICE_DISCOVERY_STATUS_SUCCEEDED),
        "case \"earlier job\": expected the job whose run is over to have succeeded, got {:?}",
        earlier_read.string("Status")
    );
    assert_eq!(
        later_read.string("Status"),
        Some(DEVICE_DISCOVERY_STATUS_RUNNING),
        "case \"later job\": expected the job started later to still be running, got {:?}",
        later_read.string("Status")
    );
    assert!(
        later_read.member("FinishedAt").is_none()
            && earlier_read.member("FinishedAt").is_some(),
        "case \"two jobs\": expected a FinishedAt on the finished job only, got {:?} for the \
         earlier job and {:?} for the later one",
        earlier_read.member("FinishedAt"),
        later_read.member("FinishedAt")
    );

    assert_eq!(
        running.item_ids(),
        vec![later.clone()],
        "case \"StatusFilter RUNNING\": expected only the job still running, got {:?}",
        running.items()
    );
    assert_eq!(
        succeeded.item_ids(),
        vec![earlier.clone()],
        "case \"StatusFilter SUCCEEDED\": expected only the job whose run is over, got {:?}",
        succeeded.items()
    );
    // The list's summary and the get report the same status for the same job, both derived
    // from the clock.
    for (case, listed, read) in [
        ("StatusFilter RUNNING", &running, &later_read),
        ("StatusFilter SUCCEEDED", &succeeded, &earlier_read),
    ] {
        assert_eq!(
            listed
                .items()
                .first()
                .and_then(|item| item.get("Status"))
                .and_then(Value::as_str),
            read.string("Status"),
            "case {case:?}: expected the listed status to match the get, got {:?} and {:?}",
            listed.items(),
            read.string("Status")
        );
    }
}

/// RULE: reading a job neither advances nor shifts it. Repeated reads of a running job keep
/// reporting `RUNNING` with no `FinishedAt`, and once the job is over every read reports the
/// same `FinishedAt` — the due instant one span after its own `StartedAt`, not the instant
/// it happened to be read. Holds for every `DiscoveryType` the model declares.
#[tokio::test]
async fn polling_neither_advances_nor_shifts_a_job() {
    let router = emulator();
    let controller = controller(&router, "repeated reads").await;
    let association = account_association(&router, "repeated reads").await;

    /// How many times each case reads the same job: more than one, so a status that moved
    /// because it was read would show up.
    const READS: usize = 3;

    let mut started: Vec<(&str, String, Instant)> = Vec::new();
    for discovery_type in DISCOVERY_TYPE_VALUES {
        let before_start = Instant::now();
        let id = start(
            &router,
            discovery_type,
            surface(discovery_type, &controller, &association),
        )
        .await;
        started.push((discovery_type, id, before_start));
    }

    // While the runs are not over, no number of reads moves the status on.
    for (discovery_type, id, before_start) in &started {
        for read_number in 0..READS {
            let case = format!("{discovery_type} read {read_number} while running");
            let read = get(&router, &case, id).await;
            assert!(
                before_start.elapsed() < DEVICE_DISCOVERY_DURATION,
                "case {case:?}: the read must land before the due instant, it happened {:?} \
                 after the job was started, of a {DEVICE_DISCOVERY_DURATION:?} run",
                before_start.elapsed()
            );
            assert_eq!(
                read.string("Status"),
                Some(DEVICE_DISCOVERY_STATUS_RUNNING),
                "case {case:?}: expected the job to still be running, got {:?}",
                read.string("Status")
            );
            assert!(
                read.member("FinishedAt").is_none(),
                "case {case:?}: a running job has not finished, got FinishedAt {:?}",
                read.member("FinishedAt")
            );
        }
    }

    // Past every job's due instant the reads agree with each other, and the finish time is
    // the due instant the start owed — one span after the job's own StartedAt.
    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    for (discovery_type, id, _) in &started {
        let mut finish_times: Vec<f64> = Vec::new();
        for read_number in 0..READS {
            let case = format!("{discovery_type} read {read_number} after the run");
            let read = get(&router, &case, id).await;
            assert_eq!(
                read.string("Status"),
                Some(DEVICE_DISCOVERY_STATUS_SUCCEEDED),
                "case {case:?}: expected the job to have succeeded, got {:?}",
                read.string("Status")
            );
            let started_at = read.seconds("StartedAt").unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a StartedAt timestamp, got body {:?}",
                    read.body
                )
            });
            let finished_at = read.seconds("FinishedAt").unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a FinishedAt timestamp, got body {:?}",
                    read.body
                )
            });
            let span = finished_at - started_at;
            assert!(
                (span - DEVICE_DISCOVERY_DURATION.as_secs_f64()).abs() < 0.001,
                "case {case:?}: expected FinishedAt one {DEVICE_DISCOVERY_DURATION:?} run \
                 after StartedAt, got {span} s between {started_at} and {finished_at}"
            );
            finish_times.push(finished_at);
        }
        assert!(
            finish_times.windows(2).all(|pair| pair[0] == pair[1]),
            "case {discovery_type:?}: expected every read to report the same FinishedAt, got \
             {finish_times:?}"
        );
    }
}
