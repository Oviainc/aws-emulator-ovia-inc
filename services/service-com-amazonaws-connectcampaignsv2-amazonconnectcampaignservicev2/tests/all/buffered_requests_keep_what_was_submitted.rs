// SPDX-License-Identifier: CC0-1.0

//! What a campaign's request buffers KEEP: a request taken into a buffer is stored as
//! it was submitted.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handler together, and then reads the record the call wrote straight out of the
//! store — which is the only reader there is, since no modelled operation of this
//! service reports a pending request back.
//!
//! The rule under test, over both batch operations:
//!
//! * a request a campaign takes is recorded with every member it carried, so the
//!   buffer says what each pending request is FOR and not merely that one is pending —
//!   the whole `OutboundRequest$channelSubtypeParameters` union for a dial request
//!   (who it reaches, on which channel, with which attributes, source number, template
//!   and call options) and the whole `ProfileOutboundRequest$eventTriggerContext` for a
//!   profile request (its `sourceEvent` and its `channelContext` alike);
//! * the channel a dial request is for is DERIVED from the stored parameters rather
//!   than kept beside them, so the record cannot disagree with itself.
//!
//! The tables below span what the rule generalises over: which member of the union is
//! in force, which optional members of that block the request fills, and the size and
//! content of the attribute and template maps it carries — including each published
//! bound of `RingTimeout`'s `@range` and of `SessionId`'s `@length`.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    ChannelSubtypeParameters, EventTriggerContext,
};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State;

use super::common::{call, create_campaign_with, drive_lifecycle, instant, router_with_engine, Harness};

/// How far ahead of now every request below expires: far enough that no row is weighed
/// as expired, which is a rule of its own (`outbound_request_expiration.rs`).
const LIVE_FOR_SECS: f64 = 3_600.0;

/// A `channelSubtypeConfig` configuring all four channels, so a campaign built with it
/// takes a request on any of them.
///
/// The required members of each block come from the model: `outboundMode` and
/// `defaultOutboundConfig` on every channel block, `connectContactFlowId` on
/// `TelephonyOutboundConfig`, `connectSourcePhoneNumberArn` + `wisdomTemplateArn` on
/// `SmsOutboundConfig` and `WhatsAppOutboundConfig`, and `connectSourceEmailAddress` +
/// `wisdomTemplateArn` on `EmailOutboundConfig`.
fn every_channel_config() -> Value {
    json!({
        "telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        },
        "sms": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-1",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-1"
            }
        },
        "email": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {
                "connectSourceEmailAddress": "campaigns@example.com",
                "wisdomTemplateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-2"
            }
        },
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

/// A campaign on a freshly onboarded instance, configured for every channel and
/// started, so it takes a batch on any of them. Returns its id.
async fn running_campaign(router: &Harness, region: &str, instance: &str) -> String {
    let id = create_campaign_with(
        router,
        region,
        instance,
        &format!("dialler-for-{instance}"),
        json!({"channelSubtypeConfig": every_channel_config()}),
    )
    .await;
    drive_lifecycle(router, region, &id, &["start"], "start the campaign").await;
    id
}

/// A stored map as the JSON object the request carried it in.
fn stored_map(entries: &HashMap<String, String>) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// Put `value` on `object` under `member` when the record holds one, so an optional
/// member is absent from the projection exactly when it is absent from the record.
fn set_if_present(object: &mut Value, member: &str, value: Option<Value>) {
    if let Some(value) = value {
        object[member] = value;
    }
}

