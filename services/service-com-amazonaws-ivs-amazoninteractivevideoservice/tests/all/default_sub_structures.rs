// SPDX-License-Identifier: CC0-1.0

//! The nested configuration structures every channel and recording-configuration
//! projection reports, driven over the wire through `MultiServiceRouter::dispatch`
//! exactly as a signed `aws ivs` call arrives.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: a
//! nested configuration structure whose every member carries a documented default is
//! MATERIALISED on the resource when it is created, not echoed from the request — so
//! every projection of that resource (the create answer, a read, an update, a batch
//! read) reports the structure with the members the request sent and the documented
//! default for each member it left out, whether or not the request named the
//! structure at all. The structures this covers are `Channel`'s
//! `multitrackInputConfiguration` and `RecordingConfiguration`'s
//! `thumbnailConfiguration` and `renditionConfiguration`.

use serde_json::{json, Value};

use super::channel_lifecycle::{text, Ivs, CHANNEL_ARN_PREFIX};

/// Every value the model's `MultitrackPolicy` enumeration declares (source:
/// `smithy_query(query="shape", shape="com.amazonaws.ivs#MultitrackPolicy")` —
/// ALLOW | REQUIRE).
const MULTITRACK_POLICIES: &[&str] = &["ALLOW", "REQUIRE"];

/// Every value the model's `MultitrackMaximumResolution` enumeration declares
/// (source: `smithy_query(query="shape",
/// shape="com.amazonaws.ivs#MultitrackMaximumResolution")` — SD | HD | FULL_HD).
const MULTITRACK_MAXIMUM_RESOLUTIONS: &[&str] = &["SD", "HD", "FULL_HD"];

/// The `multitrackInputConfiguration` a channel reports when its request chose
/// nothing for one: `enabled` is the modelled `@default(false)`, and `policy` /
/// `maximumResolution` are the pair the service applies when the request named
/// neither.
fn defaulted_multitrack() -> Value {
    json!({ "enabled": false, "policy": "ALLOW", "maximumResolution": "FULL_HD" })
}

/// The `thumbnailConfiguration` a recording configuration reports when its request
/// chose nothing for one: `recordingMode` "Default: `INTERVAL`",
/// `targetIntervalSeconds` "Default: 60" (documented for INTERVAL mode only) and
/// `storage` "Default: `SEQUENTIAL`", from the `ThumbnailConfiguration` member docs.
/// `resolution` is documented "Default: Null", so it is the one member that stays
/// absent.
fn defaulted_thumbnail() -> Value {
    json!({ "recordingMode": "INTERVAL", "targetIntervalSeconds": 60, "storage": ["SEQUENTIAL"] })
}

/// The `renditionConfiguration` a recording configuration reports when its request
/// chose nothing for one, from the `RenditionConfiguration$renditionSelection` doc
/// ("Default: `ALL`"). `renditions` is irrelevant unless the selection is `CUSTOM`,
/// so it stays absent.
fn defaulted_rendition() -> Value {
    json!({ "renditionSelection": "ALL" })
}

/// An S3 destination naming `bucket`, the only destination type the model declares.
fn destination(bucket: &str) -> Value {
    json!({ "s3": { "bucketName": bucket } })
}

/// One channel row: the case name, the `CreateChannel` body, and the
/// `multitrackInputConfiguration` every projection of that channel must report.
type ChannelCase = (String, Value, Value);

