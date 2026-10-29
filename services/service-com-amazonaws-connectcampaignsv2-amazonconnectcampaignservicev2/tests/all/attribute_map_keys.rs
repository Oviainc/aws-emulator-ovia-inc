// SPDX-License-Identifier: CC0-1.0

//! The KEYS of the attribute maps behind `channelSubtypeParameters`, end to end over
//! the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handler together. The rule under test: a map whose KEY shape declares a
//! `@length` and a `@pattern` admits only keys that satisfy them, so a request carrying
//! any other key is refused WHOLE with HTTP 400 `ValidationException` naming that key,
//! and nothing of the batch is taken — a map key is the one leaf no constraint walk
//! reaches, so the rule holds wherever such a map arrives rather than only where the
//! walk descends.
//!
//! The maps under test are the four the batch accepts, each targeting the model's
//! `Attributes` and so keyed by `AttributeName` (source:
//! `smithy_query(query="shape", shape="Attributes,AttributeName")` — `@length` 0..32767
//! with `@pattern ^[a-zA-Z0-9\-_]+$`): a telephony request's `attributes`, and the
//! `templateParameters` of its SMS, email and WhatsApp siblings. The bounds this suite
//! asserts are read back out of `smithy-model/` by
//! [`the_attribute_key_rules_this_crate_restates_are_the_models`], so no row below can
//! be asserting a bound the model no longer declares.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State, ATTRIBUTE_NAME_MAX_LEN,
    ATTRIBUTE_NAME_MIN_LEN, ATTRIBUTE_NAME_PATTERN,
};

use super::common::{call, create_campaign_with, error_code, router_with_engine, Harness};

/// The region every case here signs for: one region holds every record a case reads,
/// and nothing in the rule under test is region-dependent.
const REGION: &str = "ap-southeast-2";

/// The instant every request in this suite expires at — far enough ahead that no case
/// is about expiry, which `outbound_request_expiration` owns.
const FAR_FUTURE: &str = "2099-01-01T00:00:00Z";

/// One channel of the `channelSubtypeParameters` union: the union member it arrives
/// under and the member its `Attributes` map arrives under inside it.
///
/// Source: the four members of `ChannelSubtypeParameters` and the `Attributes`-targeted
/// member each of their structures carries (`attributes` on telephony,
/// `templateParameters` on the other three).
const CHANNELS: [(&str, &str); 4] = [
    ("telephony", "attributes"),
    ("sms", "templateParameters"),
    ("email", "templateParameters"),
    ("whatsApp", "templateParameters"),
];

/// A JSON map over `keys`, every key carrying a value `AttributeValue` admits.
fn attributes<'k>(keys: impl IntoIterator<Item = &'k str>) -> Value {
    let mut map = Map::new();
    for key in keys {
        map.insert(key.to_string(), json!("v"));
    }
    Value::Object(map)
}

/// One outbound request for `channel`, carrying `attributes` as that channel's
/// attribute map, under the client token `token`.
fn request(channel: &str, token: &str, attributes: Value) -> Value {
    let parameters = match channel {
        "telephony" => json!({"telephony": {
            "destinationPhoneNumber": "+15555550100",
            "attributes": attributes,
        }}),
        "sms" => json!({"sms": {
            "destinationPhoneNumber": "+15555550101",
            "templateParameters": attributes,
        }}),
        "email" => json!({"email": {
            "destinationEmailAddress": "dialed@example.com",
            "templateParameters": attributes,
        }}),
        "whatsApp" => json!({"whatsApp": {
            "destinationPhoneNumber": "+15555550102",
            "templateParameters": attributes,
        }}),
        other => panic!("{other} is not a ChannelSubtypeParameters member"),
    };
    json!({
        "clientToken": token,
        "expirationTime": FAR_FUTURE,
        "channelSubtypeParameters": parameters,
    })
}

/// `PutOutboundRequestBatch` for `id` with `requests`.
async fn put_batch(router: &Harness, id: &str, requests: Vec<Value>) -> (u16, Value) {
    call(
        router,
        REGION,
        "PUT",
        &format!("/v2/campaigns/{id}/outbound-requests"),
        &json!({"outboundRequests": requests}),
    )
    .await
}

