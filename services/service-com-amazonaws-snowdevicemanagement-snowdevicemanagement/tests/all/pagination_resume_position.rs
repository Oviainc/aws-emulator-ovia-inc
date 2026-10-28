// SPDX-License-Identifier: CC0-1.0

//! One reading of `nextToken`, shared by every paginated operation this service models.
//!
//! The rule under test: a request that names no `nextToken` is served from the start of
//! the enumeration, a request whose `nextToken` is a cursor THIS enumeration issued
//! resumes exactly where the previous page stopped, and any other value is refused with
//! the modelled `ValidationException` (HTTP 400) worded the same way for every
//! paginated operation, naming the operation that would have issued the cursor
//! (`smithy_query(query="operation",operation="ListDevices,ListTasks,ListExecutions,ListDeviceResources")`
//! — all four take `nextToken`/`maxResults` and model `ValidationException`).
//!
//! The table's rows are the four paginated operations, each reached over HTTP, so the
//! one branch they now share is driven once per operation: the absent-token branch, the
//! issued-cursor branch and the refusal branch. Nothing reads the state struct for an
//! answer, and the enumerations are compared before and after the refusals, so a page
//! that is not really resumed — or a refusal that quietly consumed a position — fails
//! here.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region this suite signs every request for.
const REGION: &str = "us-west-2";

/// How many devices are activated in [`REGION`]: enough for a device list with pages to
/// cut, a task over two devices (so its execution list has more than one item) and a
/// task over one.
const DEVICES: u64 = 3;

/// One emulator: a router carrying only this service over one shared engine, with an
/// activated inventory in [`REGION`].
///
/// The API models no device-creation operation, so the inventory goes in through the one
/// state seam that activates a device.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state =
            service_state(&*engine).expect("the in-memory engine carries this service's state");
        state
            .lock()
            .activate_devices(REGION, DEVICES, SystemTime::now());
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message, or the empty string on a success.
    fn error_message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// Dispatch one request signed for [`REGION`].
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "host",
            format!("snow-device-management.{REGION}.amazonaws.com"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/\
                 snow-device-management/aws4_request, SignedHeaders=host, Signature=abc"
            ),
        );
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

/// One paginated enumeration: a row of the table below.
#[derive(Debug, Clone)]
struct Paginated {
    /// The operation's name, which is what its refusal of a foreign cursor names.
    operation: &'static str,
    /// The operation's HTTP path, resource label included.
    path: String,
    /// The narrowing query parameters, already joined (`""` when the request names none).
    filters: String,
    /// The output member holding the page's items.
    items: &'static str,
}

impl Paginated {
    /// The URI for one page of this enumeration.
    fn uri(&self, max_results: Option<i32>, next_token: Option<&str>) -> String {
        let mut query: Vec<String> = Vec::new();
        if !self.filters.is_empty() {
            query.push(self.filters.clone());
        }
        if let Some(max_results) = max_results {
            query.push(format!("maxResults={max_results}"));
        }
        if let Some(next_token) = next_token {
            query.push(format!("nextToken={next_token}"));
        }
        if query.is_empty() {
            self.path.clone()
        } else {
            format!("{}?{}", self.path, query.join("&"))
        }
    }
}

/// Ask for one page of `listing`.
async fn page(
    router: &MultiServiceRouter,
    listing: &Paginated,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    call(router, "GET", &listing.uri(max_results, next_token), None).await
}

/// The ids a page reports, in the order it reports them, for whatever member identifies
/// an item of this enumeration.
fn item_keys(answer: &Answer, listing: &Paginated, case: &str) -> Vec<String> {
    answer.body[listing.items]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {} array, got {}",
                listing.items, answer.body
            )
        })
        .iter()
        .map(|item| {
            for member in ["managedDeviceId", "taskId", "id"] {
                if let Some(value) = item[member].as_str() {
                    return value.to_string();
                }
            }
            panic!("case {case:?}: expected an identified item, got {item}")
        })
        .collect()
}

/// A page the operation served, or a panic naming the case.
fn served(answer: &Answer, listing: &Paginated, case: &str) -> Vec<String> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    item_keys(answer, listing, case)
}

/// The ids of the devices activated in [`REGION`], in the order `ListDevices` serves
/// them.
async fn devices(router: &MultiServiceRouter) -> Vec<String> {
    let listing = devices_listing();
    let answer = page(router, &listing, None, None).await;
    served(&answer, &listing, "the activated inventory")
}

fn devices_listing() -> Paginated {
    Paginated {
        operation: "ListDevices",
        path: "/managed-devices".to_string(),
        filters: String::new(),
        items: "devices",
    }
}