/// The stored `channelSubtypeParameters` as the JSON the request carried: the union's
/// member name, then every member of that block the record holds.
///
/// Member names, requiredness and optionality are the model's
/// (`smithy_query(query="shape", shape="TelephonyChannelSubtypeParameters")` and its
/// SMS, email and WhatsApp siblings), so this projection is the submitted body again
/// whenever nothing was dropped.
fn parameters_as_submitted(parameters: &ChannelSubtypeParameters) -> Value {
    match parameters {
        ChannelSubtypeParameters::Telephony(telephony) => {
            let mut block = json!({
                "destinationPhoneNumber": telephony.destination_phone_number,
                "attributes": stored_map(&telephony.attributes),
            });
            set_if_present(
                &mut block,
                "connectSourcePhoneNumber",
                telephony
                    .connect_source_phone_number
                    .clone()
                    .map(Value::from),
            );
            set_if_present(
                &mut block,
                "answerMachineDetectionConfig",
                telephony
                    .answer_machine_detection_config
                    .as_ref()
                    .map(|detection| {
                        let mut config = json!({
                            "enableAnswerMachineDetection":
                                detection.enable_answer_machine_detection,
                        });
                        set_if_present(
                            &mut config,
                            "awaitAnswerMachinePrompt",
                            detection.await_answer_machine_prompt.map(Value::from),
                        );
                        config
                    }),
            );
            set_if_present(
                &mut block,
                "ringTimeout",
                telephony.ring_timeout.map(Value::from),
            );
            json!({"telephony": block})
        }
        ChannelSubtypeParameters::Sms(sms) => {
            let mut block = json!({
                "destinationPhoneNumber": sms.destination_phone_number,
                "templateParameters": stored_map(&sms.template_parameters),
            });
            set_if_present(
                &mut block,
                "connectSourcePhoneNumberArn",
                sms.connect_source_phone_number_arn.clone().map(Value::from),
            );
            set_if_present(
                &mut block,
                "templateArn",
                sms.template_arn.clone().map(Value::from),
            );
            json!({"sms": block})
        }
        ChannelSubtypeParameters::Email(email) => {
            let mut block = json!({
                "destinationEmailAddress": email.destination_email_address,
                "templateParameters": stored_map(&email.template_parameters),
            });
            set_if_present(
                &mut block,
                "connectSourceEmailAddress",
                email.connect_source_email_address.clone().map(Value::from),
            );
            set_if_present(
                &mut block,
                "templateArn",
                email.template_arn.clone().map(Value::from),
            );
            json!({"email": block})
        }
        ChannelSubtypeParameters::whatsApp(whats_app) => {
            let mut block = json!({
                "destinationPhoneNumber": whats_app.destination_phone_number,
                "templateParameters": stored_map(&whats_app.template_parameters),
            });
            set_if_present(
                &mut block,
                "connectSourcePhoneNumberArn",
                whats_app
                    .connect_source_phone_number_arn
                    .clone()
                    .map(Value::from),
            );
            set_if_present(
                &mut block,
                "templateArn",
                whats_app.template_arn.clone().map(Value::from),
            );
            json!({"whatsApp": block})
        }
    }
}

/// The stored `eventTriggerContext` as the JSON the profile request carried, as
/// [`parameters_as_submitted`] projects a dial request's parameters.
///
/// Member names and optionality are the model's (`smithy_query(query="shape",
/// shape="EventTriggerContext")` and its `ChannelContext` /
/// `WebNotificationContext` closure).
fn context_as_submitted(context: &EventTriggerContext) -> Value {
    let mut object = json!({});
    set_if_present(
        &mut object,
        "sourceEvent",
        context.source_event.clone().map(Value::from),
    );
    set_if_present(
        &mut object,
        "channelContext",
        context.channel_context.as_ref().map(|channel| {
            let mut channel_object = json!({});
            set_if_present(
                &mut channel_object,
                "webNotificationContext",
                channel.web_notification_context.as_ref().map(|web| {
                    let mut web_object = json!({});
                    set_if_present(
                        &mut web_object,
                        "sessionId",
                        web.session_id.clone().map(Value::from),
                    );
                    set_if_present(
                        &mut web_object,
                        "browserId",
                        web.browser_id.clone().map(Value::from),
                    );
                    web_object
                }),
            );
            channel_object
        }),
    );
    object
}

/// What the dial buffer holds for the request taken under `client_token`: its minted
/// id, the `ChannelSubtype` the record derives, and the parameters it kept, projected
/// back into the form they were submitted in.
fn stored_dial_request(
    engine: &engine::InMemoryEngine,
    region: &str,
    campaign: &str,
    client_token: &str,
) -> Option<(String, String, Value)> {
    engine
        .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
        .lock()
        .campaign(region, campaign)
        .expect("the campaign under test")
        .outbound_requests
        .get(client_token)
        .map(|stored| {
            (
                stored.id.clone(),
                stored.channel_subtype().to_string(),
                parameters_as_submitted(&stored.channel_subtype_parameters),
            )
        })
}

/// What the profile buffer holds for the request taken under `client_token`: its
/// minted id, the `profileId` it is to reach, and the `eventTriggerContext` it kept —
/// `None` for the context when the request carried none.
fn stored_profile_request(
    engine: &engine::InMemoryEngine,
    region: &str,
    campaign: &str,
    client_token: &str,
) -> Option<(String, String, Option<Value>)> {
    engine
        .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
        .lock()
        .campaign(region, campaign)
        .expect("the campaign under test")
        .profile_outbound_requests
        .get(client_token)
        .map(|stored| {
            (
                stored.id.clone(),
                stored.profile_id.clone(),
                stored
                    .event_trigger_context
                    .as_ref()
                    .map(context_as_submitted),
            )
        })
}

