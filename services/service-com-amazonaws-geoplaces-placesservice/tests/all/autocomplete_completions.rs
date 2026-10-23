// SPDX-License-Identifier: CC0-1.0

//! Autocomplete: completing a partially typed query against the seeded place
//! catalog, the sections of each completion it marks as matched, and the number of
//! completions one call reports.
//!
//! Each test is a TABLE whose rows differ along what its rule generalises over —
//! how much of a word the query typed, how many words it typed, which part of a
//! place the typed words name (its own name, an alternate name, a street, an
//! address number, a postal code), and the page size asked for — rather than along
//! one recorded request. The emulator is driven as real HTTP through the same
//! router the front door serves, reusing the harness in [`crate::places_catalog`].

use serde_json::{json, Value};

use crate::places_catalog::{assert_success, items, text, Emulator, PRICING_BUCKET_HEADER};

/// Autocomplete's route (`POST /v2/autocomplete`, this crate's `OPERATIONS.md`).
const AUTOCOMPLETE: &str = "/v2/autocomplete";

/// "Default value: 5" (`AutocompleteRequest$MaxResults`).
const DEFAULT_MAX_RESULTS: usize = 5;

/// The published bounds of `AutocompleteRequest$MaxResults` (`range 1..20`); the
/// upper one is also the maximum length of `AutocompleteResultItemList`
/// (`len 0..20`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 20;

/// The pricing tier every Places response is charged in by default.
const PRICING_BUCKET_CORE: &str = "Core";

// ---------------------------------------------------------------------------
// Reading a response
// ---------------------------------------------------------------------------

/// The titles a completion list reported, in order.
fn titles(body: &Value) -> Vec<String> {
    items(body).iter().map(|item| text(item, "Title")).collect()
}

/// One `HighlightList` entry as the wire reports it: `(StartIndex, EndIndex,
/// Value)`.
type Section = (i64, i64, String);

/// One highlighted member of a result item: the member path, the text(s) of the
/// response that member reports, and the sections reported for it.
type Marked = (String, Vec<String>, Vec<Section>);