/// The rows the channel rule is exercised over. They differ along what the rule
/// generalises over: whether the structure is named at all, whether it is named but
/// empty, which of its members are sent, and the value chosen for each — every
/// member of both enumerations, including the values the defaults themselves do not
/// use.
fn channel_cases() -> Vec<ChannelCase> {
    let mut cases: Vec<ChannelCase> = vec![
        (
            "no multitrackInputConfiguration at all".to_string(),
            json!({ "name": "mt-absent" }),
            defaulted_multitrack(),
        ),
        (
            "a multitrackInputConfiguration sent empty".to_string(),
            json!({ "name": "mt-empty", "multitrackInputConfiguration": {} }),
            defaulted_multitrack(),
        ),
        (
            "enabled sent false and nothing else".to_string(),
            json!({ "name": "mt-off",
                    "multitrackInputConfiguration": { "enabled": false } }),
            defaulted_multitrack(),
        ),
        (
            "policy alone, with multitrack left disabled".to_string(),
            json!({ "name": "mt-policy",
                    "multitrackInputConfiguration": { "policy": "REQUIRE" } }),
            json!({ "enabled": false, "policy": "REQUIRE", "maximumResolution": "FULL_HD" }),
        ),
        (
            "maximumResolution alone, with multitrack left disabled".to_string(),
            json!({ "name": "mt-resolution",
                    "multitrackInputConfiguration": { "maximumResolution": "SD" } }),
            json!({ "enabled": false, "policy": "ALLOW", "maximumResolution": "SD" }),
        ),
        (
            "both enum members sent, with multitrack left disabled".to_string(),
            json!({ "name": "mt-pair",
                    "multitrackInputConfiguration": { "policy": "REQUIRE",
                                                      "maximumResolution": "HD" } }),
            json!({ "enabled": false, "policy": "REQUIRE", "maximumResolution": "HD" }),
        ),
    ];
    // Multitrack ENABLED, over every (policy, maximumResolution) pair the two
    // enumerations declare. `containerFormat` must be FRAGMENTED_MP4 for an enabled
    // configuration, which is the one cross-member rule the model states for it.
    for policy in MULTITRACK_POLICIES {
        for resolution in MULTITRACK_MAXIMUM_RESOLUTIONS {
            let sent = json!({ "enabled": true, "policy": policy,
                               "maximumResolution": resolution });
            cases.push((
                format!("multitrack enabled with policy {policy} and resolution {resolution}"),
                json!({ "name": "mt-on", "containerFormat": "FRAGMENTED_MP4",
                        "multitrackInputConfiguration": sent.clone() }),
                sent,
            ));
        }
    }
    cases
}

/// The rule for `Channel`: the create answer, the single-channel read, the batch read
/// and an update that does not name the structure all report the same materialised
/// `multitrackInputConfiguration`.
#[tokio::test]
async fn every_channel_projection_reports_the_materialised_multitrack_configuration() {
    const MEMBER: &str = "multitrackInputConfiguration";

    for (case, request, expected) in channel_cases() {
        let ivs = Ivs::new();
        let created = ivs.create(request.clone()).await["channel"].clone();
        assert_eq!(
            created.get(MEMBER).cloned().unwrap_or(Value::Null),
            expected,
            "case {case:?}: CreateChannel must report {expected}, got {created}"
        );

        let arn = text(&created, "arn");
        let fetched = ivs.ok("GetChannel", json!({ "arn": arn })).await["channel"].clone();
        assert_eq!(
            fetched.get(MEMBER).cloned().unwrap_or(Value::Null),
            expected,
            "case {case:?}: GetChannel must report {expected}, got {fetched}"
        );

        let batch = ivs
            .ok("BatchGetChannel", json!({ "arns": [arn.clone()] }))
            .await;
        let batched = batch["channels"]
            .as_array()
            .and_then(|channels| channels.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            batched.get(MEMBER).cloned().unwrap_or(Value::Null),
            expected,
            "case {case:?}: BatchGetChannel must report {expected}, got {batch}"
        );

        // An update that names other members keeps the structure the create
        // materialised, and reports it on its own answer.
        let updated = ivs
            .ok("UpdateChannel", json!({ "arn": arn, "name": "renamed" }))
            .await["channel"]
            .clone();
        assert_eq!(
            updated.get(MEMBER).cloned().unwrap_or(Value::Null),
            expected,
            "case {case:?}: UpdateChannel must report the stored {expected}, got {updated}"
        );

        // `ChannelSummary` declares no multitrack member at all, so the list
        // projection reports none: materialising a default is about the members a
        // projection HAS.
        let listed = ivs.ok("ListChannels", json!({})).await;
        let summary = listed["channels"]
            .as_array()
            .and_then(|channels| channels.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            summary.get(MEMBER).is_none(),
            "case {case:?}: ChannelSummary declares no {MEMBER} member, got {summary}"
        );
    }
}

