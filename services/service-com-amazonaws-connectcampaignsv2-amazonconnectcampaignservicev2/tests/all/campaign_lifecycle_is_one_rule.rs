// SPDX-License-Identifier: CC0-1.0

//! The ONE lifecycle rule all four campaign actions move a campaign by, driven end to
//! end over the wire through each of the four operations.
//!
//! `StartCampaign`, `PauseCampaign`, `ResumeCampaign` and `StopCampaign` move a
//! campaign through the same rule on this service's store, so these cases hold all
//! four to that rule rather than to one copy of it:
//!
//! * the call is judged against the state the campaign is at NOW — the clock-derived
//!   one, so a campaign whose scheduled run window has closed is `Completed` — and a
//!   legal action writes the state the transition table names, which
//!   `GetCampaignState` then reports;
//! * an action that is not legal from that state is refused with HTTP 409
//!   `InvalidCampaignStateException` naming the state it was judged at, with the same
//!   sentence from every one of the four, and the stored state is left alone;
//! * an id the request's region holds no campaign under is HTTP 404
//!   `ResourceNotFoundException` with the same sentence from every one of the four,
//!   and a campaign of another region is exactly that: absent, and untouched.

use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::CAMPAIGN_NOT_FOUND_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, campaign_state, create_campaign, drive_lifecycle, error_code, router};

/// One lifecycle operation: the route segment it is addressed by and the verb its
/// refusal names it with.
#[derive(Debug, Clone, Copy)]
struct Action {
    operation: &'static str,
    segment: &'static str,
    verb: &'static str,
}

/// The four operations that move a campaign's lifecycle, each as the route the model
/// binds it to (`POST /v2/campaigns/{id}/{segment}`).
const ACTIONS: &[Action] = &[
    Action {
        operation: "StartCampaign",
        segment: "start",
        verb: "started",
    },
    Action {
        operation: "PauseCampaign",
        segment: "pause",
        verb: "paused",
    },
    Action {
        operation: "ResumeCampaign",
        segment: "resume",
        verb: "resumed",
    },
    Action {
        operation: "StopCampaign",
        segment: "stop",
        verb: "stopped",
    },
];

/// A state a campaign is reachable at through the API, and how a case gets it there:
/// a campaign created with `window` as its run window (offsets in seconds from now,
/// `None` for no schedule at all), put through `setup` — each action asserted to
/// succeed.
#[derive(Debug, Clone, Copy)]
struct Origin {
    state: &'static str,
    window: Option<(f64, f64)>,
    setup: &'static [&'static str],
}

/// Every state the API can put a campaign at, and the route there. `Failed` is the
/// one value of the model's `CampaignState` catalog no call reaches — nothing in this
/// slice fails a campaign — so no row names it; the whole catalog is covered against
/// the stored record by `campaign_schedule_completion`.
const ORIGINS: &[Origin] = &[
    Origin {
        state: "Initialized",
        window: None,
        setup: &[],
    },
    Origin {
        state: "Running",
        window: None,
        setup: &["start"],
    },
    Origin {
        state: "Paused",
        window: None,
        setup: &["start", "pause"],
    },
    Origin {
        state: "Stopped",
        window: None,
        setup: &["start", "stop"],
    },
    // The clock's own state: a campaign started inside a run window that has since
    // closed has run to the end of its schedule, which is the terminal `Completed`
    // without any call having written it.
    Origin {
        state: "Completed",
        window: Some((-7_200.0, -3_600.0)),
        setup: &["start"],
    },
];

/// The state a legal action moves a campaign to, or `None` when the action is refused
/// from there.
///
/// Written out here rather than asked of the crate, so a case holds the handlers to
/// the table instead of to whatever the code computes: Start is the only action a
/// campaign that has never entered its run takes, Pause suspends a running campaign,
/// Resume puts a suspended one back to work, Stop ends a run already under way, no
/// action is a no-op, and nothing moves out of a terminal state.
fn expected_next(from: &str, operation: &str) -> Option<&'static str> {
    match (from, operation) {
        ("Initialized" | "Paused" | "Stopped", "StartCampaign") => Some("Running"),
        ("Running", "PauseCampaign") => Some("Paused"),
        ("Paused", "ResumeCampaign") => Some("Running"),
        ("Running" | "Paused", "StopCampaign") => Some("Stopped"),
        _ => None,
    }
}

/// `now + offset_secs` as the RFC 3339 instant a `Schedule` member carries on the
/// wire: `Schedule$startTime` / `$endTime` target `TimeStamp`, which the model marks
/// `@timestampFormat("date-time")`.
fn instant(offset_secs: f64) -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a test clock after the Unix epoch")
        .as_secs_f64();
    DateTime::from_secs_f64(now + offset_secs)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// The `schedule` body an [`Origin`]'s window is sent as, if it carries one.
fn schedule_of(window: Option<(f64, f64)>) -> Option<Value> {
    window.map(|(start, end)| json!({"startTime": instant(start), "endTime": instant(end)}))
}

/// One lifecycle action on `id`, as the request the model binds it to.
async fn act(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    action: Action,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/{}", action.segment),
        &Value::Null,
    )
    .await
}