/// One `HighlightList` as `(StartIndex, EndIndex, Value)` triples. An absent
/// `StartIndex` or `EndIndex` reads as -1, which no assertion accepts.
fn sections(list: Option<&Value>) -> Vec<Section> {
    list.and_then(Value::as_array)
        .map(|found| {
            found
                .iter()
                .map(|section| {
                    (
                        section
                            .get("StartIndex")
                            .and_then(Value::as_i64)
                            .unwrap_or(-1),
                        section.get("EndIndex").and_then(Value::as_i64).unwrap_or(-1),
                        text(section, "Value"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `[start, end)` CHAR span of a piece of text, as a `Highlight` indexes it.
fn char_span(text: &str, start: i64, end: i64) -> String {
    let start = start.max(0) as usize;
    let length = (end - start as i64).max(0) as usize;
    text.chars().skip(start).take(length).collect()
}

/// The words of a piece of text, normalized the way the service indexes a term:
/// lowercased, with every non-alphanumeric character a separator.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Every highlighted part of one result item: the member the sections belong to,
/// the text(s) of the response that member reports, and the sections themselves.
///
/// `CountryHighlights$Code` carries ONE list while the reported `Country` carries
/// an alpha-2 and an alpha-3 code, so that entry names both as candidate texts.
/// `Address$Intersection` is absent from every record this catalog seeds, so no
/// row of this table reports intersection highlights.
fn highlighted(item: &Value) -> Vec<Marked> {
    let highlights = item.get("Highlights").cloned().unwrap_or(Value::Null);
    let address = item.get("Address").cloned().unwrap_or(Value::Null);
    let address_highlights = highlights.get("Address").cloned().unwrap_or(Value::Null);

    let mut found = Vec::new();
    let title_sections = sections(highlights.get("Title"));
    if !title_sections.is_empty() {
        found.push((
            "Title".to_string(),
            vec![text(item, "Title")],
            title_sections,
        ));
    }
    // The `Address` members this catalog can report text for; a member no record
    // carries can highlight nothing.
    for member in [
        "Label",
        "Locality",
        "District",
        "SubDistrict",
        "Street",
        "Block",
        "SubBlock",
        "PostalCode",
        "AddressNumber",
        "Building",
    ] {
        let reported = sections(address_highlights.get(member));
        if !reported.is_empty() {
            found.push((
                format!("Address.{member}"),
                vec![text(&address, member)],
                reported,
            ));
        }
    }
    // The nested highlight structures, as `(member, its highlight list, the
    // member's own texts that list marks)`. A `Country` carries an alpha-2 and an
    // alpha-3 code while `CountryHighlights$Code` carries ONE list for them.
    let nested: [(&str, &str, &[&str]); 6] = [
        ("Country", "Code", &["Code2", "Code3"]),
        ("Country", "Name", &["Name"]),
        ("Region", "Code", &["Code"]),
        ("Region", "Name", &["Name"]),
        ("SubRegion", "Code", &["Code"]),
        ("SubRegion", "Name", &["Name"]),
    ];
    for (member, list, sources) in nested {
        let reported = address.get(member).cloned().unwrap_or(Value::Null);
        let member_highlights = address_highlights
            .get(member)
            .cloned()
            .unwrap_or(Value::Null);
        let found_sections = sections(member_highlights.get(list));
        if !found_sections.is_empty() {
            found.push((
                format!("Address.{member}.{list}"),
                sources
                    .iter()
                    .map(|source| text(&reported, source))
                    .collect(),
                found_sections,
            ));
        }
    }
    found
}

/// Every highlighted section of one item marks the text the RESPONSE carries:
/// `Highlights` describes "the sections of the response which matched to input
/// query terms", so a section's `Value` is the `[StartIndex, EndIndex)` span of the
/// member it belongs to. Answers how many sections the item reported.
fn assert_sections_mark_the_response(case: &str, item: &Value) -> usize {
    let mut counted = 0;
    for (member, candidates, found) in highlighted(item) {
        for (start, end, value) in found {
            assert!(
                start >= 0 && end > start,
                "case {case}: expected a non-empty {member} section, got ({start}, {end}, {value:?})"
            );
            let spans: Vec<String> = candidates
                .iter()
                .map(|text| char_span(text, start, end))
                .collect();
            assert!(
                spans.contains(&value),
                "case {case}: expected the {member} section ({start}, {end}) to spell the span it \
                 marks in the response, got {value:?} against {spans:?}"
            );
            counted += 1;
        }
    }
    counted
}

/// Every highlighted section was named by the query: each of its words either
/// begins with a typed word (the whole word a term matched) or is begun by one (the
/// characters a partially typed word, or a standard abbreviation, covers).
fn assert_sections_were_typed(case: &str, item: &Value, query: &str) {
    let typed = words(query);
    for (member, _, found) in highlighted(item) {
        for (_, _, value) in found {
            for word in words(&value) {
                assert!(
                    typed
                        .iter()
                        .any(|term| word.starts_with(term) || term.starts_with(&word)),
                    "case {case}: expected every word of the {member} section {value:?} to be \
                     named by the query {query:?}, got {word:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Which completions a partial query answers with
// ---------------------------------------------------------------------------

/// A row of the completion table: what the user typed, and a place the service
/// must complete it to.
struct PrefixRow {
    case: &'static str,
    query: &'static str,
    /// The `Title` the completion list must carry.
    expected_title: &'static str,
}

/// Autocomplete "completes potential places and addresses as the user types, based
/// on the partial input": a query whose words name the leading characters of a
/// place's own text answers with that place, and every completion it answers with
/// is one the typed words name.
///
/// The rows differ along how much of the last word was typed (a whole word, half of
/// one), how many words were typed, and WHICH text of the place they name — its own
/// name, an alternate name it is commonly searched by, its street and address
/// number, its postal code — which is what the completion rule generalises over.
#[tokio::test]
async fn a_partial_query_is_completed_to_the_places_its_words_name() {
    let emulator = Emulator::new();
    let rows = [
        PrefixRow {
            case: "half-typed-last-word",
            query: "Space Need",
            expected_title: "Space Needle",
        },
        PrefixRow {
            case: "three-words-last-one-partial",
            query: "Pike Place Mar",
            expected_title: "Pike Place Market",
        },
        PrefixRow {
            case: "alternate-name",
            query: "Eiffel Tow",
            expected_title: "Tour Eiffel",
        },
        PrefixRow {
            case: "long-name-partial-last-word",
            query: "Golden Gate Brid",
            expected_title: "Golden Gate Bridge",
        },
        PrefixRow {
            case: "name-with-a-short-word",
            query: "Statue of Lib",
            expected_title: "Statue of Liberty",
        },
        PrefixRow {
            case: "name-in-another-language",
            query: "Musee du Lou",
            expected_title: "Musee du Louvre",
        },
        PrefixRow {
            case: "street-and-number",
            query: "400 Broad",
            expected_title: "400 Broad St, Seattle, WA 98109, United States",
        },
        PrefixRow {
            case: "postal-code",
            query: "20500",
            expected_title: "White House",
        },
    ];

    for row in rows {
        let body = emulator
            .post_ok(
                row.case,
                AUTOCOMPLETE,
                // The whole page, so a row asserts WHICH places are completed
                // rather than how the default page size truncates them.
                &json!({"QueryText": row.query, "MaxResults": MAX_RESULTS_MAX}),
            )
            .await;
        let found = titles(&body);
        assert!(
            found.iter().any(|title| title == row.expected_title),
            "case {}: expected {:?} among the completions of {:?}, got {found:?}",
            row.case,
            row.expected_title,
            row.query
        );
        assert!(
            found.len() <= MAX_RESULTS_MAX as usize,
            "case {}: expected at most {MAX_RESULTS_MAX} completions, got {}",
            row.case,
            found.len()
        );
        for item in items(&body) {
            let title = text(&item, "Title");
            let counted = assert_sections_mark_the_response(row.case, &item);
            assert_sections_were_typed(row.case, &item, row.query);
            assert!(
                counted > 0,
                "case {}: expected the completion {title:?} to mark the sections the query \
                 {:?} named, got {:?}",
                row.case,
                row.query,
                item.get("Highlights")
            );
        }
    }

    // Text that names nothing in the catalog completes to NO place: an empty
    // `ResultItems` (modeled `len 0..20`), never an invented completion. The last
    // row is a CATEGORY phrase: Autocomplete completes the text a place reports,
    // while it is Suggest that is documented to answer a "search category", so a
    // category word no reported text spells completes to nothing.
    for query in ["Qzzxyw", "Zzzzville", "Nowhere Such Place", "Coffee Sho"] {
        let body = emulator
            .post_ok(query, AUTOCOMPLETE, &json!({"QueryText": query}))
            .await;
        assert!(
            items(&body).is_empty(),
            "case {query}: expected no completions, got {:?}",
            titles(&body)
        );
    }
}

/// A completion names a place of the catalog: its `PlaceId` resolves through
/// GetPlace to the same `Title` and `Address.Label` the completion reported.
#[tokio::test]
async fn a_completion_resolves_through_get_place_to_the_same_place() {
    let emulator = Emulator::new();
    for query in ["Space Need", "400 Broad", "Pari", "Starbuck"] {
        let body = emulator
            .post_ok(query, AUTOCOMPLETE, &json!({"QueryText": query}))
            .await;
        assert!(
            !items(&body).is_empty(),
            "case {query}: expected at least one completion to read back"
        );
        for item in items(&body) {
            let place_id = text(&item, "PlaceId");
            let detail = emulator
                .get_ok(query, &format!("/v2/place/{place_id}"))
                .await;
            assert_eq!(
                text(&detail, "Title"),
                text(&item, "Title"),
                "case {query}: expected the completion {place_id} to resolve to its own title"
            );
            let completed = item
                .get("Address")
                .and_then(|address| address.get("Label"))
                .cloned()
                .unwrap_or(Value::Null);
            let resolved = detail
                .get("Address")
                .and_then(|address| address.get("Label"))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                completed, resolved,
                "case {query}: expected the completion {place_id} to resolve to its own label"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The sections a completion marks
// ---------------------------------------------------------------------------

/// One expected section of a response's text: `(StartIndex, EndIndex, Value)`.
type ExpectedSection = (i64, i64, &'static str);

/// One expected highlighted address member, with the sections it must carry.
type ExpectedMember = (&'static str, &'static [ExpectedSection]);

/// A row of the highlight table: what the user typed, the completion it inspects,
/// and the sections of that completion's own text the service must mark.
struct HighlightRow {
    case: &'static str,
    query: &'static str,
    /// The completion this row inspects, by its reported `Title`.
    title: &'static str,
    /// `Highlights.Title`, as `(StartIndex, EndIndex, Value)`.
    title_sections: &'static [ExpectedSection],
    /// `Highlights.Address`: each member the service must mark, with its sections.
    /// A member absent from this list must be absent from the response.
    address_sections: &'static [ExpectedMember],
}

/// `Highlights` "describes how the parts of the response element matched the input
/// query by returning the sections of the response which matched to input query
/// terms": each section is a `[StartIndex, EndIndex)` CHAR span of the member it
/// belongs to — of the RESPONSE's text, not of the query — spelling the characters
/// a typed word accounted for, and two sections with no unmatched word between them
/// are marked as one.
///
/// The rows differ along where in the response's text the typed words sit (at its
/// start, in the middle of it), how many words they account for, and which members
/// carry them (the title alone, the title and the label, the address components) —
/// which is what the highlighting rule generalises over. The "alternate-name" row
/// is the one that separates a section of the RESPONSE from a section of the query:
/// "Eiffel" is typed first and reported second.
#[tokio::test]
async fn a_completion_marks_the_sections_of_its_own_text_the_query_named() {
    let emulator = Emulator::new();
    let rows = [
        HighlightRow {
            case: "name-at-the-start",
            query: "Space Need",
            title: "Space Needle",
            title_sections: &[(0, 10, "Space Need")],
            // The assembled label leads with the place's own name, so the same
            // characters are a section of it.
            address_sections: &[("Label", &[(0, 10, "Space Need")])],
        },
        HighlightRow {
            case: "alternate-name",
            query: "Eiffel Tow",
            title: "Tour Eiffel",
            title_sections: &[(5, 11, "Eiffel")],
            address_sections: &[("Label", &[(5, 11, "Eiffel")])],
        },
        HighlightRow {
            case: "street-and-number",
            query: "400 Broad",
            title: "400 Broad St, Seattle, WA 98109, United States",
            title_sections: &[(0, 9, "400 Broad")],
            address_sections: &[
                ("Label", &[(0, 9, "400 Broad")]),
                ("Street", &[(0, 5, "Broad")]),
                ("AddressNumber", &[(0, 3, "400")]),
            ],
        },
        HighlightRow {
            case: "postal-code",
            query: "98109",
            title: "Space Needle",
            // The query names no word of the title, so the title carries no
            // section at all — an absent member, not an empty list.
            title_sections: &[],
            address_sections: &[
                ("Label", &[(40, 45, "98109")]),
                ("PostalCode", &[(0, 5, "98109")]),
            ],
        },
        HighlightRow {
            case: "country-name",
            query: "Fran",
            title: "France",
            title_sections: &[(0, 4, "Fran")],
            address_sections: &[
                ("Label", &[(0, 4, "Fran")]),
                ("Country.Name", &[(0, 4, "Fran")]),
            ],
        },
    ];

    for row in rows {
        let body = emulator
            .post_ok(
                row.case,
                AUTOCOMPLETE,
                &json!({"QueryText": row.query, "MaxResults": MAX_RESULTS_MAX}),
            )
            .await;
        let item = items(&body)
            .into_iter()
            .find(|item| text(item, "Title") == row.title)
            .unwrap_or_else(|| {
                panic!(
                    "case {}: expected {:?} among the completions of {:?}, got {:?}",
                    row.case,
                    row.title,
                    row.query,
                    titles(&body)
                )
            });
        assert_sections_mark_the_response(row.case, &item);
        assert_sections_were_typed(row.case, &item, row.query);

        let highlights = item.get("Highlights").cloned().unwrap_or(Value::Null);
        let expected_title: Vec<Section> = row
            .title_sections
            .iter()
            .map(|(start, end, value)| (*start, *end, (*value).to_string()))
            .collect();
        assert_eq!(
            sections(highlights.get("Title")),
            expected_title,
            "case {}: Highlights.Title of {:?}",
            row.case,
            row.title
        );

        let marked = highlighted(&item);
        for (member, expected) in row.address_sections {
            let path = format!("Address.{member}");
            let found = marked
                .iter()
                .find(|(marked, _, _)| *marked == path)
                .map(|(_, _, sections)| sections.clone())
                .unwrap_or_default();
            let expected: Vec<Section> = expected
                .iter()
                .map(|(start, end, value)| (*start, *end, (*value).to_string()))
                .collect();
            assert_eq!(
                found, expected,
                "case {}: Highlights.{path} of {:?}",
                row.case, row.title
            );
        }
        // Nothing beyond the listed members is marked: a member the query named
        // nothing of is absent.
        let found_members: Vec<String> = marked
            .iter()
            .map(|(member, _, _)| member.clone())
            .filter(|member| member != "Title")
            .collect();
        let expected_members: Vec<String> = row
            .address_sections
            .iter()
            .map(|(member, _)| format!("Address.{member}"))
            .collect();
        assert_eq!(
            found_members, expected_members,
            "case {}: the address members marked for {:?}",
            row.case, row.title
        );
    }
}

// ---------------------------------------------------------------------------
// The language a completion is rendered in
// ---------------------------------------------------------------------------

/// "A list of BCP 47 compliant language codes for the results to be rendered in.
/// If there is no data for the result in the requested language, data will be
/// returned in the default language for the entry": a completion is rendered in the
/// language asked for where the entry holds text in it, reports THAT language, and
/// marks the sections of the text it rendered — so the same place, found by the name
/// one language gives it, marks nothing when the response spells it in another.
#[tokio::test]
async fn a_completion_is_rendered_in_the_language_asked_for_and_marks_that_text() {
    let emulator = Emulator::new();
    // A catalog entry whose own language is not English and which is also held in
    // English, so the two renderings differ.
    const TYPED: &str = "Simfero";

    let rows = [
        ("asked-for-english", Some("en"), "en", true),
        ("the-entry-s-own-language", None, "uk", false),
    ];
    for (case, asked, expected_language, spells_what_was_typed) in rows {
        let mut body = json!({"QueryText": TYPED, "MaxResults": MAX_RESULTS_MAX});
        if let Some(language) = asked {
            body["Language"] = json!(language);
        }
        let completions = emulator.post_ok(case, AUTOCOMPLETE, &body).await;
        let item = items(&completions)
            .into_iter()
            .find(|item| text(item, "Language") == expected_language)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected a completion rendered in {expected_language:?}, got {:?}",
                    items(&completions)
                )
            });
        let title = text(&item, "Title");
        assert_sections_mark_the_response(case, &item);
        let marked = sections(
            item.get("Highlights")
                .and_then(|highlights| highlights.get("Title")),
        );
        if spells_what_was_typed {
            assert!(
                title.to_lowercase().starts_with(&TYPED.to_lowercase()),
                "case {case}: expected the rendered title to spell what was typed, got {title:?}"
            );
            assert_eq!(
                marked,
                vec![(0, TYPED.chars().count() as i64, TYPED.to_string())],
                "case {case}: Highlights.Title of {title:?}"
            );
        } else {
            assert!(
                !title.to_lowercase().contains(&TYPED.to_lowercase()),
                "case {case}: expected the title rendered in the entry's own language not to \
                 spell what was typed, got {title:?}"
            );
            assert!(
                marked.is_empty(),
                "case {case}: expected no title section in a rendering that does not spell what \
                 was typed, got {marked:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// How many completions one call reports
// ---------------------------------------------------------------------------

/// A row of the page-size table: the `MaxResults` asked for, and how many
/// completions the call must report.
struct PageRow {
    case: &'static str,
    max_results: Option<i64>,
    expected: usize,
}

/// "An optional limit for the number of results returned in a single call. Default
/// value: 5" (`AutocompleteRequest$MaxResults`): a call reports at most the limit it
/// asked for, and at most the documented default where it asked for none.
///
/// The rows differ along the limit asked for — none, each published bound, and
/// sizes between them — against a query that matches MORE places than any of them,
/// so each row's count is the limit rather than the size of the catalog's answer.
#[tokio::test]
async fn max_results_bounds_the_number_of_completions() {
    let emulator = Emulator::new();
    // A query the catalog completes to more places than the largest limit below,
    // so every row is bounded by its limit and not by the matches.
    const QUERY: &str = "Seattle";

    let whole_page = emulator
        .post_ok(
            "the matches",
            AUTOCOMPLETE,
            &json!({"QueryText": QUERY, "MaxResults": MAX_RESULTS_MAX}),
        )
        .await;
    let matches = items(&whole_page).len();
    assert!(
        matches > DEFAULT_MAX_RESULTS,
        "case the matches: expected {QUERY:?} to match more than {DEFAULT_MAX_RESULTS} places so \
         the limits below bind, got {matches}"
    );
    assert!(
        matches <= MAX_RESULTS_MAX as usize,
        "case the matches: expected at most {MAX_RESULTS_MAX} completions, got {matches}"
    );

    let rows = [
        PageRow {
            case: "no-limit-takes-the-documented-default",
            max_results: None,
            expected: DEFAULT_MAX_RESULTS,
        },
        PageRow {
            case: "lowest-published-limit",
            max_results: Some(MAX_RESULTS_MIN),
            expected: MAX_RESULTS_MIN as usize,
        },
        PageRow {
            case: "below-the-default",
            max_results: Some(3),
            expected: 3,
        },
        PageRow {
            case: "above-the-default",
            max_results: Some(7),
            expected: 7.min(matches),
        },
        PageRow {
            case: "highest-published-limit",
            max_results: Some(MAX_RESULTS_MAX),
            expected: matches,
        },
    ];

    for row in rows {
        let mut body = json!({"QueryText": QUERY});
        if let Some(limit) = row.max_results {
            body["MaxResults"] = json!(limit);
        }
        let call = emulator.post(AUTOCOMPLETE, &body).await;
        assert_success(row.case, &call);
        assert_eq!(
            call.header(PRICING_BUCKET_HEADER),
            PRICING_BUCKET_CORE,
            "case {}: the pricing tier the call is charged in",
            row.case
        );
        let found = titles(&call.json());
        assert_eq!(
            found.len(),
            row.expected,
            "case {}: expected {} completions for MaxResults {:?}, got {found:?}",
            row.case,
            row.expected,
            row.max_results
        );
    }

    // One past each published bound is refused, not clamped: `MaxResults` is
    // modeled `range 1..20`.
    for (case, limit) in [
        ("below-the-lowest-limit", MAX_RESULTS_MIN - 1),
        ("above-the-highest-limit", MAX_RESULTS_MAX + 1),
    ] {
        let call = emulator
            .post(AUTOCOMPLETE, &json!({"QueryText": QUERY, "MaxResults": limit}))
            .await;
        assert_eq!(
            call.status, 400,
            "case {case}: expected MaxResults {limit} to be refused, got {} with {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case}: the error code refusing MaxResults {limit}"
        );
    }
}

/// `QueryText` is `@required` and modeled `len 1..200`, so a request that leaves
/// it out, empties it or types past the ceiling is refused; one that satisfies that
/// length while spelling no word at all names no term to complete, so it completes
/// to nothing instead.
#[tokio::test]
async fn a_query_that_types_no_term_is_refused_or_completes_to_nothing() {
    let emulator = Emulator::new();
    let refused = [
        ("empty", json!({"QueryText": ""})),
        ("absent", json!({})),
        (
            "longer-than-the-published-ceiling",
            json!({"QueryText": "a".repeat(201)}),
        ),
    ];
    for (case, body) in refused {
        let call = emulator.post(AUTOCOMPLETE, &body).await;
        assert_eq!(
            call.status, 400,
            "case {case}: expected the query to be refused, got {} with {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case}: the error code refusing the query"
        );
    }

    for (case, query) in [("blank", "   "), ("separators-only", ", -")] {
        let body = emulator
            .post_ok(case, AUTOCOMPLETE, &json!({"QueryText": query}))
            .await;
        assert!(
            items(&body).is_empty(),
            "case {case}: expected no completions for a query naming no term, got {:?}",
            titles(&body)
        );
    }
}

/// "The fields BiasPosition, FilterBoundingBox, and FilterCircle are mutually
/// exclusive" (`AutocompleteRequest$BiasPosition`): they are alternative ways to say
/// WHERE, so a request setting two of them is refused, while each one alone — and
/// none of them — is answered.
#[tokio::test]
async fn the_ways_of_saying_where_are_mutually_exclusive() {
    let emulator = Emulator::new();
    // The Space Needle's own neighbourhood, and a box and a circle around it.
    let bias = json!([-122.349_358, 47.620_506]);
    let box_ = json!([-122.36, 47.60, -122.33, 47.63]);
    let circle = json!({"Center": [-122.349_358, 47.620_506], "Radius": 2_000});

    let accepted = [
        ("none", json!({"QueryText": "Space Need"})),
        (
            "bias-position",
            json!({"QueryText": "Space Need", "BiasPosition": bias}),
        ),
        (
            "bounding-box",
            json!({"QueryText": "Space Need", "Filter": {"BoundingBox": box_}}),
        ),
        (
            "circle",
            json!({"QueryText": "Space Need", "Filter": {"Circle": circle}}),
        ),
    ];
    for (case, body) in accepted {
        let found = titles(&emulator.post_ok(case, AUTOCOMPLETE, &body).await);
        assert!(
            found.iter().any(|title| title == "Space Needle"),
            "case {case}: expected the completion to survive the area asked for, got {found:?}"
        );
    }

    let refused = [
        (
            "bias-and-box",
            json!({"QueryText": "Space Need", "BiasPosition": bias, "Filter": {"BoundingBox": box_}}),
        ),
        (
            "bias-and-circle",
            json!({"QueryText": "Space Need", "BiasPosition": bias, "Filter": {"Circle": circle}}),
        ),
        (
            "box-and-circle",
            json!({"QueryText": "Space Need", "Filter": {"BoundingBox": box_, "Circle": circle}}),
        ),
    ];
    for (case, body) in refused {
        let call = emulator.post(AUTOCOMPLETE, &body).await;
        assert_eq!(
            call.status, 400,
            "case {case}: expected two ways of saying where to be refused, got {} with {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {case}: the error code refusing the request"
        );
    }
}

/// `Filter.IncludeCountries` and `Filter.IncludePlaceTypes` keep only the
/// completions that possess the property asked for: "a structure which contains a
/// set of inclusion/exclusion properties that results must possess in order to be
/// returned as a result" (`AutocompleteRequest$Filter`).
#[tokio::test]
async fn a_filter_keeps_only_the_completions_that_possess_what_it_asks_for() {
    let emulator = Emulator::new();
    // "Pari" completes to Paris, France and to its homonym in Texas, so a country
    // filter has two countries to choose between.
    let rows = [
        (
            "no-filter",
            json!({"QueryText": "Pari", "MaxResults": MAX_RESULTS_MAX}),
            vec!["FR", "US"],
        ),
        (
            "one-country",
            json!({"QueryText": "Pari", "MaxResults": MAX_RESULTS_MAX, "Filter": {"IncludeCountries": ["FRA"]}}),
            vec!["FR"],
        ),
        (
            "the-other-country",
            json!({"QueryText": "Pari", "MaxResults": MAX_RESULTS_MAX, "Filter": {"IncludeCountries": ["USA"]}}),
            vec!["US"],
        ),
    ];
    for (case, body, expected) in rows {
        let body = emulator.post_ok(case, AUTOCOMPLETE, &body).await;
        let found: Vec<String> = items(&body)
            .iter()
            .map(|item| {
                item.get("Address")
                    .and_then(|address| address.get("Country"))
                    .map(|country| text(country, "Code2"))
                    .unwrap_or_default()
            })
            .collect();
        assert!(
            !found.is_empty(),
            "case {case}: expected at least one completion to filter"
        );
        for code in &found {
            assert!(
                expected.contains(&code.as_str()),
                "case {case}: expected every completion in {expected:?}, got {found:?}"
            );
        }
    }

    // `IncludePlaceTypes` is one of the eight `AutocompleteFilterPlaceType` values,
    // tested on a type the completion's own place type is NOT, so the filter is
    // what decides.
    let locality_only = emulator
        .post_ok(
            "place-type-locality",
            AUTOCOMPLETE,
            &json!({
                "QueryText": "Pari",
                "MaxResults": MAX_RESULTS_MAX,
                "Filter": {"IncludePlaceTypes": ["Locality"]},
            }),
        )
        .await;
    let found: Vec<String> = items(&locality_only)
        .iter()
        .map(|item| text(item, "PlaceType"))
        .collect();
    assert!(
        !found.is_empty() && found.iter().all(|place_type| place_type == "Locality"),
        "case place-type-locality: expected only localities, got {found:?}"
    );
}
