// SPDX-License-Identifier: CC0-1.0

//! The AUDIT leg of this front door: every CodePipeline call it serves is a management
//! event of `codepipeline.amazonaws.com`.
//!
//! RULE: a service that serves a control-plane API call records that call in the account's
//! management-event log — the operation's own name as `eventName`, this service's
//! `eventSource`, the `Management` category, `readOnly` from the operation's own semantics,
//! the Region and the credential it was served for, the parameters the request carried as
//! `requestParameters`, what a MUTATING call answered as `responseElements` (a reporting
//! call and a failed call have none), and, for a call that failed, the FAULT SHAPE's name
//! as `errorCode` with the message the caller was answered with. Each served call gets its
//! OWN record, so a decision and a later refused replay of it are two records with distinct
//! `eventID`s. A call this emulator has not implemented answers 501 and is recorded as
//! nothing: it was never served.
//!
//! The tables below span what the rule generalises over rather than the one call a
//! recording made: the kinds of call (create, read, trigger, answer an approval), both
//! outcomes (served and refused), three fault shapes over three operations, and an
//! operation that is still a stub.
//!
//! Every call goes over the wire through the same router the production emulator uses, as
//! an `awsJson1_1` `POST /` with `X-Amz-Target` (the harness this crate's suites share);
//! the records are read back out of the SHARED log in the engine the service was built
//! from — the one store CloudTrail's `LookupEvents` reads. The end-to-end leg through that
//! operation, and the attribution of a decision to the reviewer's role session, is
//! `core/router/tests/all/codepipeline_management_events.rs`, which needs CloudTrail and
//! STS mounted beside this service.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::service::management_events::EVENT_SOURCE;
use service_common::management_events::{
    ManagementEvent, ManagementEventLog, HIDDEN_SENSITIVE_VALUE,
};

use crate::execution_engine::{artifact_bucket, creation_run, source_stage, Emulator, WAIT};
use crate::pipeline_lifecycle::ROLE_ARN;

/// The access key id the shared request builder signs every call with, which is what the
/// recorded caller of each call below is.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The Region the shared request builder signs for, and so the Region the records live in.
const REGION: &str = "us-east-1";

/// The `eventCategory` CloudTrail stamps on a control-plane call's record.
const MANAGEMENT: &str = "Management";

/// The bucket the approval pipeline's source action reads its revision from.
const SOURCE_BUCKET: &str = "audit-approval-source";

/// A token satisfying `ApprovalToken`'s modelled pattern
/// (`smithy_query(query="operation", operation="PutApprovalResult")` reports
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that no approval ever
/// published.
const UNISSUED_TOKEN: &str = "00000000-0000-0000-0000-000000000000";

/// The webhook secret, the partner's client token and the job nonce one case sends. Values
/// that appear nowhere else, so finding one inside a record would be unambiguous.
const WEBHOOK_SECRET: &str = "n0t-in-the-trail-hmac";
const CLIENT_TOKEN: &str = "n0t-in-the-trail-client";
const JOB_NONCE: &str = "n0t-in-the-trail-nonce";

/// Every record in the shared log, oldest first, as the CloudTrail event document a lookup
/// reports.
fn records(emulator: &Emulator) -> Vec<Value> {
    let log = emulator.engine().state::<ManagementEventLog>();
    let held = log.lock();
    held.iter()
        .map(ManagementEvent::to_cloudtrail_event_json)
        .collect()
}

/// The records of the calls to `event_name`, oldest first.
fn records_of(emulator: &Emulator, event_name: &str) -> Vec<Value> {
    records(emulator)
        .into_iter()
        .filter(|event| event.get("eventName") == Some(&json!(event_name)))
        .collect()
}

/// One row of the served-call table: the call it makes, what the caller sees and what the
/// record of it must carry.
struct Served {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The HTTP status the caller sees. A 501 is the stub channel: nothing was served, so
    /// nothing is recorded.
    status: u16,
    /// The operation's own semantics, as the record's `readOnly`.
    read_only: bool,
    /// The fault shape's name for a refused call, `None` for one that was served.
    error_code: Option<&'static str>,
}

