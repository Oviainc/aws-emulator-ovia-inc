// SPDX-License-Identifier: CC0-1.0

//! The length bound on an `Amendment$FieldPath`, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — and reads the amended record back through
//! `GetBenefitApplication`.
//!
//! The rule under test: a length bound this service enforces for a string itself counts
//! CHARACTERS, as the centralized walk counts a `@length` on a string, and a value past
//! it is refused with that length requirement. `Amendment$FieldPath` is an unconstrained
//! `String` (`smithy_query(query="shape",shape="Amendment")`) and the document it reaches
//! into is an unconstrained `Document` whose member names are caller-chosen text, so the
//! room one path has must not depend on the script those names are written in, and a path
//! refused only on length must not be reported as naming no amendable field.
//!
//! The rows span what the rule generalises over: the character WIDTH of the key (one,
//! two, three and four bytes per character, so one character count weighs 1x, 2x, 3x and
//! 4x as many bytes) and the path's length relative to the published bound (the shortest
//! path into the document, a length well inside the bound, one character short of it, at
//! it, one past it and far past it).

use serde_json::json;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::MAX_FIELD_PATH_LEN;

use crate::amend_benefit_application::{clause, message_of};
use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};

/// The field an amendment reaches into: the one member of the record modelled as a
/// `Document`, so the segment below it is a name the caller chose.
const DETAILS_FIELD: &str = "BenefitApplicationDetails";

/// The requirement a `FieldPath` past the bound fails: the length wording this service
/// words EVERY length with (`service::constraint_requirement` of a max-length violation).
fn too_long_requirement() -> String {
    format!("Member must have length less than or equal to {MAX_FIELD_PATH_LEN}")
}

/// The requirement a path naming nothing amendable fails — which a path refused only on
/// LENGTH must not be reported with.
const UNAMENDABLE_PREFIX: &str = "Member must name one of the amendable fields";

/// A `FieldPath` into the details document of exactly `characters` characters, whose key
/// is written in `unit` — one character repeated.
fn details_path(unit: &str, characters: usize) -> (String, String) {
    assert_eq!(
        unit.chars().count(),
        1,
        "each width is built from ONE character, got {unit:?}"
    );
    let prefix = format!("{DETAILS_FIELD}.");
    let key = unit.repeat(
        characters
            .checked_sub(prefix.chars().count())
            .unwrap_or_else(|| panic!("a {characters}-character path holds no key at all")),
    );
    (format!("{prefix}{key}"), key)
}

/// A `FieldPath` whose character count the published bound admits is applied and the key
/// it names is stored verbatim; one character past the bound is refused with the LENGTH
/// requirement it failed — in every character width alike, so the room a path has does
/// not depend on the script its key is written in.
#[tokio::test]
async fn the_field_path_bound_counts_characters_not_bytes() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");
    let created = harness
        .create(
            "seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-path-seed",
                "BenefitIdentifier": benefit_id,
                "BenefitApplicationDetails": { "CampaignName": "spring" },
            }),
        )
        .await;
    let id = text("seed", &created, "Id");

    // (case, one character of that width) — one, two, three and four UTF-8 bytes.
    let widths: [(&str, &str); 4] = [
        ("one-byte", "a"),
        ("two-byte", "é"),
        ("three-byte", "\u{30ad}"),
        ("four-byte", "\u{1d11e}"),
    ];

    // The character counts the rule is stated over: the shortest path that names a key,
    // one well inside the bound, one short of the bound, the bound, one past it and far
    // past it.
    let counts: [usize; 6] = [
        DETAILS_FIELD.len() + 2,
        126,
        MAX_FIELD_PATH_LEN - 1,
        MAX_FIELD_PATH_LEN,
        MAX_FIELD_PATH_LEN + 1,
        MAX_FIELD_PATH_LEN * 2,
    ];

    let mut row = 0usize;
    for (width, unit) in widths {
        for characters in counts {
            let (path, key) = details_path(unit, characters);
            assert_eq!(
                path.chars().count(),
                characters,
                "case {width:?}: the row builds a path of {characters} characters, got {path:?}"
            );
            // The prefix is ASCII (one byte per character); the key weighs this width's,
            // so every width but the first sends more BYTES than characters.
            let expected_bytes = DETAILS_FIELD.len() + 1 + unit.len() * key.chars().count();
            assert_eq!(
                path.len(),
                expected_bytes,
                "case {width:?}: expected a {characters}-character path of {expected_bytes} \
                 bytes, got {} in {path:?}",
                path.len()
            );

            let inside = characters <= MAX_FIELD_PATH_LEN;
            let case = format!("{width}, {characters} characters");
            let value = format!("v{row}");
            let before = harness.get(&case, &id).await;
            let (status, out) = harness
                .call(
                    "AmendBenefitApplication",
                    &json!({
                        "Catalog": CATALOG,
                        "ClientToken": format!("tok-path-{row}"),
                        "Identifier": id,
                        "Revision": text(&case, &before, "Revision"),
                        "AmendmentReason": "the partner corrected the application",
                        "Amendments": [{ "FieldPath": path, "NewValue": value }],
                    }),
                )
                .await;

            if inside {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a path within the bound must be applied, got {status} {out:?}"
                );
                // Round-trip: the caller-chosen key is addressed verbatim, so the read
                // reports the value under exactly the key the path named.
                let after = harness.get(&case, &id).await;
                assert_eq!(
                    after["BenefitApplicationDetails"][&key],
                    json!(value),
                    "case {case:?}: expected {value:?} stored at {key:?}, got {after:?}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a path past the bound must be refused, got {status} {out:?}"
                );
                assert_eq!(
                    error_code(&out),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {out:?}"
                );
                let message = message_of(&out);
                assert_eq!(
                    message,
                    clause("amendments.1.member.fieldPath", &too_long_requirement()),
                    "case {case:?}: expected the length requirement, got {message:?}"
                );
                assert!(
                    !message.contains(UNAMENDABLE_PREFIX),
                    "case {case:?}: a path refused on length must not be reported as naming \
                     no amendable field, got {message:?}"
                );
                let after = harness.get(&case, &id).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused amendment must change nothing, got {after:?}"
                );
            }

            row += 1;
        }
    }
}
