// SPDX-License-Identifier: CC0-1.0

//! The live-manifest members a DASH origin endpoint is server-filled with.
//!
//! RULE under test: every optional `DashPackage` member for which MediaPackage publishes a
//! configuration default is filled with that default on EVERY write of the endpoint
//! (`CreateOriginEndpoint` and `UpdateOriginEndpoint`), stored, and rendered by every
//! projection of the stored record — the write's own response, `DescribeOriginEndpoint`
//! and each `ListOriginEndpoints` entry — while a member the request did supply is never
//! overwritten and a member with no published default is never invented.
//!
//! `origin_endpoint_defaults.rs` pins the whole default table across all four packaging
//! formats; this suite is the DASH live-manifest half of it, and is the one that also
//! drives the UPDATE write path.
//!
//! The rows span what the rule generalises over: which of the members the request supplies
//! (none, each one alone, all of them), the form of the supplied value (an enum value, a
//! positive duration, an explicit zero, a value that happens to equal the default) and
//! whether the request also fills the DASH members that were already server-filled.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::ops::types::shared_types as types;
use service_com_amazonaws_mediapackage_mediapackage::state;

// The one test harness of this target: the router builder and the HTTP round trip every
// sibling suite drives, so this suite cannot dispatch differently from them.
use crate::channel_lifecycle::{call, create_channel, emulator};

/// The channel every endpoint in this suite packages.
const CHANNEL: &str = "dash-defaults-channel";

// ---------------------------------------------------------------------------
// The members under test
// ---------------------------------------------------------------------------

/// The live-manifest members of a `DashPackage`, each with the default MediaPackage fills
/// an unset one with and one non-default value a request may send instead.
///
/// Sources: the member set and the enums from
/// `smithy_query(query="shape",shape="com.amazonaws.mediapackage#DashPackage,com.amazonaws.mediapackage#ManifestLayout,com.amazonaws.mediapackage#UtcTiming",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`,
/// and the configuration MediaPackage publishes for a live DASH endpoint for the values.
/// Written as literals rather than read from `crate::state`, so this suite pins the values
/// themselves instead of agreeing with whatever the service happens to hold.
fn live_manifest_members() -> Vec<(&'static str, Value, Value)> {
    vec![
        ("manifestLayout", json!("FULL"), json!("COMPACT")),
        ("utcTiming", json!("NONE"), json!("HTTP-HEAD")),
        ("manifestWindowSeconds", json!(60), json!(30)),
        ("minUpdatePeriodSeconds", json!(15), json!(5)),
        ("suggestedPresentationDelaySeconds", json!(25), json!(10)),
        ("segmentDurationSeconds", json!(2), json!(4)),
    ]
}

/// The `dashPackage` bodies under test, one row per point of the class.
fn cases() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> =
        // Nothing supplied: every member above must be materialised.
        vec![("nothing supplied".to_string(), json!({}))];

    // Each member supplied ALONE: the one sent survives and the other five are still
    // filled, so no member's fill depends on its siblings being absent.
    for (member, _, supplied) in live_manifest_members() {
        rows.push((
            format!("only {member} supplied"),
            json!({ member: supplied }),
        ));
    }

    let mut all = serde_json::Map::new();
    let mut at_defaults = serde_json::Map::new();
    for (member, default, supplied) in live_manifest_members() {
        all.insert(member.to_string(), supplied);
        at_defaults.insert(member.to_string(), default);
    }
    rows.push(("all supplied".to_string(), Value::Object(all)));
    // A request that sends exactly the defaults must be stored unchanged: filling is
    // idempotent, so a caller can send back an endpoint it read.
    rows.push((
        "every member supplied at its default".to_string(),
        Value::Object(at_defaults),
    ));
    // An explicit zero is a supplied value, not an unset member: "no window", "no delay"
    // and "no minimum update period" must survive instead of being replaced.
    rows.push((
        "explicit zero durations".to_string(),
        json!({
            "manifestWindowSeconds": 0,
            "minUpdatePeriodSeconds": 0,
            "suggestedPresentationDelaySeconds": 0,
            "segmentDurationSeconds": 0
        }),
    ));
    // The enum values the report did not cite, with the URI member that only has meaning
    // beside a non-NONE `utcTiming`.
    rows.push((
        "the remaining enum values".to_string(),
        json!({
            "manifestLayout": "DRM_TOP_LEVEL_COMPACT",
            "utcTiming": "HTTP-XSDATE",
            "utcTimingUri": "https://time.example.test/utc"
        }),
    ));
    // Supplied alongside the DASH members that were already server-filled: the two halves
    // of the block are filled by one rule, not two.
    rows.push((
        "mixed with the other DASH members".to_string(),
        json!({
            "manifestWindowSeconds": 90,
            "minBufferTimeSeconds": 4,
            "profile": "HBBTV_1_5",
            "periodTriggers": ["ADS"]
        }),
    ));
    rows
}

/// What `$.dashPackage` of every projection of an endpoint written with `sent` must carry:
/// the value `sent` supplied where it supplied one, else that member's default.
fn expected_dash_members(sent: &Value) -> Vec<(String, Value)> {
    live_manifest_members()
        .into_iter()
        .map(|(member, default, _)| {
            let value = sent.get(member).cloned().unwrap_or(default);
            (member.to_string(), value)
        })
        .collect()
}

