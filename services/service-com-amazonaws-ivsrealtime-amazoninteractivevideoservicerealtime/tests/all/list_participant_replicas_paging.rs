// SPDX-License-Identifier: CC0-1.0

//! `ListParticipantReplicas` as a PAGE over persisted state: how many replicas one call
//! reports, which replicas they are, and when the request is refused.
//!
//! `tests/all/participant_replication.rs` pins the round trip of one start and one page of
//! two. The rules here are the ones a listing holds to for EVERY page size and EVERY
//! participant, each exercised over a set of inputs rather than on one request:
//!
//! 1. PAGE SIZE. A page holds the first `min(maxResults, replicas still to report)`
//!    replicas of the named participant in start order, and carries a `nextToken` exactly
//!    when replicas remain. A request that omits `maxResults` pages by the documented
//!    default, which is pinned on one replica MORE than that default so no other default
//!    would pass. A `nextToken` names a POSITION, not a page size, so the next call resumes
//!    after the replica its page ended at whatever `maxResults` it then asks for.
//! 2. SCOPE. A page reports the replicas of the named participant in the named source
//!    stage and no others: not a sibling participant's of the same stage, and not the same
//!    participant's of another stage.
//! 3. REFUSAL. `maxResults` is refused outside its modelled range and accepted on both
//!    bounds, and a `nextToken` naming no position in THIS listing is refused, each as the
//!    modelled `ValidationException` (HTTP 400).
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// `MaxParticipantReplicaResults`' modelled `@range`, from
/// `smithy_query(query="operation",operation="ListParticipantReplicas")`: `1..50`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 50;

/// Page size a request that omits `maxResults` gets, from
/// `ListParticipantReplicasRequest$maxResults`: "Maximum number of results to return.
/// Default: 50."
const DEFAULT_MAX_RESULTS: usize = 50;

