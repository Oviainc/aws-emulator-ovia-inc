// SPDX-License-Identifier: CC0-1.0

//! The layout members whose documented default is the EMPTY STRING, over the wire.
//!
//! THE RULE: a member whose own documentation states the empty string as its default is
//! MATERIALISED at the empty string when the request leaves it out, so every report of
//! the resource carries the member; a value the request did carry is reported as it
//! arrived; and a member whose documentation states no default at all is reported by
//! nothing when the request leaves it out.
//!
//! The members this suite spans, each from its own doc in
//! `smithy_query(query="shape",shape="GridConfiguration,PipConfiguration")`:
//!
//! * `GridConfiguration$featuredParticipantAttribute` — "Default: `\"\"` (no featured
//!   participant)."
//! * `PipConfiguration$featuredParticipantAttribute` — the same sentence.
//! * `PipConfiguration$pipParticipantAttribute` — "Default: `\"\"` (no PiP
//!   participant)."
//!
//! and the member that marks the rule's boundary, present on both layouts:
//! `participantOrderAttribute`, whose doc states no default ("Participants with
//! `participantOrderAttribute` set to `\"\"` or not specified are ordered based on their
//! arrival time into the stage"), so an absent one stays absent.
//!
//! All four target `AttributeKey`, `@length(0..128)` with `@pattern
//! "^[a-zA-Z0-9-_]*$"`, so the rows below carry both published length bounds and one
//! past the maximum — which is refused rather than defaulted.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region every request of this suite is signed for.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`), and
/// so the account the channel ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The value the three members above document as their default.
const EMPTY_ATTRIBUTE: &str = "";

/// `AttributeKey`'s published `@length` maximum.
const ATTRIBUTE_KEY_MAX_LENGTH: usize = 128;

/// One IVS RealTime emulator over one shared engine, driven through the router that
/// serves production traffic.
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

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/{operation}"))
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

    /// `operation`, asserting the modelled success and answering with the member of the
    /// response body `key` names.
    async fn succeed(&self, case: &str, operation: &str, body: &Value, key: &str) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must answer HTTP 200, got {status} {response}"
        );
        response[key].clone()
    }

    /// Create a stage and answer with the ARN the service minted for it.
    async fn stage(&self, case: &str) -> String {
        let stage = self
            .succeed(case, "CreateStage", &json!({ "name": "composited" }), "stage")
            .await;
        arn_of(case, &stage)
    }
}

/// The ARN member of a resource the service just reported.
fn arn_of(case: &str, resource: &Value) -> String {
    resource["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted arn, got {resource}"))
        .to_string()
}

