// SPDX-License-Identifier: CC0-1.0

//! The language a result's own NAME is written in.
//!
//! `Language` is "a list of BCP 47 compliant language codes for the results to be
//! rendered in. If there is no data for the result in the requested language, data
//! will be returned in the default language for the entry", so a request that names
//! one fixes the language of every result's text. A request that names NONE still
//! asked for a place by SOME name — a catalog entry is findable by the name any
//! language gives it — and the name it asked by is the display name it gets back:
//! the place's own name is written in the language the query named it in, and falls
//! back to the entry's own only where the query named the entry's own name, or named
//! no name of the place at all.
//!
//! The table's rows differ along the LANGUAGE the query names the place in, how the
//! name is typed (in full, partly typed, with the city beside it), which record it
//! belongs to, and what the request asks for (nothing, a language the entry holds,
//! one it does not) — rather than along one recorded request. Every row is asserted
//! on each operation that matches records by free-form text, and the operations that
//! resolve a record by id or by position — which carry no query to name anything —
//! keep reporting the entry's own name.

use serde_json::{json, Value};

use crate::places_catalog::{items, position, text, Emulator, Place};
use crate::result_item_composition::address;

/// The routes of the operations that match records by a free-form `QueryText`
/// (this crate's `OPERATIONS.md`), each of which reports a `Title` per result.
const TEXT_ROUTES: [(&str, &str); 4] = [
    ("SearchText", "/v2/search-text"),
    ("Geocode", "/v2/geocode"),
    ("Autocomplete", "/v2/autocomplete"),
    ("Suggest", "/v2/suggest"),
];

/// A page size inside every one of those operations' `MaxResults` ranges
/// (`AutocompleteRequest$MaxResults` is the narrowest, `range 1..20`), so a row's
/// place is never cut off by the default page.
const MAX_RESULTS: i64 = 20;

/// A row of the name-language table.
struct NameRow {
    case: &'static str,
    /// The catalog record under test, by the name the entry itself gives it.
    record: &'static str,
    /// `QueryText`.
    query: &'static str,
    /// `Language`, a `LanguageTag` (`len 2..35`), or `None` for a request that asks
    /// for no language at all.
    asked: Option<&'static str>,
    /// The name every operation must report as that record's `Title`.
    name: &'static str,
}

/// The record the catalog holds under its own name.
fn seeded(catalog: &[Place], name: &str) -> Place {
    catalog
        .iter()
        .find(|place| place.name == name)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case fixture: the catalog must seed {name:?}, got {:?}",
                catalog.iter().map(|place| &place.name).collect::<Vec<_>>()
            )
        })
}

/// The item of a response titled `name`, or `None` when no item is.
fn titled(body: &Value, name: &str) -> Option<Value> {
    items(body)
        .into_iter()
        .find(|item| text(item, "Title") == name)
}

/// The titles a response reported, in order.
fn titles(body: &Value) -> Vec<String> {
    items(body).iter().map(|item| text(item, "Title")).collect()
}