/// Create one task over `targets`, answering its `taskId`.
async fn queue_task(router: &MultiServiceRouter, targets: &[String]) -> String {
    let case = format!("CreateTask over {targets:?}");
    let answer = call(
        router,
        "POST",
        "/task",
        Some(json!({ "targets": targets, "command": { "unlock": {} } })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a taskId, got {}", answer.body))
        .to_string()
}

/// The four paginated enumerations, over an inventory that gives each of them more than
/// one item, so each has a page to cut and a cursor to replay.
async fn listings(router: &MultiServiceRouter) -> Vec<Paginated> {
    let devices = devices(router).await;
    assert_eq!(
        devices.len(),
        usize::try_from(DEVICES).expect("a small count fits in usize"),
        "case \"fixture\": the Region holds its activated inventory, got {devices:?}"
    );
    let over_two = queue_task(router, &devices[0..2]).await;
    let _over_one = queue_task(router, &devices[2..3]).await;
    vec![
        devices_listing(),
        Paginated {
            operation: "ListTasks",
            path: "/tasks".to_string(),
            filters: String::new(),
            items: "tasks",
        },
        Paginated {
            operation: "ListExecutions",
            path: "/executions".to_string(),
            filters: format!("taskId={over_two}"),
            items: "executions",
        },
        Paginated {
            operation: "ListDeviceResources",
            path: format!("/managed-device/{}/resources", devices[0]),
            filters: String::new(),
            items: "resources",
        },
    ]
}

/// Walk `listing` one item at a time, following the cursor each page hands back, and
/// answer the ids in the order the walk saw them.
async fn walk_one_at_a_time(router: &MultiServiceRouter, listing: &Paginated) -> Vec<String> {
    let case = format!("{} walked one item per page", listing.operation);
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // A bound on the walk, so a cursor that never advances fails instead of hanging.
    for _ in 0..64 {
        let answer = page(router, listing, Some(1), token.as_deref()).await;
        let keys = served(&answer, listing, &case);
        assert!(
            keys.len() <= 1,
            "case {case:?}: a page of one holds at most one item, got {keys:?}"
        );
        seen.extend(keys);
        match answer.body["nextToken"].as_str() {
            None => return seen,
            Some(next) => token = Some(next.to_string()),
        }
    }
    panic!("case {case:?}: the walk did not finish within 64 pages, saw {seen:?}");
}

#[tokio::test]
async fn every_paginated_operation_reads_next_token_the_same_way() {
    let router = emulator();
    let listings = listings(&router).await;

    for listing in &listings {
        let operation = listing.operation;

        // The absent-token branch: no `nextToken` starts at the beginning of the
        // enumeration, which is what the resumed walk below is compared against.
        let case = format!("{operation} with no nextToken");
        let whole = served(&page(&router, listing, None, None).await, listing, &case);
        assert!(
            whole.len() > 1,
            "case {case:?}: this fixture gives the enumeration more than one item, so it \
             has a page to cut, got {whole:?}"
        );

        // The issued-cursor branch: every page but the first resumes on a cursor the
        // previous one handed back, and the walk sees the whole enumeration, in order,
        // each item exactly once.
        let resumed = walk_one_at_a_time(&router, listing).await;
        assert_eq!(
            resumed, whole,
            "case {case:?}: resuming on each issued cursor must walk the same \
             enumeration as one unpaginated page, got {resumed:?}"
        );
    }

    // The refusal branch: a value no page of the enumeration handed back. The rows
    // differ along the shape of the value — a cursor of a sibling operation, a cursor
    // of the same operation damaged, and values that are no cursor at all — and every
    // one of them is refused by every operation with the same wording.
    for listing in &listings {
        let operation = listing.operation;
        let before = served(
            &page(&router, listing, None, None).await,
            listing,
            &format!("{operation} before the refusals"),
        );

        let own = page(&router, listing, Some(1), None).await.body["nextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {operation:?}: a cut page hands back a cursor"))
            .to_string();
        let mut rows: Vec<(String, String)> = vec![
            ("hex of a word".to_string(), ::hex::encode("resume")),
            ("not hex at all".to_string(), "zzzz".to_string()),
            (
                "its own cursor, truncated".to_string(),
                own[..own.len() - 2].to_string(),
            ),
            ("its own cursor, extended".to_string(), format!("{own}00")),
        ];
        for sibling in &listings {
            if sibling.operation == operation {
                continue;
            }
            let sibling_cursor = page(&router, sibling, Some(1), None).await.body["nextToken"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {:?}: a cut page hands back a cursor", sibling.operation)
                })
                .to_string();
            rows.push((
                format!("a cursor {} issued", sibling.operation),
                sibling_cursor,
            ));
        }

        for (shape, token) in &rows {
            let case = format!("{operation} replaying {shape}");
            let answer = page(&router, listing, Some(1), Some(token)).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a nextToken this enumeration never issued is a \
                 request-shape problem, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {}",
                answer.body
            );
            // One wording for all four operations, differing only in the operation the
            // cursor would have come from.
            assert_eq!(
                answer.error_message(),
                format!(
                    "nextToken is not a pagination token {operation} issued for this request"
                ),
                "case {case:?}: expected the shared refusal wording, got {:?}",
                answer.error_message()
            );
        }

        let case = format!("{operation} after the refusals");
        assert_eq!(
            served(&page(&router, listing, None, None).await, listing, &case),
            before,
            "case {case:?}: a refused page must not change what a later caller sees"
        );
    }
}
