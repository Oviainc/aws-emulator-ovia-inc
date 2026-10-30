// SPDX-License-Identifier: CC0-1.0

//! A submission this service's own state refuses costs no read of anybody's objects.
//!
//! The rule every row below is one case of: a refusal `CreateJob` can decide from the
//! request together with this service's own stored state is answered BEFORE the operation
//! calls another emulated service, so a submission that is refused locally issues no input
//! read at all — and a submission that passes every local refusal reads exactly the inputs
//! it states that the probe can address.
//!
//! What makes the reads observable: the input probe is the only outbound work `CreateJob`
//! does, and it reads the job's `Settings.Inputs[].FileInput` S3 locations over the
//! in-process callback — which is routed by the very router this harness builds. So this
//! suite mounts an OBSERVER beside MediaConvert: a service that claims nothing and only
//! records each S3-scoped request the router offers it. The addresses it records are the
//! object reads the submission really cost, and an unclaimed request is answered exactly
//! as it is in an emulator carrying MediaConvert alone, so nothing about the probe's
//! verdict changes (that half is pinned by `input_probe.rs`).
//!
//! Which refusals are local comes from the model: `Queue`, `JobTemplate` and
//! `HopDestinations[].Queue` are references to this service's own resources, `Settings` is
//! the graph whose outputs name this account's presets, and the account's input policy is
//! this service's own state
//! (`smithy_query(query="operation",operation="CreateJob",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`).

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::EmulatedService;
use smithy_server_runtime::{ErasedServiceDefinition, Service};

use super::job_lifecycle::{arn, Emulator, Reply, ROLE};
use super::policy_enforcement::stored_jobs;

/// The credential scope of a request addressed to S3, which is what the probe's client
/// signs with: the observer counts these and nothing else, so MediaConvert's own requests
/// are never mistaken for object reads.
const S3_SCOPE: &str = "/s3/aws4_request";

/// The scheme an S3 object address is written in, both in a job's `Settings.Inputs[]` and
/// in what the observer reports.
const S3_SCHEME: &str = "s3://";

/// Two S3 inputs, so a submission refused by local state would cost TWO reads if the
/// refusal were decided after the probe rather than before it.
const INPUT_A: &str = "s3://probe-bucket/in/a.mp4";
const INPUT_B: &str = "s3://probe-bucket/in/b.mov";

/// An input that is not an S3 location, which the probe cannot address and therefore never
/// reads (`Input$FileInput` admits `https://…` as well as `s3://…`).
const INPUT_HTTPS: &str = "https://example.com/in/c.mp4";

/// The object addresses the router was asked to read, oldest first. Shared with the
/// observer the router holds.
#[derive(Clone, Default, Debug)]
struct Reads {
    addresses: Arc<Mutex<Vec<String>>>,
}

impl Reads {
    /// The addresses read since the last call, oldest first, CLEARING the record — so each
    /// row reports only its own reads.
    fn take(&self) -> Vec<String> {
        let mut addresses = self.addresses.lock().expect("the recorded reads");
        std::mem::take(&mut *addresses)
    }

    fn record(&self, address: String) {
        self.addresses
            .lock()
            .expect("the recorded reads")
            .push(address);
    }
}

/// A service that serves NOTHING and only records the S3-addressed requests the router
/// offers it, so a case can see which object reads a `CreateJob` really issued.
struct ReadObserver {
    /// The model-derived half of a `Service` is not an author seam, so the observer
    /// borrows this service's own — it is never used for dispatch, because the observer
    /// claims no request.
    inner: EmulatedService<engine::InMemoryEngine>,
    reads: Reads,
}

impl ReadObserver {
    fn new(engine: Arc<engine::InMemoryEngine>, reads: Reads) -> Self {
        Self {
            inner: EmulatedService::new(engine),
            reads,
        }
    }
}

impl Service for ReadObserver {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        self.inner.definition()
    }

    /// Recording here is what makes the reads observable: STEP 1 of
    /// `MultiServiceRouter::dispatch` filters EVERY registered service by `can_handle`, so
    /// this is called once per request the router dispatches, including each one a handler
    /// sends over the in-process callback.
    fn can_handle(&self, parts: &::http::request::Parts) -> bool {
        let addressed_s3 = parts
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains(S3_SCOPE));
        if addressed_s3 {
            // The probe addresses S3 path-style, so the request's path is
            // `/<bucket>/<key>` — the object address the job's input named.
            self.reads.record(format!(
                "{S3_SCHEME}{}",
                parts.uri.path().trim_start_matches('/')
            ));
        }
        // The observer claims nothing: an S3 request is left unclaimed exactly as it is in
        // an emulator that carries MediaConvert alone.
        false
    }
}

