// SPDX-License-Identifier: CC0-1.0

//! `ListCodegenJobs`, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! 1. The operation is `@paginated(items: entities)`: whatever `maxResults` it is given,
//!    following `nextToken` to the end enumerates every codegen job stored for the
//!    addressed (appId, environmentName) exactly once, in one stable order, and no page
//!    is longer than was asked for. An omitted `maxResults` takes the member's modelled
//!    `@default`.
//! 2. A page is a projection of the SAME records `GetCodegenJob` answers from: every
//!    member of `CodegenJobSummary` carries what the stored job carries, and only the
//!    jobs of the addressed app environment are reported.
//! 3. A token this operation never issued for that app environment names no page, so it
//!    is refused (HTTP 400) rather than served as a page.
//! 4. Listing is a report: it never moves a job's status, which only the clock owes.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The published bounds of `ListCodegenJobsLimit`, the shape
/// `ListCodegenJobsRequest$maxResults` targets:
/// `smithy_query(query="shape",shape="ListCodegenJobsLimit")` — `@range(min: 1, max:
/// 100)`. Spelled out here independently of the crate's own constants.
const MAX_RESULTS_FLOOR: i32 = 1;
const MAX_RESULTS_CEILING: i32 = 100;

/// The modelled `@default` of that member, from
/// `smithy_query(query="operation",operation="ListCodegenJobs")` — `"default": 100`.
const MAX_RESULTS_DEFAULT: i32 = 100;

/// Every member `CodegenJobSummary` declares
/// (`smithy_query(query="shape",shape="CodegenJobSummary")`): the three `@required` ones
/// and the two timestamps. The shape declares no `status`, so a summary carries none.
const SUMMARY_REQUIRED_MEMBERS: &[&str] = &["appId", "environmentName", "id"];
const SUMMARY_TIMESTAMP_MEMBERS: &[&str] = &["createdAt", "modifiedAt"];

/// The `CodegenJobStatus` the model declares for a job whose run has not ended, from
/// `smithy_query(query="shape",shape="CodegenJobStatus")` — `["in_progress", "failed",
/// "succeeded"]`.
const IN_PROGRESS: &str = "in_progress";

/// A wait shorter than the span the emulator takes to make a job's terminal status due, so
/// a job read after it is still the transient `in_progress`.
const WITHIN_THE_DUE_INSTANT: std::time::Duration = std::time::Duration::from_millis(20);

/// A wait longer than that span, so the clock has made the terminal status due.
const PAST_THE_DUE_INSTANT: std::time::Duration = std::time::Duration::from_millis(800);

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address, unless a case is about addressing
/// another one. `AppId` is `@length(min: 1, max: 20)` with pattern `^d[a-z0-9]+$`.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The error-type discriminator the body carries — `__type`, or the `code` / `Code`
    /// spellings the same JSON error parsers also accept — or the empty string when it
    /// carries none. AmplifyUIBuilder names no exception on the wire, so every refusal
    /// must carry none of them.
    fn error_code(&self) -> &str {
        ["__type", "code", "Code"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

fn jobs_uri(app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/codegen-jobs")
}

/// The smallest `StartCodegenJobData` the model admits: `renderConfig` is its only
/// `@required` member, and `CodegenJobRenderConfig` is a union with one variant.
fn minimal_data() -> Value {
    json!({"renderConfig": {"react": {}}})
}

/// Start a job in one app environment and return the id it was minted under.
async fn start_in(router: &MultiServiceRouter, app_id: &str, environment: &str) -> String {
    let answer = call(
        router,
        "POST",
        &jobs_uri(app_id, environment),
        Some(minimal_data()),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "starting a job in {app_id}/{environment} must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a started job carries an id, got {:?}", answer.body))
        .to_string()
}

async fn start(router: &MultiServiceRouter) -> String {
    start_in(router, APP_ID, ENVIRONMENT).await
}

async fn list_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    query: &str,
) -> Answer {
    let uri = format!("{}{query}", jobs_uri(app_id, environment));
    call(router, "GET", &uri, None).await
}

async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    list_in(router, APP_ID, ENVIRONMENT, query).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    let uri = format!("{}/{id}", jobs_uri(APP_ID, ENVIRONMENT));
    call(router, "GET", &uri, None).await
}

/// The `entities` a page carries, panicking when the answer is not a page.
fn entities(answer: &Answer) -> Vec<Value> {
    answer.body["entities"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "a page carries the @required entities list, got {} with {:?}",
                answer.status, answer.body
            )
        })
        .clone()
}

/// The ids a page reports, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    entities(answer)
        .iter()
        .map(|entity| {
            entity["id"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries an id, got {entity:?}"))
                .to_string()
        })
        .collect()
}

/// True when a JSON value is a `date-time` string, the wire form the model pins on
/// `CodegenJobSummary$createdAt` and `$modifiedAt`.
fn is_date_time(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| DateTime::from_str(text, Format::DateTime).is_ok())
}

