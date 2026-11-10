// SPDX-License-Identifier: CC0-1.0

//! The LIFETIME of a layout member's documented default, over the wire.
//!
//! THE RULE: a layout member the start request leaves out is reported at the default
//! that member's OWN documentation states, a member the request carried is reported as
//! it arrived, and the value is decided ONCE — when the composition is started — so it
//! is stored with the composition and every later report of that composition repeats it
//! unchanged.
//!
//! `GridConfiguration$gridGap` is the member that separates the two halves of the rule:
//! the model declares `@default 0` for it while its documentation states "Default: `2`",
//! so an absent member and a member sent at the declared default must stay
//! distinguishable (`smithy_query(query="shape",shape="GridConfiguration")`). The same
//! member name on the sibling `PipConfiguration` documents `0`, so one layout's default
//! is never read off the other's.
//!
//! What this suite adds to the per-member table in `documented_member_defaults.rs` is the
//! INVARIANT over the composition's life: the layout a start reported is the layout the
//! idempotent replay of that same start reports, and the layout a read reports after the
//! clock has moved the composition out of `STARTING` — a default recomputed per report,
//! or recomputed differently on one of those paths, fails here even when the start itself
//! is right.
//!
//! A gap outside the member's `@range` is refused rather than defaulted; that refusal is
//! pinned in `documented_member_defaults.rs` and is not repeated here.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status and the JSON body — never an
//! internal call.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`), and
/// so the account the channel ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region every request of this suite is signed for.
const REGION: &str = "us-east-1";

// The layout defaults the model documents, each from that member's own doc in
// `smithy_query(query="doc",shape="GridConfiguration",member="…")` /
// `…shape="PipConfiguration"`:
//   GridConfiguration$gridGap           "Default: 2."
//   GridConfiguration$videoAspectRatio  "Default: VIDEO."
//   GridConfiguration$videoFillMode     "defaults to COVER fill mode for participants
//                                        in the grid"
//   GridConfiguration$omitStoppedVideo  "Default: false."
//   PipConfiguration$gridGap            "Default: 0."
//   PipConfiguration$pipOffset          "Default: 0."
//   PipConfiguration$videoFillMode      "Default: COVER."
//   PipConfiguration$pipBehavior        "Default: STATIC."
//   PipConfiguration$pipPosition        "Default: BOTTOM_RIGHT."
//   GridConfiguration$featuredParticipantAttribute  "Default: \"\" (no featured
//                                        participant)."
//   PipConfiguration$featuredParticipantAttribute   the same sentence
//   PipConfiguration$pipParticipantAttribute        "Default: \"\" (no PiP participant)."
const DEFAULT_GRID_GAP: i64 = 2;
const DEFAULT_GRID_ASPECT_RATIO: &str = "VIDEO";
const DEFAULT_GRID_FILL_MODE: &str = "COVER";
const DEFAULT_OMIT_STOPPED_VIDEO: bool = false;
const DEFAULT_PIP_GRID_GAP: i64 = 0;
const DEFAULT_PIP_OFFSET: i64 = 0;
const DEFAULT_PIP_FILL_MODE: &str = "COVER";
const DEFAULT_PIP_BEHAVIOR: &str = "STATIC";
const DEFAULT_PIP_POSITION: &str = "BOTTOM_RIGHT";
const NO_FEATURED_PARTICIPANT: &str = "";
const NO_PIP_PARTICIPANT: &str = "";

/// The `CompositionState` a started composition reports once the clock has made its
/// transition due, from `smithy_query(query="shape",shape="CompositionState")`.
const STATE_ACTIVE: &str = "ACTIVE";

/// Long enough that the `STARTING` -> `ACTIVE` transition the emulator owes has fallen
/// due, whatever scheduling delay the test host adds.
fn past_the_activation_span() -> Duration {
    state::COMPOSITION_ACTIVATION_SPAN + Duration::from_millis(400)
}

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
            .uri(format!("https://ivsrealtime.{REGION}.amazonaws.com/{operation}"))
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

    async fn start(&self, case: &str, body: &Value) -> Value {
        self.succeed(case, "StartComposition", body, "composition")
            .await
    }

    async fn get(&self, case: &str, arn: &str) -> Value {
        self.succeed(case, "GetComposition", &json!({ "arn": arn }), "composition")
            .await
    }
}

/// The ARN member of a resource the service just reported.
fn arn_of(case: &str, resource: &Value) -> String {
    resource["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a minted arn, got {resource}"))
        .to_string()
}