/// One output group holding one output, whose encoder settings come from `encoder`.
fn output_group(encoder: Value) -> Value {
    let mut output = json!({ "nameModifier": "_out" });
    if let (Some(output), Some(members)) = (output.as_object_mut(), encoder.as_object()) {
        for (member, value) in members {
            output.insert(member.clone(), value.clone());
        }
    }
    json!([{
        "name": "File Group",
        "outputs": [output],
        "outputGroupSettings": {
            "type": "FILE_GROUP_SETTINGS",
            "fileGroupSettings": { "destination": "s3://probe-bucket/out/" }
        }
    }])
}

/// A `Settings` graph reading `inputs` and writing `output_groups`.
fn settings(inputs: &[&str], output_groups: Value) -> Value {
    let inputs: Vec<Value> = inputs
        .iter()
        .map(|uri| json!({ "fileInput": uri }))
        .collect();
    json!({ "inputs": inputs, "outputGroups": output_groups })
}

/// An output group whose output states the container it is written in — the one shape that
/// describes something this service can write without naming a stored preset.
fn writable_output_group() -> Value {
    output_group(json!({ "containerSettings": { "container": "MP4" } }))
}

/// One row: a submission, what the service must answer it with, and which object addresses
/// it is allowed to have read while deciding that.
struct Row {
    case: &'static str,
    /// The members merged into a request that already carries `Role`.
    request: Value,
    /// The account input policy in force for this row, or `None` for an account with none.
    policy: Option<Value>,
    status: u16,
    /// The modelled error, or `""` for an accepted submission.
    error_type: &'static str,
    /// The object addresses the submission must cost a read of, in probe order.
    reads: Vec<&'static str>,
}

/// The table: an accepted submission first, so a row whose expectation is an EMPTY read
/// list is never the first thing asserted and an observer that saw nothing fails instead of
/// passing; then every class of locally decidable refusal, each stating two S3 inputs so a
/// read it costs would be visible; then the rest of the accepted submissions, which must
/// really read the inputs they state.
fn rows() -> Vec<Row> {
    let both = [INPUT_A, INPUT_B];
    vec![
        Row {
            case: "a submission local state admits, stating one S3 input",
            request: json!({ "settings": settings(&[INPUT_A], writable_output_group()) }),
            policy: None,
            status: 201,
            error_type: "",
            reads: vec![INPUT_A],
        },
        Row {
            case: "a graph with no inputs",
            request: json!({ "settings": settings(&[], writable_output_group()) }),
            policy: None,
            status: 400,
            error_type: "BadRequestException",
            reads: vec![],
        },
        Row {
            case: "a graph with an empty output-group list",
            request: json!({ "settings": settings(&both, json!([])) }),
            policy: None,
            status: 400,
            error_type: "BadRequestException",
            reads: vec![],
        },
        Row {
            case: "an output group with no outputs",
            request: json!({
                "settings": settings(&both, json!([{
                    "name": "File Group",
                    "outputs": [],
                    "outputGroupSettings": {
                        "type": "FILE_GROUP_SETTINGS",
                        "fileGroupSettings": { "destination": "s3://probe-bucket/out/" }
                    }
                }]))
            }),
            policy: None,
            status: 400,
            error_type: "BadRequestException",
            reads: vec![],
        },
        Row {
            case: "an output that states no container at all",
            request: json!({ "settings": settings(&both, output_group(json!({}))) }),
            policy: None,
            status: 400,
            error_type: "BadRequestException",
            reads: vec![],
        },
        Row {
            case: "an output naming a preset this account does not hold",
            request: json!({
                "settings": settings(
                    &both,
                    output_group(json!({ "preset": "no-such-preset" }))
                )
            }),
            policy: None,
            status: 400,
            error_type: "BadRequestException",
            reads: vec![],
        },
        Row {
            case: "an output stating both a preset and container settings",
            request: json!({
                "settings": settings(
                    &both,
                    output_group(json!({
                        "preset": "no-such-preset",
                        "containerSettings": { "container": "MP4" }
                    }))
                )
            }),
            policy: None,
            status: 400,
            error_type: "BadRequestException",
            reads: vec![],
        },
        Row {
            case: "a hop destination naming a queue nothing holds",
            request: json!({
                "hopDestinations": [{ "queue": "no-such-queue", "waitMinutes": 5 }],
                "settings": settings(&both, writable_output_group())
            }),
            policy: None,
            status: 404,
            error_type: "NotFoundException",
            reads: vec![],
        },
        Row {
            case: "a hop to a queue ARN nothing holds, after a hop that names no queue",
            request: json!({
                "queue": HOP_SOURCE_QUEUE,
                "hopDestinations": [
                    { "waitMinutes": 10 },
                    { "queue": arn("queues/no-such-queue"), "waitMinutes": 20 }
                ],
                "settings": settings(&both, writable_output_group())
            }),
            policy: None,
            status: 404,
            error_type: "NotFoundException",
            reads: vec![],
        },
        Row {
            case: "a queue nothing holds",
            request: json!({
                "queue": "no-such-queue",
                "settings": settings(&both, writable_output_group())
            }),
            policy: None,
            status: 404,
            error_type: "NotFoundException",
            reads: vec![],
        },
        Row {
            case: "a job template nothing holds",
            request: json!({
                "jobTemplate": "no-such-template",
                "settings": settings(&both, writable_output_group())
            }),
            policy: None,
            status: 404,
            error_type: "NotFoundException",
            reads: vec![],
        },
        Row {
            case: "an input at a location the account's policy disallows",
            request: json!({ "settings": settings(&both, writable_output_group()) }),
            policy: Some(json!({ "s3Inputs": "DISALLOWED", "httpsInputs": "ALLOWED" })),
            status: 403,
            error_type: "ForbiddenException",
            reads: vec![],
        },
        // Two local refusals at once, which pins the order the walk answers them in: a
        // reference that resolves to nothing is answered before the permission the account
        // withheld, exactly as the decision taken under the storing guard always was.
        Row {
            case: "a disallowed input AND a hop destination naming a queue nothing holds",
            request: json!({
                "hopDestinations": [{ "queue": "no-such-queue", "waitMinutes": 5 }],
                "settings": settings(&both, writable_output_group())
            }),
            policy: Some(json!({ "s3Inputs": "DISALLOWED", "httpsInputs": "ALLOWED" })),
            status: 404,
            error_type: "NotFoundException",
            reads: vec![],
        },
        // The rest of the positive control: a submission nothing local refuses IS probed,
        // so the rows above are counting reads that would really have been issued.
        Row {
            case: "a submission local state admits, stating two S3 inputs",
            request: json!({ "settings": settings(&both, writable_output_group()) }),
            policy: None,
            status: 201,
            error_type: "",
            reads: vec![INPUT_A, INPUT_B],
        },
        Row {
            case: "a submission local state admits, with an input the probe cannot address",
            request: json!({
                "settings": settings(&[INPUT_HTTPS, INPUT_B], writable_output_group())
            }),
            policy: None,
            status: 201,
            error_type: "",
            reads: vec![INPUT_B],
        },
    ]
}

