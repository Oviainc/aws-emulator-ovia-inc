// SPDX-License-Identifier: CC0-1.0

//! The ONE configuration-change rule every campaign-configuration operation judges a
//! call by, driven end to end over the wire through each of those operations.
//!
//! `UpdateCampaignSource`, `UpdateCampaignSchedule`, `UpdateCampaignFlowAssociation`,
//! the entry-limits, communication-limits and communication-time updates and the three
//! deletions beside them all resolve and judge a campaign through the same entry point
//! on this service's store, so these cases hold every one of them to that rule rather
//! than to one copy of it:
//!
//! * a campaign the clock has in a state that takes no further configuration change is
//!   refused with HTTP 409 `InvalidCampaignStateException` naming that state, with the
//!   same sentence from every one of the operations, and nothing about the campaign is
//!   written;
//! * a campaign in any state that still takes the change is served, and the call moves
//!   no lifecycle;
//! * an id the request's region holds no campaign under is HTTP 404
//!   `ResourceNotFoundException` with the same sentence from every one of them.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::CAMPAIGN_NOT_FOUND_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use super::common::{
    call, campaign_state, create_campaign_with, describe_campaign, drive_lifecycle, error_code,
    instant, router, Harness,
};

/// One campaign-configuration operation: the request the model binds it to, relative
/// to `/v2/campaigns/{id}`, and a well-formed body for it.
#[derive(Debug, Clone)]
struct Configure {
    operation: &'static str,
    method: &'static str,
    suffix: String,
    body: Value,
}

/// Every operation that changes a campaign's configuration — the ones whose model
/// declares `InvalidCampaignStateException` beside `ResourceNotFoundException`.
///
/// The `config` query values come from the model's own catalogs
/// (`CommunicationLimitsConfigType` and `CommunicationTimeConfigType`, as
/// `COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES` / `COMMUNICATION_TIME_CONFIG_TYPE_VALUES`
/// in this crate's `ops::types` render them).
fn configuration_operations() -> Vec<Configure> {
    let row = |operation: &'static str, method: &'static str, suffix: &str, body: Value| Configure {
        operation,
        method,
        suffix: suffix.to_string(),
        body,
    };
    vec![
        row(
            "UpdateCampaignSource",
            "POST",
            "/source",
            json!({"source": {
                "customerProfilesSegmentArn":
                    "arn:aws:profile:us-east-1:000000000000:segment/one-rule"
            }}),
        ),
        row(
            "UpdateCampaignSchedule",
            "POST",
            "/schedule",
            // A window that is still open, so a served call leaves the campaign's
            // clock-derived state exactly where it was.
            json!({"schedule": {"startTime": instant(60.0), "endTime": instant(7_200.0)}}),
        ),
        row(
            "UpdateCampaignFlowAssociation",
            "POST",
            "/flow",
            json!({
                "connectCampaignFlowArn":
                    "arn:aws:connect:us-east-1:000000000000:instance/i-1/flow/f-1"
            }),
        ),
        row(
            "UpdateCampaignEntryLimits",
            "POST",
            "/entry-limits",
            json!({"entryLimitsConfig": {"maxEntryCount": 4, "minEntryInterval": "PT45M"}}),
        ),
        row("DeleteCampaignEntryLimits", "DELETE", "/entry-limits", Value::Null),
        row(
            "UpdateCampaignCommunicationLimits",
            "POST",
            "/communication-limits",
            json!({"communicationLimitsOverride": {"allChannelSubtypes": {
                "communicationLimitsList": [
                    {"maxCountPerRecipient": 3, "frequency": 2, "unit": "DAY"}
                ]
            }}}),
        ),
        row(
            "DeleteCampaignCommunicationLimits",
            "DELETE",
            "/communication-limits?config=ALL_CHANNEL_SUBTYPES",
            Value::Null,
        ),
        row(
            "UpdateCampaignCommunicationTime",
            "POST",
            "/communication-time",
            json!({"communicationTimeConfig": {
                "localTimeZoneConfig": {"defaultTimeZone": "America/Los_Angeles"},
                "telephony": {"openHours": {
                    "dailyHours": {"TUESDAY": [{"startTime": "T08:00", "endTime": "T18:00"}]}
                }}
            }}),
        ),
        row(
            "DeleteCampaignCommunicationTime",
            "DELETE",
            "/communication-time?config=TELEPHONY",
            Value::Null,
        ),
    ]
}

