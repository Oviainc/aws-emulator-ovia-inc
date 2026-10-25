// SPDX-License-Identifier: CC0-1.0

//! What a `nextToken` is, over HTTP: a cursor into ONE enumeration, issued by this
//! service, and a position in nothing else.
//!
//! The rule under test, for every paginated operation this service models
//! (`ListDevices`, `ListTasks`, `ListExecutions`, `ListDeviceResources`): a page is
//! resumed only by a token a previous page of the SAME enumeration handed back — the
//! same operation, the same signed Region and the same narrowing members (the filters,
//! and the resource a sub-list hangs off) — and every other value of `nextToken` is
//! refused with the modelled `ValidationException` (HTTP 400) instead of being read as
//! a sort key. All four operations model it, with `nextToken` constrained
//! `@length(1..1024) @pattern("[a-zA-Z0-9+/=]*")`
//! (`smithy_query(query="operation",operation="ListDevices,ListTasks,ListExecutions")`).
//!
//! Every case is a TABLE: the rows differ along what the rule generalises over — which
//! operation issued the cursor, which one replays it, which Region, which filter value,
//! and the shape of a value no call issued — never a single recorded request. Nothing
//! reaches into the state struct to read an answer, so a cursor that is not really
//! checked cannot pass.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    device_instance_ids, managed_device_id, ordering_job_id, service_state,
};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region most of this suite signs for.
const HOME_REGION: &str = "us-west-2";

/// A second Region with its own activated inventory, so a cursor can be replayed
/// somewhere its enumeration does not exist.
const OTHER_REGION: &str = "eu-central-1";

/// How many devices are activated in [`HOME_REGION`]. Enough for two ordering jobs
/// (the fixture puts two devices on each), so a `jobId` filter has a page of its own.
const HOME_DEVICES: u64 = 4;

/// How many devices are activated in [`OTHER_REGION`] — more than one, so its own
/// `ListDevices` has a page to cut and a cursor to issue.
const OTHER_DEVICES: u64 = 2;

/// The only `ResourceSummary$resourceType` an activated device's resources carry
/// (`device_resources.rs` pins it), which is what makes it a `type` filter that selects
/// everything.
const EC2_INSTANCE_TYPE: &str = "AWS::EC2::Instance";

/// A `type` value no resource of any device carries, so it selects nothing — a filter
/// that still narrows the enumeration a cursor belongs to.
const UNMATCHED_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// The bounds `NextToken` is modelled with (`@length(min: 1, max: 1024)` and
/// `@pattern("[a-zA-Z0-9+/=]*")`), spelled out here so a token the implementation
/// cannot send back over the wire fails this suite.
const TOKEN_MIN_LENGTH: usize = 1;
const TOKEN_MAX_LENGTH: usize = 1024;

/// One emulator: a router carrying only this service over one shared engine, with Snow
/// Device Management activated in BOTH Regions.
///
/// The API models no device-creation operation, so the inventory goes in through the one
/// state seam that activates a device.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = service_state(&*engine).expect("the in-memory engine carries this service's state");
        let mut store = state.lock();
        let now = SystemTime::now();
        store.activate_devices(HOME_REGION, HOME_DEVICES, now);
        store.activate_devices(OTHER_REGION, OTHER_DEVICES, now);
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

/// Dispatch one request signed for `region`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "host",
            format!("snow-device-management.{region}.amazonaws.com"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/\
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

/// One paginated enumeration: the request that asks for a page of it.
///
/// A row of every table below. `filters` holds the query parameters that NARROW the
/// enumeration, which — with the operation and the Region — is what a cursor into it is
/// bound to; `path` carries the resource a sub-list hangs off, where the operation binds
/// one as a path label.
#[derive(Debug, Clone)]
struct Listing {
    /// How a failing case names this enumeration.
    label: String,
    /// The Region the request is signed for.
    region: String,
    /// The operation's HTTP path, resource label included.
    path: String,
    /// The output member holding the page's items.
    items: &'static str,
    /// The narrowing query parameters, already joined (`""` when the request names
    /// none).
    filters: String,
}

impl Listing {
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
    listing: &Listing,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    call_in(
        router,
        &listing.region,
        "GET",
        &listing.uri(max_results, next_token),
        None,
    )
    .await
}

/// The ids a page reports, in the order it reports them, for whatever member identifies
/// an item of this enumeration.
fn item_keys(answer: &Answer, listing: &Listing, case: &str) -> Vec<String> {
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

/// A `nextToken` has to satisfy its modelled length and pattern, or no client can send
/// it back.
fn assert_modelled_token(token: &str, case: &str) {
    assert!(
        (TOKEN_MIN_LENGTH..=TOKEN_MAX_LENGTH).contains(&token.len())
            && token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')),
        "case {case:?}: nextToken {token:?} is outside NextToken's \
         @length({TOKEN_MIN_LENGTH}..{TOKEN_MAX_LENGTH}) / @pattern([a-zA-Z0-9+/=]*)"
    );
}

/// The cursor `listing` issues when its first page holds a single item.
async fn issued_cursor(router: &MultiServiceRouter, listing: &Listing) -> String {
    let case = format!("{} issues a cursor", listing.label);
    let answer = page(router, listing, Some(1), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    let token = answer.body["nextToken"].as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: this fixture holds more than one item, so the page carries a \
             cursor, got {}",
            answer.body
        )
    });
    assert_modelled_token(token, &case);
    token.to_string()
}

