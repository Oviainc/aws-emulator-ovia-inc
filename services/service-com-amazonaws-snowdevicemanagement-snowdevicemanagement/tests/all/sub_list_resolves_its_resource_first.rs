// SPDX-License-Identifier: CC0-1.0

//! In what ORDER a sub-list of one resource refuses a request, end to end over HTTP.
//!
//! The rule under test: where an operation enumerates the records that hang off a
//! resource the request ADDRESSES, whether that resource exists is settled before any
//! member that merely says which slice of it to serve — so an id naming no such
//! resource of the account in the signed Region is the modelled
//! `ResourceNotFoundException` (HTTP 404) whatever page cursor, filter or page size
//! arrived with it, and the cursor's `ValidationException` (HTTP 400) is reachable only
//! for a resource that is really there. Both refusals are modelled on both operations
//! (`smithy_query(query="operation",operation="ListDeviceResources,ListExecutions")`
//! declares `ResourceNotFoundException` and `ValidationException` for each), so only the
//! order decides, and a caller must not be told its cursor is wrong when the thing it
//! addressed does not exist.
//!
//! The two enumerations this service hangs off an addressed resource are both rows of
//! every table below: `ListDeviceResources` (the resources of the `managedDeviceId` its
//! path labels) and `ListExecutions` (the executions of the `taskId` its query names).
//! `ListDevices` and `ListTasks` address no resource — they enumerate a Region's own
//! records and model no `ResourceNotFoundException` — so they are not part of this rule;
//! what a cursor is BOUND to is pinned in `pagination_cursor_binding.rs`.
//!
//! Each table's rows differ along what the rule generalises over: which enumeration, the
//! SHAPE of the unknown id (both modelled `@length(1..64)` bounds, the shape this store
//! mints, an unrelated shape, and an id that exists but in another Region), the shape of
//! the `nextToken` (absent, several values no page ever handed back, and a cursor this
//! service really issued for a sibling resource), the narrowing filter, and the page
//! size.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one most of the fixture is
/// activated in.
const SIGNED_REGION: &str = "ap-southeast-2";

/// Another Region of the same account with an inventory and a task of its own, so an id
/// that really exists somewhere is still unknown in the signed Region.
const OTHER_REGION: &str = "eu-west-3";

/// How many devices are activated in [`SIGNED_REGION`]. The fixture's first device runs
/// more than one resource, so a cursor can be issued for its sub-list.
const ACTIVATED_DEVICES: u64 = 2;

/// The resource type an Amazon EC2 instance carries, which is what makes it a `type`
/// filter that selects every resource a device runs (pinned in `device_resources.rs`).
const EC2_INSTANCE_TYPE: &str = "AWS::EC2::Instance";

/// A `type` value no resource of any device carries, so it selects nothing.
const UNMATCHED_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success. restJson1
    /// renders `__type` as a shape id on some services, so the code is its last segment.
    fn error_code(&self) -> &str {
        self.body["__type"]
            .as_str()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
    }

    /// The `message` member of an error body, or the empty string.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// Dispatch one request signed for `region`.
async fn call(
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

/// One enumeration this service hangs off a resource the request addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubList {
    /// `ListDeviceResources`: the resources of the addressed managed device.
    DeviceResources,
    /// `ListExecutions`: the executions of the addressed task.
    TaskExecutions,
}

impl SubList {
    /// Every row of a table over this rule.
    fn all() -> Vec<Self> {
        vec![Self::DeviceResources, Self::TaskExecutions]
    }

    /// The operation a failing case names.
    fn label(self) -> &'static str {
        match self {
            Self::DeviceResources => "ListDeviceResources",
            Self::TaskExecutions => "ListExecutions",
        }
    }

    /// The output member holding the page's items.
    fn items(self) -> &'static str {
        match self {
            Self::DeviceResources => "resources",
            Self::TaskExecutions => "executions",
        }
    }

    /// The narrowing values this enumeration can be asked under: none, one that selects
    /// everything it holds, and one that selects nothing. A filter never decides whether
    /// the addressed resource exists.
    fn filters(self) -> Vec<Option<&'static str>> {
        match self {
            Self::DeviceResources => {
                vec![None, Some(EC2_INSTANCE_TYPE), Some(UNMATCHED_RESOURCE_TYPE)]
            }
            // `ExecutionState` values, as the model lists them.
            Self::TaskExecutions => vec![None, Some("QUEUED"), Some("CANCELED")],
        }
    }

    /// The URI for one page of this enumeration of `parent`.
    fn uri(
        self,
        parent: &str,
        filter: Option<&str>,
        max_results: Option<i32>,
        next_token: Option<&str>,
    ) -> String {
        let mut query: Vec<String> = Vec::new();
        let path = match self {
            Self::DeviceResources => {
                if let Some(filter) = filter {
                    query.push(format!("type={filter}"));
                }
                format!("/managed-device/{parent}/resources")
            }
            Self::TaskExecutions => {
                query.push(format!("taskId={parent}"));
                if let Some(filter) = filter {
                    query.push(format!("state={filter}"));
                }
                "/executions".to_string()
            }
        };
        if let Some(max_results) = max_results {
            query.push(format!("maxResults={max_results}"));
        }
        if let Some(next_token) = next_token {
            query.push(format!("nextToken={next_token}"));
        }
        if query.is_empty() {
            path
        } else {
            format!("{path}?{}", query.join("&"))
        }
    }
}

