// SPDX-License-Identifier: CC0-1.0

//! WHERE a notified job's lifecycle events are published, and WHEN none are.
//!
//! The RULE asserted here: a job whose submission asked to be notified
//! (`notificationConfiguration.eventBridgeConfiguration.eventBridgeEnabled` true) has its
//! lifecycle published onto the default event bus OF THE REGION THE JOB LIVES IN — the
//! Region its own `invocationArn` names, which is the Region the submission was signed for
//! — and nowhere else, so a rule on another Region's default bus sees nothing of it. A
//! submission that did not ask to be notified publishes nothing in any Region, whether it
//! spelled the member out with `eventBridgeEnabled` false or carried no
//! `notificationConfiguration` at all.
//!
//! This is the half of the notification rule that `job_completion_events` does not span:
//! that suite pins WHAT is published for one Region's job (the `source`, the per-transition
//! `detail-type`, `detail.job_id`, and that an unnotified job publishes nothing), and the
//! rows here vary the REGION the job is submitted in and the FORM the opt-in member takes.
//! An event bus is a regional resource, so an implementation that published every job's
//! events in one fixed Region would deliver this suite's events to the wrong Region's rule
//! — which is exactly what the per-Region queues below would show.
//!
//! EventBridge, SQS and Amazon S3 are mounted BESIDE this service on the SAME engine, so
//! each Region's publication travels the real in-process callback, through that Region's
//! own bus, to a real SQS target in that Region.

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

/// The key of the asset every row's submission points at, staged in its own Region's
/// bucket so every row's job has an asset to read where it runs.
const STAGED_KEY: &str = "input/asset.pdf";

/// How a row's submission spells the member the opt-in is read off.
///
/// The three forms a client can send: the member absent altogether (it is optional —
/// `smithy_query` operation `InvokeDataAutomationAsync`, member `notificationConfiguration`),
/// and the member present with `eventBridgeEnabled` false or true (`EventBridgeConfiguration`
/// requires that Boolean, so a present member always states one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptIn {
    /// No `notificationConfiguration` member at all.
    Absent,
    /// The member present, asking for no delivery.
    Disabled,
    /// The member present, asking to be notified.
    Enabled,
}

/// One row: the case, the Region the submission is signed for, the form its opt-in takes,
/// and the `detail-type`s its job's own Region must deliver (empty when nothing may be).
struct Row {
    case: &'static str,
    region: &'static str,
    opt_in: OptIn,
    delivered: &'static [&'static str],
}

/// The rows span the two things this suite generalises over: the Region the job lives in
/// (three Regions of three different Region groups, so a fixed Region in the
/// implementation fails a row) and the form the opt-in member takes.
const ROWS: &[Row] = &[
    Row {
        case: "a notified job submitted in us-east-1",
        region: "us-east-1",
        opt_in: OptIn::Enabled,
        delivered: &[JOB_CREATED, JOB_SUCCEEDED],
    },
    Row {
        case: "a notified job submitted in eu-west-1",
        region: "eu-west-1",
        opt_in: OptIn::Enabled,
        delivered: &[JOB_CREATED, JOB_SUCCEEDED],
    },
    Row {
        case: "a notified job submitted in ap-southeast-2",
        region: "ap-southeast-2",
        opt_in: OptIn::Enabled,
        delivered: &[JOB_CREATED, JOB_SUCCEEDED],
    },
    Row {
        case: "a submission carrying no notificationConfiguration at all",
        region: "us-east-1",
        opt_in: OptIn::Absent,
        delivered: &[],
    },
    Row {
        case: "a submission whose eventBridgeConfiguration asks for no delivery",
        region: "eu-west-1",
        opt_in: OptIn::Disabled,
        delivered: &[],
    },
];

/// The Regions the rows use, each once, in the order they first appear.
fn regions() -> Vec<&'static str> {
    let mut regions: Vec<&'static str> = Vec::new();
    for row in ROWS {
        if !regions.contains(&row.region) {
            regions.push(row.region);
        }
    }
    regions
}

/// The bucket a Region's asset is staged in.
fn asset_bucket(region: &str) -> String {
    format!("da-assets-{region}")
}

/// The project ARN a Region's submissions name, arranged by [`crate::support::seed_project`].
fn project_arn(region: &str) -> String {
    format!(
        "arn:aws:bedrock:{region}:{}:data-automation-project/notified-in-{region}",
        emulator_server::emulator_account_id()
    )
}

/// A legal `dataAutomationProfileArn` for `region` (`smithy_query` shape
/// `DataAutomationProfileArn`): AWS's own account publishes the profile, so it is one every
/// account may use.
fn profile_arn(region: &str) -> String {
    format!("arn:aws:bedrock:{region}:aws:data-automation-profile/{region}.data-automation-v1")
}