/// Assert `body`'s `dashPackage` carries every member `sent` implies, and that the one
/// member with no published default is present only when `sent` carried it.
fn assert_dash_members(case: &str, projection: &str, sent: &Value, body: &Value) {
    let package = body.get("dashPackage").unwrap_or(&Value::Null);
    for (member, value) in expected_dash_members(sent) {
        let actual = package.get(&member);
        assert_eq!(
            actual,
            Some(&value),
            "case {case:?}: {projection} must carry dashPackage.{member} = {value}, got \
             {actual:?} in {body}"
        );
    }
    // `UtcTimingUri` is "the value attribute of the UTCTiming field when utcTiming is set
    // to HTTP-ISO, HTTP-HEAD or HTTP-XSDATE" and publishes no default, so it must never be
    // invented: a member without a published default stays exactly as the request sent it.
    let uri = package.get("utcTimingUri");
    assert_eq!(
        uri,
        sent.get("utcTimingUri"),
        "case {case:?}: {projection} must carry dashPackage.utcTimingUri only as sent, got \
         {uri:?} in {body}"
    );
}

#[tokio::test]
async fn every_projection_of_a_created_dash_endpoint_carries_the_live_manifest_defaults() {
    let router = emulator();
    create_channel(&router, CHANNEL).await;

    for (index, (case, package)) in cases().into_iter().enumerate() {
        let case = case.as_str();
        let id = format!("created-{index}");
        let request = json!({ "channelId": CHANNEL, "id": id, "dashPackage": package });

        // 1. The create response.
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&request)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );
        assert_dash_members(case, "the create response", &package, &created);

        // 2. The read path: the SAME stored record.
        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described}"
        );
        assert_dash_members(case, "DescribeOriginEndpoint", &package, &described);
        assert_eq!(
            described, created,
            "case {case:?}: DescribeOriginEndpoint must render the same stored record as the \
             create response"
        );

        // 3. The listing, which must reuse that same projection.
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/origin_endpoints?channelId={CHANNEL}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListOriginEndpoints expected 200, got {status} {listed}"
        );
        let entry = listed
            .get("originEndpoints")
            .and_then(Value::as_array)
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id.as_str()))
                    .cloned()
            })
            .unwrap_or(Value::Null);
        assert_dash_members(case, "the ListOriginEndpoints entry", &package, &entry);
        assert_eq!(
            entry, created,
            "case {case:?}: the listed endpoint must be the same stored record"
        );
    }
}

#[tokio::test]
async fn an_update_server_fills_the_same_dash_members_as_a_create() {
    let router = emulator();
    create_channel(&router, CHANNEL).await;

    // Each row is updated onto an endpoint created with EVERY member supplied, so a member
    // the update leaves unset has a stored non-default value that must NOT survive: an
    // update is a fresh write of the packaging, server-filled from the update's own body.
    let created_with: Value = cases()
        .into_iter()
        .find(|(case, _)| case == "all supplied")
        .map(|(_, package)| package)
        .expect("the all-supplied row");

    for (index, (case, package)) in cases().into_iter().enumerate() {
        let case = case.as_str();
        let id = format!("updated-{index}");
        let create = json!({ "channelId": CHANNEL, "id": id, "dashPackage": created_with });
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&create)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );

        let update = json!({ "dashPackage": package });
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{id}"),
            Some(&update),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateOriginEndpoint expected 200, got {status} {updated}"
        );
        assert_dash_members(case, "the update response", &package, &updated);

        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described}"
        );
        assert_dash_members(case, "DescribeOriginEndpoint", &package, &described);
        assert_eq!(
            described, updated,
            "case {case:?}: DescribeOriginEndpoint must render the same stored record as the \
             update response"
        );
    }
}

#[tokio::test]
async fn each_dash_live_manifest_default_is_a_value_the_model_and_the_service_accept() {
    // A filled-in enum default is not free text: it must be a member of the enum the model
    // declares for that member (`types::MANIFEST_LAYOUT_VALUES` / `types::UTC_TIMING_VALUES`,
    // generated from the Smithy model).
    let enums: Vec<(&str, &str, &[&str])> = vec![
        (
            "ManifestLayout",
            state::DEFAULT_DASH_MANIFEST_LAYOUT,
            types::MANIFEST_LAYOUT_VALUES,
        ),
        (
            "UtcTiming",
            state::DEFAULT_DASH_UTC_TIMING,
            types::UTC_TIMING_VALUES,
        ),
    ];
    for (member, default, accepted) in enums {
        assert!(
            accepted.contains(&default),
            "case {member:?}: the default {default:?} must be one of {accepted:?}"
        );
    }

    // And a filled-in value must be one the service would ACCEPT in a request, member by
    // member: a default the create path refuses is one a caller could not send back after
    // reading it. Driven on the wire, one endpoint per member, so each default is judged by
    // the service's own validation rather than by this suite restating it.
    let router = emulator();
    create_channel(&router, CHANNEL).await;
    for (index, (member, default, _)) in live_manifest_members().into_iter().enumerate() {
        let id = format!("default-accepted-{index}");
        let request = json!({
            "channelId": CHANNEL,
            "id": id,
            "dashPackage": { member: default.clone() }
        });
        let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&request)).await;
        assert_eq!(
            status, 200,
            "case {member:?}: a create sending the default {default} expected 200, got \
             {status} {created}"
        );
        let actual = created.pointer(&format!("/dashPackage/{member}"));
        assert_eq!(
            actual,
            Some(&default),
            "case {member:?}: the default {default} must be stored as sent, got {actual:?} \
             in {created}"
        );
    }
}