/// Ask for one page of `sub_list` of `parent`, signed for [`SIGNED_REGION`].
async fn page(
    router: &MultiServiceRouter,
    sub_list: SubList,
    parent: &str,
    filter: Option<&str>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let uri = sub_list.uri(parent, filter, max_results, next_token);
    call(router, SIGNED_REGION, "GET", &uri, None).await
}

/// The item ids a page reports, in the order it reports them.
fn item_keys(answer: &Answer, sub_list: SubList, case: &str) -> Vec<String> {
    answer.body[sub_list.items()]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a {} array, got {}",
                sub_list.items(),
                answer.body
            )
        })
        .iter()
        .map(|item| {
            for member in ["managedDeviceId", "id"] {
                if let Some(value) = item[member].as_str() {
                    return value.to_string();
                }
            }
            panic!("case {case:?}: expected an identified item, got {item}")
        })
        .collect()
}

/// The records every table below reads: what each enumeration is addressed to in the
/// signed Region, and the ids of the same kinds that exist only in [`OTHER_REGION`].
struct Fixture {
    /// The devices activated in [`SIGNED_REGION`], in inventory order.
    devices: Vec<String>,
    /// A task of [`SIGNED_REGION`] over every one of those devices, so its execution
    /// list holds more than one record.
    task: String,
    /// A SECOND task of [`SIGNED_REGION`] over the same devices, so a cursor exists for
    /// an enumeration of the same operation that is not the one under test.
    sibling_task: String,
    /// A device activated only in [`OTHER_REGION`].
    other_region_device: String,
    /// A task created only in [`OTHER_REGION`].
    other_region_task: String,
}

impl Fixture {
    /// The resource `sub_list` is addressed to in the signed Region, which really exists.
    fn present(&self, sub_list: SubList) -> &str {
        match sub_list {
            SubList::DeviceResources => &self.devices[0],
            SubList::TaskExecutions => &self.task,
        }
    }

    /// A SECOND resource of the same kind that also exists, so a cursor can be issued
    /// for one enumeration and replayed on another of the same operation.
    fn sibling(&self, sub_list: SubList) -> &str {
        match sub_list {
            SubList::DeviceResources => &self.devices[1],
            SubList::TaskExecutions => &self.sibling_task,
        }
    }

    /// Ids of the kind `sub_list` addresses that name no such resource of the account in
    /// the signed Region: the modelled `@length(1..64)` floor and ceiling, the shape this
    /// store mints, an unrelated shape, and an id that exists in another Region.
    fn absent(&self, sub_list: SubList) -> Vec<String> {
        let (prefix, elsewhere) = match sub_list {
            SubList::DeviceResources => ("smd", &self.other_region_device),
            SubList::TaskExecutions => ("st", &self.other_region_task),
        };
        vec![
            "x".to_string(),
            "f".repeat(64),
            format!("{prefix}-ffffffffffffffff"),
            "does-not-exist".to_string(),
            elsewhere.clone(),
        ]
    }
}

/// Create one task in `region` over `targets` and answer its id.
async fn queue_task(router: &MultiServiceRouter, region: &str, targets: &[String]) -> String {
    let answer = call(
        router,
        region,
        "POST",
        "/task",
        Some(json!({ "targets": targets, "command": { "unlock": {} } })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"fixture\": CreateTask in {region} must succeed, got {} with {}",
        answer.status, answer.body
    );
    answer.body["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"fixture\": expected a taskId, got {}", answer.body))
        .to_string()
}

