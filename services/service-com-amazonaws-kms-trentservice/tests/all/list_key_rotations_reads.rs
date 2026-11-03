// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the READER of a KMS key's key materials, `ListKeyRotations`,
//! driven over the wire through the shared [`super::harness`].
//!
//! The sibling [`super::key_rotation_on_demand`] suite drives the WRITER of those
//! records; this one drives the reader, so the rules under test are the reader's own:
//!
//! 1. The listing belongs to the key the `KeyId` member RESOLVES to, whatever form that
//!    identifier takes (key id, key ARN, `alias/<name>`, alias ARN), and holds one entry
//!    per completed rotation of that key's material, oldest first, naming the material
//!    each rotation made current — the newest of them being the one the key encrypts
//!    under now.
//! 2. A KMS key is a regional resource, so the reader serves only the keys of the
//!    endpoint's Region and account: an identifier that names no key there is
//!    `NotFoundException`, and an ARN that is no KMS key or alias ARN at all is
//!    `InvalidArnException`.
//! 3. The listing is paged by `Limit` and by a `Marker` the reader minted for that key:
//!    no page is longer than the limit, `Truncated` says whether entries remain,
//!    `NextMarker` is present only then, and the pages together hold every entry exactly
//!    once in the unpaged order. A token that names no key material of this key is
//!    `InvalidMarkerException`.
//! 4. `IncludeKeyMaterial` chooses which key materials are listed: the default lists the
//!    rotations alone, and `ALL_KEY_MATERIAL` prepends the material the key was created
//!    with, which is not a rotation and so carries neither a rotation date nor a rotation
//!    type. It can be asked only of a key whose material rotates.
//! 5. The listing is a REPORT derived from those records: writing the key for any other
//!    reason leaves it as it was, and reading it changes nothing a later caller sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    alias_arn, b64, create_alias, create_alias_in, create_key, describe, failure, failure_in,
    is_lower_hex, ok, ok_in, rotate_on_demand, rotate_on_demand_in, router, text, REGION,
};

/// How many on-demand rotations one key allows, from
/// `service_com_amazonaws_kms_trentservice::state`, so the rows that fill a key's whole
/// rotation history move with the service rather than restating its quota.
use service_com_amazonaws_kms_trentservice::state::MAX_ON_DEMAND_ROTATIONS;

/// The largest `Limit` this listing accepts: `LimitType`'s modelled `@range(1..1000)`
/// (`smithy_query(query="shape",shape="LimitType")`).
const MAX_LIMIT: usize = 1000;

/// The page a request that names no `Limit` is paced at: "if you do not include a value,
/// it defaults to 100"
/// (`smithy_query(query="doc",shape="ListKeyRotationsRequest",member="Limit")`).
const DEFAULT_LIMIT: usize = 100;

/// The length of a `KeyMaterialId`, which is `@length(64..64)` of `^[a-f0-9]+$`
/// (`smithy_query(query="shape",shape="RotationsListEntry")`).
const KEY_MATERIAL_ID_LENGTH: usize = 64;

/// The `IncludeKeyMaterial` value that adds the material the key was created with, and
/// the one that lists the rotations alone — the two values of that enum
/// (`smithy_query(query="shape",shape="IncludeKeyMaterial")`).
const ALL_KEY_MATERIAL: &str = "ALL_KEY_MATERIAL";
/// See [`ALL_KEY_MATERIAL`].
const ROTATIONS_ONLY: &str = "ROTATIONS_ONLY";

/// The members of a `RotationsListEntry` that describe IMPORTED key material, which
/// "[are] only present for symmetric encryption KMS keys with EXTERNAL origin"
/// (`smithy_query(query="shape",shape="RotationsListEntry")`), so a key whose material
/// KMS generates carries none of them.
const IMPORTED_MATERIAL_MEMBERS: [&str; 4] = [
    "KeyMaterialDescription",
    "ImportState",
    "ExpirationModel",
    "ValidTo",
];

/// A second Region, for the rows about which endpoint serves which key.
const OTHER_REGION: &str = "us-west-2";

/// The longest listing a key can have — one entry per on-demand rotation plus the
/// material the key was created with — fits the page a request that names no `Limit` is
/// paced at, which is what lets a test read the whole history in one unpaged answer.
const _: () = assert!(MAX_ON_DEMAND_ROTATIONS < DEFAULT_LIMIT);