/// A campaign configured for all four channels and already `Running`, so a request on
/// any of them is one it serves — the fixture an accepted key is asserted taken against.
///
/// The required members of each channel block come from their own shapes:
/// `outboundMode` and `defaultOutboundConfig` on every block,
/// `connectContactFlowId` on `TelephonyOutboundConfig`, `connectSourcePhoneNumberArn`
/// and `wisdomTemplateArn` on the SMS and WhatsApp configs, and
/// `connectSourceEmailAddress` with `wisdomTemplateArn` on the email config.
async fn running_campaign(router: &Harness, instance: &str, name: &str) -> String {
    let id = create_campaign_with(
        router,
        REGION,
        instance,
        name,
        json!({"channelSubtypeConfig": {
            "telephony": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"},
            },
            "sms": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {
                    "connectSourcePhoneNumberArn":
                        "arn:aws:connect:ap-southeast-2:000000000000:phone-number/p-1",
                    "wisdomTemplateArn":
                        "arn:aws:wisdom:ap-southeast-2:000000000000:template/t-1",
                },
            },
            "email": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {
                    "connectSourceEmailAddress": "campaigns@example.com",
                    "wisdomTemplateArn":
                        "arn:aws:wisdom:ap-southeast-2:000000000000:template/t-2",
                },
            },
            "whatsApp": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {
                    "connectSourcePhoneNumberArn":
                        "arn:aws:connect:ap-southeast-2:000000000000:phone-number/p-2",
                    "wisdomTemplateArn":
                        "arn:aws:wisdom:ap-southeast-2:000000000000:template/t-3",
                },
            },
        }}),
    )
    .await;
    let (status, body) = call(
        router,
        REGION,
        "POST",
        &format!("/v2/campaigns/{id}/start"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"start the campaign\": expected HTTP 200, got {status} {body}"
    );
    id
}

/// The client tokens the campaign's buffer holds, straight out of the store — how a
/// case asserts a refused batch took nothing and an accepted one was taken.
fn buffered(engine: &engine::InMemoryEngine, id: &str) -> BTreeSet<String> {
    engine
        .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
        .lock()
        .campaign(REGION, id)
        .expect("the campaign under test")
        .outbound_requests
        .keys()
        .cloned()
        .collect()
}

/// The requirement clause a refusal states for a key outside the key shape's
/// `@pattern`, naming the expression as the model spells it.
fn pattern_requirement() -> String {
    format!("Member must satisfy regular expression pattern: {ATTRIBUTE_NAME_PATTERN}")
}

/// The requirement clause a refusal states for a key past the key shape's `@length`
/// maximum.
fn max_length_requirement() -> String {
    format!("Member must have length less than or equal to {ATTRIBUTE_NAME_MAX_LEN}")
}

/// The whole message a map at `member` is refused with when `offenders` are not keys
/// the shape admits: the validation envelope every constraint refusal in this emulator
/// is worded with, one clause per offending key, ordered by key.
fn expected_message(member: &str, offenders: &[&str], requirement: &str) -> String {
    let mut ordered: Vec<&str> = offenders.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    let clauses: Vec<String> = ordered
        .iter()
        .map(|key| {
            format!(
                "Value '{key}' at '{member}.{key}.member' failed to satisfy constraint: \
                 {requirement}"
            )
        })
        .collect();
    if clauses.len() == 1 {
        format!("1 validation error detected: {}", clauses[0])
    } else {
        format!(
            "{} validation errors detected: {}",
            clauses.len(),
            clauses.join("; ")
        )
    }
}