/// Assert a page was refused with HTTP 400 `ValidationException` naming the member at
/// fault.
fn assert_refused(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a nextToken this enumeration never issued is a request-shape \
         problem, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {}",
        answer.body
    );
    assert!(
        answer.error_message().contains("nextToken"),
        "case {case:?}: the refusal names the member at fault, got {:?}",
        answer.error_message()
    );
}

/// The ids of the devices activated in `region`, in the order `ListDevices` serves them.
async fn devices_in(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let listing = Listing {
        label: format!("ListDevices in {region}"),
        region: region.to_string(),
        path: "/managed-devices".to_string(),
        items: "devices",
        filters: String::new(),
    };
    let case = listing.label.clone();
    let answer = page(router, &listing, None, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    item_keys(&answer, &listing, &case)
}

/// Create one task in `region` over `targets`, answering its `taskId`.
async fn queue_task(router: &MultiServiceRouter, region: &str, targets: &[String]) -> String {
    let case = format!("CreateTask in {region} over {targets:?}");
    let answer = call_in(
        router,
        region,
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

/// The records every table below pages over: the activated inventory of both Regions
/// plus two tasks in [`HOME_REGION`], the first of them over two devices so its
/// execution list has a page to cut.
struct Fixture {
    home_devices: Vec<String>,
    other_devices: Vec<String>,
    task_over_two: String,
    task_over_one: String,
}

async fn fixture(router: &MultiServiceRouter) -> Fixture {
    let home_devices = devices_in(router, HOME_REGION).await;
    let other_devices = devices_in(router, OTHER_REGION).await;
    assert_eq!(
        home_devices.len(),
        usize::try_from(HOME_DEVICES).expect("a small count fits in usize"),
        "case \"fixture\": the home Region holds its activated inventory"
    );
    assert_eq!(
        other_devices.len(),
        usize::try_from(OTHER_DEVICES).expect("a small count fits in usize"),
        "case \"fixture\": the other Region holds its activated inventory"
    );
    let task_over_two = queue_task(router, HOME_REGION, &home_devices[0..2]).await;
    let task_over_one = queue_task(router, HOME_REGION, &home_devices[2..3]).await;
    Fixture {
        home_devices,
        other_devices,
        task_over_two,
        task_over_one,
    }
}

/// Every enumeration this suite can both issue a cursor for and replay one against: one
/// per paginated operation, each holding more than one item.
fn listings(fixture: &Fixture) -> Vec<Listing> {
    vec![
        Listing {
            label: "ListDevices".to_string(),
            region: HOME_REGION.to_string(),
            path: "/managed-devices".to_string(),
            items: "devices",
            filters: String::new(),
        },
        Listing {
            label: "ListTasks".to_string(),
            region: HOME_REGION.to_string(),
            path: "/tasks".to_string(),
            items: "tasks",
            filters: String::new(),
        },
        Listing {
            label: "ListExecutions".to_string(),
            region: HOME_REGION.to_string(),
            path: "/executions".to_string(),
            items: "executions",
            filters: format!("taskId={}", fixture.task_over_two),
        },
        Listing {
            label: "ListDeviceResources".to_string(),
            region: HOME_REGION.to_string(),
            path: format!("/managed-device/{}/resources", fixture.home_devices[0]),
            items: "resources",
            filters: String::new(),
        },
    ]
}

#[tokio::test]
async fn a_next_token_no_call_issued_is_refused_by_every_paginated_operation() {
    let router = emulator();
    let fixture = fixture(&router).await;
    let listings = listings(&fixture);

    // The table rows: the shape of a value no page ever handed back. Every row
    // satisfies `NextToken`'s own pattern, so the refusal comes from the handler and
    // not from the boundary validator, and the first rows are what a caller reaching
    // for the resume key inside a cursor would send.
    let mut rows: Vec<(String, String)> = vec![
        (
            "hex of a word".to_string(),
            ::hex::encode("abcd".as_bytes()),
        ),
        (
            "hex of a device id".to_string(),
            ::hex::encode(managed_device_id(0).as_bytes()),
        ),
        (
            "hex of a device id past the inventory".to_string(),
            ::hex::encode(managed_device_id(u64::MAX).as_bytes()),
        ),
        (
            "hex of a task id".to_string(),
            ::hex::encode(fixture.task_over_two.as_bytes()),
        ),
        (
            "hex of a resource id".to_string(),
            ::hex::encode(device_instance_ids(0)[0].as_bytes()),
        ),
        (
            "hex of a cursor-shaped payload with another marker".to_string(),
            ::hex::encode(
                json!(["cursor", "ListDevices", HOME_REGION, [null], managed_device_id(0)])
                    .to_string()
                    .as_bytes(),
            ),
        ),
        ("not hex".to_string(), "zz".to_string()),
        ("odd-length hex".to_string(), "abc".to_string()),
        ("hex of bytes that are not UTF-8".to_string(), "ffff".to_string()),
        ("the pattern's padding alone".to_string(), "====".to_string()),
        ("uppercase hex of nothing".to_string(), "ZZZZZZZZ".to_string()),
    ];
    // A cursor this service really did issue, damaged: truncated, extended, and with one
    // character replaced. None of them is a value a page handed back either.
    let issued = issued_cursor(&router, &listings[0]).await;
    rows.push((
        "an issued cursor, truncated".to_string(),
        issued[..issued.len() - 2].to_string(),
    ));
    rows.push((
        "an issued cursor, extended".to_string(),
        format!("{issued}00"),
    ));
    let mut flipped = issued.clone();
    flipped.replace_range(0..1, if issued.starts_with('5') { "6" } else { "5" });
    rows.push(("an issued cursor, one digit changed".to_string(), flipped));

    for listing in &listings {
        // What the enumeration holds before any refusal, so a refused call can be shown
        // to have changed nothing.
        let case = format!("{} before the refusals", listing.label);
        let before = item_keys(&page(&router, listing, None, None).await, listing, &case);

        for (shape, token) in &rows {
            let case = format!("{} with a nextToken that is {shape}", listing.label);
            assert_refused(&page(&router, listing, None, Some(token)).await, &case);
            // The same value is refused whatever page size it arrives with.
            assert_refused(&page(&router, listing, Some(1), Some(token)).await, &case);
        }

        let case = format!("{} after the refusals", listing.label);
        assert_eq!(
            item_keys(&page(&router, listing, None, None).await, listing, &case),
            before,
            "case {case:?}: a refused call must not change what a later caller sees"
        );
    }
}

#[tokio::test]
async fn a_cursor_is_bound_to_the_operation_that_issued_it() {
    let router = emulator();
    let fixture = fixture(&router).await;
    let listings = listings(&fixture);

    // The table rows: every ordered pair of paginated operations. The diagonal is the
    // positive control — a cursor replayed on the enumeration that issued it resumes a
    // page — and every other cell is a refusal.
    for issuer in &listings {
        let cursor = issued_cursor(&router, issuer).await;
        for replayer in &listings {
            let case = format!(
                "a {} cursor replayed on {}",
                issuer.label, replayer.label
            );
            let answer = page(&router, replayer, Some(1), Some(&cursor)).await;
            if issuer.label == replayer.label {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: its own cursor resumes a page, got {} with {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    item_keys(&answer, replayer, &case).len(),
                    1,
                    "case {case:?}: the resumed page holds the one item asked for, got {}",
                    answer.body
                );
            } else {
                assert_refused(&answer, &case);
            }
        }
    }
}

#[tokio::test]
async fn a_cursor_is_bound_to_the_region_it_was_issued_in() {
    let router = emulator();
    let fixture = fixture(&router).await;
    assert_ne!(
        fixture.home_devices, fixture.other_devices,
        "case \"fixture\": the two Regions hold different devices"
    );

    // The table rows: every ordered pair of Regions with an inventory of its own. A
    // cursor is a position in the enumeration of ONE Region's records.
    for issuing in [HOME_REGION, OTHER_REGION] {
        let issuer = Listing {
            label: format!("ListDevices in {issuing}"),
            region: issuing.to_string(),
            path: "/managed-devices".to_string(),
            items: "devices",
            filters: String::new(),
        };
        let cursor = issued_cursor(&router, &issuer).await;
        for replaying in [HOME_REGION, OTHER_REGION] {
            let replayer = Listing {
                label: format!("ListDevices in {replaying}"),
                region: replaying.to_string(),
                path: "/managed-devices".to_string(),
                items: "devices",
                filters: String::new(),
            };
            let case = format!("a cursor issued in {issuing} replayed in {replaying}");
            let answer = page(&router, &replayer, Some(1), Some(&cursor)).await;
            if issuing == replaying {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a cursor resumes the Region it was issued in, got {} \
                     with {}",
                    answer.status, answer.body
                );
            } else {
                assert_refused(&answer, &case);
            }
        }
    }
}

#[tokio::test]
async fn a_cursor_is_bound_to_the_filters_the_page_was_served_under() {
    let router = emulator();
    let fixture = fixture(&router).await;

    // The table rows: one paginated enumeration per row, each named by the members that
    // narrow it, paired with the SAME operation narrowed differently — a filter added, a
    // filter dropped, a filter's value changed, and the resource a sub-list hangs off
    // replaced. A cursor is a position in the narrowed enumeration, so only the first
    // (identical) variant resumes.
    //
    // `ListTasks`' and `ListExecutions`' `state` filters appear only as the variant, and
    // never as the issuer, because an execution's state is the clock's to advance while
    // the test runs; which records a state filter selects is pinned elsewhere.
    let first_job = ordering_job_id(0);
    let second_job = ordering_job_id(1);
    let rows: Vec<(Listing, Vec<Listing>)> = vec![
        (
            Listing {
                label: "ListDevices over the whole inventory".to_string(),
                region: HOME_REGION.to_string(),
                path: "/managed-devices".to_string(),
                items: "devices",
                filters: String::new(),
            },
            vec![
                Listing {
                    label: format!("ListDevices with jobId={first_job}"),
                    region: HOME_REGION.to_string(),
                    path: "/managed-devices".to_string(),
                    items: "devices",
                    filters: format!("jobId={first_job}"),
                },
                Listing {
                    label: format!("ListDevices with jobId={second_job}"),
                    region: HOME_REGION.to_string(),
                    path: "/managed-devices".to_string(),
                    items: "devices",
                    filters: format!("jobId={second_job}"),
                },
            ],
        ),
        (
            Listing {
                label: format!("ListDevices with jobId={first_job}"),
                region: HOME_REGION.to_string(),
                path: "/managed-devices".to_string(),
                items: "devices",
                filters: format!("jobId={first_job}"),
            },
            vec![
                Listing {
                    label: "ListDevices over the whole inventory".to_string(),
                    region: HOME_REGION.to_string(),
                    path: "/managed-devices".to_string(),
                    items: "devices",
                    filters: String::new(),
                },
                Listing {
                    label: format!("ListDevices with jobId={second_job}"),
                    region: HOME_REGION.to_string(),
                    path: "/managed-devices".to_string(),
                    items: "devices",
                    filters: format!("jobId={second_job}"),
                },
            ],
        ),
        (
            Listing {
                label: "ListTasks over every task".to_string(),
                region: HOME_REGION.to_string(),
                path: "/tasks".to_string(),
                items: "tasks",
                filters: String::new(),
            },
            vec![
                Listing {
                    label: "ListTasks with state=IN_PROGRESS".to_string(),
                    region: HOME_REGION.to_string(),
                    path: "/tasks".to_string(),
                    items: "tasks",
                    filters: "state=IN_PROGRESS".to_string(),
                },
                Listing {
                    label: "ListTasks with state=COMPLETED".to_string(),
                    region: HOME_REGION.to_string(),
                    path: "/tasks".to_string(),
                    items: "tasks",
                    filters: "state=COMPLETED".to_string(),
                },
            ],
        ),
        (
            Listing {
                label: "ListExecutions of the task over two devices".to_string(),
                region: HOME_REGION.to_string(),
                path: "/executions".to_string(),
                items: "executions",
                filters: format!("taskId={}", fixture.task_over_two),
            },
            vec![
                Listing {
                    label: "ListExecutions of the other task".to_string(),
                    region: HOME_REGION.to_string(),
                    path: "/executions".to_string(),
                    items: "executions",
                    filters: format!("taskId={}", fixture.task_over_one),
                },
                Listing {
                    label: "ListExecutions of the same task, QUEUED only".to_string(),
                    region: HOME_REGION.to_string(),
                    path: "/executions".to_string(),
                    items: "executions",
                    filters: format!("taskId={}&state=QUEUED", fixture.task_over_two),
                },
            ],
        ),
        (
            Listing {
                label: "ListDeviceResources of the first device".to_string(),
                region: HOME_REGION.to_string(),
                path: format!("/managed-device/{}/resources", fixture.home_devices[0]),
                items: "resources",
                filters: String::new(),
            },
            vec![
                Listing {
                    label: "ListDeviceResources of the second device".to_string(),
                    region: HOME_REGION.to_string(),
                    path: format!("/managed-device/{}/resources", fixture.home_devices[1]),
                    items: "resources",
                    filters: String::new(),
                },
                Listing {
                    label: format!("ListDeviceResources of the first device, type={EC2_INSTANCE_TYPE}"),
                    region: HOME_REGION.to_string(),
                    path: format!("/managed-device/{}/resources", fixture.home_devices[0]),
                    items: "resources",
                    filters: format!("type={EC2_INSTANCE_TYPE}"),
                },
                Listing {
                    label: format!(
                        "ListDeviceResources of the first device, type={UNMATCHED_RESOURCE_TYPE}"
                    ),
                    region: HOME_REGION.to_string(),
                    path: format!("/managed-device/{}/resources", fixture.home_devices[0]),
                    items: "resources",
                    filters: format!("type={UNMATCHED_RESOURCE_TYPE}"),
                },
            ],
        ),
    ];

    for (issuer, variants) in &rows {
        let cursor = issued_cursor(&router, issuer).await;

        // The positive control: the enumeration that issued the cursor resumes with it.
        let case = format!("{} resumed with its own cursor", issuer.label);
        let answer = page(&router, issuer, Some(1), Some(&cursor)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            answer.status, answer.body
        );

        for variant in variants {
            let case = format!("a cursor from {} replayed on {}", issuer.label, variant.label);
            assert_refused(&page(&router, variant, Some(1), Some(&cursor)).await, &case);
        }
    }
}

#[tokio::test]
async fn a_bound_cursor_still_walks_every_enumeration_exactly_once() {
    let router = emulator();
    let fixture = fixture(&router).await;
    let first_job = ordering_job_id(0);

    // The table rows: the enumerations above, including the narrowed ones, × the page
    // size, from the modelled `maxResults` floor to one past what the row holds and up
    // to the ceiling. Binding a cursor must not cost the walk: the pages still
    // concatenate to the unpaged answer, each item once and in order.
    let mut rows: Vec<Listing> = listings(&fixture);
    rows.push(Listing {
        label: format!("ListDevices with jobId={first_job}"),
        region: HOME_REGION.to_string(),
        path: "/managed-devices".to_string(),
        items: "devices",
        filters: format!("jobId={first_job}"),
    });
    rows.push(Listing {
        label: "ListDeviceResources of the second device".to_string(),
        region: HOME_REGION.to_string(),
        path: format!("/managed-device/{}/resources", fixture.home_devices[1]),
        items: "resources",
        filters: String::new(),
    });
    rows.push(Listing {
        label: "ListDevices in the other Region".to_string(),
        region: OTHER_REGION.to_string(),
        path: "/managed-devices".to_string(),
        items: "devices",
        filters: String::new(),
    });

    for listing in &rows {
        let case = format!("{} unpaged", listing.label);
        let whole = item_keys(&page(&router, listing, None, None).await, listing, &case);
        assert!(
            whole.len() >= 2,
            "case {case:?}: a walk is only a walk over more than one item, got {whole:?}"
        );

        for size in 1..=i32::try_from(whole.len() + 1).expect("a small count fits in i32") {
            let case = format!("{} at maxResults={size}", listing.label);
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let answer = page(&router, listing, Some(size), token.as_deref()).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200, got {} with {}",
                    answer.status, answer.body
                );
                let reported = item_keys(&answer, listing, &case);
                assert!(
                    reported.len() <= usize::try_from(size).expect("a positive size fits usize"),
                    "case {case:?}: a page never exceeds maxResults, got {reported:?}"
                );
                walked.extend(reported);
                pages += 1;
                assert!(
                    pages <= whole.len() + 1,
                    "case {case:?}: paging must terminate, got {pages} pages for {} items",
                    whole.len()
                );
                match answer.body["nextToken"].as_str() {
                    Some(next) => {
                        assert_modelled_token(next, &case);
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                walked, whole,
                "case {case:?}: the pages concatenate to the whole answer, each item once \
                 and in order"
            );
        }
    }
}
