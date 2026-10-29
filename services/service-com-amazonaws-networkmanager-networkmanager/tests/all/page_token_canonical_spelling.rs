// SPDX-License-Identifier: CC0-1.0

//! A continuation token is honoured only in the ONE spelling this service minted it in,
//! driven over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`).
//!
//! The rule under test: a `NextToken` is opaque, so only the exact string a response of
//! this service carried names a position in a listing; any other string — including one
//! that differs from a minted token only in the case of its hex digits, and therefore
//! decodes to the same bytes — is a token this service never issued and is refused on the
//! member, `ValidationException` (HTTP 400) with `Reason` `FieldValidationFailed` and one
//! `Fields` entry naming `NextToken`.
//!
//! The table below spans that class in both directions: the listings (three of the
//! service's paginated operations, each paging over its own kind of record) crossed with
//! re-spellings of the token each one really minted (every digit upper-cased, alternating
//! digits upper-cased, and a single hex letter upper-cased at each end of the string).
//! Every row is a string that decodes to the bytes of the minted token, which is what the
//! counter-case pins down: the minted spelling itself still resumes the listing after all
//! of them, so the refusal is about the spelling and not about pagination.
//!
//! Nothing here reaches into `crate::state`: every resource is created and every token is
//! obtained through an operation.

use serde_json::{json, Value};

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator};

/// The `Reason` a `ValidationException` carries when ONE input member is at fault, from
/// the modelled enumeration (`smithy_query shape=ValidationExceptionReason`:
/// `UnknownOperation`, `CannotParse`, `FieldValidationFailed`, `Other`).
const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// The input member every paginated listing carries its continuation token in
/// (`NextToken`, `@httpQuery("nextToken")` on each listing's request shape).
const FIELD_NEXT_TOKEN: &str = "NextToken";

/// One paginated listing under test: the case name, the URI it is reached at, the output
/// member its page arrives in and the id member of each item on that page.
struct Listing {
    case: &'static str,
    uri: String,
    list: &'static str,
    id_member: &'static str,
    /// The two ids the listing holds, in the key order the pages walk.
    ids: Vec<String>,
}

/// Request `listing` with a page size of one and `token` in its `nextToken` member.
async fn page(emulator: &Emulator, listing: &Listing, token: Option<&str>) -> (u16, Value) {
    let separator = if listing.uri.contains('?') { '&' } else { '?' };
    let mut uri = format!("{}{separator}maxResults=1", listing.uri);
    if let Some(token) = token {
        uri.push_str(&format!("&nextToken={token}"));
    }
    emulator.get(&uri).await
}

/// Re-spellings of `token` that differ from it only in the case of its hex digits, so each
/// one decodes to the bytes of the token this service minted while being a string no
/// response of this service ever carried.
fn recasings(token: &str) -> Vec<(&'static str, String)> {
    // Only the `a`-`f` digits have a case at all, so every row below re-cases a selection
    // of THOSE, which is why each one really differs from the minted spelling.
    let letters: Vec<usize> = token
        .char_indices()
        .filter(|(_, digit)| digit.is_ascii_alphabetic())
        .map(|(index, _)| index)
        .collect();
    assert!(
        letters.len() >= 2,
        "the minted token carries hex letters to re-case, got {token:?}"
    );
    let uppercase = |targets: &[usize]| -> String {
        token
            .char_indices()
            .map(|(index, digit)| {
                if targets.contains(&index) {
                    digit.to_ascii_uppercase()
                } else {
                    digit
                }
            })
            .collect()
    };
    let alternating: Vec<usize> = letters.iter().copied().step_by(2).collect();
    let first = letters[0];
    let last = letters[letters.len() - 1];
    vec![
        ("every digit upper-cased", token.to_ascii_uppercase()),
        ("alternating hex letters upper-cased", uppercase(&alternating)),
        ("the first hex letter upper-cased", uppercase(&[first])),
        ("the last hex letter upper-cased", uppercase(&[last])),
    ]
}

