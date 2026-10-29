// SPDX-License-Identifier: CC0-1.0

//! The length bound on the `FileName` a write DERIVES for a stored `FileDetail`, end to
//! end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — and reads the stored record back through
//! `GetBenefitApplication`.
//!
//! The rule under test: a `@length` bound this service applies to a string itself counts
//! CHARACTERS, so the name derived from a `FileURI` is reported for every last path
//! segment the modelled `FileDetail$FileName` `@length(min: 1, max: 255)` admits
//! (`smithy_query(query="shape",shape="FileDetail")`) and for no longer one, whatever
//! each character weighs in UTF-8.
//!
//! The rows span what the rule generalises over: the character WIDTH of the name (one,
//! two, three and four bytes per character), its position relative to the published
//! bounds (at the minimum, in the middle, at the maximum, one past it and far past it)
//! and the two operations that store a `FileDetail` (`CreateBenefitApplication` and
//! `UpdateBenefitApplication`). The `FileType` derived from the same segment is asserted
//! beside the name, since it is reported only when the name is.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    MAX_FILE_NAME_LEN, MIN_FILE_NAME_LEN,
};

use crate::benefit_application_lifecycle::{text, Harness, CATALOG};

/// The extension every name below one character long carries, and the `FileType` the
/// model's `FileType` enum gives it (`FILE_TYPES_BY_EXTENSION`, pinned row by row in
/// `tests/all/benefit_application_lifecycle.rs`).
const EXTENSION: &str = ".pdf";
const EXTENSION_TYPE: &str = "application/pdf";

/// How many characters of a `characters`-character name [`EXTENSION`] takes: all of them
/// when the name is too short to hold one beside a body, none otherwise.
fn extension_characters(characters: usize) -> usize {
    if characters > EXTENSION.len() {
        EXTENSION.len()
    } else {
        0
    }
}

/// A name of exactly `characters` characters built from `unit`, carrying [`EXTENSION`]
/// whenever it is long enough to hold one, so the `FileType` derived from the same
/// segment is exercised by the same row.
fn name_of(unit: &str, characters: usize) -> String {
    assert_eq!(
        unit.chars().count(),
        1,
        "each width is built from ONE character, got {unit:?}"
    );
    let body = unit.repeat(characters - extension_characters(characters));
    if extension_characters(characters) == 0 {
        body
    } else {
        format!("{body}{EXTENSION}")
    }
}

/// Assert the stored file reports `uri` and derives `name` exactly when the modelled
/// bound admits it.
fn assert_derived(case: &str, file: &Value, uri: &str, name: &str, inside: bool) {
    assert_eq!(
        file["FileURI"],
        json!(uri),
        "case {case:?}: the URI must round-trip, got {file:?}"
    );
    if !inside {
        assert!(
            file.get("FileName").is_none_or(Value::is_null),
            "case {case:?}: a name outside the bound carries none, got {file:?}"
        );
        assert!(
            file.get("FileType").is_none_or(Value::is_null),
            "case {case:?}: no type is derived without a name, got {file:?}"
        );
        return;
    }
    assert_eq!(
        file["FileName"],
        json!(name),
        "case {case:?}: expected the last path segment as the FileName, got {file:?}"
    );
    let stored = text(case, file, "FileName");
    assert_eq!(
        stored.chars().count(),
        name.chars().count(),
        "case {case:?}: expected {} stored characters, got {stored:?}",
        name.chars().count()
    );
    if name.ends_with(EXTENSION) {
        assert_eq!(
            file["FileType"],
            json!(EXTENSION_TYPE),
            "case {case:?}: expected the type of the derived name, got {file:?}"
        );
    } else {
        assert!(
            file.get("FileType").is_none_or(Value::is_null),
            "case {case:?}: a name with no extension carries no type, got {file:?}"
        );
    }
}

/// The one stored file of the application `identifier`.
async fn stored_file(harness: &Harness, case: &str, identifier: &str) -> Value {
    let got = harness.get(case, identifier).await;
    let files = got["FileDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected FileDetails, got {got:?}"))
        .clone();
    assert_eq!(
        files.len(),
        1,
        "case {case:?}: expected one stored file, got {files:?}"
    );
    files[0].clone()
}

/// A `FileName` of any character width the modelled bound admits is derived and reported;
/// one character past the bound is not, in every width alike.
#[tokio::test]
async fn the_derived_file_name_bound_counts_characters_not_bytes() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");

    // (case, one character of that width) — one, two, three and four UTF-8 bytes, so a
    // name of a given CHARACTER count weighs 1x, 2x, 3x and 4x as many bytes.
    let widths: [(&str, &str); 4] = [
        ("one-byte", "a"),
        ("two-byte", "é"),
        ("three-byte", "\u{4e2d}"),
        ("four-byte", "\u{1d11e}"),
    ];

    // The character counts the rule is stated over: each published bound, one past the
    // maximum, far past it, and two counts between them.
    let counts: [usize; 6] = [
        MIN_FILE_NAME_LEN,
        5,
        MAX_FILE_NAME_LEN - 1,
        MAX_FILE_NAME_LEN,
        MAX_FILE_NAME_LEN + 1,
        MAX_FILE_NAME_LEN * 3,
    ];

    let mut row = 0usize;
    for (width, unit) in widths {
        for characters in counts {
            let name = name_of(unit, characters);
            assert_eq!(
                name.chars().count(),
                characters,
                "the row builds a name of {characters} characters, got {name:?}"
            );
            // `EXTENSION` is ASCII, so it weighs one byte per character; the body weighs
            // this width's.
            let expected_bytes = unit.len() * (characters - extension_characters(characters))
                + extension_characters(characters);
            assert_eq!(
                name.len(),
                expected_bytes,
                "case {width:?}: expected a {characters}-character name of {expected_bytes} \
                 bytes, got {} in {name:?}",
                name.len()
            );
            let inside = (MIN_FILE_NAME_LEN..=MAX_FILE_NAME_LEN).contains(&characters);
            let case = format!("{width}, {characters} characters, created");

            // CreateBenefitApplication stores the file.
            let created_uri = format!("s3://b/{name}");
            let created = harness
                .create(
                    &case,
                    &json!({
                        "Catalog": CATALOG,
                        "ClientToken": format!("tok-name-{row}"),
                        "BenefitIdentifier": benefit_id,
                        "FileDetails": [{ "FileURI": created_uri, "BusinessUseCase": "evidence" }],
                    }),
                )
                .await;
            let id = text(&case, &created, "Id");
            let file = stored_file(&harness, &case, &id).await;
            assert_derived(&case, &file, &created_uri, &name, inside);

            // UpdateBenefitApplication builds the stored file the same way, so the same
            // name under a different URI answers the same.
            let case = format!("{width}, {characters} characters, updated");
            let updated_uri = format!("https://files.example.com/evidence/{name}");
            let before = harness.get(&case, &id).await;
            harness
                .ok(
                    &case,
                    "UpdateBenefitApplication",
                    &json!({
                        "Catalog": CATALOG,
                        "ClientToken": format!("tok-name-update-{row}"),
                        "Identifier": id,
                        "Revision": text(&case, &before, "Revision"),
                        "FileDetails": [{ "FileURI": updated_uri, "BusinessUseCase": "evidence" }],
                    }),
                )
                .await;
            let file = stored_file(&harness, &case, &id).await;
            assert_derived(&case, &file, &updated_uri, &name, inside);

            row += 1;
        }
    }
}