/// Submit `requests` to `campaign` and return the minted id of every taken request,
/// keyed by its `clientToken`, asserting the batch took all of them.
async fn take_all(
    router: &Harness,
    region: &str,
    campaign: &str,
    path: &str,
    member: &str,
    requests: Vec<Value>,
) -> BTreeMap<String, String> {
    let (status, body) = call(
        router,
        region,
        "PUT",
        &format!("/v2/campaigns/{campaign}/{path}"),
        &json!({member: requests}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {path:?}: expected the batch to be accepted, got {status} {body}"
    );
    assert_eq!(
        body.get("failedRequests").and_then(Value::as_array),
        Some(&Vec::new()),
        "case {path:?}: every request of the batch is submittable, got {body}"
    );
    body.get("successfulRequests")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {path:?}: expected a successfulRequests list, got {body}"))
        .iter()
        .map(|entry| {
            (
                entry
                    .get("clientToken")
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
                    .to_string(),
                entry
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
                    .to_string(),
            )
        })
        .collect()
}

/// One row of the dial-request table: the case name, the `ChannelSubtype` the row is
/// for, and a builder for the `channelSubtypeParameters` it carries — a builder rather
/// than a value, so the body the case submits and the body it asserts against are
/// built by the same code.
type DialCase = (&'static str, &'static str, fn() -> Value);

/// A dial request taken into a campaign's buffer keeps the whole
/// `channelSubtypeParameters` it was submitted with, over a table spanning the four
/// channels of the union, the optional members of each block, and the size of the
/// attribute and template maps.
#[tokio::test]
async fn a_dial_request_keeps_every_channel_subtype_parameter_it_carried() {
    let region = "us-east-1";
    let (router, engine) = router_with_engine();
    let campaign = running_campaign(&router, region, "instance-1").await;

    // (case, the ChannelSubtype the row is for, the parameters it carries)
    let cases: &[DialCase] = &[
        ("telephony, required members only", "TELEPHONY", || {
            json!({"telephony": {
                "destinationPhoneNumber": "+15555550100",
                "attributes": {}
            }})
        }),
        ("telephony, every optional member filled", "TELEPHONY", || {
            json!({"telephony": {
                "destinationPhoneNumber": "+15555550101",
                "attributes": {"CustomerId": "c-1", "Tier": "gold", "Locale": "en-US"},
                "connectSourcePhoneNumber": "+15555550199",
                "answerMachineDetectionConfig": {
                    "enableAnswerMachineDetection": true,
                    "awaitAnswerMachinePrompt": true
                },
                // `RingTimeout`'s @range is 15..60; this row sits at its minimum.
                "ringTimeout": 15
            }})
        }),
        (
            "telephony, ring timeout at the published maximum",
            "TELEPHONY",
            || {
                json!({"telephony": {
                    "destinationPhoneNumber": "+1-555-555-0102",
                    "attributes": {"Campaign_Attribute-1": ""},
                    "answerMachineDetectionConfig": {
                        "enableAnswerMachineDetection": false,
                        "awaitAnswerMachinePrompt": false
                    },
                    // The maximum of the same @range.
                    "ringTimeout": 60
                }})
            },
        ),
        ("sms, required members only", "SMS", || {
            json!({"sms": {
                "destinationPhoneNumber": "+15555550200",
                "templateParameters": {}
            }})
        }),
        ("sms, every optional member filled", "SMS", || {
            json!({"sms": {
                "destinationPhoneNumber": "+15555550201",
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-1",
                "templateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-1",
                "templateParameters": {"FirstName": "Ada", "Amount": "12.50"}
            }})
        }),
        ("email, required members only", "EMAIL", || {
            json!({"email": {
                "destinationEmailAddress": "customer@example.com",
                "templateParameters": {}
            }})
        }),
        ("email, every optional member filled", "EMAIL", || {
            json!({"email": {
                "destinationEmailAddress": "someone.else@example.com",
                "connectSourceEmailAddress": "campaigns@example.com",
                "templateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-2",
                "templateParameters": {"Subject": "Your appointment"}
            }})
        }),
        ("whatsApp, required members only", "WHATSAPP", || {
            json!({"whatsApp": {
                "destinationPhoneNumber": "+15555550300",
                "templateParameters": {}
            }})
        }),
        ("whatsApp, every optional member filled", "WHATSAPP", || {
            json!({"whatsApp": {
                "destinationPhoneNumber": "+15555550301",
                "connectSourcePhoneNumberArn":
                    "arn:aws:connect:us-east-1:000000000000:phone-number/p-2",
                "templateArn": "arn:aws:wisdom:us-east-1:000000000000:template/t-3",
                "templateParameters": {"Body": "See you at 10:00", "Locale": "en_US"}
            }})
        }),
    ];

    let requests: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(row, (_, _, parameters))| {
            json!({
                "clientToken": format!("row-{row}"),
                "expirationTime": instant(LIVE_FOR_SECS),
                "channelSubtypeParameters": parameters(),
            })
        })
        .collect();
    let taken = take_all(
        &router,
        region,
        &campaign,
        "outbound-requests",
        "outboundRequests",
        requests,
    )
    .await;

    for (row, (case, channel_subtype, parameters)) in cases.iter().enumerate() {
        let token = format!("row-{row}");
        let reported = taken
            .get(&token)
            .unwrap_or_else(|| panic!("case {case:?}: expected {token} taken, got {taken:?}"));
        let (stored_id, stored_channel, stored_parameters) =
            stored_dial_request(&engine, region, &campaign, &token).unwrap_or_else(|| {
                panic!("case {case:?}: expected the buffer to hold a record under {token:?}")
            });
        assert_eq!(
            &stored_id, reported,
            "case {case:?}: the record must carry the dial id the response reported, got \
             {stored_id:?}"
        );
        assert_eq!(
            stored_channel, *channel_subtype,
            "case {case:?}: expected the record to derive channel {channel_subtype:?} from the \
             parameters it kept, got {stored_channel:?}"
        );
        assert_eq!(
            stored_parameters,
            parameters(),
            "case {case:?}: expected the record to keep the parameters as submitted, got \
             {stored_parameters}"
        );
    }
}

