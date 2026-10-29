// SPDX-License-Identifier: CC0-1.0

//! The initialization vector MediaPackage CREATES for a CMAF encryption configuration.
//!
//! RULE under test: where the model states that the SERVICE supplies a configuration
//! member's value when the request omits it, that value is materialised at write time —
//! by `CreateOriginEndpoint` and by `UpdateOriginEndpoint` alike — so every projection of
//! the stored endpoint (the create or update response, `DescribeOriginEndpoint` and each
//! `ListOriginEndpoints` entry) reports it, in the form the model documents for that
//! member, while a value the request DID send is kept exactly as sent.
//!
//! The one member of this service's configuration shapes whose documentation states that
//! is `CmafEncryption$ConstantInitializationVector`: "an optional 128-bit, 16-byte hex
//! value represented by a 32-character string […] if you don't specify a value, then
//! MediaPackage creates the constant initialization vector (IV)"
//! (`smithy_query(query="doc",shape="com.amazonaws.mediapackage#CmafEncryption",member="ConstantInitializationVector",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`).
//! Its HLS sibling documents the opposite — "when not specified the initialization vector
//! will be periodically rotated" — and `DashEncryption` and `MssEncryption` model no such
//! member at all
//! (`smithy_query(query="shape",shape="CmafEncryption,HlsEncryption,DashEncryption,MssEncryption",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
//! so the suites below assert the created value where the rule holds AND its absence on
//! every block the rule does not cover.
//!
//! The rows differ along what the rule generalises over: whether the encryption block
//! carries a vector of its own and in which form (lower-case hex, upper-case hex), which
//! other encryption members come with it (method, key rotation interval, certificate),
//! the shape of the surrounding CMAF package (child-manifest count, segment members,
//! stream selection), which packaging format carries the encryption block, which
//! operation writes it (create, then each form of update) and which projection reads it
//! back. The uniqueness and stability invariants are driven over generated endpoint ids
//! with the shared seeded generator.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — with the harness `tests/all/channel_lifecycle.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, create_channel, emulator, SeededGenerator, REGION};

/// The channel every endpoint in this suite packages.
const CHANNEL: &str = "iv-channel";

/// The wire path of the member under test, inside a CMAF package.
const IV_POINTER: &str = "/cmafPackage/encryption/constantInitializationVector";

/// A SPEKE key provider, which every packaging format's `Encryption` requires
/// (`SpekeKeyProvider$ResourceId`, `$RoleArn`, `$SystemIds` and `$Url` are all required),
/// so an encryption row reaches the handler instead of being refused for a missing member.
fn speke() -> Value {
    json!({
        "resourceId": "endpoint-iv",
        "roleArn": "arn:aws:iam::000000000000:role/mediapackage-speke",
        "systemIds": ["81376844-f976-481e-a84e-cc25d39b0b33"],
        "url": "https://speke.example.com/v1/copyProtection"
    })
}

