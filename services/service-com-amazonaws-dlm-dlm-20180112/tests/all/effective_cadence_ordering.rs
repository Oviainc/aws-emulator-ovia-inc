// SPDX-License-Identifier: CC0-1.0

//! The documented ordering between a default policy's creation frequency and its
//! retention period, judged on the configuration the policy would RUN on — the
//! one a read of it reports, defaults included.
//!
//! The rule under test: Amazon Data Lifecycle Manager accepts a default policy's
//! cadence only when the retention period the policy ends up with is greater
//! than the creation frequency it ends up with, where a member the request
//! leaves out takes the default its own documentation publishes — so the two
//! members a read reports always satisfy the ordering, whether the caller stated
//! both of them, one, or neither.
//!
//! `RetainInterval` — "The retention period can range from 2 to 14 days, but it
//! must be greater than the creation frequency to ensure that the policy retains
//! at least 1 snapshot or AMI at any given time. If you do not specify a value,
//! the default is 7." `CreateInterval` — "The creation frequency can range from
//! 1 to 7 days. If you do not specify a value, the default is 1."
//! (`smithy_query(query="doc", shape="CreateLifecyclePolicyRequest",
//! member="RetainInterval")` / `member="CreateInterval"`, the same sentences on
//! `UpdateLifecyclePolicyRequest` and `PolicyDetails`.)
//!
//! The tables span the class rather than the one request the issue cited: every
//! creation frequency in its published range stated alone, every retention
//! period in its published range stated alone, neither member stated, both
//! places a request can state them (the request body and the `PolicyDetails`
//! structure), and both the create and the update — including the update whose
//! `PolicyDetails` replaces the stored configuration wholesale, so the member it
//! omits goes back to its default.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-east-2";

