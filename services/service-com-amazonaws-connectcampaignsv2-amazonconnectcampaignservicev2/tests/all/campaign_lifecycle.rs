// SPDX-License-Identifier: CC0-1.0

//! The campaign lifecycle, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled
//! constraints and the handler together. The rules under test:
//!
//! * a created campaign is STORED — every member of it is read back by
//!   `DescribeCampaign`, and the response ARN is the one the create response gave;
//! * `CampaignSummary$channelSubtypes` is DERIVED from the stored
//!   `channelSubtypeConfig`, one entry per channel block present;
//! * `GetCampaignState` reports the state the last legal `Start`/`Pause`/`Stop`
//!   wrote, and an illegal transition is refused with HTTP 409
//!   `InvalidCampaignStateException` leaving the state alone;
//! * paging over any page size walks every campaign exactly once, with no
//!   `nextToken` on the last page;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException` on every
//!   campaign operation, including after `DeleteCampaign`.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    CampaignAction, CampaignState, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};

/// Onboard the Connect instance a `CreateCampaign` body names, so the create has a
/// campaign target: `CreateCampaign` mints a campaign only for an instance whose
/// onboarding job has SUCCEEDED, and that job settles one
/// [`ONBOARDING_JOB_DURATION`] after `StartInstanceOnboardingJob`. Idempotent and
/// cheap to repeat: an instance already onboarded is left alone, so the wait is paid
/// once per instance rather than once per create.
async fn onboard_for(router: &MultiServiceRouter, region: &str, body: &Value) {
    let Some(instance) = body.get("connectInstanceId").and_then(Value::as_str) else {
        return;
    };
    let (status, status_body) = call(
        router,
        region,
        "GET",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &Value::Null,
    )
    .await;
    if status == 200
        && status_body
            .pointer("/connectInstanceOnboardingJobStatus/status")
            .and_then(Value::as_str)
            == Some("SUCCEEDED")
    {
        return;
    }
    let (status, started) = call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartInstanceOnboardingJob {instance}: expected 200, got {status} {started}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(100)).await;
}

/// `CreateCampaign`, asserting it succeeded, and returning the response body. The
/// instance the body names is onboarded first (see [`onboard_for`]) — a campaign
/// belongs to an onboarded Connect instance, so a create without one is a 404.
async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> Value {
    onboard_for(router, region, body).await;
    let (status, response) = call(router, region, "PUT", "/v2/campaigns", body).await;
    assert_eq!(
        status, 200,
        "CreateCampaign {body}: expected 200, got {status} {response}"
    );
    response
}

/// A minimal telephony `channelSubtypeConfig`: `outboundMode` and
/// `defaultOutboundConfig` are both required by the model.
fn telephony_config() -> Value {
    json!({
        "telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        }
    })
}

fn sms_config() -> Value {
    json!({
        "sms": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-1",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-1"
            }
        }
    })
}

fn email_config() -> Value {
    json!({
        "email": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourceEmailAddress": "campaigns@example.com",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-2"
            }
        }
    })
}

fn whatsapp_config() -> Value {
    json!({
        "whatsApp": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-2",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-3"
            }
        }
    })
}

/// Merge two JSON objects into one request body.
fn merged(base: &Value, extra: &Value) -> Value {
    let mut out = base.clone();
    let target = out.as_object_mut().expect("a JSON object body");
    for (key, value) in extra.as_object().expect("a JSON object overlay") {
        target.insert(key.clone(), value.clone());
    }
    out
}

