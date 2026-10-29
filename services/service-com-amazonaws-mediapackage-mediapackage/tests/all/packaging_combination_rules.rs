// SPDX-License-Identifier: CC0-1.0

//! Packaging members an OriginEndpoint may only carry TOGETHER with a particular value of
//! another member.
//!
//! RULE under test: where the model states a packaging member's requirement on ANOTHER
//! member of the same configuration, a `CreateOriginEndpoint` or `UpdateOriginEndpoint`
//! request whose members break that requirement is refused with
//! `UnprocessableEntityException` (HTTP 422), keyed by the member the caller has to change
//! and stating that member's requirement, and persists nothing — the create stores no
//! endpoint and the update leaves the stored one exactly as it was. A request whose members
//! satisfy it is stored and rendered back by every projection of the endpoint. The two
//! requirements this suite covers, one group of cases each:
//!
//! 1. AD MARKERS. `AdMarkers`' DATERANGE option "inserts EXT-X-DATERANGE tags to signal ad
//!    and program transition events in HLS and CMAF manifests. For this option, you must set
//!    a programDateTimeIntervalSeconds value that is greater than 0"
//!    (`smithy_query(query="doc",shape="com.amazonaws.mediapackage#HlsPackage",member="AdMarkers",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`,
//!    the same sentence on `HlsManifestCreateOrUpdateParameters$AdMarkers`). An OMITTED
//!    interval breaks it exactly as an explicit zero does, because an omitted one is stored
//!    and reported as `state::DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS` (zero). Every
//!    OTHER option of the enum states no such requirement, so it is accepted with any
//!    interval.
//! 2. ENCRYPTION CONTRACT. `EncryptionContractConfiguration` "can be used for DASH or CMAF
//!    endpoints that use SPEKE 2.0 ... You must disable key rotation for this endpoint by
//!    setting keyRotationIntervalSeconds to 0"
//!    (`smithy_query(query="doc",shape="com.amazonaws.mediapackage#EncryptionContractConfiguration",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
//!    so a contract asked for beside a live key rotation, or on a packaging format that does
//!    not use SPEKE 2.0, is refused; a key rotation asked for WITHOUT a contract is bounded
//!    by the shared duration floor alone and stored.
//!
//! Neither is a modelled constraint — `AdMarkers` is a plain enum, the interval and the
//! rotation target the bare `__integer` with no `@range`, and the contract is an optional
//! structure member — so the framework's constraint walk can refuse none of it and the rules
//! are asserted here over a SET of inputs rather than on the one value an issue reported.
//!
//! The rows differ along what each rule generalises over: WHICH configuration carries the
//! pair and how deeply it is nested (an `HlsPackage`, the first child manifest of a CMAF
//! package and a later one, so the refusal's list index is exercised; each of the four
//! packaging formats' encryption blocks), the value the dependent member carries (omitted,
//! zero, one, a documented interval, the largest `__integer`), which option of the
//! ad-marker enum is selected (the whole catalog, from the model's own enum), the presets
//! the contract names, and which operation carries the request (create or update).
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//! The client, the region and the helpers are the ones `tests/all/channel_lifecycle.rs`
//! owns.

use serde_json::{json, Map, Value};
use service_com_amazonaws_mediapackage_mediapackage::ops::types::shared_types as types;
use service_com_amazonaws_mediapackage_mediapackage::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, create_channel, emulator, error_code};

/// The channel every endpoint below packages.
const CHANNEL_ID: &str = "combination-channel";