/// The `Rotations` list of one answer.
fn entries(case: &str, answer: &Value) -> Vec<Value> {
    answer["Rotations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Rotations list in {answer}"))
        .clone()
}

/// List one key's key materials, requiring success.
async fn list(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "ListKeyRotations", request).await
}

/// Rotate the key `identifier` names `count` times, reporting the `KeyMaterialId` each
/// rotation made current, oldest first.
async fn rotate(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    count: usize,
) -> Vec<String> {
    let mut materials = Vec::new();
    for round in 1..=count {
        let case = format!("{case} rotation {round}");
        rotate_on_demand(router, &case, identifier).await;
        let described = describe(router, identifier).await;
        materials.push(text(&case, &described, "CurrentKeyMaterialId"));
    }
    materials
}

/// Every identifier form that resolves to a key answers that key's rotations, oldest
/// first, one entry per rotation.
///
/// The rows differ in the kind of key the rotations happened on (the default symmetric
/// encryption key, the same spec and usage stated explicitly, a multi-Region primary
/// key), in how many rotations it has had (none, one, several), and in the identifier
/// form the read names it by.
#[tokio::test]
async fn every_identifier_form_lists_the_rotations_of_the_resolved_key() {
    let router = router();

    let kinds: [(&str, Value); 3] = [
        ("the default symmetric encryption key", json!({})),
        (
            "an explicit SYMMETRIC_DEFAULT ENCRYPT_DECRYPT key",
            json!({"KeySpec": "SYMMETRIC_DEFAULT", "KeyUsage": "ENCRYPT_DECRYPT"}),
        ),
        ("a multi-Region primary key", json!({"MultiRegion": true})),
    ];

    for (index, (described, request)) in kinds.into_iter().enumerate() {
        for wanted in [0usize, 1, 3] {
            let case = format!("{described} with {wanted} rotation(s)");
            let metadata = create_key(&router, &request).await;
            let key_id = text(&case, &metadata, "KeyId");
            let key_arn = text(&case, &metadata, "Arn");
            let created_with = text(&case, &metadata, "CurrentKeyMaterialId");
            let alias = format!("alias/rotation-listing-{index}-{wanted}");
            create_alias(&router, &case, &alias, &key_id).await;
            let expected = rotate(&router, &case, &key_id, wanted).await;

            let forms = [
                ("key id", key_id.clone()),
                ("key ARN", key_arn.clone()),
                ("alias name", alias.clone()),
                ("alias ARN", alias_arn(REGION, &alias)),
            ];
            let mut answers: Vec<(String, Value)> = Vec::new();
            for (form, identifier) in &forms {
                let case = format!("{case} read by {form}");
                let answer = list(&router, &json!({"KeyId": identifier})).await;
                let listed = entries(&case, &answer);
                assert_eq!(
                    listed.len(),
                    wanted,
                    "case {case:?}: expected one entry per rotation, got {answer}"
                );
                assert_eq!(
                    answer["Truncated"],
                    json!(false),
                    "case {case:?}: a listing that fits one page is not truncated, got {answer}"
                );
                assert!(
                    answer["NextMarker"].is_null(),
                    "case {case:?}: a complete listing names nowhere to resume, got {answer}"
                );

                let mut previous_date = 0.0f64;
                for (position, entry) in listed.iter().enumerate() {
                    let case = format!("{case} entry {position}");
                    // "Unique identifier of the key": the entry names the key the
                    // material belongs to by its key ARN — the fully qualified
                    // identifier — whichever identifier form asked for it.
                    assert_eq!(
                        entry["KeyId"],
                        json!(key_arn),
                        "case {case:?}: expected the entry to name the key {key_arn}, got {entry}"
                    );
                    let material = text(&case, entry, "KeyMaterialId");
                    assert_eq!(
                        material, expected[position],
                        "case {case:?}: expected the material rotation {} made current, got \
                         {material:?}",
                        position + 1
                    );
                    assert!(
                        material.len() == KEY_MATERIAL_ID_LENGTH && is_lower_hex(&material),
                        "case {case:?}: a KeyMaterialId is {KEY_MATERIAL_ID_LENGTH} lowercase \
                         hex characters, got {material:?}"
                    );
                    assert_ne!(
                        material, created_with,
                        "case {case:?}: a rotation reports the material it made current, not the \
                         material the key was created with"
                    );
                    assert_eq!(
                        entry["RotationType"],
                        json!("ON_DEMAND"),
                        "case {case:?}: expected the kind of rotation that happened, got {entry}"
                    );
                    let date = entry["RotationDate"]
                        .as_f64()
                        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationDate in {entry}"));
                    assert!(
                        date >= previous_date,
                        "case {case:?}: the rotations are listed oldest first, got {date} after \
                         {previous_date}"
                    );
                    previous_date = date;
                    // The material the key encrypts under now is the one the newest
                    // rotation made current; every version it replaced decrypts only.
                    let expected_state = if position + 1 == listed.len() {
                        "CURRENT"
                    } else {
                        "NON_CURRENT"
                    };
                    assert_eq!(
                        entry["KeyMaterialState"],
                        json!(expected_state),
                        "case {case:?}: expected KeyMaterialState {expected_state}, got {entry}"
                    );
                    for member in IMPORTED_MATERIAL_MEMBERS {
                        assert!(
                            entry[member].is_null(),
                            "case {case:?}: {member} describes imported key material, which this \
                             key has none of, got {entry}"
                        );
                    }
                }

                if let Some(newest) = listed.last() {
                    let current = text(
                        &case,
                        &describe(&router, &key_id).await,
                        "CurrentKeyMaterialId",
                    );
                    assert_eq!(
                        newest["KeyMaterialId"],
                        json!(current),
                        "case {case:?}: the newest rotation's material is the key's current one, \
                         got {newest}"
                    );
                }
                answers.push((case, answer));
            }
            for (other_case, answer) in &answers[1..] {
                assert_eq!(
                    answer, &answers[0].1,
                    "case {other_case:?}: every identifier form must answer the same listing, got \
                     {answer} and {}",
                    answers[0].1
                );
            }
        }
    }
}

