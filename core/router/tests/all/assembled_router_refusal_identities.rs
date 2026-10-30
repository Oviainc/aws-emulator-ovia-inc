// SPDX-License-Identifier: CC0-1.0

//! Each service's own refusal identity, served by the ASSEMBLED emulator.
//!
//! RULE under test: **a request is answered by the service its SigV4 credential scope and
//! its operation name resolve to, so every refusal carries THAT operation's own modelled
//! error — its error code, its HTTP status and the one sentence AWS words for that error
//! class, computed from the request and the store — and mounting every other service beside
//! it changes none of them.**
//!
//! Why it lives here rather than in any one service crate: every per-service suite builds a
//! `MultiServiceRouter` holding its OWN service alone, so none of them can observe a
//! neighbour's `can_handle` claiming a request, or the shared refusal rendering answering in
//! another service's envelope. The assembled router — the one `build_router` produces and
//! the one a client talks to — is the surface where a refusal can be served by the wrong
//! service, and a refusal served by the wrong service is exactly a refusal whose code,
//! status or sentence changes. This suite drives that surface.
//!
//! The rows span what the rule generalises over: five services, two wire protocols
//! (`awsJson1_1` and `restJson1`), thirteen operations, four modelled refusal classes at
//! three HTTP statuses, and for each class the inputs its own rule generalises over —
//! resource names of differing length, case and character set; page sizes on each side of
//! the published range and inside it; Regions that do and do not perform an account-global
//! action; and opaque tokens of each form a reader classifies. Every section also drives
//! requests the same service must SERVE, read back where there is something to read, so a
//! refusal is never the whole of what a section proves.
//!
//! The services here are the ones whose refusal identity is decided by the REQUEST alone,
//! so a row needs no provisioning to reach the refusing path and the assembled router is
//! the only variable left. A refusal reached only through a provisioning sequence belongs
//! with that sequence, and the sequences this report also names already run against the
//! assembled router in their own suites beside this file —
//! `cloudformation_canceled_delete_keeps_exported_resources.rs`,
//! `cloudformation_provisions_codepipeline_pipelines.rs` and
//! `cloudformation_opensearch_domain_deployments.rs`.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use service_com_amazonaws_xray_awsxray::state::{
    retrieval_token_check, RETRIEVAL_STATUS_CANCELLED, RETRIEVAL_STATUS_COMPLETE,
    RETRIEVAL_STATUS_RUNNING, RETRIEVAL_STATUS_SCHEDULED,
};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region most sections sign for. Nothing below is Region-scoped except the ECR Public
/// section, which names its own Regions.
const REGION: &str = "us-west-2";

/// The `Content-Type` of the `awsJson1_1` services below.
const JSON_1_1: &str = "application/x-amz-json-1.1";

/// The `Content-Type` of the `restJson1` services below.
const REST_JSON: &str = "application/json";

/// One request to the assembled router: who it is signed for, where it is addressed and
/// what it carries.
struct Call<'a> {
    signing_name: &'a str,
    region: &'a str,
    path: &'a str,
    content_type: &'a str,
    /// `Some` for an RPC protocol that names its operation in `X-Amz-Target`.
    target: Option<String>,
    body: String,
}

/// What one call answered: the status, the wire error code on whichever channel the
/// protocol renders it, and the message.
#[derive(Debug)]
struct Answer {
    status: u16,
    code: String,
    message: String,
    body: Value,
}

/// The assembled emulator — every service on one engine — and the runtime its requests are
/// driven on.
struct Emulator {
    router: smithy_server_runtime::MultiServiceRouter,
    runtime: tokio::runtime::Runtime,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = build_router(engine);
        // These rows exercise handlers, not SigV4; the credential scope still has to route
        // and still carries the Region a handler reads.
        router.set_request_authentication(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a current-thread runtime");
        Self { router, runtime }
    }

