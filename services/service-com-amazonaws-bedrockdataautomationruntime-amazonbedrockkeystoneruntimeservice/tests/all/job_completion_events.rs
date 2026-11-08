// SPDX-License-Identifier: CC0-1.0

//! A job whose submission asked for it PUBLISHES its lifecycle to EventBridge.
//!
//! The RULE asserted here: a submission carrying
//! `notificationConfiguration.eventBridgeConfiguration.eventBridgeEnabled` true has every
//! lifecycle transition of its job published onto the account's DEFAULT EventBridge bus
//! under the `source` `aws.bedrock` and the `detail-type` of that transition
//! (`Bedrock Data Automation Job Created`, then `Bedrock Data Automation Job Succeeded` or
//! `Bedrock Data Automation Job Failed With Client Error`), whose `detail` carries
//! `job_id` — the id segment of the job's own `invocationArn`. EventBridge then routes it,
//! so a rule whose `EventPattern` matches delivers it to that rule's targets. A submission
//! that asked for nothing publishes nothing at all: accepting the flag and emitting
//! nothing is the defect this pins, and so is emitting for a job nobody asked about.
//!
//! The rows span what the rule generalises over: whether the submission asked to be
//! notified, and WHICH terminal status the job reaches (a job whose asset the object store
//! holds is a `Succeeded`; one whose asset it does not is a `Failed With Client Error`) —
//! and the outcome event arrives WITHOUT the test ever polling
//! `GetDataAutomationStatus`, because a client driven by the event never does.
//!
//! EventBridge, SQS and Amazon S3 are mounted BESIDE this service on the SAME engine, so
//! the publication travels the real in-process callback, through the real bus, to a real
//! SQS target: nothing here reads the publisher's own queue to decide whether delivery
//! happened.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use service_com_amazonaws_s3_amazons3 as s3;
use smithy_server_runtime::MultiServiceRouter;