/// The wire error code of a refusal, as the protocol spells it.
fn error_code(response: &Value) -> String {
    response["__type"]
        .as_str()
        .or_else(|| response["code"].as_str())
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One channel destination is the smallest start request the model allows; the channel
/// lives in the sibling low-latency service, so any well-formed channel ARN serves.
fn destinations() -> Value {
    json!([{ "channel": { "channelArn": format!("arn:aws:ivs:{REGION}:{ACCOUNT}:channel/live") } }])
}

/// One row of the class: which layout the start request configures, the layout object it
/// carries, and what each attribute member of that layout must then report — `Null` for
/// a member no report may carry.
struct Row {
    case: &'static str,
    layout: &'static str,
    requested: Value,
    featured: Value,
    pip_participant: Value,
    order: Value,
}

/// The rows the class is spanned with. They differ along what the rule generalises over:
/// which layout carries the members, whether the request names each attribute at all,
/// and — where it does — the FORM of the name: the documented empty string itself, a
/// single character, the published `@length` maximum, and a name using every character
/// class the member's `@pattern` admits.
fn rows() -> Vec<Row> {
    // At the published `@length` maximum of `AttributeKey`, inside its `@pattern`.
    let longest = "f".repeat(ATTRIBUTE_KEY_MAX_LENGTH);
    let longest = longest.as_str();

    vec![
        Row {
            case: "an empty grid: the documented empty-string default",
            layout: "grid",
            requested: json!({}),
            featured: json!(EMPTY_ATTRIBUTE),
            pip_participant: Value::Null,
            order: Value::Null,
        },
        Row {
            case: "a grid naming a sibling member only",
            layout: "grid",
            requested: json!({ "gridGap": 4 }),
            featured: json!(EMPTY_ATTRIBUTE),
            pip_participant: Value::Null,
            order: Value::Null,
        },
        Row {
            case: "a grid naming a featured participant attribute",
            layout: "grid",
            requested: json!({ "featuredParticipantAttribute": "host" }),
            featured: json!("host"),
            pip_participant: Value::Null,
            order: Value::Null,
        },
        Row {
            case: "a grid naming the documented default explicitly",
            layout: "grid",
            requested: json!({ "featuredParticipantAttribute": EMPTY_ATTRIBUTE }),
            featured: json!(EMPTY_ATTRIBUTE),
            pip_participant: Value::Null,
            order: Value::Null,
        },
        Row {
            case: "a grid naming a one-character featured attribute",
            layout: "grid",
            requested: json!({ "featuredParticipantAttribute": "f" }),
            featured: json!("f"),
            pip_participant: Value::Null,
            order: Value::Null,
        },
        Row {
            case: "a grid naming a featured attribute at the published length maximum",
            layout: "grid",
            requested: json!({ "featuredParticipantAttribute": longest }),
            featured: json!(longest),
            pip_participant: Value::Null,
            order: Value::Null,
        },
        Row {
            case: "a grid naming the ordering attribute but not the featured one",
            layout: "grid",
            requested: json!({ "participantOrderAttribute": "seat-1_A" }),
            featured: json!(EMPTY_ATTRIBUTE),
            pip_participant: Value::Null,
            order: json!("seat-1_A"),
        },
        Row {
            case: "an empty picture-in-picture layout: both documented empty strings",
            layout: "pip",
            requested: json!({}),
            featured: json!(EMPTY_ATTRIBUTE),
            pip_participant: json!(EMPTY_ATTRIBUTE),
            order: Value::Null,
        },
        Row {
            case: "a picture-in-picture layout naming a sibling member only",
            layout: "pip",
            requested: json!({ "pipPosition": "TOP_LEFT" }),
            featured: json!(EMPTY_ATTRIBUTE),
            pip_participant: json!(EMPTY_ATTRIBUTE),
            order: Value::Null,
        },
        Row {
            case: "a picture-in-picture layout naming the PiP participant only",
            layout: "pip",
            requested: json!({ "pipParticipantAttribute": "guest" }),
            featured: json!(EMPTY_ATTRIBUTE),
            pip_participant: json!("guest"),
            order: Value::Null,
        },
        Row {
            case: "a picture-in-picture layout naming the featured participant only",
            layout: "pip",
            requested: json!({ "featuredParticipantAttribute": "host" }),
            featured: json!("host"),
            pip_participant: json!(EMPTY_ATTRIBUTE),
            order: Value::Null,
        },
        Row {
            case: "a picture-in-picture layout naming both participant attributes",
            layout: "pip",
            requested: json!({
                "featuredParticipantAttribute": "host",
                "pipParticipantAttribute": "guest",
            }),
            featured: json!("host"),
            pip_participant: json!("guest"),
            order: Value::Null,
        },
        Row {
            case: "a picture-in-picture layout naming both at the published length maximum",
            layout: "pip",
            requested: json!({
                "featuredParticipantAttribute": longest,
                "pipParticipantAttribute": longest,
            }),
            featured: json!(longest),
            pip_participant: json!(longest),
            order: Value::Null,
        },
        Row {
            case: "a picture-in-picture layout naming the ordering attribute only",
            layout: "pip",
            requested: json!({ "participantOrderAttribute": "seat" }),
            featured: json!(EMPTY_ATTRIBUTE),
            pip_participant: json!(EMPTY_ATTRIBUTE),
            order: json!("seat"),
        },
    ]
}

/// Every attribute member whose documentation states the empty string as its default is
/// reported — at that empty string when the request left it out, and as it arrived when
/// the request carried it — by the start AND by the read, so the materialised default is
/// persisted with the composition rather than shaped onto one response.
#[tokio::test]
async fn an_empty_string_documented_default_is_materialised_by_every_report() {
    let service = RealTime::new();
    let stage_arn = service.stage("attributes").await;

    for row in rows() {
        let case = row.case;
        let selector = row.layout;
        let request = json!({
            "stageArn": stage_arn.as_str(),
            "destinations": destinations(),
            "layout": { selector: row.requested },
        });
        let started = service
            .succeed(case, "StartComposition", &request, "composition")
            .await;

        // (the member, what it must report)
        let expected: Vec<(&str, &Value)> = vec![
            ("featuredParticipantAttribute", &row.featured),
            ("pipParticipantAttribute", &row.pip_participant),
            ("participantOrderAttribute", &row.order),
        ];

        let arn = arn_of(case, &started);
        let read = service
            .succeed(
                case,
                "GetComposition",
                &json!({ "arn": arn.as_str() }),
                "composition",
            )
            .await;

        for (reporter, composition) in [("StartComposition", &started), ("GetComposition", &read)] {
            let layout = &composition["layout"][row.layout];
            for (member, want) in &expected {
                let actual = &layout[*member];
                assert_eq!(
                    actual, *want,
                    "case {case:?}: expected {reporter} to report {member} as {want} in the \
                     {} layout, got {actual:?} in {layout}",
                    row.layout
                );
            }
        }
    }
}

/// The documented default applies to a member the request LEFT OUT, never to a value the
/// member's `@length` excludes: one character past the published maximum is the modelled
/// `ValidationException` (HTTP 400) on every layout member that targets `AttributeKey`.
#[tokio::test]
async fn an_attribute_past_the_published_length_is_refused_not_defaulted() {
    let service = RealTime::new();
    let stage_arn = service.stage("refusal").await;
    let too_long = "f".repeat(ATTRIBUTE_KEY_MAX_LENGTH + 1);

    // Rows: each `AttributeKey` member of each layout, one character past the maximum.
    let members: Vec<(&str, &str)> = vec![
        ("grid", "featuredParticipantAttribute"),
        ("grid", "participantOrderAttribute"),
        ("pip", "featuredParticipantAttribute"),
        ("pip", "pipParticipantAttribute"),
        ("pip", "participantOrderAttribute"),
    ];
    for (layout, member) in members {
        let case = format!("{layout}.{member} one character past the length maximum");
        let (status, response) = service
            .call(
                "StartComposition",
                &json!({
                    "stageArn": stage_arn.as_str(),
                    "destinations": destinations(),
                    "layout": { layout: { member: too_long.as_str() } },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
    }
}