/// A string member of a record, or `None` when the record does not carry it.
fn at(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The SHAPE name of an `awsJson1_1` error answer, whose `__type` may carry the model's
/// namespace.
fn shape_name(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error body, got {body}"))
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert the facts every record of a served call carries, whatever the call was.
fn assert_recorded_call(case: &str, event: &Value, operation: &str, read_only: bool) {
    assert_eq!(
        at(event, "/eventName").as_deref(),
        Some(operation),
        "case {case:?}: the record names the operation, got {event}"
    );
    assert_eq!(
        at(event, "/eventSource").as_deref(),
        Some(EVENT_SOURCE),
        "case {case:?}: the record names this service as its source, got {event}"
    );
    assert_eq!(
        at(event, "/eventCategory").as_deref(),
        Some(MANAGEMENT),
        "case {case:?}: a control-plane call is a management event, got {event}"
    );
    assert_eq!(
        at(event, "/eventType").as_deref(),
        Some("AwsApiCall"),
        "case {case:?}: a recorded API call is an AwsApiCall, got {event}"
    );
    assert_eq!(
        at(event, "/awsRegion").as_deref(),
        Some(REGION),
        "case {case:?}: the record is in the Region the call was served in, got {event}"
    );
    assert_eq!(
        at(event, "/userIdentity/accessKeyId").as_deref(),
        Some(ACCESS_KEY_ID),
        "case {case:?}: the record names the credential the call was signed with, got {event}"
    );
    assert_eq!(
        event.get("readOnly"),
        Some(&json!(read_only)),
        "case {case:?}: expected readOnly {read_only}, got {event}"
    );
}

/// The declaration of a pipeline whose `Review` stage holds one manual-approval action.
fn approval_pipeline(name: &str, bucket: &str) -> Value {
    json!({
        "pipeline": {
            "name": name,
            "roleArn": ROLE_ARN,
            "artifactStore": {"type": "S3", "location": bucket},
            "stages": [
                source_stage(SOURCE_BUCKET, "source.zip", "SourceArtifact"),
                {
                    "name": "Review",
                    "actions": [{
                        "name": "ManualApproval",
                        "actionTypeId": {
                            "category": "Approval",
                            "owner": "AWS",
                            "provider": "Manual",
                            "version": "1",
                        },
                    }],
                },
            ],
        }
    })
}

/// A pipeline whose approval action has been reached, and the token it published.
async fn pipeline_awaiting_approval(emulator: &Emulator, name: &str) -> String {
    let bucket = artifact_bucket(name);
    emulator.bucket(&bucket).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator
        .put_object(SOURCE_BUCKET, "source.zip", b"approval bytes")
        .await;
    emulator
        .ok("CreatePipeline", &approval_pipeline(name, &bucket))
        .await;
    // The execution the pipeline's own creation started is the one that reaches the
    // approval.
    creation_run(emulator, name).await;

    let deadline = Instant::now() + WAIT;
    loop {
        let state = emulator.ok("GetPipelineState", &json!({"name": name})).await;
        let token = state["stageStates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|stage| stage["stageName"] == json!("Review"))
            .filter_map(|stage| stage["actionStates"].as_array())
            .flatten()
            .find(|action| action["actionName"] == json!("ManualApproval"))
            .and_then(|action| action["latestExecution"]["token"].as_str())
            .map(str::to_string);
        if let Some(token) = token {
            return token;
        }
        assert!(
            Instant::now() < deadline,
            "the approval action never published a token; last state was {state}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// RULE: every call this front door SERVES is recorded — whatever kind of call it is and
/// whatever it answered — and one this emulator has not implemented is recorded as nothing.
///
/// The rows differ along the kind of call (create, read of one resource, read of a listing,
/// trigger), along the outcome (served, refused) and along whether the operation is
/// implemented at all, which is what the rule generalises over.
#[tokio::test]
async fn every_served_call_is_recorded_and_an_unimplemented_one_is_not() {
    let emulator = Emulator::new();
    emulator.bucket(&artifact_bucket("audit-served")).await;

    let rows: Vec<Served> = vec![
        Served {
            case: "a create",
            operation: "CreatePipeline",
            body: approval_pipeline("audit-served", &artifact_bucket("audit-served")),
            status: 200,
            read_only: false,
            error_code: None,
        },
        Served {
            case: "a read of one pipeline",
            operation: "GetPipeline",
            body: json!({"name": "audit-served"}),
            status: 200,
            read_only: true,
            error_code: None,
        },
        Served {
            case: "a read of a listing",
            operation: "ListPipelines",
            body: json!({}),
            status: 200,
            read_only: true,
            error_code: None,
        },
        Served {
            case: "a refused read of one pipeline",
            operation: "GetPipeline",
            body: json!({"name": "no-such-pipeline"}),
            status: 400,
            read_only: true,
            error_code: Some("PipelineNotFoundException"),
        },
        Served {
            case: "a refused create",
            operation: "CreatePipeline",
            body: approval_pipeline("audit-served", &artifact_bucket("audit-served")),
            status: 400,
            read_only: false,
            error_code: Some("PipelineNameInUseException"),
        },
        Served {
            case: "a tag read of an unknown ARN",
            operation: "ListTagsForResource",
            body: json!({"resourceArn": format!(
                "arn:aws:codepipeline:{REGION}:{}:no-such-pipeline",
                engine::emulator_account_id()
            )}),
            status: 400,
            read_only: true,
            error_code: Some("ResourceNotFoundException"),
        },
        Served {
            // Still a stub in this emulator: it answers 501, so nothing was served and
            // nothing is recorded.
            case: "an unimplemented operation",
            operation: "ListRuleTypes",
            body: json!({}),
            status: 501,
            read_only: true,
            error_code: None,
        },
    ];

    for row in &rows {
        let (case, operation, expected_status, expected_error) =
            (row.case, row.operation, row.status, row.error_code);
        let before = records(&emulator).len();
        let (status, answered) = emulator.cp(operation, &row.body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {answered}"
        );
        let recorded = records(&emulator);
        if expected_status == 501 {
            assert_eq!(
                recorded.len(),
                before,
                "case {case:?}: an unimplemented call records nothing, got {recorded:?}"
            );
            continue;
        }
        assert_eq!(
            recorded.len(),
            before + 1,
            "case {case:?}: expected exactly one new record, got {recorded:?}"
        );
        let event = recorded.last().expect("checked just above");
        assert_recorded_call(case, event, operation, row.read_only);
        assert_eq!(
            at(event, "/errorCode").as_deref(),
            expected_error,
            "case {case:?}: expected errorCode {expected_error:?}, got {event}"
        );
        match expected_error {
            None => assert!(
                at(event, "/errorMessage").is_none(),
                "case {case:?}: a served call carries no error message, got {event}"
            ),
            Some(_) => {
                assert!(
                    at(event, "/errorMessage").is_some_and(|message| !message.is_empty()),
                    "case {case:?}: the record carries the message the caller was answered \
                     with, got {event}"
                );
                assert_eq!(
                    event.get("responseElements"),
                    Some(&Value::Null),
                    "case {case:?}: a failed call answered nothing, got {event}"
                );
            }
        }
        // The record names the request the CALLER was answered under, so a client holding
        // the response finds the record of that very call.
        assert!(
            at(event, "/requestID").is_some_and(|id| !id.is_empty()),
            "case {case:?}: the record names the request it answers, got {event}"
        );
    }

    // What the two kinds of call carry: a mutating one records what it answered, a
    // reporting one records nothing of the sort, and both record the members the request
    // sent.
    let create = records_of(&emulator, "CreatePipeline")
        .into_iter()
        .next()
        .expect("the create is recorded");
    assert_eq!(
        at(&create, "/requestParameters/pipeline/name").as_deref(),
        Some("audit-served"),
        "the create's record carries the declaration it was sent, got {create}"
    );
    assert_eq!(
        at(&create, "/responseElements/pipeline/name").as_deref(),
        Some("audit-served"),
        "a mutating call records what it answered, got {create}"
    );
    let listed = records_of(&emulator, "ListPipelines")
        .into_iter()
        .next()
        .expect("the listing is recorded");
    assert_eq!(
        listed.get("responseElements"),
        Some(&Value::Null),
        "a reporting call records no responseElements, got {listed}"
    );
}

/// RULE: an approval DECISION and a later refused replay of the same token are two records
/// of their own — distinct `eventID`s, the parameters each call carried, the answered
/// values on the one that was served and the fault shape's own name and message on the one
/// that was refused.
///
/// This is the pair a governance client reconciles: who answered the approval, and that a
/// spent or unissued token was refused.
#[tokio::test]
async fn an_approval_decision_and_its_refused_replay_are_both_recorded() {
    let emulator = Emulator::new();
    let token = pipeline_awaiting_approval(&emulator, "audit-approval").await;
    let summary = "signed off by the reviewer";

    let decision = json!({
        "pipelineName": "audit-approval",
        "stageName": "Review",
        "actionName": "ManualApproval",
        "result": {"status": "Approved", "summary": summary},
        "token": token,
    });
    let answered = emulator.ok("PutApprovalResult", &decision).await;
    assert!(
        answered["approvedAt"].is_number() || answered["approvedAt"].is_string(),
        "the accepted decision reports approvedAt, got {answered}"
    );

    // The replay of the now-spent token. Which refusal it is belongs to the approval
    // handler, not to the audit leg: the code the CALLER was answered with is read off the
    // response and the record must repeat it.
    let (status, refused) = emulator.cp("PutApprovalResult", &decision).await;
    assert_eq!(
        status, 400,
        "the replay of a spent token is refused, got {status} with {refused}"
    );
    let refused_code = shape_name(&refused);
    let refused_message = refused["message"]
        .as_str()
        .or_else(|| refused["Message"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        !refused_message.is_empty(),
        "the refusal must state a message, got {refused}"
    );

    let recorded = records_of(&emulator, "PutApprovalResult");
    assert_eq!(
        recorded.len(),
        2,
        "the decision and its replay are two records, got {recorded:?}"
    );
    let served = &recorded[0];
    let replay = &recorded[1];

    assert_recorded_call("the decision", served, "PutApprovalResult", false);
    assert_recorded_call("the replay", replay, "PutApprovalResult", false);
    assert_ne!(
        at(served, "/eventID"),
        at(replay, "/eventID"),
        "each served call is its own event, got {recorded:?}"
    );

    // The decision's record: the members the reviewer sent, and the value it answered.
    for (pointer, expected) in [
        ("/requestParameters/pipelineName", "audit-approval"),
        ("/requestParameters/stageName", "Review"),
        ("/requestParameters/actionName", "ManualApproval"),
        ("/requestParameters/result/status", "Approved"),
        ("/requestParameters/result/summary", summary),
        ("/requestParameters/token", token.as_str()),
    ] {
        assert_eq!(
            at(served, pointer).as_deref(),
            Some(expected),
            "the decision's record carries {pointer} as sent, got {served}"
        );
    }
    assert_eq!(
        at(served, "/errorCode"),
        None,
        "a served decision carries no error code, got {served}"
    );
    assert!(
        served.get("responseElements").is_some_and(|elements| {
            elements.get("approvedAt").is_some()
        }),
        "the decision's record carries what it answered, got {served}"
    );

    // The replay's record: the refusal verbatim, and nothing answered.
    assert_eq!(
        at(replay, "/errorCode").as_deref(),
        Some(refused_code.as_str()),
        "the record repeats the code the caller was answered with, got {replay}"
    );
    assert_eq!(
        at(replay, "/errorMessage").as_deref(),
        Some(refused_message.as_str()),
        "the record repeats the message the caller was answered with, got {replay}"
    );
    assert_eq!(
        replay.get("responseElements"),
        Some(&Value::Null),
        "a refused call answered nothing, got {replay}"
    );
}

/// RULE: the `errorCode` of a failed call's record is the FAULT SHAPE's name — the one the
/// caller was answered with, for every refusal class, not only the one a recording captured.
///
/// The rows span three fault shapes over three operations, and both kinds of call (a read
/// and two writes).
#[tokio::test]
async fn a_failed_call_records_the_fault_shape_the_caller_saw() {
    let emulator = Emulator::new();
    let token = pipeline_awaiting_approval(&emulator, "audit-faults").await;

    // (case, operation, body, the fault shape the caller must see)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an approval for a pipeline nothing holds",
            "PutApprovalResult",
            json!({
                "pipelineName": "no-such-pipeline",
                "stageName": "Review",
                "actionName": "ManualApproval",
                "result": {"status": "Approved", "summary": "nope"},
                "token": token,
            }),
            "PipelineNotFoundException",
        ),
        (
            "an approval for a stage the pipeline does not have",
            "PutApprovalResult",
            json!({
                "pipelineName": "audit-faults",
                "stageName": "NoSuchStage",
                "actionName": "ManualApproval",
                "result": {"status": "Approved", "summary": "nope"},
                "token": token,
            }),
            "StageNotFoundException",
        ),
        (
            "an approval whose token no approval published",
            "PutApprovalResult",
            json!({
                "pipelineName": "audit-faults",
                "stageName": "Review",
                "actionName": "ManualApproval",
                "result": {"status": "Approved", "summary": "nope"},
                "token": UNISSUED_TOKEN,
            }),
            "InvalidApprovalTokenException",
        ),
        (
            "a start of a pipeline nothing holds",
            "StartPipelineExecution",
            json!({"name": "no-such-pipeline"}),
            "PipelineNotFoundException",
        ),
        (
            "a read of an execution of a pipeline nothing holds",
            "GetPipelineExecution",
            json!({
                "pipelineName": "no-such-pipeline",
                "pipelineExecutionId": "11111111-2222-3333-4444-555555555555",
            }),
            "PipelineNotFoundException",
        ),
    ];

    for (case, operation, body, expected_shape) in &rows {
        let before = records_of(&emulator, operation).len();
        let (status, refused) = emulator.cp(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        let answered = shape_name(&refused);
        assert_eq!(
            answered, *expected_shape,
            "case {case:?}: expected the caller to see {expected_shape:?}, got {refused}"
        );
        let recorded = records_of(&emulator, operation);
        assert_eq!(
            recorded.len(),
            before + 1,
            "case {case:?}: the refused call must be recorded, got {recorded:?}"
        );
        let event = recorded.last().expect("checked just above");
        assert_eq!(
            at(event, "/errorCode").as_deref(),
            Some(*expected_shape),
            "case {case:?}: expected errorCode {expected_shape:?}, got {event}"
        );
        assert!(
            at(event, "/errorMessage").is_some_and(|message| !message.is_empty()),
            "case {case:?}: the record carries the message the caller saw, got {event}"
        );
    }
}

/// One row of the redaction table: the call it makes, the member pointers whose values the
/// record must withhold, and the plaintexts no record may carry.
struct Withheld {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// JSON pointers into the record, as a record spells its members.
    pointers: Vec<&'static str>,
    /// The values the call sent at those members.
    plaintexts: Vec<&'static str>,
}

/// RULE: a member a call carries CREDENTIAL material in is never stored in a record — the
/// record carries CloudTrail's fixed redaction literal in its place, so the record still
/// shows that the call carried the member while the plaintext is retrievable from event
/// history nowhere.
///
/// The rows span the credential-bearing members of this service's surface rather than one
/// of them: the webhook HMAC secret inside a definition (a member the serving API already
/// withholds from its own answers), the partner's `clientToken` on two different
/// third-party job operations, and the `nonce` a job claim carries — and both outcomes, a
/// served call and a refused one.
#[tokio::test]
async fn a_record_withholds_every_credential_bearing_member() {
    let emulator = Emulator::new();
    emulator.bucket(&artifact_bucket("audit-secrets")).await;
    emulator
        .ok(
            "CreatePipeline",
            &approval_pipeline("audit-secrets", &artifact_bucket("audit-secrets")),
        )
        .await;

    // (case, operation, body, the member pointers whose values the record must withhold,
    //  the plaintext values that must appear in no record at all)
    let rows: Vec<Withheld> = vec![
        Withheld {
            case: "a webhook definition carrying its HMAC secret",
            operation: "PutWebhook",
            body: json!({"webhook": {
                "name": "audit-hook",
                "targetPipeline": "audit-secrets",
                "targetAction": "Src",
                "filters": [{"jsonPath": "$.ref", "matchEquals": "refs/heads/main"}],
                "authentication": "GITHUB_HMAC",
                "authenticationConfiguration": {"SecretToken": WEBHOOK_SECRET},
            }}),
            // A record spells a member with a lower-case first letter
            // (`service_common::management_events::recorded_member_name`), so the modelled
            // `SecretToken` is `secretToken` in the document.
            pointers: vec!["/requestParameters/webhook/authenticationConfiguration/secretToken"],
            plaintexts: vec![WEBHOOK_SECRET],
        },
        Withheld {
            case: "a third-party job failure report carrying the partner's client token",
            operation: "PutThirdPartyJobFailureResult",
            body: json!({
                "jobId": "no-such-third-party-job",
                "clientToken": CLIENT_TOKEN,
                "failureDetails": {"type": "JobFailed", "message": "nothing ran"},
            }),
            pointers: vec!["/requestParameters/clientToken"],
            plaintexts: vec![CLIENT_TOKEN],
        },
        Withheld {
            case: "a third-party job claim carrying the token and the nonce",
            operation: "AcknowledgeThirdPartyJob",
            body: json!({
                "jobId": "no-such-third-party-job",
                "nonce": JOB_NONCE,
                "clientToken": CLIENT_TOKEN,
            }),
            pointers: vec![
                "/requestParameters/clientToken",
                "/requestParameters/nonce",
            ],
            plaintexts: vec![CLIENT_TOKEN, JOB_NONCE],
        },
    ];

    for row in &rows {
        let (case, operation) = (row.case, row.operation);
        let before = records_of(&emulator, operation).len();
        let (status, answered) = emulator.cp(operation, &row.body).await;
        assert!(
            status == 200 || (400..500).contains(&status),
            "case {case:?}: the call must be served or refused, got {status} with {answered}"
        );
        let recorded = records_of(&emulator, operation);
        assert_eq!(
            recorded.len(),
            before + 1,
            "case {case:?}: the call must be recorded, got {recorded:?}"
        );
        let event = recorded.last().expect("checked just above");
        for pointer in &row.pointers {
            assert_eq!(
                at(event, pointer).as_deref(),
                Some(HIDDEN_SENSITIVE_VALUE),
                "case {case:?}: the record must withhold {pointer}, got {event}"
            );
        }
        // Nowhere in the WHOLE log, so neither the parameters, the answered values nor the
        // message of any record carries the plaintext.
        let whole_log = records(&emulator)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>()
            .join("\n");
        for plaintext in &row.plaintexts {
            assert!(
                !whole_log.contains(plaintext),
                "case {case:?}: no record may carry {plaintext:?}, got {whole_log}"
            );
        }
    }
}