/// The message a response carries.
fn message_of(response: &Value) -> &str {
    response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// The attribute keys the key shape refuses, each with the requirement it breaks.
///
/// The rows differ along what the `@pattern` generalises over: which character outside
/// its class the key carries (space, `!`, `.`, `:`, `/`, `+`, `@`), where in the key
/// that character sits (leading, interior, trailing), whether the key is non-ASCII, and
/// the empty key — which the `@length` minimum of 0 admits but the pattern's `+`, which
/// wants at least one character, does not.
fn keys_outside_the_key_shape() -> Vec<(&'static str, &'static str, String)> {
    vec![
        (
            "a key with an interior space",
            "Customer Id",
            pattern_requirement(),
        ),
        (
            "a key with a trailing bang",
            "CustomerId!",
            pattern_requirement(),
        ),
        ("a key with a dot", "customer.id", pattern_requirement()),
        ("a key with a colon", "customer:id", pattern_requirement()),
        ("a key with a slash", "customer/id", pattern_requirement()),
        ("a key with a plus", "customer+id", pattern_requirement()),
        ("a key with an at sign", "customer@id", pattern_requirement()),
        (
            "a key with a leading space",
            " CustomerId",
            pattern_requirement(),
        ),
        (
            "a key with a trailing space",
            "CustomerId ",
            pattern_requirement(),
        ),
        ("a non-ASCII key", "clienté", pattern_requirement()),
        ("the empty key", "", pattern_requirement()),
    ]
}

/// The attribute keys the key shape admits.
///
/// The rows differ along the character classes the `@pattern` spells (upper and lower
/// case letters, digits, dash, underscore), the key's length (one character, the
/// shortest the pattern admits, up to a long mixed key) and whether a dash or
/// underscore leads, separates or trails.
const KEYS_INSIDE_THE_KEY_SHAPE: [(&str, &str); 7] = [
    ("a mixed-case alphanumeric key", "CustomerId7"),
    ("a dash-separated key", "customer-id"),
    ("an underscore-separated key", "customer_id"),
    ("a digits-only key", "0123456789"),
    ("a single-character key", "a"),
    ("a key of every admitted class", "A-_z9"),
    ("a key that leads and trails with the punctuation", "-_customer_-"),
];

/// A key the `AttributeName` shape does not admit refuses the WHOLE batch, before any
/// request of it is taken: a constraint violation is a bad request, not a per-request
/// outcome.
///
/// Every row of [`keys_outside_the_key_shape`] is sent on every channel the union
/// carries, each beside a well-formed request the campaign does serve, so the rule is
/// asserted over all four attribute maps rather than the one the issue cited.
#[tokio::test]
async fn an_attribute_key_outside_the_key_shape_refuses_the_whole_batch() {
    let (router, engine) = router_with_engine();
    let id = running_campaign(&router, "attribute-keys", "keyed").await;

    let before = buffered(&engine, &id);
    for (channel, map_member) in CHANNELS {
        for (index, (key_case, key, requirement)) in
            keys_outside_the_key_shape().into_iter().enumerate()
        {
            let case = format!("{channel}.{map_member} sent {key_case}");
            // A request the campaign does serve, first in the list, so a 400 is the
            // batch's refusal and not that request's per-request failure.
            let well_formed = request(
                "telephony",
                &format!("well-formed-{channel}-{index}"),
                attributes(["CustomerId"]),
            );
            let offending = request(
                channel,
                &format!("offending-{channel}-{index}"),
                attributes([key]),
            );
            let (status, body) = put_batch(&router, &id, vec![well_formed, offending]).await;

            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {}",
                error_code(&body)
            );
            // The offending request is the SECOND element of the batch, which the
            // refusal names the way the boundary walk spells a list element, then the
            // union member, then the map, then the offending key.
            assert_eq!(
                message_of(&body),
                expected_message(
                    &format!(
                        "outboundRequests.2.member.channelSubtypeParameters.\
                         {channel}.{map_member}"
                    ),
                    &[key],
                    &requirement,
                ),
                "case {case:?}: the refusal was worded {:?}",
                message_of(&body)
            );
            let after = buffered(&engine, &id);
            assert_eq!(
                after, before,
                "case {case:?}: expected the refused batch to take nothing, got {after:?} \
                 (was {before:?})"
            );
        }
    }
}

