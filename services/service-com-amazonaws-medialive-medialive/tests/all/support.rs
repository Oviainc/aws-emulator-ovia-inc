// SPDX-License-Identifier: CC0-1.0

//! The template-group helpers the group suites share, layered on the crate's one
//! wire harness ([`crate::wire`]).
//!
//! [`timestamp`] and [`member`] read a response member for EVERY group suite, so
//! there is one reading of a wire timestamp in this crate rather than one per
//! suite, and [`reported_tags`] states once what a response's `Tags` member is.
//! The request helpers here are the cloudwatch alarm template group's own
//! (the eventbridge suite keeps its own URI and calls); a third suite should lift
//! what it would otherwise copy into this module rather than add a third copy.
//!
//! No test lives here and no request is built here: the dispatch, the signing
//! and the error-code reading all stay in `wire`, so this module cannot drift
//! from how every other suite reaches the emulator. What it adds is the group
//! collection's URI, its two implemented operations as one call each, the
//! label encoder every `@httpLabel` suite shares ([`percent_encoded`]), the
//! readers the group assertions need, the two clock readings a timestamp
//! assertion compares ([`epoch_seconds`], [`unix_seconds`]), the one channel
//! a signal map's discovery is entered from ([`entry_point`]), the channel
//! configuration every suite that creates a channel declares
//! ([`channel_encoder_settings`], [`attachable_input`],
//! [`resolved_channel_body`]), the input
//! setup every suite that creates an input shares ([`input_security_group`],
//! [`sdi_source`], [`claimed_device`], [`resolved_input_body`]) and the network
//! setup every suite that maps a cluster onto one shares
//! ([`resolved_network_ids`]).

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

use crate::wire::{call, call_in_region};

/// The collection URI `CreateCloudWatchAlarmTemplateGroup` is bound to
/// (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplateGroup")`: POST
/// /prod/cloudwatch-alarm-template-groups).
pub const GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";

/// `CreateCloudWatchAlarmTemplateGroup`, signed for `region`.
pub async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    body: Value,
) -> (u16, Value) {
    call_in_region(router, region, "POST", GROUPS, Some(body)).await
}

/// `GetCloudWatchAlarmTemplateGroup` on `identifier`, signed for `region`.
pub async fn get_in(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{GROUPS}/{}", percent_encoded(identifier)),
        None,
    )
    .await
}

/// Percent-encode one URI path segment.
///
/// `Identifier` and `ResourceArn` are `@httpLabel`s, so a label that is not made
/// of URI-unreserved characters travels percent-encoded. Encoding every
/// non-alphanumeric byte is a superset of what a label needs (it also covers the
/// `:` and `/` of an ARN), and the `percent-encoding` crate owns the encoding
/// itself.
pub fn percent_encoded(label: &str) -> String {
    percent_encoding::utf8_percent_encode(label, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A timestamp member, read as the instant its RFC 3339 / ISO-8601 string names.
///
/// The model targets every timestamp member of this service at
/// `__timestampIso8601`, which carries `@timestampFormat("date-time")`, so the only
/// wire form that is the modelled one is a date-time STRING: the restJson1 default
/// (a JSON number of epoch seconds) fails here rather than being accepted, which is
/// what makes every suite that reads a timestamp through this helper pin the form as
/// well as the instant.
pub fn timestamp(case: &str, member: &str, body: &Value) -> DateTime {
    match &body[member] {
        Value::String(text) => DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
            panic!("case {case:?}: {member} {text:?} is not an ISO-8601 date-time: {error}")
        }),
        other => panic!(
            "case {case:?}: expected {member} as an ISO-8601 date-time string, got {other:?}"
        ),
    }
}

/// The `tags` member a response reports for a resource created with `requested` tags.
///
/// `Tags` targets `TagMap` and is always reported as the map it is modelled as, so a
/// resource created with no tags at all reports the EMPTY map rather than omitting the
/// member: zero tags is an empty tag set, not an unknown one.
pub fn reported_tags(requested: Option<&Value>) -> Value {
    requested.cloned().unwrap_or_else(|| json!({}))
}