/// The configuration a case's campaign is created carrying, so a write that slipped
/// past a refusal would show in `DescribeCampaign` whichever operation made it.
fn created_configuration() -> Value {
    json!({
        "source": {
            "customerProfilesSegmentArn":
                "arn:aws:profile:us-east-1:000000000000:segment/as-created"
        },
        "connectCampaignFlowArn":
            "arn:aws:connect:us-east-1:000000000000:instance/i-0/flow/f-0",
        "entryLimitsConfig": {"maxEntryCount": 9, "minEntryInterval": "P1D"},
        "communicationLimitsOverride": {"allChannelSubtypes": {
            "communicationLimitsList": [
                {"maxCountPerRecipient": 1, "frequency": 1, "unit": "DAY"}
            ]
        }},
        "communicationTimeConfig": {
            "localTimeZoneConfig": {"defaultTimeZone": "America/New_York"},
            "telephony": {"openHours": {
                "dailyHours": {"MONDAY": [{"startTime": "T09:00", "endTime": "T17:00"}]}
            }}
        },
    })
}

/// One configuration call on `id`, as the request the model binds it to.
async fn configure(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    op: &Configure,
) -> (u16, Value) {
    call(
        router,
        region,
        op.method,
        &format!("/v2/campaigns/{id}{}", op.suffix),
        &op.body,
    )
    .await
}

/// A campaign carrying [`created_configuration`], plus `window` as its schedule when
/// given (offsets in seconds from now — a closed window is how a case reaches the
/// state the CLOCK owns), put through `setup`. Hands back the minted id.
async fn configured_campaign(
    router: &Harness,
    region: &str,
    instance: &str,
    name: &str,
    window: Option<(f64, f64)>,
    setup: &[&str],
    case: &str,
) -> String {
    let mut extra = created_configuration();
    if let Some((start, end)) = window {
        extra["schedule"] = json!({"startTime": instant(start), "endTime": instant(end)});
    }
    let id = create_campaign_with(router, region, instance, name, extra).await;
    drive_lifecycle(router, region, &id, setup, case).await;
    id
}

/// A state a campaign still takes a configuration change in, and how a case gets it
/// there: the run window it is created with (`None` for no schedule at all) and the
/// lifecycle actions driven over it.
#[derive(Debug, Clone, Copy)]
struct Configurable {
    state: &'static str,
    window: Option<(f64, f64)>,
    setup: &'static [&'static str],
}

/// Every non-terminal value of the model's `CampaignState` catalog the API can reach:
/// `Initialized` before any action, and the three a lifecycle call writes. `Failed` is
/// the one value no call in this slice reaches.
const CONFIGURABLE: &[Configurable] = &[
    Configurable {
        state: "Initialized",
        window: None,
        setup: &[],
    },
    Configurable {
        state: "Running",
        window: None,
        setup: &["start"],
    },
    Configurable {
        state: "Paused",
        window: None,
        setup: &["start", "pause"],
    },
    Configurable {
        state: "Stopped",
        window: None,
        setup: &["start", "stop"],
    },
    // Started inside a window that is still OPEN: the clock has not finished this
    // campaign, so it is still configurable.
    Configurable {
        state: "Running",
        window: Some((-60.0, 7_200.0)),
        setup: &["start"],
    },
];

