// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IVS ad configuration, driven over the wire.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with
//! a restJson1 request, exactly as a signed `aws ivs` call arrives, on the harness
//! `channel_lifecycle` already owns — so what it asserts is the emulator's HTTP
//! behaviour, not a handler called directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `CreateAdConfiguration` persists an ad configuration under a freshly minted,
//!    distinct `AdConfigurationArn`, and `GetAdConfiguration` answers that exact
//!    stored resource for that ARN.
//! 2. A member the caller omits takes the default the model documents for it: `name`
//!    the empty string, `postRollConfiguration` disabled with `durationSeconds` 15,
//!    and `tags` the empty map. Inside `postRollConfiguration` both members are
//!    modelled `@required`, so a request that sends the structure must send both.
//! 3. A value outside a modelled constraint is refused with HTTP 400
//!    `ValidationException` and persists nothing; a well-formed ARN naming no stored
//!    ad configuration is HTTP 404 `ResourceNotFoundException`.
//! 4. `GetAdConfiguration` reports: repeating it answers identically.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::channel_lifecycle::{arn_is_well_formed, text, Ivs};

/// The ARN prefix every ad configuration this emulator mints must carry, from the
/// modelled `AdConfigurationArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:ad-configuration/[a-zA-Z0-9-]+$` plus the
/// canonical emulator account id.
const AD_CONFIGURATION_ARN_PREFIX: &str = "arn:aws:ivs:us-east-1:000000000000:ad-configuration/";

/// A MediaTailor playback configuration ARN of the form the modelled
/// `MediaTailorPlaybackConfigurationArn` pattern
/// `^arn:aws:mediatailor:[a-z0-9-]+:[0-9]+:playbackConfiguration/[a-zA-Z0-9-]+$`
/// pins, so the request's own constraints are satisfied and what each case varies is
/// the thing it means to vary.
fn playback_arn(suffix: &str) -> String {
    format!("arn:aws:mediatailor:us-east-1:000000000000:playbackConfiguration/{suffix}")
}

/// One MediaTailor integration entry, as the wire carries it.
fn playback_entry(suffix: &str) -> Value {
    json!({ "playbackConfigurationArn": playback_arn(suffix) })
}

/// Create an ad configuration and return the `adConfiguration` it answered.
async fn create(ivs: &Ivs, request: Value) -> Value {
    ivs.ok("CreateAdConfiguration", request).await["adConfiguration"].clone()
}

/// Read an ad configuration back and return the `adConfiguration` it answered.
async fn get(ivs: &Ivs, arn: &str) -> Value {
    ivs.ok("GetAdConfiguration", json!({ "arn": arn })).await["adConfiguration"].clone()
}

/// The `arn` of a returned `adConfiguration`.
fn arn_of(ad_configuration: &Value) -> String {
    text(ad_configuration, "arn")
}

/// A returned `adConfiguration` with its minted `arn` removed, which is the part a
/// case can state up front: the ARN itself is checked separately, against the
/// modelled pattern and for distinctness.
fn without_arn(ad_configuration: &Value) -> Value {
    let mut copy = ad_configuration.clone();
    copy.as_object_mut()
        .expect("adConfiguration is an object")
        .remove("arn");
    copy
}

/// One row of the create/read table: a request, and the whole resource (bar its
/// minted ARN) the service must answer and then store for it.
struct Case {
    case: String,
    request: Value,
    expected: Value,
}

