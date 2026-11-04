// SPDX-License-Identifier: CC0-1.0

//! `GetAgentTaskResponseUrl`: the presigned URL an AWS Ground Station Agent uploads its
//! task response log to.
//!
//! The RULE under test: for an agent this account has registered, the operation reports a
//! presigned Amazon S3 `PutObject` URL of the object holding THAT agent's response log for
//! THAT task — one object per agent per task, in the bucket this service takes an
//! account's agent logs in — so an agent that PUTs its log to the URL stores it in object
//! storage; the URL reported for a task is reported again while it is still signed for,
//! and goes on being taken whatever else is read, because only an upload the clock has run
//! out on is withdrawn; and an `{agentId}` no agent was registered under is refused
//! `InvalidParameterException` (HTTP 431).
//!
//! So this suite mounts the S3 service beside Ground Station on ONE engine — what the
//! assembled emulator does — and FOLLOWS each URL it is handed through S3's own front
//! door, so what it asserts is an upload object storage really took rather than the shape
//! of a string.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    service_state, AGENT_TASK_RESPONSE_URL_VALIDITY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::agent_lifecycle::{capability_arn, get_configuration, register, uuid_from};
use crate::s3_recording_buckets::{
    harness_with_object_storage, harness_with_object_storage_and_engine,
};
use crate::scheduling_loop::{send, Reply, ABSENT_UUID, REGION};

/// A SigV4 `Authorization` header in the `s3` scope: the routing signal that picks the S3
/// service for a request this suite sends to object storage in its OWN name (reading an
/// uploaded object back). An agent's upload carries no such header — see [`put_url`].
const S3_AUTH: &str = "AWS4-HMAC-SHA256 Credential=AKIDTESTS3/20260101/us-east-1/s3/\
                       aws4_request, SignedHeaders=host, Signature=abcdef";

/// The SigV4 algorithm a presigned URL names, and the fields it carries beside it —
/// AWS's published query-string authentication parameters.
const PRESIGN_ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// A `RegisterAgent` body for the `index`th agent of a table: the required members, with
/// the agent cpu cores the service requires one of the `AgentDetails` pair for, and an
/// instance, an address and a capability of its own so no two agents of a table are the
/// same registration.
fn registration(index: usize) -> Value {
    json!({
        "agentDetails": {
            "agentVersion": "1.0.0",
            "instanceId": format!("i-{index:010x}"),
            "instanceType": "m5.large",
            "agentCpuCores": [0, 1],
            "componentVersions": [{"componentType": "Agent", "versions": ["1.0.0"]}],
        },
        "discoveryData": {
            "publicIpAddresses": [format!("52.0.0.{}", index + 1)],
            "privateIpAddresses": [format!("10.0.0.{}", index + 1)],
            "capabilityArns": [capability_arn(index)],
        },
    })
}

/// `GET /agentResponseUrl/{agentId}/{taskId}`.
async fn response_url(router: &MultiServiceRouter, agent_id: &str, task_id: &str) -> Reply {
    send(
        router,
        "GET",
        &format!("/agentResponseUrl/{agent_id}/{task_id}"),
        None,
    )
    .await
}