/// A profile request taken into a campaign's profile buffer keeps the whole
/// `eventTriggerContext` it was submitted with, over a table spanning the absence of
/// the context, each of its members on its own, and both together.
#[tokio::test]
async fn a_profile_request_keeps_the_whole_event_trigger_context_it_carried() {
    let region = "eu-west-1";
    let (router, engine) = router_with_engine();
    let campaign = running_campaign(&router, region, "instance-1").await;

    // `SessionId` and `BrowserId` are @length 1..36, so these rows reach both of that
    // range's published bounds.
    let longest_session_id = "s".repeat(36);

    // (case, the eventTriggerContext the row carries, if any)
    let cases: &[(&str, Option<Value>)] = &[
        ("no event trigger context at all", None),
        ("an empty event trigger context", Some(json!({}))),
        (
            "a source event only",
            Some(json!({"sourceEvent": "order-placed"})),
        ),
        (
            "a web notification session only",
            Some(json!({"channelContext": {"webNotificationContext": {"sessionId": "s"}}})),
        ),
        (
            "a web notification browser only",
            Some(json!({"channelContext": {"webNotificationContext": {"browserId": "b-1"}}})),
        ),
        (
            "a source event and a whole web notification context",
            Some(json!({
                "sourceEvent": "cart-abandoned",
                "channelContext": {"webNotificationContext": {
                    "sessionId": longest_session_id,
                    "browserId": "browser-0001"
                }}
            })),
        ),
    ];

    // `ProfileId`'s @pattern is `^[a-f0-9]{32}$`, so each row names a profile of
    // exactly 32 hex characters.
    let profile_of = |row: usize| -> String { format!("{:0>31}{row:x}", "abcdef0123456789abcdef") };

    let requests: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(row, (_, context))| {
            let mut request = json!({
                "clientToken": format!("row-{row}"),
                "profileId": profile_of(row),
                "expirationTime": instant(LIVE_FOR_SECS),
            });
            set_if_present(&mut request, "eventTriggerContext", context.clone());
            request
        })
        .collect();
    let taken = take_all(
        &router,
        region,
        &campaign,
        "profile-outbound-requests",
        "profileOutboundRequests",
        requests,
    )
    .await;

    for (row, (case, context)) in cases.iter().enumerate() {
        let token = format!("row-{row}");
        let reported = taken
            .get(&token)
            .unwrap_or_else(|| panic!("case {case:?}: expected {token} taken, got {taken:?}"));
        let (stored_id, stored_profile, stored_context) =
            stored_profile_request(&engine, region, &campaign, &token).unwrap_or_else(|| {
                panic!("case {case:?}: expected the buffer to hold a record under {token:?}")
            });
        assert_eq!(
            &stored_id, reported,
            "case {case:?}: the record must carry the request id the response reported, got \
             {stored_id:?}"
        );
        assert_eq!(
            stored_profile,
            profile_of(row),
            "case {case:?}: expected the record to keep the profile it is to reach, got \
             {stored_profile:?}"
        );
        assert_eq!(
            stored_context, *context,
            "case {case:?}: expected the record to keep the context as submitted, got \
             {stored_context:?}"
        );
    }
}