/// A place found by the name ONE language gives it is named in THAT language when
/// the request asks for no `Language`, while a request that asks for one is answered
/// in it (or in the entry's own, where the entry holds no text in it).
///
/// The names each row queries by are the seeded record's own ones — its title, its
/// alternate names and its localized titles (`seed_catalog`'s `with_alternate_names`
/// / `with_localized_titles` in `crate::state`) — and the rows cover a language the
/// issue's own example did not name (German, Spanish) as well as English.
#[tokio::test]
async fn a_place_found_by_the_name_one_language_gives_it_is_named_in_that_language() {
    let emulator = Emulator::new();
    let catalog = emulator.catalog();
    let rows = [
        NameRow {
            // The entry's own name: there is no other language to resolve to.
            case: "the-entry-s-own-name",
            record: "Tour Eiffel",
            query: "Tour Eiffel",
            asked: None,
            name: "Tour Eiffel",
        },
        NameRow {
            // An alternate name carries no language of its own, so the entry's own
            // name answers it.
            case: "an-alternate-name",
            record: "Tour Eiffel",
            query: "La Tour Eiffel",
            asked: None,
            name: "Tour Eiffel",
        },
        NameRow {
            case: "the-english-name",
            record: "Tour Eiffel",
            query: "Eiffel Tower",
            asked: None,
            name: "Eiffel Tower",
        },
        NameRow {
            case: "the-german-name",
            record: "Tour Eiffel",
            query: "Eiffelturm",
            asked: None,
            name: "Eiffelturm",
        },
        NameRow {
            case: "the-spanish-name",
            record: "Tour Eiffel",
            query: "Torre Eiffel",
            asked: None,
            name: "Torre Eiffel",
        },
        NameRow {
            // Only the leading characters of the name's last word were typed.
            case: "a-partly-typed-english-name",
            record: "Tour Eiffel",
            query: "Eiffel Tow",
            asked: None,
            name: "Eiffel Tower",
        },
        NameRow {
            // The name with the city beside it: a word the name accounts for
            // nothing of does not unname it.
            case: "the-english-name-and-the-city",
            record: "Tour Eiffel",
            query: "Eiffel Tower Paris",
            asked: None,
            name: "Eiffel Tower",
        },
        NameRow {
            // One word every one of this entry's names spells the same way names no
            // language in particular, so the entry's own name answers it.
            case: "a-word-every-language-shares",
            record: "Tour Eiffel",
            query: "Eiffel",
            asked: None,
            name: "Tour Eiffel",
        },
        NameRow {
            // A request that names a language is rendered in it, whichever language
            // its query used.
            case: "a-language-asked-for-overrides-the-query-s",
            record: "Tour Eiffel",
            query: "Eiffel Tower",
            asked: Some("de"),
            name: "Eiffelturm",
        },
        NameRow {
            // "If there is no data for the result in the requested language, data
            // will be returned in the default language for the entry."
            case: "a-language-asked-for-that-the-entry-does-not-hold",
            record: "Tour Eiffel",
            query: "Eiffel Tower",
            asked: Some("ja"),
            name: "Tour Eiffel",
        },
        NameRow {
            // A second record, so no row's answer can come from one entry's data.
            case: "the-english-name-of-another-record",
            record: "Musee du Louvre",
            query: "Louvre Museum",
            asked: None,
            name: "Louvre Museum",
        },
        NameRow {
            case: "the-spanish-name-of-another-record",
            record: "Musee du Louvre",
            query: "Museo del Louvre",
            asked: None,
            name: "Museo del Louvre",
        },
        NameRow {
            case: "a-partly-typed-own-name",
            record: "Musee du Louvre",
            query: "Musee du Lou",
            asked: None,
            name: "Musee du Louvre",
        },
        NameRow {
            case: "a-regional-variant-of-a-held-language",
            record: "Musee du Louvre",
            query: "Louvre",
            asked: Some("en-GB"),
            name: "Louvre Museum",
        },
    ];

    for row in rows {
        let place = seeded(&catalog, row.record);
        // The address text the entry carries after its own name: the part of its
        // label the place's name does NOT lead, which no query renames — only the
        // name follows the language the query used.
        let tail = place
            .label
            .strip_prefix(place.name.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the record's label must lead with its own name, got {:?}",
                    row.case, place.label
                )
            })
            .to_string();

        for (operation, route) in TEXT_ROUTES {
            let mut body = json!({"QueryText": row.query, "MaxResults": MAX_RESULTS});
            if let Some(asked) = row.asked {
                body["Language"] = json!(asked);
            }
            let case = format!("{}-{operation}", row.case);
            let returned = emulator.post_ok(&case, route, &body).await;
            let item = titled(&returned, row.name).unwrap_or_else(|| {
                panic!(
                    "case {case}: expected an item named {:?} for the query {:?}, got {:?}",
                    row.name,
                    row.query,
                    titles(&returned)
                )
            });
            // The same record, named differently — never a second record invented to
            // carry the name. Suggest reports a place's own members in a nested
            // `Place` structure; every other operation reports them on the item.
            let reported = item.get("Place").cloned().unwrap_or_else(|| item.clone());
            assert_eq!(
                text(&reported, "PlaceId"),
                place.place_id,
                "case {case}: expected the name {:?} on the seeded record, got {item:?}",
                row.name
            );
            // The place's own name leads its assembled label, so the label follows
            // the same language while the address text after it stays the entry's.
            assert_eq!(
                text(&address(&reported), "Label"),
                format!("{}{tail}", row.name),
                "case {case}: expected the label led by {:?} over the entry's own \
                 address text, got {reported:?}",
                row.name
            );
        }
    }

    // A request that names no place by name has no name to answer under: GetPlace
    // resolves a `PlaceId` and ReverseGeocode a position, so both keep reporting the
    // entry's own name whatever language a client last searched in.
    for record in ["Tour Eiffel", "Musee du Louvre"] {
        let place = seeded(&catalog, record);
        let detail = emulator
            .get_ok(record, &format!("/v2/place/{}", place.place_id))
            .await;
        assert_eq!(
            (text(&detail, "Title"), text(&address(&detail), "Label")),
            (place.name.clone(), place.label.clone()),
            "case {record}: GetPlace resolves no query, so it reports the entry's own name"
        );
        let reverse = emulator
            .post_ok(
                record,
                "/v2/reverse-geocode",
                &json!({"QueryPosition": [place.position[0], place.position[1]]}),
            )
            .await;
        let first = items(&reverse).first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            (text(&first, "Title"), position(&first)),
            (place.name.clone(), place.position.to_vec()),
            "case {record}: ReverseGeocode resolves a position, so it reports the entry's \
             own name, got {first:?}"
        );
    }
}