/// The object `url` addresses: everything before its query string.
fn object_url(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// The value of the query field `name` of `url`, or `None` when the URL carries no such
/// field. The query string of a presigned URL is `name=value` pairs separated by `&`.
fn query_field(url: &str, name: &str) -> Option<String> {
    url.split_once('?')?.1.split('&').find_map(|pair| {
        let (field, value) = pair.split_once('=')?;
        (field == name).then(|| value.to_string())
    })
}

/// Where AWS Ground Station takes this account's agent task response logs: one bucket per
/// account per region under this service's ARN namespace, and one object per agent per
/// task under it.
///
/// Written out here rather than taken from the crate's own helpers, so this asserts the
/// layout a client sees instead of echoing whatever the handler computed.
fn expected_object_url(agent_id: &str, task_id: &str) -> String {
    format!(
        "https://s3.{REGION}.amazonaws.com/groundstation-agent-logs-{REGION}-{}\
         /agent/{agent_id}/task/{task_id}/agent-task-response.log",
        engine::emulator_account_id()
    )
}

/// Follow a presigned URL the way the agent would: the URL exactly as it was reported,
/// with NO `Authorization` header, because a presigned request carries its credential in
/// the query string. Answers the HTTP status.
async fn put_url(router: &MultiServiceRouter, url: &str, body: &[u8]) -> u16 {
    let request = http::Request::builder()
        .method("PUT")
        .uri(url)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("build the upload request");
    router.dispatch(request).await.status().as_u16()
}

/// Read an object back through S3's own front door: a path-style `GetObject` whose
/// credential scope names `s3`. Answers the status and the bytes.
async fn get_object(router: &MultiServiceRouter, object_url: &str) -> (u16, Vec<u8>) {
    let request = http::Request::builder()
        .method("GET")
        .uri(object_url)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTH)
        .body(bytes::Bytes::new())
        .expect("build the GetObject request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.into_body().to_vec())
}

/// What the service holds for `task_id` of `agent_id`, as `(credential id, URL)`, or
/// `None` when it holds no upload for that task (`state::Agent::log_uploads`, which no
/// modelled read reports).
fn held_upload(
    engine: &engine::InMemoryEngine,
    agent_id: &str,
    task_id: &str,
) -> Option<(String, String)> {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .agents
        .get(agent_id)?
        .log_uploads
        .get(task_id)
        .map(|upload| {
            (
                upload.credential_id.clone(),
                upload.presigned_log_url.clone(),
            )
        })
}

/// The task ids a table asks for a response URL for: values differing along what the rule
/// generalises over — the `Uuid` label itself, at each end of the range its pattern spells
/// and in between (`taskId` is a `Uuid`, len 36..36, pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$` —
/// `smithy_query(query="operation", operation="GetAgentTaskResponseUrl")`).
fn task_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "the lowest value the pattern spells",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("a task id of the agent suite's own form", uuid_from(0x7a5c, 7)),
        (
            "the highest value the pattern spells",
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        ),
    ]
}

#[tokio::test]
async fn every_agent_task_is_handed_a_presigned_upload_url_for_its_own_log_object() {
    let router = harness_with_object_storage();

    // Two agents, so what the URL carries varies with the agent as well as with the task.
    let mut agents: Vec<String> = Vec::new();
    for index in 0..2 {
        let registered = register(&router, registration(index)).await;
        assert_eq!(
            registered.status, 200,
            "agent {index}: expected RegisterAgent 200, got {} {}",
            registered.status, registered.body
        );
        agents.push(registered.string("agentId"));
    }

    let mut seen: Vec<String> = Vec::new();
    let mut signers: Vec<String> = Vec::new();
    let mut signed: Vec<(String, String, String)> = Vec::new();
    for agent_id in &agents {
        for (label, task_id) in task_ids() {
            let case = format!("agent {agent_id}, task {label}");
            let reply = response_url(&router, agent_id, &task_id).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: expected GetAgentTaskResponseUrl 200, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.string("agentId"),
                *agent_id,
                "case {case}: expected the agent the request addressed, got {}",
                reply.body
            );
            assert_eq!(
                reply.string("taskId"),
                task_id,
                "case {case}: expected the task the request named, got {}",
                reply.body
            );

            // The URL addresses THIS agent's log object for THIS task.
            let url = reply.string("presignedLogUrl");
            assert_eq!(
                object_url(&url),
                expected_object_url(agent_id, &task_id),
                "case {case}: the URL must address this agent's log object for this task, \
                 got {url}"
            );

            // And it is presigned: the query-string credential scope, validity and
            // signature a client needs to upload with it.
            assert_eq!(
                query_field(&url, "X-Amz-Algorithm").as_deref(),
                Some(PRESIGN_ALGORITHM),
                "case {case}: expected a presigned URL naming {PRESIGN_ALGORITHM}, got {url}"
            );
            let credential = query_field(&url, "X-Amz-Credential")
                .unwrap_or_else(|| panic!("case {case}: the URL carries no credential: {url}"));
            assert!(
                credential.contains(&format!("{REGION}%2Fs3%2Faws4_request")),
                "case {case}: the URL must be signed for object storage in {REGION}, got \
                 {credential}"
            );
            assert_eq!(
                query_field(&url, "X-Amz-Expires").as_deref(),
                Some(AGENT_TASK_RESPONSE_URL_VALIDITY.as_secs().to_string().as_str()),
                "case {case}: the URL must be signed for the validity this service gives \
                 one, got {url}"
            );
            assert!(
                query_field(&url, "X-Amz-Date").is_some_and(|date| !date.is_empty()),
                "case {case}: the URL must carry the instant it was signed at, got {url}"
            );
            assert!(
                query_field(&url, "X-Amz-Signature").is_some_and(|signature| !signature.is_empty()),
                "case {case}: the URL must carry a signature, got {url}"
            );

            // Every upload is its own: no two of them are the same URL, and no two are
            // authorised by the same credential.
            assert!(
                !seen.contains(&url),
                "case {case}: every agent task has its own upload URL, but {url} was \
                 reported twice"
            );
            assert!(
                !signers.contains(&credential),
                "case {case}: every upload URL is signed with a credential of its own, but \
                 {credential} signed two"
            );
            seen.push(url.clone());
            signers.push(credential);
            signed.push((agent_id.clone(), task_id, url));
        }
    }

    // Every URL WORKS, and still works after every other one has been signed: the uploads
    // are followed only once the whole table has been handed out, in the opposite order,
    // so a URL that was signed first is used last. Following it stores the log in object
    // storage, and the object reads back byte for byte through S3's own front door.
    for (agent_id, task_id, url) in signed.into_iter().rev() {
        let case = format!("agent {agent_id}, task {task_id}");
        let log = format!("agent {agent_id} finished task {task_id}").into_bytes();
        let uploaded = put_url(&router, &url, &log).await;
        assert_eq!(
            uploaded, 200,
            "case {case}: expected the presigned upload to be taken (200), got {uploaded} \
             for {url}"
        );
        let object = expected_object_url(&agent_id, &task_id);
        let (read_status, read_back) = get_object(&router, &object).await;
        assert_eq!(
            read_status, 200,
            "case {case}: expected the uploaded log to be readable (200), got {read_status} \
             for {object}"
        );
        assert_eq!(
            read_back, log,
            "case {case}: the object must hold the bytes that were uploaded, got {}",
            String::from_utf8_lossy(&read_back)
        );
    }
}