/// The table: rows differ along what rule 1 and rule 2 generalise over — how many
/// MediaTailor entries the list carries (the modelled 1..3), whether `name`,
/// `postRollConfiguration`, its `enabled` flag and `tags` are sent at all, and the
/// values they take at each published bound.
fn table() -> Vec<Case> {
    // 128 characters, the modelled `AdConfigurationName` maximum length, drawn from
    // every class its pattern `^[a-zA-Z0-9-_]*$` allows.
    let longest_name = "aZ9-_".repeat(26)[..128].to_string();

    vec![
        Case {
            case: "only the required list, one entry: every other member defaults".to_string(),
            request: json!({ "mediaTailorPlaybackConfigurations": [playback_entry("one")] }),
            expected: json!({
                "name": "",
                "mediaTailorPlaybackConfigurations": [playback_entry("one")],
                "postRollConfiguration": { "durationSeconds": 15, "enabled": false },
                "tags": {},
            }),
        },
        Case {
            case: "two entries and a name".to_string(),
            request: json!({
                "name": "ads-two",
                "mediaTailorPlaybackConfigurations": [playback_entry("one"), playback_entry("two")],
            }),
            expected: json!({
                "name": "ads-two",
                "mediaTailorPlaybackConfigurations": [playback_entry("one"), playback_entry("two")],
                "postRollConfiguration": { "durationSeconds": 15, "enabled": false },
                "tags": {},
            }),
        },
        Case {
            case: "three entries (the modelled maximum), post-roll enabled at the lower bound"
                .to_string(),
            request: json!({
                "name": "ads_three",
                "mediaTailorPlaybackConfigurations": [
                    playback_entry("one"), playback_entry("two"), playback_entry("three"),
                ],
                "postRollConfiguration": { "durationSeconds": 1, "enabled": true },
            }),
            expected: json!({
                "name": "ads_three",
                "mediaTailorPlaybackConfigurations": [
                    playback_entry("one"), playback_entry("two"), playback_entry("three"),
                ],
                "postRollConfiguration": { "durationSeconds": 1, "enabled": true },
                "tags": {},
            }),
        },
        Case {
            case: "post-roll at the upper bound of AdDurationSeconds".to_string(),
            request: json!({
                "mediaTailorPlaybackConfigurations": [playback_entry("upper")],
                "postRollConfiguration": { "durationSeconds": 300, "enabled": true },
            }),
            expected: json!({
                "name": "",
                "mediaTailorPlaybackConfigurations": [playback_entry("upper")],
                "postRollConfiguration": { "durationSeconds": 300, "enabled": true },
                "tags": {},
            }),
        },
        Case {
            case: "post-roll with both members sent at an interior value".to_string(),
            request: json!({
                "mediaTailorPlaybackConfigurations": [playback_entry("interior")],
                "postRollConfiguration": { "durationSeconds": 30, "enabled": false },
            }),
            expected: json!({
                "name": "",
                "mediaTailorPlaybackConfigurations": [playback_entry("interior")],
                "postRollConfiguration": { "durationSeconds": 30, "enabled": false },
                "tags": {},
            }),
        },
        Case {
            case: "an explicitly empty name is the documented default value".to_string(),
            request: json!({
                "name": "",
                "mediaTailorPlaybackConfigurations": [playback_entry("empty-name")],
            }),
            expected: json!({
                "name": "",
                "mediaTailorPlaybackConfigurations": [playback_entry("empty-name")],
                "postRollConfiguration": { "durationSeconds": 15, "enabled": false },
                "tags": {},
            }),
        },
        Case {
            case: "a name at the modelled 128-character maximum".to_string(),
            request: json!({
                "name": longest_name,
                "mediaTailorPlaybackConfigurations": [playback_entry("longest")],
            }),
            expected: json!({
                "name": longest_name,
                "mediaTailorPlaybackConfigurations": [playback_entry("longest")],
                "postRollConfiguration": { "durationSeconds": 15, "enabled": false },
                "tags": {},
            }),
        },
        Case {
            case: "tags are stored as sent".to_string(),
            request: json!({
                "name": "tagged",
                "mediaTailorPlaybackConfigurations": [playback_entry("tagged")],
                "tags": { "team": "video", "env": "test" },
            }),
            expected: json!({
                "name": "tagged",
                "mediaTailorPlaybackConfigurations": [playback_entry("tagged")],
                "postRollConfiguration": { "durationSeconds": 15, "enabled": false },
                "tags": { "team": "video", "env": "test" },
            }),
        },
        Case {
            case: "an explicitly empty tag map".to_string(),
            request: json!({
                "mediaTailorPlaybackConfigurations": [playback_entry("untagged")],
                "tags": {},
            }),
            expected: json!({
                "name": "",
                "mediaTailorPlaybackConfigurations": [playback_entry("untagged")],
                "postRollConfiguration": { "durationSeconds": 15, "enabled": false },
                "tags": {},
            }),
        },
    ]
}