/// Whether `value` is written in the form the model documents for a constant
/// initialization vector: 16 bytes as [`state::INITIALIZATION_VECTOR_CHARS`] hex
/// characters.
fn is_documented_form(value: &str) -> bool {
    value.chars().count() == state::INITIALIZATION_VECTOR_CHARS
        && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// The `constantInitializationVector` of `body`'s CMAF encryption block, or `None` when
/// the projection carries none.
fn reported_vector(body: &Value) -> Option<&str> {
    body.pointer(IV_POINTER).and_then(Value::as_str)
}

/// Create `id` on [`CHANNEL`] with `extras` merged in, and answer the create response.
async fn create_endpoint(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    extras: &Value,
) -> Value {
    let mut request = json!({ "channelId": CHANNEL, "id": id });
    for (member, value) in extras.as_object().expect("an object of request members") {
        request[member] = value.clone();
    }
    let (status, created) = call(router, "POST", "/origin_endpoints", Some(&request)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateOriginEndpoint expected 200, got {status} {created}"
    );
    created
}

/// Read `id` back through `DescribeOriginEndpoint`.
async fn describe_endpoint(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = call(router, "GET", &format!("/origin_endpoints/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described}"
    );
    described
}

/// The `ListOriginEndpoints` entry for `id`.
async fn listed_endpoint(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, listed) = call(
        router,
        "GET",
        &format!("/origin_endpoints?channelId={CHANNEL}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListOriginEndpoints expected 200, got {status} {listed}"
    );
    listed
        .get("originEndpoints")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// What a row expects the stored CMAF encryption block to report for the member under
/// test.
#[derive(Clone, Copy)]
enum Expected {
    /// The service created the vector: the request sent none.
    Created,
    /// The request sent this vector, so it is stored and reported verbatim.
    AsSent(&'static str),
    /// The projection carries no vector at all: no CMAF encryption block to carry one.
    Absent,
}

/// The create bodies under test, one row per point of the class.
fn cases() -> Vec<(&'static str, Value, Expected)> {
    vec![
        // The reported case: an encryption block with nothing but its required key
        // provider.
        (
            "cmaf encryption, no vector",
            json!({
                "cmafPackage": {
                    "hlsManifests": [{ "id": "only" }],
                    "encryption": { "spekeKeyProvider": speke() }
                }
            }),
            Expected::Created,
        ),
        // The same omission beside each of the block's OTHER optional members, and both
        // values of the modelled encryption method
        // (`smithy_query(query="shape",shape="CmafEncryptionMethod",…)`: SAMPLE_AES,
        // AES_CTR) — neither of which the omitted member depends on.
        (
            "cmaf encryption, no vector, SAMPLE_AES",
            json!({
                "cmafPackage": {
                    "hlsManifests": [{ "id": "only" }],
                    "encryption": {
                        "encryptionMethod": "SAMPLE_AES",
                        "spekeKeyProvider": speke()
                    }
                }
            }),
            Expected::Created,
        ),
        (
            "cmaf encryption, no vector, AES_CTR with key rotation",
            json!({
                "cmafPackage": {
                    "hlsManifests": [{ "id": "only" }],
                    "encryption": {
                        "encryptionMethod": "AES_CTR",
                        "keyRotationIntervalSeconds": 300,
                        "spekeKeyProvider": speke()
                    }
                }
            }),
            Expected::Created,
        ),
        // A richer surrounding package: three child manifests, the segment members and a
        // stream selection. The created vector does not depend on any of them.
        (
            "cmaf encryption, no vector, three children and a stream selection",
            json!({
                "cmafPackage": {
                    "hlsManifests": [
                        { "id": "a" },
                        { "id": "b", "manifestName": "high" },
                        { "id": "c", "playlistType": "VOD" }
                    ],
                    "segmentDurationSeconds": 4,
                    "segmentPrefix": "prefixed",
                    "streamSelection": { "streamOrder": "VIDEO_BITRATE_ASCENDING" },
                    "encryption": { "spekeKeyProvider": speke() }
                }
            }),
            Expected::Created,
        ),
        // A SUPPLIED vector is never overwritten, whichever case its hex digits are
        // written in.
        (
            "cmaf encryption, lower-case vector sent",
            json!({
                "cmafPackage": {
                    "hlsManifests": [{ "id": "only" }],
                    "encryption": {
                        "constantInitializationVector": "0123456789abcdef0123456789abcdef",
                        "spekeKeyProvider": speke()
                    }
                }
            }),
            Expected::AsSent("0123456789abcdef0123456789abcdef"),
        ),
        (
            "cmaf encryption, upper-case vector sent",
            json!({
                "cmafPackage": {
                    "hlsManifests": [{ "id": "only" }],
                    "encryption": {
                        "constantInitializationVector": "A382A901F3C1F7718512266CFFBB0B5E",
                        "encryptionMethod": "AES_CTR",
                        "spekeKeyProvider": speke()
                    }
                }
            }),
            Expected::AsSent("A382A901F3C1F7718512266CFFBB0B5E"),
        ),
        // No encryption block: there is no configuration for a created vector to belong
        // to, so none is invented.
        (
            "cmaf package, no encryption",
            json!({ "cmafPackage": { "hlsManifests": [{ "id": "only" }] } }),
            Expected::Absent,
        ),
        (
            "cmaf package, no encryption, segment members only",
            json!({
                "cmafPackage": {
                    "hlsManifests": [{ "id": "only" }],
                    "segmentDurationSeconds": 2,
                    "segmentPrefix": "clear"
                }
            }),
            Expected::Absent,
        ),
    ]
}

/// Assert `projection` reports what `expected` requires for the member under test.
fn assert_vector(case: &str, projection: &str, expected: Expected, body: &Value) {
    let actual = reported_vector(body);
    match expected {
        Expected::Created => {
            let actual = actual.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {projection} must carry a created \
                     {IV_POINTER}, got none in {body}"
                )
            });
            assert!(
                is_documented_form(actual),
                "case {case:?}: {projection} must report a created vector of {} hex \
                 characters, got {actual:?}",
                state::INITIALIZATION_VECTOR_CHARS
            );
        }
        Expected::AsSent(sent) => assert_eq!(
            actual,
            Some(sent),
            "case {case:?}: {projection} must keep the vector the request sent ({sent:?}), \
             got {actual:?} in {body}"
        ),
        Expected::Absent => assert_eq!(
            actual, None,
            "case {case:?}: {projection} must carry no {IV_POINTER}, got {actual:?} in {body}"
        ),
    }
}

#[tokio::test]
async fn every_projection_reports_the_created_cmaf_initialization_vector() {
    let router = emulator();
    create_channel(&router, CHANNEL).await;

    for (index, (case, extras, expected)) in cases().into_iter().enumerate() {
        let id = format!("iv-endpoint-{index}");
        let created = create_endpoint(&router, case, &id, &extras).await;
        assert_vector(case, "the create response", expected, &created);

        let described = describe_endpoint(&router, case, &id).await;
        assert_vector(case, "DescribeOriginEndpoint", expected, &described);
        assert_eq!(
            described, created,
            "case {case:?}: DescribeOriginEndpoint must render the same stored record as \
             the create response"
        );

        let entry = listed_endpoint(&router, case, &id).await;
        assert_vector(case, "the ListOriginEndpoints entry", expected, &entry);
        assert_eq!(
            entry, created,
            "case {case:?}: the listed endpoint must be the same stored record"
        );

        // A created value is not re-created on a READ: the second describe reports the
        // value the first one did.
        let again = describe_endpoint(&router, case, &id).await;
        assert_eq!(
            reported_vector(&again),
            reported_vector(&created),
            "case {case:?}: a read must not change the stored vector, got {:?} then {:?}",
            reported_vector(&created),
            reported_vector(&again)
        );
    }
}

#[tokio::test]
async fn no_other_encryption_block_gains_an_initialization_vector() {
    let router = emulator();
    create_channel(&router, CHANNEL).await;

    // The model documents a created vector for the CMAF block ALONE: the HLS sibling
    // rotates its vector instead of creating a constant one, and the DASH and MSS
    // encryption shapes model no such member. So an encryption block of those formats,
    // sent without one, is stored exactly as sent.
    let rows: Vec<(&str, Value)> = vec![
        (
            "hls encryption",
            json!({ "hlsPackage": { "encryption": { "spekeKeyProvider": speke() } } }),
        ),
        (
            "hls encryption with a rotation interval",
            json!({
                "hlsPackage": {
                    "encryption": {
                        "keyRotationIntervalSeconds": 60,
                        "repeatExtXKey": true,
                        "spekeKeyProvider": speke()
                    }
                }
            }),
        ),
        (
            "dash encryption",
            json!({ "dashPackage": { "encryption": { "spekeKeyProvider": speke() } } }),
        ),
        (
            "mss encryption",
            json!({ "mssPackage": { "encryption": { "spekeKeyProvider": speke() } } }),
        ),
    ];
    for (index, (case, extras)) in rows.into_iter().enumerate() {
        let id = format!("iv-other-{index}");
        let created = create_endpoint(&router, case, &id, &extras).await;
        let described = describe_endpoint(&router, case, &id).await;
        for (projection, body) in [("the create response", &created), ("the read", &described)] {
            for package in ["hlsPackage", "dashPackage", "cmafPackage", "mssPackage"] {
                let pointer = format!("/{package}/encryption/constantInitializationVector");
                let actual = body.pointer(&pointer);
                assert_eq!(
                    actual, None,
                    "case {case:?}: {projection} must carry no {pointer}, got {actual:?} \
                     in {body}"
                );
            }
        }
    }
}

/// A CMAF package carrying `encryption` (or none at all) and one child manifest, for the
/// update rows below.
fn cmaf_package(encryption: Option<Value>) -> Value {
    let mut package = json!({ "hlsManifests": [{ "id": "only" }] });
    if let Some(encryption) = encryption {
        package["encryption"] = encryption;
    }
    package
}

#[tokio::test]
async fn an_update_materialises_the_vector_exactly_as_a_create_does() {
    let router = emulator();
    create_channel(&router, CHANNEL).await;

    let id = "iv-updated";
    let created = create_endpoint(
        &router,
        "create without a vector",
        id,
        &json!({ "cmafPackage": cmaf_package(Some(json!({ "spekeKeyProvider": speke() }))) }),
    )
    .await;
    let created_vector = reported_vector(&created)
        .expect("the create response carries the created vector")
        .to_string();

    // Each row is an UPDATE body and what the stored endpoint must then report. The rows
    // span the forms an update takes: resending the package without a vector, sending one
    // of its own, dropping the encryption block, and naming no packaging configuration at
    // all (which leaves the stored one in place).
    let sent = "FFEEDDCCBBAA99887766554433221100";
    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            "update resends the block without a vector",
            json!({ "cmafPackage": cmaf_package(Some(json!({ "spekeKeyProvider": speke() }))) }),
            Expected::Created,
        ),
        (
            "update sends a vector of its own",
            json!({
                "cmafPackage": cmaf_package(Some(json!({
                    "constantInitializationVector": sent,
                    "spekeKeyProvider": speke()
                })))
            }),
            Expected::AsSent(sent),
        ),
        (
            "update resends the block without a vector again",
            json!({
                "cmafPackage": cmaf_package(Some(json!({
                    "encryptionMethod": "SAMPLE_AES",
                    "spekeKeyProvider": speke()
                })))
            }),
            Expected::Created,
        ),
        (
            "update names no packaging configuration",
            json!({ "description": "left as stored" }),
            Expected::Created,
        ),
        (
            "update drops the encryption block",
            json!({ "cmafPackage": cmaf_package(None) }),
            Expected::Absent,
        ),
    ];
    for (case, body, expected) in rows {
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/origin_endpoints/{id}"),
            Some(&body),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateOriginEndpoint expected 200, got {status} {updated}"
        );
        assert_vector(case, "the update response", expected, &updated);
        let described = describe_endpoint(&router, case, id).await;
        assert_vector(case, "DescribeOriginEndpoint", expected, &described);

        // The vector the SERVICE creates belongs to the endpoint, so every write that
        // leaves the caller's encryption block without one reports the same value the
        // create did.
        if matches!(expected, Expected::Created) {
            let actual = reported_vector(&described);
            assert_eq!(
                actual,
                Some(created_vector.as_str()),
                "case {case:?}: the created vector must stay {created_vector:?} across \
                 writes, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn each_endpoints_created_vector_is_its_own_and_well_formed() {
    // The invariants of a created value, over generated inputs: every endpoint that
    // leaves the member unset gets a vector in the documented form, two endpoints never
    // share one, and the value is the one the service derives for that endpoint
    // ([`state::created_initialization_vector`]) rather than anything taken from the
    // request. A failing case reproduces from the seed printed below.
    const SEED: u64 = 0x1F_2E_3D_4C;
    const CASES: usize = 12;

    let router = emulator();
    create_channel(&router, CHANNEL).await;

    let mut generator = SeededGenerator::new(SEED);
    let mut seen: Vec<(String, String)> = Vec::new();
    for index in 0..CASES {
        // The generated part of the case: the endpoint's id, how many child manifests its
        // package publishes, and whether it carries a key rotation interval.
        let id = format!("iv-prop-{index}-{:016x}", generator.next_u64());
        let children: Vec<Value> = (0..generator.next_in(3))
            .map(|child| json!({ "id": format!("child-{child}") }))
            .collect();
        let mut encryption = json!({ "spekeKeyProvider": speke() });
        if generator.next_u64().is_multiple_of(2) {
            encryption["keyRotationIntervalSeconds"] = json!(generator.next_in(600));
        }
        let case = format!("seed {SEED}, case {index}, id {id}");
        let created = create_endpoint(
            &router,
            &case,
            &id,
            &json!({
                "cmafPackage": {
                    "hlsManifests": children,
                    "encryption": encryption
                }
            }),
        )
        .await;

        let actual = reported_vector(&created)
            .unwrap_or_else(|| panic!("case {case:?}: expected a created vector, got none"));
        assert!(
            is_documented_form(actual),
            "case {case:?}: expected {} hex characters, got {actual:?}",
            state::INITIALIZATION_VECTOR_CHARS
        );
        let derived = state::created_initialization_vector(REGION, CHANNEL, &id);
        assert_eq!(
            actual, derived,
            "case {case:?}: expected the vector the service creates for this endpoint \
             ({derived:?}), got {actual:?}"
        );
        if let Some((other, _)) = seen.iter().find(|(_, vector)| vector == actual) {
            panic!(
                "case {case:?}: expected a vector of its own, got {actual:?}, which \
                 endpoint {other:?} already reports"
            );
        }
        seen.push((id, actual.to_string()));
    }
    assert_eq!(
        seen.len(),
        CASES,
        "seed {SEED}: every generated endpoint must have reported a created vector, got {}",
        seen.len()
    );
}