/// The update half of the rule: a partial structure sent on an update is filled from
/// the same defaults, and a structure the update does not name is left exactly as the
/// store holds it.
#[tokio::test]
async fn an_update_materialises_the_members_its_multitrack_configuration_omits() {
    const MEMBER: &str = "multitrackInputConfiguration";
    // Rows differ in which members the UPDATE names, including none at all.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the update names no multitrack configuration",
            json!({}),
            json!({ "enabled": false, "policy": "REQUIRE", "maximumResolution": "SD" }),
        ),
        (
            "the update sends an empty multitrack configuration",
            json!({ "multitrackInputConfiguration": {} }),
            defaulted_multitrack(),
        ),
        (
            "the update sends policy alone",
            json!({ "multitrackInputConfiguration": { "policy": "ALLOW" } }),
            json!({ "enabled": false, "policy": "ALLOW", "maximumResolution": "FULL_HD" }),
        ),
        (
            "the update sends maximumResolution alone",
            json!({ "multitrackInputConfiguration": { "maximumResolution": "HD" } }),
            json!({ "enabled": false, "policy": "ALLOW", "maximumResolution": "HD" }),
        ),
        (
            "the update enables multitrack with both members",
            json!({ "containerFormat": "FRAGMENTED_MP4",
                    "multitrackInputConfiguration": { "enabled": true, "policy": "REQUIRE",
                                                      "maximumResolution": "FULL_HD" } }),
            json!({ "enabled": true, "policy": "REQUIRE", "maximumResolution": "FULL_HD" }),
        ),
    ];

    for (case, update, expected) in cases {
        let ivs = Ivs::new();
        let arn = ivs
            .create_arn(json!({ "name": "updatable",
                                "multitrackInputConfiguration": { "policy": "REQUIRE",
                                                                  "maximumResolution": "SD" } }))
            .await;

        let mut request = update.clone();
        request["arn"] = json!(arn);
        let updated = ivs.ok("UpdateChannel", request).await["channel"].clone();
        assert_eq!(
            updated.get(MEMBER).cloned().unwrap_or(Value::Null),
            expected,
            "case {case:?}: UpdateChannel must report {expected}, got {updated}"
        );

        // The mutation is in state, not only in the answer.
        let fetched = ivs.ok("GetChannel", json!({ "arn": arn })).await["channel"].clone();
        assert_eq!(
            fetched.get(MEMBER).cloned().unwrap_or(Value::Null),
            expected,
            "case {case:?}: GetChannel must observe {expected}, got {fetched}"
        );
    }
}