/// Rules 1 and 2: each row's create answers the resource the model's defaults
/// dictate, mints an ARN of the modelled form, and `GetAdConfiguration` answers that
/// same stored resource afterwards — including for rows created earlier in the run,
/// so a later create disturbs nothing.
#[tokio::test]
async fn create_persists_every_member_and_get_reads_it_back() {
    let ivs = Ivs::new();
    let mut created: Vec<(String, String, Value)> = Vec::new();

    for Case {
        case,
        request,
        expected,
    } in table()
    {
        let created_body = create(&ivs, request.clone()).await;
        assert_eq!(
            without_arn(&created_body),
            expected,
            "case {case:?}: expected the create to answer {expected}, got {created_body}"
        );

        let arn = arn_of(&created_body);
        assert!(
            arn_is_well_formed(&arn, AD_CONFIGURATION_ARN_PREFIX),
            "case {case:?}: expected an arn matching the modelled AdConfigurationArn \
             pattern under {AD_CONFIGURATION_ARN_PREFIX}, got {arn:?}"
        );

        let fetched = get(&ivs, &arn).await;
        assert_eq!(
            fetched, created_body,
            "case {case:?}: expected GetAdConfiguration to answer the created resource \
             {created_body}, got {fetched}"
        );
        created.push((case, arn, created_body));
    }

    // Every create minted its own ARN, and every earlier resource still reads back
    // unchanged after the later creates.
    let arns: BTreeSet<&String> = created.iter().map(|(_, arn, _)| arn).collect();
    assert_eq!(
        arns.len(),
        created.len(),
        "expected {} distinct ad configuration arns, got {:?}",
        created.len(),
        arns
    );
    for (case, arn, expected) in &created {
        let fetched = get(&ivs, arn).await;
        assert_eq!(
            &fetched, expected,
            "case {case:?}: expected {arn} to still read back as {expected} after the \
             later creates, got {fetched}"
        );
    }
}

/// Rule 1 for two separate creates of the SAME request: identical input is two
/// resources, each reachable under its own ARN.
#[tokio::test]
async fn identical_requests_create_separate_resources() {
    let ivs = Ivs::new();
    let request = json!({
        "name": "same-request",
        "mediaTailorPlaybackConfigurations": [playback_entry("same")],
    });

    let mut arns = BTreeSet::new();
    for attempt in 1..=4 {
        let created = create(&ivs, request.clone()).await;
        let arn = arn_of(&created);
        assert!(
            arns.insert(arn.clone()),
            "case attempt {attempt}: expected a freshly minted arn, got the already \
             seen {arn:?}"
        );
        let fetched = get(&ivs, &arn).await;
        assert_eq!(
            fetched["name"],
            json!("same-request"),
            "case attempt {attempt}: expected the stored name to be \"same-request\", \
             got {fetched}"
        );
    }
}

/// Rule 3's refusal half for the create: a request outside a modelled constraint is
/// a 400 `ValidationException`, and nothing it carried is persisted.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let ivs = Ivs::new();
    // A resource that must be the ONLY thing in the store when this test ends.
    let survivor = arn_of(
        &create(
            &ivs,
            json!({
                "name": "survivor",
                "mediaTailorPlaybackConfigurations": [playback_entry("survivor")],
            }),
        )
        .await,
    );

    // One past each published bound of the modelled constraints, plus each required
    // member left out.
    let refused: Vec<(&str, Value)> = vec![
        (
            "mediaTailorPlaybackConfigurations absent (required)",
            json!({ "name": "no-list" }),
        ),
        (
            "an empty list, one below the modelled minimum of 1",
            json!({ "mediaTailorPlaybackConfigurations": [] }),
        ),
        (
            "four entries, one past the modelled maximum of 3",
            json!({ "mediaTailorPlaybackConfigurations": [
                playback_entry("one"), playback_entry("two"),
                playback_entry("three"), playback_entry("four"),
            ] }),
        ),
        (
            "a playbackConfigurationArn outside its modelled pattern",
            json!({ "mediaTailorPlaybackConfigurations": [
                { "playbackConfigurationArn": "not-an-arn" },
            ] }),
        ),
        (
            "a name 129 characters long, one past the modelled maximum",
            json!({
                "name": "a".repeat(129),
                "mediaTailorPlaybackConfigurations": [playback_entry("one")],
            }),
        ),
        (
            "a name outside the modelled ^[a-zA-Z0-9-_]*$ pattern",
            json!({
                "name": "has spaces",
                "mediaTailorPlaybackConfigurations": [playback_entry("one")],
            }),
        ),
        (
            "durationSeconds 0, one below the modelled minimum of 1",
            json!({
                "mediaTailorPlaybackConfigurations": [playback_entry("one")],
                "postRollConfiguration": { "durationSeconds": 0, "enabled": true },
            }),
        ),
        (
            "durationSeconds 301, one past the modelled maximum of 300",
            json!({
                "mediaTailorPlaybackConfigurations": [playback_entry("one")],
                "postRollConfiguration": { "durationSeconds": 301, "enabled": true },
            }),
        ),
        (
            "postRollConfiguration without its required durationSeconds",
            json!({
                "mediaTailorPlaybackConfigurations": [playback_entry("one")],
                "postRollConfiguration": { "enabled": true },
            }),
        ),
        (
            "postRollConfiguration without its required enabled",
            json!({
                "mediaTailorPlaybackConfigurations": [playback_entry("one")],
                "postRollConfiguration": { "durationSeconds": 30 },
            }),
        ),
    ];

    for (case, request) in refused {
        let reply = ivs.call("CreateAdConfiguration", &request).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // None of the refusals persisted anything: the one resource that was created is
    // still exactly what the store holds for its ARN, and nothing else is readable.
    let fetched = get(&ivs, &survivor).await;
    assert_eq!(
        fetched["name"],
        json!("survivor"),
        "case after the refusals: expected the surviving resource to be unchanged, \
         got {fetched}"
    );
}