/// A member the request did not supply is absent from the response, which reads
/// the same as JSON null here.
pub fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// A wire timestamp as seconds since the Unix epoch, so it can be compared with a
/// [`SystemTime`] the test took around the call.
pub fn epoch_seconds(stamp: DateTime) -> f64 {
    stamp.secs() as f64 + f64::from(stamp.subsec_nanos()) / 1e9
}

/// The same reading of a [`SystemTime`] the test took itself.
pub fn unix_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_secs_f64()
}

/// One input security group, answering its minted `Id`.
///
/// The setup every suite whose input configuration carries an
/// `InputSecurityGroups` entry — or whose `Type` is one an upstream system
/// pushes to, which the whitelist is what admits — goes through, so no suite has
/// to invent a group id that nothing minted.
pub async fn input_security_group(router: &MultiServiceRouter) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/inputSecurityGroups",
        Some(json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateInputSecurityGroup to answer 201, got {status}: {created}"
    );
    created["securityGroup"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One SdiSource named `name`, answering its minted `Id` — the setup a suite
/// whose input ingests through `SdiSources` goes through.
pub async fn sdi_source(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/sdiSources",
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateSdiSource to answer 201, got {status}: {created}"
    );
    created["sdiSource"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The input device `id`, claimed into this account, answering that same id —
/// the setup a suite whose input ingests through `InputDevices` goes through.
pub async fn claimed_device(router: &MultiServiceRouter, id: &str) -> String {
    let (status, claimed) = call(router, "POST", "/prod/claimDevice", Some(json!({ "id": id }))).await;
    assert_eq!(
        status, 200,
        "setup: expected ClaimDevice({id}) to answer 200, got {status}: {claimed}"
    );
    id.to_string()
}

/// Whether this account already holds the resource `uri` reads.
async fn already_held(router: &MultiServiceRouter, uri: &str) -> bool {
    let (status, _) = call(router, "GET", uri, None).await;
    status == 200
}

/// The same `CreateInput` body, pointed at resources this account really holds.
///
/// Every id an input configuration names has to resolve to a resource of this
/// account, and an input of a type an upstream system pushes to has to name the
/// whitelist that admits it, so a suite's row says WHAT its input ingests
/// through and this helper puts those resources in the account: an input
/// security group per `inputSecurityGroups` entry (and one for a pushed-to row
/// that names none), an SdiSource per `sdiSources` entry (the row's entry is
/// used as its `Name`), a claim per `inputDevices` entry — a device id is the
/// unit's own, so it is claimed exactly as the row spells it — and the pair of
/// redundant source URLs a pulled-from row that names none needs.
///
/// An entry that ALREADY names a resource this account holds is left pointing at
/// it, so a suite that created the resource itself keeps the id it chose and
/// this helper is idempotent.
///
/// A row that is about the `Vpc` rules is left alone where it names no group: a
/// VPC input's ingest addresses are created in the customer's VPC, and `Vpc` is
/// "not compatible with the inputSecurityGroups property".
pub async fn resolved_input_body(router: &MultiServiceRouter, body: Value) -> Value {
    let mut body = body;
    let pushed_to = matches!(
        body.get("type").and_then(Value::as_str),
        Some("RTMP_PUSH" | "RTP_PUSH" | "UDP_PUSH")
    ) && body.get("vpc").is_none()
        && body
            .get("inputNetworkLocation")
            .and_then(Value::as_str)
            .unwrap_or("AWS")
            == "AWS";
    let requested_groups = body
        .get("inputSecurityGroups")
        .and_then(Value::as_array)
        .cloned();
    match requested_groups {
        Some(entries) if !entries.is_empty() => {
            let mut resolved = Vec::new();
            for entry in entries {
                let named = entry.as_str().unwrap_or_default().to_string();
                if already_held(router, &format!("/prod/inputSecurityGroups/{named}")).await {
                    resolved.push(json!(named));
                } else {
                    resolved.push(json!(input_security_group(router).await));
                }
            }
            body["inputSecurityGroups"] = Value::Array(resolved);
        }
        _ if pushed_to => {
            body["inputSecurityGroups"] = json!([input_security_group(router).await]);
        }
        _ => {}
    }
    if let Some(entries) = body.get("sdiSources").and_then(Value::as_array).cloned() {
        let mut resolved = Vec::new();
        for entry in entries {
            let named = entry.as_str().unwrap_or("sdi-source").to_string();
            if already_held(router, &format!("/prod/sdiSources/{named}")).await {
                resolved.push(json!(named));
            } else {
                resolved.push(json!(sdi_source(router, &named).await));
            }
        }
        body["sdiSources"] = Value::Array(resolved);
    }
    if let Some(entries) = body.get("inputDevices").and_then(Value::as_array).cloned() {
        for entry in entries {
            if let Some(id) = entry.get("id").and_then(Value::as_str) {
                if !already_held(router, &format!("/prod/inputDevices/{id}")).await {
                    claimed_device(router, id).await;
                }
            }
        }
    }
    // A row of a pulled-from type that names no source has nothing to ingest
    // from — `Sources` is "The source URLs for a PULL-type input. Every PULL
    // type input needs exactly two source URLs for redundancy" — so it is given
    // that redundant pair.
    if matches!(
        body.get("type").and_then(Value::as_str),
        Some("URL_PULL" | "RTMP_PULL" | "MP4_FILE" | "TS_FILE")
    ) && body.get("sources").is_none()
    {
        body["sources"] = json!([
            { "url": "https://example.com/a.m3u8" },
            { "url": "https://example.com/b.m3u8" },
        ]);
    }
    body
}

/// The same cluster request — or the `networkSettings` a row expects back —
/// pointed at networks this account really holds.
///
/// An `InterfaceMapping`'s `NetworkId` has to name a network of this account, so
/// a suite's row says WHICH network each interface is mapped onto by SPELLING an
/// id, and this helper puts a network behind every spelling it finds: a spelling
/// that already names a network this account holds is left pointing at it, and
/// any other spelling becomes one network created under that spelling as its
/// `Name`. Keyed by the spelling through `RequestId`, so one spelling resolves to
/// the SAME network however often a suite asks for it — which is what lets a row
/// resolve its request and the settings it expects back through the same call and
/// compare them.
///
/// `DefaultRoute` names an interface rather than a network, so it is left exactly
/// as the row spelled it.
pub async fn resolved_network_ids(router: &MultiServiceRouter, value: Value) -> Value {
    let mut spellings: Vec<String> = Vec::new();
    collect_network_ids(&value, &mut spellings);
    let mut resolved: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for spelling in spellings {
        if resolved.contains_key(&spelling) {
            continue;
        }
        let id = held_network(router, &spelling).await;
        resolved.insert(spelling, id);
    }
    substituted_network_ids(value, &resolved)
}

/// The id of a network this account holds for the spelling `spelled`.
async fn held_network(router: &MultiServiceRouter, spelled: &str) -> String {
    if already_held(router, &format!("/prod/networks/{spelled}")).await {
        return spelled.to_string();
    }
    let (status, network) = call(
        router,
        "POST",
        "/prod/networks",
        Some(json!({
            "name": spelled,
            "requestId": spelled,
            "ipPools": [{ "cidr": "10.0.0.0/16" }],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateNetwork for {spelled:?} to answer 201, got {status}: {network}"
    );
    network["id"].as_str().unwrap_or_default().to_string()
}

/// Every `networkId` spelled anywhere in `value`, in the order they appear.
fn collect_network_ids(value: &Value, into: &mut Vec<String>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                match (name.as_str(), member.as_str()) {
                    ("networkId", Some(spelled)) => into.push(spelled.to_string()),
                    _ => collect_network_ids(member, into),
                }
            }
        }
        Value::Array(entries) => {
            for entry in entries {
                collect_network_ids(entry, into);
            }
        }
        _ => {}
    }
}

/// The same `value` with every `networkId` replaced by the id `resolved` holds
/// for its spelling.
fn substituted_network_ids(
    value: Value,
    resolved: &std::collections::HashMap<String, String>,
) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .into_iter()
                .map(|(name, member)| {
                    let member = if name == "networkId" {
                        match member.as_str().and_then(|spelled| resolved.get(spelled)) {
                            Some(id) => json!(id),
                            None => member,
                        }
                    } else {
                        substituted_network_ids(member, resolved)
                    };
                    (name, member)
                })
                .collect(),
        ),
        Value::Array(entries) => Value::Array(
            entries
                .into_iter()
                .map(|entry| substituted_network_ids(entry, resolved))
                .collect(),
        ),
        other => other,
    }
}