/// The `CampaignState` variants this crate stores render exactly the wire values
/// the model's `CampaignState` enum declares — the catalog is
/// `ops::types::CAMPAIGN_STATE_VALUES`, taken from the Smithy model, not retyped
/// here.
#[test]
fn every_stored_state_renders_a_modelled_campaign_state_value() {
    use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::CAMPAIGN_STATE_VALUES;

    let variants = [
        CampaignState::Completed,
        CampaignState::Failed,
        CampaignState::Initialized,
        CampaignState::Paused,
        CampaignState::Running,
        CampaignState::Stopped,
    ];
    for variant in variants {
        assert!(
            CAMPAIGN_STATE_VALUES.contains(&variant.as_wire()),
            "case {variant:?}: expected a value in {CAMPAIGN_STATE_VALUES:?}, got {:?}",
            variant.as_wire()
        );
    }
    let mut rendered: Vec<&str> = variants.iter().map(|v| v.as_wire()).collect();
    rendered.sort_unstable();
    assert_eq!(
        rendered, CAMPAIGN_STATE_VALUES,
        "case CampaignState: expected the model's whole value set, got {rendered:?}"
    );
}

/// The transition table, over every state and every action: only the legal pairs
/// move, and each moves to exactly one state.
#[test]
fn the_transition_table_moves_only_on_a_legal_action() {
    // (from, action, expected next state or None when the action is refused)
    let cases: &[(CampaignState, CampaignAction, Option<CampaignState>)] = &[
        (
            CampaignState::Initialized,
            CampaignAction::Start,
            Some(CampaignState::Running),
        ),
        (
            CampaignState::Paused,
            CampaignAction::Start,
            Some(CampaignState::Running),
        ),
        (
            CampaignState::Stopped,
            CampaignAction::Start,
            Some(CampaignState::Running),
        ),
        (CampaignState::Running, CampaignAction::Start, None),
        (CampaignState::Completed, CampaignAction::Start, None),
        (CampaignState::Failed, CampaignAction::Start, None),
        (
            CampaignState::Running,
            CampaignAction::Pause,
            Some(CampaignState::Paused),
        ),
        (CampaignState::Initialized, CampaignAction::Pause, None),
        (CampaignState::Paused, CampaignAction::Pause, None),
        (CampaignState::Stopped, CampaignAction::Pause, None),
        (CampaignState::Completed, CampaignAction::Pause, None),
        (CampaignState::Failed, CampaignAction::Pause, None),
        (
            CampaignState::Initialized,
            CampaignAction::Stop,
            Some(CampaignState::Stopped),
        ),
        (
            CampaignState::Running,
            CampaignAction::Stop,
            Some(CampaignState::Stopped),
        ),
        (
            CampaignState::Paused,
            CampaignAction::Stop,
            Some(CampaignState::Stopped),
        ),
        (CampaignState::Stopped, CampaignAction::Stop, None),
        (CampaignState::Completed, CampaignAction::Stop, None),
        (CampaignState::Failed, CampaignAction::Stop, None),
    ];

    for (from, action, expected) in cases {
        let actual = from.after(*action);
        assert_eq!(
            actual, *expected,
            "case {from:?} under {action:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// Create then describe, over a TABLE of campaigns that differ in which optional
/// config blocks, channel subtypes, type and tags they carry: every member supplied
/// comes back from the store unchanged, and the server-owned id/ARN agree between
/// the two responses.
#[tokio::test]
async fn create_then_describe_returns_every_stored_member() {
    let region = "us-west-2";
    let router = router();

    let cases: Vec<(&str, Value)> = vec![
        (
            "telephony only, no optional blocks",
            json!({
                "name": "demo-campaign",
                "connectInstanceId": "instance-1",
                "channelSubtypeConfig": telephony_config(),
            }),
        ),
        (
            "managed, tags, schedule and entry limits",
            json!({
                "name": "managed-with-schedule",
                "connectInstanceId": "instance_2.a",
                "type": "MANAGED",
                "channelSubtypeConfig": merged(&telephony_config(), &sms_config()),
                // `Schedule$startTime` / `$endTime` target `TimeStamp`, which the
                // model marks `@timestampFormat("date-time")`, so each is an RFC 3339
                // instant on the wire — the form every AWS SDK sends and expects back.
                "schedule": {
                    "startTime": "2026-01-01T00:00:00Z",
                    "endTime": "2026-02-01T23:59:59Z",
                    "refreshFrequency": "P1D"
                },
                "entryLimitsConfig": {"maxEntryCount": 7, "minEntryInterval": "P1D"},
                "tags": {"env": "test", "team": "outbound"},
            }),
        ),
        (
            "journey, every channel subtype, source and flow arn",
            json!({
                "name": "journey-all-channels",
                "connectInstanceId": "instance-3",
                "type": "JOURNEY",
                "channelSubtypeConfig": merged(
                    &merged(&telephony_config(), &sms_config()),
                    &merged(&email_config(), &whatsapp_config()),
                ),
                "source": {
                    "customerProfilesSegmentArn":
                        "arn:aws:profile:us-west-2:000000000000:domains/d1/segment-definitions/s1"
                },
                "connectCampaignFlowArn":
                    "arn:aws:connect:us-west-2:000000000000:instance/i-3/flow/f-3",
                "communicationLimitsOverride": {
                    "allChannelSubtypes": {
                        "communicationLimitsList": [
                            {"maxCountPerRecipient": 3, "frequency": 1, "unit": "DAY"}
                        ]
                    },
                    "instanceLimitsHandling": "OPT_OUT"
                },
                "communicationTimeConfig": {
                    "localTimeZoneConfig": {
                        "defaultTimeZone": "America/Los_Angeles",
                        "localTimeZoneDetection": ["ZIP_CODE"],
                        "localTimeZoneDetectionScope": "PRIMARY_ONLY"
                    },
                    "telephony": {
                        "openHours": {
                            "dailyHours": {
                                "MONDAY": [{"startTime": "T09:00", "endTime": "T17:00"}]
                            }
                        }
                    }
                },
            }),
        ),
    ];

    for (case, request) in cases {
        let created = create(&router, region, &request).await;
        let id = created["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !id.is_empty(),
            "case {case:?}: expected a minted id, got {created}"
        );
        let expected_arn = format!(
            "arn:aws:connect-campaigns:{region}:{}:campaign/{id}",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            created["arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected arn {expected_arn:?}, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created.get("tags").cloned().unwrap_or(Value::Null),
            request.get("tags").cloned().unwrap_or(Value::Null),
            "case {case:?}: expected the stored tags back, got {:?}",
            created.get("tags")
        );

        let (status, described) =
            call(&router, region, "GET", &format!("/v2/campaigns/{id}"), &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeCampaign expected 200, got {status} {described}"
        );
        let campaign = &described["campaign"];
        assert_eq!(
            campaign["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected campaign.id {id:?}, got {:?}",
            campaign["id"]
        );
        assert_eq!(
            campaign["arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected campaign.arn {expected_arn:?}, got {:?}",
            campaign["arn"]
        );
        // Every member the request carried must come back byte-identical.
        for (member, sent) in request.as_object().expect("an object request") {
            assert_eq!(
                &campaign[member.as_str()],
                sent,
                "case {case:?}: expected campaign.{member} {sent}, got {}",
                campaign[member.as_str()]
            );
        }
    }
}

/// `channelSubtypes` on a summary is derived from the stored config, not echoed:
/// one entry per channel block present, unique, in the model's `ChannelSubtype`
/// catalog order.
#[tokio::test]
async fn list_derives_channel_subtypes_from_the_stored_config() {
    let region = "eu-west-1";

    // (case, channelSubtypeConfig, expected channelSubtypes)
    let cases: Vec<(&str, Option<Value>, Vec<&str>)> = vec![
        ("no config at all", None, vec![]),
        ("telephony", Some(telephony_config()), vec!["TELEPHONY"]),
        ("sms", Some(sms_config()), vec!["SMS"]),
        ("email", Some(email_config()), vec!["EMAIL"]),
        ("whatsApp", Some(whatsapp_config()), vec!["WHATSAPP"]),
        (
            "sms and telephony",
            Some(merged(&telephony_config(), &sms_config())),
            vec!["SMS", "TELEPHONY"],
        ),
        (
            "all four channels",
            Some(merged(
                &merged(&telephony_config(), &sms_config()),
                &merged(&email_config(), &whatsapp_config()),
            )),
            vec!["EMAIL", "SMS", "TELEPHONY", "WHATSAPP"],
        ),
    ];

    for (case, config, expected) in cases {
        // One campaign per case, in its own store, so the single summary returned
        // is the one under test.
        let router = router();
        let mut request = json!({"name": case, "connectInstanceId": "instance-1"});
        if let Some(config) = config {
            request
                .as_object_mut()
                .expect("an object request")
                .insert("channelSubtypeConfig".to_string(), config);
        }
        create(&router, region, &request).await;

        let (status, listed) = call(
            &router,
            region,
            "POST",
            "/v2/campaigns-summary",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCampaigns expected 200, got {status} {listed}"
        );
        let actual: Vec<&str> = listed["campaignSummaryList"][0]["channelSubtypes"]
            .as_array()
            .map(|values| values.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected channelSubtypes {expected:?}, got {actual:?}"
        );
    }
}

/// A campaign starts `Initialized`, and each legal action is reported by
/// `GetCampaignState`; each illegal one answers 409 `InvalidCampaignStateException`
/// and leaves the state exactly where it was. The sequence covers every legal
/// transition and every refusal reachable through the API.
#[tokio::test]
async fn get_campaign_state_reports_what_the_lifecycle_calls_wrote() {
    let region = "ap-south-1";
    let router = router();
    let created = create(
        &router,
        region,
        &json!({
            "name": "lifecycle",
            "connectInstanceId": "instance-1",
            "channelSubtypeConfig": telephony_config(),
        }),
    )
    .await;
    let id = created["id"].as_str().expect("a minted id").to_string();

    async fn state(router: &MultiServiceRouter, region: &str, id: &str, case: &str) -> String {
        let (status, body) = call(
            router,
            region,
            "GET",
            &format!("/v2/campaigns/{id}/state"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCampaignState expected 200, got {status} {body}"
        );
        body["state"]
            .as_str()
            .unwrap_or("<no state>")
            .to_string()
    }

    assert_eq!(
        state(&router, region, &id, "freshly created").await,
        "Initialized",
        "case \"freshly created\": expected Initialized"
    );

    // (action, expected status, expected state afterwards)
    let steps: &[(&str, u16, &str)] = &[
        ("pause", 409, "Initialized"),
        ("start", 200, "Running"),
        ("start", 409, "Running"),
        ("pause", 200, "Paused"),
        ("pause", 409, "Paused"),
        ("start", 200, "Running"),
        ("stop", 200, "Stopped"),
        ("stop", 409, "Stopped"),
        ("pause", 409, "Stopped"),
        ("start", 200, "Running"),
    ];

    for (action, expected_status, expected_state) in steps {
        let case = format!("{action} from {}", state(&router, region, &id, action).await);
        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/campaigns/{id}/{action}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if *expected_status == 409 {
            assert!(
                error_code(&body).ends_with("InvalidCampaignStateException"),
                "case {case:?}: expected InvalidCampaignStateException, got {}",
                error_code(&body)
            );
            assert_eq!(
                body["state"].as_str(),
                Some(*expected_state),
                "case {case:?}: expected the refusal to name state {expected_state:?}, got {:?}",
                body["state"]
            );
        }
        let actual = state(&router, region, &id, &case).await;
        assert_eq!(
            actual, *expected_state,
            "case {case:?}: expected state {expected_state:?} afterwards, got {actual:?}"
        );
    }
}

/// Paging with any page size walks every campaign exactly once, in creation order,
/// and the last page carries no `nextToken`.
#[tokio::test]
async fn pagination_walks_every_campaign_exactly_once() {
    let region = "us-east-2";
    const CAMPAIGNS: usize = 7;

    for page_size in [1_usize, 2, 3, 6, 7, 8, 50] {
        let router = router();
        let mut created = Vec::new();
        for index in 0..CAMPAIGNS {
            let response = create(
                &router,
                region,
                &json!({
                    "name": format!("campaign-{index}"),
                    "connectInstanceId": "instance-1",
                    "channelSubtypeConfig": telephony_config(),
                }),
            )
            .await;
            created.push(response["id"].as_str().expect("a minted id").to_string());
        }

        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= CAMPAIGNS + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = token.as_ref() {
                request
                    .as_object_mut()
                    .expect("an object request")
                    .insert("nextToken".to_string(), json!(token));
            }
            let (status, body) =
                call(&router, region, "POST", "/v2/campaigns-summary", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListCampaigns expected 200, got {status} {body}"
            );
            let page = body["campaignSummaryList"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} summaries, got {}",
                page.len()
            );
            for summary in &page {
                walked.push(
                    summary["id"]
                        .as_str()
                        .unwrap_or("<no id>")
                        .to_string(),
                );
            }
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            walked, created,
            "case {case:?}: expected every campaign once in creation order, got {walked:?}"
        );
    }
}

/// `maxResults` smaller than the result set yields a token; the final page never
/// does. A token this service did not mint is refused as a bad request.
#[tokio::test]
async fn a_bogus_next_token_is_refused() {
    let region = "us-east-1";
    let router = router();
    create(
        &router,
        region,
        &json!({"name": "only", "connectInstanceId": "instance-1"}),
    )
    .await;

    // (case, token)
    let cases: &[(&str, &str)] = &[
        ("not hex at all", "not-a-cursor"),
        ("hex of text that is not a cursor", "6e6f742d68657265"),
        ("hex of empty text", ""),
        ("hex of a negative number", "2d31"),
    ];
    for (case, token) in cases {
        let (status, body) = call(
            &router,
            region,
            "POST",
            "/v2/campaigns-summary",
            &json!({"nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}

/// A campaign deleted between two pages does not abort an in-flight walk: the
/// cursor is a creation position, so the next page is simply what was created
/// after it. Covers deleting a campaign on the page already returned and one still
/// to come.
#[tokio::test]
async fn a_delete_between_pages_does_not_abort_the_walk() {
    let region = "us-east-1";
    const CAMPAIGNS: usize = 5;

    // (case, which campaign to delete after the first page of 2)
    for (case, doomed_index) in [
        ("delete one already returned", 0_usize),
        ("delete the one just returned", 1),
        ("delete the next one up", 2),
        ("delete the last one", CAMPAIGNS - 1),
    ] {
        let router = router();
        let mut ids = Vec::new();
        for index in 0..CAMPAIGNS {
            let created = create(
                &router,
                region,
                &json!({
                    "name": format!("campaign-{index}"),
                    "connectInstanceId": "instance-1",
                }),
            )
            .await;
            ids.push(created["id"].as_str().expect("a minted id").to_string());
        }

        let (status, first) = call(
            &router,
            region,
            "POST",
            "/v2/campaigns-summary",
            &json!({"maxResults": 2}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: first page expected 200, got {status} {first}"
        );
        let token = first["nextToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !token.is_empty(),
            "case {case:?}: expected a nextToken after the first page, got {first}"
        );
        let mut walked: Vec<String> = first["campaignSummaryList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|summary| summary["id"].as_str().map(str::to_string))
            .collect();

        let doomed = ids[doomed_index].clone();
        let (status, body) = call(
            &router,
            region,
            "DELETE",
            &format!("/v2/campaigns/{doomed}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteCampaign expected 200, got {status} {body}"
        );

        let mut token = Some(token);
        while let Some(cursor) = token.take() {
            let (status, body) = call(
                &router,
                region,
                "POST",
                "/v2/campaigns-summary",
                &json!({"maxResults": 2, "nextToken": cursor}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a page after the delete expected 200, got {status} {body}"
            );
            for summary in body["campaignSummaryList"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                walked.push(
                    summary["id"]
                        .as_str()
                        .unwrap_or("<no id>")
                        .to_string(),
                );
            }
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
        }

        // Everything still stored when its page was fetched is walked exactly
        // once: the deleted campaign only disappears if its page came after the
        // delete.
        let mut expected: Vec<String> = ids.clone();
        if doomed_index >= 2 {
            expected.retain(|id| id != &doomed);
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the walk to cover {expected:?}, got {walked:?}"
        );
    }
}

/// The instance-id filter selects exactly the campaigns of that Connect instance,
/// over several instances with different campaign counts.
#[tokio::test]
async fn the_instance_id_filter_selects_only_that_instance() {
    let region = "ca-central-1";
    let router = router();

    // (connectInstanceId, how many campaigns it owns)
    let population: &[(&str, usize)] = &[("instance-a", 3), ("instance.b", 1), ("instance_c", 2)];
    for (instance, count) in population {
        for index in 0..*count {
            create(
                &router,
                region,
                &json!({
                    "name": format!("{instance}-{index}"),
                    "connectInstanceId": instance,
                    "channelSubtypeConfig": telephony_config(),
                }),
            )
            .await;
        }
    }

    for (instance, count) in population {
        let (status, body) = call(
            &router,
            region,
            "POST",
            "/v2/campaigns-summary",
            &json!({"filters": {"instanceIdFilter": {"value": instance, "operator": "Eq"}}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {instance:?}: ListCampaigns expected 200, got {status} {body}"
        );
        let summaries = body["campaignSummaryList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            *count,
            "case {instance:?}: expected {count} summaries, got {}",
            summaries.len()
        );
        for summary in &summaries {
            assert_eq!(
                summary["connectInstanceId"].as_str(),
                Some(*instance),
                "case {instance:?}: expected only that instance, got {:?}",
                summary["connectInstanceId"]
            );
        }
    }

    // An instance with no campaigns is an empty page, not an error.
    let (status, body) = call(
        &router,
        region,
        "POST",
        "/v2/campaigns-summary",
        &json!({"filters": {"instanceIdFilter": {"value": "instance-none", "operator": "Eq"}}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"instance-none\": expected 200, got {status} {body}"
    );
    assert_eq!(
        body["campaignSummaryList"].as_array().map(Vec::len),
        Some(0),
        "case \"instance-none\": expected no summaries, got {}",
        body["campaignSummaryList"]
    );
}

/// Delete removes the campaign from every read: describe and state answer 404, the
/// list no longer carries it, and deleting it again is 404 too. The campaigns
/// beside it are untouched.
#[tokio::test]
async fn delete_removes_the_campaign_from_every_read() {
    let region = "us-east-1";
    let router = router();

    let mut ids = Vec::new();
    for index in 0..3 {
        let created = create(
            &router,
            region,
            &json!({
                "name": format!("campaign-{index}"),
                "connectInstanceId": "instance-1",
                "channelSubtypeConfig": telephony_config(),
            }),
        )
        .await;
        ids.push(created["id"].as_str().expect("a minted id").to_string());
    }
    let doomed = ids.remove(1);

    let (status, body) = call(
        &router,
        region,
        "DELETE",
        &format!("/v2/campaigns/{doomed}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete the middle campaign\": expected 200, got {status} {body}"
    );

    // (case, method, path)
    let reads: Vec<(String, &str, String)> = vec![
        (
            "DescribeCampaign after delete".to_string(),
            "GET",
            format!("/v2/campaigns/{doomed}"),
        ),
        (
            "GetCampaignState after delete".to_string(),
            "GET",
            format!("/v2/campaigns/{doomed}/state"),
        ),
        (
            "StartCampaign after delete".to_string(),
            "POST",
            format!("/v2/campaigns/{doomed}/start"),
        ),
        (
            "PauseCampaign after delete".to_string(),
            "POST",
            format!("/v2/campaigns/{doomed}/pause"),
        ),
        (
            "StopCampaign after delete".to_string(),
            "POST",
            format!("/v2/campaigns/{doomed}/stop"),
        ),
        (
            "DeleteCampaign after delete".to_string(),
            "DELETE",
            format!("/v2/campaigns/{doomed}"),
        ),
    ];
    for (case, method, path) in &reads {
        let (status, body) = call(&router, region, method, path, &Value::Null).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
    }

    let (status, body) = call(
        &router,
        region,
        "POST",
        "/v2/campaigns-summary",
        &json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"list after delete\": expected 200, got {status} {body}"
    );
    let remaining: Vec<&str> = body["campaignSummaryList"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|summary| summary["id"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        remaining,
        ids.iter().map(String::as_str).collect::<Vec<&str>>(),
        "case \"list after delete\": expected the surviving campaigns in creation order, got {remaining:?}"
    );
}

/// An id no campaign was ever created with is 404 on every campaign operation —
/// over several well-formed ids, so the refusal is "no such campaign" rather than
/// anything about one id's shape.
#[tokio::test]
async fn an_unknown_id_is_404_on_every_campaign_operation() {
    let region = "us-east-1";
    let router = router();

    // `CampaignId` also admits `/`, but an `@httpLabel` segment does not span one,
    // so such an id is not addressable on this route at all; these are the ids a
    // client can actually send.
    for unknown in ["missing", "c0ffee", "A-B:C1", "0"] {
        for (method, suffix) in [
            ("GET", ""),
            ("GET", "/state"),
            ("POST", "/start"),
            ("POST", "/pause"),
            ("POST", "/stop"),
            ("DELETE", ""),
        ] {
            let case = format!("{method} /v2/campaigns/{unknown}{suffix}");
            let (status, body) = call(
                &router,
                region,
                method,
                &format!("/v2/campaigns/{unknown}{suffix}"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {}",
                error_code(&body)
            );
        }
    }
}

/// Two creates never share an id or an ARN, and each record keeps its own members:
/// the store is keyed by the minted id, not by anything in the request.
#[tokio::test]
async fn each_create_mints_a_distinct_id_and_keeps_its_own_record() {
    let region = "us-east-1";
    let router = router();

    // The same request body twice: only the server-minted id distinguishes them.
    let request = json!({
        "name": "same-name",
        "connectInstanceId": "instance-1",
        "channelSubtypeConfig": telephony_config(),
        "tags": {"env": "test"},
    });

    let first = create(&router, region, &request).await;
    let second = create(&router, region, &request).await;
    assert_ne!(
        first["id"], second["id"],
        "case \"two identical creates\": expected distinct ids, got {first} and {second}"
    );
    assert_ne!(
        first["arn"], second["arn"],
        "case \"two identical creates\": expected distinct arns, got {first} and {second}"
    );

    // Moving one campaign's state leaves the other where it was.
    let first_id = first["id"].as_str().expect("a minted id");
    let second_id = second["id"].as_str().expect("a minted id");
    let (status, body) = call(
        &router,
        region,
        "POST",
        &format!("/v2/campaigns/{first_id}/start"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"start the first campaign\": expected 200, got {status} {body}"
    );
    for (case, id, expected) in [
        ("the started campaign", first_id, "Running"),
        ("its untouched twin", second_id, "Initialized"),
    ] {
        let (status, body) = call(
            &router,
            region,
            "GET",
            &format!("/v2/campaigns/{id}/state"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCampaignState expected 200, got {status} {body}"
        );
        assert_eq!(
            body["state"].as_str(),
            Some(expected),
            "case {case:?}: expected state {expected:?}, got {:?}",
            body["state"]
        );
    }
}

/// The store does not grow without bound: once it holds `MAX_CAMPAIGNS` campaigns a
/// further create is refused with the modelled `ServiceQuotaExceededException`
/// (HTTP 402) and nothing is stored, and deleting one makes room for exactly one
/// more.
#[tokio::test]
async fn creates_past_the_store_ceiling_are_refused_as_a_quota_breach() {
    use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::MAX_CAMPAIGNS;

    let region = "us-east-1";
    let router = router();
    let request = json!({"name": "filler", "connectInstanceId": "instance-1"});

    let mut first_id = String::new();
    for index in 0..MAX_CAMPAIGNS {
        let created = create(&router, region, &request).await;
        if index == 0 {
            first_id = created["id"].as_str().expect("a minted id").to_string();
        }
    }

    // (case, how many campaigns the store holds, expected status)
    let (status, body) = call(&router, region, "PUT", "/v2/campaigns", &request).await;
    assert_eq!(
        status, 402,
        "case \"create at the ceiling\": expected HTTP 402, got {status} {body}"
    );
    assert!(
        error_code(&body).ends_with("ServiceQuotaExceededException"),
        "case \"create at the ceiling\": expected ServiceQuotaExceededException, got {}",
        error_code(&body)
    );

    // The refusal stored nothing: the page count is unchanged, and freeing one slot
    // admits exactly one more create before the ceiling bites again.
    let (status, body) = call(
        &router,
        region,
        "DELETE",
        &format!("/v2/campaigns/{first_id}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"delete to free a slot\": expected 200, got {status} {body}"
    );
    create(&router, region, &request).await;
    let (status, body) = call(&router, region, "PUT", "/v2/campaigns", &request).await;
    assert_eq!(
        status, 402,
        "case \"create at the ceiling again\": expected HTTP 402, got {status} {body}"
    );
}

/// Reads do not change what a later caller sees: describing, listing and reading the
/// state repeatedly leaves the campaign's state and the result set exactly as they
/// were.
#[tokio::test]
async fn reads_do_not_change_what_a_later_caller_sees() {
    let region = "us-east-1";
    let router = router();
    let created = create(
        &router,
        region,
        &json!({
            "name": "read-me",
            "connectInstanceId": "instance-1",
            "channelSubtypeConfig": telephony_config(),
        }),
    )
    .await;
    let id = created["id"].as_str().expect("a minted id").to_string();

    let mut described: Vec<Value> = Vec::new();
    let mut listed: Vec<Value> = Vec::new();
    let mut states: Vec<Value> = Vec::new();
    for round in 0..3 {
        let case = format!("round {round}");
        let (status, body) =
            call(&router, region, "GET", &format!("/v2/campaigns/{id}"), &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeCampaign expected 200, got {status} {body}"
        );
        described.push(body);
        let (status, body) =
            call(&router, region, "POST", "/v2/campaigns-summary", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCampaigns expected 200, got {status} {body}"
        );
        listed.push(body);
        let (status, body) = call(
            &router,
            region,
            "GET",
            &format!("/v2/campaigns/{id}/state"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCampaignState expected 200, got {status} {body}"
        );
        states.push(body);
    }

    for (round, triple) in described.iter().zip(&listed).zip(&states).enumerate() {
        let ((describe, list), state) = triple;
        assert_eq!(
            describe, &described[0],
            "case \"describe round {round}\": expected the first read's answer, got {describe}"
        );
        assert_eq!(
            list, &listed[0],
            "case \"list round {round}\": expected the first read's answer, got {list}"
        );
        assert_eq!(
            state["state"].as_str(),
            Some("Initialized"),
            "case \"state round {round}\": expected Initialized, got {state}"
        );
    }
}