/// Every state the API reaches crossed with every lifecycle operation: a legal action
/// writes the state the table names and a refused one answers 409 with that state and
/// the shared sentence, leaving the campaign where it was.
#[tokio::test]
async fn all_four_actions_move_a_campaign_by_one_rule() {
    let region = "us-west-2";
    let instance = "lifecycle-one-rule";
    let router = router();

    for origin in ORIGINS {
        for action in ACTIONS {
            let case = format!("{} from {}", action.operation, origin.state);
            let id = create_campaign(
                &router,
                region,
                instance,
                &format!("one-rule-{}-{}", action.segment, origin.state),
                schedule_of(origin.window),
            )
            .await;
            drive_lifecycle(&router, region, &id, origin.setup, &case).await;
            let before = campaign_state(&router, region, &id, &case).await;
            assert_eq!(
                before, origin.state,
                "case {case:?}: expected the setup to leave the campaign at {:?}, got {before:?}",
                origin.state
            );

            let (status, body) = act(&router, region, &id, *action).await;
            match expected_next(origin.state, action.operation) {
                Some(next) => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected HTTP 200, got {status} {body}"
                    );
                    let after = campaign_state(&router, region, &id, &case).await;
                    assert_eq!(
                        after, next,
                        "case {case:?}: expected the campaign at {next:?}, got {after:?}"
                    );
                }
                None => {
                    assert_eq!(
                        status, 409,
                        "case {case:?}: expected HTTP 409, got {status} {body}"
                    );
                    assert!(
                        error_code(&body).ends_with("InvalidCampaignStateException"),
                        "case {case:?}: expected InvalidCampaignStateException, got {}",
                        error_code(&body)
                    );
                    assert_eq!(
                        body["state"].as_str(),
                        Some(origin.state),
                        "case {case:?}: expected the refusal to name state {:?}, got {:?}",
                        origin.state,
                        body["state"]
                    );
                    let expected_message = format!(
                        "A campaign in state {} cannot be {}",
                        origin.state, action.verb
                    );
                    assert_eq!(
                        body["message"].as_str(),
                        Some(expected_message.as_str()),
                        "case {case:?}: expected {expected_message:?}, got {:?}",
                        body["message"]
                    );
                    let after = campaign_state(&router, region, &id, &case).await;
                    assert_eq!(
                        after, origin.state,
                        "case {case:?}: a refusal must write nothing, got {after:?}"
                    );
                }
            }
        }
    }
}

/// An id no campaign of the request's region answers to is the same refusal from all
/// four operations, over several well-formed ids — so the refusal is "no such
/// campaign" rather than anything about one id's shape.
#[tokio::test]
async fn all_four_actions_refuse_an_unknown_id_alike() {
    let region = "eu-west-1";
    let router = router();
    // One stored campaign, so the store is not simply empty: these ids are absent, not
    // unreachable.
    let stored = create_campaign(&router, region, "lifecycle-one-rule-404", "kept", None).await;

    // Each row is within `CampaignId`'s pattern `^[-:/a-zA-Z0-9]+$` and its 1..256
    // length bound (source: the operations' `id` member), carries no `/` — the id
    // travels as one path label — and none of them is the id the create above minted.
    let unknown = [
        ("a minted-shaped id", "000000009999".to_string()),
        ("a word", "no-such-campaign".to_string()),
        ("a single character", "x".to_string()),
        ("a colon-shaped id", "campaign:42".to_string()),
        ("the longest id the model takes", "a".repeat(256)),
    ];

    for (id_case, id) in unknown {
        for action in ACTIONS {
            let case = format!("{} on {id_case}", action.operation);
            let (status, body) = act(&router, region, &id, *action).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {}",
                error_code(&body)
            );
            assert_eq!(
                body["message"].as_str(),
                Some(CAMPAIGN_NOT_FOUND_MESSAGE),
                "case {case:?}: expected {CAMPAIGN_NOT_FOUND_MESSAGE:?}, got {:?}",
                body["message"]
            );
        }
    }

    // The refusals changed nothing: the campaign the region does hold is still where
    // its create left it.
    let after = campaign_state(&router, region, &stored, "the kept campaign").await;
    assert_eq!(
        after, "Initialized",
        "case \"the kept campaign\": expected Initialized after the refusals, got {after:?}"
    );
}

/// All four operations resolve the id in the region the request was signed for: a
/// campaign of another region is absent here and untouched by the refusal, while the
/// same action against its own region's endpoint is served.
#[tokio::test]
async fn all_four_actions_resolve_the_id_in_the_requests_region() {
    let home = "us-east-1";
    let away = "ap-south-1";
    let instance = "lifecycle-one-rule-regions";
    let router = router();

    for action in ACTIONS {
        let case = format!("{} across regions", action.operation);
        let id = create_campaign(
            &router,
            home,
            instance,
            &format!("one-rule-region-{}", action.segment),
            None,
        )
        .await;
        // Put the campaign at a state this action is legal from, so what the other
        // region's endpoint answers cannot be confused with an illegal transition.
        let setup: &[&str] = match action.segment {
            "start" => &[],
            "pause" => &["start"],
            "resume" => &["start", "pause"],
            _ => &["start"],
        };
        drive_lifecycle(&router, home, &id, setup, &case).await;
        let before = campaign_state(&router, home, &id, &case).await;

        let (status, body) = act(&router, away, &id, *action).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from another region's endpoint, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
        assert_eq!(
            body["message"].as_str(),
            Some(CAMPAIGN_NOT_FOUND_MESSAGE),
            "case {case:?}: expected {CAMPAIGN_NOT_FOUND_MESSAGE:?}, got {:?}",
            body["message"]
        );

        let unchanged = campaign_state(&router, home, &id, &case).await;
        assert_eq!(
            unchanged, before,
            "case {case:?}: another region's call must write nothing, got {unchanged:?}"
        );

        let (status, served) = act(&router, home, &id, *action).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the same action to be served in its own region, \
             got {status} {served}"
        );
    }
}