/// A key the `AttributeName` shape admits is taken on every channel, so the check
/// refuses only what the key shape leaves out.
#[tokio::test]
async fn an_attribute_key_inside_the_key_shape_is_taken() {
    let (router, engine) = router_with_engine();
    let id = running_campaign(&router, "attribute-keys-admitted", "keyed").await;

    for (channel, map_member) in CHANNELS {
        for (index, (key_case, key)) in KEYS_INSIDE_THE_KEY_SHAPE.into_iter().enumerate() {
            let case = format!("{channel}.{map_member} sent {key_case}");
            // One token per row, so every row is a request the campaign takes rather
            // than an idempotent repeat of the row before it.
            let token = format!("{channel}-{index}");
            // Several admitted keys in one map, so an accepted map is not just a map of
            // one: the row's key beside two the other rows never send.
            let (status, body) = put_batch(
                &router,
                &id,
                vec![request(
                    channel,
                    &token,
                    attributes([key, "Attempt_1", "retry-count"]),
                )],
            )
            .await;

            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            let taken: Vec<&str> = body
                .get("successfulRequests")
                .and_then(Value::as_array)
                .map(|requests| {
                    requests
                        .iter()
                        .filter_map(|request| request["clientToken"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                taken.contains(&token.as_str()),
                "case {case:?}: expected the request to be reported successful, got {body}"
            );
            assert!(
                body.pointer("/successfulRequests/0/id")
                    .and_then(Value::as_str)
                    .is_some(),
                "case {case:?}: expected a minted dial request id, got {body}"
            );
            // The store, not just the response: the campaign holds the request the
            // accepted map came in on.
            let held = buffered(&engine, &id);
            assert!(
                held.contains(&token),
                "case {case:?}: expected the campaign's buffer to hold {token:?}, got {held:?}"
            );
        }
    }
}

/// The key shape's published `@length` bounds hold: a key of exactly the maximum is
/// taken, and one character past it is refused with the length requirement, the whole
/// batch taking nothing.
#[tokio::test]
async fn an_attribute_key_is_held_to_the_key_shapes_published_length_bounds() {
    let (router, engine) = router_with_engine();
    let id = running_campaign(&router, "attribute-keys-length", "keyed").await;

    let at_maximum = "a".repeat(ATTRIBUTE_NAME_MAX_LEN as usize);
    let (status, body) = put_batch(
        &router,
        &id,
        vec![request(
            "telephony",
            "at-the-maximum",
            attributes([at_maximum.as_str()]),
        )],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a key at the published length maximum\": expected HTTP 200, got {status} {body}"
    );
    assert!(
        buffered(&engine, &id).contains("at-the-maximum"),
        "case \"a key at the published length maximum\": expected the request to be taken"
    );

    let before = buffered(&engine, &id);
    let past_maximum = "a".repeat(ATTRIBUTE_NAME_MAX_LEN as usize + 1);
    let (status, body) = put_batch(
        &router,
        &id,
        vec![request(
            "sms",
            "past-the-maximum",
            attributes([past_maximum.as_str()]),
        )],
    )
    .await;
    let case = "a key one character past the published length maximum";
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert!(
        error_code(&body).ends_with("ValidationException"),
        "case {case:?}: expected ValidationException, got {}",
        error_code(&body)
    );
    let message = message_of(&body);
    assert!(
        message.contains(&max_length_requirement()),
        "case {case:?}: expected the refusal to state {:?}, got {message:?}",
        max_length_requirement()
    );
    // A key is caller-supplied text, so the refusal echoes a bounded prefix of it with
    // the byte count rather than the whole key: a huge key cannot be amplified into a
    // huge response.
    assert!(
        message.contains(&format!("[{} bytes]", past_maximum.len())),
        "case {case:?}: expected the refusal to elide the offending key, got {message:?}"
    );
    assert!(
        message.len() < past_maximum.len(),
        "case {case:?}: expected the refusal to be shorter than the key it refuses, got {} \
         characters",
        message.len()
    );
    let after = buffered(&engine, &id);
    assert_eq!(
        after, before,
        "case {case:?}: expected the refused batch to take nothing, got {after:?} \
         (was {before:?})"
    );
}

/// Several keys outside the shape in one map are one refusal: one sentence carrying one
/// clause per offending key, ordered by key, with the admissible keys beside them
/// reported on not at all — and nothing of the map taken.
#[tokio::test]
async fn every_key_outside_the_key_shape_is_reported_in_one_sentence() {
    let (router, engine) = router_with_engine();
    let id = running_campaign(&router, "attribute-keys-many", "keyed").await;

    // (case, the keys the map carries, the ones the shape refuses)
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "one key off the shape beside two in it",
            vec!["CustomerId", "bad key", "retry_count"],
            vec!["bad key"],
        ),
        (
            "two keys off the shape beside one in it",
            vec!["z bad", "CustomerId", "a.bad"],
            vec!["z bad", "a.bad"],
        ),
        (
            "three keys off the shape",
            vec!["c!", "b:", "a/"],
            vec!["c!", "b:", "a/"],
        ),
        (
            "the empty key beside an admitted one",
            vec!["", "CustomerId"],
            vec![""],
        ),
    ];

    let before = buffered(&engine, &id);
    for (case, keys, offenders) in cases {
        let (status, body) = put_batch(
            &router,
            &id,
            vec![request("telephony", "many-offenders", attributes(keys))],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            message_of(&body),
            expected_message(
                "outboundRequests.1.member.channelSubtypeParameters.telephony.attributes",
                &offenders,
                &pattern_requirement(),
            ),
            "case {case:?}: the refusal was worded {:?}",
            message_of(&body)
        );
        let after = buffered(&engine, &id);
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused batch to take nothing, got {after:?} \
             (was {before:?})"
        );
    }
}