/// The queue the hop row submits its job to, so that row's first hop — which names no
/// queue, and therefore moves the job to the default queue — is a real destination: a hop
/// "cannot be the original queue to which the job is submitted".
const HOP_SOURCE_QUEUE: &str = "hop-source-queue";

/// The request a row submits: its members laid over the required `Role`.
fn request(row: &Row) -> Value {
    let mut body = json!({ "role": ROLE });
    if let (Some(body), Some(members)) = (body.as_object_mut(), row.request.as_object()) {
        for (member, value) in members {
            body.insert(member.clone(), value.clone());
        }
    }
    body
}

/// Store `policy` as the account's input policy, or delete whatever it holds for `None`.
async fn set_policy(emulator: &Emulator, case: &str, policy: Option<&Value>) {
    let reply: Reply = match policy {
        Some(policy) => {
            emulator
                .send(
                    "PUT",
                    "/2017-08-29/policy",
                    Some(json!({ "policy": policy })),
                )
                .await
        }
        None => emulator.send("DELETE", "/2017-08-29/policy", None).await,
    };
    assert!(
        (200..300).contains(&reply.status),
        "case {case:?}: setting the account policy to {policy:?} must succeed, got {} \
         with body {}",
        reply.status,
        reply.body
    );
}

#[tokio::test]
async fn a_submission_refused_by_local_state_reads_no_input() {
    let reads = Reads::default();
    let watched = reads.clone();
    let emulator = Emulator::beside(move |engine| ReadObserver::new(engine, watched));

    let created = emulator
        .create_queue(json!({ "name": HOP_SOURCE_QUEUE }))
        .await;
    assert_eq!(
        created.status, 201,
        "setup: CreateQueue must succeed, got {} with body {}",
        created.status, created.body
    );
    // Whatever the setup read is not a row's reads.
    reads.take();

    let rows = rows();
    let mut accepted = 0;
    for row in &rows {
        let case = row.case;
        if row.policy.is_some() {
            set_policy(&emulator, case, row.policy.as_ref()).await;
            reads.take();
        }

        let reply = emulator.create_job(request(row)).await;
        let read = reads.take();

        assert_eq!(
            reply.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with body {}",
            row.status, reply.status, reply.body
        );
        if row.status == 201 {
            accepted += 1;
        } else {
            assert_eq!(
                reply.error_type.as_deref(),
                Some(row.error_type),
                "case {case:?}: expected {}, got {:?} with body {}",
                row.error_type,
                reply.error_type,
                reply.body
            );
        }
        assert_eq!(
            read, row.reads,
            "case {case:?}: expected the submission to read {:?}, it read {read:?}",
            row.reads
        );

        if row.policy.is_some() {
            set_policy(&emulator, case, None).await;
            reads.take();
        }
    }

    // Nothing a refused row submitted was stored, so the reads asserted above are the
    // whole cost of each refusal.
    let stored = stored_jobs(&emulator).await;
    assert_eq!(
        stored, accepted,
        "only the accepted submissions may be stored: expected {accepted}, got {stored}"
    );
}