/// The grid layout a composition reports when the request configured `members` and left
/// every other member out: each absent member at the default its own documentation
/// states.
fn grid_reporting(members: Value) -> Value {
    let mut grid = json!({
        "featuredParticipantAttribute": NO_FEATURED_PARTICIPANT,
        "omitStoppedVideo": DEFAULT_OMIT_STOPPED_VIDEO,
        "videoAspectRatio": DEFAULT_GRID_ASPECT_RATIO,
        "videoFillMode": DEFAULT_GRID_FILL_MODE,
        "gridGap": DEFAULT_GRID_GAP,
    });
    for (member, value) in members.as_object().cloned().unwrap_or_default() {
        grid[member] = value;
    }
    json!({ "grid": grid })
}

/// The same, for the picture-in-picture layout, whose `gridGap` documents its own
/// default.
fn pip_reporting(members: Value) -> Value {
    let mut pip = json!({
        "featuredParticipantAttribute": NO_FEATURED_PARTICIPANT,
        "omitStoppedVideo": DEFAULT_OMIT_STOPPED_VIDEO,
        "videoFillMode": DEFAULT_PIP_FILL_MODE,
        "gridGap": DEFAULT_PIP_GRID_GAP,
        "pipParticipantAttribute": NO_PIP_PARTICIPANT,
        "pipBehavior": DEFAULT_PIP_BEHAVIOR,
        "pipOffset": DEFAULT_PIP_OFFSET,
        "pipPosition": DEFAULT_PIP_POSITION,
    });
    for (member, value) in members.as_object().cloned().unwrap_or_default() {
        pip[member] = value;
    }
    json!({ "pip": pip })
}

/// One row of the layout class: the `layout` the start request carries (`Value::Null` for
/// a request that carries none) and the layout every report of that composition must
/// then carry.
struct Row {
    case: &'static str,
    requested: Value,
    reported: Value,
}

/// The rows the class is spanned with. They differ along what the rule generalises over:
/// whether the request carries a layout at all, which of the two layouts it configures,
/// whether it sizes the gap, the value it sizes it with — including the `0` the model
/// declares as the member's `@default`, which an absent member must NOT be read as — and
/// which siblings it carries beside the gap.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no layout at all: the documented grid fallback",
            requested: Value::Null,
            reported: grid_reporting(json!({})),
        },
        Row {
            case: "a layout object configuring neither layout",
            requested: json!({}),
            reported: grid_reporting(json!({})),
        },
        Row {
            case: "an empty grid",
            requested: json!({ "grid": {} }),
            reported: grid_reporting(json!({})),
        },
        Row {
            case: "a grid sizing a sibling member, not the gap",
            requested: json!({ "grid": { "videoAspectRatio": "SQUARE" } }),
            reported: grid_reporting(json!({ "videoAspectRatio": "SQUARE" })),
        },
        Row {
            case: "a grid naming the featured participant, not the gap",
            requested: json!({ "grid": { "featuredParticipantAttribute": "host" } }),
            reported: grid_reporting(json!({ "featuredParticipantAttribute": "host" })),
        },
        Row {
            case: "a grid omitting stopped video, not sizing the gap",
            requested: json!({ "grid": { "omitStoppedVideo": true } }),
            reported: grid_reporting(json!({ "omitStoppedVideo": true })),
        },
        Row {
            // The published minimum of the member's `@range`, and the value the model
            // declares as its `@default`: a caller who asks for it gets it, which is what
            // keeps "absent" and "sent at the declared default" different requests.
            case: "a grid sizing the gap at the published minimum",
            requested: json!({ "grid": { "gridGap": 0 } }),
            reported: grid_reporting(json!({ "gridGap": 0 })),
        },
        Row {
            case: "a grid sizing the gap at one pixel",
            requested: json!({ "grid": { "gridGap": 1 } }),
            reported: grid_reporting(json!({ "gridGap": 1 })),
        },
        Row {
            case: "a grid sizing the gap at the documented default itself",
            requested: json!({ "grid": { "gridGap": DEFAULT_GRID_GAP } }),
            reported: grid_reporting(json!({ "gridGap": DEFAULT_GRID_GAP })),
        },
        Row {
            case: "a grid sizing the gap far above the published minimum",
            requested: json!({ "grid": { "gridGap": 4_096 } }),
            reported: grid_reporting(json!({ "gridGap": 4_096 })),
        },
        Row {
            // `GridGap` is `@range(min 0)` with no maximum, so the largest value the
            // modelled integer carries is inside the range.
            case: "a grid sizing the gap at the largest modelled integer",
            requested: json!({ "grid": { "gridGap": i32::MAX } }),
            reported: grid_reporting(json!({ "gridGap": i32::MAX })),
        },
        Row {
            case: "an empty picture-in-picture layout",
            requested: json!({ "pip": {} }),
            reported: pip_reporting(json!({})),
        },
        Row {
            case: "a picture-in-picture layout sizing the gap and the offset",
            requested: json!({ "pip": { "gridGap": 7, "pipOffset": 9 } }),
            reported: pip_reporting(json!({ "gridGap": 7, "pipOffset": 9 })),
        },
        Row {
            case: "a picture-in-picture layout sizing a window, not the gap",
            requested: json!({ "pip": { "pipWidth": 240, "pipHeight": 135 } }),
            reported: pip_reporting(json!({ "pipWidth": 240, "pipHeight": 135 })),
        },
    ]
}