/// One recording-configuration row: the case name, the members the create request
/// adds to the required destination, and the `thumbnailConfiguration` and
/// `renditionConfiguration` every projection must report.
type RecordingCase = (&'static str, Value, Value, Value);

/// The rule for `RecordingConfiguration`: both nested configurations are reported by
/// the create answer and by the read, whichever of the two (if either) the request
/// named.
#[tokio::test]
async fn every_recording_configuration_projection_reports_both_configurations() {
    // Rows differ along what the rule generalises over: which of the two structures
    // the request names, whether it is named but empty, which members of each are
    // sent, and the values chosen — including `storage` LATEST and a `resolution`,
    // which the defaults never produce.
    let cases: Vec<RecordingCase> = vec![
        (
            "only the required destination",
            json!({}),
            defaulted_thumbnail(),
            defaulted_rendition(),
        ),
        (
            "a renditionConfiguration alone, selecting ALL",
            json!({ "renditionConfiguration": { "renditionSelection": "ALL" } }),
            defaulted_thumbnail(),
            json!({ "renditionSelection": "ALL" }),
        ),
        (
            "a renditionConfiguration alone, selecting NONE",
            json!({ "renditionConfiguration": { "renditionSelection": "NONE" } }),
            defaulted_thumbnail(),
            json!({ "renditionSelection": "NONE" }),
        ),
        (
            "a renditionConfiguration alone, selecting a custom set",
            json!({ "renditionConfiguration": { "renditionSelection": "CUSTOM",
                                                "renditions": ["HD", "LOWEST_RESOLUTION"] } }),
            defaulted_thumbnail(),
            json!({ "renditionSelection": "CUSTOM", "renditions": ["HD", "LOWEST_RESOLUTION"] }),
        ),
        (
            "a renditionConfiguration sent empty",
            json!({ "renditionConfiguration": {} }),
            defaulted_thumbnail(),
            defaulted_rendition(),
        ),
        (
            "a thumbnailConfiguration alone, at the documented defaults",
            json!({ "thumbnailConfiguration": { "recordingMode": "INTERVAL",
                                                "targetIntervalSeconds": 60,
                                                "storage": ["SEQUENTIAL"] } }),
            defaulted_thumbnail(),
            defaulted_rendition(),
        ),
        (
            "a thumbnailConfiguration alone, with thumbnails disabled",
            json!({ "thumbnailConfiguration": { "recordingMode": "DISABLED" } }),
            json!({ "recordingMode": "DISABLED", "storage": ["SEQUENTIAL"] }),
            defaulted_rendition(),
        ),
        (
            "a thumbnailConfiguration alone, every member chosen",
            json!({ "thumbnailConfiguration": { "recordingMode": "INTERVAL",
                                                "targetIntervalSeconds": 1,
                                                "resolution": "FULL_HD",
                                                "storage": ["LATEST"] } }),
            json!({ "recordingMode": "INTERVAL", "targetIntervalSeconds": 1,
                    "resolution": "FULL_HD", "storage": ["LATEST"] }),
            defaulted_rendition(),
        ),
        (
            "a thumbnailConfiguration sent empty",
            json!({ "thumbnailConfiguration": {} }),
            defaulted_thumbnail(),
            defaulted_rendition(),
        ),
        (
            "both configurations, each partly specified",
            json!({ "thumbnailConfiguration": { "storage": ["SEQUENTIAL", "LATEST"] },
                    "renditionConfiguration": { "renditionSelection": "NONE" } }),
            json!({ "recordingMode": "INTERVAL", "targetIntervalSeconds": 60,
                    "storage": ["SEQUENTIAL", "LATEST"] }),
            json!({ "renditionSelection": "NONE" }),
        ),
    ];

    let ivs = Ivs::new();
    for (case, extras, expected_thumbnail, expected_rendition) in cases {
        let mut request = json!({ "destinationConfiguration": destination("ivs-recordings") });
        for (member, value) in extras.as_object().expect("the row's extras are an object") {
            request[member] = value.clone();
        }

        let created = ivs.ok("CreateRecordingConfiguration", request.clone()).await
            ["recordingConfiguration"]
            .clone();
        for (member, expected) in [
            ("thumbnailConfiguration", &expected_thumbnail),
            ("renditionConfiguration", &expected_rendition),
        ] {
            assert_eq!(
                &created.get(member).cloned().unwrap_or(Value::Null),
                expected,
                "case {case:?}: CreateRecordingConfiguration must report {member} \
                 {expected}, got {created}"
            );
        }

        let arn = text(&created, "arn");
        let fetched = ivs
            .ok("GetRecordingConfiguration", json!({ "arn": arn }))
            .await["recordingConfiguration"]
            .clone();
        for (member, expected) in [
            ("thumbnailConfiguration", &expected_thumbnail),
            ("renditionConfiguration", &expected_rendition),
        ] {
            assert_eq!(
                &fetched.get(member).cloned().unwrap_or(Value::Null),
                expected,
                "case {case:?}: GetRecordingConfiguration must report {member} \
                 {expected}, got {fetched}"
            );
        }
    }
}

/// A channel this emulator never minted is not projected at all: the rule above adds
/// members to the resources that exist, and changes nothing about an ARN naming none.
#[tokio::test]
async fn an_absent_channel_is_still_reported_as_absent() {
    let ivs = Ivs::new();
    let absent = format!("{CHANNEL_ARN_PREFIX}absentchannel");

    let reply = ivs.call("GetChannel", &json!({ "arn": absent })).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case an ARN naming no channel: expected 404 ResourceNotFoundException, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );

    let batch = ivs
        .ok("BatchGetChannel", json!({ "arns": [absent.clone()] }))
        .await;
    assert_eq!(
        batch["channels"].as_array().map(Vec::len),
        Some(0),
        "case an ARN naming no channel: the batch read reports no channel, got {batch}"
    );
}