    fn send(&self, call: Call<'_>) -> Answer {
        let host = format!("{}.{}.amazonaws.com", call.signing_name, call.region);
        let mut request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{}", call.path))
            .header("host", host.as_str())
            .header("content-type", call.content_type)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{}/{}/aws4_request, \
                     SignedHeaders=host, Signature=not-verified",
                    call.region, call.signing_name
                ),
            );
        if let Some(target) = call.target.as_deref() {
            request = request.header("x-amz-target", target);
        }
        let request = request
            .body(bytes::Bytes::from(call.body))
            .expect("a well-formed request");
        let response = self.runtime.block_on(self.router.dispatch(request));
        let status = response.status().as_u16();
        // `restJson1` resolves an error shape from `x-amzn-errortype`; the `awsJson`
        // protocols render it as the body's `__type`. Either is this request's error code.
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let text = String::from_utf8_lossy(response.body()).to_string();
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let rendered = body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let rendered = if rendered.is_empty() {
            header_code
        } else {
            rendered
        };
        // Both channels may carry a trailing URL (`Code:http://…`) and a leading Smithy
        // namespace (`com.amazonaws.svc#Code`); the error code is what is left.
        let code = rendered
            .split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string();
        let message = body
            .get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Answer {
            status,
            code,
            message,
            body,
        }
    }

    /// One operation of an `awsJson1_1` service, named in `X-Amz-Target`.
    fn json_rpc(
        &self,
        signing_name: &str,
        region: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> Answer {
        self.send(Call {
            signing_name,
            region,
            path: "/",
            content_type: JSON_1_1,
            target: Some(format!("{target_prefix}.{operation}")),
            body: body.to_string(),
        })
    }

    /// One operation of a `restJson1` service, named by its own URI.
    fn rest_json(&self, signing_name: &str, region: &str, path: &str, body: &Value) -> Answer {
        self.send(Call {
            signing_name,
            region,
            path,
            content_type: REST_JSON,
            target: None,
            body: body.to_string(),
        })
    }
}

/// Assert one refusal is the whole triple the rule fixes: status, code and sentence.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str, message: &str) {
    assert_eq!(
        (answer.status, answer.code.as_str(), answer.message.as_str()),
        (status, code, message),
        "case {case:?}: expected ({status}, {code:?}, {message:?}), got \
         ({}, {:?}, {:?}) in {}",
        answer.status,
        answer.code,
        answer.message,
        answer.body
    );
}

/// Assert one request was SERVED, so the section's refusals are the rule and not the
/// service being unreachable.
fn assert_served(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the call to be served, got {} {:?} {:?}",
        answer.status, answer.code, answer.body
    );
}

// ---------------------------------------------------------------------------------
// Firehose: a control-plane lookup that resolved no delivery stream
// ---------------------------------------------------------------------------------

/// Every control-plane operation that resolves a `DeliveryStreamName`, with every OTHER
/// member of its request valid, so the lookup is what refuses it.
fn firehose_lookups(name: &str) -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "a describe",
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": name }),
        ),
        (
            "a delete",
            "DeleteDeliveryStream",
            json!({ "DeliveryStreamName": name }),
        ),
        (
            "a tag listing",
            "ListTagsForDeliveryStream",
            json!({ "DeliveryStreamName": name }),
        ),
        (
            "a tag write",
            "TagDeliveryStream",
            json!({
                "DeliveryStreamName": name,
                "Tags": [{ "Key": "owner", "Value": "team" }],
            }),
        ),
        (
            "a tag removal",
            "UntagDeliveryStream",
            json!({ "DeliveryStreamName": name, "TagKeys": ["owner"] }),
        ),
    ]
}

/// Names differing in every way a `DeliveryStreamName` can — its 1-character minimum and
/// its 64-character maximum (`smithy_query(query="shape", shape="DeliveryStreamName")`),
/// the characters it is spelled with, and its letter case. None of them occurs inside the
/// expected sentence, so the "never echoed" assertion means what it says.
fn absent_stream_names() -> Vec<String> {
    vec![
        "q".to_string(),
        "ghost-stream".to_string(),
        "ghost.stream".to_string(),
        "GHOST_1".to_string(),
        "z".repeat(64),
    ]
}

// ---------------------------------------------------------------------------------
// Amazon ECS: a page size outside the range its own member documents
// ---------------------------------------------------------------------------------