/// One emulator plus the fixture it holds. The API models no device-creation operation,
/// so the inventory goes in through the one activation seam; the tasks are created over
/// the wire.
async fn emulator() -> (MultiServiceRouter, Fixture) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let (devices, elsewhere) = {
        let state =
            service_state(&*engine).expect("the in-memory engine carries this service's state");
        let now = SystemTime::now();
        let devices = state.lock().activate_devices(SIGNED_REGION, ACTIVATED_DEVICES, now);
        let elsewhere = state.lock().activate_devices(OTHER_REGION, 1, now);
        (devices, elsewhere)
    };
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let task = queue_task(&router, SIGNED_REGION, &devices).await;
    let sibling_task = queue_task(&router, SIGNED_REGION, &devices).await;
    let other_region_task = queue_task(&router, OTHER_REGION, &elsewhere).await;
    let fixture = Fixture {
        devices,
        task,
        sibling_task,
        other_region_device: elsewhere[0].clone(),
        other_region_task,
    };
    (router, fixture)
}

/// A cursor this service really issued for `sub_list` of `parent`: the token the first
/// page of size one handed back.
async fn issued_cursor(router: &MultiServiceRouter, sub_list: SubList, parent: &str) -> String {
    let case = format!("{} of {parent} at maxResults=1", sub_list.label());
    let uri = sub_list.uri(parent, None, Some(1), None);
    let answer = call(router, SIGNED_REGION, "GET", &uri, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the enumeration must hold more than one page for a cursor \
                 to exist, got {}",
                answer.body
            )
        })
        .to_string()
}

/// One `nextToken` a case sends.
#[derive(Debug, Clone)]
struct TokenCase {
    /// How a failing case names this value.
    shape: String,
    /// The value itself.
    token: String,
    /// Whether it is the cursor the enumeration under test issued ITSELF, which is the
    /// one value that resumes a page rather than being refused.
    resumes: bool,
}