#[tokio::test]
async fn a_tasks_url_is_reported_again_and_outlives_the_signing_of_another_tasks() {
    let (router, engine) = harness_with_object_storage_and_engine();
    let registered = register(&router, registration(0)).await;
    let agent_id = registered.string("agentId");

    let (first_task, second_task) = (uuid_from(0xa1, 1), uuid_from(0xa1, 2));
    let first = response_url(&router, &agent_id, &first_task).await;
    assert_eq!(
        first.status, 200,
        "case first-call: expected 200, got {} {}",
        first.status, first.body
    );
    let url = first.string("presignedLogUrl");

    // What the service HOLDS for the first task: the URL it reported and the credential it
    // signed it with. Read from the store because nothing modelled reports it, and because
    // a presigned signature is only verified against the credential store when the front
    // door's authentication is on, which no suite in this target turns on — so the store is
    // where "the credential behind this URL is still there" can be seen at all.
    let held_for_first = || {
        held_upload(&engine, &agent_id, &first_task)
            .unwrap_or_else(|| panic!("the service holds no upload for the first task"))
    };
    let (first_credential, first_held_url) = held_for_first();
    assert_eq!(
        first_held_url, url,
        "case first-call: the service must hold the URL it reported, got {first_held_url}"
    );

    // A second read of the same task reports the SAME upload: the agent is handed the
    // upload it already holds rather than a second one, so the URL it is already using
    // goes on working.
    let again = response_url(&router, &agent_id, &first_task).await;
    assert_eq!(
        again.status, 200,
        "case second-call-same-task: expected 200, got {} {}",
        again.status, again.body
    );
    assert_eq!(
        again.string("presignedLogUrl"),
        url,
        "case second-call-same-task: the task's upload URL must be reported again, got {}",
        again.body
    );

    // Another task of the same agent is its own upload, at its own object.
    let other = response_url(&router, &agent_id, &second_task).await;
    assert_eq!(
        other.status, 200,
        "case another-task: expected 200, got {} {}",
        other.status, other.body
    );
    let other_url = other.string("presignedLogUrl");
    assert_ne!(
        other_url, url,
        "case another-task: a different task must not be handed the first task's upload"
    );
    assert_eq!(
        object_url(&other_url),
        expected_object_url(&agent_id, &second_task),
        "case another-task: the URL must address that task's own log object, got {other_url}"
    );

    // Reading about the second task REVOKED NOTHING: the first task's upload is held
    // exactly as it was, same URL and same signing credential, because only an upload the
    // clock has run out on is ever withdrawn.
    let (credential_after, url_after) = held_for_first();
    assert_eq!(
        (url_after.as_str(), credential_after.as_str()),
        (url.as_str(), first_credential.as_str()),
        "case another-task: the first task's upload and the credential signing it must be \
         untouched, got {url_after} signed by {credential_after}"
    );
    assert!(
        held_upload(&engine, &agent_id, &second_task).is_some(),
        "case another-task: the second task's upload must be held beside the first's"
    );

    // And the FIRST task's URL is still taken afterwards: reading about another task
    // revokes nothing, because only an upload the clock has run out on is ever withdrawn.
    // The log is uploaded in the opposite order to the reads, so each upload is taken
    // after the other task's URL was signed.
    for (case, task_id, upload_url) in [
        ("the task signed first, uploaded last", &first_task, &url),
        ("the task signed last, uploaded first", &second_task, &other_url),
    ]
    .into_iter()
    .rev()
    {
        let log = format!("agent {agent_id} finished task {task_id}").into_bytes();
        let uploaded = put_url(&router, upload_url, &log).await;
        assert_eq!(
            uploaded, 200,
            "case {case}: a URL that has not expired must still be taken (200), got \
             {uploaded} for {upload_url}"
        );
        let object = expected_object_url(&agent_id, task_id);
        let (read_status, read_back) = get_object(&router, &object).await;
        assert_eq!(
            read_status, 200,
            "case {case}: expected the uploaded log to be readable (200), got {read_status} \
             for {object}"
        );
        assert_eq!(
            read_back, log,
            "case {case}: the object must hold the bytes that were uploaded, got {}",
            String::from_utf8_lossy(&read_back)
        );
    }

    // And none of it changed what the agent's own modelled read reports: asking for an
    // upload URL is a read of the agent, not a report about it.
    let configuration = get_configuration(&router, &agent_id).await;
    assert_eq!(
        configuration.status, 200,
        "case agent-read-after: expected GetAgentConfiguration 200, got {} {}",
        configuration.status, configuration.body
    );
    assert_eq!(
        configuration.string("agentId"),
        agent_id,
        "case agent-read-after: expected the agent that was registered, got {}",
        configuration.body
    );
    assert_eq!(
        configuration.field("taskingDocument"),
        &Value::Null,
        "case agent-read-after: nothing has tasked this agent, so it holds no tasking \
         document, got {}",
        configuration.body
    );
}