/// Page sizes on each side of the inclusive range `maxResults` documents for the daemon
/// task-definition listing ("This value can be between 1 and 100"), and sizes inside it:
/// each published bound, one past each bound, and a value far outside.
fn ecs_page_sizes() -> Vec<(&'static str, i64, bool)> {
    vec![
        ("one below the published minimum", 0, false),
        ("a negative page size", -1, false),
        ("one past the published maximum", 101, false),
        ("far past the published maximum", 10_000, false),
        ("the published minimum", 1, true),
        ("a page size inside the range", 50, true),
        ("the published maximum", 100, true),
    ]
}

// ---------------------------------------------------------------------------------
// X-Ray: an opaque retrieval token of each form a reader classifies
// ---------------------------------------------------------------------------------

/// The operations that take a `RetrievalToken`, each with the name its own refusal gives
/// the member in the outer clause (`smithy_query(query="operation",
/// operation="CancelTraceRetrieval,ListRetrievedTraces,GetRetrievedTracesGraph")`).
const TOKEN_READERS: [(&str, &str); 3] = [
    ("/CancelTraceRetrieval", "retrieval token"),
    ("/ListRetrievedTraces", "execution id"),
    ("/GetRetrievedTracesGraph", "execution id"),
];

/// The request body a reader needs to reach its token check: `TraceFormat` is a member of
/// `ListRetrievedTraces` alone, and that operation judges it first.
fn token_reader_body(path: &str, token: &str) -> Value {
    let mut body = json!({ "RetrievalToken": token });
    if path == "/ListRetrievedTraces" {
        body["TraceFormat"] = json!("XRAY");
    }
    body
}

/// The sentence a value the service could not have ISSUED is refused with: an outer clause
/// naming the member and the submitted value, then the clause of the decoder the value was
/// handed to — which names the value a second time only when the value was of the token
/// form and so reached the decoder proper.
fn malformed_token_message(member: &str, token: &str, of_the_token_form: bool) -> String {
    if of_the_token_form {
        format!("Invalid {member} {token} passed: Invalid execution id provided: {token}")
    } else {
        format!("Invalid {member} {token} passed: Invalid Execution Id")
    }
}

/// The sentence an INTACT token that names no stored retrieval is refused with.
fn retrieval_not_found_message(token: &str) -> String {
    format!("RetrievalToken `{token}` was not issued by this service, or its retrieval has timed out")
}