/// An id a create or an update can be addressed by, built from a case name: the service
/// admits letters, digits and `state::ID_PUNCTUATION_CHARACTERS`, so the path and value
/// punctuation a case name carries becomes `-`.
fn endpoint_id(case: &str) -> String {
    case.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// A SPEKE key provider, which every packaging format's `Encryption` requires
/// (`SpekeKeyProvider$ResourceId`, `$RoleArn`, `$SystemIds` and `$Url` are all required), so
/// an encryption case reaches the content rules instead of being refused for a missing
/// member.
fn speke() -> Value {
    json!({
        "resourceId": "endpoint-combinations",
        "roleArn": "arn:aws:iam::000000000000:role/mediapackage-speke",
        "systemIds": ["81376844-f976-481e-a84e-cc25d39b0b33"],
        "url": "https://speke.example.com/v1/copyProtection"
    })
}

/// A member set carrying only the members a case supplies, so an omitted one is a member
/// the request really does not send.
fn members<'a, I>(supplied: I) -> Value
where
    I: IntoIterator<Item = (&'a str, Option<Value>)>,
{
    let mut object = Map::new();
    for (member, value) in supplied {
        if let Some(value) = value {
            object.insert(member.to_string(), value);
        }
    }
    Value::Object(object)
}

/// A request body naming exactly ONE packaging configuration — an endpoint packages its
/// channel in exactly one format — so the member a refusal is keyed by is never ambiguous.
fn package_body(block: &str, package: Value) -> Value {
    let mut object = Map::new();
    object.insert(block.to_string(), package);
    Value::Object(object)
}

/// `body` plus the ids a create needs.
fn create_body(body: &Value, id: &str) -> Value {
    let mut body = body.clone();
    let object = body
        .as_object_mut()
        .expect("every case body is a JSON object");
    object.insert("id".to_string(), json!(id));
    object.insert("channelId".to_string(), json!(CHANNEL_ID));
    body
}

/// Assert that `body` is refused on create with the service's 422 envelope keyed by
/// `member` and stating `requirement`, and that nothing was stored.
async fn refused_on_create(
    router: &MultiServiceRouter,
    case: &str,
    body: &Value,
    member: &str,
    requirement: &str,
) {
    let id = endpoint_id(case);
    let (status, refusal) = call(
        router,
        "POST",
        "/origin_endpoints",
        Some(&create_body(body, &id)),
    )
    .await;
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {refusal:?}"
    );
    assert_eq!(
        error_code(&refusal),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
    );
    assert_eq!(
        refusal.pointer(&format!("/errors/{member}")),
        Some(&json!(requirement)),
        "case {case:?}: the refusal must be keyed by {member:?} and state the requirement, \
         got {refusal:?}"
    );

    // Nothing was persisted: the endpoint the refused create named is unreadable.
    let (status, described) = call(router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 404,
        "case {case:?}: the refused create must store no endpoint, got {status} {described:?}"
    );
}

/// Assert that `body` is accepted on create, and that the stored endpoint a later read
/// renders is the one the create answered with. Answers the create response.
async fn accepted_on_create(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let id = endpoint_id(case);
    let (status, created) = call(
        router,
        "POST",
        "/origin_endpoints",
        Some(&create_body(body, &id)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {created:?}"
    );
    let (status, described) = call(router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
    );
    assert_eq!(
        described, created,
        "case {case:?}: the read must render the stored record, got {described:?}"
    );
    created
}

// ---------------------------------------------------------------------------
// 1. DATERANGE ad markers require a date-time interval greater than zero
// ---------------------------------------------------------------------------

/// One manifest configuration that can carry `AdMarkers`: the wire path its members travel
/// under, and the request body that puts an ad-marker option and an interval in it.
///
/// The two shapes below are the COMPLETE set of members targeting `AdMarkers` on this
/// operation's input (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#HlsPackage,com.amazonaws.mediapackage#DashPackage,com.amazonaws.mediapackage#CmafPackageCreateOrUpdateParameters,com.amazonaws.mediapackage#MssPackage,com.amazonaws.mediapackage#HlsManifestCreateOrUpdateParameters",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`:
/// an `HlsPackage` and a CMAF package's child manifests; a DASH or MSS package has no such
/// member).
struct Manifest {
    /// The wire path PREFIX of the manifest configuration, which the refusal's member path
    /// starts with.
    prefix: &'static str,
    /// The body carrying `ad_markers` and `interval` in that configuration, each omitted
    /// when the case supplies none.
    body: fn(Option<&str>, Option<i32>) -> Value,
}

/// The members a manifest configuration carries for one case.
fn manifest_members(ad_markers: Option<&str>, interval: Option<i32>) -> Value {
    members([
        ("adMarkers", ad_markers.map(|value| json!(value))),
        (
            "programDateTimeIntervalSeconds",
            interval.map(|value| json!(value)),
        ),
    ])
}

/// A CMAF child manifest carrying `id` and the case's members.
fn child(id: &str, ad_markers: Option<&str>, interval: Option<i32>) -> Value {
    let mut child = manifest_members(ad_markers, interval);
    child
        .as_object_mut()
        .expect("manifest_members answers an object")
        .insert("id".to_string(), json!(id));
    child
}

/// The manifest configurations the ad-marker rule holds for: the endpoint's own HLS package,
/// the first child manifest of a CMAF package, and a LATER child of a three-child package,
/// so the index the refusal names is a real one rather than always zero.
const MANIFESTS: &[Manifest] = &[
    Manifest {
        prefix: "hlsPackage.",
        body: |ad_markers, interval| {
            package_body("hlsPackage", manifest_members(ad_markers, interval))
        },
    },
    Manifest {
        prefix: "cmafPackage.hlsManifests[0].",
        body: |ad_markers, interval| {
            package_body(
                "cmafPackage",
                json!({ "hlsManifests": [child("only", ad_markers, interval)] }),
            )
        },
    },
    Manifest {
        prefix: "cmafPackage.hlsManifests[2].",
        body: |ad_markers, interval| {
            package_body(
                "cmafPackage",
                json!({
                    "hlsManifests": [
                        child("first", None, None),
                        child("second", Some("PASSTHROUGH"), None),
                        child("third", ad_markers, interval)
                    ]
                }),
            )
        },
    },
];

/// The member a refused manifest configuration under `prefix` is keyed by.
fn interval_member(prefix: &str) -> String {
    format!("{prefix}programDateTimeIntervalSeconds")
}

/// The intervals DATERANGE must refuse: none at all (which the service would store as the
/// zero default) and an explicit zero. A NEGATIVE interval is refused by the shared duration
/// floor instead, whatever the ad markers — `tests/all/origin_endpoint_durations.rs` is that
/// member's suite.
const REFUSED_INTERVALS: &[Option<i32>] = &[None, Some(0)];

/// The intervals DATERANGE must accept: the smallest value above the floor it states, a
/// documented tag interval, and the largest `__integer` there is.
const ACCEPTED_INTERVALS: &[i32] = &[1, 60, i32::MAX];

#[tokio::test]
async fn daterange_ad_markers_without_a_positive_interval_are_refused_on_create() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    for manifest in MANIFESTS {
        for interval in REFUSED_INTERVALS {
            let case = format!(
                "{} DATERANGE interval {interval:?}",
                manifest.prefix.trim_end_matches('.')
            );
            let body = (manifest.body)(Some(state::DATERANGE_AD_MARKERS), *interval);
            refused_on_create(
                &router,
                &case,
                &body,
                &interval_member(manifest.prefix),
                &state::daterange_interval_requirement(),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn daterange_ad_markers_with_a_positive_interval_are_stored_and_read_back() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    for manifest in MANIFESTS {
        for interval in ACCEPTED_INTERVALS {
            let case = format!(
                "{} DATERANGE interval {interval}",
                manifest.prefix.trim_end_matches('.')
            );
            let body = (manifest.body)(Some(state::DATERANGE_AD_MARKERS), Some(*interval));
            let created = accepted_on_create(&router, &case, &body).await;
            let pointer = format!("/{}", interval_member(manifest.prefix))
                .replace("[2]", "/2")
                .replace("[0]", "/0")
                .replace('.', "/");
            assert_eq!(
                created.pointer(&pointer),
                Some(&json!(interval)),
                "case {case:?}: the stored endpoint must carry the interval at {pointer:?}, \
                 got {created:?}"
            );
            assert_eq!(
                created.pointer(&pointer.replace("programDateTimeIntervalSeconds", "adMarkers")),
                Some(&json!(state::DATERANGE_AD_MARKERS)),
                "case {case:?}: the stored endpoint must keep the ad-marker option, \
                 got {created:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_other_ad_marker_option_is_accepted_with_any_interval() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    // The whole catalog of the member's enum, from the model
    // (`types::AD_MARKERS_VALUES`), minus the one option that states a requirement on the
    // interval: none of the rest does, so each is accepted with no interval at all and with
    // the zero the service fills an absent one with.
    let others = types::AD_MARKERS_VALUES
        .iter()
        .filter(|option| **option != state::DATERANGE_AD_MARKERS);
    for option in others {
        for manifest in MANIFESTS {
            for interval in REFUSED_INTERVALS {
                let case = format!(
                    "{} {option} interval {interval:?}",
                    manifest.prefix.trim_end_matches('.')
                );
                let body = (manifest.body)(Some(option), *interval);
                accepted_on_create(&router, &case, &body).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 2. A SPEKE 2.0 encryption contract requires DASH or CMAF packaging with key rotation off
// ---------------------------------------------------------------------------

/// One packaging format's encryption block, as the encryption-contract rule reads it.
struct Format {
    /// The wire name of the packaging block, which a refusal's member path starts with.
    block: &'static str,
    /// Whether this format's endpoints use SPEKE 2.0, and so may name an encryption
    /// contract at all.
    speke_2_0: bool,
    /// Whether the format's `Encryption` shape declares a key-rotation member
    /// (`MssEncryption` declares none, so an MSS endpoint can never ask for a rotation).
    rotates_keys: bool,
    /// Whether the format requires a child manifest list beside its encryption block: a
    /// CMAF package must publish at least `state::MIN_CMAF_HLS_MANIFESTS` of them, so the
    /// case reaches the contract rule instead of that one.
    child_manifests: bool,
}

/// Every packaging format an OriginEndpoint can be created with, each carrying a
/// `SpekeKeyProvider` — and so the encryption-contract member — under its `Encryption`.
/// SPEKE 2.0 is the DASH and CMAF formats' (see this module's docs).
const FORMATS: &[Format] = &[
    Format {
        block: "hlsPackage",
        speke_2_0: false,
        rotates_keys: true,
        child_manifests: false,
    },
    Format {
        block: "dashPackage",
        speke_2_0: true,
        rotates_keys: true,
        child_manifests: false,
    },
    Format {
        block: "cmafPackage",
        speke_2_0: true,
        rotates_keys: true,
        child_manifests: true,
    },
    Format {
        block: "mssPackage",
        speke_2_0: false,
        rotates_keys: false,
        child_manifests: false,
    },
];

/// An encryption contract naming the preset pair at `presets` of the model's own preset
/// enums (`types::PRESET_SPEKE20_AUDIO_VALUES` / `types::PRESET_SPEKE20_VIDEO_VALUES`),
/// both of whose members are required.
fn contract(presets: usize) -> Value {
    json!({
        "presetSpeke20Audio": types::PRESET_SPEKE20_AUDIO_VALUES[presets],
        "presetSpeke20Video": types::PRESET_SPEKE20_VIDEO_VALUES[presets]
    })
}

/// A request body naming `format`'s packaging configuration with an encryption block: the
/// SPEKE provider carries `contract` when the case supplies one, and the block asks for
/// `rotation` when it supplies one.
fn encryption_body(format: &Format, contract: Option<Value>, rotation: Option<i32>) -> Value {
    let mut provider = speke();
    if let Some(contract) = contract {
        provider
            .as_object_mut()
            .expect("the provider is an object")
            .insert("encryptionContractConfiguration".to_string(), contract);
    }
    let encryption = members([
        ("spekeKeyProvider", Some(provider)),
        (
            "keyRotationIntervalSeconds",
            rotation.map(|seconds| json!(seconds)),
        ),
    ]);
    let package = members([
        ("encryption", Some(encryption)),
        (
            "hlsManifests",
            format
                .child_manifests
                .then(|| json!([{ "id": "child" }])),
        ),
    ]);
    package_body(format.block, package)
}

/// The member a refused key rotation is keyed by.
fn rotation_member(block: &str) -> String {
    format!("{block}.encryption.keyRotationIntervalSeconds")
}

/// The member a refused encryption contract is keyed by.
fn contract_member(block: &str) -> String {
    format!("{block}.encryption.spekeKeyProvider.encryptionContractConfiguration")
}

/// Key rotations a contract must refuse: every interval that leaves rotation ENABLED — the
/// smallest one, the quarter-hour the issue reported, a day and the largest `__integer`.
const ENABLED_ROTATIONS: &[i32] = &[1, 900, 86_400, i32::MAX];

/// Key rotations a contract must accept: none asked for at all, and the value that disables
/// rotation.
const DISABLED_ROTATIONS: &[Option<i32>] = &[
    None,
    Some(state::DISABLED_KEY_ROTATION_INTERVAL_SECONDS),
];

#[tokio::test]
async fn an_encryption_contract_beside_enabled_key_rotation_is_refused_on_create() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    for format in FORMATS.iter().filter(|format| format.speke_2_0) {
        for (presets, rotation) in ENABLED_ROTATIONS.iter().enumerate() {
            let case = format!("{} contract rotation {rotation}", format.block);
            let body = encryption_body(format, Some(contract(presets)), Some(*rotation));
            refused_on_create(
                &router,
                &case,
                &body,
                &rotation_member(format.block),
                &state::disabled_key_rotation_requirement(),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn an_encryption_contract_with_key_rotation_disabled_is_stored_and_read_back() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    for format in FORMATS.iter().filter(|format| format.speke_2_0) {
        for (presets, rotation) in DISABLED_ROTATIONS.iter().enumerate() {
            let case = format!("{} contract rotation {rotation:?}", format.block);
            let contract = contract(presets);
            let body = encryption_body(format, Some(contract.clone()), *rotation);
            let created = accepted_on_create(&router, &case, &body).await;
            let pointer = format!(
                "/{}/encryption/spekeKeyProvider/encryptionContractConfiguration",
                format.block
            );
            assert_eq!(
                created.pointer(&pointer),
                Some(&contract),
                "case {case:?}: the stored endpoint must carry the contract at {pointer:?}, \
                 got {created:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_encryption_contract_on_a_format_without_speke_2_0_is_refused_on_create() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    for format in FORMATS.iter().filter(|format| !format.speke_2_0) {
        // Whatever the rotation — including the one a SPEKE 2.0 format would have accepted —
        // the contract itself is the member such a caller has to drop.
        let rotations: Vec<Option<i32>> = if format.rotates_keys {
            DISABLED_ROTATIONS
                .iter()
                .copied()
                .chain(ENABLED_ROTATIONS.iter().map(|seconds| Some(*seconds)))
                .collect()
        } else {
            vec![None]
        };
        for (presets, rotation) in rotations.iter().enumerate() {
            let case = format!("{} contract rotation {rotation:?}", format.block);
            let body = encryption_body(
                format,
                Some(contract(presets % types::PRESET_SPEKE20_AUDIO_VALUES.len())),
                *rotation,
            );
            refused_on_create(
                &router,
                &case,
                &body,
                &contract_member(format.block),
                &state::encryption_contract_packaging_requirement(),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn key_rotation_without_an_encryption_contract_is_stored_on_every_format() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    for format in FORMATS.iter().filter(|format| format.rotates_keys) {
        for rotation in ENABLED_ROTATIONS {
            let case = format!("{} no contract rotation {rotation}", format.block);
            let body = encryption_body(format, None, Some(*rotation));
            let created = accepted_on_create(&router, &case, &body).await;
            let pointer = format!("/{}", rotation_member(format.block)).replace('.', "/");
            assert_eq!(
                created.pointer(&pointer),
                Some(&json!(rotation)),
                "case {case:?}: the stored endpoint must keep the rotation at {pointer:?}, \
                 got {created:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The same two requirements on an UPDATE
// ---------------------------------------------------------------------------

/// One update case: what it sends, and the member and requirement it must be refused with.
struct Update {
    case: &'static str,
    /// The body of the refused `UpdateOriginEndpoint`.
    body: fn() -> Value,
    /// The member path the refusal is keyed by.
    member: fn() -> String,
    /// The requirement that member states, read from the service's own words.
    requirement: fn() -> String,
}

/// One case per requirement, each on a configuration the create of the endpoint accepted, so
/// what the update changes is only the member the rule is about.
const UPDATES: &[Update] = &[
    Update {
        case: "hls daterange without an interval",
        body: || package_body("hlsPackage", manifest_members(Some("DATERANGE"), None)),
        member: || interval_member("hlsPackage."),
        requirement: state::daterange_interval_requirement,
    },
    Update {
        case: "cmaf child daterange with a zero interval",
        body: || {
            package_body(
                "cmafPackage",
                json!({ "hlsManifests": [child("only", Some("DATERANGE"), Some(0))] }),
            )
        },
        member: || interval_member("cmafPackage.hlsManifests[0]."),
        requirement: state::daterange_interval_requirement,
    },
    Update {
        case: "cmaf contract beside a live rotation",
        body: || encryption_body(&FORMATS[2], Some(contract(0)), Some(900)),
        member: || rotation_member("cmafPackage"),
        requirement: state::disabled_key_rotation_requirement,
    },
    Update {
        case: "hls contract on a format without speke 2.0",
        body: || encryption_body(&FORMATS[0], Some(contract(1)), Some(0)),
        member: || contract_member("hlsPackage"),
        requirement: state::encryption_contract_packaging_requirement,
    },
];

#[tokio::test]
async fn a_refused_update_leaves_the_stored_endpoint_alone() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    for update in UPDATES {
        // One endpoint per case, created from a configuration both requirements admit, so a
        // refused update is compared against a record nothing else touched.
        let id = endpoint_id(update.case);
        let body = package_body(
            "cmafPackage",
            json!({ "hlsManifests": [child("only", Some("DATERANGE"), Some(60))] }),
        );
        let (status, created) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(&body, &id)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateOriginEndpoint expected 200, got {status} {created:?}",
            update.case
        );

        let (status, refusal) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{id}"),
            Some(&(update.body)()),
        )
        .await;
        assert_eq!(
            status, 422,
            "case {:?}: expected 422, got {status} {refusal:?}",
            update.case
        );
        assert_eq!(
            error_code(&refusal),
            "UnprocessableEntityException",
            "case {:?}: expected UnprocessableEntityException, got {refusal:?}",
            update.case
        );
        assert_eq!(
            refusal.pointer(&format!("/errors/{}", (update.member)())),
            Some(&json!((update.requirement)())),
            "case {:?}: the refusal must be keyed by {:?} and state the requirement, \
             got {refusal:?}",
            update.case,
            (update.member)()
        );

        let (status, described) =
            call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}",
            update.case
        );
        assert_eq!(
            described, created,
            "case {:?}: a refused update must leave the stored record alone, got {described:?}",
            update.case
        );
    }
}

#[tokio::test]
async fn an_update_satisfying_both_requirements_is_stored() {
    let router = emulator();
    create_channel(&router, CHANNEL_ID).await;

    let id = "update-accepted";
    let (status, created) = call(
        &router,
        "POST",
        "/origin_endpoints",
        Some(&create_body(
            &package_body("hlsPackage", manifest_members(None, None)),
            id,
        )),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint expected 200, got {status} {created:?}"
    );

    // DATERANGE with an interval above the floor, on a CMAF package whose SPEKE 2.0
    // contract comes with rotation disabled: both requirements hold, so the update is
    // carried out and the read renders what it stored.
    let mut body = encryption_body(
        &FORMATS[2],
        Some(contract(0)),
        Some(state::DISABLED_KEY_ROTATION_INTERVAL_SECONDS),
    );
    body["cmafPackage"]["hlsManifests"] =
        json!([child("only", Some(state::DATERANGE_AD_MARKERS), Some(120))]);
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/origin_endpoints/{id}"),
        Some(&body),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateOriginEndpoint expected 200, got {status} {updated:?}"
    );
    assert_eq!(
        updated.pointer("/cmafPackage/hlsManifests/0/programDateTimeIntervalSeconds"),
        Some(&json!(120)),
        "the update must store the interval it sent, got {updated:?}"
    );
    assert_eq!(
        updated.pointer("/cmafPackage/encryption/keyRotationIntervalSeconds"),
        Some(&json!(state::DISABLED_KEY_ROTATION_INTERVAL_SECONDS)),
        "the update must store the disabled rotation it sent, got {updated:?}"
    );
    let (status, described) = call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeOriginEndpoint expected 200, got {status} {described:?}"
    );
    assert_eq!(
        described, updated,
        "the read must render the updated record, got {described:?}"
    );
}