/// `PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default policy."
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The published bounds of the two cadence members, and the documented default
/// each takes when a request states none.
const CREATE_INTERVAL_MIN: i64 = 1;
const CREATE_INTERVAL_MAX: i64 = 7;
const RETAIN_INTERVAL_MIN: i64 = 2;
const RETAIN_INTERVAL_MAX: i64 = 14;
const CREATE_INTERVAL_DEFAULT: i64 = 1;
const RETAIN_INTERVAL_DEFAULT: i64 = 7;

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`
/// in the emulator's own account, so `iam:PassRole` is granted.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

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
    fn policy(&self) -> &Value {
        &self.body["Policy"]
    }

    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
            .unwrap_or_default()
    }
}

/// A SigV4 credential scope naming this service — what routes the request to dlm.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
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

async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/policies", Some(request)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/policies/{id}"), None).await
}

async fn update(router: &MultiServiceRouter, id: &str, request: Value) -> Answer {
    call(router, "PATCH", &format!("/policies/{id}"), Some(request)).await
}

/// How many policies the store holds, read through `GetLifecyclePolicies`.
async fn policy_count(router: &MultiServiceRouter) -> usize {
    let answer = call(router, "GET", "/policies", None).await;
    answer.body["Policies"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// Assert `answer` is this service's modelled refusal of the ordering, naming
/// both members and the two values the policy would have run on.
fn assert_ordering_refused(case: &str, effective: (i64, i64), answer: &Answer) {
    let (create_interval, retain_interval) = effective;
    assert_eq!(
        answer.status, INVALID_REQUEST_STATUS,
        "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().ends_with(INVALID_REQUEST_EXCEPTION),
        "case {case:?}: expected the modelled {INVALID_REQUEST_EXCEPTION}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        at(&answer.body, "/Code"),
        json!(INVALID_PARAMETER_CODE),
        "case {case:?}: expected Code {INVALID_PARAMETER_CODE:?}, got {:?}",
        answer.body
    );
    for expected in [
        format!("RetainInterval ({retain_interval} days)"),
        format!("CreateInterval ({create_interval} days)"),
    ] {
        assert!(
            answer.message().contains(&expected),
            "case {case:?}: the refusal must name {expected:?}, got {:?}",
            answer.message()
        );
    }
}

/// Assert the stored policy `id` reports `expected` and, with it, the invariant
/// the ordering rule exists for: a retention period greater than the creation
/// frequency.
async fn assert_reads_back(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    expected: (i64, i64),
) {
    let (expected_create, expected_retain) = expected;
    let read = get(router, id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the policy must be readable, got {} with {:?}",
        read.status, read.body
    );
    let create_interval = at(read.policy(), "/PolicyDetails/CreateInterval");
    let retain_interval = at(read.policy(), "/PolicyDetails/RetainInterval");
    assert_eq!(
        create_interval,
        json!(expected_create),
        "case {case:?}: expected CreateInterval {expected_create}, got {create_interval:?}"
    );
    assert_eq!(
        retain_interval,
        json!(expected_retain),
        "case {case:?}: expected RetainInterval {expected_retain}, got {retain_interval:?}"
    );
    let reported_create = create_interval
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: CreateInterval must be a number"));
    let reported_retain = retain_interval
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: RetainInterval must be a number"));
    assert!(
        reported_retain > reported_create,
        "case {case:?}: a stored policy must report a retention period greater than its \
         creation frequency, got {reported_retain} beside {reported_create}"
    );
}

/// One cadence a request states partially, and what the service owes for it.
struct PartialCadence {
    case: String,
    create_interval: Option<i64>,
    retain_interval: Option<i64>,
    /// The pair the policy would run on: each member as stated, else its
    /// documented default.
    effective: (i64, i64),
}

impl PartialCadence {
    /// The rule: the cadence is refused exactly when the pair the policy would
    /// run on does not have the retention period above the creation frequency.
    fn refused(&self) -> bool {
        self.effective.1 <= self.effective.0
    }
}

/// Every partially stated cadence: each creation frequency in its published
/// range stated alone, each retention period in its published range stated
/// alone, and neither member stated.
fn partial_cadences() -> Vec<PartialCadence> {
    let mut rows = vec![PartialCadence {
        case: "neither member stated, so both take their documented default".to_string(),
        create_interval: None,
        retain_interval: None,
        effective: (CREATE_INTERVAL_DEFAULT, RETAIN_INTERVAL_DEFAULT),
    }];
    for create_interval in CREATE_INTERVAL_MIN..=CREATE_INTERVAL_MAX {
        rows.push(PartialCadence {
            case: format!(
                "a creation frequency of {create_interval} days stated alone, beside the \
                 retention default"
            ),
            create_interval: Some(create_interval),
            retain_interval: None,
            effective: (create_interval, RETAIN_INTERVAL_DEFAULT),
        });
    }
    for retain_interval in RETAIN_INTERVAL_MIN..=RETAIN_INTERVAL_MAX {
        rows.push(PartialCadence {
            case: format!(
                "a retention period of {retain_interval} days stated alone, beside the \
                 creation-frequency default"
            ),
            create_interval: None,
            retain_interval: Some(retain_interval),
            effective: (CREATE_INTERVAL_DEFAULT, retain_interval),
        });
    }
    rows
}

/// The row stated in the request BODY of a default-policy create.
fn body_create_request(row: &PartialCadence) -> Value {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "a partially stated cadence in the request body",
        "State": "ENABLED",
        "DefaultPolicy": "VOLUME",
    });
    if let Some(create_interval) = row.create_interval {
        request["CreateInterval"] = json!(create_interval);
    }
    if let Some(retain_interval) = row.retain_interval {
        request["RetainInterval"] = json!(retain_interval);
    }
    request
}

/// The same row stated inside the `PolicyDetails` of a default-policy create.
fn details_create_request(row: &PartialCadence) -> Value {
    let mut details = json!({"PolicyLanguage": DEFAULT_POLICY_LANGUAGE, "ResourceType": "VOLUME"});
    if let Some(create_interval) = row.create_interval {
        details["CreateInterval"] = json!(create_interval);
    }
    if let Some(retain_interval) = row.retain_interval {
        details["RetainInterval"] = json!(retain_interval);
    }
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "a partially stated cadence in PolicyDetails",
        "State": "ENABLED",
        "PolicyDetails": details,
    })
}

/// A create that states one of the two cadence members is judged on the pair the
/// policy would run on: the member it omits takes its documented default, and
/// the configuration a read reports therefore never breaks the ordering.
#[tokio::test]
async fn a_create_judges_the_ordering_against_the_omitted_member_default() {
    let router = emulator();
    for row in partial_cadences() {
        for (where_stated, request) in [
            ("in the request body", body_create_request(&row)),
            ("in PolicyDetails", details_create_request(&row)),
        ] {
            let case = format!("{} ({where_stated})", row.case);
            let before = policy_count(&router).await;
            let answer = create(&router, request).await;

            if row.refused() {
                assert_ordering_refused(&case, row.effective, &answer);
                assert_eq!(
                    policy_count(&router).await,
                    before,
                    "case {case:?}: a refused create must leave no policy behind"
                );
                continue;
            }

            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_reads_back(&router, &case, &answer.policy_id(), row.effective).await;
        }
    }
}

/// A default `VOLUME` policy holding whatever cadence `cadence` states, as the
/// subject of one patch row. A row needing a policy that never stored a member
/// states none for it, so the stored configuration really is unset.
async fn default_policy_holding(
    router: &MultiServiceRouter,
    case: &str,
    cadence: (Option<i64>, Option<i64>),
) -> String {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "the subject of a patch",
        "State": "ENABLED",
        "DefaultPolicy": "VOLUME",
    });
    if let Some(create_interval) = cadence.0 {
        request["CreateInterval"] = json!(create_interval);
    }
    if let Some(retain_interval) = cadence.1 {
        request["RetainInterval"] = json!(retain_interval);
    }
    let answer = create(router, request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the subject policy must be created, got {} with {:?}",
        answer.status, answer.body
    );
    answer.policy_id()
}

/// A patch stating its cadence inside `PolicyDetails`, which for a default policy
/// also carries the two members that say WHICH kind of policy the configuration
/// describes.
fn details_patch(members: Value) -> Value {
    let mut details = json!({"PolicyLanguage": DEFAULT_POLICY_LANGUAGE, "ResourceType": "VOLUME"});
    for (member, value) in members.as_object().expect("the row states an object") {
        details[member] = value.clone();
    }
    json!({"PolicyDetails": details})
}

/// One patch of a policy holding a known cadence, and the pair the patch would
/// leave the policy running on.
struct PatchRow {
    case: &'static str,
    /// The `(CreateInterval, RetainInterval)` the subject policy was created
    /// with; `None` for a member the create never stated.
    stored: (Option<i64>, Option<i64>),
    patch: Value,
    /// The pair the patched policy would run on.
    effective: (i64, i64),
    /// Whether the patch is refused, which the rule decides from `effective`.
    refused: bool,
}

/// `UpdateLifecyclePolicy` judges the same ordering on the pair the patched
/// policy would run on: a member this request does not state, and that the
/// policy holds no stored value for — either because nothing ever stated it, or
/// because a `PolicyDetails` replaces the configuration wholesale — takes its
/// documented default.
#[tokio::test]
async fn an_update_judges_the_ordering_against_the_omitted_member_default() {
    let router = emulator();
    let rows = vec![
        PatchRow {
            case: "a creation frequency at its maximum, against a policy holding no retention \
                   period",
            stored: (None, None),
            patch: json!({"CreateInterval": CREATE_INTERVAL_MAX}),
            effective: (CREATE_INTERVAL_MAX, RETAIN_INTERVAL_DEFAULT),
            refused: true,
        },
        PatchRow {
            case: "a creation frequency one day below the retention default, against a policy \
                   holding no retention period",
            stored: (None, None),
            patch: json!({"CreateInterval": RETAIN_INTERVAL_DEFAULT - 1}),
            effective: (RETAIN_INTERVAL_DEFAULT - 1, RETAIN_INTERVAL_DEFAULT),
            refused: false,
        },
        PatchRow {
            case: "a retention period at its minimum, against a policy holding no creation \
                   frequency",
            stored: (None, None),
            patch: json!({"RetainInterval": RETAIN_INTERVAL_MIN}),
            effective: (CREATE_INTERVAL_DEFAULT, RETAIN_INTERVAL_MIN),
            refused: false,
        },
        PatchRow {
            case: "a PolicyDetails stating only a creation frequency at its maximum, which drops \
                   the stored retention period",
            stored: (Some(3), Some(10)),
            patch: details_patch(json!({"CreateInterval": CREATE_INTERVAL_MAX})),
            effective: (CREATE_INTERVAL_MAX, RETAIN_INTERVAL_DEFAULT),
            refused: true,
        },
        PatchRow {
            case: "a PolicyDetails stating only a creation frequency the retention default still \
                   exceeds",
            stored: (Some(3), Some(10)),
            patch: details_patch(json!({"CreateInterval": RETAIN_INTERVAL_DEFAULT - 1})),
            effective: (RETAIN_INTERVAL_DEFAULT - 1, RETAIN_INTERVAL_DEFAULT),
            refused: false,
        },
        PatchRow {
            case: "a PolicyDetails stating only a retention period, which drops the stored \
                   creation frequency",
            stored: (Some(5), Some(12)),
            patch: details_patch(json!({"RetainInterval": RETAIN_INTERVAL_MAX})),
            effective: (CREATE_INTERVAL_DEFAULT, RETAIN_INTERVAL_MAX),
            refused: false,
        },
        PatchRow {
            case: "a creation frequency at its maximum, against a policy whose stored retention \
                   period exceeds it",
            stored: (Some(3), Some(10)),
            patch: json!({"CreateInterval": CREATE_INTERVAL_MAX}),
            effective: (CREATE_INTERVAL_MAX, 10),
            refused: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let id = default_policy_holding(&router, case, row.stored).await;
        let before = get(&router, &id).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the subject policy must be readable, got {} with {:?}",
            before.status, before.body
        );
        let snapshot = before.policy().clone();

        assert_eq!(
            row.refused,
            row.effective.1 <= row.effective.0,
            "case {case:?}: the row must state the outcome the rule gives for the pair \
             {:?} the patched policy would run on",
            row.effective
        );

        let answer = update(&router, &id, row.patch).await;
        if row.refused {
            assert_ordering_refused(case, row.effective, &answer);
            let after = get(&router, &id).await;
            assert_eq!(
                after.policy(),
                &snapshot,
                "case {case:?}: a refused update must leave the stored policy untouched, got {:?}",
                after.policy()
            );
            continue;
        }

        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_reads_back(&router, case, &id, row.effective).await;
    }
}

/// The two paths agree: a cadence the create refuses is one no update can leave
/// stored either, and a cadence a read reports is one the create accepts when
/// the caller spells both members out. No effective configuration is both
/// produced by one path and refused by the other.
#[tokio::test]
async fn a_cadence_a_read_reports_is_one_a_create_stating_it_in_full_accepts() {
    let router = emulator();
    for row in partial_cadences() {
        let case = format!("{} (restated in full)", row.case);
        let (create_interval, retain_interval) = row.effective;
        let partial = create(&router, body_create_request(&row)).await;
        let full = create(
            &router,
            json!({
                "ExecutionRoleArn": role_arn(),
                "Description": "the same cadence spelled out",
                "State": "ENABLED",
                "DefaultPolicy": "VOLUME",
                "CreateInterval": create_interval,
                "RetainInterval": retain_interval,
            }),
        )
        .await;
        assert_eq!(
            partial.status, full.status,
            "case {case:?}: the cadence {:?} must be answered the same way whether or not the \
             caller states the member the service would fill in, got {} ({:?}) partially stated \
             and {} ({:?}) in full",
            row.effective, partial.status, partial.body, full.status, full.body
        );
        if full.status == 200 {
            assert_reads_back(&router, &case, &full.policy_id(), row.effective).await;
        }
    }
}