/// One test, five sections, ONE assembled router: the sections share a store on purpose —
/// the Account rows assert a sequence (a read, a refused start, the read after it) and the
/// ECR Public row reads back the write the row before it made — and assembling the router
/// is what this suite's cost is, so splitting the sections would either lose those
/// sequences or pay that cost once per section. Every assertion names its case and the
/// value it saw, so the failing row is identified without being the only row that ran.
#[test]
fn every_refusal_keeps_its_own_code_status_and_sentence_in_the_assembled_router() {
    std::thread::Builder::new()
        .name("assembled-router-refusal-identities".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let emulator = Emulator::new();
            let account = engine::emulator_account_id();

            // -------------------------------------------------------------------------
            // 1. Firehose. A control-plane lookup that resolved no stream is the modelled
            //    `ResourceNotFoundException` at HTTP 400, worded from the ACCOUNT the
            //    lookup was scoped to and never from the name it was given.
            // -------------------------------------------------------------------------
            let firehose_not_found = format!("Firehose under account {account} not found.");
            for name in absent_stream_names() {
                for (operation_case, operation, request) in firehose_lookups(&name) {
                    let case = format!("firehose {operation_case} of {name:?}");
                    let answer = emulator.json_rpc(
                        "firehose",
                        REGION,
                        "Firehose_20150804",
                        operation,
                        &request,
                    );
                    assert_refused(
                        &case,
                        &answer,
                        400,
                        "ResourceNotFoundException",
                        &firehose_not_found,
                    );
                    assert!(
                        !answer.message.contains(&name),
                        "case {case:?}: the probed name must not be echoed, got {:?}",
                        answer.message
                    );
                }
            }
            // The same service serves its listing, so the refusals above are the lookup.
            let listed = emulator.json_rpc(
                "firehose",
                REGION,
                "Firehose_20150804",
                "ListDeliveryStreams",
                &json!({}),
            );
            assert_served("firehose lists its streams", &listed);
            assert_eq!(
                listed.body["DeliveryStreamNames"],
                json!([]),
                "case \"firehose lists its streams\": an untouched account holds none, got {}",
                listed.body
            );

            // -------------------------------------------------------------------------
            // 2. Account Management. The in-flight email update a status read's own
            //    parameters assume is not a sub-resource a caller can address, so a
            //    report with no update to report is the modelled 400 `ValidationException`
            //    — not a 404 — while an account the caller may not act on is the modelled
            //    403 `AccessDeniedException`. Driven as the sequence a client runs: the
            //    self-targeted start is denied, and the status read after it still reports
            //    the request as one it cannot act on.
            // -------------------------------------------------------------------------
            // Read BEFORE any start is attempted: the sentence is the absent update.
            let never_started = emulator.rest_json(
                "account",
                "us-east-1",
                "/getPrimaryEmailUpdateStatus",
                &json!({}),
            );
            assert_refused(
                "account reads a status it never started",
                &never_started,
                400,
                "ValidationException",
                "No email update found for account",
            );
            let self_targeted = emulator.rest_json(
                "account",
                "us-east-1",
                "/startPrimaryEmailUpdate",
                &json!({ "AccountId": account, "PrimaryEmail": "owner@example.com" }),
            );
            assert_refused(
                "account starts an update targeting its own account",
                &self_targeted,
                403,
                "AccessDeniedException",
                "The operation failed because the management account can't specify its own \
                 AccountId; this operation acts on a member account of the organization",
            );
            // And AFTER the denied start, which stored nothing: the same sentence, because
            // a refused start leaves no update to report.
            let after_denied_start = emulator.rest_json(
                "account",
                "us-east-1",
                "/getPrimaryEmailUpdateStatus",
                &json!({}),
            );
            assert_refused(
                "account reads the status after a denied self-targeted start",
                &after_denied_start,
                400,
                "ValidationException",
                "No email update found for account",
            );
            // The same service serves its own reporting call, so the refusals in this
            // section are the classes the rule fixes and not a service that refuses
            // everything or is mounted under another's claim.
            let regions = emulator.rest_json("account", "us-east-1", "/listRegions", &json!({}));
            assert_served("account lists its Region opt-in statuses", &regions);
            assert!(
                regions.body["Regions"]
                    .as_array()
                    .is_some_and(|listed| !listed.is_empty()),
                "case \"account lists its Region opt-in statuses\": expected a non-empty \
                 Regions list, got {}",
                regions.body
            );
            // An account this emulator holds no record of is a different class again, so
            // the sentence above is the update's absence and not the account's. The id is
            // any well-formed one that is NOT the caller's own, which is what puts the read
            // on the other-account path rather than the self-target path above.
            let foreign_account = "123456789012";
            assert_ne!(
                foreign_account, account,
                "the other-account row needs an id that is not the caller's own"
            );
            let foreign = emulator.rest_json(
                "account",
                "us-east-1",
                "/getPrimaryEmailUpdateStatus",
                &json!({ "AccountId": foreign_account }),
            );
            assert_refused(
                "account reads a status of an account it holds no record of",
                &foreign,
                403,
                "AccessDeniedException",
                &format!(
                    "The operation failed because the calling identity doesn't have the \
                     minimum required permissions for account {foreign_account}"
                ),
            );

            // -------------------------------------------------------------------------
            // 3. Amazon ECS. A page size outside the inclusive range its own member
            //    documents is the modelled `InvalidParameterException` at HTTP 400,
            //    stating that range — and a page size inside it is served.
            // -------------------------------------------------------------------------
            let ecs_page_refusal = "MaxResults must be a positive number between 1-100.";
            for (size_case, max_results, inside) in ecs_page_sizes() {
                let case = format!("ecs lists daemon task definitions with {size_case}");
                let answer = emulator.json_rpc(
                    "ecs",
                    REGION,
                    "AmazonEC2ContainerServiceV20141113",
                    "ListDaemonTaskDefinitions",
                    &json!({ "maxResults": max_results }),
                );
                if inside {
                    assert_served(&case, &answer);
                    assert_eq!(
                        answer.body["daemonTaskDefinitions"],
                        json!([]),
                        "case {case:?}: an untouched account has registered none, got {}",
                        answer.body
                    );
                } else {
                    assert_refused(
                        &case,
                        &answer,
                        400,
                        "InvalidParameterException",
                        ecs_page_refusal,
                    );
                }
            }

            // -------------------------------------------------------------------------
            // 4. ECR Public. An account-global control action is performed in ONE Region
            //    of its surface, so a credential scope naming any other Region is the
            //    service-common `UnsupportedCommandException` at HTTP 400, naming the
            //    operation and the Region that does perform it.
            // -------------------------------------------------------------------------
            for region in ["us-west-2", "us-east-2", "eu-west-3", "us-gov-west-1"] {
                let case = format!("ecr-public writes its registry catalog from {region}");
                let answer = emulator.json_rpc(
                    "ecr-public",
                    region,
                    "SpencerFrontendService",
                    "PutRegistryCatalogData",
                    &json!({ "displayName": "ctl team-7 registry" }),
                );
                assert_refused(
                    &case,
                    &answer,
                    400,
                    "UnsupportedCommandException",
                    "PutRegistryCatalogData command is only supported in us-east-1.",
                );
            }
            let written = emulator.json_rpc(
                "ecr-public",
                "us-east-1",
                "SpencerFrontendService",
                "PutRegistryCatalogData",
                &json!({ "displayName": "ctl team-7 registry" }),
            );
            assert_served(
                "ecr-public writes its registry catalog from us-east-1",
                &written,
            );
            // The round trip: the accepted write is readable, so the refusals above left
            // the record alone and the served row stored something.
            let read_back = emulator.json_rpc(
                "ecr-public",
                "us-east-1",
                "SpencerFrontendService",
                "GetRegistryCatalogData",
                &json!({}),
            );
            assert_served("ecr-public reads its registry catalog back", &read_back);
            assert_eq!(
                read_back.body["registryCatalogData"]["displayName"],
                json!("ctl team-7 registry"),
                "case \"ecr-public reads its registry catalog back\": expected the name the \
                 accepted write stored, got {}",
                read_back.body
            );

            // -------------------------------------------------------------------------
            // 5. X-Ray. A `RetrievalToken` this service could not have ISSUED is a
            //    malformed parameter — `InvalidRequestException` at HTTP 400 — before any
            //    lookup, and the 404 `ResourceNotFoundException` is reserved for an INTACT
            //    token that names no stored retrieval. Which class a value falls in is
            //    decided by the value, never by what the store holds.
            // -------------------------------------------------------------------------
            // An intact token: an identifying half and the check half derived from it.
            let identity = "0123456789abcdef";
            let intact = format!("{identity}{}", retrieval_token_check(identity));
            // The same token with its last check digit changed — of the issued form, and
            // its check no longer verifies: an issued token damaged in transit.
            let damaged = {
                let mut digits: Vec<char> = intact.chars().collect();
                let last = digits.len() - 1;
                digits[last] = if digits[last] == '0' { '1' } else { '0' };
                digits.into_iter().collect::<String>()
            };
            // (case, token, whether it is of the issued token form)
            let malformed_tokens: Vec<(&str, String, bool)> = vec![
                (
                    "a value that is not of the token form at all",
                    "not-a-retrieval".to_string(),
                    false,
                ),
                (
                    // `RetrievalToken`'s modelled `@length` is 0..1020
                    // (`smithy_query(query="operation", operation="ListRetrievedTraces")`),
                    // so an empty value passes the boundary and reaches the classifier.
                    "an empty value",
                    String::new(),
                    false,
                ),
                (
                    "a value of the right width that is not hexadecimal",
                    "z".repeat(intact.len()),
                    false,
                ),
                (
                    "a hexadecimal value one digit short of the form",
                    intact[..intact.len() - 1].to_string(),
                    false,
                ),
                ("an issued token damaged in transit", damaged, true),
            ];
            for (path, member) in TOKEN_READERS {
                for (token_case, token, of_the_token_form) in &malformed_tokens {
                    let case = format!("xray {path} given {token_case}");
                    let answer = emulator.rest_json(
                        "xray",
                        REGION,
                        path,
                        &token_reader_body(path, token),
                    );
                    assert_refused(
                        &case,
                        &answer,
                        400,
                        "InvalidRequestException",
                        &malformed_token_message(member, token, *of_the_token_form),
                    );
                }
                // The intact token verifies, so it reaches the lookup — and the lookup is
                // what reports that no retrieval stands behind it.
                let case = format!("xray {path} given an intact token naming no retrieval");
                let answer =
                    emulator.rest_json("xray", REGION, path, &token_reader_body(path, &intact));
                assert_refused(
                    &case,
                    &answer,
                    404,
                    "ResourceNotFoundException",
                    &retrieval_not_found_message(&intact),
                );
            }
            // The service serves a retrieval it DID issue: the token it mints resolves on
            // every reader above, which is what makes the 404 rows the lookup reporting
            // nothing behind a token rather than the readers refusing whatever they get.
            let started = emulator.rest_json(
                "xray",
                REGION,
                "/StartTraceRetrieval",
                &json!({
                    // A 35-character trace id: the version, 8 hex digits and 24 hex digits.
                    "TraceIds": ["1-00000001-000000000000000000000001"],
                    "StartTime": 1_767_225_600,
                    "EndTime": 1_767_225_900,
                }),
            );
            assert_served("xray starts a retrieval", &started);
            let issued = started.body["RetrievalToken"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !issued.is_empty(),
                "case \"xray starts a retrieval\": expected a minted RetrievalToken, got {}",
                started.body
            );
            // Cancel last: it is the one reader that changes the retrieval it resolves,
            // and the change it makes is observable on the read that follows.
            for path in ["/ListRetrievedTraces", "/GetRetrievedTracesGraph"] {
                let case = format!("xray {path} given the token it issued");
                let answer =
                    emulator.rest_json("xray", REGION, path, &token_reader_body(path, &issued));
                assert_served(&case, &answer);
                let status = answer.body["RetrievalStatus"].as_str().unwrap_or_default();
                // The retrieval's status is a function of the instants the clock owes it,
                // and this suite does not drive the clock, so WHICH of the three
                // non-terminal-or-complete statuses is owed at this instant depends on how
                // long the request before it took: the set is the tightest assertion
                // available here. `trace_retrieval_lifecycle.rs` in the X-Ray crate pins
                // the exact status per offset, against a clock it states.
                assert!(
                    [
                        RETRIEVAL_STATUS_SCHEDULED,
                        RETRIEVAL_STATUS_RUNNING,
                        RETRIEVAL_STATUS_COMPLETE
                    ]
                    .contains(&status),
                    "case {case:?}: expected the retrieval to report a status it is owed, \
                     got {status:?} in {}",
                    answer.body
                );
            }
            let cancelled = emulator.rest_json(
                "xray",
                REGION,
                "/CancelTraceRetrieval",
                &token_reader_body("/CancelTraceRetrieval", &issued),
            );
            assert_served("xray cancels the retrieval it issued", &cancelled);
            let after_cancel = emulator.rest_json(
                "xray",
                REGION,
                "/ListRetrievedTraces",
                &token_reader_body("/ListRetrievedTraces", &issued),
            );
            assert_served("xray reads the cancelled retrieval back", &after_cancel);
            assert_eq!(
                after_cancel.body["RetrievalStatus"],
                json!(RETRIEVAL_STATUS_CANCELLED),
                "case \"xray reads the cancelled retrieval back\": expected \
                 {RETRIEVAL_STATUS_CANCELLED}, got {}",
                after_cancel.body
            );
        })
        .expect("spawning the sized thread")
        .join()
        .expect("the suite thread");
}