/// The `nextToken` values a case sends: values no page of any enumeration ever handed
/// back, a cursor this service really issued for a SIBLING resource of the same
/// operation, and the cursor the enumeration under test issued itself. Every one
/// satisfies `NextToken`'s modelled `@pattern("[a-zA-Z0-9+/=]*")`, so a refusal comes
/// from the handler and not from the boundary validator.
async fn token_shapes(
    router: &MultiServiceRouter,
    sub_list: SubList,
    fixture: &Fixture,
) -> Vec<TokenCase> {
    let mut cases: Vec<TokenCase> = [
        ("not hex", "zz".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of bytes that are not UTF-8", "ffff".to_string()),
        ("the pattern's padding alone", "====".to_string()),
        ("hex of a word", ::hex::encode("abcd".as_bytes())),
    ]
    .into_iter()
    .map(|(shape, token)| TokenCase {
        shape: shape.to_string(),
        token,
        resumes: false,
    })
    .collect();
    cases.push(TokenCase {
        shape: "a cursor issued for a sibling resource".to_string(),
        token: issued_cursor(router, sub_list, fixture.sibling(sub_list)).await,
        resumes: false,
    });
    cases.push(TokenCase {
        shape: "a cursor issued for the resource that exists".to_string(),
        token: issued_cursor(router, sub_list, fixture.present(sub_list)).await,
        resumes: true,
    });
    cases
}

#[tokio::test]
async fn an_unknown_addressed_resource_is_not_found_whatever_page_was_asked_of_it() {
    let (router, fixture) = emulator().await;
    let mut statuses: BTreeSet<u16> = BTreeSet::new();
    let mut codes: BTreeSet<String> = BTreeSet::new();

    for sub_list in SubList::all() {
        let tokens = token_shapes(&router, sub_list, &fixture).await;

        for parent in fixture.absent(sub_list) {
            // The baseline: the plainest possible ask of the same unknown id, with no
            // cursor, no filter and no page size.
            let case = format!("{} of {parent:?} asked plainly", sub_list.label());
            let baseline = page(&router, sub_list, &parent, None, None, None).await;
            assert_eq!(
                baseline.status, 404,
                "case {case:?}: an id that names no resource of this account in the \
                 signed Region is the modelled ResourceNotFoundException, got {} with {}",
                baseline.status, baseline.body
            );
            assert_eq!(
                baseline.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {}",
                baseline.body
            );
            assert!(
                baseline.message().contains(&parent),
                "case {case:?}: the refusal is built from the request, so it names the id \
                 that resolved to nothing, got {:?}",
                baseline.message()
            );

            // The rows: every shape of `nextToken` × every narrowing filter × the page
            // size. None of them is what the answer comes from, so each one answers the
            // baseline's refusal exactly.
            for case_token in &tokens {
                for filter in sub_list.filters() {
                    for size in [None, Some(1), Some(100)] {
                        let case = format!(
                            "{} of {parent:?} with a nextToken that is {}, filter \
                             {filter:?} and maxResults {size:?}",
                            sub_list.label(),
                            case_token.shape
                        );
                        let answer = page(
                            &router,
                            sub_list,
                            &parent,
                            filter,
                            size,
                            Some(&case_token.token),
                        )
                        .await;
                        assert_eq!(
                            answer.status, baseline.status,
                            "case {case:?}: the addressed id decides the refusal, so the \
                             status cannot change with the page asked for; expected {}, \
                             got {} with {}",
                            baseline.status, answer.status, answer.body
                        );
                        assert_eq!(
                            answer.body, baseline.body,
                            "case {case:?}: expected the same refusal body as the plain \
                             ask ({}), got {}",
                            baseline.body, answer.body
                        );
                        statuses.insert(answer.status);
                        codes.insert(answer.error_code().to_string());
                    }
                }
            }

            // And with no cursor but every other member set: the filter and the page size
            // are not what decides it either.
            for filter in sub_list.filters() {
                for size in [None, Some(1)] {
                    let case = format!(
                        "{} of {parent:?} with filter {filter:?} and maxResults {size:?}",
                        sub_list.label()
                    );
                    let answer = page(&router, sub_list, &parent, filter, size, None).await;
                    assert_eq!(
                        answer.body, baseline.body,
                        "case {case:?}: expected the same refusal as the plain ask ({}), \
                         got {}",
                        baseline.body, answer.body
                    );
                    statuses.insert(answer.status);
                    codes.insert(answer.error_code().to_string());
                }
            }
        }
    }

    assert_eq!(
        statuses,
        BTreeSet::from([404]),
        "case \"one outcome\": an unknown addressed resource is one refusal class, \
         whatever was asked of it; got {statuses:?}"
    );
    assert_eq!(
        codes,
        BTreeSet::from(["ResourceNotFoundException".to_string()]),
        "case \"one outcome\": an unknown addressed resource is one error code, whatever \
         was asked of it; got {codes:?}"
    );
}

#[tokio::test]
async fn a_cursor_is_only_judged_for_a_resource_that_exists() {
    let (router, fixture) = emulator().await;

    for sub_list in SubList::all() {
        let present = fixture.present(sub_list).to_string();
        let tokens = token_shapes(&router, sub_list, &fixture).await;

        for case_token in &tokens {
            let case = format!(
                "{} of the resource that exists with a nextToken that is {}",
                sub_list.label(),
                case_token.shape
            );
            let answer = page(
                &router,
                sub_list,
                &present,
                None,
                None,
                Some(&case_token.token),
            )
            .await;
            if case_token.resumes {
                // The positive control: its own cursor still resumes a page, so the
                // ordering did not cost the walk.
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: an enumeration resumes with its own cursor, got {} \
                     with {}",
                    answer.status, answer.body
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {case:?}: for a resource that is really there, a token this \
                 enumeration never issued is the modelled ValidationException, got {} \
                 with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                answer.body
            );
            assert!(
                answer.message().contains("nextToken"),
                "case {case:?}: the refusal names the member at fault, got {:?}",
                answer.message()
            );
        }
    }
}

#[tokio::test]
async fn the_refusals_change_nothing_a_later_caller_sees() {
    let (router, fixture) = emulator().await;

    for sub_list in SubList::all() {
        let present = fixture.present(sub_list).to_string();
        let case = format!("{} before the refusals", sub_list.label());
        let before = item_keys(
            &page(&router, sub_list, &present, None, None, None).await,
            sub_list,
            &case,
        );
        assert!(
            before.len() >= 2,
            "case {case:?}: the enumeration must hold more than one record for a cursor \
             to exist, got {before:?}"
        );

        let tokens = token_shapes(&router, sub_list, &fixture).await;
        for parent in fixture
            .absent(sub_list)
            .into_iter()
            .chain([present.clone()])
        {
            for case_token in &tokens {
                page(
                    &router,
                    sub_list,
                    &parent,
                    None,
                    Some(1),
                    Some(&case_token.token),
                )
                .await;
            }
        }

        let case = format!("{} after the refusals", sub_list.label());
        let after = item_keys(
            &page(&router, sub_list, &present, None, None, None).await,
            sub_list,
            &case,
        );
        assert_eq!(
            after, before,
            "case {case:?}: a refused or resumed read must not change what a later caller \
             sees, got {after:?}"
        );
    }
}