/// How long the suite waits for a best-effort delivery: comfortably past both emulated
/// lifecycle spans and the settle that follows them.
fn delivery_deadline() -> Duration {
    bda::state::CREATED_FOR + bda::state::IN_PROGRESS_FOR + Duration::from_secs(3)
}

/// One emulator: this service, EventBridge, SQS and Amazon S3 on one shared engine.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    for region in regions() {
        crate::support::seed_project(&engine, &project_arn(region), &format!("notified-{region}"));
    }
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

/// Stage the asset every Region's submissions point at, in a bucket of that Region.
fn stage_assets(engine: &Arc<engine::InMemoryEngine>) {
    let state = s3::state::service_state(&**engine).expect("the object store on this engine");
    let mut store = state.lock();
    for region in regions() {
        let bucket = asset_bucket(region);
        assert_eq!(
            store.create_bucket(&bucket, region),
            s3::state::BucketCreation::Created,
            "the staging bucket {bucket:?} must be created"
        );
        assert_eq!(
            store.put_object(
                &bucket,
                STAGED_KEY,
                s3::state::Object::stored(
                    b"%PDF-1.7 emulator data automation asset".to_vec(),
                    None
                ),
            ),
            Some(s3::state::ObjectWrite::Created),
            "the asset {STAGED_KEY:?} must be staged in {bucket:?}"
        );
    }
}

/// Call one operation of any mounted service the way an SDK would: awsJson, `POST /` with
/// `X-Amz-Target: <target>.<operation>` and a credential scoped to `signing_name` in
/// `region`.
async fn rpc(
    router: &MultiServiceRouter,
    region: &str,
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
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
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

/// One data-automation call, signed for `region`.
async fn bda_call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    rpc(
        router,
        region,
        "bedrock",
        "AmazonBedrockKeystoneRuntimeService",
        "1.1",
        operation,
        body,
    )
    .await
}

/// One EventBridge call, signed for `region` — which is the Region whose default bus it
/// reaches, because an event bus is a regional resource.
async fn events(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    rpc(router, region, "events", "AWSEvents", "1.1", operation, body).await
}