/// Walk every page of one list with one `maxResults`, returning the ids in order and the
/// length of each page.
async fn walk(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    page_size: Option<i32>,
    stop_after: usize,
) -> (Vec<String>, Vec<usize>) {
    let mut walked = Vec::new();
    let mut lengths = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = String::new();
        if let Some(page_size) = page_size {
            query.push_str(&format!("?maxResults={page_size}"));
        }
        if let Some(token) = &token {
            query.push_str(if query.is_empty() { "?" } else { "&" });
            query.push_str(&format!("nextToken={token}"));
        }
        let answer = list_in(router, app_id, environment, &query).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={page_size:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let page = listed_ids(&answer);
        lengths.push(page.len());
        walked.extend(page);
        assert!(
            lengths.len() <= stop_after,
            "case maxResults={page_size:?}: pagination did not terminate, walked {walked:?}"
        );
        match answer.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    (walked, lengths)
}

/// Rule 1 and rule 2: whatever page size a caller asks for — including none, which takes
/// the modelled default — walking `nextToken` to the end enumerates exactly the jobs of
/// the addressed app environment, once each, in start order.
#[tokio::test]
async fn list_codegen_jobs_pages_over_one_stable_order() {
    let router = emulator();
    let mut started = Vec::new();
    for _ in 0..7 {
        started.push(start(&router).await);
    }
    // A job in another environment of the same app, and one in another app: neither
    // belongs to the listed environment.
    let other_environment = start_in(&router, APP_ID, "production").await;
    let other_app = start_in(&router, "d9999zzzz", ENVIRONMENT).await;

    // The rows differ along the page size the walk asks for: both published bounds, the
    // sizes between that cut the seven jobs into a different number of pages, and no
    // `maxResults` at all, which takes the member's modelled default.
    for page_size in [
        Some(MAX_RESULTS_FLOOR),
        Some(2),
        Some(3),
        Some(6),
        Some(7),
        Some(MAX_RESULTS_CEILING),
        None,
    ] {
        let (walked, lengths) =
            walk(&router, APP_ID, ENVIRONMENT, page_size, started.len() + 1).await;
        let asked = page_size.unwrap_or(MAX_RESULTS_DEFAULT);
        for length in &lengths {
            assert!(
                *length <= asked as usize,
                "case maxResults={page_size:?}: a page carried {length} jobs, more than the \
                 {asked} asked for"
            );
        }
        assert_eq!(
            walked, started,
            "case maxResults={page_size:?}: walking every page must enumerate every codegen \
             job of that app environment once, in start order, got {walked:?}"
        );
        assert!(
            !walked.contains(&other_environment) && !walked.contains(&other_app),
            "case maxResults={page_size:?}: only the addressed (appId, environmentName) is \
             reported, got {walked:?}"
        );
        assert_eq!(
            walked.iter().collect::<BTreeSet<_>>().len(),
            walked.len(),
            "case maxResults={page_size:?}: no job may be reported twice, got {walked:?}"
        );
    }

    // A page that ends the list carries no token.
    let whole = list(&router, &format!("?maxResults={MAX_RESULTS_CEILING}")).await;
    assert_eq!(
        whole.body["nextToken"],
        Value::Null,
        "the last page must omit nextToken, got {:?}",
        whole.body
    );

    // Each other app environment reports only its own job.
    for (app_id, environment, expected) in [
        (APP_ID, "production", &other_environment),
        ("d9999zzzz", ENVIRONMENT, &other_app),
    ] {
        let page = listed_ids(&list_in(&router, app_id, environment, "").await);
        assert_eq!(
            page,
            vec![expected.clone()],
            "case {app_id}/{environment}: an app environment reports its own jobs, got {page:?}"
        );
    }

    // An app environment nothing started in is an empty page, not a refusal.
    let empty = list_in(&router, "d0000quiet", "nothing", "").await;
    assert_eq!(
        empty.status, 200,
        "an app environment with no jobs must answer HTTP 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert!(
        entities(&empty).is_empty() && empty.body["nextToken"] == Value::Null,
        "an app environment with no jobs must report an empty list and no token, got {:?}",
        empty.body
    );

    // One past each published bound of `maxResults` is refused at the boundary.
    for page_size in [MAX_RESULTS_FLOOR - 1, MAX_RESULTS_CEILING + 1] {
        let refused = list(&router, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={page_size}: a value outside @range(1, 100) must be refused, got \
             {} with {:?}",
            refused.status, refused.body
        );
    }
}

/// Rule 3: a token this operation never issued for that app environment names no page.
/// The rows differ in HOW the token fails: a shape the service never emits, a correctly
/// shaped token issued while listing another app environment, and a correctly scoped
/// token whose cursor was never handed out — above the highest one and below the first.
#[tokio::test]
async fn list_codegen_jobs_refuses_a_token_it_never_issued() {
    let router = emulator();
    for _ in 0..3 {
        start(&router).await;
    }
    for _ in 0..2 {
        start_in(&router, APP_ID, "production").await;
    }

    let issued = list(&router, "?maxResults=1").await.body["nextToken"]
        .as_str()
        .expect("a first page of three jobs carries a nextToken")
        .to_string();
    let elsewhere = list_in(&router, APP_ID, "production", "?maxResults=1").await.body
        ["nextToken"]
        .as_str()
        .expect("a first page of two jobs carries a nextToken")
        .to_string();
    assert_ne!(
        issued, elsewhere,
        "a token is scoped to the app environment it was issued for, got {issued:?} for both"
    );

    let never_issued_cursor = format!("{}{}", &issued[..issued.len() - 16], "f".repeat(16));
    let cursor_below_the_first = format!("{}{}", &issued[..issued.len() - 16], "0".repeat(16));
    for token in [
        "not-a-token",
        "c-",
        "c-00000000000000zz",
        "0",
        &elsewhere,
        &never_issued_cursor,
        &cursor_below_the_first,
    ] {
        let refused = list(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case nextToken={token:?}: a token ListCodegenJobs never issued for this app \
             environment must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "",
            "case nextToken={token:?}: a refusal must name no error type, got {:?}",
            refused.body
        );
    }

    // The token it DID issue serves the rest of that list.
    let resumed = list(&router, &format!("?nextToken={issued}")).await;
    assert_eq!(
        resumed.status, 200,
        "case nextToken={issued:?}: a token this list issued must serve a page, got {} with {:?}",
        resumed.status, resumed.body
    );
    assert_eq!(
        listed_ids(&resumed).len(),
        2,
        "case nextToken={issued:?}: resuming after the first of three jobs must report the \
         other two, got {:?}",
        resumed.body
    );
}

/// Rule 2: every member of a summary is the one the stored job carries, so a page and
/// `GetCodegenJob` never disagree — and the summary carries only the members the model
/// declares.
#[tokio::test]
async fn list_codegen_jobs_reports_the_stored_job() {
    let router = emulator();
    let mut started = Vec::new();
    for _ in 0..3 {
        started.push(start(&router).await);
    }
    // Past the due instant every job holds the terminal status the clock owed it, so the
    // timestamps a page reports are settled and can be compared with the reader's.
    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;

    let page = entities(&list(&router, "").await);
    assert_eq!(
        page.len(),
        started.len(),
        "the page must carry one summary per started job, got {page:?}"
    );
    for (summary, id) in page.iter().zip(&started) {
        let job = get(&router, id).await;
        assert_eq!(
            job.status, 200,
            "case job {id}: GetCodegenJob must answer HTTP 200, got {} with {:?}",
            job.status, job.body
        );
        for member in SUMMARY_REQUIRED_MEMBERS {
            assert_eq!(
                summary[*member], job.body[*member],
                "case job {id}: the summary's {member} must be the stored job's, got {:?} \
                 against {:?}",
                summary[*member], job.body[*member]
            );
        }
        for member in SUMMARY_TIMESTAMP_MEMBERS {
            assert!(
                is_date_time(&summary[*member]),
                "case job {id}: {member} must be a date-time string, got {:?}",
                summary[*member]
            );
            assert_eq!(
                summary[*member], job.body[*member],
                "case job {id}: the summary's {member} must be the stored job's, got {:?} \
                 against {:?}",
                summary[*member], job.body[*member]
            );
        }
        let members: BTreeSet<&str> = summary
            .as_object()
            .unwrap_or_else(|| panic!("case job {id}: a summary is an object, got {summary:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        let declared: BTreeSet<&str> = SUMMARY_REQUIRED_MEMBERS
            .iter()
            .chain(SUMMARY_TIMESTAMP_MEMBERS)
            .copied()
            .collect();
        assert!(
            members.is_subset(&declared),
            "case job {id}: a summary carries only the members CodegenJobSummary declares, \
             got {members:?}"
        );
        assert_eq!(
            summary["appId"],
            json!(APP_ID),
            "case job {id}: the summary names the app it was started in, got {summary:?}"
        );
        assert_eq!(
            summary["environmentName"],
            json!(ENVIRONMENT),
            "case job {id}: the summary names the environment it was started in, got {summary:?}"
        );
    }
}

/// Rule 4: a list is a report. Reading a page of jobs the clock has not settled yet
/// leaves each one where it was, so a reader straight after still sees `in_progress`.
#[tokio::test]
async fn listing_codegen_jobs_does_not_move_a_status() {
    let router = emulator();
    let mut started = Vec::new();
    for _ in 0..2 {
        started.push(start(&router).await);
    }

    for round in 0..3 {
        let page = listed_ids(&list(&router, "").await);
        assert_eq!(
            page, started,
            "round {round}: a page must report every started job, got {page:?}"
        );
        for id in &started {
            let job = get(&router, id).await;
            assert_eq!(
                job.body["status"],
                json!(IN_PROGRESS),
                "round {round}, job {id}: listing must not advance a status the clock has not \
                 made due, got {:?}",
                job.body
            );
        }
        tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;
    }
}