/// Every configuration operation refuses a campaign the CLOCK has in a terminal state,
/// with one status, one code, one reported state and one sentence — and writes nothing.
///
/// The state is reached without any call having written it: the campaign was started
/// inside a run window that has since closed, which is the terminal `Completed`.
#[tokio::test]
async fn every_configuration_operation_refuses_a_terminal_campaign_alike() {
    let region = "us-west-2";
    let instance = "configuration-one-rule";
    let router = router();

    for op in configuration_operations() {
        let case = format!("{} on a Completed campaign", op.operation);
        let id = configured_campaign(
            &router,
            region,
            instance,
            &format!("one-rule-terminal-{}", op.operation),
            Some((-7_200.0, -3_600.0)),
            &["start"],
            &case,
        )
        .await;
        let state = campaign_state(&router, region, &id, &case).await;
        assert_eq!(
            state, "Completed",
            "case {case:?}: expected the closed run window to leave the campaign Completed, \
             got {state:?}"
        );
        let before = describe_campaign(&router, region, &id, &case).await;

        let (status, body) = configure(&router, region, &id, &op).await;
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
            Some("Completed"),
            "case {case:?}: expected the refusal to name state \"Completed\", got {:?}",
            body["state"]
        );
        let expected_message = "A campaign in state Completed cannot be reconfigured";
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message),
            "case {case:?}: expected {expected_message:?}, got {:?}",
            body["message"]
        );

        let after = describe_campaign(&router, region, &id, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refusal must write nothing, got {after}"
        );
    }
}

/// Every configuration operation is SERVED for a campaign in any state that still
/// takes the change, and the call moves no lifecycle.
#[tokio::test]
async fn every_configuration_operation_serves_a_configurable_campaign() {
    let region = "eu-west-1";
    let instance = "configuration-one-rule-served";
    let router = router();

    for op in configuration_operations() {
        for (index, row) in CONFIGURABLE.iter().enumerate() {
            let case = format!(
                "{} on a {} campaign (row {index})",
                op.operation, row.state
            );
            let id = configured_campaign(
                &router,
                region,
                instance,
                &format!("one-rule-served-{}-{index}", op.operation),
                row.window,
                row.setup,
                &case,
            )
            .await;
            let before = campaign_state(&router, region, &id, &case).await;
            assert_eq!(
                before, row.state,
                "case {case:?}: expected the setup to leave the campaign at {:?}, \
                 got {before:?}",
                row.state
            );

            let (status, body) = configure(&router, region, &id, &op).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            let after = campaign_state(&router, region, &id, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: a configuration change moves no lifecycle, got {after:?}"
            );
        }
    }
}

/// An id no campaign of the request's region answers to is the same refusal from every
/// configuration operation, over several well-formed ids — so the refusal is "no such
/// campaign" rather than anything about one id's shape, and a campaign of another
/// region is exactly that: absent here, and untouched.
#[tokio::test]
async fn every_configuration_operation_refuses_an_unknown_id_alike() {
    let home = "us-east-1";
    let away = "ap-south-1";
    let instance = "configuration-one-rule-404";
    let router = router();
    let kept = configured_campaign(&router, home, instance, "kept", None, &[], "the kept campaign")
        .await;
    let before = describe_campaign(&router, home, &kept, "the kept campaign").await;

    // Each row is within `CampaignId`'s pattern `^[-:/a-zA-Z0-9]+$` and its 1..256
    // length bound (source: the operations' `id` member), carries no `/` — the id
    // travels as one path label — and none of them is the id the create above minted.
    let unknown = [
        ("a minted-shaped id", "000000009999".to_string()),
        ("a word", "no-such-campaign".to_string()),
        ("a single character", "x".to_string()),
        ("a colon-shaped id", "campaign:42".to_string()),
        ("the longest id the model takes", "a".repeat(256)),
        // The campaign another region holds: absent from this one.
        ("a campaign of another region", kept.clone()),
    ];

    for op in configuration_operations() {
        for (id_case, id) in &unknown {
            let case = format!("{} on {id_case}", op.operation);
            let region = if id_case == &"a campaign of another region" {
                away
            } else {
                home
            };
            let (status, body) = configure(&router, region, id, &op).await;
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

    // The refusals changed nothing: the campaign the home region does hold is exactly
    // as its create left it.
    let after = describe_campaign(&router, home, &kept, "the kept campaign").await;
    assert_eq!(
        after, before,
        "case \"the kept campaign\": the refusals must write nothing, got {after}"
    );
}