/// One SQS call, signed for `region`.
async fn sqs(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    rpc(router, region, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// An SQS queue called `name` in `region`; answers its url and the ARN a rule names it by.
async fn given_queue(router: &MultiServiceRouter, region: &str, name: &str) -> (String, String) {
    let (status, body) = sqs(router, region, "CreateQueue", json!({ "QueueName": name })).await;
    assert_eq!(status, 200, "CreateQueue answered {status} {body}");
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        router,
        region,
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
async fn grant_delivery(
    router: &MultiServiceRouter,
    region: &str,
    url: &str,
    queue_arn: &str,
    rule_arn: &str,
) {
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
        region,
        "SetQueueAttributes",
        json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(status, 200, "SetQueueAttributes answered {status} {body}");
}

/// A rule on `region`'s default bus matching every data-automation job event, pointed at a
/// queue of that Region; answers the queue's url.
async fn given_job_event_rule(router: &MultiServiceRouter, region: &str) -> String {
    let (url, queue_arn) = given_queue(router, region, &format!("da-job-events-{region}")).await;
    let name = format!("da-job-events-{region}");
    let pattern = json!({
        "source": [EVENT_SOURCE],
        "detail-type": [JOB_CREATED, JOB_SUCCEEDED, JOB_FAILED_CLIENT, JOB_FAILED_SERVICE],
    })
    .to_string();
    let (status, body) = events(
        router,
        region,
        "PutRule",
        json!({ "Name": name, "EventPattern": pattern, "State": "ENABLED" }),
    )
    .await;
    assert_eq!(status, 200, "PutRule in {region} answered {status} {body}");
    let rule_arn = body["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no RuleArn in {body}"))
        .to_string();
    let (status, body) = events(
        router,
        region,
        "PutTargets",
        json!({ "Rule": name, "Targets": [{ "Id": "sqs-target", "Arn": queue_arn }] }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets in {region} answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutTargets in {region} reported a failed entry: {body}"
    );
    grant_delivery(router, region, &url, &queue_arn, &rule_arn).await;
    url
}

/// Every message waiting on the queue at `url`, as parsed event envelopes.
async fn drain(router: &MultiServiceRouter, region: &str, url: &str) -> Vec<Value> {
    let (status, body) = sqs(
        router,
        region,
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

/// The `region` and `account` fields of an ARN: `arn:<partition>:<service>:<region>:<account>:…`.
fn arn_region_and_account(arn: &str) -> (&str, &str) {
    let mut fields = arn.split(':').skip(3);
    let region = fields.next().unwrap_or_default();
    let account = fields.next().unwrap_or_default();
    (region, account)
}

/// The body one row submits.
fn submission(row: &Row) -> Value {
    let mut body = json!({
        "inputConfiguration": {
            "s3Uri": format!("s3://{}/{STAGED_KEY}", asset_bucket(row.region)),
        },
        "outputConfiguration": {"s3Uri": format!("s3://da-out-{}/notified/", row.region)},
        "dataAutomationProfileArn": profile_arn(row.region),
        "dataAutomationConfiguration": {"dataAutomationProjectArn": project_arn(row.region)},
    });
    match row.opt_in {
        OptIn::Absent => {}
        OptIn::Disabled => {
            body["notificationConfiguration"] =
                json!({"eventBridgeConfiguration": {"eventBridgeEnabled": false}});
        }
        OptIn::Enabled => {
            body["notificationConfiguration"] =
                json!({"eventBridgeConfiguration": {"eventBridgeEnabled": true}});
        }
    }
    body
}

#[tokio::test]
async fn a_notified_jobs_events_are_published_only_on_its_own_regions_bus() {
    let (router, engine) = emulator();
    stage_assets(&engine);

    // One rule, queue and policy per Region: each Region's default bus is a resource of
    // its own, so each has to be arranged separately — and a job's events may reach only
    // the rule of the Region the job lives in.
    let mut queues: Vec<(&'static str, String)> = Vec::new();
    for region in regions() {
        queues.push((region, given_job_event_rule(&router, region).await));
    }

    // Submit every row's job, then wait once: no row polls the job's status, so every
    // delivery below is one the publication made on the job's own account.
    let mut submitted: Vec<(&Row, String)> = Vec::new();
    for row in ROWS {
        let (status, answered) =
            bda_call(&router, row.region, "InvokeDataAutomationAsync", submission(row)).await;
        assert_eq!(
            status, 200,
            "case {:?}: InvokeDataAutomationAsync answered {status} {answered}",
            row.case
        );
        let arn = answered["invocationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no invocationArn in {answered}", row.case))
            .to_string();
        assert_eq!(
            arn_region_and_account(&arn).0,
            row.region,
            "case {:?}: the job's ARN must name the Region it was submitted in, got {arn}",
            row.case
        );
        submitted.push((row, arn));
    }

    // Poll each Region's TARGET QUEUE, the way a client driven by the event does, until
    // every expected event has arrived or the bounded wait runs out.
    let mut delivered: Vec<(&'static str, Value)> = Vec::new();
    let expected: usize = ROWS.iter().map(|row| row.delivered.len()).sum();
    let deadline = std::time::Instant::now() + delivery_deadline();
    while std::time::Instant::now() < deadline {
        for (region, url) in &queues {
            for event in drain(&router, region, url).await {
                delivered.push((region, event));
            }
        }
        if delivered.len() >= expected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    for (row, arn) in &submitted {
        let case = row.case;
        let id = crate::support::invocation_id(arn);
        let (job_region, job_account) = arn_region_and_account(arn);
        let mine: Vec<&(&'static str, Value)> = delivered
            .iter()
            .filter(|(_, event)| event["detail"]["job_id"] == json!(id))
            .collect();

        if row.delivered.is_empty() {
            assert!(
                mine.is_empty(),
                "case {case:?}: a job that asked for no notification must publish nothing in \
                 any Region, got {mine:?}"
            );
            continue;
        }

        // Every event about this job was delivered by the rule of the job's OWN Region,
        // and reports that Region and the job's account as the scope it happened in.
        for (delivered_in, event) in &mine {
            assert_eq!(
                *delivered_in, job_region,
                "case {case:?}: the job's event reached the rule of {delivered_in}, but the \
                 job lives in {job_region}: {event}"
            );
            assert_eq!(
                event["region"],
                json!(job_region),
                "case {case:?}: expected the event to report Region {job_region}, got {event}"
            );
            assert_eq!(
                event["account"],
                json!(job_account),
                "case {case:?}: expected the event to report account {job_account}, got {event}"
            );
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

        let types: Vec<&str> = mine
            .iter()
            .map(|(_, event)| event["detail-type"].as_str().unwrap_or_default())
            .collect();
        for want in row.delivered {
            assert_eq!(
                types.iter().filter(|kind| *kind == want).count(),
                1,
                "case {case:?}: expected exactly one {want:?} event for job {id} in \
                 {job_region}, got {types:?}"
            );
        }
        assert_eq!(
            types.len(),
            row.delivered.len(),
            "case {case:?}: expected exactly {} events for job {id}, got {types:?}",
            row.delivered.len()
        );
    }

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