/// The reader serves the keys of the endpoint it was called at: each Region's key lists
/// its own rotations, and an identifier that resolves to no key of this account and
/// Region is refused rather than answered with an empty listing.
#[tokio::test]
async fn the_reader_serves_only_the_keys_of_its_own_region_and_account() {
    let router = router();
    let account = engine::emulator_account_id();

    // Rows: a Region, and how many rotations its key has had. The same alias name in
    // two Regions names two keys.
    let regions: [(&str, usize); 2] = [(REGION, 2), (OTHER_REGION, 1)];
    let mut keys: Vec<(&str, String, usize)> = Vec::new();
    for (region, rotations) in regions {
        let case = format!("the key of {region}");
        let metadata = ok_in(
            &router,
            region,
            "CreateKey",
            &json!({"Description": case.clone()}),
        )
        .await["KeyMetadata"]
            .clone();
        let key_id = text(&case, &metadata, "KeyId");
        create_alias_in(&router, region, &case, "alias/regional-rotations", &key_id).await;
        for _ in 0..rotations {
            rotate_on_demand_in(&router, region, &case, &key_id).await;
        }
        keys.push((region, key_id, rotations));
    }

    for (region, key_id, rotations) in &keys {
        for identifier in [key_id.as_str(), "alias/regional-rotations"] {
            let case = format!("{region} serves its own key, read by {identifier}");
            let answer = ok_in(
                &router,
                region,
                "ListKeyRotations",
                &json!({"KeyId": identifier}),
            )
            .await;
            assert_eq!(
                entries(&case, &answer).len(),
                *rotations,
                "case {case:?}: expected {rotations} rotation(s), got {answer}"
            );
        }

        for (other_region, _, _) in &keys {
            if other_region == region {
                continue;
            }
            let case = format!("the key of {region} read at {other_region}");
            let (status, code) = failure_in(
                &router,
                other_region,
                "ListKeyRotations",
                &json!({"KeyId": key_id}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?}: expected NotFoundException, got {code}"
            );
        }
    }

    let local_key_id = keys[0].1.clone();

    // Rows: a well-formed identifier that names no key of this account and Region.
    let not_found: [(&str, String); 6] = [
        (
            "an unknown key id",
            "22222222-2222-4222-2222-222222222222".to_string(),
        ),
        (
            "an unknown key ARN",
            format!("arn:aws:kms:{REGION}:{account}:key/mrk-{}", "2".repeat(32)),
        ),
        ("an unknown alias", "alias/no-such-rotations".to_string()),
        (
            "an unknown alias ARN",
            alias_arn(REGION, "alias/no-such-rotations"),
        ),
        (
            "this key's ARN in another Region",
            format!("arn:aws:kms:{OTHER_REGION}:{account}:key/{local_key_id}"),
        ),
        (
            "this key's ARN in another account",
            format!("arn:aws:kms:{REGION}:999999999999:key/{local_key_id}"),
        ),
    ];
    for (case, identifier) in not_found {
        let (status, code) =
            failure(&router, "ListKeyRotations", &json!({"KeyId": identifier})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException, got {code}"
        );
    }

    // Rows: an ARN that is no KMS key or alias ARN at all, which this operation models
    // its own error for.
    let malformed: [(&str, String); 5] = [
        (
            "an unknown resource type",
            format!("arn:aws:kms:{REGION}:{account}:thing/{local_key_id}"),
        ),
        ("another service's ARN", "arn:aws:s3:::my-bucket".to_string()),
        (
            "an empty key id",
            format!("arn:aws:kms:{REGION}:{account}:key/"),
        ),
        (
            "no Region segment",
            format!("arn:aws:kms::{account}:key/{local_key_id}"),
        ),
        ("nothing but the prefix", "arn:".to_string()),
    ];
    for (case, identifier) in malformed {
        let (status, code) =
            failure(&router, "ListKeyRotations", &json!({"KeyId": identifier})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "InvalidArnException",
            "case {case:?}: expected InvalidArnException, got {code}"
        );
    }
}