/// Every layout member the request left out is reported at its own documented default,
/// and that value is decided once: a read of the composition, and a read after the clock
/// has moved it to `ACTIVE`, report the identical layout — and the start cannot be made to
/// decide the defaults a second time under the token it was first made with.
#[tokio::test]
async fn a_layout_default_is_stored_once_and_repeated_by_every_later_report() {
    let service = RealTime::new();
    let stage_arn = service.stage("layout").await;
    // One channel destination is the smallest start request the model allows; the channel
    // lives in the sibling low-latency service, so any well-formed channel ARN serves.
    let destinations = json!([{ "channel": {
        "channelArn": format!("arn:aws:ivs:{REGION}:{ACCOUNT}:channel/live"),
    }}]);

    // (the case, the layout it must report, the composition it started)
    let mut started: Vec<(&str, Value, String)> = Vec::new();
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let mut request = json!({
            "stageArn": stage_arn.as_str(),
            "destinations": destinations.clone(),
            "idempotencyToken": format!("layout-{index}"),
        });
        if !row.requested.is_null() {
            request["layout"] = row.requested.clone();
        }

        let composition = service.start(case, &request).await;
        assert_eq!(
            composition["layout"], row.reported,
            "case {case:?}: expected the layout {}, got {}",
            row.reported, composition["layout"]
        );

        // THE STORED LAYOUT IS DECIDED ONCE: repeating the same request under the same
        // `@idempotencyToken` names the composition the first call started, which already
        // exists — the modelled conflict rather than a second decision of the defaults —
        // and a read of that composition reports the layout the start stored.
        let arn = arn_of(case, &composition);
        let (status, refused) = service.call("StartComposition", &request).await;
        assert_eq!(
            status, 409,
            "case {case:?}: repeating the start must answer HTTP 409, got {status} {refused}"
        );
        let read = service.get(case, &arn).await;
        assert_eq!(
            read["layout"], row.reported,
            "case {case:?}: expected the read to report the layout {}, got {}",
            row.reported, read["layout"]
        );

        started.push((case, row.reported, arn));
    }

    // PERSISTENCE ACROSS THE CLOCK-OWED TRANSITION: the layout is not part of what the
    // clock owes a started composition, so the read that reports `ACTIVE` reports the
    // layout the start did.
    tokio::time::sleep(past_the_activation_span()).await;
    for (case, expected, arn) in started {
        let read = service.get(case, &arn).await;
        assert_eq!(
            read["state"],
            json!(STATE_ACTIVE),
            "case {case:?}: expected the composition to have become {STATE_ACTIVE}, got {read}"
        );
        assert_eq!(
            read["layout"], expected,
            "case {case:?}: expected GetComposition to report the layout {expected}, got {}",
            read["layout"]
        );
    }
}

/// The two layouts' `gridGap` members target one `GridGap` shape and document different
/// defaults, so a composition started with one layout never reports the other's default:
/// the pair is asserted together, on requests that differ in nothing else.
#[tokio::test]
async fn each_layout_reports_its_own_gap_default_not_the_siblings() {
    let service = RealTime::new();
    let stage_arn = service.stage("gap").await;
    let destinations = json!([{ "channel": {
        "channelArn": format!("arn:aws:ivs:{REGION}:{ACCOUNT}:channel/live"),
    }}]);

    // (case, the layout member that selects the layout, the gap it must report)
    let cases: Vec<(&str, &str, i64)> = vec![
        ("the grid layout", "grid", DEFAULT_GRID_GAP),
        ("the picture-in-picture layout", "pip", DEFAULT_PIP_GRID_GAP),
    ];
    for (case, layout, expected) in cases {
        let request = json!({
            "stageArn": stage_arn.as_str(),
            "destinations": destinations.clone(),
            "layout": { layout: {} },
        });
        let composition = service.start(case, &request).await;
        assert_eq!(
            composition["layout"][layout]["gridGap"],
            json!(expected),
            "case {case:?}: expected gridGap {expected}, got {}",
            composition["layout"]
        );
        // The layout the request did NOT configure is reported by nothing: a composition
        // runs one layout.
        let other = if layout == "grid" { "pip" } else { "grid" };
        assert!(
            composition["layout"][other].is_null(),
            "case {case:?}: expected no {other} layout, got {}",
            composition["layout"]
        );
    }
}
