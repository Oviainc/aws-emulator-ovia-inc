// SPDX-License-Identifier: CC0-1.0

//! What a result ITEM reports about a place's IDENTITY and its GEOMETRY: the
//! opaque handle every operation hands out, the box a place covers, the precision
//! every coordinate is reported at, and the values the place data itself holds.
//!
//! Each test is a TABLE spanning the class its rule generalises over — every
//! record the catalog holds, every operation that projects one, every granularity
//! from a point of interest up to a country — rather than one recorded request.
//! The emulator is driven as real HTTP through the same router the front door
//! serves, reusing the harness in [`crate::places_catalog`].

use serde_json::{json, Value};

use crate::places_catalog::{items, position, text, Emulator, Place};
use crate::result_item_composition::address;

const GEOCODE: &str = "/v2/geocode";
const REVERSE_GEOCODE: &str = "/v2/reverse-geocode";
const SEARCH_TEXT: &str = "/v2/search-text";
const SEARCH_NEARBY: &str = "/v2/search-nearby";
const AUTOCOMPLETE: &str = "/v2/autocomplete";
const SUGGEST: &str = "/v2/suggest";
const PLACE: &str = "/v2/place";

/// The fixed leading characters of every place handle the service hands out: the
/// version prefix of the provider's opaque token, which a client that buckets or
/// validates a stored handle sees on every id whatever place it names.
const HANDLE_VERSION_PREFIX: &str = "AQAAADgA";

/// Characters a place handle is spelled with: the base64url alphabet, which is
/// the standard one with `-` and `_` as its last two characters and no padding.
const HANDLE_ALPHABET: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Decimal places a reported WGS 84 coordinate carries. Every `Position`,
/// `MapView` and `AccessPoints` coordinate is reported at this precision.
const COORDINATE_DECIMALS: i32 = 5;

/// The modeled `PlaceId` bound: `SensitiveString` `length(min 0, max 500)` on
/// every result item that carries one (`smithy_query` query=shape
/// shape=GeocodeResultItem).
const PLACE_ID_MAX: usize = 500;

/// `PlaceType` values of the places that cover real AREA, as the model's
/// `PlaceType` enum names them: the levels whose `MapView` is the feature's own
/// bounding box rather than the viewport of a point.
const AREAL_PLACE_TYPES: [&str; 5] = ["District", "Locality", "SubRegion", "Region", "Country"];