#[tokio::test]
async fn an_upload_url_is_refused_for_an_agent_that_was_never_registered() {
    let router = harness_with_object_storage();
    // One agent IS registered, so the refusal below is about the agent the request names
    // rather than about an account that has registered nothing.
    let registered = register(&router, registration(0)).await;
    let known_agent = registered.string("agentId");

    // Rows: agent ids no registration minted — one that names nothing at all, and a NEAR
    // MISS of the registered agent's id, so acceptance cannot be a prefix match.
    let mut near_miss: Vec<char> = known_agent.chars().collect();
    let last = near_miss.len() - 1;
    near_miss[last] = if near_miss[last] == 'a' { 'b' } else { 'a' };
    let near_miss: String = near_miss.into_iter().collect();
    let rows = [
        ("an id that names nothing", ABSENT_UUID.to_string()),
        ("a near miss of a registered agent", near_miss),
    ];

    for (case, agent_id) in rows {
        for (task_case, task_id) in task_ids() {
            let refused = response_url(&router, &agent_id, &task_id).await;
            assert_eq!(
                refused.status, 431,
                "case {case} with {task_case}: expected 431, got {} {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "InvalidParameterException",
                "case {case} with {task_case}: expected InvalidParameterException, got {}",
                refused.body
            );
            assert_eq!(
                refused.string("message"),
                "Invalid AgentID.",
                "case {case} with {task_case}: wrong refusal text, got {}",
                refused.body
            );
        }
    }

    // The agent that IS registered is still handed its upload URL.
    let served = response_url(&router, &known_agent, &task_ids()[0].1).await;
    assert_eq!(
        served.status, 200,
        "case the-registered-agent: expected 200, got {} {}",
        served.status, served.body
    );
}