/// A key outside the shape is refused BEFORE the campaign is resolved and before its
/// lifecycle is weighed: an input constraint precedes resource resolution, so the same
/// request that would answer 404 or 409 answers 400 when its attribute map carries a
/// key the shape forbids.
#[tokio::test]
async fn an_attribute_key_outside_the_key_shape_is_refused_before_the_campaign_is_resolved() {
    let (router, _engine) = router_with_engine();
    // A campaign that exists but has never started, so a well-formed batch against it
    // is the lifecycle refusal.
    let initialized = create_campaign_with(
        &router,
        REGION,
        "attribute-keys-order",
        "never started",
        json!({}),
    )
    .await;

    // (case, the campaign the batch names, the status a well-formed batch gets)
    let cases: Vec<(&str, String, u16)> = vec![
        (
            "an id no campaign answers to",
            "00000000000000000000".to_string(),
            404,
        ),
        ("a campaign that is not running", initialized, 409),
    ];

    for (case, id, well_formed_status) in cases {
        let (status, body) = put_batch(
            &router,
            &id,
            vec![request("telephony", "ordering", attributes(["bad key!"]))],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a key off the shape, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            error_code(&body)
        );

        // The same batch with an admitted key does reach the resolution the 400 above
        // pre-empted, so that 400 came from the key and nothing else.
        let (status, body) = put_batch(
            &router,
            &id,
            vec![request("telephony", "ordering", attributes(["CustomerId"]))],
        )
        .await;
        assert_eq!(
            status, well_formed_status,
            "case {case:?}: expected HTTP {well_formed_status} for an admitted key, \
             got {status} {body}"
        );
    }
}

/// The bounds the emulator judges an attribute key by are the model's.
///
/// An attribute key is the one leaf of this union no generated schema carries the
/// constraints of — the codegen hangs the map's VALUE shape off the map member and
/// describes the key nowhere — so the crate restates them once
/// (`state::ATTRIBUTE_NAME_PATTERN` and friends). This pins that restatement, and this
/// suite's own bounds, to `smithy-model/`, so a model refresh that moved a bound fails
/// here instead of leaving the emulator judging by a stale one.
#[test]
fn the_attribute_key_rules_this_crate_restates_are_the_models() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json"
    );
    let text = std::fs::read_to_string(path).expect("the service's Smithy model is readable");
    let model: Value = serde_json::from_str(&text).expect("the service's Smithy model is JSON");
    let shape = |name: &str| {
        model["shapes"][format!("com.amazonaws.connectcampaignsv2#{name}")].clone()
    };

    // The map's key targets the shape whose rules the crate restates.
    let attributes = shape("Attributes");
    assert_eq!(
        attributes["key"]["target"],
        json!("com.amazonaws.connectcampaignsv2#AttributeName"),
        "case \"Attributes key target\": expected the attribute map to be keyed by the \
         shape this suite asserts, got {attributes}"
    );

    let traits = shape("AttributeName")["traits"].clone();
    assert_eq!(
        traits["smithy.api#length"]["min"],
        json!(ATTRIBUTE_NAME_MIN_LEN),
        "case \"AttributeName @length min\": the modelled floor must be the one the \
         handlers enforce, got {traits}"
    );
    assert_eq!(
        traits["smithy.api#length"]["max"],
        json!(ATTRIBUTE_NAME_MAX_LEN),
        "case \"AttributeName @length max\": the modelled ceiling must be the one the \
         handlers enforce, got {traits}"
    );
    assert_eq!(
        traits["smithy.api#pattern"],
        json!(ATTRIBUTE_NAME_PATTERN),
        "case \"AttributeName @pattern\": the modelled expression must be the one the \
         handlers enforce, got {traits}"
    );

    // Every channel's attribute map is the same shape, so one set of rules judges all
    // four: the rule under test is the key shape's, not one channel's.
    for structure in [
        "TelephonyChannelSubtypeParameters",
        "SmsChannelSubtypeParameters",
        "EmailChannelSubtypeParameters",
        "WhatsAppChannelSubtypeParameters",
    ] {
        let members = shape(structure)["members"].clone();
        let map_member = if structure.starts_with("Telephony") {
            "attributes"
        } else {
            "templateParameters"
        };
        assert_eq!(
            members[map_member]["target"],
            json!("com.amazonaws.connectcampaignsv2#Attributes"),
            "case \"{structure}.{map_member}\": expected the channel's attribute map to \
             target Attributes, got {members}"
        );
    }
}