/// The `source` and the four `detail-type`s a data-automation job's events carry, spelled
/// out here rather than imported, so a wrong constant in the implementation cannot make a
/// test pass.
const EVENT_SOURCE: &str = "aws.bedrock";
const JOB_CREATED: &str = "Bedrock Data Automation Job Created";
const JOB_SUCCEEDED: &str = "Bedrock Data Automation Job Succeeded";
const JOB_FAILED_CLIENT: &str = "Bedrock Data Automation Job Failed With Client Error";
const JOB_FAILED_SERVICE: &str = "Bedrock Data Automation Job Failed With Service Error";

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` (`smithy_query` shape `DataAutomationProfileArn`).
const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// The project every submission names, arranged by [`crate::support::seed_project`].
const PROJECT_ARN: &str =
    "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/notified";

/// The bucket the staged asset lives in, and the key of the asset that IS there.
const ASSET_BUCKET: &str = "notified-assets";
const STAGED_KEY: &str = "input/asset.pdf";

/// A key the staging never creates, so the job pointed at it cannot read its asset.
const ABSENT_KEY: &str = "input/never-staged.pdf";

/// How long the suite waits for a best-effort delivery: comfortably past both emulated
/// lifecycle spans and the settle that follows them.
fn delivery_deadline() -> Duration {
    bda::state::CREATED_FOR + bda::state::IN_PROGRESS_FOR + Duration::from_secs(2)
}

/// One row: the case, whether the submission asks to be notified, the asset key it points
/// at, and the `detail-type` its outcome must carry (`None` when nothing may be
/// delivered).
struct Row {
    case: &'static str,
    notified: bool,
    key: &'static str,
    outcome_detail_type: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "a notified job whose asset the store holds",
        notified: true,
        key: STAGED_KEY,
        outcome_detail_type: Some(JOB_SUCCEEDED),
    },
    Row {
        case: "a notified job whose asset the store does not hold",
        notified: true,
        key: ABSENT_KEY,
        outcome_detail_type: Some(JOB_FAILED_CLIENT),
    },
    Row {
        case: "a job that asked for no notification",
        notified: false,
        key: STAGED_KEY,
        outcome_detail_type: None,
    },
];

/// One emulator: this service, EventBridge, SQS and Amazon S3 on one shared engine.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "notified-projects");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(s3::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Stage the asset the rows that must succeed point at.
fn stage_asset(engine: &Arc<engine::InMemoryEngine>) {
    let state = s3::state::service_state(&**engine).expect("the object store on this engine");
    let mut store = state.lock();
    assert_eq!(
        store.create_bucket(ASSET_BUCKET, REGION),
        s3::state::BucketCreation::Created,
        "the staging bucket {ASSET_BUCKET:?} must be created"
    );
    assert_eq!(
        store.put_object(
            ASSET_BUCKET,
            STAGED_KEY,
            s3::state::Object::stored(b"%PDF-1.7 emulator data automation asset".to_vec(), None),
        ),
        Some(s3::state::ObjectWrite::Created),
        "the asset {STAGED_KEY:?} must be staged"
    );
}

/// Call one operation of any mounted service the way an SDK would: awsJson, `POST /` with
/// `X-Amz-Target: <target>.<operation>` and a credential scoped to `signing_name`.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    json_version: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("{target}.{operation}"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One data-automation call.
async fn bda_call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    rpc(
        router,
        "bedrock",
        "AmazonBedrockKeystoneRuntimeService",
        "1.1",
        operation,
        body,
    )
    .await
}

/// One EventBridge call.
async fn events(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    rpc(router, "events", "AWSEvents", "1.1", operation, body).await
}

/// One SQS call.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// An SQS queue called `name`; answers its url and the ARN a rule names it by.
async fn given_queue(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, body) = sqs(router, "CreateQueue", json!({ "QueueName": name })).await;
    assert_eq!(status, 200, "CreateQueue answered {status} {body}");
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        router,
        "GetQueueAttributes",
        json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(status, 200, "GetQueueAttributes answered {status} {body}");
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueArn in {body}"))
        .to_string();
    (url, arn)
}

/// Grant `events.amazonaws.com` the `sqs:SendMessage` a rule's target needs on the queue
/// at `url`, conditioned on the rule's own ARN — EventBridge makes that call as its own
/// service principal and relies on the queue's resource-based policy.
async fn grant_delivery(router: &MultiServiceRouter, url: &str, queue_arn: &str, rule_arn: &str) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": rule_arn } },
        }],
    })
    .to_string();
    let (status, body) = sqs(
        router,
        "SetQueueAttributes",
        json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(status, 200, "SetQueueAttributes answered {status} {body}");
}

/// Put the rule `name` with `pattern` and point it at `queue_arn`; answers the rule's ARN.
async fn given_rule(
    router: &MultiServiceRouter,
    name: &str,
    pattern: Value,
    queue_arn: &str,
) -> String {
    let (status, body) = events(
        router,
        "PutRule",
        json!({ "Name": name, "EventPattern": pattern.to_string(), "State": "ENABLED" }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");
    let rule_arn = body["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no RuleArn in {body}"))
        .to_string();
    let (status, body) = events(
        router,
        "PutTargets",
        json!({ "Rule": name, "Targets": [{ "Id": "sqs-target", "Arn": queue_arn }] }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutTargets reported a failed entry: {body}"
    );
    rule_arn
}

/// Every message waiting on the queue at `url`, as parsed event envelopes.
async fn drain(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let (status, body) = sqs(
        router,
        "ReceiveMessage",
        json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(status, 200, "ReceiveMessage answered {status} {body}");
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            serde_json::from_str::<Value>(message["Body"].as_str().unwrap_or_default())
                .unwrap_or(Value::Null)
        })
        .collect()
}

/// The id segment of an `invocationArn` — the value the event's `detail.job_id` carries.
fn job_id(invocation_arn: &str) -> &str {
    crate::support::invocation_id(invocation_arn)
}

#[tokio::test]
async fn a_notified_jobs_lifecycle_is_delivered_through_the_default_bus() {
    let (router, engine) = emulator();
    stage_asset(&engine);

    // One queue behind a rule matching every data-automation job event, and one behind a
    // rule that cannot match anything this service publishes.
    let (matching_url, matching_arn) = given_queue(&router, "data-automation-events").await;
    let (other_url, other_arn) = given_queue(&router, "unrelated-events").await;
    let matching_rule = given_rule(
        &router,
        "data-automation-job-events",
        json!({
            "source": [EVENT_SOURCE],
            "detail-type": [JOB_CREATED, JOB_SUCCEEDED, JOB_FAILED_CLIENT, JOB_FAILED_SERVICE],
        }),
        &matching_arn,
    )
    .await;
    let other_rule = given_rule(
        &router,
        "another-services-events",
        json!({ "source": ["aws.ecs"], "detail-type": ["ECS Task State Change"] }),
        &other_arn,
    )
    .await;
    grant_delivery(&router, &matching_url, &matching_arn, &matching_rule).await;
    grant_delivery(&router, &other_url, &other_arn, &other_rule).await;

    // Submit every row's job, then wait once: no row polls the job's status, so every
    // delivery below is one the publication made on the job's own account.
    let mut submitted: Vec<(&Row, String)> = Vec::new();
    for row in ROWS {
        let mut body = json!({
            "inputConfiguration": {"s3Uri": format!("s3://{ASSET_BUCKET}/{}", row.key)},
            "outputConfiguration": {"s3Uri": "s3://out-bucket/notified/"},
            "dataAutomationProfileArn": PROFILE_ARN,
            "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
        });
        if row.notified {
            body["notificationConfiguration"] =
                json!({"eventBridgeConfiguration": {"eventBridgeEnabled": true}});
        } else {
            body["notificationConfiguration"] =
                json!({"eventBridgeConfiguration": {"eventBridgeEnabled": false}});
        }
        let (status, answered) = bda_call(&router, "InvokeDataAutomationAsync", body).await;
        assert_eq!(
            status, 200,
            "case {:?}: InvokeDataAutomationAsync answered {status} {answered}",
            row.case
        );
        let arn = answered["invocationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no invocationArn in {answered}", row.case))
            .to_string();
        submitted.push((row, arn));
    }

    // Poll the TARGET QUEUE, the way a client driven by the event does, until every
    // expected event has arrived or the bounded wait runs out.
    let mut delivered: Vec<Value> = Vec::new();
    let deadline = std::time::Instant::now() + delivery_deadline();
    let expected = ROWS
        .iter()
        .filter(|row| row.notified)
        .map(|row| 1 + usize::from(row.outcome_detail_type.is_some()))
        .sum::<usize>();
    while std::time::Instant::now() < deadline {
        delivered.extend(drain(&router, &matching_url).await);
        if delivered.len() >= expected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    for (row, arn) in &submitted {
        let case = row.case;
        let id = job_id(arn);
        let mine: Vec<&Value> = delivered
            .iter()
            .filter(|event| event["detail"]["job_id"] == json!(id))
            .collect();

        if !row.notified {
            assert!(
                mine.is_empty(),
                "case {case:?}: a job that asked for no notification must publish nothing, \
                 got {mine:?}"
            );
            continue;
        }

        let types: Vec<&str> = mine
            .iter()
            .map(|event| event["detail-type"].as_str().unwrap_or_default())
            .collect();
        assert!(
            types.contains(&JOB_CREATED),
            "case {case:?}: expected a {JOB_CREATED:?} event for job {id}, got {types:?}"
        );
        let want_outcome = row
            .outcome_detail_type
            .expect("a notified row states its outcome");
        assert!(
            types.contains(&want_outcome),
            "case {case:?}: expected a {want_outcome:?} event for job {id}, got {types:?}"
        );
        // Every event about this job names this service as its source and the job as the
        // resource it is about.
        for event in &mine {
            assert_eq!(
                event["source"],
                json!(EVENT_SOURCE),
                "case {case:?}: expected source {EVENT_SOURCE}, got {event}"
            );
            assert_eq!(
                event["resources"],
                json!([arn]),
                "case {case:?}: the event must name the job's own ARN, got {event}"
            );
        }
        // The outcome is published once, not on every drain: the transition is queued by
        // the one call that materialised it.
        assert_eq!(
            types.iter().filter(|kind| **kind == want_outcome).count(),
            1,
            "case {case:?}: expected exactly one {want_outcome:?} event, got {types:?}"
        );
    }

    // The rule that cannot match this service's events received nothing.
    let other = drain(&router, &other_url).await;
    assert!(
        other.is_empty(),
        "a rule for another service's events must receive none of these, got {other:?}"
    );

    // And nothing is left waiting: every queued event was published by the call that
    // produced it, and every settle a submission started gave its slot back.
    let store = bda::state::service_state(&*engine).expect("this service's store on this engine");
    let queued = store.lock().pending_job_event_count();
    assert_eq!(
        queued, 0,
        "every queued event must have been published, got {queued} waiting"
    );
    let in_flight = store.lock().settles_in_flight();
    assert_eq!(
        in_flight, 0,
        "every settle must have released its slot, got {in_flight} in flight"
    );
}

#[test]
fn the_settles_a_stream_of_notified_submissions_starts_are_bounded() {
    use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState as Store;

    // Each notified submission starts one settle on its job's behalf, so the number in
    // flight is what a stream of them could grow: the cap is taken before the work starts
    // and given back however it ends.
    let cap = bda::state::job_events::MAX_SETTLES_IN_FLIGHT;
    let mut store = Store::default();

    for i in 0..cap {
        assert!(
            store.try_begin_settle(),
            "settle {i}: a slot must be free below the cap, with {} in flight",
            store.settles_in_flight()
        );
    }
    assert_eq!(
        store.settles_in_flight(),
        cap,
        "the cap must be reached exactly"
    );
    // Past the cap no further settle starts, however many submissions arrive.
    for i in 0..3 {
        assert!(
            !store.try_begin_settle(),
            "submission {i} past the cap: no slot may be handed out, got {} in flight",
            store.settles_in_flight()
        );
    }
    // A finished settle frees exactly one slot, and the counter never goes below zero
    // however many finishes arrive.
    store.finish_settle();
    assert!(
        store.try_begin_settle(),
        "a freed slot must be reusable, got {} in flight",
        store.settles_in_flight()
    );
    for _ in 0..(cap + 5) {
        store.finish_settle();
    }
    assert_eq!(
        store.settles_in_flight(),
        0,
        "the count of settles in flight must not go below zero"
    );
}

#[tokio::test]
async fn a_notified_job_reports_the_same_outcome_to_a_poller_and_to_the_bus() {
    // The event reports the job the control plane reports: the settle the submission
    // started and a status read are the same one path, so a caller that polls after the
    // event sees exactly the status the event's `detail-type` named.
    let (router, engine) = emulator();
    stage_asset(&engine);
    let case = "a notified job polled after its event";

    let (url, queue_arn) = given_queue(&router, "outcome-events").await;
    let rule = given_rule(
        &router,
        "outcome-rule",
        json!({ "source": [EVENT_SOURCE], "detail-type": [JOB_SUCCEEDED] }),
        &queue_arn,
    )
    .await;
    grant_delivery(&router, &url, &queue_arn, &rule).await;

    let (status, answered) = bda_call(
        &router,
        "InvokeDataAutomationAsync",
        json!({
            "inputConfiguration": {"s3Uri": format!("s3://{ASSET_BUCKET}/{STAGED_KEY}")},
            "outputConfiguration": {"s3Uri": "s3://out-bucket/outcome/"},
            "dataAutomationProfileArn": PROFILE_ARN,
            "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
            "notificationConfiguration": {"eventBridgeConfiguration": {"eventBridgeEnabled": true}},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: InvokeDataAutomationAsync answered {status} {answered}"
    );
    let arn = answered["invocationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no invocationArn in {answered}"))
        .to_string();

    let mut succeeded: Vec<Value> = Vec::new();
    let deadline = std::time::Instant::now() + delivery_deadline();
    while std::time::Instant::now() < deadline {
        succeeded.extend(drain(&router, &url).await);
        if !succeeded.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        succeeded.len(),
        1,
        "case {case:?}: expected one {JOB_SUCCEEDED:?} event for job {}, got {succeeded:?}",
        job_id(&arn)
    );
    assert_eq!(
        succeeded[0]["detail"]["job_id"],
        json!(job_id(&arn)),
        "case {case:?}: the event must carry the job's own id, got {}",
        succeeded[0]
    );

    let (status, read) = bda_call(
        &router,
        "GetDataAutomationStatus",
        json!({"invocationArn": arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetDataAutomationStatus answered {status} {read}"
    );
    assert_eq!(
        read["status"].as_str(),
        Some("Success"),
        "case {case:?}: the poller must see the status the event reported, got {read}"
    );
    assert!(
        !read["jobCompletionTime"].is_null(),
        "case {case:?}: the settled job must report a completion time, got {read}"
    );
}