/// Rule 3 for the read: a well-formed ARN naming nothing stored is a 404, a value
/// outside the modelled `AdConfigurationArn` pattern is a 400, and neither disturbs
/// what is stored.
#[tokio::test]
async fn reading_an_arn_the_store_does_not_hold_is_a_404() {
    let ivs = Ivs::new();
    let live = arn_of(
        &create(
            &ivs,
            json!({
                "name": "live",
                "mediaTailorPlaybackConfigurations": [playback_entry("live")],
            }),
        )
        .await,
    );

    // Well formed, but naming no stored ad configuration: an unknown id, another
    // account, another region, and the id of a DIFFERENT resource kind's ARN shape.
    let not_found = [
        format!("{AD_CONFIGURATION_ARN_PREFIX}doesnotexist"),
        "arn:aws:ivs:us-east-1:123456789012:ad-configuration/abcdef123456".to_string(),
        "arn:aws:ivs:eu-west-1:000000000000:ad-configuration/abcdef123456".to_string(),
    ];
    for arn in not_found {
        let reply = ivs.call("GetAdConfiguration", &json!({ "arn": arn })).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {arn:?}: a well-formed arn naming no ad configuration is a 404, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    let malformed = [
        "not-an-arn",
        "",
        "arn:aws:ivs:us-east-1:000000000000:channel/abcdef123456",
        "arn:aws:ivs:US-EAST-1:000000000000:ad-configuration/abcdef123456",
        "arn:aws:ivs:us-east-1:000000000000:ad-configuration/has_underscore",
    ];
    for arn in malformed {
        let reply = ivs.call("GetAdConfiguration", &json!({ "arn": arn })).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {arn:?}: a value outside the modelled AdConfigurationArn pattern is a \
             400, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // A missing required member is the same refusal class.
    let reply = ivs.call("GetAdConfiguration", &json!({})).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (400, "ValidationException"),
        "case absent arn: expected 400 ValidationException, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );

    // And none of that disturbed the resource that does exist.
    let fetched = get(&ivs, &live).await;
    assert_eq!(
        arn_of(&fetched),
        live,
        "case the live resource: expected it to still read back under {live}, got {fetched}"
    );
}

/// Rule 4: `GetAdConfiguration` is a reporting operation, so repeating it answers
/// identically — a read never changes what a later caller sees.
#[tokio::test]
async fn reads_are_repeatable() {
    let ivs = Ivs::new();
    let arn = arn_of(
        &create(
            &ivs,
            json!({
                "name": "repeatable",
                "mediaTailorPlaybackConfigurations": [
                    playback_entry("one"), playback_entry("two"),
                ],
                "postRollConfiguration": { "durationSeconds": 42, "enabled": true },
                "tags": { "k": "v" },
            }),
        )
        .await,
    );

    let first = get(&ivs, &arn).await;
    for repeat in 2..=4 {
        let again = get(&ivs, &arn).await;
        assert_eq!(
            again, first,
            "case read {repeat} of {arn}: expected the same resource as the first read \
             {first}, got {again}"
        );
    }
}