/// A global network holding two sites and two devices, beside a second global network, so
/// each listing in the table has two records and therefore mints a token.
async fn fixture(emulator: &Emulator) -> Vec<Listing> {
    let first_network = emulator.create_global_network("page token spelling a").await;
    let second_network = emulator.create_global_network("page token spelling b").await;

    let mut sites = vec![
        emulator.create_site(&first_network, "site-a").await,
        emulator.create_site(&first_network, "site-b").await,
    ];
    let mut devices = vec![
        create_device_ok(
            emulator,
            &first_network,
            json!({ "Description": "device-a" }),
        )
        .await,
        create_device_ok(
            emulator,
            &first_network,
            json!({ "Description": "device-b" }),
        )
        .await,
    ];
    let mut networks = vec![first_network.clone(), second_network];
    // The pages walk the listing in key order, which for these listings is the order of the
    // minted ids.
    networks.sort();
    sites.sort();
    devices.sort();

    vec![
        Listing {
            case: "DescribeGlobalNetworks",
            uri: "/global-networks".to_string(),
            list: "GlobalNetworks",
            id_member: "GlobalNetworkId",
            ids: networks,
        },
        Listing {
            case: "GetSites",
            uri: format!("/global-networks/{first_network}/sites"),
            list: "Sites",
            id_member: "SiteId",
            ids: sites,
        },
        Listing {
            case: "GetDevices",
            uri: format!("/global-networks/{first_network}/devices"),
            list: "Devices",
            id_member: "DeviceId",
            ids: devices,
        },
    ]
}

#[tokio::test]
async fn a_recased_spelling_of_a_minted_page_token_is_refused_on_the_member() {
    let emulator = Emulator::new();

    for listing in fixture(&emulator).await {
        let (status, first) = page(&emulator, &listing, None).await;
        assert_eq!(
            status, 200,
            "case {}/first page: expected 200, got {status} with {first}",
            listing.case
        );
        assert_eq!(
            ids_of(&first, listing.list, listing.id_member),
            listing.ids[..1].to_vec(),
            "case {}/first page: one record is due on a page of one, got {first}",
            listing.case
        );
        let minted = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {}/first page: two records need a NextToken, got {first}",
                    listing.case
                )
            })
            .to_string();
        assert!(
            minted
                .chars()
                .all(|digit| digit.is_ascii_digit() || ('a'..='f').contains(&digit)),
            "case {}/first page: the minted token is lowercase hex, got {minted:?}",
            listing.case
        );

        for (recasing, token) in recasings(&minted) {
            let case = format!("{}/{recasing}", listing.case);
            assert_ne!(
                token, minted,
                "case {case}: the row must differ from the minted token, got {token:?}"
            );
            assert!(
                token.eq_ignore_ascii_case(&minted),
                "case {case}: the row must be a re-casing of the minted token {minted:?}, got {token:?}"
            );

            let (status, body) = page(&emulator, &listing, Some(&token)).await;
            assert_eq!(
                status, 400,
                "case {case}: expected 400 for a spelling this service never issued, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["Reason"],
                json!(REASON_FIELD_VALIDATION_FAILED),
                "case {case}: expected Reason {REASON_FIELD_VALIDATION_FAILED}, got {body}"
            );
            assert_eq!(
                body["Fields"][0]["Name"],
                json!(FIELD_NEXT_TOKEN),
                "case {case}: the refusal must name the {FIELD_NEXT_TOKEN} member, got {body}"
            );
            assert!(
                body["Fields"][1].is_null(),
                "case {case}: one member is at fault, so one Fields entry is due, got {body}"
            );
            assert!(
                body[listing.list].is_null(),
                "case {case}: a refused request must carry no page, got {body}"
            );
        }

        // The counter-case: the spelling this service minted still resumes the listing
        // after every refusal above, so the refusals are about the spelling.
        let (status, resumed) = page(&emulator, &listing, Some(&minted)).await;
        assert_eq!(
            status, 200,
            "case {}/minted spelling: the token this service issued must be honoured, got {status} with {resumed}",
            listing.case
        );
        assert_eq!(
            ids_of(&resumed, listing.list, listing.id_member),
            listing.ids[1..].to_vec(),
            "case {}/minted spelling: the token must resume after the first record, got {resumed}",
            listing.case
        );
        assert!(
            resumed["NextToken"].is_null(),
            "case {}/minted spelling: the last page carries no token, got {resumed}",
            listing.case
        );
    }
}