/// The reader pages a key's whole rotation history: no page is longer than `Limit`, a
/// page is full while entries remain, `Truncated` says whether they do, `NextMarker`
/// appears only then, and the pages together hold every entry exactly once in the
/// unpaged order.
///
/// The rows differ in how many rotations the key has had (none, one, several, the whole
/// per-key on-demand quota), in whether the material the key was created with is included
/// as well, and in the page size asked for — one entry at a time, a size that divides the
/// history and one that does not, no `Limit` at all, and the published bounds of
/// `LimitType`.
#[tokio::test]
async fn the_reader_pages_the_whole_rotation_history() {
    let router = router();

    let histories: [usize; 4] = [0, 1, 7, MAX_ON_DEMAND_ROTATIONS];
    let limits: [Option<usize>; 6] = [Some(1), Some(2), Some(5), Some(MAX_LIMIT), None, Some(7)];

    // A key holds at most one entry per on-demand rotation plus the material it was
    // created with, which is fewer than the page a request naming no `Limit` is paced at
    // (the module's constant assertion), so the answer to a request that names none is
    // the whole history.
    for history in histories {
        let case = format!("{history} rotation(s)");
        let metadata = create_key(&router, &json!({"Description": case.clone()})).await;
        let key_id = text(&case, &metadata, "KeyId");
        rotate(&router, &case, &key_id, history).await;

        for include in [None, Some(ALL_KEY_MATERIAL)] {
            // The whole listing, as the reader answers it unpaged.
            let mut request = json!({"KeyId": key_id});
            if let Some(include) = include {
                request["IncludeKeyMaterial"] = json!(include);
            }
            let whole = list(&router, &request).await;
            let expected = entries(&case, &whole);

            for limit in limits {
                let case = format!("{case}, include {include:?}, limit {limit:?}");
                let page_size = limit.unwrap_or(DEFAULT_LIMIT);
                let mut walked: Vec<Value> = Vec::new();
                let mut marker: Option<String> = None;
                let mut pages = 0usize;
                loop {
                    let mut request = json!({"KeyId": key_id});
                    if let Some(include) = include {
                        request["IncludeKeyMaterial"] = json!(include);
                    }
                    if let Some(limit) = limit {
                        request["Limit"] = json!(limit);
                    }
                    if let Some(marker) = marker.as_deref() {
                        request["Marker"] = json!(marker);
                    }
                    let answer = list(&router, &request).await;
                    pages += 1;
                    assert!(
                        pages <= expected.len() + 1,
                        "case {case:?}: the walk must terminate, got {pages} page(s)"
                    );

                    let page = entries(&case, &answer);
                    let remaining = expected.len() - walked.len();
                    // The model promises "not more than the specified number of items,
                    // but it might return fewer"; this emulator pages one key's stored
                    // key materials deterministically, so "fewer" happens only on the
                    // last page and a short page anywhere else is a defect.
                    assert_eq!(
                        page.len(),
                        remaining.min(page_size),
                        "case {case:?}: expected a full page while entries remain, got {answer}"
                    );
                    walked.extend(page);

                    let truncated = answer["Truncated"].as_bool().unwrap_or_else(|| {
                        panic!("case {case:?}: expected a Truncated flag in {answer}")
                    });
                    assert_eq!(
                        truncated,
                        walked.len() < expected.len(),
                        "case {case:?}: Truncated must say whether entries remain after {} of {}, \
                         got {answer}",
                        walked.len(),
                        expected.len()
                    );
                    if !truncated {
                        assert!(
                            answer["NextMarker"].is_null(),
                            "case {case:?}: the last page names nowhere to resume, got {answer}"
                        );
                        break;
                    }
                    marker = Some(text(&case, &answer, "NextMarker"));
                }

                assert_eq!(
                    walked, expected,
                    "case {case:?}: the pages together must hold the unpaged listing, in order"
                );
            }
        }

        // One past either published bound of `LimitType` is no page size at all.
        for limit in [0i64, MAX_LIMIT as i64 + 1] {
            let case = format!("{case}, limit {limit} past the published bound");
            let (status, code) = failure(
                &router,
                "ListKeyRotations",
                &json!({"KeyId": key_id, "Limit": limit}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: expected ValidationException, got {code}"
            );
        }
    }
}

/// A `Marker` names the key material the previous page ended at, so a token this key's
/// listing holds no such material for — one minted for another key, or one this reader
/// never minted at all — resumes nowhere and is refused rather than silently resuming at
/// whichever entry now sits in that position.
#[tokio::test]
async fn a_marker_naming_no_key_material_of_this_key_is_refused() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "markers"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let materials = rotate(&router, "setup", &key_id, 4).await;

    // A token taken from a page of a DIFFERENT key, whose key material versions this
    // key holds none of.
    let other = create_key(&router, &json!({"Description": "another key"})).await;
    let other_id = text("setup", &other, "KeyId");
    rotate(&router, "setup", &other_id, 2).await;
    let other_page = list(&router, &json!({"KeyId": other_id, "Limit": 1})).await;
    let other_marker = text("setup", &other_page, "NextMarker");

    // A token this reader minted for THIS key, but for the listing that includes the
    // material the key was created with: that material is not a rotation, so the default
    // listing holds no entry the token names.
    let all_page = list(
        &router,
        &json!({"KeyId": key_id, "IncludeKeyMaterial": ALL_KEY_MATERIAL, "Limit": 1}),
    )
    .await;
    let first_material_marker = text("setup", &all_page, "NextMarker");

    for (case, marker) in [
        ("a token from another key's page", other_marker),
        ("a token that is not hex at all", "not-a-marker".to_string()),
        (
            "hex that carries no list prefix",
            hex::encode(&materials[0]),
        ),
        (
            "a token for the material the key was created with, which this listing omits",
            first_material_marker,
        ),
    ] {
        let (status, code) = failure(
            &router,
            "ListKeyRotations",
            &json!({"KeyId": key_id, "Marker": marker}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "InvalidMarkerException",
            "case {case:?}: expected InvalidMarkerException, got {code}"
        );
    }

    // The token minted for THIS key is accepted, and resumes after the entry its page
    // ended on.
    let first = list(&router, &json!({"KeyId": key_id, "Limit": 2})).await;
    let marker = text("resume", &first, "NextMarker");
    let second = list(&router, &json!({"KeyId": key_id, "Marker": marker})).await;
    let walked: Vec<String> = entries("resume", &first)
        .iter()
        .chain(entries("resume", &second).iter())
        .map(|entry| text("resume", entry, "KeyMaterialId"))
        .collect();
    assert_eq!(
        walked, materials,
        "case resume: the two pages together must hold every rotation in order, got {first} \
         then {second}"
    );
}

/// `ALL_KEY_MATERIAL` adds the material the key was created with as the first entry,
/// which is not a rotation and so carries neither a rotation date nor a rotation type,
/// while the default and the explicit `ROTATIONS_ONLY` list the rotations alone. A key
/// whose material does not rotate has no rotation to list, and asking for its key
/// materials is the one parameter that is not supported for it.
///
/// The rows differ in how many rotations the key has had, and — for the refusal — in the
/// kinds of key `smithy_query(query="shape",shape="KeySpec")` names outside symmetric
/// encryption, whose material neither automatic nor on-demand rotation replaces.
#[tokio::test]
async fn including_all_key_material_adds_the_material_the_key_was_created_with() {
    let router = router();

    for history in [0usize, 1, 3] {
        let case = format!("{history} rotation(s)");
        let metadata = create_key(&router, &json!({"Description": case.clone()})).await;
        let key_id = text(&case, &metadata, "KeyId");
        let key_arn = text(&case, &metadata, "Arn");
        let created_with = text(&case, &metadata, "CurrentKeyMaterialId");
        let rotated = rotate(&router, &case, &key_id, history).await;

        let default = list(&router, &json!({"KeyId": key_id})).await;
        let explicit = list(
            &router,
            &json!({"KeyId": key_id, "IncludeKeyMaterial": ROTATIONS_ONLY}),
        )
        .await;
        assert_eq!(
            explicit, default,
            "case {case:?}: {ROTATIONS_ONLY} is the default, got {explicit} and {default}"
        );

        let all = list(
            &router,
            &json!({"KeyId": key_id, "IncludeKeyMaterial": ALL_KEY_MATERIAL}),
        )
        .await;
        let listed = entries(&case, &all);
        assert_eq!(
            listed.len(),
            history + 1,
            "case {case:?}: expected the material the key was created with beside its \
             {history} rotation(s), got {all}"
        );
        let first = &listed[0];
        assert_eq!(
            first["KeyMaterialId"],
            json!(created_with),
            "case {case:?}: the first entry is the material the key was created with, got {all}"
        );
        assert_eq!(
            first["KeyId"],
            json!(key_arn),
            "case {case:?}: expected the entry to name the key {key_arn}, got {first}"
        );
        assert!(
            first["RotationDate"].is_null() && first["RotationType"].is_null(),
            "case {case:?}: the first key material is not a rotation, got {all}"
        );
        // The material the key was created with is the current one only while no
        // rotation has replaced it.
        let expected_state = if history == 0 { "CURRENT" } else { "NON_CURRENT" };
        assert_eq!(
            first["KeyMaterialState"],
            json!(expected_state),
            "case {case:?}: expected KeyMaterialState {expected_state}, got {all}"
        );
        for member in IMPORTED_MATERIAL_MEMBERS {
            assert!(
                first[member].is_null(),
                "case {case:?}: {member} describes imported key material, which this key has \
                 none of, got {first}"
            );
        }
        assert_eq!(
            listed[1..].to_vec(),
            entries(&case, &default),
            "case {case:?}: the rest of the listing is the rotations the default lists, got {all}"
        );
        let listed_rotations: Vec<String> = listed[1..]
            .iter()
            .map(|entry| text(&case, entry, "KeyMaterialId"))
            .collect();
        assert_eq!(
            listed_rotations, rotated,
            "case {case:?}: expected the rotations oldest first, got {all}"
        );
    }

    // Rows: a key whose key material neither automatic nor on-demand rotation replaces,
    // by the key spec and usage that make it so.
    let unrotatable: [(&str, &str); 6] = [
        ("HMAC_224", "GENERATE_VERIFY_MAC"),
        ("HMAC_512", "GENERATE_VERIFY_MAC"),
        ("RSA_3072", "ENCRYPT_DECRYPT"),
        ("RSA_4096", "SIGN_VERIFY"),
        ("ECC_NIST_P521", "SIGN_VERIFY"),
        ("SM2", "ENCRYPT_DECRYPT"),
    ];
    for (key_spec, key_usage) in unrotatable {
        let case = format!("{key_spec} key with usage {key_usage}");
        let metadata = create_key(
            &router,
            &json!({"KeySpec": key_spec, "KeyUsage": key_usage}),
        )
        .await;
        let key_id = text(&case, &metadata, "KeyId");

        // Such a key has simply had no rotation, which is a value and not a failure.
        let answer = list(&router, &json!({"KeyId": key_id})).await;
        assert_eq!(
            entries(&case, &answer),
            Vec::<Value>::new(),
            "case {case:?}: a key that has never rotated lists no rotation, got {answer}"
        );
        assert_eq!(
            answer["Truncated"],
            json!(false),
            "case {case:?}: a complete listing is not truncated, got {answer}"
        );

        let (status, code) = failure(
            &router,
            "ListKeyRotations",
            &json!({"KeyId": key_id, "IncludeKeyMaterial": ALL_KEY_MATERIAL}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "UnsupportedOperationException",
            "case {case:?}: expected UnsupportedOperationException, got {code}"
        );
    }
}

/// The listing is derived from the rotation records the key carries, so writing the key
/// for any other reason — and using its key material — leaves it byte for byte as it was,
/// and reading it persists nothing a later caller could see.
#[tokio::test]
async fn reading_the_listing_changes_nothing_a_later_caller_sees() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "unrelated writes"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    rotate(&router, "setup", &key_id, 2).await;
    let listing = list(&router, &json!({"KeyId": key_id})).await;
    let all = list(
        &router,
        &json!({"KeyId": key_id, "IncludeKeyMaterial": ALL_KEY_MATERIAL}),
    )
    .await;

    // Rows: a call that writes or uses the key without rotating its material.
    let writes: [(&str, &str, Value); 7] = [
        (
            "a new description",
            "UpdateKeyDescription",
            json!({"KeyId": key_id, "Description": "renamed"}),
        ),
        (
            "a new tag",
            "TagResource",
            json!({"KeyId": key_id, "Tags": [{"TagKey": "env", "TagValue": "prod"}]}),
        ),
        (
            "a tag removed",
            "UntagResource",
            json!({"KeyId": key_id, "TagKeys": ["env"]}),
        ),
        (
            "an alias added",
            "CreateAlias",
            json!({"AliasName": "alias/unrelated-rotation-write", "TargetKeyId": key_id}),
        ),
        (
            "the key material used to encrypt",
            "Encrypt",
            json!({"KeyId": key_id, "Plaintext": b64(b"list key rotations")}),
        ),
        (
            "a data key generated under the key",
            "GenerateDataKeyWithoutPlaintext",
            json!({"KeyId": key_id, "KeySpec": "AES_256"}),
        ),
        (
            "an automatic rotation schedule configured",
            "EnableKeyRotation",
            json!({"KeyId": key_id, "RotationPeriodInDays": 90}),
        ),
    ];
    for (case, operation, request) in writes {
        ok(&router, operation, &request).await;
        let answer = list(&router, &json!({"KeyId": key_id})).await;
        assert_eq!(
            answer, listing,
            "case {case:?}: {operation} must leave the rotation listing exactly as it was, got \
             {answer} (was {listing})"
        );
    }

    // A disabled key keeps the rotations it has had: disabling a key does not discard
    // the record of its key materials.
    ok(&router, "DisableKey", &json!({"KeyId": key_id})).await;
    let disabled = list(&router, &json!({"KeyId": key_id})).await;
    assert_eq!(
        disabled, listing,
        "case a disabled key: expected the rotations the key has had, got {disabled}"
    );
    ok(&router, "EnableKey", &json!({"KeyId": key_id})).await;

    // Reading is not writing: repeated reads, by either `IncludeKeyMaterial` value and
    // through a paged walk, answer the same listing and leave the key as it was.
    let described = describe(&router, &key_id).await;
    for round in 1..=3usize {
        let case = format!("read {round}");
        let again = list(&router, &json!({"KeyId": key_id})).await;
        assert_eq!(
            again, listing,
            "case {case:?}: reading the listing must not change what the next read sees, got \
             {again}"
        );
        let again_all = list(
            &router,
            &json!({"KeyId": key_id, "IncludeKeyMaterial": ALL_KEY_MATERIAL, "Limit": 1}),
        )
        .await;
        assert_eq!(
            entries(&case, &again_all).len(),
            1,
            "case {case:?}: expected the page that was asked for, got {again_all}"
        );
        let whole_all = list(
            &router,
            &json!({"KeyId": key_id, "IncludeKeyMaterial": ALL_KEY_MATERIAL}),
        )
        .await;
        assert_eq!(
            whole_all, all,
            "case {case:?}: a paged read must not change the whole listing, got {whole_all}"
        );
    }
    let after_reads = describe(&router, &key_id).await;
    assert_eq!(
        after_reads, described,
        "case reads: reading the listing must persist nothing, got {after_reads} (was {described})"
    );
}