/// One channel named `name`, answering its ARN — the top-level resource a signal
/// map's `DiscoveryEntryPointArn` names.
pub async fn entry_point(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": name, "encoderSettings": channel_encoder_settings() })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateChannel to answer 201, got {status}: {created}"
    );
    created["channel"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `EncoderSettings` a channel configuration has to declare, carrying the
/// four members the shape requires (`smithy_query(query="shape",
/// shape="EncoderSettings")`: AudioDescriptions, OutputGroups, TimecodeConfig,
/// VideoDescriptions).
///
/// The setup every suite that creates a channel shares: a channel encodes
/// through the encoder settings its configuration declares, so a create that
/// declares none is refused — see
/// `crate::channel_configuration_validation`, which is the suite ABOUT that
/// rule.
pub fn channel_encoder_settings() -> Value {
    json!({
        "audioDescriptions": [],
        "outputGroups": [],
        "timecodeConfig": { "source": "EMBEDDED" },
        "videoDescriptions": [],
    })
}

/// One input a channel can attach, answering its minted `Id`.
///
/// The setup every suite whose channel configuration carries an
/// `InputAttachments` entry shares: an attachment's `InputId` has to name an
/// input this account holds, so no suite has to invent an input id that nothing
/// minted. `URL_PULL` is the type that needs no other resource in the account,
/// just the redundant pair of source URLs every PULL-type input needs.
pub async fn attachable_input(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/inputs",
        Some(json!({
            "name": name,
            "type": "URL_PULL",
            "sources": [
                { "url": "https://example.com/a.m3u8" },
                { "url": "https://example.com/b.m3u8" },
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected CreateInput to answer 201, got {status}: {created}"
    );
    created["input"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The same `CreateChannel` body, as a channel configuration this service can
/// run.
///
/// A channel configuration has to declare the encoder settings the channel
/// encodes through, and every id it names has to resolve to a resource this
/// account holds, so a suite's row says only what ITS rule is about and this
/// helper supplies the rest — exactly as [`resolved_input_body`] does for an
/// input's configuration: the encoder settings for a body that declares none,
/// and one input behind every `inputId` spelling that names no input this
/// account holds. Keyed by the spelling, so one spelling resolves to the SAME
/// input however often a body repeats it, and a body that already names an
/// input this account holds (or already declares its own encoder settings) is
/// left exactly as it is.
pub async fn resolved_channel_body(router: &MultiServiceRouter, body: Value) -> Value {
    let mut body = body;
    if body.get("encoderSettings").is_none() {
        body["encoderSettings"] = channel_encoder_settings();
    }
    resolved_channel_attachments(router, body).await
}

/// The same body with every `inputAttachments` entry pointed at an input this
/// account holds, and nothing else changed.
///
/// What [`resolved_channel_body`] does for a CREATE's references, for a PATCH
/// that must name only the members its own rule is about: an `UpdateChannel`
/// body goes through here, so adding the encoder settings a create has to
/// declare does not turn a patch that names one member into one that names two.
pub async fn resolved_channel_attachments(router: &MultiServiceRouter, body: Value) -> Value {
    let mut body = body;
    let Some(entries) = body
        .get("inputAttachments")
        .and_then(Value::as_array)
        .cloned()
    else {
        return body;
    };
    let mut minted: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut resolved = Vec::with_capacity(entries.len());
    for entry in entries {
        let mut entry = entry;
        let spelled = entry
            .get("inputId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !spelled.is_empty() && already_held(router, &format!("/prod/inputs/{spelled}")).await {
            resolved.push(entry);
            continue;
        }
        let id = match minted.get(&spelled) {
            Some(id) => id.clone(),
            None => {
                let name = if spelled.is_empty() {
                    "attachable"
                } else {
                    spelled.as_str()
                };
                let id = attachable_input(router, name).await;
                minted.insert(spelled, id.clone());
                id
            }
        };
        entry["inputId"] = json!(id);
        resolved.push(entry);
    }
    body["inputAttachments"] = Value::Array(resolved);
    body
}