/// `MapView`, as `[west, south, east, north]`.
fn map_view(value: &Value) -> Vec<f64> {
    value
        .get("MapView")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// Every coordinate anywhere inside a response: the members the model binds to
/// `Position`, `BoundingBox` and an access point's position.
fn coordinates(value: &Value, path: &str, found: &mut Vec<(String, f64)>) {
    match value {
        Value::Object(members) => {
            for (member, nested) in members {
                coordinates(nested, &format!("{path}.{member}"), found);
            }
        }
        Value::Array(values) => {
            let geometry = path.ends_with("Position") || path.ends_with("MapView");
            for (index, nested) in values.iter().enumerate() {
                match nested.as_f64() {
                    Some(degrees) if geometry => {
                        found.push((format!("{path}[{index}]"), degrees));
                    }
                    _ => coordinates(nested, &format!("{path}[{index}]"), found),
                }
            }
        }
        _ => {}
    }
}

/// `value` at the precision a coordinate is reported to.
fn rounded(value: f64) -> f64 {
    let scale = 10_f64.powi(COORDINATE_DECIMALS);
    (value * scale).round() / scale
}

/// Is `position` inside `[west, south, east, north]`?
fn box_contains(box_: &[f64], position: &[f64]) -> bool {
    box_.len() == 4
        && position.len() == 2
        && position[0] >= box_[0]
        && position[0] <= box_[2]
        && position[1] >= box_[1]
        && position[1] <= box_[3]
}

/// Every response that projects a result item for `place`, named by the operation
/// that produced it: the whole set of projections a handle or a coordinate can
/// reach a client through.
async fn projections(emulator: &Emulator, place: &Place) -> Vec<(String, Value)> {
    let at = json!([place.position[0], place.position[1]]);
    let mut found = vec![(
        "GetPlace".to_string(),
        emulator
            .get_ok(
                &place.name,
                &format!("{PLACE}/{}?AdditionalFeatures=Access", place.place_id),
            )
            .await,
    )];
    for (operation, path, body) in [
        (
            "Geocode",
            GEOCODE,
            json!({"QueryText": place.title, "MaxResults": 100}),
        ),
        (
            "ReverseGeocode",
            REVERSE_GEOCODE,
            json!({"QueryPosition": at, "MaxResults": 100, "AdditionalFeatures": ["Access"]}),
        ),
        (
            "SearchText",
            SEARCH_TEXT,
            json!({"QueryText": place.title, "MaxResults": 100}),
        ),
        (
            "SearchNearby",
            SEARCH_NEARBY,
            json!({"QueryPosition": at, "QueryRadius": 1_000, "MaxResults": 100}),
        ),
        (
            "Autocomplete",
            AUTOCOMPLETE,
            json!({"QueryText": place.title, "MaxResults": 20}),
        ),
        (
            "Suggest",
            SUGGEST,
            json!({"QueryText": place.title, "MaxResults": 20}),
        ),
    ] {
        let case = format!("{}-{operation}", place.name);
        found.push((
            operation.to_string(),
            emulator.post_ok(&case, path, &body).await,
        ));
    }
    found
}

/// Every `PlaceId` a response hands out, wherever the item carries it: a suggestion
/// nests its place under `Place`, every other operation reports it on the item.
fn handles(body: &Value) -> Vec<String> {
    items(body)
        .iter()
        .map(|item| match item.get("Place") {
            Some(place) => text(place, "PlaceId"),
            None => text(item, "PlaceId"),
        })
        .filter(|handle| !handle.is_empty())
        .collect()
}

// ---------------------------------------------------------------------------
// The place handle
// ---------------------------------------------------------------------------

/// A `PlaceId` is the provider's OPAQUE handle: one token class — the service's
/// version prefix, the base64url alphabet and a single length well inside the
/// modeled `length(0, 500)` — whatever place it names and whichever operation
/// hands it out, and it resolves back through GetPlace to the place it came from.
///
/// The rows are every record the catalog holds, which spans the granularities
/// (point of interest and point address up to country), the countries and the
/// scripts the catalog is written in, so an id derived from the record's own text
/// is exercised on every shape of record.
#[tokio::test]
async fn every_place_handle_is_one_opaque_token_class() {
    let emulator = Emulator::new();
    let catalog = emulator.catalog();
    assert!(
        catalog.len() > 20,
        "case catalog: the handle table spans the whole catalog, got {} places",
        catalog.len()
    );
    let mut lengths: Vec<usize> = Vec::new();
    for place in &catalog {
        let handle = place.place_id.clone();
        assert!(
            handle.starts_with(HANDLE_VERSION_PREFIX),
            "case {}: a handle opens with the service's version prefix \
             {HANDLE_VERSION_PREFIX:?}, got {handle:?}",
            place.name
        );
        assert!(
            handle.chars().all(|character| HANDLE_ALPHABET.contains(character)),
            "case {}: a handle is spelled in the base64url alphabet, got {handle:?}",
            place.name
        );
        assert!(
            handle.len() <= PLACE_ID_MAX,
            "case {}: a handle stays inside the modeled PlaceId bound of \
             {PLACE_ID_MAX}, got {} characters",
            place.name,
            handle.len()
        );
        lengths.push(handle.len());
        // The handle resolves: it is what a client hands back to GetPlace.
        let read = emulator
            .get_ok(&place.name, &format!("{PLACE}/{handle}"))
            .await;
        assert_eq!(
            text(&read, "PlaceId"),
            handle,
            "case {}: GetPlace must resolve the handle it hands out, got {:?}",
            place.name,
            text(&read, "PlaceId")
        );
    }
    lengths.dedup();
    assert_eq!(
        lengths.len(),
        1,
        "case length-class: every handle belongs to one length class, got {lengths:?}"
    );
    assert!(
        lengths[0] > HANDLE_VERSION_PREFIX.len() + 64,
        "case length-class: a handle carries a per-place payload behind its prefix, \
         got {} characters",
        lengths[0]
    );
}

/// A handle is STABLE and UNIQUE: two emulators built from the same catalog hand
/// out the same handle for the same place, and no two places share one — what a
/// client that persists, logs and de-duplicates on the handle depends on.
#[tokio::test]
async fn a_place_handle_is_stable_across_emulators_and_unique_within_one() {
    let first = Emulator::new();
    let second = Emulator::new();
    let mut seen: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let reread = second.catalog();
    let catalog = first.catalog();
    assert_eq!(
        catalog.len(),
        reread.len(),
        "case catalog: the seeded catalog is deterministic, got {} and {} places",
        catalog.len(),
        reread.len()
    );
    // The catalog is a deterministic sequence, so the same place sits at the same
    // index of both emulators' copies — which is what lets two records of one
    // brand at different addresses be told apart here.
    for (place, same) in catalog.iter().zip(reread.iter()) {
        assert_eq!(
            (&same.name, &same.place_type, same.position),
            (&place.name, &place.place_type, place.position),
            "case {}: the catalog is seeded in one order",
            place.name
        );
        assert_eq!(
            same.place_id, place.place_id,
            "case {} at {:?}: the same place has the same handle in every emulator, \
             got {:?} and {:?}",
            place.name, place.position, place.place_id, same.place_id
        );
        if let Some(previous) = seen.insert(place.place_id.clone(), place.name.clone()) {
            panic!(
                "case {}: a handle names one place, but {previous:?} carries it too",
                place.name
            );
        }
    }
}

/// Every operation that projects a result item reports the SAME handle for a
/// place, so an id taken from one call resolves in another.
///
/// The rows are a place of each granularity the catalog holds, each read through
/// every projecting operation.
#[tokio::test]
async fn every_operation_reports_the_same_handle_for_a_place() {
    let emulator = Emulator::new();
    let catalog = emulator.catalog();
    // One place per `PlaceType` the catalog holds: the handle must not depend on
    // the granularity of the record it names.
    let mut by_type: std::collections::BTreeMap<String, Place> = std::collections::BTreeMap::new();
    for place in catalog {
        by_type.entry(place.place_type.clone()).or_insert(place);
    }
    assert!(
        by_type.len() >= 5,
        "case granularities: expected a place at several granularities, got {:?}",
        by_type.keys().collect::<Vec<_>>()
    );
    for place in by_type.values() {
        for (operation, body) in projections(&emulator, place).await {
            let reported = handles(&body);
            let expected = if operation == "GetPlace" {
                vec![text(&body, "PlaceId")]
            } else {
                reported.clone()
            };
            assert!(
                expected.contains(&place.place_id),
                "case {}-{operation}: expected the place's own handle {:?} among the \
                 handles reported, got {expected:?}",
                place.name,
                place.place_id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The extent a place covers, and the precision a coordinate is reported at
// ---------------------------------------------------------------------------

/// `MapView` is "the bounding box enclosing the geometric shape (area or line)
/// that an individual result covers", ordered `[{westward lng}, {southern lat},
/// {eastward lng}, {northern lat}]`: it is well-ordered and it ENCLOSES the
/// place's own `Position`.
///
/// The rows are every record the catalog holds, read through GetPlace, so the rule
/// is asserted at every granularity rather than on one result.
#[tokio::test]
async fn a_place_extent_is_well_ordered_and_encloses_the_place() {
    let emulator = Emulator::new();
    for place in emulator.catalog() {
        let read = emulator
            .get_ok(&place.name, &format!("{PLACE}/{}", place.place_id))
            .await;
        let view = map_view(&read);
        let at = position(&read);
        assert_eq!(
            view.len(),
            4,
            "case {}: MapView is four coordinates, got {view:?}",
            place.name
        );
        assert!(
            view[0] < view[2] && view[1] < view[3],
            "case {}: expected [west, south, east, north] with west of east and \
             south of north, got {view:?}",
            place.name
        );
        assert!(
            box_contains(&view, &at),
            "case {}: a place's own extent encloses its position, {view:?} does not \
             contain {at:?}",
            place.name
        );
    }
}

/// A place's extent is the geometry of THAT place, not a function of its
/// `PlaceType`: two places of one type cover differently sized areas, and an area
/// is not centred on its own position the way a point's viewport is.
///
/// The rows are each `PlaceType` the catalog holds area records at, from a city
/// district up to a country.
#[tokio::test]
async fn an_area_carries_its_own_extent_rather_than_one_derived_from_its_type() {
    let emulator = Emulator::new();
    let mut spans: std::collections::BTreeMap<String, Vec<(String, [f64; 2])>> =
        std::collections::BTreeMap::new();
    for place in emulator.catalog() {
        if !AREAL_PLACE_TYPES.contains(&place.place_type.as_str()) {
            continue;
        }
        let read = emulator
            .get_ok(&place.name, &format!("{PLACE}/{}", place.place_id))
            .await;
        let view = map_view(&read);
        let at = position(&read);
        assert_eq!(view.len(), 4, "case {}: expected a MapView", place.name);
        // An area's box encloses the feature, which no point of the feature is the
        // centre of: a box centred on the position is a box computed FROM it.
        let offset = [
            ((view[0] + view[2]) / 2.0 - at[0]).abs(),
            ((view[1] + view[3]) / 2.0 - at[1]).abs(),
        ];
        assert!(
            offset[0] > 1e-4 || offset[1] > 1e-4,
            "case {}: an area's extent is its own geometry, so it is not centred on \
             its position; got MapView {view:?} around {at:?}",
            place.name
        );
        spans
            .entry(place.place_type.clone())
            .or_default()
            .push((place.name.clone(), [view[2] - view[0], view[3] - view[1]]));
    }
    assert_eq!(
        spans.len(),
        AREAL_PLACE_TYPES.len(),
        "case types: expected a record at every area granularity {AREAL_PLACE_TYPES:?}, \
         got {:?}",
        spans.keys().collect::<Vec<_>>()
    );
    for (place_type, areas) in &spans {
        assert!(
            areas.len() > 1,
            "case {place_type}: the catalog holds several places of this type, got \
             {areas:?}"
        );
        let distinct = areas
            .iter()
            .map(|(_, span)| format!("{:.5}x{:.5}", span[0], span[1]))
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            distinct.len() > 1,
            "case {place_type}: places of one type cover differently sized areas, so \
             the extent cannot be keyed to the type; got {areas:?}"
        );
    }
}

/// Every coordinate the service reports — `Position`, `MapView` and an access
/// point's position — is reported at the service's coordinate precision, five
/// fractional degrees, and not at the full precision of the arithmetic behind it.
///
/// The rows are a place of each granularity read through EVERY projecting
/// operation, so a coordinate escaping on any member of any response fails.
#[tokio::test]
async fn every_reported_coordinate_carries_the_services_coordinate_precision() {
    let emulator = Emulator::new();
    let mut by_type: std::collections::BTreeMap<String, Place> = std::collections::BTreeMap::new();
    for place in emulator.catalog() {
        by_type.entry(place.place_type.clone()).or_insert(place);
    }
    let mut checked = 0_usize;
    for place in by_type.values() {
        for (operation, body) in projections(&emulator, place).await {
            let mut found: Vec<(String, f64)> = Vec::new();
            coordinates(&body, "$", &mut found);
            for (path, degrees) in &found {
                // A coordinate reported to five decimals is unchanged by rounding
                // to five decimals; the tolerance is far finer than the sixth
                // decimal a coarser value would carry.
                assert!(
                    (*degrees - rounded(*degrees)).abs() <= 1e-9,
                    "case {}-{operation}: {path} must be reported to \
                     {COORDINATE_DECIMALS} decimals, got {degrees}",
                    place.name
                );
            }
            checked += found.len();
        }
    }
    assert!(
        checked > 100,
        "case coverage: expected the table to reach many coordinates, got {checked}"
    );
}

// ---------------------------------------------------------------------------
// The values the place data holds
// ---------------------------------------------------------------------------

/// The data a result reports is the place data's OWN: a store of a business chain
/// is titled with the brand name as that data writes it — the chain's upper-case
/// brand form — while an independent business keeps the name it is written under,
/// and a street address carries the postal code held for its delivery segment,
/// which in the United States is a five-digit ZIP or a ZIP+4.
///
/// The rows differ along what the query names (a brand, a category, a street) and
/// along the operation, so the rule is asserted over every place those queries
/// reach rather than over one result.
#[tokio::test]
async fn place_data_reports_the_values_the_catalog_holds_for_a_place() {
    /// A US postal code: five digits, with the ZIP+4 add-on where the data holds
    /// one.
    fn is_us_postal_code(code: &str) -> bool {
        let (zip, add_on) = match code.split_once('-') {
            Some((zip, add_on)) => (zip, Some(add_on)),
            None => (code, None),
        };
        zip.len() == 5
            && zip.chars().all(char::is_numeric)
            && add_on.is_none_or(|add_on| add_on.len() == 4 && add_on.chars().all(char::is_numeric))
    }

    let emulator = Emulator::new();
    let mut chain_titles = 0_usize;
    let mut independent_titles = 0_usize;
    let mut zip_plus_four = 0_usize;
    for (case, path, body) in [
        (
            "brand",
            SEARCH_TEXT,
            json!({"QueryText": "Starbucks", "MaxResults": 100}),
        ),
        (
            "category",
            SEARCH_TEXT,
            json!({"QueryText": "Coffee", "MaxResults": 100}),
        ),
        (
            "street",
            GEOCODE,
            json!({"QueryText": "Mercer St Seattle", "MaxResults": 100}),
        ),
        (
            "nearby",
            SEARCH_NEARBY,
            json!({
                "QueryPosition": [-122.349_358, 47.620_506],
                "QueryRadius": 3_000,
                "MaxResults": 100,
            }),
        ),
    ] {
        let response = emulator.post_ok(case, path, &body).await;
        let returned = items(&response);
        assert!(
            !returned.is_empty(),
            "case {case}: the catalog answers this query, got no items"
        );
        for item in returned {
            let title = text(&item, "Title");
            let chains = item
                .get("BusinessChains")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if chains.is_empty() {
                if title != title.to_uppercase() {
                    independent_titles += 1;
                }
            } else {
                assert_eq!(
                    title,
                    title.to_uppercase(),
                    "case {case}: a chain store is titled with the brand form the place \
                     data holds, got {title:?}"
                );
                chain_titles += 1;
            }

            let address = address(&item);
            let postal_code = text(&address, "PostalCode");
            let country = address
                .get("Country")
                .map(|country| text(country, "Code2"))
                .unwrap_or_default();
            if country == "US" && !postal_code.is_empty() {
                assert!(
                    is_us_postal_code(&postal_code),
                    "case {case}: expected a five-digit ZIP or a ZIP+4 for {title:?}, \
                     got {postal_code:?}"
                );
                if postal_code.contains('-') {
                    zip_plus_four += 1;
                    assert!(
                        text(&address, "Label").contains(&postal_code),
                        "case {case}: the assembled label carries the postal code the \
                         place holds, got {:?} for {postal_code:?}",
                        text(&address, "Label")
                    );
                }
            }
        }
    }
    for (what, count) in [
        ("chain-store titles", chain_titles),
        ("independent titles", independent_titles),
        ("ZIP+4 postal codes", zip_plus_four),
    ] {
        assert!(
            count > 0,
            "case coverage: the table must reach some {what}, got {count}"
        );
    }
}