/// Replicas the paging fixture starts for one publisher — more than one page at every page
/// size the table below asks for, and fewer than one page at the modelled maximum.
const FIXTURE_REPLICAS: usize = 4;

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs` in `us-east-1`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00";
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.us-east-1.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A stage, created the way a client creates one, reported by its ARN.
    async fn stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name:?} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// A publisher of `stage_arn`: a participant of the token the model says a
    /// `participantId` comes from, with the default capabilities (`PUBLISH, SUBSCRIBE`).
    async fn publisher(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self
            .call(
                "CreateParticipantToken",
                &json!({ "stageArn": stage_arn }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken for {stage_arn} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }

    /// Replicate `participant_id` of `source` into `destination`, asserting it was
    /// accepted.
    async fn replicate(&self, case: &str, source: &str, participant_id: &str, destination: &str) {
        let request = json!({
            "sourceStageArn": source,
            "destinationStageArn": destination,
            "participantId": participant_id,
        });
        let (status, response) = self.call("StartParticipantReplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// One `ListParticipantReplicas` page, with any extra request members (`maxResults`,
    /// `nextToken`) merged in. Answers the status and body unchanged.
    async fn list(
        &self,
        source_stage_arn: &str,
        participant_id: &str,
        extra: &Value,
    ) -> (u16, Value) {
        let mut request = json!({
            "sourceStageArn": source_stage_arn,
            "participantId": participant_id,
        });
        if let Some(extra) = extra.as_object() {
            let target = request
                .as_object_mut()
                .expect("the request under construction is an object");
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        self.call("ListParticipantReplicas", &request).await
    }

    /// One page, asserting it was served.
    async fn page(
        &self,
        case: &str,
        source_stage_arn: &str,
        participant_id: &str,
        extra: &Value,
    ) -> Value {
        let (status, response) = self.list(source_stage_arn, participant_id, extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipantReplicas with {extra} must answer HTTP 200, \
             got {status} {response}"
        );
        response
    }
}

/// A string member, read as the empty string when absent — what a client sees for a member
/// the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// The destination stage of every replica of one page, in the order the page reports them.
fn destinations(case: &str, page: &Value) -> Vec<String> {
    page["replicas"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: replicas must be a list, got {page}"))
        .iter()
        .map(|replica| member(replica, "destinationStageArn"))
        .collect()
}

/// An IVS stage ARN in `us-east-1` naming the stage id `id`, in the form the model's
/// `StageArn` pattern requires.
fn stage_arn(id: &str) -> String {
    format!("arn:aws:ivs:us-east-1:{ACCOUNT}:stage/{id}")
}

/// The page sizes `total` records must be reported in when each page asks for at most
/// `page_size`: full pages until fewer than `page_size` are left, then the remainder.
fn expected_page_sizes(total: usize, page_size: usize) -> Vec<usize> {
    let mut sizes = Vec::new();
    let mut left = total;
    while left > page_size {
        sizes.push(page_size);
        left -= page_size;
    }
    sizes.push(left);
    sizes
}

/// Walk every page of one participant's replicas at `max_results` (`None` requests no page
/// size), answering the page sizes seen and the destinations in report order.
async fn walk(
    emulator: &RealTime,
    case: &str,
    source: &str,
    participant: &str,
    max_results: Option<i64>,
) -> (Vec<usize>, Vec<String>) {
    let mut sizes = Vec::new();
    let mut seen = Vec::new();
    let mut next_token: Option<String> = None;
    // One call per record plus one, so a listing that never ends fails the case rather
    // than hanging it.
    for _ in 0..=FIXTURE_REPLICAS + 1 {
        let mut extra = json!({});
        if let Some(max_results) = max_results {
            extra["maxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            extra["nextToken"] = json!(token);
        }
        let page = emulator.page(case, source, participant, &extra).await;
        let listed = destinations(case, &page);
        sizes.push(listed.len());
        seen.extend(listed);
        next_token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return (sizes, seen);
        }
    }
    panic!("case {case:?}: paging at {max_results:?} did not end after {sizes:?}");
}

// ---------------------------------------------------------------------------
// Rule 1: page size
// ---------------------------------------------------------------------------

/// The default is the documented one, not merely "enough": with ONE MORE replica than the
/// default, a request that sizes no page reports exactly the default and carries a token,
/// and the leftover replica follows it. A smaller default would end the first page early
/// and a larger one would swallow the leftover, so only the documented value passes.
#[tokio::test]
async fn a_request_that_sizes_no_page_pages_by_the_documented_default() {
    let emulator = RealTime::new();
    let case = "one replica more than the documented default";
    let source = emulator.stage(case, "source").await;
    let participant = emulator.publisher(case, &source).await;
    let total = DEFAULT_MAX_RESULTS + 1;
    let mut started = Vec::new();
    for index in 0..total {
        let destination = emulator.stage(case, &format!("destination-{index}")).await;
        emulator
            .replicate(case, &source, &participant, &destination)
            .await;
        started.push(destination);
    }

    let first = emulator.page(case, &source, &participant, &json!({})).await;
    let listed = destinations(case, &first);
    assert_eq!(
        listed.len(),
        DEFAULT_MAX_RESULTS,
        "case {case:?}: a request that sizes no page must report {DEFAULT_MAX_RESULTS} of \
         {total} replicas, got {} of them",
        listed.len()
    );
    assert_eq!(
        listed,
        started[..DEFAULT_MAX_RESULTS].to_vec(),
        "case {case:?}: the default page must be the first {DEFAULT_MAX_RESULTS} replicas \
         started, got {listed:?}"
    );
    let token = member(&first, "nextToken");
    assert!(
        !token.is_empty(),
        "case {case:?}: a default page with a replica left over must carry a nextToken, \
         got {first}"
    );

    let second = emulator
        .page(case, &source, &participant, &json!({ "nextToken": token }))
        .await;
    assert_eq!(
        destinations(case, &second),
        started[DEFAULT_MAX_RESULTS..].to_vec(),
        "case {case:?}: the next default page must report the leftover replica, got {second}"
    );
    assert!(
        second.get("nextToken").is_none(),
        "case {case:?}: the last page must carry no nextToken, got {second}"
    );
}

/// Every page size, over one publisher's replicas: each page holds what was asked for
/// until the records run out, carries a `nextToken` only while replicas remain, and the
/// pages together report every replica exactly once in start order.
#[tokio::test]
async fn a_page_holds_the_replicas_asked_for_at_every_page_size() {
    let emulator = RealTime::new();
    let case = "four replicas of one publisher";
    let source = emulator.stage(case, "source").await;
    let participant = emulator.publisher(case, &source).await;
    let mut started = Vec::new();
    for index in 0..FIXTURE_REPLICAS {
        let destination = emulator.stage(case, &format!("destination-{index}")).await;
        emulator
            .replicate(case, &source, &participant, &destination)
            .await;
        started.push(destination);
    }

    // The rows differ along what the rule generalises over: the page size asked for,
    // including both modelled bounds and the absence of the member (whose value is pinned
    // on its own boundary by the test above; here it is one page over these four).
    let rows: Vec<(String, Option<i64>, usize)> = vec![
        (
            "no maxResults, so the documented default".to_string(),
            None,
            DEFAULT_MAX_RESULTS,
        ),
        (
            format!("maxResults at the modelled minimum {MAX_RESULTS_MIN}"),
            Some(MAX_RESULTS_MIN),
            MAX_RESULTS_MIN as usize,
        ),
        ("maxResults of 2".to_string(), Some(2), 2),
        (
            "maxResults one below the record count".to_string(),
            Some(FIXTURE_REPLICAS as i64 - 1),
            FIXTURE_REPLICAS - 1,
        ),
        (
            "maxResults exactly the record count".to_string(),
            Some(FIXTURE_REPLICAS as i64),
            FIXTURE_REPLICAS,
        ),
        (
            format!("maxResults at the modelled maximum {MAX_RESULTS_MAX}"),
            Some(MAX_RESULTS_MAX),
            MAX_RESULTS_MAX as usize,
        ),
    ];

    for (case, max_results, page_size) in rows {
        let case = case.as_str();
        let (sizes, seen) = walk(&emulator, case, &source, &participant, max_results).await;
        let expected = expected_page_sizes(FIXTURE_REPLICAS, page_size);
        assert_eq!(
            sizes, expected,
            "case {case:?}: {FIXTURE_REPLICAS} replicas at a page size of {page_size} must \
             page {expected:?}, got {sizes:?}"
        );
        assert_eq!(
            seen, started,
            "case {case:?}: paging must report every replica once, in start order, got {seen:?}"
        );
    }
}

/// A `nextToken` names the position a page ended at, not the size of the page that issued
/// it: resuming with any `maxResults` reports the replicas after that position and no
/// others.
#[tokio::test]
async fn a_next_token_resumes_the_position_whatever_the_next_page_asks_for() {
    let emulator = RealTime::new();
    let case = "resume after the first replica";
    let source = emulator.stage(case, "source").await;
    let participant = emulator.publisher(case, &source).await;
    let mut started = Vec::new();
    for index in 0..FIXTURE_REPLICAS {
        let destination = emulator.stage(case, &format!("destination-{index}")).await;
        emulator
            .replicate(case, &source, &participant, &destination)
            .await;
        started.push(destination);
    }

    let first = emulator
        .page(case, &source, &participant, &json!({ "maxResults": 1 }))
        .await;
    assert_eq!(
        destinations(case, &first),
        started[..1].to_vec(),
        "case {case:?}: a page of one must report the first replica started, got {first}"
    );
    let token = member(&first, "nextToken");
    assert!(
        !token.is_empty(),
        "case {case:?}: a page with replicas left over must carry a nextToken, got {first}"
    );

    // Each row resumes from the SAME token with a different page size: what follows the
    // position is fixed, only how much of it one call reports changes.
    let resumptions: Vec<(String, i64, usize)> = (MAX_RESULTS_MIN..=FIXTURE_REPLICAS as i64)
        .map(|max_results| {
            let expected = (FIXTURE_REPLICAS - 1).min(max_results as usize);
            (
                format!("resumed with maxResults {max_results}"),
                max_results,
                expected,
            )
        })
        .collect();
    for (case, max_results, expected) in resumptions {
        let case = case.as_str();
        let page = emulator
            .page(
                case,
                &source,
                &participant,
                &json!({ "maxResults": max_results, "nextToken": token }),
            )
            .await;
        let listed = destinations(case, &page);
        assert_eq!(
            listed,
            started[1..1 + expected].to_vec(),
            "case {case:?}: the page must report the {expected} replicas after the first, \
             got {page}"
        );
        let has_token = page.get("nextToken").is_some();
        assert_eq!(
            has_token,
            1 + expected < FIXTURE_REPLICAS,
            "case {case:?}: a nextToken must be carried exactly while replicas remain, \
             got {page}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: scope
// ---------------------------------------------------------------------------

/// A page is scoped to the participant and the source stage it names: a sibling
/// participant's replicas of the same stage, and another stage's replicas, are not in it.
#[tokio::test]
async fn a_page_reports_only_the_named_participant_of_the_named_stage() {
    let emulator = RealTime::new();
    let case = "two publishers of one stage, and a second stage";
    let first_stage = emulator.stage(case, "first-source").await;
    let second_stage = emulator.stage(case, "second-source").await;
    let busy = emulator.publisher(case, &first_stage).await;
    let quiet = emulator.publisher(case, &first_stage).await;
    let elsewhere = emulator.publisher(case, &second_stage).await;

    let mut busy_destinations = Vec::new();
    for index in 0..2 {
        let destination = emulator.stage(case, &format!("busy-{index}")).await;
        emulator
            .replicate(case, &first_stage, &busy, &destination)
            .await;
        busy_destinations.push(destination);
    }
    let quiet_destination = emulator.stage(case, "quiet-0").await;
    emulator
        .replicate(case, &first_stage, &quiet, &quiet_destination)
        .await;
    let elsewhere_destination = emulator.stage(case, "elsewhere-0").await;
    emulator
        .replicate(case, &second_stage, &elsewhere, &elsewhere_destination)
        .await;

    let absent_stage = stage_arn("absent-stage");
    let rows: Vec<(&str, &str, &str, Vec<String>)> = vec![
        (
            "the publisher with two replicas",
            first_stage.as_str(),
            busy.as_str(),
            busy_destinations.clone(),
        ),
        (
            "its sibling in the same stage",
            first_stage.as_str(),
            quiet.as_str(),
            vec![quiet_destination.clone()],
        ),
        (
            "a publisher of the other stage",
            second_stage.as_str(),
            elsewhere.as_str(),
            vec![elsewhere_destination.clone()],
        ),
        (
            "a participant of another stage, asked of this one",
            first_stage.as_str(),
            elsewhere.as_str(),
            Vec::new(),
        ),
        (
            "a replicated publisher, asked of the other stage",
            second_stage.as_str(),
            busy.as_str(),
            Vec::new(),
        ),
        (
            "a stage this store does not hold",
            absent_stage.as_str(),
            busy.as_str(),
            Vec::new(),
        ),
    ];

    for (case, source, participant, expected) in rows {
        let page = emulator.page(case, source, participant, &json!({})).await;
        let listed = destinations(case, &page);
        assert_eq!(
            listed, expected,
            "case {case:?}: the page must report exactly {expected:?}, got {page}"
        );
        assert!(
            page.get("nextToken").is_none(),
            "case {case:?}: a complete page carries no nextToken, got {page}"
        );
        for replica in page["replicas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: replicas must be a list, got {page}"))
        {
            assert_eq!(
                member(replica, "sourceStageArn"),
                source,
                "case {case:?}: every replica must name the source stage asked for, got {page}"
            );
            assert_eq!(
                member(replica, "participantId"),
                participant,
                "case {case:?}: every replica must name the participant asked for, got {page}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: refusal
// ---------------------------------------------------------------------------

/// A `nextToken` names a position in THIS listing: one this listing never issued names
/// none, so it is refused rather than read as "start from the beginning", whatever shape
/// it takes. The absent member and the empty string are the two ways to ask for no
/// position at all, and both start the listing.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let emulator = RealTime::new();
    let case = "one replica, listed with every shape of token";
    let source = emulator.stage(case, "source").await;
    let destination = emulator.stage(case, "destination").await;
    let participant = emulator.publisher(case, &source).await;
    emulator
        .replicate(case, &source, &participant, &destination)
        .await;

    // A token another listing of this service issued: a position, but not one in this
    // listing. `maxResults` of 1 over the stages created above leaves replicas behind, so
    // the page carries one.
    let (status, stages) = emulator
        .call("ListStages", &json!({ "maxResults": 1 }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListStages must answer HTTP 200, got {status} {stages}"
    );
    let foreign = member(&stages, "nextToken");
    assert!(
        !foreign.is_empty(),
        "case {case:?}: the fixture needs a token from the other listing, got {stages}"
    );
    // `PaginationToken`'s modelled `@length` maximum, from the same query: 1024.
    let longest = "a".repeat(1024);
    // The rows differ along what the rule generalises over: where the offered token came
    // from and what it is made of, each within the modelled `PaginationToken` constraints
    // (length 0..1024, pattern ^[a-zA-Z0-9+/=_-]*$).
    let rows: Vec<(&str, &str, u16)> = vec![
        ("the empty string, which names no position", "", 200),
        ("a token from another listing of this service", &foreign, 400),
        ("a word that is no token at all", "not-a-token", 400),
        (
            "base64 of a position under no listing's marker",
            // `aGVsbG8tNw==` is "hello-7": the shape of a token, under a marker no
            // listing of this service issues.
            "aGVsbG8tNw==",
            400,
        ),
        (
            "a token at the modelled maximum length",
            &longest,
            400,
        ),
    ];

    for (case, token, expected_status) in rows {
        let (status, response) = emulator
            .list(&source, &participant, &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: a nextToken of {token:?} must answer HTTP {expected_status}, \
             got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: the refusal must carry the code ValidationException, \
                 got {response}"
            );
            assert!(
                response.get("replicas").is_none(),
                "case {case:?}: a refused page must report no replicas, got {response}"
            );
        } else {
            assert_eq!(
                destinations(case, &response),
                vec![destination.clone()],
                "case {case:?}: a request naming no position must start the listing, \
                 got {response}"
            );
        }
    }
}

/// `maxResults` is accepted on both modelled bounds and refused outside them, as the
/// modelled `ValidationException` (HTTP 400) — and a refused page reports nothing.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let emulator = RealTime::new();
    let case = "one replica, asked for at every page size";
    let source = emulator.stage(case, "source").await;
    let destination = emulator.stage(case, "destination").await;
    let participant = emulator.publisher(case, &source).await;
    emulator
        .replicate(case, &source, &participant, &destination)
        .await;

    // Both published bounds, one step inside each, and one step past each.
    let rows: Vec<(String, i64, u16)> = vec![
        (
            format!("one below the modelled minimum {MAX_RESULTS_MIN}"),
            MAX_RESULTS_MIN - 1,
            400,
        ),
        (
            format!("the modelled minimum {MAX_RESULTS_MIN}"),
            MAX_RESULTS_MIN,
            200,
        ),
        (
            format!("one above the modelled minimum {MAX_RESULTS_MIN}"),
            MAX_RESULTS_MIN + 1,
            200,
        ),
        (
            format!("one below the modelled maximum {MAX_RESULTS_MAX}"),
            MAX_RESULTS_MAX - 1,
            200,
        ),
        (
            format!("the modelled maximum {MAX_RESULTS_MAX}"),
            MAX_RESULTS_MAX,
            200,
        ),
        (
            format!("one above the modelled maximum {MAX_RESULTS_MAX}"),
            MAX_RESULTS_MAX + 1,
            400,
        ),
    ];

    for (case, max_results, expected_status) in rows {
        let case = case.as_str();
        let (status, response) = emulator
            .list(&source, &participant, &json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: maxResults {max_results} must answer HTTP {expected_status}, \
             got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: maxResults {max_results} must be refused as \
                 ValidationException, got {response}"
            );
            assert!(
                response.get("replicas").is_none(),
                "case {case:?}: a refused page must report no replicas, got {response}"
            );
        } else {
            assert_eq!(
                destinations(case, &response),
                vec![destination.clone()],
                "case {case:?}: an accepted page must report the one replica, got {response}"
            );
        }
    }

    // The refusals changed nothing a later caller sees.
    let after = emulator.page(case, &source, &participant, &json!({})).await;
    assert_eq!(
        destinations(case, &after),
        vec![destination.clone()],
        "case {case:?}: the one replica must still be reported after the refusals, got {after}"
    );
}
