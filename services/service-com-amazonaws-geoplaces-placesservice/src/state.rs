// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-geoplaces-placesservice service.
//!
//! Amazon Location Service Places (`geo-places`) has no customer-created resources:
//! every operation it models is `@readonly` and reports over AWS's own place data.
//! That data therefore lives in this service's state, seeded once (the engine builds
//! the state through `Default` on first touch, atomically) with a deterministic
//! catalog of places across several countries and regions — see [`seed_catalog`] for
//! what that catalog has to cover and why.
//!
//! Every handler PROJECTS its response out of these records, so the results of
//! different operations agree: a `PlaceId` handed out by Autocomplete, Geocode,
//! SearchText, SearchNearby or ReverseGeocode resolves through GetPlace to the same
//! `Title` and `Address`, and a `Position` handed out by any of them
//! reverse-geocodes back to the place it came from.
//!
//! Nothing here is mutated by a request. Every operation this crate implements is
//! `@readonly`, so even their pagination carries no server-side cursor: a `NextToken`
//! is a self-describing, opaque string (see [`encode_page_token`]) rather than an
//! entry written into state by a read, and so is the `QueryId` a Suggest query-term
//! suggestion hands out (see [`encode_query_id`]).

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AccessPoint, Address, BusinessChain, Category, ContactDetails, Contacts, Country, FoodType,
    Highlight, OpeningHours, Region, StreetComponents, SubRegion, TimeZone, ValidationException,
    ValidationExceptionField,
};

// ---------------------------------------------------------------------------
// Modeled vocabulary this module names
// ---------------------------------------------------------------------------

/// `PlaceType` values this catalog uses, from the model's `PlaceType` enum
/// (`PLACE_TYPE_VALUES` in `ops::types::shared_types`).
pub const PLACE_TYPE_POINT_OF_INTEREST: &str = "PointOfInterest";
/// See [`PLACE_TYPE_POINT_OF_INTEREST`].
pub const PLACE_TYPE_POINT_ADDRESS: &str = "PointAddress";
/// See [`PLACE_TYPE_POINT_OF_INTEREST`].
pub const PLACE_TYPE_LOCALITY: &str = "Locality";
/// See [`PLACE_TYPE_POINT_OF_INTEREST`].
pub const PLACE_TYPE_DISTRICT: &str = "District";
/// See [`PLACE_TYPE_POINT_OF_INTEREST`].
pub const PLACE_TYPE_SUB_REGION: &str = "SubRegion";
/// See [`PLACE_TYPE_POINT_OF_INTEREST`].
pub const PLACE_TYPE_REGION: &str = "Region";
/// See [`PLACE_TYPE_POINT_OF_INTEREST`].
pub const PLACE_TYPE_COUNTRY: &str = "Country";

/// `TypePlacement` values, from the model's enum of the same name: where a street
/// type sits relative to the street's base name.
pub const TYPE_PLACEMENT_BEFORE: &str = "BeforeBaseName";
/// See [`TYPE_PLACEMENT_BEFORE`].
pub const TYPE_PLACEMENT_AFTER: &str = "AfterBaseName";

/// The `IntendedUse` value that says the caller will persist the results. The
/// per-operation docs state such requests "will be charged at a higher rate", i.e.
/// they fall in a different pricing bucket than the default `SingleUse`.
pub const INTENDED_USE_STORAGE: &str = "Storage";

/// The pricing TIER a Places query is charged in by default.
///
/// `PricingBucket` is "the pricing bucket for which the query is charged at"
/// (`GeocodeResponse$PricingBucket`), bound to `x-amz-geo-pricing-bucket` and
/// `@required` on every Places response. The model enumerates no values; recorded
/// behavior is that every Places response — whichever operation served it —
/// carries this same tier token, so the bucket names a TIER and never the
/// operation.
pub const PRICING_BUCKET_CORE: &str = "Core";

/// The tier a query that declares `IntendedUse: Storage` is charged in: the
/// per-operation docs say such requests "will be charged at a higher rate", so
/// they are a tier of their own rather than the default [`PRICING_BUCKET_CORE`].
pub const PRICING_BUCKET_STORAGE: &str = "Storage";

/// `ParsedQueryComponent$QueryComponent` for a span of a FREE-FORM query: the text
/// was recognized inside `QueryText` rather than supplied under a named
/// `QueryComponents` member.
pub const QUERY_COMPONENT_FREE_TEXT: &str = "Query";

/// The address components a query is matched and parsed against, COARSEST FIRST:
/// the `GeocodeQueryComponents` members, which are also the members
/// `MatchScores.Components.Address` scores and `ParsedQuery.Address` keys.
pub const ADDRESS_COMPONENTS: [&str; 8] = [
    "Country",
    "Region",
    "SubRegion",
    "Locality",
    "District",
    "PostalCode",
    "Street",
    "AddressNumber",
];

/// `MatchScores.Components.Title` / `ParsedQuery.Title`, as
/// [`PlaceRecord::explain_match`] names the place's own name.
pub const TITLE_COMPONENT: &str = "Title";

/// Each [`ADDRESS_COMPONENTS`] entry with the `PlaceType`s a structured query
/// resolving at that level answers with.
///
/// A `GeocodeQueryComponents` member names an address LEVEL, so a structured query
/// resolves at the granularity of the finest member it supplies: the place that
/// satisfies "country X, region Y, locality Z" is the locality itself, not the
/// streets and businesses inside it.
pub const COMPONENT_GRANULARITY: [(&str, &[&str]); 8] = [
    ("Country", &[PLACE_TYPE_COUNTRY]),
    ("Region", &[PLACE_TYPE_REGION]),
    ("SubRegion", &[PLACE_TYPE_SUB_REGION]),
    ("Locality", &[PLACE_TYPE_LOCALITY]),
    ("District", &[PLACE_TYPE_DISTRICT]),
    ("PostalCode", &["PostalCode"]),
    ("Street", &["Street", PLACE_TYPE_POINT_ADDRESS]),
    (
        "AddressNumber",
        &[
            PLACE_TYPE_POINT_ADDRESS,
            "InterpolatedAddress",
            "SecondaryAddress",
        ],
    ),
];

/// The `PlaceType`s a structured query answers with: those of the finest component
/// it supplied, or `None` when it supplied no component the model names.
pub fn granularity_place_types(
    components: &[(&'static str, String)],
) -> Option<&'static [&'static str]> {
    COMPONENT_GRANULARITY
        .iter()
        .rfind(|(component, _)| {
            components
                .iter()
                .any(|(supplied, _)| supplied == component)
        })
        .map(|(_, place_types)| *place_types)
}

/// Every `PlaceType` the model's enum declares, grouped into granularity LEVELS,
/// COARSEST LEVEL FIRST. The enum walks the place hierarchy down — a country
/// encloses a region, a region a sub-region, a sub-region a locality, and so on to
/// a single point inside a street — so the declaration order IS the granularity
/// order; the types grouped in one level name the same KIND of geometry (a line
/// for a street or an intersection, a single point for an address or the business
/// standing at it), so neither of them is finer than the other.
///
/// SOURCE: `com.amazonaws.geoplaces#PlaceType`
/// (`smithy_query(query="shape",shape="PlaceType")`).
pub const PLACE_TYPE_LEVELS: [&[&str]; 11] = [
    &[PLACE_TYPE_COUNTRY],
    &[PLACE_TYPE_REGION],
    &[PLACE_TYPE_SUB_REGION],
    &[PLACE_TYPE_LOCALITY],
    &[PLACE_TYPE_DISTRICT],
    &["SubDistrict"],
    &["PostalCode"],
    &["Block"],
    &["SubBlock"],
    &["Intersection", "Street"],
    &[
        PLACE_TYPE_POINT_OF_INTEREST,
        PLACE_TYPE_POINT_ADDRESS,
        "InterpolatedAddress",
        "SecondaryAddress",
        "InferredSecondaryAddress",
    ],
];

/// How FINE a `PlaceType` is: the higher the rank, the more specific the place.
///
/// A type [`PLACE_TYPE_LEVELS`] does not name ranks below every one it does, so an
/// unknown type never outranks a known one.
pub fn granularity_rank(place_type: &str) -> usize {
    PLACE_TYPE_LEVELS
        .iter()
        .position(|level| level.contains(&place_type))
        .map_or(0, |rank| rank + 1)
}

/// `AdditionalFeatures` values this emulator can answer, from the per-operation
/// `*AdditionalFeature` enums in the model.
pub const FEATURE_ACCESS: &str = "Access";
/// See [`FEATURE_ACCESS`].
pub const FEATURE_CONTACT: &str = "Contact";
/// See [`FEATURE_ACCESS`].
pub const FEATURE_TIME_ZONE: &str = "TimeZone";

/// `SuggestResultItemType` values, from the model's enum of the same name: what
/// one suggestion IS — a place the query names, or a query term to search with.
pub const SUGGEST_ITEM_PLACE: &str = "Place";
/// See [`SUGGEST_ITEM_PLACE`].
pub const SUGGEST_ITEM_QUERY: &str = "Query";

/// `QueryType` values, from the model's enum of the same name: the kinds of term
/// a `Query` suggestion can stand for. A food type is neither, so the catalog's
/// food-type vocabulary names no query suggestion of its own.
pub const QUERY_TYPE_CATEGORY: &str = "Category";
/// See [`QUERY_TYPE_CATEGORY`].
pub const QUERY_TYPE_BUSINESS_CHAIN: &str = "BusinessChain";

/// `ValidationExceptionReason` values, from the model's enum of the same name.
pub const REASON_MISSING: &str = "Missing";
/// See [`REASON_MISSING`].
pub const REASON_CANNOT_PARSE: &str = "CannotParse";
/// See [`REASON_MISSING`].
pub const REASON_FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";
/// See [`REASON_MISSING`].
pub const REASON_OTHER: &str = "Other";

/// The inclusive maximum of the modeled `DistanceMeters` range used by the result
/// items' `Distance` member (`range 0..4294967295`).
const DISTANCE_MAX: i64 = 4_294_967_295;

/// Default `MaxResults` for Geocode, SearchText and SearchNearby: "Default value:
/// 20" in each request shape's `MaxResults` documentation.
pub const DEFAULT_MAX_RESULTS: i32 = 20;

/// Default `MaxResults` for ReverseGeocode: "Default value: 1" in
/// `ReverseGeocodeRequest$MaxResults`.
pub const DEFAULT_REVERSE_MAX_RESULTS: i32 = 1;

/// The inclusive maximum of Geocode's, SearchText's, SearchNearby's and
/// ReverseGeocode's modeled `MaxResults` range (`range 1..100`), which is also the
/// maximum length of their `*ResultItemList` outputs (`len 0..100`).
pub const MAX_RESULTS_CEILING: i32 = 100;

/// Default `MaxResults` for Autocomplete: "Default value: 5" in
/// `AutocompleteRequest$MaxResults`.
pub const AUTOCOMPLETE_DEFAULT_MAX_RESULTS: i32 = 5;

/// The inclusive maximum of Autocomplete's modeled `MaxResults` range
/// (`range 1..20`), which is also the maximum length of its
/// `AutocompleteResultItemList` output (`len 0..20`).
pub const AUTOCOMPLETE_MAX_RESULTS_CEILING: i32 = 20;

/// The inclusive bounds of the modeled `QueryRadius` (`DistanceMeters`,
/// `range 1..21000000`) on ReverseGeocode and SearchNearby.
pub const QUERY_RADIUS_MIN: i64 = 1;
/// See [`QUERY_RADIUS_MIN`].
pub const QUERY_RADIUS_MAX: i64 = 21_000_000;

/// The inclusive maximum of Suggest's modeled `MaxQueryRefinements` range
/// (`range 1..10`), which is also the maximum length of the `QueryRefinementList`
/// they are reported in (`len 0..10`).
///
/// The model documents no default for `MaxQueryRefinements`, so a request that
/// asks for no narrower bound is answered with as many refinements as the modeled
/// list carries.
pub const QUERY_REFINEMENTS_CEILING: i32 = 10;

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Mean Earth radius in metres (IUGG mean radius), used for the great-circle
/// distances the service reports in `Distance`.
const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// Metres in one degree of latitude on that sphere, used to size a `MapView` box
/// around a point.
const METRES_PER_DEGREE_LAT: f64 = EARTH_RADIUS_M * std::f64::consts::PI / 180.0;

/// Decimal places a reported WGS 84 coordinate carries.
///
/// Every coordinate this service reports — `Position`, `MapView`, `AccessPoints` —
/// is a `Double` the service rounds to this many fractional degrees (about a metre
/// of latitude), so a response carries a coordinate and not the full binary
/// expansion of whatever arithmetic produced it.
const COORDINATE_DECIMALS: u32 = 5;

/// `degrees` at the precision a reported coordinate carries
/// ([`COORDINATE_DECIMALS`]).
fn coordinate(degrees: f64) -> f64 {
    let scale = 10_f64.powi(COORDINATE_DECIMALS as i32);
    (degrees * scale).round() / scale
}

/// A `[longitude, latitude]` pair at reported coordinate precision.
fn coordinate_pair(position: [f64; 2]) -> [f64; 2] {
    [coordinate(position[0]), coordinate(position[1])]
}

/// A `[west, south, east, north]` box at reported coordinate precision.
fn coordinate_box(box_: [f64; 4]) -> [f64; 4] {
    [
        coordinate(box_[0]),
        coordinate(box_[1]),
        coordinate(box_[2]),
        coordinate(box_[3]),
    ]
}

/// A `[longitude, latitude]` pair taken from a modeled `Position` list, or `None`
/// when the list is not exactly two elements (the modeled `len 2..2` is refused at
/// the boundary, so that part only guards an in-process caller) or the pair is not
/// a position at all.
///
/// Every `Position` the model carries is documented as being "in World Geodetic
/// System (WGS 84) format", so a pair outside that range — or a non-finite one — is
/// no position, whichever member it arrived on: refusing it here is what holds a
/// `QueryPosition`, a `BiasPosition` and a filter circle's centre to one contract,
/// instead of ranking places by a distance computed from a meaningless coordinate.
pub fn position(values: &[f64]) -> Option<[f64; 2]> {
    match values {
        [longitude, latitude] if is_wgs84([*longitude, *latitude]) => {
            Some([*longitude, *latitude])
        }
        _ => None,
    }
}

/// A `[west, south, east, north]` box taken from a modeled `BoundingBox` list, or
/// `None` when the list is not exactly four elements (modeled `len 4..4`).
pub fn bounding_box(values: &[f64]) -> Option<[f64; 4]> {
    match values {
        [west, south, east, north] => Some([*west, *south, *east, *north]),
        _ => None,
    }
}

/// The great-circle distance between two WGS 84 positions, in whole metres,
/// clamped into the modeled `Distance` range.
pub fn distance_metres(from: [f64; 2], to: [f64; 2]) -> i64 {
    let (lon1, lat1) = (from[0].to_radians(), from[1].to_radians());
    let (lon2, lat2) = (to[0].to_radians(), to[1].to_radians());
    let half_delta_lat = ((lat2 - lat1) / 2.0).sin();
    let half_delta_lon = ((lon2 - lon1) / 2.0).sin();
    let chord = half_delta_lat.mul_add(
        half_delta_lat,
        lat1.cos() * lat2.cos() * half_delta_lon * half_delta_lon,
    );
    let metres = 2.0 * EARTH_RADIUS_M * chord.clamp(0.0, 1.0).sqrt().asin();
    metres.round().clamp(0.0, DISTANCE_MAX as f64) as i64
}

/// The `MapView` box of a place whose geometry is a POINT — an address, a point of
/// interest, an access point: "the bounding box enclosing the geometric shape (area
/// or line) that an individual result covers" degenerates to the point itself, so
/// the box the service reports is the small viewport it frames that point with,
/// [`POINT_EXTENT_M`] either side of it, rendered as `[west, south, east, north]`
/// the way the modeled `BoundingBox` documents.
///
/// A place with real AREA carries its own box instead
/// ([`PlaceRecord::with_map_view`]): nothing derives an area from a `PlaceType`.
fn point_view(position: [f64; 2]) -> [f64; 4] {
    let latitude_delta = POINT_EXTENT_M / METRES_PER_DEGREE_LAT;
    let cosine = position[1].to_radians().cos().abs().max(1e-6);
    let longitude_delta = POINT_EXTENT_M / (METRES_PER_DEGREE_LAT * cosine);
    [
        position[0] - longitude_delta,
        position[1] - latitude_delta,
        position[0] + longitude_delta,
        position[1] + latitude_delta,
    ]
}

/// Half the side of the viewport a point feature's [`point_view`] is framed with,
/// in metres.
const POINT_EXTENT_M: f64 = 150.0;

// ---------------------------------------------------------------------------
// Positions a caller supplies
// ---------------------------------------------------------------------------

/// Is `[longitude, latitude]` a position in the World Geodetic System (WGS 84)
/// range the modeled `Position` documents?
pub fn is_wgs84(position: [f64; 2]) -> bool {
    let [longitude, latitude] = position;
    longitude.is_finite()
        && latitude.is_finite()
        && (-180.0..=180.0).contains(&longitude)
        && (-90.0..=90.0).contains(&latitude)
}

// ---------------------------------------------------------------------------
// Text normalization and matching
// ---------------------------------------------------------------------------

/// The tokens a piece of place text or a free-form query indexes as: lowercased,
/// with every non-alphanumeric character treated as a separator.
pub fn normalize_tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|character| if character.is_alphanumeric() { character } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// The words of a piece of text AS IT SPELLS THEM: the same runs of alphanumeric
/// characters [`normalize_tokens`] splits on, each keeping its own casing, so a
/// word taken from the catalog is reported the way the catalog writes it.
pub fn display_words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Street-type words with their standard abbreviations: the pairs a query may use
/// interchangeably, so "Pennsylvania Avenue" and "Pennsylvania Ave" are one street,
/// and the words [`AddressRecord::street_components_modeled`] splits a street name
/// on. The English rows are USPS Publication 28 Appendix C1 standard suffix
/// abbreviations; the French rows are the equivalent La Poste abbreviations, for
/// this catalog's French addresses.
const STREET_TYPES: [(&str, &str); 23] = [
    ("aly", "alley"),
    ("av", "avenue"),
    ("ave", "avenue"),
    ("bd", "boulevard"),
    ("blvd", "boulevard"),
    ("cir", "circle"),
    ("ct", "court"),
    ("cv", "cove"),
    ("dr", "drive"),
    ("expy", "expressway"),
    ("fwy", "freeway"),
    ("hwy", "highway"),
    ("imp", "impasse"),
    ("ln", "lane"),
    ("pkwy", "parkway"),
    ("pl", "place"),
    ("plz", "plaza"),
    ("r", "rue"),
    ("rd", "road"),
    ("sq", "square"),
    ("st", "street"),
    ("ter", "terrace"),
    ("trl", "trail"),
];

/// Directional identifiers with their written-out forms (USPS Publication 28
/// Appendix B). A directional that leads or trails a street name locates the street
/// but is "not included in the base name" (`StreetComponents$Prefix`/`$Suffix`).
const DIRECTIONALS: [(&str, &str); 8] = [
    ("n", "north"),
    ("s", "south"),
    ("e", "east"),
    ("w", "west"),
    ("ne", "northeast"),
    ("nw", "northwest"),
    ("se", "southeast"),
    ("sw", "southwest"),
];

/// Is `short` the standard abbreviation of the whole word `long`?
fn abbreviates(short: &str, long: &str) -> bool {
    STREET_TYPES
        .iter()
        .chain(DIRECTIONALS.iter())
        .any(|(abbreviation, word)| *abbreviation == short && *word == long)
}

/// Is this normalized word a street type, written out or abbreviated?
fn is_street_type(word: &str) -> bool {
    STREET_TYPES
        .iter()
        .any(|(abbreviation, written)| *abbreviation == word || *written == word)
}

/// Is this normalized word a directional identifier, written out or abbreviated?
fn is_directional(word: &str) -> bool {
    DIRECTIONALS
        .iter()
        .any(|(abbreviation, written)| *abbreviation == word || *written == word)
}

/// How well an indexed term answers a piece of a query: 1 when they are the same
/// word or standard abbreviations of each other ([`STREET_TYPES`],
/// [`DIRECTIONALS`]), half when the query word is a leading part of the term (a
/// partially typed word, which is what a free-form query member documents), and
/// `None` when they are unrelated.
///
/// The half match is ONE-DIRECTIONAL on purpose. A shorter indexed term is not a
/// match for a longer query word unless the two are a known abbreviation pair, or
/// every street type in the catalog ("St") would half-match any query word that
/// happens to start with it ("Starbucks"), and a text search would answer with
/// places that have nothing to do with the query.
fn affinity(term: &str, wanted: &str) -> Option<f64> {
    if term.is_empty() || wanted.is_empty() {
        return None;
    }
    if term == wanted || abbreviates(term, wanted) || abbreviates(wanted, term) {
        return Some(1.0);
    }
    term.starts_with(wanted).then_some(0.5)
}

/// How well a multi-word value answers a multi-word piece of a place's text, or
/// `None` when any of its words answers none of the text's.
///
/// The score is the share of the LONGER of the two that was accounted for, word by
/// word, so "Broad Street" answers the street "Broad St" in full (the street type
/// is a standard abbreviation of the word) while "Broad" answers it by half: it
/// names part of it.
fn text_affinity(terms: &[String], wanted: &[String]) -> Option<f64> {
    if terms.is_empty() || wanted.is_empty() {
        return None;
    }
    let mut matched = 0.0;
    for word in wanted {
        matched += terms
            .iter()
            .filter_map(|term| affinity(term, word))
            .fold(f64::NAN, f64::max);
        if matched.is_nan() {
            return None;
        }
    }
    Some(matched / terms.len().max(wanted.len()) as f64)
}

/// One token of a query text, with the span it occupies in that text as CHAR
/// indices — the indices `ParsedQueryComponent$StartIndex`/`$EndIndex` report —
/// and as BYTE offsets, so the text of a span is sliced in constant time rather
/// than rescanned.
#[derive(Debug, Clone)]
struct QueryToken {
    text: String,
    start: usize,
    end: usize,
    byte_start: usize,
    byte_end: usize,
}

/// A free-form query tokenized ONCE: the text and its tokens, so a request that
/// explains its match against many candidates does the tokenizing a single time.
///
/// The number of tokens is bounded by [`QUERY_TEXT_MAX`], which every operation
/// taking a free-form query enforces ([`check_query_text`]), so the matching work
/// one request can ask for has a fixed ceiling.
#[derive(Debug, Clone)]
pub struct QueryTokens<'a> {
    text: &'a str,
    tokens: Vec<QueryToken>,
}

impl<'a> QueryTokens<'a> {
    /// Tokenize a query text: [`normalize_tokens`] with the positions kept, so a
    /// parsed component can point back into the text the caller sent.
    pub fn new(text: &'a str) -> Self {
        let mut tokens: Vec<QueryToken> = Vec::new();
        let mut current = String::new();
        let mut start = 0;
        let mut byte_start = 0;
        for (char_index, (byte_index, character)) in text.char_indices().enumerate() {
            if character.is_alphanumeric() {
                if current.is_empty() {
                    start = char_index;
                    byte_start = byte_index;
                }
                current.extend(character.to_lowercase());
            } else if !current.is_empty() {
                tokens.push(QueryToken {
                    text: std::mem::take(&mut current),
                    start,
                    end: char_index,
                    byte_start,
                    byte_end: byte_index,
                });
            }
        }
        if !current.is_empty() {
            tokens.push(QueryToken {
                text: current,
                start,
                end: text.chars().count(),
                byte_start,
                byte_end: text.len(),
            });
        }
        Self { text, tokens }
    }

    /// Did the caller send any query text at all?
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// The normalized tokens alone, as [`normalize_tokens`] yields them: what a
    /// piece of a result's own text is matched against when the spans in the
    /// QUERY are not needed (highlighting a response, scoring a term).
    pub fn normalized(&self) -> Vec<String> {
        self.tokens.iter().map(|token| token.text.clone()).collect()
    }

    /// Each token with the span it occupies in the query: the text AS THE CALLER
    /// TYPED IT, the normalized token it indexes as, and the `[start, end)` CHAR
    /// span — the indices a `QueryRefinement` reports.
    pub fn spans(&self) -> Vec<(&str, &str, i32, i32)> {
        self.tokens
            .iter()
            .map(|token| {
                (
                    &self.text[token.byte_start..token.byte_end],
                    token.text.as_str(),
                    token.start as i32,
                    token.end as i32,
                )
            })
            .collect()
    }
}

/// Where a piece of a place's text was recognized inside a query, and how well.
#[derive(Debug, Clone)]
struct Located {
    score: f64,
    start_index: i32,
    end_index: i32,
    value: String,
}

/// Locate a place's own text (its name, or one of its address components) inside a
/// tokenized query: the best consecutive run of query tokens that answers it word
/// for word, scored as the mean [`affinity`] over those words, and reported with
/// the span and the text AS THE CALLER TYPED IT.
///
/// The span's text is built ONCE, for the run that is kept, and sliced by the byte
/// offsets the tokens carry: scanning the query again per candidate run would make
/// the work grow with the SQUARE of the query's length.
fn locate(query: &QueryTokens<'_>, text: &str) -> Option<Located> {
    let wanted = normalize_tokens(text);
    let tokens = query.tokens.as_slice();
    if wanted.is_empty() || wanted.len() > tokens.len() {
        return None;
    }
    let mut best: Option<(f64, usize)> = None;
    for (offset, window) in tokens.windows(wanted.len()).enumerate() {
        let mut total = 0.0;
        for (token, word) in window.iter().zip(&wanted) {
            match affinity(word, &token.text) {
                Some(score) => total += score,
                None => {
                    total = f64::NAN;
                    break;
                }
            }
        }
        if total.is_nan() {
            continue;
        }
        let score = total / wanted.len() as f64;
        if best.is_none_or(|(kept, _)| score > kept) {
            best = Some((score, offset));
        }
    }
    let (score, offset) = best?;
    let first = &tokens[offset];
    let last = &tokens[offset + wanted.len() - 1];
    Some(Located {
        score,
        start_index: first.start as i32,
        end_index: last.end as i32,
        value: query.text[first.byte_start..last.byte_end].to_string(),
    })
}

// ---------------------------------------------------------------------------
// Highlighting a RESULT's own text
// ---------------------------------------------------------------------------

/// The inclusive maximum number of entries a `HighlightList` carries
/// (`HighlightList`, `len 0..200`).
pub const HIGHLIGHT_LIST_MAX: usize = 200;

/// One section of a piece of a RESULT's text that a query's terms accounted for:
/// the span it occupies in that text, as the CHAR indices a `Highlight` reports,
/// and the text of the span as the response carries it.
///
/// `AutocompleteHighlights` is documented as "how the parts of the response element
/// matched the input query by returning the sections OF THE RESPONSE which matched
/// to input query terms", so a highlight points into the text the response reports —
/// a result's `Title` or one of its address components — and not into the query.
/// It is the complement of [`ComponentMatch`], which points into the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextHighlight {
    /// `Highlight$StartIndex`: the first char of the section.
    pub start_index: i32,
    /// `Highlight$EndIndex`: one past its last char.
    pub end_index: i32,
    /// `Highlight$Value`: the section as the response spells it.
    pub value: String,
}

/// One word of a piece of a result's own text, with where it sits in that text.
struct TextWord<'a> {
    /// The word as the text spells it.
    text: &'a str,
    /// Char index of the word's first character in the whole text.
    start: usize,
    /// Byte offset of the word's first character in the whole text.
    byte_start: usize,
    /// The word normalized the way an indexed term is ([`normalize_tokens`]), so
    /// it compares against a query token through [`affinity`].
    normalized: String,
}

/// The words of a piece of text: the runs of alphanumeric characters
/// [`normalize_tokens`] splits on, each one keeping its place in the text.
fn words_of(text: &str) -> Vec<TextWord<'_>> {
    fn word(text: &str, start: usize, byte_start: usize, byte_end: usize) -> TextWord<'_> {
        let slice = &text[byte_start..byte_end];
        TextWord {
            text: slice,
            start,
            byte_start,
            normalized: slice.to_lowercase(),
        }
    }

    let mut words = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    for (char_index, (byte_index, character)) in text.char_indices().enumerate() {
        if character.is_alphanumeric() {
            if open.is_none() {
                open = Some((char_index, byte_index));
            }
        } else if let Some((start, byte_start)) = open.take() {
            words.push(word(text, start, byte_start, byte_index));
        }
    }
    if let Some((start, byte_start)) = open {
        words.push(word(text, start, byte_start, text.len()));
    }
    words
}

/// The sections of one piece of a result's text that a query's terms accounted
/// for, in the order they occur in that text.
///
/// A word of the text is a section when a query term [matches] it: the WHOLE word
/// where the term named it in full (or by a standard abbreviation of it), and the
/// leading characters a partially typed term covers otherwise — which is what a
/// client asked to complete a query highlights as the user types. Two sections with
/// no word of their own between them are reported as ONE section, because the
/// characters separating them (a space, a comma) are not text the query left
/// unmatched.
///
/// [matches]: affinity
pub fn highlight_matches(text: &str, query_tokens: &[String]) -> Vec<TextHighlight> {
    // `[char start, char end, byte start, byte end]` of each section, so a section
    // that grows by joining the next one is sliced out of the text once, at the end.
    let mut spans: Vec<[usize; 4]> = Vec::new();
    for word in words_of(text) {
        let matched = query_tokens
            .iter()
            .filter_map(|token| match affinity(&word.normalized, token)? {
                score if score >= 1.0 => Some(word.normalized.chars().count()),
                _ => Some(token.chars().count()),
            })
            .max();
        let Some(matched) = matched else {
            continue;
        };
        // Taken off the word AS THE TEXT SPELLS IT: a highlight's value is a
        // section of the response, so it keeps the response's own casing. Counting
        // the characters taken rather than assuming `matched` of them is what keeps
        // the span right for a word whose lowercase form is longer than itself.
        let taken: String = word.text.chars().take(matched).collect();
        if taken.is_empty() {
            continue;
        }
        let char_end = word.start + taken.chars().count();
        let byte_end = word.byte_start + taken.len();
        let joins_previous = spans.last().is_some_and(|last| {
            !text[last[3]..word.byte_start]
                .chars()
                .any(char::is_alphanumeric)
        });
        match spans.last_mut().filter(|_| joins_previous) {
            Some(last) => {
                last[1] = char_end;
                last[3] = byte_end;
            }
            None => spans.push([word.start, char_end, word.byte_start, byte_end]),
        }
    }
    spans
        .into_iter()
        .take(HIGHLIGHT_LIST_MAX)
        .map(|[start, end, byte_start, byte_end]| TextHighlight {
            start_index: start as i32,
            end_index: end as i32,
            value: text[byte_start..byte_end].to_string(),
        })
        .collect()
}

/// The modeled `Highlight` list for one piece of a result's text, or `None` when
/// the query named none of it: every `HighlightList` member is optional on every
/// shape that carries one, so "the query matched nothing here" is an ABSENT member
/// rather than an empty list.
pub fn modeled_highlights(text: &str, query_tokens: &[String]) -> Option<Vec<Highlight>> {
    let found = highlight_matches(text, query_tokens);
    (!found.is_empty()).then(|| {
        found
            .into_iter()
            .map(|section| Highlight {
                start_index: Some(section.start_index),
                end_index: Some(section.end_index),
                value: Some(section.value),
            })
            .collect()
    })
}

/// One component of a place that a query matched: how well, and the span of the
/// query it was recognized in.
#[derive(Debug, Clone)]
pub struct ComponentMatch {
    /// The component matched: a [`ADDRESS_COMPONENTS`] entry, or
    /// [`TITLE_COMPONENT`].
    pub component: &'static str,
    /// `MatchScore`: 1 when every word was recognized in full (modeled `range
    /// 0..1`).
    pub score: f64,
    /// `ParsedQueryComponent$StartIndex`.
    pub start_index: i32,
    /// `ParsedQueryComponent$EndIndex`.
    pub end_index: i32,
    /// `ParsedQueryComponent$Value`: the text as the caller sent it.
    pub value: String,
    /// `ParsedQueryComponent$QueryComponent`: the request member the text came
    /// from — a `QueryComponents` member's name, or
    /// [`QUERY_COMPONENT_FREE_TEXT`].
    pub query_component: String,
}

/// How a query matched one place, component by component: the data behind a
/// geocode result's `MatchScores` and `ParsedQuery`.
#[derive(Debug, Clone, Default)]
pub struct MatchExplanation {
    /// `MatchScoreDetails$Overall`.
    pub overall: f64,
    /// The place's own name, where the query named it.
    pub title: Option<ComponentMatch>,
    /// The address components the query named, coarsest first.
    pub components: Vec<ComponentMatch>,
}

impl MatchExplanation {
    /// The best score recorded for one component, or 0 when the query never named
    /// it: `MatchScore` carries a modeled `@default` of 0, which is "no tokens were
    /// matched" (`ComponentMatchScores`).
    pub fn score_of(&self, component: &str) -> f64 {
        self.components
            .iter()
            .filter(|matched| matched.component == component)
            .map(|matched| matched.score)
            .fold(0.0, f64::max)
    }

    /// The parsed spans recorded for one component, in the order they were parsed,
    /// or `None` when the query never named it.
    pub fn spans_of(&self, component: &str) -> Option<Vec<&ComponentMatch>> {
        let spans: Vec<&ComponentMatch> = self
            .components
            .iter()
            .filter(|matched| matched.component == component)
            .collect();
        (!spans.is_empty()).then_some(spans)
    }

    /// Did the query name anything at all about this place?
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.components.is_empty()
    }
}

/// A candidate place and why it is one: its position in [`places`], the relevance
/// of the match, and its distance in metres from the request's reference position
/// (the `QueryPosition` or `BiasPosition`), or 0 when the request gave none.
///
/// [`places`]: ComAmazonawsGeoplacesPlacesserviceState::places
#[derive(Debug, Clone, Copy)]
pub struct Scored {
    pub index: usize,
    pub score: f64,
    /// How much of the query named the place's OWN name (its title or one of its
    /// alternate names), as [`PlaceRecord::name_score`] measures it. It is NOT
    /// the reported `MatchScore` — it only ranks two places the query matched
    /// equally well (see [`order_by_relevance`]).
    ///
    /// [`order_by_relevance`]: ComAmazonawsGeoplacesPlacesserviceState::order_by_relevance
    pub name_score: f64,
    pub distance: i64,
}

/// One query TERM a free-form query names: a category or a business chain the
/// catalog holds, as [`ComAmazonawsGeoplacesPlacesserviceState::query_terms`]
/// finds it. It is what a `Query` suggestion stands for — the follow-on query a
/// caller can run instead of one place.
#[derive(Debug, Clone)]
pub struct QueryTerm {
    /// The term as the catalog spells it, which is the suggestion's `Title`.
    pub term: String,
    /// `SuggestQueryResult$QueryType`: [`QUERY_TYPE_CATEGORY`] or
    /// [`QUERY_TYPE_BUSINESS_CHAIN`].
    pub query_type: &'static str,
    /// How much of the query the term's own words accounted for, scored the way
    /// [`PlaceRecord::text_score`] scores a record. Never reported — it only
    /// ranks two terms the query named.
    pub score: f64,
}

/// One refinement of a query: the span of the caller's text that names a word
/// only in part, and the catalog word it completes to — the modeled
/// `QueryRefinement`.
#[derive(Debug, Clone)]
pub struct Refinement {
    /// `QueryRefinement$OriginalTerm`: the span AS THE CALLER TYPED IT.
    pub original_term: String,
    /// `QueryRefinement$RefinedTerm`: the word to use in its place.
    pub refined_term: String,
    /// `QueryRefinement$StartIndex`: the first char of the original term.
    pub start_index: i32,
    /// `QueryRefinement$EndIndex`: one past its last char.
    pub end_index: i32,
}

// ---------------------------------------------------------------------------
// The catalog records
// ---------------------------------------------------------------------------

/// The NAME text of one address, as one language writes it: the overrides a
/// [`language variant`] applies to the record's own text.
///
/// Only name-valued components appear here. A code (`CountryCode2`, a region
/// code), a postal code and an address number are the same string in every
/// language, so no language renames them.
///
/// [`language variant`]: AddressRecord::names_by_language
#[derive(Debug, Clone, Default)]
pub struct AddressNames {
    pub country_name: Option<String>,
    pub region_name: Option<String>,
    /// The sub-region's bare name as this language writes it. A localized name
    /// carries whatever division word that language spells out, so it REPLACES
    /// the default's [`AddressRecord::sub_region_division`] rather than taking it
    /// over.
    pub sub_region_name: Option<String>,
    pub locality: Option<String>,
    pub district: Option<String>,
    pub street: Option<String>,
}

impl AddressNames {
    /// The names this variant gives, named by the member they override — used to
    /// index a record by its text in EVERY language it holds.
    fn given(&self) -> impl Iterator<Item = &str> {
        [
            self.country_name.as_deref(),
            self.region_name.as_deref(),
            self.sub_region_name.as_deref(),
            self.locality.as_deref(),
            self.district.as_deref(),
            self.street.as_deref(),
        ]
        .into_iter()
        .flatten()
    }
}

/// The language one request's results are rendered in: the `Language` it named,
/// and the free-form query it matched records by, where it sent one.
///
/// `Language` is "a list of BCP 47 compliant language codes for the results to be
/// rendered in", so a request that names one renders every result's text in it.
/// A request that names NONE still asked for a place by SOME name: it typed the
/// name one language gives the place, and that name is the display name of the
/// result it gets back, so the place's own name is rendered in the language the
/// query named it in. Only a request that neither names a language nor names the
/// place by a name of another language leaves a result in "the default language
/// for the entry".
///
/// An operation that resolves records by position or by id carries no query, so
/// its results are rendered in the language asked for, or the entry's own.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResultLanguage<'a> {
    /// The request's `Language` member, where it set one.
    requested: Option<&'a str>,
    /// The normalized tokens of the free-form query the request matched records
    /// by, empty for a request that sent none.
    query: &'a [String],
}

impl<'a> ResultLanguage<'a> {
    /// The rendering for a request that matched records by a free-form query,
    /// tokenized by [`normalize_tokens`].
    pub fn matched(requested: Option<&'a str>, query: &'a [String]) -> Self {
        Self { requested, query }
    }

    /// The `Language` the request named, which every piece of an entry's text
    /// other than the place's own name is rendered in.
    fn requested(&self) -> Option<&'a str> {
        self.requested
    }
}

/// A request that named no language renders results in the language asked for by
/// nothing but the entry's own text — the rendering an operation with no
/// free-form query to match a name against reports.
impl<'a> From<Option<&'a str>> for ResultLanguage<'a> {
    fn from(requested: Option<&'a str>) -> Self {
        Self {
            requested,
            query: &[],
        }
    }
}

/// The variant of `table` a requested BCP 47 language tag selects, with the tag
/// the catalog spells it under, or `None` when the entry holds no text in that
/// language — in which case "data will be returned in the default language for the
/// entry" (`ReverseGeocodeRequest$Language`).
///
/// The match is RFC 4647 "lookup": the whole requested tag first, then
/// progressively shorter prefixes of it cut at a subtag boundary, so `uk-UA`
/// selects an entry's `uk` text. Tags are compared case-insensitively, as BCP 47
/// defines them to be.
fn lookup_language<'a, T>(
    table: &'a BTreeMap<String, T>,
    requested: Option<&str>,
) -> Option<(&'a str, &'a T)> {
    let requested = requested?;
    let mut candidate = requested;
    loop {
        if let Some((tag, value)) = table
            .iter()
            .find(|(tag, _)| tag.eq_ignore_ascii_case(candidate))
        {
            return Some((tag.as_str(), value));
        }
        // No subtag left to drop: the requested language matches nothing in the table.
        let cut = candidate.rfind('-')?;
        candidate = &candidate[..cut];
    }
}

/// A place's address, as every operation that reports an `Address` renders it.
///
/// The `Label` is NOT stored: it is assembled from these components on the way out
/// ([`assemble_label`]), which is what `Address$Label` documents it to be.
#[derive(Debug, Clone)]
pub struct AddressRecord {
    pub country_code2: String,
    pub country_code3: String,
    pub country_name: String,
    pub region_code: String,
    pub region_name: String,
    /// The sub-region's NAME, bare: `Address$SubRegion$Name` is "name for the
    /// county or sub-region", so it holds what the area is CALLED and not the
    /// generic division word a display title writes beside it — that word is
    /// [`Self::sub_region_division`], and it reaches a response only through the
    /// `Title`/`Label` of a place that IS this sub-region.
    pub sub_region_name: String,
    /// The administrative division word the sub-region's display name carries
    /// beside its bare name ("County" for a US county), or empty where the
    /// language writes none. Never reported as a component of its own.
    pub sub_region_division: String,
    pub locality: String,
    pub district: Option<String>,
    pub postal_code: String,
    pub street: Option<String>,
    pub address_number: Option<String>,
    /// The BCP 47 language this record's OWN text is written in: "the default
    /// language for the entry" the per-operation `Language` member falls back to,
    /// and the language reported as the street components' `Language`.
    pub language: String,
    /// This entry's name text in other languages, keyed by BCP 47 language tag —
    /// what a request's `Language` selects ("a list of BCP 47 compliant language
    /// codes for the results to be rendered in"), with [`Self::language`] as the
    /// fallback.
    ///
    /// A variant RENAMES a component this record holds; it never adds one, so a
    /// record that names no locality has no locality in any language.
    pub names_by_language: BTreeMap<String, AddressNames>,
}

impl AddressRecord {
    /// This address as one language writes it: the variant the requested BCP 47
    /// tag selects applied over the record's own text, with [`Self::language`] set
    /// to the language the result is actually rendered in.
    ///
    /// With no language asked for — or none this entry holds text in — the record
    /// answers in its own language, which is what `Language` documents ("if there
    /// is no data for the result in the requested language, data will be returned
    /// in the default language for the entry").
    pub fn rendered_in(&self, language: Option<&str>) -> Self {
        let Some((tag, names)) = lookup_language(&self.names_by_language, language) else {
            return self.clone();
        };
        // A variant renames what this record HOLDS: an override of a component
        // the record leaves empty is not applied, so no language adds a component
        // the place does not have.
        let renamed = |current: &str, localized: Option<&String>| match localized {
            Some(localized) if !current.is_empty() => localized.clone(),
            _ => current.to_string(),
        };
        let optional = |current: &Option<String>, localized: Option<&String>| match localized {
            Some(localized) if current.is_some() => Some(localized.clone()),
            _ => current.clone(),
        };
        let sub_region_name = renamed(&self.sub_region_name, names.sub_region_name.as_ref());
        Self {
            country_name: renamed(&self.country_name, names.country_name.as_ref()),
            region_name: renamed(&self.region_name, names.region_name.as_ref()),
            sub_region_division: if sub_region_name == self.sub_region_name {
                self.sub_region_division.clone()
            } else {
                // The localized name carries its own division word, if that
                // language writes one at all.
                String::new()
            },
            sub_region_name,
            locality: renamed(&self.locality, names.locality.as_ref()),
            district: optional(&self.district, names.district.as_ref()),
            street: optional(&self.street, names.street.as_ref()),
            language: tag.to_string(),
            ..self.clone()
        }
    }

    /// The sub-region's DISPLAY name: its bare [`Self::sub_region_name`] with the
    /// administrative division word its title carries, where it has one. Used to
    /// name a place that IS this sub-region — never as the `SubRegion` component,
    /// which the model documents as the name alone.
    pub fn sub_region_display(&self) -> String {
        if self.sub_region_name.is_empty() || self.sub_region_division.is_empty() {
            return self.sub_region_name.clone();
        }
        format!("{} {}", self.sub_region_name, self.sub_region_division)
    }

    /// This address as the modeled `Address` structure, with the assembled `Label`
    /// the place computed and the street split into `StreetComponents`.
    ///
    /// A component this catalog does not hold is ABSENT, not an empty string:
    /// every `Address` member is optional, so "this place has no postal code" is a
    /// missing member and `""` would be a postal code of no characters.
    pub fn to_modeled(&self, label: String) -> Address {
        let present = |value: &str| (!value.is_empty()).then(|| value.to_string());
        let country = Country {
            code2: present(&self.country_code2),
            code3: present(&self.country_code3),
            name: present(&self.country_name),
        };
        let region = Region {
            code: present(&self.region_code),
            name: present(&self.region_name),
        };
        let sub_region = SubRegion {
            code: None,
            name: present(&self.sub_region_name),
        };
        Address {
            label: present(&label),
            country: (country.code2.is_some() || country.name.is_some()).then_some(country),
            region: (region.code.is_some() || region.name.is_some()).then_some(region),
            sub_region: sub_region.name.is_some().then_some(sub_region),
            locality: present(&self.locality),
            district: self.district.clone(),
            postal_code: present(&self.postal_code),
            street: self.street.clone(),
            street_components: self.street_components_modeled(),
            address_number: self.address_number.clone(),
            ..Address::default()
        }
    }

    /// `Address.StreetComponents` — "Components of the street" — parsed out of the
    /// street name: the base name, the street type and the side of the base name
    /// the regional convention places it on (after it in English, before it in
    /// French), the separator between them, and the directional prefix or suffix
    /// that is not part of the base name.
    pub fn street_components_modeled(&self) -> Option<Vec<StreetComponents>> {
        let street = self.street.as_deref().filter(|street| !street.is_empty())?;
        let mut words: Vec<&str> = street.split_whitespace().collect();
        let normalized = |word: &str| normalize_tokens(word).join("");

        let mut prefix = None;
        let mut suffix = None;
        if words.len() > 1 && is_directional(&normalized(words[0])) {
            prefix = Some(words.remove(0).to_string());
        }
        if words.len() > 1 && words.last().is_some_and(|word| is_directional(&normalized(word))) {
            suffix = words.pop().map(str::to_string);
        }

        let mut street_type = None;
        let mut type_placement = None;
        if words.len() > 1 && words.last().is_some_and(|word| is_street_type(&normalized(word))) {
            street_type = words.pop().map(str::to_string);
            type_placement = Some(TYPE_PLACEMENT_AFTER.to_string());
        } else if words.len() > 1 && is_street_type(&normalized(words[0])) {
            street_type = Some(words.remove(0).to_string());
            type_placement = Some(TYPE_PLACEMENT_BEFORE.to_string());
        }

        let base_name = words.join(" ");
        // The modeled `TypeSeparator` is `""` or `" "`; this catalog's street
        // names separate the type from the base name with a space.
        let type_separator = street_type.as_ref().map(|_| " ".to_string());
        Some(vec![StreetComponents {
            base_name: (!base_name.is_empty()).then_some(base_name),
            r#type: street_type,
            type_placement,
            type_separator,
            prefix,
            suffix,
            direction: None,
            language: (!self.language.is_empty()).then(|| self.language.clone()),
        }])
    }

    /// The texts a `GeocodeQueryComponents` member may match against, named by that
    /// member. A component the catalog does not carry matches nothing — an absent
    /// component holds no text, so it cannot answer a query word.
    ///
    /// A component is matchable by EVERY name the entry holds for it: its own text,
    /// the text of each language variant, and — for the sub-region — both its bare
    /// name and the display name its division word makes, so a caller may name the
    /// county either way.
    pub fn component_texts(&self, component: &str) -> Vec<String> {
        let mut texts: Vec<String> = self.component_texts_of(self, component);
        if component == "SubRegion" {
            texts.push(self.sub_region_display());
        }
        for tag in self.names_by_language.keys() {
            let localized = self.rendered_in(Some(tag));
            texts.extend(self.component_texts_of(&localized, component));
            if component == "SubRegion" {
                texts.push(localized.sub_region_display());
            }
        }
        texts.retain(|text| !text.is_empty());
        texts.sort();
        texts.dedup();
        texts
    }

    /// The text ONE rendering of this address holds for a component.
    fn component_texts_of(&self, rendered: &Self, component: &str) -> Vec<String> {
        let texts: Vec<&str> = match component {
            "Country" => vec![
                rendered.country_code2.as_str(),
                rendered.country_code3.as_str(),
                rendered.country_name.as_str(),
            ],
            "Region" => vec![rendered.region_code.as_str(), rendered.region_name.as_str()],
            "SubRegion" => vec![rendered.sub_region_name.as_str()],
            "Locality" => vec![rendered.locality.as_str()],
            "District" => rendered.district.as_deref().into_iter().collect(),
            "Street" => rendered.street.as_deref().into_iter().collect(),
            "AddressNumber" => rendered.address_number.as_deref().into_iter().collect(),
            "PostalCode" => vec![rendered.postal_code.as_str()],
            _ => Vec::new(),
        };
        texts
            .into_iter()
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// How well a supplied structured component answers this address's own
    /// component, or `None` when it does not answer it at all.
    pub fn component_affinity(&self, component: &str, value: &str) -> Option<f64> {
        let wanted = normalize_tokens(value);
        if wanted.is_empty() {
            return None;
        }
        self.component_texts(component)
            .iter()
            .filter_map(|text| text_affinity(&normalize_tokens(text), &wanted))
            .fold(None, |best: Option<f64>, score| {
                Some(best.map_or(score, |best| best.max(score)))
            })
    }

    /// `[address number] [street]`, the street line of a postal address, or `None`
    /// when the address names no street.
    fn street_line(&self) -> Option<String> {
        let street = self.street.as_deref().filter(|street| !street.is_empty())?;
        Some(
            match self
                .address_number
                .as_deref()
                .filter(|number| !number.is_empty())
            {
                Some(number) => format!("{number} {street}"),
                None => street.to_string(),
            },
        )
    }
}

/// The assembled `Address$Label`: "Assembled address value built out of the address
/// components, according to the regional postal rules. This is the correctly
/// formatted address", led by the place's own `name` where it has one, as a point
/// of interest does.
///
/// Two conventions, chosen by the address's country:
///
/// * US — the state abbreviation and the ZIP code ride the settlement line:
///   `[name, ][number street, ]settlement, STATE[ ZIP], country`. The ZIP belongs
///   to a street address, so a city reads "Seattle, WA, United States" while an
///   address in it reads "400 Broad St, Seattle, WA 98109, United States".
/// * Elsewhere — the postal code precedes the settlement, and the region is named
///   only where the address has no street line:
///   `[name, ][number street, ][postal ]settlement, country`.
///
/// Either convention skips a component the address does not hold rather than
/// rendering it empty, so a place reads at its OWN level: a country as "France", a
/// sub-region as "King County, WA, United States" — a place that IS the sub-region
/// is named by the sub-region's DISPLAY name ([`AddressRecord::sub_region_display`]),
/// division word and all, while the `SubRegion` component itself stays the bare
/// name the model documents. The district is part of the label only for a place
/// that IS a district — for a business or an address it is finer detail than the
/// postal address needs.
fn assemble_label(address: &AddressRecord, name: Option<&str>, place_type: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        parts.push(name.to_string());
    }
    let street_line = address.street_line();
    if let Some(line) = street_line.clone() {
        parts.push(line);
    }

    // The settlement: the finest level at or above this place's own granularity
    // that the address names, below the region.
    let mut settlement: Vec<String> = Vec::new();
    if place_type == PLACE_TYPE_DISTRICT {
        settlement.extend(address.district.clone());
    }
    settlement.extend(
        [address.locality.clone(), address.sub_region_display()]
            .into_iter()
            .find(|level| !level.is_empty()),
    );
    settlement.retain(|level| !level.is_empty());

    let us_style = address.country_code2 == "US";
    let settlement_line = if us_style {
        // Beside a finer level the region is written as its code, as its full name
        // where the region is the finest level the address names.
        let region = if settlement.is_empty() {
            address.region_name.as_str()
        } else {
            address.region_code.as_str()
        };
        let mut line = settlement
            .iter()
            .map(String::as_str)
            .chain([region])
            .filter(|level| !level.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        if street_line.is_some() && !address.postal_code.is_empty() && !line.is_empty() {
            line = format!("{line} {}", address.postal_code);
        }
        line
    } else if street_line.is_some() && !address.postal_code.is_empty() && !settlement.is_empty() {
        format!("{} {}", address.postal_code, settlement.join(", "))
    } else {
        settlement
            .iter()
            .map(String::as_str)
            .chain([address.region_name.as_str()])
            .filter(|level| !level.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !settlement_line.is_empty() {
        parts.push(settlement_line);
    }
    if !address.country_name.is_empty() {
        parts.push(address.country_name.clone());
    }
    parts.join(", ")
}

/// One of a place's categories: the modeled `Category`.
#[derive(Debug, Clone)]
pub struct CategoryRecord {
    pub id: String,
    pub name: String,
    pub primary: bool,
}

/// A place's time zone: the modeled `TimeZone`.
#[derive(Debug, Clone)]
pub struct TimeZoneRecord {
    pub name: String,
    pub offset: String,
    pub offset_seconds: i64,
}

/// One place in the catalog — the single source of truth every operation projects.
#[derive(Debug, Clone)]
pub struct PlaceRecord {
    /// Opaque, stable identifier; see [`synthesize_place_id`].
    pub place_id: String,
    /// A `PlaceType` enum value.
    pub place_type: String,
    pub title: String,
    /// The place's other names — a name in another language, or the name it is
    /// commonly searched by. A place is findable by ANY of its names, so a query
    /// in one language answers with the record written in another.
    pub alternate_names: Vec<String>,
    /// The place's OWN name as other languages write it, keyed by BCP 47 language
    /// tag: what a request's `Language` selects for the `Title` ("the localized
    /// display name of this result item based on request parameter language"),
    /// with the name in [`AddressRecord::language`] as the fallback.
    ///
    /// A place is findable by every one of these too, the way it is by its
    /// [`Self::alternate_names`].
    pub titles_by_language: BTreeMap<String, String>,
    pub address: AddressRecord,
    /// The attribution this place carries under a political view, keyed by the
    /// view's alpha-3 country code: "the political view applies to the results of
    /// the request to represent unresolved territorial claims through the point of
    /// view of the specified country" (`CountryCode`). A place over which no
    /// claim is unresolved carries none of these, so every view reports the same
    /// [`Self::address`].
    pub political_attributions: BTreeMap<String, AddressRecord>,
    /// `[longitude, latitude]`, WGS 84, as the modeled `Position` orders them, at
    /// the precision the service reports a coordinate to
    /// ([`COORDINATE_DECIMALS`]).
    pub position: [f64; 2],
    /// The box enclosing the geometric shape this place covers: `[west, south,
    /// east, north]`, as the modeled `BoundingBox` orders them.
    ///
    /// It is this place's OWN extent — an area's real bounding box, a point
    /// feature's viewport ([`point_view`]) — so it is neither derived from the
    /// place's `PlaceType` nor symmetric about [`Self::position`].
    pub map_view: [f64; 4],
    pub categories: Vec<CategoryRecord>,
    pub food_types: Vec<String>,
    pub business_chains: Vec<String>,
    pub phones: Vec<String>,
    pub websites: Vec<String>,
    /// `OpeningHours$Display` lines.
    pub opening_hours: Vec<String>,
    pub access_points: Vec<[f64; 2]>,
    pub time_zone: Option<TimeZoneRecord>,
    /// The normalized tokens free-form text queries match against, derived from the
    /// record's own text by [`index_terms`].
    pub search_terms: Vec<String>,
    /// The normalized tokens a partially typed query is completed against, derived
    /// from the text this record REPORTS by [`text_terms`].
    pub completion_terms: Vec<String>,
    /// How many OTHER records of the catalog this place's extent covers, computed
    /// once when the catalog is seeded.
    ///
    /// It is a measure of how much this place anchors: a well-known city holds
    /// landmarks, addresses and districts, while its lesser-known homonym holds
    /// nothing. Used ONLY to order two places a query matched equally well and
    /// that are equally near the request's reference position — the order in which
    /// a client expects the better-known of two same-named places first. Never
    /// reported.
    pub places_covered: usize,
}

impl PlaceRecord {
    /// A catalog record for `title` at `position`.
    ///
    /// The record's `MapView` starts as the viewport of a POINT feature
    /// ([`point_view`]); a place with real area carries its own box instead, named
    /// beside it with [`Self::with_map_view`].
    fn new(
        title: &str,
        place_type: &str,
        position: [f64; 2],
        address: AddressRecord,
    ) -> Self {
        let position = coordinate_pair(position);
        Self {
            place_id: synthesize_place_id(title, position),
            place_type: place_type.to_string(),
            title: title.to_string(),
            alternate_names: Vec::new(),
            titles_by_language: BTreeMap::new(),
            address,
            political_attributions: BTreeMap::new(),
            position,
            map_view: coordinate_box(point_view(position)),
            categories: Vec::new(),
            food_types: Vec::new(),
            business_chains: Vec::new(),
            phones: Vec::new(),
            websites: Vec::new(),
            opening_hours: Vec::new(),
            access_points: Vec::new(),
            time_zone: None,
            search_terms: Vec::new(),
            completion_terms: Vec::new(),
            places_covered: 0,
        }
    }

    /// The box enclosing the area this place covers, as `[west, south, east,
    /// north]`: the geometry of a feature with real extent — a district, a
    /// settlement, a county, a state, a country — which is neither square nor
    /// centred on the place's own position.
    fn with_map_view(mut self, map_view: [f64; 4]) -> Self {
        self.map_view = coordinate_box(map_view);
        self
    }

    /// The place's other names: see [`PlaceRecord::alternate_names`].
    fn with_alternate_names(mut self, names: &[&str]) -> Self {
        self.alternate_names = names.iter().map(|name| (*name).to_string()).collect();
        self
    }

    /// The place's own name in other languages, as `(BCP 47 tag, name)` pairs:
    /// see [`PlaceRecord::titles_by_language`].
    fn with_localized_titles(mut self, titles: &[(&str, &str)]) -> Self {
        self.titles_by_language = titles
            .iter()
            .map(|(tag, name)| ((*tag).to_string(), (*name).to_string()))
            .collect();
        self
    }

    /// The attribution this place carries under one political view, named by that
    /// view's alpha-3 country code: see [`PlaceRecord::political_attributions`].
    fn with_political_attribution(mut self, view: &str, address: AddressRecord) -> Self {
        self.political_attributions.insert(view.to_string(), address);
        self
    }

    /// Categories, the first of which is the place's primary one.
    fn with_categories(mut self, categories: &[(&str, &str)]) -> Self {
        self.categories = categories
            .iter()
            .enumerate()
            .map(|(rank, (id, name))| CategoryRecord {
                id: (*id).to_string(),
                name: (*name).to_string(),
                primary: rank == 0,
            })
            .collect();
        self
    }

    fn with_food_types(mut self, food_types: &[&str]) -> Self {
        self.food_types = food_types.iter().map(|name| (*name).to_string()).collect();
        self
    }

    fn with_business_chains(mut self, chains: &[&str]) -> Self {
        self.business_chains = chains.iter().map(|name| (*name).to_string()).collect();
        self
    }

    fn with_contacts(mut self, phones: &[&str], websites: &[&str]) -> Self {
        self.phones = phones.iter().map(|value| (*value).to_string()).collect();
        self.websites = websites.iter().map(|value| (*value).to_string()).collect();
        self
    }

    fn with_opening_hours(mut self, display: &[&str]) -> Self {
        self.opening_hours = display.iter().map(|line| (*line).to_string()).collect();
        self
    }

    fn with_access_points(mut self, access_points: &[[f64; 2]]) -> Self {
        self.access_points = access_points
            .iter()
            .map(|point| coordinate_pair(*point))
            .collect();
        self
    }

    fn with_time_zone(mut self, name: &str, offset: &str, offset_seconds: i64) -> Self {
        self.time_zone = Some(TimeZoneRecord {
            name: name.to_string(),
            offset: offset.to_string(),
            offset_seconds,
        });
        self
    }

    /// The place's time zone, named as one of this module's `(name, offset,
    /// offset seconds)` zone constants.
    fn in_time_zone(self, zone: (&str, &str, i64)) -> Self {
        self.with_time_zone(zone.0, zone.1, zone.2)
    }

    /// The place's OWN name, for a place that has one: a point of interest is a
    /// named business or landmark. An address or an administrative area has no
    /// name of its own — what names it is its assembled address label.
    pub fn name(&self) -> Option<&str> {
        (self.place_type == PLACE_TYPE_POINT_OF_INTEREST).then_some(self.title.as_str())
    }

    /// The place's own name as one request renders it: the localized name the tag
    /// that request resolves to selects ([`PlaceRecord::name_language`]), or the
    /// record's own where it holds none in that language.
    pub fn name_in<'a>(&'a self, language: ResultLanguage<'a>) -> Option<&'a str> {
        let name = self.name()?;
        Some(
            lookup_language(&self.titles_by_language, self.name_language(language))
                .map_or(name, |(_, localized)| localized.as_str()),
        )
    }

    /// The BCP 47 language tag this place's own name is rendered under for one
    /// request: the `Language` it named, and otherwise the language that request's
    /// query named the place in ([`PlaceRecord::name_language_named_by`]).
    pub fn name_language<'a>(&'a self, language: ResultLanguage<'a>) -> Option<&'a str> {
        language
            .requested()
            .or_else(|| self.name_language_named_by(language.query))
    }

    /// The language a free-form query named this place in: the BCP 47 tag of the
    /// localized name the query names BETTER than any name the entry writes in its
    /// own language, or `None` when none of them does — in which case the query
    /// called the place what the entry itself calls it, or named no name of it at
    /// all.
    ///
    /// How much of a name a query names is [`named_share`]. The strictly-better
    /// test is what keeps a query that names the entry's own name as well as
    /// another language's answering in the entry's own — a query naming one word
    /// several languages spell the same way names no language in particular.
    pub fn name_language_named_by(&self, query_tokens: &[String]) -> Option<&str> {
        let own = std::iter::once(self.title.as_str())
            .chain(self.alternate_names.iter().map(String::as_str))
            .map(|name| named_share(name, query_tokens))
            .fold(0.0_f64, f64::max);
        self.titles_by_language
            .iter()
            .map(|(tag, localized)| (tag.as_str(), named_share(localized, query_tokens)))
            .filter(|(_, share)| *share > own)
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(tag, _)| tag)
    }

    /// Every name this place is KNOWN by: its own title, its localized titles and
    /// its alternate names.
    ///
    /// Unlike [`PlaceRecord::name`], which is the name that LEADS a postal label
    /// and so belongs to a point of interest alone, these are the names a query
    /// may call the place by — a locality's name is its own name too, even though
    /// its label does not repeat it.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.title.as_str())
            .chain(self.alternate_names.iter().map(String::as_str))
            .chain(self.titles_by_language.values().map(String::as_str))
    }

    /// The names a postal label may lead with: a point of interest's own name, in
    /// every language it holds one, and its alternates.
    fn label_names(&self) -> Vec<&str> {
        match self.name() {
            Some(name) => std::iter::once(name)
                .chain(self.alternate_names.iter().map(String::as_str))
                .chain(self.titles_by_language.values().map(String::as_str))
                .collect(),
            None => Vec::new(),
        }
    }

    /// This place's address under a political view: the attribution that view
    /// gives it, or its default attribution when that view leaves it unchanged
    /// (which is every view, for a place no claim is unresolved over).
    pub fn address_in(&self, political_view: Option<&str>) -> &AddressRecord {
        political_view
            .and_then(|view| self.political_attributions.get(view))
            .unwrap_or(&self.address)
    }

    /// This place's address under a political view, rendered in the requested
    /// language: the ONE place a reported `Address` is resolved from.
    pub fn address_rendered_in(
        &self,
        political_view: Option<&str>,
        language: ResultLanguage<'_>,
    ) -> AddressRecord {
        self.address_in(political_view).rendered_in(language.requested())
    }

    /// This place's assembled address label: see [`assemble_label`].
    pub fn assembled_label(&self) -> String {
        self.assembled_label_in(None, ResultLanguage::default())
    }

    /// This place's assembled address label under a political view, rendered in
    /// the requested language — led, for a place that has a name of its own, by
    /// that name as this request renders it.
    pub fn assembled_label_in<'a>(
        &'a self,
        political_view: Option<&str>,
        language: ResultLanguage<'a>,
    ) -> String {
        assemble_label(
            &self.address_rendered_in(political_view, language),
            self.name_in(language),
            &self.place_type,
        )
    }

    /// `Title`: "The localized display name of this result item"
    /// (`GeocodeResultItem$Title`) — the place's own name, or, for an address or an
    /// administrative area, the assembled address label that names it. A client
    /// shows this as the single display line for the place, so a bare address
    /// component is not one.
    pub fn title_modeled(&self) -> String {
        self.title_modeled_in(None, ResultLanguage::default())
    }

    /// `Title` under a political view, rendered as this request renders text: a
    /// label that names a country names the one that view attributes the place to,
    /// and the place's own name is written in the language the request resolves to
    /// ([`PlaceRecord::name_language`]).
    pub fn title_modeled_in<'a>(
        &'a self,
        political_view: Option<&str>,
        language: ResultLanguage<'a>,
    ) -> String {
        self.name_in(language).map_or_else(
            || self.assembled_label_in(political_view, language),
            str::to_string,
        )
    }

    /// `Address`, with the assembled `Label` this place renders.
    pub fn address_modeled(&self) -> Address {
        self.address_modeled_in(None, ResultLanguage::default())
    }

    /// `Address` under a political view, rendered in the requested language.
    pub fn address_modeled_in<'a>(
        &'a self,
        political_view: Option<&str>,
        language: ResultLanguage<'a>,
    ) -> Address {
        self.address_rendered_in(political_view, language)
            .to_modeled(self.assembled_label_in(political_view, language))
    }

    /// Does this place COVER `position` — does the point fall inside the place's
    /// own extent, the `MapView` "bounding box of the place" a result reports?
    pub fn covers(&self, position: [f64; 2]) -> bool {
        box_contains(self.map_view, position)
    }

    /// `Position`, as a modeled `Position` list, at the precision the service
    /// reports a coordinate to ([`COORDINATE_DECIMALS`]).
    pub fn position_modeled(&self) -> Vec<f64> {
        coordinate_pair(self.position).to_vec()
    }

    /// `MapView`, as a modeled `BoundingBox` list, at the precision the service
    /// reports a coordinate to ([`COORDINATE_DECIMALS`]).
    pub fn map_view_modeled(&self) -> Vec<f64> {
        coordinate_box(self.map_view).to_vec()
    }

    /// `Categories`, or `None` when this place has none: the modeled list is
    /// `len 1..100`, so an empty list is not a legal value.
    pub fn categories_modeled(&self) -> Option<Vec<Category>> {
        if self.categories.is_empty() {
            return None;
        }
        Some(
            self.categories
                .iter()
                .map(|category| Category {
                    id: category.id.clone(),
                    name: category.name.clone(),
                    localized_name: Some(category.name.clone()),
                    primary: Some(category.primary),
                })
                .collect(),
        )
    }

    /// `FoodTypes`, or `None` when this place has none (modeled `len 1..100`).
    pub fn food_types_modeled(&self) -> Option<Vec<FoodType>> {
        if self.food_types.is_empty() {
            return None;
        }
        Some(
            self.food_types
                .iter()
                .map(|name| FoodType {
                    localized_name: name.clone(),
                    id: Some(normalize_tokens(name).join("_")),
                    primary: Some(true),
                })
                .collect(),
        )
    }

    /// `BusinessChains`, or `None` when this place belongs to none
    /// (modeled `len 1..100`).
    pub fn business_chains_modeled(&self) -> Option<Vec<BusinessChain>> {
        if self.business_chains.is_empty() {
            return None;
        }
        // The `Id` is the chain identifier AS STORED. Case-folding it would hand
        // out an id that no longer compares equal to the one the catalog holds,
        // and chain filtering compares ids as strings.
        Some(
            self.business_chains
                .iter()
                .map(|name| BusinessChain {
                    name: Some(name.clone()),
                    id: Some(name.clone()),
                })
                .collect(),
        )
    }

    /// `Contacts`, or `None` when this place has none recorded.
    pub fn contacts_modeled(&self) -> Option<Contacts> {
        if self.phones.is_empty() && self.websites.is_empty() {
            return None;
        }
        let details = |values: &[String]| {
            if values.is_empty() {
                None
            } else {
                Some(
                    values
                        .iter()
                        .map(|value| ContactDetails {
                            label: Some(self.title.clone()),
                            value: Some(value.clone()),
                            categories: self.categories_modeled(),
                        })
                        .collect::<Vec<_>>(),
                )
            }
        };
        Some(Contacts {
            phones: details(&self.phones),
            faxes: None,
            websites: details(&self.websites),
            emails: None,
        })
    }

    /// `OpeningHours`, or `None` when this place publishes none
    /// (modeled `len 1..100`).
    pub fn opening_hours_modeled(&self) -> Option<Vec<OpeningHours>> {
        if self.opening_hours.is_empty() {
            return None;
        }
        Some(vec![OpeningHours {
            display: Some(self.opening_hours.clone()),
            open_now: None,
            components: None,
            categories: self.categories_modeled(),
        }])
    }

    /// `AccessPoints`, or `None` when this place has none recorded. Each position
    /// is reported at the precision the service reports a coordinate to
    /// ([`COORDINATE_DECIMALS`]).
    pub fn access_points_modeled(&self) -> Option<Vec<AccessPoint>> {
        if self.access_points.is_empty() {
            return None;
        }
        Some(
            self.access_points
                .iter()
                .enumerate()
                .map(|(rank, point)| AccessPoint {
                    position: Some(coordinate_pair(*point).to_vec()),
                    r#type: None,
                    primary: Some(rank == 0),
                    label: None,
                })
                .collect(),
        )
    }

    /// `TimeZone`, or `None` when this place has none recorded.
    pub fn time_zone_modeled(&self) -> Option<TimeZone> {
        self.time_zone.as_ref().map(|zone| TimeZone {
            name: zone.name.clone(),
            offset: Some(zone.offset.clone()),
            offset_seconds: zone.offset_seconds,
        })
    }

    /// How well a free-form query matches this record, or `None` when it does not.
    ///
    /// A record matches when EVERY token of the normalized query [matches] one of
    /// the record's indexed terms. The score is the share of tokens that matched a
    /// whole term, counting an abbreviated one as half, so a query whose every token
    /// is recognized in full scores 1.0.
    ///
    /// [matches]: affinity
    pub fn text_score(&self, query_tokens: &[String]) -> Option<f64> {
        score_against(&self.search_terms, query_tokens)
    }

    /// How well a partially typed query COMPLETES to this record, or `None` when it
    /// does not complete to it. Scored like [`PlaceRecord::text_score`].
    ///
    /// The terms are the record's own reported text alone — the names it is known
    /// by and its address — and not the category vocabulary a text search also
    /// indexes: Autocomplete "completes potential places and addresses as the user
    /// types" and "enhances the efficiency and accuracy of address by completing
    /// query based on a few entered keystrokes", while it is Suggest that is
    /// documented to answer "relevant places, points of interest, query terms or
    /// search category". So a completion is always a completion of text the result
    /// itself spells, which is what lets it mark the sections that matched.
    pub fn completion_score(&self, query_tokens: &[String]) -> Option<f64> {
        score_against(&self.completion_terms, query_tokens)
    }

    /// The share of a free-form query that named this place's OWN name — its title
    /// or one of its alternate names — counting an abbreviated word as half.
    ///
    /// This is NOT the reported `MatchScore`: it ranks a place the query NAMED
    /// above a place that merely sits in an area the query named ("Washington" the
    /// city and the state, rather than every address inside them), which is the
    /// order a client asking for a place expects. A query that names none of the
    /// place's names scores 0 here and is still a match on the other terms.
    pub fn name_score(&self, query_tokens: &[String]) -> f64 {
        if query_tokens.is_empty() {
            return 0.0;
        }
        let name_terms: Vec<String> = self
            .names()
            .flat_map(normalize_tokens)
            .collect();
        let matched: f64 = query_tokens
            .iter()
            .map(|token| {
                name_terms
                    .iter()
                    .filter_map(|term| affinity(term, token))
                    .fold(0.0, f64::max)
            })
            .sum();
        matched / query_tokens.len() as f64
    }

    /// How well a structured query matches this record, or `None` when any supplied
    /// component does not match the corresponding address component. Scored like
    /// [`PlaceRecord::text_score`]: a component matched in full counts 1, an
    /// abbreviated one counts half.
    pub fn component_score(&self, components: &[(&str, String)]) -> Option<f64> {
        if components.is_empty() {
            return None;
        }
        let mut matched = 0.0;
        for (component, value) in components {
            matched += self.address.component_affinity(component, value)?;
        }
        Some(matched / components.len() as f64)
    }

    /// How one geocode query matched this place, component by component: the
    /// free-form text is located inside the place's own name and address
    /// components, and every structured component the caller supplied is scored
    /// against the address component it names and echoed as parsed.
    ///
    /// The query arrives already tokenized ([`QueryTokens`]), so a request
    /// explaining its match against many candidates tokenizes it once.
    ///
    /// This is the data behind `MatchScores` ("Indicates how well the entire input
    /// matches the returned. It is equal to 1 if all input tokens are recognized
    /// and matched") and `ParsedQuery` ("Parsed components in the provided
    /// QueryText"): a component recognized in full scores 1, one recognized from a
    /// leading part of a word scores half, and a component the query never named is
    /// absent, which `MatchScore`'s modeled default reports as 0.
    pub fn explain_match(
        &self,
        overall: f64,
        query: &QueryTokens<'_>,
        components: &[(&'static str, String)],
    ) -> MatchExplanation {
        let mut explanation = MatchExplanation {
            overall,
            ..MatchExplanation::default()
        };
        if !query.is_empty() {
            let located_name = self
                .label_names()
                .into_iter()
                .filter_map(|name| locate(query, name))
                .max_by(|left, right| left.score.total_cmp(&right.score));
            if let Some(found) = located_name {
                explanation.title = Some(ComponentMatch {
                    component: TITLE_COMPONENT,
                    score: found.score,
                    start_index: found.start_index,
                    end_index: found.end_index,
                    value: found.value,
                    query_component: QUERY_COMPONENT_FREE_TEXT.to_string(),
                });
            }
            for component in ADDRESS_COMPONENTS {
                let best = self
                    .address
                    .component_texts(component)
                    .into_iter()
                    .filter_map(|text| locate(query, &text))
                    .max_by(|left, right| left.score.total_cmp(&right.score));
                if let Some(found) = best {
                    explanation.components.push(ComponentMatch {
                        component,
                        score: found.score,
                        start_index: found.start_index,
                        end_index: found.end_index,
                        value: found.value,
                        query_component: QUERY_COMPONENT_FREE_TEXT.to_string(),
                    });
                }
            }
        }
        for (component, value) in components {
            let Some(score) = self.address.component_affinity(component, value) else {
                continue;
            };
            explanation.components.push(ComponentMatch {
                component,
                score,
                start_index: 0,
                end_index: value.chars().count() as i32,
                value: value.clone(),
                query_component: (*component).to_string(),
            });
        }
        explanation
    }

    /// Is this place in one of `codes` (alpha-2 or alpha-3 country codes)?
    pub fn in_countries(&self, codes: &[String]) -> bool {
        codes.iter().any(|code| {
            code.eq_ignore_ascii_case(&self.address.country_code2)
                || code.eq_ignore_ascii_case(&self.address.country_code3)
        })
    }

    /// Is this place's `PlaceType` one of `place_types`?
    pub fn has_place_type(&self, place_types: &[String]) -> bool {
        place_types
            .iter()
            .any(|place_type| place_type.eq_ignore_ascii_case(&self.place_type))
    }

    /// Does this place carry one of the named categories (by id or by name)?
    pub fn has_category(&self, categories: &[String]) -> bool {
        categories.iter().any(|wanted| {
            self.categories.iter().any(|category| {
                wanted.eq_ignore_ascii_case(&category.id)
                    || wanted.eq_ignore_ascii_case(&category.name)
            })
        })
    }

    /// Does this place offer one of the named food types?
    pub fn has_food_type(&self, food_types: &[String]) -> bool {
        food_types.iter().any(|wanted| {
            self.food_types
                .iter()
                .any(|food_type| wanted.eq_ignore_ascii_case(food_type))
        })
    }

    /// Does this place belong to one of the named business chains?
    pub fn has_business_chain(&self, chains: &[String]) -> bool {
        chains.iter().any(|wanted| {
            self.business_chains
                .iter()
                .any(|chain| wanted.eq_ignore_ascii_case(chain))
        })
    }

    /// Is this place's position inside `[west, south, east, north]`?
    pub fn within_bounding_box(&self, box_: [f64; 4]) -> bool {
        box_contains(box_, self.position)
    }
}

/// Is `position` inside `[west, south, east, north]`? A box whose west edge is
/// east of its east edge crosses the antimeridian.
///
/// The ONE containment test: a place's position inside a filter's box, and a
/// queried position inside a place's own extent, are the same question asked of
/// different boxes.
pub fn box_contains(box_: [f64; 4], position: [f64; 2]) -> bool {
    let [west, south, east, north] = box_;
    let [longitude, latitude] = position;
    let within_longitude = if west <= east {
        longitude >= west && longitude <= east
    } else {
        longitude >= west || longitude <= east
    };
    within_longitude && latitude >= south && latitude <= north
}

/// The version header every place handle this service hands out opens with: the
/// handle's leading bytes, which come out of [`place_id_token`] as the fixed
/// character prefix a client sees on every id.
///
/// The header is a whole number of 3-byte base64 groups, so it encodes to a prefix
/// that does not shift with the payload behind it.
const PLACE_ID_VERSION: [u8; 6] = [0x01, 0x00, 0x00, 0x00, 0x38, 0x00];

/// Bytes of per-place payload a place handle carries behind its
/// [`PLACE_ID_VERSION`] header. Header plus payload encode to a 126-character
/// token, well inside the modeled `PlaceId` `length(0, 500)`.
const PLACE_ID_PAYLOAD_BYTES: usize = 88;

/// `bytes` as one unpadded base64url token, so the whole handle is URL- and
/// path-safe — a `PlaceId` travels in `GET /v2/place/{PlaceId}`.
///
/// The encoding is the runtime's own base64 encoder; base64url (RFC 4648 section
/// 5, "base64url") is that same encoding over an alphabet whose last two
/// characters are `-` and `_` instead of `+` and `/`, written without the `=`
/// padding, which is the whole of the adjustment made here.
fn place_id_token(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// The opaque `PlaceId` of a record: the service's version header followed by a
/// payload derived from what the record IS (its title and its position), rendered
/// as one opaque base64url token of a fixed length class.
///
/// The handle is a HANDLE: the model documents nothing about its content
/// (`SensitiveString`, `length(0, 500)`), and a client reads it as an opaque token
/// it persists, logs, de-duplicates on and hands back to GetPlace. Deriving the
/// payload from the record keeps it stable across runs, so one operation's id still
/// resolves in another, while the header and the length class make every id the
/// same shape whatever place it names.
fn synthesize_place_id(title: &str, position: [f64; 2]) -> String {
    let seed = format!("{title}|{:.6}|{:.6}", position[0], position[1]);
    let mut bytes = PLACE_ID_VERSION.to_vec();
    // The payload is longer than one digest, so it is expanded a digest at a time,
    // each block keyed by its own index.
    let mut block: u32 = 0;
    while bytes.len() < PLACE_ID_VERSION.len() + PLACE_ID_PAYLOAD_BYTES {
        let mut digest = Sha256::new();
        digest.update(seed.as_bytes());
        digest.update(block.to_be_bytes());
        bytes.extend_from_slice(&digest.finalize());
        block += 1;
    }
    bytes.truncate(PLACE_ID_VERSION.len() + PLACE_ID_PAYLOAD_BYTES);
    place_id_token(&bytes)
}

/// How well a query matches a set of indexed terms, or `None` when it does not
/// match them at all: the shared body of [`PlaceRecord::text_score`] and
/// [`PlaceRecord::completion_score`], which differ only in the terms they match
/// against.
fn score_against(terms: &[String], query_tokens: &[String]) -> Option<f64> {
    if query_tokens.is_empty() {
        return None;
    }
    let mut matched = 0.0;
    for token in query_tokens {
        matched += terms
            .iter()
            .filter_map(|term| affinity(term, token))
            .fold(f64::NAN, f64::max);
        if matched.is_nan() {
            return None;
        }
    }
    Some(matched / query_tokens.len() as f64)
}

/// How much of one of a place's NAMES a query named: the share of that name's
/// words the query accounted for, counting a word named only by its leading
/// characters, or by a standard abbreviation, as half ([`affinity`]).
///
/// Unlike [`text_affinity`], a query word the name accounts for nothing of costs
/// the name nothing — a query naming a landmark and the city around it still names
/// the landmark's name in full — and unlike [`locate`], the name's words need not
/// sit together in the query. Dividing by the NAME's length is what makes two
/// names comparable: a query naming one word of a two-word name names half of it,
/// whichever language writes it.
fn named_share(name: &str, query_tokens: &[String]) -> f64 {
    let words = normalize_tokens(name);
    if words.is_empty() || query_tokens.is_empty() {
        return 0.0;
    }
    let named: f64 = words
        .iter()
        .map(|word| {
            query_tokens
                .iter()
                .filter_map(|token| affinity(word, token))
                .fold(0.0, f64::max)
        })
        .sum();
    named / words.len() as f64
}

/// The normalized tokens of the text a record REPORTS: every name it is known by,
/// its assembled label and each component of its address.
///
/// Every LANGUAGE the entry holds text in IS indexed: a place is findable by the
/// name any language gives it, and the result then renders in the language the
/// request asked for, so a query in one language never answers with a name the
/// response does not carry.
///
/// A political view's alternative attribution is NOT indexed: a result is reported
/// under the attribution the request's view selects, and a query answered with an
/// attribution it did not ask for would name a country the response then
/// contradicts.
fn text_terms(place: &PlaceRecord) -> Vec<String> {
    let label = place.assembled_label();
    let mut sources: Vec<String> = place.names().map(str::to_string).collect();
    sources.push(label);
    sources.extend(
        [
            place.address.locality.as_str(),
            place.address.region_code.as_str(),
            place.address.region_name.as_str(),
            place.address.country_name.as_str(),
            place.address.country_code2.as_str(),
            place.address.country_code3.as_str(),
            place.address.postal_code.as_str(),
        ]
        .into_iter()
        .map(str::to_string),
    );
    // The sub-region under both the bare name the `SubRegion` component reports
    // and the display name its division word makes.
    sources.push(place.address.sub_region_name.clone());
    sources.push(place.address.sub_region_display());
    sources.extend(place.address.street.clone());
    sources.extend(place.address.district.clone());
    // Every language the entry holds text in: an entry written in one language is
    // still findable by the name another language gives it, which is what its
    // alternate names do for a place's own name.
    for names in place.address.names_by_language.values() {
        sources.extend(names.given().map(str::to_string));
    }
    normalized_terms(&sources)
}

/// The normalized tokens a record is found by: the text it reports
/// ([`text_terms`]), its categories, its food types and its business chains.
///
/// The CATEGORIES and FOOD TYPES are indexed because a category word is a query
/// term: a large part of what a text search is asked is "find me a coffee shop",
/// which names what the place IS rather than what it is called.
fn index_terms(place: &PlaceRecord) -> Vec<String> {
    let mut sources: Vec<String> = place
        .categories
        .iter()
        .flat_map(|category| [category.id.clone(), category.name.clone()])
        .collect();
    sources.extend(place.food_types.iter().cloned());
    sources.extend(place.business_chains.iter().cloned());

    let mut terms = normalized_terms(&sources);
    terms.extend(text_terms(place));
    terms.sort();
    terms.dedup();
    terms
}

/// The sorted, deduplicated normalized tokens of a set of texts.
fn normalized_terms(sources: &[String]) -> Vec<String> {
    let mut terms: Vec<String> = sources
        .iter()
        .flat_map(|source| normalize_tokens(source))
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The service's modeled `ValidationException`, naming the member at fault in both
/// the message and the `FieldList` the shape requires.
pub fn validation_exception(
    reason: &str,
    field: &str,
    message: impl Into<String>,
) -> ValidationException {
    validation_exception_for(reason, &[field], message)
}

/// As [`validation_exception`], for a rule that spans SEVERAL members — an
/// alternative one of which had to be set, say: each one is named in the
/// `FieldList`, because each one is a place the caller could have satisfied it.
pub fn validation_exception_for(
    reason: &str,
    fields: &[&str],
    message: impl Into<String>,
) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: message.clone(),
        reason: reason.to_string(),
        field_list: fields
            .iter()
            .map(|field| ValidationExceptionField {
                name: (*field).to_string(),
                message: message.clone(),
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// How a refusal is WORDED
// ---------------------------------------------------------------------------
//
// geo-places states an input-validation failure as a short sentence naming the
// member at fault and the rule it broke, derived from the request and the
// member's modeled constraint — never as prose written at the call site. Every
// message this service produces, from a handler or from the boundary validator
// (see `crate::service::EmulatedService::override_refusal`), is built by the
// helpers below, so one rule cannot drift into a second phrasing.

/// `'{member}' {rule}` — the shape of every member-named refusal this service
/// words. The member name is single-quoted and spelled as the MODEL spells it.
pub fn member_rule(member: &str, rule: &str) -> String {
    format!("'{member}' {rule}")
}

/// A value that fails a `@pattern`, naming the modeled pattern itself.
pub fn must_match_regex(member: &str, pattern: &str) -> String {
    member_rule(member, &format!("must match regex {pattern}"))
}

/// A value below a `@range` minimum.
pub fn must_not_be_less_than(member: &str, minimum: &str) -> String {
    member_rule(member, &format!("must not be less than {minimum}"))
}

/// A value above a `@range` maximum.
pub fn must_not_be_greater_than(member: &str, maximum: &str) -> String {
    member_rule(member, &format!("must not be greater than {maximum}"))
}

/// A value shorter than a `@length` minimum. Worded as a LENGTH rather than in
/// characters or entries, because `@length` applies to both a string and a list.
pub fn must_be_at_least_long(member: &str, minimum: i64) -> String {
    member_rule(member, &format!("must have a length of at least {minimum}"))
}

/// A value longer than a `@length` maximum.
pub fn must_be_at_most_long(member: &str, maximum: i64) -> String {
    member_rule(member, &format!("must have a length of at most {maximum}"))
}

/// A value whose `@length` pins one exact size (`Position` is `len 2..2`), worded
/// by the bound it actually broke — the same wording the boundary validator gives
/// that member, so one rule reads alike wherever it is caught.
pub fn must_be_length(member: &str, length: usize, required: usize) -> String {
    if length < required {
        must_be_at_least_long(member, required as i64)
    } else {
        must_be_at_most_long(member, required as i64)
    }
}

/// An opaque identifier this service never issued, so it names nothing.
pub fn must_be_valid_id(member: &str) -> String {
    member_rule(member, "must be a valid ID.")
}

/// A cursor this service cannot resume from.
pub fn must_be_valid_token(member: &str) -> String {
    member_rule(member, "must be a valid token.")
}

/// A `@required` member the request left out.
pub fn must_not_be_null(member: &str) -> String {
    member_rule(member, "must not be null.")
}

/// A value outside an enumerated type's value set.
pub fn must_be_one_of(member: &str, allowed: &[String]) -> String {
    member_rule(member, &format!("must be one of: {}", allowed.join(", ")))
}

/// Members quoted the way a message naming several of them quotes each.
fn quoted(members: &[&str]) -> String {
    members
        .iter()
        .map(|member| format!("'{member}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A rule the request could have satisfied through any one of several members,
/// and satisfied through none.
pub fn at_least_one_of(members: &[&str]) -> String {
    format!(
        "At least one of the following fields must be set: {}.",
        quoted(members)
    )
}

/// Alternative members, more than one of which the request set.
pub fn only_one_of(members: &[&str]) -> String {
    format!(
        "Only one of the following fields must be set: {}.",
        quoted(members)
    )
}

/// The member name a dotted request path names, spelled as the MODEL spells it.
///
/// The boundary validator reports the path in lowerCamel (`politicalView`,
/// `filter.includeCountries.1.member`), while a geo-places message names the
/// member (`PoliticalView`, `Filter.IncludeCountries`): the collection steps a
/// path takes to reach an element are not part of the member's name.
///
/// Capitalising each segment recovers the model's spelling because every member
/// this service declares is PascalCase in the model and lowerCamel on the wire —
/// the regular mapping the path builder applies. A member whose model spelling is
/// not a plain capitalisation of its wire name would need the input schema rather
/// than this rule.
pub fn member_of_path(path: &str) -> String {
    let named: Vec<String> = path
        .split('.')
        .filter(|segment| {
            *segment != "member" && !segment.chars().all(|character| character.is_ascii_digit())
        })
        .map(|segment| {
            let mut characters = segment.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + characters.as_str()
            })
        })
        .filter(|segment| !segment.is_empty())
        .collect();
    if named.is_empty() {
        path.to_string()
    } else {
        named.join(".")
    }
}

/// This service's wording for ONE constraint the framework found violated before
/// any handler ran: the member it names, and the modeled rule it broke.
pub fn violation_message(violation: &::smithy_server_runtime::Violation) -> String {
    use ::smithy_server_runtime::ViolationKind;

    let member = member_of_path(&violation.path);
    match &violation.kind {
        ViolationKind::MissingRequiredMember => must_not_be_null(&member),
        ViolationKind::MinLength(minimum) => must_be_at_least_long(&member, *minimum),
        ViolationKind::MaxLength(maximum) => must_be_at_most_long(&member, *maximum),
        ViolationKind::MinValue(minimum) => must_not_be_less_than(&member, minimum),
        ViolationKind::MaxValue(maximum) => must_not_be_greater_than(&member, maximum),
        ViolationKind::Pattern(pattern) => must_match_regex(&member, pattern),
        ViolationKind::EnumValue(allowed) => must_be_one_of(&member, allowed),
        ViolationKind::UniqueItems => member_rule(&member, "must not contain duplicate values"),
        // `ViolationKind` is `#[non_exhaustive]`: a kind this service does not
        // word yet still names its member rather than falling back to the
        // generic constraint-validator sentence.
        _ => member_rule(&member, "must satisfy the constraints the model declares"),
    }
}

/// This service's wording for every constraint one refused request violated.
///
/// Each violation is stated in full and the sentences are joined, rather than
/// summarised behind a count: the recorded wording states the member and the rule
/// for the one violation it carries, and a request that broke two rules has broken
/// both of them — a caller fixing its request needs each member named, and no
/// recorded geo-places message introduces a count envelope.
pub fn violations_message(violations: &[::smithy_server_runtime::Violation]) -> String {
    violations
        .iter()
        .map(violation_message)
        .collect::<Vec<_>>()
        .join("; ")
}

/// A filter list the caller set and left non-empty, or `None` — an absent filter
/// list and an empty one both mean "no constraint on this dimension". Shared by
/// every operation that declares a filter.
pub fn non_empty(values: Option<&[String]>) -> Option<&[String]> {
    values.filter(|values| !values.is_empty())
}

/// Refuse a `MaxResults` outside the modeled `range 1..100`.
///
/// Geocode, SearchText, SearchNearby and ReverseGeocode declare that range, so a
/// value outside it is outside the contract whether or not the boundary validator
/// caught it first: refusing here means an in-process caller gets the same answer
/// as a client, rather than a page quietly sized differently from the one it asked
/// for.
pub fn check_max_results(max_results: Option<i32>) -> Result<(), ValidationException> {
    check_max_results_up_to(max_results, MAX_RESULTS_CEILING)
}

/// Refuse a `MaxResults` outside `1..=ceiling`, the modeled `range` of the
/// operation being served: an operation's own ceiling is part of its contract
/// (Autocomplete's is 20 where its siblings' is 100), so each one is held to the
/// range its own model declares.
pub fn check_max_results_up_to(
    max_results: Option<i32>,
    ceiling: i32,
) -> Result<(), ValidationException> {
    let Some(requested) = max_results else {
        return Ok(());
    };
    let broken = if requested < 1 {
        Some(must_not_be_less_than("MaxResults", "1"))
    } else if requested > ceiling {
        Some(must_not_be_greater_than("MaxResults", &ceiling.to_string()))
    } else {
        None
    };
    match broken {
        Some(message) => Err(validation_exception(
            REASON_FIELD_VALIDATION_FAILED,
            "MaxResults",
            message,
        )),
        None => Ok(()),
    }
}

/// Refuse a `MaxQueryRefinements` outside Suggest's modeled `range 1..10`
/// ([`QUERY_REFINEMENTS_CEILING`]), so an in-process caller is held to the same
/// bound as a client on the wire.
pub fn check_max_query_refinements(
    max_query_refinements: Option<i32>,
) -> Result<(), ValidationException> {
    let Some(requested) = max_query_refinements else {
        return Ok(());
    };
    let broken = if requested < 1 {
        Some(must_not_be_less_than("MaxQueryRefinements", "1"))
    } else if requested > QUERY_REFINEMENTS_CEILING {
        Some(must_not_be_greater_than(
            "MaxQueryRefinements",
            &QUERY_REFINEMENTS_CEILING.to_string(),
        ))
    } else {
        None
    };
    match broken {
        Some(message) => Err(validation_exception(
            REASON_FIELD_VALIDATION_FAILED,
            "MaxQueryRefinements",
            message,
        )),
        None => Ok(()),
    }
}

/// Refuse a `QueryRadius` outside the modeled `DistanceMeters` range
/// (`range 1..21000000`).
pub fn check_query_radius(radius: Option<i64>) -> Result<(), ValidationException> {
    let Some(requested) = radius else {
        return Ok(());
    };
    let broken = if requested < QUERY_RADIUS_MIN {
        Some(must_not_be_less_than(
            "QueryRadius",
            &QUERY_RADIUS_MIN.to_string(),
        ))
    } else if requested > QUERY_RADIUS_MAX {
        Some(must_not_be_greater_than(
            "QueryRadius",
            &QUERY_RADIUS_MAX.to_string(),
        ))
    } else {
        None
    };
    match broken {
        Some(message) => Err(validation_exception(
            REASON_FIELD_VALIDATION_FAILED,
            "QueryRadius",
            message,
        )),
        None => Ok(()),
    }
}

/// The inclusive maximum number of characters a free-form query may carry.
///
/// `SearchTextRequest$QueryText` is modeled `@length 1..200`; Geocode's
/// `QueryText` is a `SensitiveString` with no length trait, so the service applies
/// its sibling's published ceiling to it: a free-form query longer than that is
/// refused rather than matched. That also BOUNDS the matching work one request can
/// ask for — the tokens a query produces, and so the runs each candidate's text is
/// located in, cannot grow past this ceiling.
pub const QUERY_TEXT_MAX: usize = 200;

/// Refuse a free-form query longer than [`QUERY_TEXT_MAX`] characters, naming the
/// member it was supplied under.
pub fn check_query_text(
    query_text: Option<&str>,
    member: &str,
) -> Result<(), ValidationException> {
    match query_text {
        Some(text) if text.chars().count() > QUERY_TEXT_MAX => Err(validation_exception(
            REASON_FIELD_VALIDATION_FAILED,
            member,
            must_be_at_most_long(member, QUERY_TEXT_MAX as i64),
        )),
        _ => Ok(()),
    }
}

/// Refuse a `Language` outside the modeled `LanguageTag` length (`len 2..35`).
pub fn check_language(language: Option<&str>) -> Result<(), ValidationException> {
    /// `LanguageTag` is modeled `len 2..35`.
    const LANGUAGE_TAG_LENGTH: std::ops::RangeInclusive<usize> = 2..=35;

    let Some(requested) = language else {
        return Ok(());
    };
    let length = requested.chars().count();
    let broken = if length < *LANGUAGE_TAG_LENGTH.start() {
        Some(must_be_at_least_long(
            "Language",
            *LANGUAGE_TAG_LENGTH.start() as i64,
        ))
    } else if length > *LANGUAGE_TAG_LENGTH.end() {
        Some(must_be_at_most_long(
            "Language",
            *LANGUAGE_TAG_LENGTH.end() as i64,
        ))
    } else {
        None
    };
    match broken {
        Some(message) => Err(validation_exception(
            REASON_FIELD_VALIDATION_FAILED,
            "Language",
            message,
        )),
        None => Ok(()),
    }
}

/// The modeled `CountryCode` pattern a `PoliticalView` must satisfy, as the model
/// writes it (`CountryCode`, `@pattern`).
pub const COUNTRY_CODE_PATTERN: &str = "^([A-Z]{2}|[A-Z]{3})$";

/// Refuse a `PoliticalView` that does not satisfy the modeled `CountryCode`
/// pattern [`COUNTRY_CODE_PATTERN`].
pub fn check_political_view(political_view: Option<&str>) -> Result<(), ValidationException> {
    let Some(requested) = political_view else {
        return Ok(());
    };
    let well_formed = matches!(requested.len(), 2 | 3)
        && requested.chars().all(|character| character.is_ascii_uppercase());
    if well_formed {
        Ok(())
    } else {
        Err(validation_exception(
            REASON_FIELD_VALIDATION_FAILED,
            "PoliticalView",
            must_match_regex("PoliticalView", COUNTRY_CODE_PATTERN),
        ))
    }
}

/// The ISO 3166-1 officially assigned country codes, as `(alpha-2, alpha-3)`
/// pairs — the register the modeled `CountryCode` ("the alpha-2 or alpha-3
/// character code for the political view of a country") draws from, and the only
/// mapping that can report a caller's alpha-2 view as the alpha-3 `CountryCode3`
/// the result items model.
///
/// SOURCE: ISO 3166-1, the officially assigned entries (249 of them); the
/// user-assigned and reserved ranges are deliberately absent, because no country
/// is assigned to them and so no alpha-3 form of one exists to report. Written out
/// here rather than taken from a crate so this service adds no entry to the
/// workspace's dependency lock, which the serialized landing step cannot merge
/// while other fixes land in parallel.
const ISO_3166_1_COUNTRIES: [(&str, &str); 249] = [
    ("AD", "AND"), ("AE", "ARE"), ("AF", "AFG"), ("AG", "ATG"), ("AI", "AIA"),
    ("AL", "ALB"), ("AM", "ARM"), ("AO", "AGO"), ("AQ", "ATA"), ("AR", "ARG"),
    ("AS", "ASM"), ("AT", "AUT"), ("AU", "AUS"), ("AW", "ABW"), ("AX", "ALA"),
    ("AZ", "AZE"), ("BA", "BIH"), ("BB", "BRB"), ("BD", "BGD"), ("BE", "BEL"),
    ("BF", "BFA"), ("BG", "BGR"), ("BH", "BHR"), ("BI", "BDI"), ("BJ", "BEN"),
    ("BL", "BLM"), ("BM", "BMU"), ("BN", "BRN"), ("BO", "BOL"), ("BQ", "BES"),
    ("BR", "BRA"), ("BS", "BHS"), ("BT", "BTN"), ("BV", "BVT"), ("BW", "BWA"),
    ("BY", "BLR"), ("BZ", "BLZ"), ("CA", "CAN"), ("CC", "CCK"), ("CD", "COD"),
    ("CF", "CAF"), ("CG", "COG"), ("CH", "CHE"), ("CI", "CIV"), ("CK", "COK"),
    ("CL", "CHL"), ("CM", "CMR"), ("CN", "CHN"), ("CO", "COL"), ("CR", "CRI"),
    ("CU", "CUB"), ("CV", "CPV"), ("CW", "CUW"), ("CX", "CXR"), ("CY", "CYP"),
    ("CZ", "CZE"), ("DE", "DEU"), ("DJ", "DJI"), ("DK", "DNK"), ("DM", "DMA"),
    ("DO", "DOM"), ("DZ", "DZA"), ("EC", "ECU"), ("EE", "EST"), ("EG", "EGY"),
    ("EH", "ESH"), ("ER", "ERI"), ("ES", "ESP"), ("ET", "ETH"), ("FI", "FIN"),
    ("FJ", "FJI"), ("FK", "FLK"), ("FM", "FSM"), ("FO", "FRO"), ("FR", "FRA"),
    ("GA", "GAB"), ("GB", "GBR"), ("GD", "GRD"), ("GE", "GEO"), ("GF", "GUF"),
    ("GG", "GGY"), ("GH", "GHA"), ("GI", "GIB"), ("GL", "GRL"), ("GM", "GMB"),
    ("GN", "GIN"), ("GP", "GLP"), ("GQ", "GNQ"), ("GR", "GRC"), ("GS", "SGS"),
    ("GT", "GTM"), ("GU", "GUM"), ("GW", "GNB"), ("GY", "GUY"), ("HK", "HKG"),
    ("HM", "HMD"), ("HN", "HND"), ("HR", "HRV"), ("HT", "HTI"), ("HU", "HUN"),
    ("ID", "IDN"), ("IE", "IRL"), ("IL", "ISR"), ("IM", "IMN"), ("IN", "IND"),
    ("IO", "IOT"), ("IQ", "IRQ"), ("IR", "IRN"), ("IS", "ISL"), ("IT", "ITA"),
    ("JE", "JEY"), ("JM", "JAM"), ("JO", "JOR"), ("JP", "JPN"), ("KE", "KEN"),
    ("KG", "KGZ"), ("KH", "KHM"), ("KI", "KIR"), ("KM", "COM"), ("KN", "KNA"),
    ("KP", "PRK"), ("KR", "KOR"), ("KW", "KWT"), ("KY", "CYM"), ("KZ", "KAZ"),
    ("LA", "LAO"), ("LB", "LBN"), ("LC", "LCA"), ("LI", "LIE"), ("LK", "LKA"),
    ("LR", "LBR"), ("LS", "LSO"), ("LT", "LTU"), ("LU", "LUX"), ("LV", "LVA"),
    ("LY", "LBY"), ("MA", "MAR"), ("MC", "MCO"), ("MD", "MDA"), ("ME", "MNE"),
    ("MF", "MAF"), ("MG", "MDG"), ("MH", "MHL"), ("MK", "MKD"), ("ML", "MLI"),
    ("MM", "MMR"), ("MN", "MNG"), ("MO", "MAC"), ("MP", "MNP"), ("MQ", "MTQ"),
    ("MR", "MRT"), ("MS", "MSR"), ("MT", "MLT"), ("MU", "MUS"), ("MV", "MDV"),
    ("MW", "MWI"), ("MX", "MEX"), ("MY", "MYS"), ("MZ", "MOZ"), ("NA", "NAM"),
    ("NC", "NCL"), ("NE", "NER"), ("NF", "NFK"), ("NG", "NGA"), ("NI", "NIC"),
    ("NL", "NLD"), ("NO", "NOR"), ("NP", "NPL"), ("NR", "NRU"), ("NU", "NIU"),
    ("NZ", "NZL"), ("OM", "OMN"), ("PA", "PAN"), ("PE", "PER"), ("PF", "PYF"),
    ("PG", "PNG"), ("PH", "PHL"), ("PK", "PAK"), ("PL", "POL"), ("PM", "SPM"),
    ("PN", "PCN"), ("PR", "PRI"), ("PS", "PSE"), ("PT", "PRT"), ("PW", "PLW"),
    ("PY", "PRY"), ("QA", "QAT"), ("RE", "REU"), ("RO", "ROU"), ("RS", "SRB"),
    ("RU", "RUS"), ("RW", "RWA"), ("SA", "SAU"), ("SB", "SLB"), ("SC", "SYC"),
    ("SD", "SDN"), ("SE", "SWE"), ("SG", "SGP"), ("SH", "SHN"), ("SI", "SVN"),
    ("SJ", "SJM"), ("SK", "SVK"), ("SL", "SLE"), ("SM", "SMR"), ("SN", "SEN"),
    ("SO", "SOM"), ("SR", "SUR"), ("SS", "SSD"), ("ST", "STP"), ("SV", "SLV"),
    ("SX", "SXM"), ("SY", "SYR"), ("SZ", "SWZ"), ("TC", "TCA"), ("TD", "TCD"),
    ("TF", "ATF"), ("TG", "TGO"), ("TH", "THA"), ("TJ", "TJK"), ("TK", "TKL"),
    ("TL", "TLS"), ("TM", "TKM"), ("TN", "TUN"), ("TO", "TON"), ("TR", "TUR"),
    ("TT", "TTO"), ("TV", "TUV"), ("TW", "TWN"), ("TZ", "TZA"), ("UA", "UKR"),
    ("UG", "UGA"), ("UM", "UMI"), ("US", "USA"), ("UY", "URY"), ("UZ", "UZB"),
    ("VA", "VAT"), ("VC", "VCT"), ("VE", "VEN"), ("VG", "VGB"), ("VI", "VIR"),
    ("VN", "VNM"), ("VU", "VUT"), ("WF", "WLF"), ("WS", "WSM"), ("YE", "YEM"),
    ("YT", "MYT"), ("ZA", "ZAF"), ("ZM", "ZMB"), ("ZW", "ZWE"),
];

/// The alpha-3 code of the country an alpha-2 or alpha-3 `CountryCode` names, or
/// `None` when [`ISO_3166_1_COUNTRIES`] assigns no country to it.
pub fn country_alpha3(code: &str) -> Option<&'static str> {
    ISO_3166_1_COUNTRIES
        .iter()
        .find(|(alpha2, alpha3)| *alpha2 == code || *alpha3 == code)
        .map(|(_, alpha3)| *alpha3)
}

/// The `PoliticalView` to report, in the alpha-3 form the result items model
/// (`CountryCode3`, `pattern ^[A-Z]{3}$`), for the alpha-2 or alpha-3 code the
/// caller asked with; `Ok(None)` when the caller asked for no political view.
///
/// A political view names a COUNTRY — "the alpha-2 or alpha-3 character code for
/// the political view of a country" (`CountryCode`) — so the view is resolved
/// against ISO 3166-1, the register that code set comes from, and NOT against the
/// places this service happens to hold: a view is a point of view on the whole
/// result set, not a place in it. A code that satisfies the modeled pattern but
/// names no country (the ISO user-assigned range, say) cannot be reported in
/// alpha-3 form at all, so it is refused; the code is `@sensitive`, so the
/// refusal names the member, not the value.
pub fn resolve_political_view(
    requested: Option<&str>,
) -> Result<Option<String>, ValidationException> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    check_political_view(Some(requested))?;
    country_alpha3(requested)
        .map(|alpha3| Some(alpha3.to_string()))
        .ok_or_else(|| {
            validation_exception(
                REASON_FIELD_VALIDATION_FAILED,
                "PoliticalView",
                member_rule(
                    "PoliticalView",
                    "must be an ISO 3166-1 alpha-2 or alpha-3 country code.",
                ),
            )
        })
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Field separator inside a decoded `NextToken`. A unit separator cannot appear in
/// an operation name, a fingerprint or a decimal offset, so the fields always split
/// cleanly.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// A short, stable fingerprint of the query a page was produced under.
///
/// A `NextToken` is only meaningful for the query that produced it: replaying it
/// against a different query would resume in a sequence its offset does not belong
/// to. The fingerprint travels inside the token so such a replay is refused instead
/// of being answered from the wrong sequence.
pub fn query_fingerprint(dimensions: &[(&str, String)]) -> String {
    let canonical = dimensions
        .iter()
        .map(|(dimension, value)| format!("{dimension}={value}"))
        .collect::<Vec<_>>()
        .join(";");
    hex::encode(Sha256::digest(canonical.as_bytes()))[..16].to_string()
}

/// Encode an opaque `NextToken` for an operation, a query and the offset the next
/// page starts at.
pub fn encode_page_token(operation: &str, fingerprint: &str, offset: usize) -> String {
    hex::encode(format!(
        "{operation}{TOKEN_SEPARATOR}{fingerprint}{TOKEN_SEPARATOR}{offset}"
    ))
}

/// Decode a `NextToken` back to the offset it resumes at, refusing one this
/// operation and query did not issue.
pub fn decode_page_token(token: &str, operation: &str, fingerprint: &str) -> Option<usize> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let mut fields = decoded.split(TOKEN_SEPARATOR);
    let (Some(token_operation), Some(token_fingerprint), Some(offset), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    if token_operation != operation || token_fingerprint != fingerprint {
        return None;
    }
    offset.parse().ok()
}

/// What a `QueryId` names, so an id minted for one purpose is not read as
/// another kind of opaque token.
const QUERY_ID_KIND: &str = "SuggestQuery";

/// Encode the opaque `QueryId` of a query-term suggestion: the kind of term
/// (`SuggestQueryResult$QueryType`) and the term itself.
///
/// "The query Id returned by the suggest API. If passed in the request, the
/// SearchText API will preform a SearchText query with the improved query terms
/// for the original query made to the suggest API"
/// (`SearchTextRequest$QueryId`): the id stands for a QUERY, so the query travels
/// inside it and the SearchText call it is handed to runs that query — nothing is
/// written into state by the read that minted it.
pub fn encode_query_id(query_type: &str, term: &str) -> String {
    hex::encode(format!(
        "{QUERY_ID_KIND}{TOKEN_SEPARATOR}{query_type}{TOKEN_SEPARATOR}{term}"
    ))
}

/// Decode a `QueryId` back to the `(QueryType, term)` it names, refusing one this
/// service never minted.
///
/// A minted id carries a query TERM — a category or a business chain the catalog
/// holds — so one carrying more text than a free-form query may itself hold
/// ([`QUERY_TEXT_MAX`]) is not an id this service issued, and the work of running
/// the query it names is bounded by the same ceiling as a `QueryText`.
pub fn decode_query_id(query_id: &str) -> Option<(String, String)> {
    let decoded = String::from_utf8(hex::decode(query_id).ok()?).ok()?;
    let mut fields = decoded.split(TOKEN_SEPARATOR);
    let (Some(kind), Some(query_type), Some(term), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    if kind != QUERY_ID_KIND || term.is_empty() || term.chars().count() > QUERY_TEXT_MAX {
        return None;
    }
    Some((query_type.to_string(), term.to_string()))
}

/// One page of candidates, and the token that resumes after it.
#[derive(Debug)]
pub struct Page {
    pub items: Vec<Scored>,
    /// Set only when candidates remain after this page.
    pub next_token: Option<String>,
}

/// Take the page starting at `offset` out of an ordered candidate list.
pub fn take_page(
    operation: &str,
    fingerprint: &str,
    ordered: &[Scored],
    offset: usize,
    page_size: usize,
) -> Page {
    let page_size = page_size.max(1);
    let items: Vec<Scored> = ordered.iter().skip(offset).take(page_size).copied().collect();
    let next_offset = offset.saturating_add(items.len());
    let next_token = (next_offset < ordered.len())
        .then(|| encode_page_token(operation, fingerprint, next_offset));
    Page { items, next_token }
}

// ---------------------------------------------------------------------------
// The state
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-geoplaces-placesservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsGeoplacesPlacesserviceState {
    /// The seeded catalog: the single source of truth for every operation.
    pub places: Vec<PlaceRecord>,
    /// `PlaceId` to its index in [`Self::places`], for GetPlace's lookup.
    pub place_index: BTreeMap<String, usize>,
    /// The `PricingBucket` each operation reports, keyed by operation name and —
    /// for the storage-priced variant — `"<operation>/Storage"`. The values are
    /// pricing TIERS ([`PRICING_BUCKET_CORE`], [`PRICING_BUCKET_STORAGE`]).
    pub pricing_buckets: BTreeMap<String, String>,
}

impl Default for ComAmazonawsGeoplacesPlacesserviceState {
    fn default() -> Self {
        seed_catalog()
    }
}

impl ComAmazonawsGeoplacesPlacesserviceState {
    /// The record a `PlaceId` names, or `None` when the catalog holds no such place.
    pub fn place(&self, place_id: &str) -> Option<&PlaceRecord> {
        self.place_index
            .get(place_id)
            .and_then(|index| self.places.get(*index))
    }

    /// The `PricingBucket` for one call.
    ///
    /// The bucket is a pricing TIER, not the operation that served the request:
    /// every Places operation is charged in the same default tier
    /// ([`PRICING_BUCKET_CORE`]), and a request that declares `IntendedUse:
    /// Storage` falls in the storage tier, because the model documents such
    /// requests as "charged at a higher rate".
    pub fn pricing_bucket(&self, operation: &str, intended_use: Option<&str>) -> String {
        let storage = intended_use.is_some_and(|value| value == INTENDED_USE_STORAGE);
        let key = if storage {
            format!("{operation}/{INTENDED_USE_STORAGE}")
        } else {
            operation.to_string()
        };
        self.pricing_buckets.get(&key).cloned().unwrap_or_else(|| {
            if storage {
                PRICING_BUCKET_STORAGE.to_string()
            } else {
                PRICING_BUCKET_CORE.to_string()
            }
        })
    }

    /// Every record a free-form query matches, in no particular order.
    pub fn text_candidates(&self, query: &str) -> Vec<Scored> {
        self.candidates(query, PlaceRecord::text_score)
    }

    /// Every record a partially typed query completes to, in no particular order.
    pub fn completion_candidates(&self, query: &str) -> Vec<Scored> {
        self.candidates(query, PlaceRecord::completion_score)
    }

    /// Every record one free-form query scores against, under the scoring the
    /// calling operation uses.
    fn candidates(
        &self,
        query: &str,
        score: impl Fn(&PlaceRecord, &[String]) -> Option<f64>,
    ) -> Vec<Scored> {
        let tokens = normalize_tokens(query);
        self.places
            .iter()
            .enumerate()
            .filter_map(|(index, place)| {
                score(place, &tokens).map(|score| Scored {
                    index,
                    score,
                    name_score: place.name_score(&tokens),
                    distance: 0,
                })
            })
            .collect()
    }

    /// The query TERMS a free-form query names: the categories and the business
    /// chains carried by the records it matched, whose own words account for every
    /// word of the query.
    ///
    /// Suggest answers "relevant places, points of interest, QUERY TERMS or
    /// SEARCH CATEGORY" and "possible matches or refinements that can be used to
    /// formulate a more accurate query", and `QueryType` enumerates exactly two
    /// kinds of such term — a category and a business chain — so these are the
    /// terms a `Query` suggestion can stand for. Best-matching term first, then
    /// alphabetically, so one catalog always suggests the same terms in the same
    /// order.
    pub fn query_terms(&self, query: &str, candidates: &[Scored]) -> Vec<QueryTerm> {
        let tokens = normalize_tokens(query);
        let mut found: Vec<QueryTerm> = Vec::new();
        for candidate in candidates {
            let place = &self.places[candidate.index];
            let vocabulary = place
                .categories
                .iter()
                .map(|category| (QUERY_TYPE_CATEGORY, category.name.clone()))
                .chain(
                    place
                        .business_chains
                        .iter()
                        .map(|chain| (QUERY_TYPE_BUSINESS_CHAIN, chain.clone())),
                );
            for (query_type, term) in vocabulary {
                // The term answers the query only when its own words account for
                // every word the caller typed: a query naming a place and nothing
                // about what it IS suggests no category to search by.
                let Some(score) = score_against(&normalize_tokens(&term), &tokens) else {
                    continue;
                };
                if found
                    .iter()
                    .any(|kept| kept.query_type == query_type && kept.term == term)
                {
                    continue;
                }
                found.push(QueryTerm {
                    term,
                    query_type,
                    score,
                });
            }
        }
        found.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.term.cmp(&right.term))
        });
        found
    }

    /// The refinements of a free-form query: for each word the caller typed that
    /// is NOT a whole word of anything the query matched, the shortest word of
    /// those matches it is the leading part of, reported with the span it
    /// replaces.
    ///
    /// A `QueryRefinement` is "the query term to be used in place of the original
    /// term" with the `[StartIndex, EndIndex)` span of that original term, so a
    /// refinement is a WORD-level substitution grounded in the catalog's own text:
    /// a partially typed word is refined to the word it completes to. A word that
    /// already spells a term of a matched record is complete, so there is nothing
    /// to refine about it. At most `limit` refinements are reported, in the order
    /// the words occur in the query.
    pub fn query_refinements(
        &self,
        query: &str,
        candidates: &[Scored],
        limit: usize,
    ) -> Vec<Refinement> {
        if limit == 0 {
            return Vec::new();
        }
        let vocabulary = self.refinement_vocabulary(candidates);
        let complete: Vec<String> = vocabulary
            .iter()
            .map(|word| word.to_lowercase())
            .collect();
        let mut refinements: Vec<Refinement> = Vec::new();
        for (typed, normalized, start_index, end_index) in QueryTokens::new(query).spans() {
            if complete.iter().any(|word| word == normalized) {
                continue;
            }
            let completion = vocabulary
                .iter()
                .filter(|word| {
                    let lowered = word.to_lowercase();
                    lowered != normalized && lowered.starts_with(normalized)
                })
                .min_by(|left, right| {
                    left.chars()
                        .count()
                        .cmp(&right.chars().count())
                        .then_with(|| left.cmp(right))
                });
            let Some(completion) = completion else {
                continue;
            };
            refinements.push(Refinement {
                original_term: typed.to_string(),
                refined_term: completion.clone(),
                start_index,
                end_index,
            });
            if refinements.len() == limit {
                break;
            }
        }
        refinements
    }

    /// The words a refinement may complete a partially typed word to: the words
    /// of the text the matched records REPORT (every name they are known by and
    /// their assembled label) and of the vocabulary they are found by (their
    /// categories, food types and business chains), each kept as the catalog
    /// spells it, sorted and deduplicated so one query always refines to the same
    /// word.
    fn refinement_vocabulary(&self, candidates: &[Scored]) -> Vec<String> {
        let mut words: Vec<String> = Vec::new();
        for candidate in candidates {
            let place = &self.places[candidate.index];
            let mut sources: Vec<String> = place.names().map(str::to_string).collect();
            sources.push(place.assembled_label());
            sources.extend(place.categories.iter().map(|category| category.name.clone()));
            sources.extend(place.food_types.iter().cloned());
            sources.extend(place.business_chains.iter().cloned());
            for source in &sources {
                words.extend(display_words(source));
            }
        }
        words.sort();
        words.dedup();
        words
    }

    /// Every record a structured query matches, in no particular order.
    pub fn component_candidates(&self, components: &[(&str, String)]) -> Vec<Scored> {
        self.places
            .iter()
            .enumerate()
            .filter_map(|(index, place)| {
                place.component_score(components).map(|score| Scored {
                    index,
                    score,
                    name_score: 0.0,
                    distance: 0,
                })
            })
            .collect()
    }

    /// Every record within `radius` metres of `position`, nearest first; with no
    /// radius, every record, nearest first.
    pub fn nearby(&self, position: [f64; 2], radius: Option<i64>) -> Vec<Scored> {
        let mut candidates: Vec<Scored> = self
            .places
            .iter()
            .enumerate()
            .map(|(index, place)| Scored {
                index,
                score: 1.0,
                name_score: 0.0,
                distance: distance_metres(position, place.position),
            })
            .filter(|candidate| radius.is_none_or(|radius| candidate.distance <= radius))
            .collect();
        self.order_by_distance(&mut candidates);
        candidates
    }

    /// The records AT `position`: the places that cover it, nearest first and, among
    /// those equally near, FINEST FIRST.
    ///
    /// Resolving a position is positional by definition, so a candidate has to
    /// contain the position rather than merely be the closest record the catalog
    /// happens to hold: a place covers `position` when the point falls inside the
    /// place's own extent (its `MapView`, which scales with the place's
    /// granularity — a country covers a region of the world, a point of interest
    /// covers its own block). A position no place covers resolves to NO records,
    /// which is an empty `ResultItems` list rather than an address somewhere else.
    ///
    /// A point inside a country is also inside the region, sub-region and locality
    /// of that country that contain it, so the candidates are ranked by
    /// [`granularity_rank`] — the most specific place containing the point first.
    /// That is what makes ReverseGeocode's documented default of one result
    /// ("Default value: 1") the finest answer the catalog can give rather than an
    /// arbitrary one of the levels covering the point: "reverse geocode converts
    /// geographic coordinates into a human-readable address or place", and the
    /// address of a point is the finest one that contains it.
    ///
    /// An explicit radius is the caller's own bound and replaces the extent test:
    /// "the maximum distance in meters from the QueryPosition from which a result
    /// will be returned" (`ReverseGeocodeRequest$QueryRadius`).
    ///
    /// Either way the `Distance` a candidate carries is its distance from the
    /// queried position to the AREA the result covers, not to the record's own
    /// centre: a place whose `MapView` — "the bounding box enclosing the geometric
    /// shape (area or line) that an individual result covers" — contains the point
    /// is AT the point, zero metres away, because the point is inside it. A record
    /// that does not cover the position keeps its great-circle distance, so a
    /// `Distance` of zero and a result covering the queried position mean each
    /// other.
    pub fn at_position(&self, position: [f64; 2], radius: Option<i64>) -> Vec<Scored> {
        let candidates = match radius {
            Some(_) => self.nearby(position, radius),
            None => self
                .places
                .iter()
                .enumerate()
                .filter(|(_, place)| place.covers(position))
                .map(|(index, place)| Scored {
                    index,
                    score: 1.0,
                    name_score: 0.0,
                    distance: distance_metres(position, place.position),
                })
                .collect(),
        };
        // What each candidate REPORTS, computed once: zero where the record covers
        // the position, beside how FINE the record is and the distance to its own
        // position, which order the records that all contain the point and so all
        // report zero.
        let mut reported: Vec<(Scored, usize, i64)> = candidates
            .into_iter()
            .map(|candidate| {
                let place = &self.places[candidate.index];
                let covers = place.covers(position);
                let own = candidate.distance;
                (
                    Scored {
                        distance: if covers { 0 } else { own },
                        ..candidate
                    },
                    granularity_rank(&place.place_type),
                    own,
                )
            })
            .collect();
        reported.sort_by(
            |(left, left_rank, left_own), (right, right_rank, right_own)| {
                left.distance
                    .cmp(&right.distance)
                    .then(right_rank.cmp(left_rank))
                    .then(left_own.cmp(right_own))
                    .then_with(|| self.tie_break(left, right))
            },
        );
        reported
            .into_iter()
            .map(|(candidate, _, _)| candidate)
            .collect()
    }

    /// Order candidates nearest first, breaking ties by title and then by `PlaceId`
    /// so the sequence a `NextToken` resumes in is total and stable.
    pub fn order_by_distance(&self, candidates: &mut [Scored]) {
        candidates.sort_by(|left, right| {
            left.distance
                .cmp(&right.distance)
                .then_with(|| self.tie_break(left, right))
        });
    }

    /// Order candidates by relevance: the share of the query each one answered
    /// first, then nearest first, then the one the query NAMED ahead of the one
    /// that merely sits in an area the query named, then by title and `PlaceId`.
    ///
    /// The name is a tie-break BELOW distance, not above it: a request that supplies
    /// a reference position asked for the places around it, so proximity decides
    /// between equally matching results, and the name only decides between results
    /// that are also equally near (every result of a request with no reference
    /// position is at the modeled default distance of 0, so there the name decides).
    pub fn order_by_relevance(&self, candidates: &mut [Scored]) {
        candidates.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then(left.distance.cmp(&right.distance))
                .then(right.name_score.total_cmp(&left.name_score))
                .then_with(|| self.tie_break(left, right))
        });
    }

    fn tie_break(&self, left: &Scored, right: &Scored) -> std::cmp::Ordering {
        let left = &self.places[left.index];
        let right = &self.places[right.index];
        right
            .places_covered
            .cmp(&left.places_covered)
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.place_id.cmp(&right.place_id))
    }

    /// The records matched by BOTH a free-form and a structured query, scored as the
    /// mean of the two scores: a request that carries `QueryText` and
    /// `QueryComponents` is asking for places that satisfy both.
    pub fn intersect(left: &[Scored], right: &[Scored]) -> Vec<Scored> {
        left.iter()
            .filter_map(|candidate| {
                right
                    .iter()
                    .find(|other| other.index == candidate.index)
                    .map(|other| Scored {
                        index: candidate.index,
                        score: (candidate.score + other.score) / 2.0,
                        name_score: candidate.name_score.max(other.name_score),
                        distance: candidate.distance,
                    })
            })
            .collect()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGeoplacesPlacesserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGeoplacesPlacesserviceState>)
}

// ---------------------------------------------------------------------------
// The seeded catalog
// ---------------------------------------------------------------------------

/// The operations whose `PricingBucket` this service seeds. Every one of the
/// service's modeled operations declares the member as `@required`.
const PRICED_OPERATIONS: [&str; 7] = [
    "Autocomplete",
    "Geocode",
    "GetPlace",
    "ReverseGeocode",
    "SearchNearby",
    "SearchText",
    "Suggest",
];

/// A time zone as `(name, offset, offset in seconds)`: the three members the
/// modeled `TimeZone` carries.
const EASTERN: (&str, &str, i64) = ("America/New_York", "-05:00", -18_000);
/// See [`EASTERN`].
const CENTRAL: (&str, &str, i64) = ("America/Chicago", "-06:00", -21_600);
/// See [`EASTERN`].
const PACIFIC: (&str, &str, i64) = ("America/Los_Angeles", "-08:00", -28_800);
/// See [`EASTERN`].
const CENTRAL_EUROPEAN: (&str, &str, i64) = ("Europe/Paris", "+01:00", 3_600);
/// See [`EASTERN`].
const EASTERN_EUROPEAN: (&str, &str, i64) = ("Europe/Kyiv", "+02:00", 7_200);

/// A US administrative area, down to the locality: the country is the same for
/// every US record, so only the levels below it are named here.
///
/// `sub_region` is the county as `(bare name, division word)`: the name alone is
/// what `Address$SubRegion$Name` reports, and the division word reaches a response
/// only through the `Title`/`Label` of the place that IS that county. US counties
/// are named by [`county`]; an area that is a county-equivalent under another word
/// (a parish, a borough, a federal district) names its own, or none.
fn us_area(
    region_code: &str,
    region_name: &str,
    sub_region: (&str, &str),
    locality: &str,
    postal_code: &str,
) -> AddressRecord {
    AddressRecord {
        country_code2: "US".to_string(),
        country_code3: "USA".to_string(),
        country_name: "United States".to_string(),
        region_code: region_code.to_string(),
        region_name: region_name.to_string(),
        sub_region_name: sub_region.0.to_string(),
        sub_region_division: sub_region.1.to_string(),
        locality: locality.to_string(),
        district: None,
        // A locality's own postal code is the central one of the settlement; a
        // street address inside it carries the code of that street
        // (see [`street_in`]).
        postal_code: postal_code.to_string(),
        street: None,
        address_number: None,
        language: "en".to_string(),
        names_by_language: BTreeMap::new(),
    }
}

/// A US county, as [`us_area`] takes a sub-region: the bare name the `SubRegion`
/// component reports, and the division word its display name carries.
fn county(name: &str) -> (&str, &str) {
    (name, "County")
}

/// An administrative area that is its own sub-region under no division word — a
/// federal district, or a city that IS the county-equivalent.
fn bare_sub_region(name: &str) -> (&str, &str) {
    (name, "")
}

/// The name text this entry holds in other languages, as
/// [`AddressRecord::names_by_language`] keys it: `(BCP 47 tag, names)` pairs.
fn names_in(variants: &[(&str, AddressNames)]) -> BTreeMap<String, AddressNames> {
    variants
        .iter()
        .map(|(tag, names)| ((*tag).to_string(), names.clone()))
        .collect()
}

/// A street address inside an administrative `area`: the same administrative
/// levels, plus the street line, the district it sits in and the postal code of
/// that street. An empty `number` is a street with no address number on it.
fn street_in(
    area: &AddressRecord,
    number: &str,
    street: &str,
    postal_code: &str,
    district: Option<&str>,
) -> AddressRecord {
    AddressRecord {
        address_number: (!number.is_empty()).then(|| number.to_string()),
        street: Some(street.to_string()),
        postal_code: postal_code.to_string(),
        district: district.map(str::to_string),
        ..area.clone()
    }
}

/// Build the catalog: a deterministic set of places spanning the administrative
/// hierarchy (country, region, sub-region, locality, district) down to points of
/// interest and point addresses, across several countries and regions, with and
/// without categories, food types, business chains, contacts, opening hours,
/// access points and time zones.
///
/// What the catalog has to COVER follows from what the operations are asked:
///
/// * Landmarks and addresses in more than one country and region, because Geocode
///   "converts a textual address or place into geographic coordinates" over AWS's
///   own global place data, and a query for a landmark is answered from it.
/// * HOMONYMOUS names in different countries (Paris, France and Paris, Texas), so
///   `Filter.IncludeCountries` — documented as the way to disambiguate — has an
///   in-country match to SELECT and not only matches to exclude.
/// * A record at every level the model's `PlaceType` enum names, because
///   `Filter.IncludePlaceTypes` selects among them and a structured query resolves
///   at the granularity of the component it supplies ([`COMPONENT_GRANULARITY`]).
/// * SEVERAL places per business chain and per category inside one area, so a page
///   of N results and a category search have something to fill them with.
/// * Places at VARIED distances around each anchor position, so a `QueryRadius`
///   selects between them rather than returning the same handful at every radius.
/// * The positions the operations are expected to resolve, because ReverseGeocode
///   answers out of the places that COVER the queried point and nothing else.
/// * A disputed territory, whose attribution a `PoliticalView` changes.
///
/// The catalog holds no place at postal-code granularity, so a structured
/// postal-code query answers with no items rather than with a place of the wrong
/// granularity.
///
/// The records carry the place data's OWN values, not prettified ones: a place with
/// real area carries the box that encloses it ([`PlaceRecord::with_map_view`]),
/// which is neither square nor centred on the place's position; a store of a
/// business chain is titled with the brand name as the data writes it, in the
/// chain's upper-case brand form, while an independent business keeps its own
/// name; and a street address carries the postal code the data holds for that
/// delivery segment, which in the United States is a ZIP+4 where the data has the
/// add-on and the five-digit code where it does not.
///
/// The category ids and names are this catalog's own: the model carries no
/// enumeration of AWS's place categories, only the `Category` structure shape.
fn seed_catalog() -> ComAmazonawsGeoplacesPlacesserviceState {
    // --- United States ------------------------------------------------------
    let washington = us_area(
        "DC",
        "District of Columbia",
        bare_sub_region("District of Columbia"),
        "Washington",
        "20001",
    );
    let seattle = us_area("WA", "Washington", county("King"), "Seattle", "98104");
    let san_francisco = us_area(
        "CA",
        "California",
        county("San Francisco"),
        "San Francisco",
        "94103",
    );
    let new_york = us_area("NY", "New York", county("New York"), "New York", "10007");
    // The US homonym of Paris, France: a different country's place of the same
    // name, which is what an `IncludeCountries` filter is documented to pick out.
    let paris_texas = us_area("TX", "Texas", county("Lamar"), "Paris", "75460");

    // --- France -------------------------------------------------------------
    let paris = AddressRecord {
        country_code2: "FR".to_string(),
        country_code3: "FRA".to_string(),
        country_name: "France".to_string(),
        region_code: "IDF".to_string(),
        region_name: "Ile-de-France".to_string(),
        sub_region_name: "Paris".to_string(),
        sub_region_division: String::new(),
        locality: "Paris".to_string(),
        postal_code: "75001".to_string(),
        language: "fr".to_string(),
        ..washington.clone()
    };

    // --- Ukraine, and the territory a political view reattributes ------------
    //
    // An entry's own text is written in the entry's own LANGUAGE — "the default
    // language for the entry" a request's `Language` falls back to — and the other
    // languages it is held in are variants beside it.
    let ukraine = AddressRecord {
        country_code2: "UA".to_string(),
        country_code3: "UKR".to_string(),
        country_name: "Україна".to_string(),
        region_code: String::new(),
        region_name: String::new(),
        sub_region_name: String::new(),
        sub_region_division: String::new(),
        locality: String::new(),
        postal_code: String::new(),
        language: "uk".to_string(),
        names_by_language: names_in(&[(
            "en",
            AddressNames {
                country_name: Some("Ukraine".to_string()),
                ..AddressNames::default()
            },
        )]),
        ..washington.clone()
    };
    // Crimea as the default (Ukrainian) view attributes it, and as the Russian
    // political view does: "the political view applies to the results of the
    // request to represent unresolved territorial claims through the point of
    // view of the specified country" (`CountryCode`). The claim is unresolved over
    // the administrative hierarchy as well as over the country: each view files the
    // territory under the divisions that view's own administration gives it.
    let crimea_ukraine = AddressRecord {
        // ISO 3166-2:UA assigns `UA-43` to the Autonomous Republic of Crimea; the
        // territory is a first-level division with no region ABOVE it, so the
        // region carries the code alone and the sub-region carries its name.
        region_code: "43".to_string(),
        sub_region_name: "Автономна Республіка Крим".to_string(),
        names_by_language: names_in(&[(
            "en",
            AddressNames {
                country_name: Some("Ukraine".to_string()),
                sub_region_name: Some("Autonomous Republic of Crimea".to_string()),
                ..AddressNames::default()
            },
        )]),
        ..ukraine.clone()
    };
    let crimea_russia = AddressRecord {
        country_code2: "RU".to_string(),
        country_code3: "RUS".to_string(),
        country_name: "Россия".to_string(),
        region_code: "CR".to_string(),
        region_name: "Южный федеральный округ".to_string(),
        sub_region_name: "Республика Крым".to_string(),
        language: "ru".to_string(),
        names_by_language: names_in(&[(
            "en",
            AddressNames {
                country_name: Some("Russia".to_string()),
                region_name: Some("Southern Federal District".to_string()),
                sub_region_name: Some("Republic of Crimea".to_string()),
                ..AddressNames::default()
            },
        )]),
        ..crimea_ukraine.clone()
    };
    // A settlement inside one of those territories, named in the attribution's own
    // language and in English: `(name, English name)`.
    let locality_of = |area: &AddressRecord, locality: (&str, &str), postal_code: &str| {
        let mut names_by_language = area.names_by_language.clone();
        if let Some(english) = names_by_language.get_mut("en") {
            english.locality = Some(locality.1.to_string());
        }
        AddressRecord {
            locality: locality.0.to_string(),
            postal_code: postal_code.to_string(),
            names_by_language,
            ..area.clone()
        }
    };
    let simferopol_ukraine = locality_of(&crimea_ukraine, ("Сімферополь", "Simferopol"), "95000");
    let simferopol_russia = locality_of(&crimea_russia, ("Симферополь", "Simferopol"), "95000");
    // The raion the recorded fixture's position sits in: a settlement-level
    // division, which is the finest place the catalog holds over that territory.
    // A raion is RURAL — its extent covers a cluster of villages rather than one
    // built-up area, so a position tens of kilometres from its centre is still
    // inside it.
    let pervomaiske_ukraine = locality_of(
        &crimea_ukraine,
        ("Первомайський район", "Pervomaiske Raion"),
        "96300",
    );
    let pervomaiske_russia = locality_of(
        &crimea_russia,
        ("Первомайский район", "Pervomaiske Raion"),
        "296300",
    );

    // The administrative levels above a locality name no settlement of their own.
    let sub_region_of = |area: &AddressRecord| AddressRecord {
        locality: String::new(),
        postal_code: String::new(),
        ..area.clone()
    };
    let above_locality = |area: &AddressRecord| AddressRecord {
        sub_region_name: String::new(),
        sub_region_division: String::new(),
        ..sub_region_of(area)
    };
    let above_region = |area: &AddressRecord| AddressRecord {
        region_code: String::new(),
        region_name: String::new(),
        ..above_locality(area)
    };
    let united_states = above_region(&washington);
    let france = above_region(&paris);

    let eastern = EASTERN;
    let pacific = PACIFIC;
    let central_european = CENTRAL_EUROPEAN;

    let mut places = vec![
        PlaceRecord::new(
            "White House",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-77.036_560, 38.897_957],
            AddressRecord {
                postal_code: "20500".to_string(),
                street: Some("Pennsylvania Ave NW".to_string()),
                address_number: Some("1600".to_string()),
                ..washington.clone()
            },
        )
        .with_categories(&[
            ("tourist_attraction", "Tourist Attraction"),
            ("government_office", "Government Office"),
        ])
        .with_contacts(&["+1 202-456-1111"], &["https://www.whitehouse.gov"])
        .with_opening_hours(&["Tue-Sat: 08:00 - 12:30"])
        .with_access_points(&[[-77.036_560, 38.897_100]])
        .with_time_zone(eastern.0, eastern.1, eastern.2),
        PlaceRecord::new(
            "1600 Pennsylvania Avenue NW",
            PLACE_TYPE_POINT_ADDRESS,
            [-77.036_530, 38.897_680],
            AddressRecord {
                postal_code: "20500".to_string(),
                street: Some("Pennsylvania Ave NW".to_string()),
                address_number: Some("1600".to_string()),
                ..washington.clone()
            },
        )
        .with_access_points(&[[-77.036_530, 38.897_600]])
        .with_time_zone(eastern.0, eastern.1, eastern.2),
        PlaceRecord::new(
            "National Air and Space Museum",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-77.019_912, 38.888_100],
            AddressRecord {
                postal_code: "20560".to_string(),
                street: Some("Independence Ave SW".to_string()),
                address_number: Some("600".to_string()),
                ..washington.clone()
            },
        )
        .with_categories(&[("museum", "Museum"), ("tourist_attraction", "Tourist Attraction")])
        .with_contacts(&["+1 202-633-2214"], &["https://airandspace.si.edu"])
        .with_opening_hours(&["Mon-Sun: 10:00 - 17:30"])
        .with_time_zone(eastern.0, eastern.1, eastern.2),
        PlaceRecord::new(
            "Lincoln Memorial",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-77.050_143, 38.889_248],
            AddressRecord {
                postal_code: "20037".to_string(),
                street: Some("Lincoln Memorial Cir NW".to_string()),
                address_number: Some("2".to_string()),
                ..washington.clone()
            },
        )
        .with_categories(&[("monument", "Monument"), ("tourist_attraction", "Tourist Attraction")])
        .with_time_zone(eastern.0, eastern.1, eastern.2),
        PlaceRecord::new(
            "Founding Farmers",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-77.044_230, 38.900_800],
            AddressRecord {
                postal_code: "20006".to_string(),
                street: Some("Pennsylvania Ave NW".to_string()),
                address_number: Some("1924".to_string()),
                ..washington.clone()
            },
        )
        .with_categories(&[("restaurant", "Restaurant")])
        .with_food_types(&["American"])
        .with_contacts(&["+1 202-822-8783"], &["https://www.wearefoundingfarmers.com"])
        .with_opening_hours(&["Mon-Fri: 11:00 - 22:00", "Sat-Sun: 09:00 - 22:00"])
        .with_time_zone(eastern.0, eastern.1, eastern.2),
        PlaceRecord::new(
            "Washington",
            PLACE_TYPE_LOCALITY,
            [-77.036_871, 38.907_192],
            washington.clone(),
        )
        .with_map_view([-77.119_76, 38.791_65, -76.909_40, 38.995_55])
        .with_time_zone(eastern.0, eastern.1, eastern.2),
        PlaceRecord::new(
            "Space Needle",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.349_358, 47.620_506],
            AddressRecord {
                postal_code: "98109".to_string(),
                street: Some("Broad St".to_string()),
                address_number: Some("400".to_string()),
                district: Some("Lower Queen Anne".to_string()),
                ..seattle.clone()
            },
        )
        .with_categories(&[
            ("tourist_attraction", "Tourist Attraction"),
            ("observation_deck", "Observation Deck"),
        ])
        .with_contacts(&["+1 206-905-2100"], &["https://www.spaceneedle.com"])
        .with_opening_hours(&["Mon-Sun: 09:00 - 22:00"])
        .with_access_points(&[[-122.349_200, 47.620_300]])
        .with_time_zone(pacific.0, pacific.1, pacific.2),
        PlaceRecord::new(
            "400 Broad Street",
            PLACE_TYPE_POINT_ADDRESS,
            [-122.349_300, 47.620_400],
            AddressRecord {
                postal_code: "98109".to_string(),
                street: Some("Broad St".to_string()),
                address_number: Some("400".to_string()),
                district: Some("Lower Queen Anne".to_string()),
                ..seattle.clone()
            },
        )
        .with_time_zone(pacific.0, pacific.1, pacific.2),
        PlaceRecord::new(
            "Pike Place Market",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.340_810, 47.608_849],
            AddressRecord {
                postal_code: "98101".to_string(),
                street: Some("Pike St".to_string()),
                address_number: Some("85".to_string()),
                district: Some("Downtown".to_string()),
                ..seattle.clone()
            },
        )
        .with_categories(&[("market", "Market"), ("tourist_attraction", "Tourist Attraction")])
        .with_contacts(&["+1 206-682-7453"], &["https://www.pikeplacemarket.org"])
        .with_opening_hours(&["Mon-Sun: 09:00 - 18:00"])
        .with_time_zone(pacific.0, pacific.1, pacific.2),
        PlaceRecord::new(
            "STARBUCKS RESERVE ROASTERY",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.327_290, 47.614_040],
            AddressRecord {
                postal_code: "98101".to_string(),
                street: Some("Pike St".to_string()),
                address_number: Some("1124".to_string()),
                district: Some("Capitol Hill".to_string()),
                ..seattle.clone()
            },
        )
        .with_categories(&[("coffee_shop", "Coffee Shop"), ("restaurant", "Restaurant")])
        .with_food_types(&["Coffee"])
        .with_business_chains(&["Starbucks"])
        .with_contacts(&["+1 206-624-0173"], &["https://www.starbucksreserve.com"])
        .with_opening_hours(&["Mon-Sun: 07:00 - 21:00"])
        .with_time_zone(pacific.0, pacific.1, pacific.2),
        PlaceRecord::new(
            "Seattle",
            PLACE_TYPE_LOCALITY,
            [-122.329_45, 47.603_57],
            seattle.clone(),
        )
        .with_map_view([-122.436_13, 47.495_58, -122.236_48, 47.734_16])
        .with_time_zone(pacific.0, pacific.1, pacific.2),
        PlaceRecord::new(
            "Tour Eiffel",
            PLACE_TYPE_POINT_OF_INTEREST,
            [2.294_481, 48.858_370],
            AddressRecord {
                postal_code: "75007".to_string(),
                street: Some("Av. Anatole France".to_string()),
                address_number: Some("5".to_string()),
                district: Some("7e Arrondissement".to_string()),
                ..paris.clone()
            },
        )
        .with_alternate_names(&["La Tour Eiffel"])
        // The landmark's name as other languages write it: a query naming it by
        // any of them is answered under the name it used.
        .with_localized_titles(&[
            ("en", "Eiffel Tower"),
            ("de", "Eiffelturm"),
            ("es", "Torre Eiffel"),
        ])
        .with_categories(&[
            ("tourist_attraction", "Tourist Attraction"),
            ("monument", "Monument"),
        ])
        .with_contacts(&["+33 892 70 12 39"], &["https://www.toureiffel.paris"])
        .with_opening_hours(&["Mon-Sun: 09:30 - 23:45"])
        .with_access_points(&[[2.294_400, 48.858_000]])
        .with_time_zone(central_european.0, central_european.1, central_european.2),
        PlaceRecord::new(
            "Paris",
            PLACE_TYPE_LOCALITY,
            [2.352_222, 48.856_613],
            paris.clone(),
        )
        .with_map_view([2.224_20, 48.815_57, 2.469_92, 48.902_15])
        .with_time_zone(central_european.0, central_european.1, central_european.2),
        // The administrative levels a structured query resolves at: a district, a
        // sub-region (county), the regions (states) and the countries.
        PlaceRecord::new(
            "Capitol Hill",
            PLACE_TYPE_DISTRICT,
            [-122.312_440, 47.623_100],
            AddressRecord {
                district: Some("Capitol Hill".to_string()),
                postal_code: "98102".to_string(),
                ..seattle.clone()
            },
        )
        .with_map_view([-122.327_20, 47.609_00, -122.303_00, 47.641_80])
        .with_time_zone(pacific.0, pacific.1, pacific.2),
        PlaceRecord::new(
            "King County",
            PLACE_TYPE_SUB_REGION,
            [-121.837_000, 47.491_000],
            AddressRecord {
                locality: String::new(),
                postal_code: String::new(),
                ..seattle.clone()
            },
        )
        .with_map_view([-122.541_00, 47.077_60, -121.065_60, 47.780_40])
        .with_time_zone(pacific.0, pacific.1, pacific.2),
        PlaceRecord::new(
            "Washington",
            PLACE_TYPE_REGION,
            [-120.740_135, 47.751_076],
            AddressRecord {
                sub_region_name: String::new(),
                locality: String::new(),
                postal_code: String::new(),
                ..seattle.clone()
            },
        )
        .with_map_view([-124.848_97, 45.543_54, -116.915_99, 49.002_49])
        .with_time_zone(pacific.0, pacific.1, pacific.2),
        PlaceRecord::new(
            "District of Columbia",
            PLACE_TYPE_REGION,
            [-77.016_389, 38.904_722],
            AddressRecord {
                sub_region_name: String::new(),
                locality: String::new(),
                postal_code: String::new(),
                ..washington.clone()
            },
        )
        .with_map_view([-77.119_76, 38.791_65, -76.909_40, 38.995_55])
        .with_time_zone(eastern.0, eastern.1, eastern.2),
        PlaceRecord::new(
            "Ile-de-France",
            PLACE_TYPE_REGION,
            [2.521_000, 48.849_000],
            AddressRecord {
                sub_region_name: String::new(),
                locality: String::new(),
                postal_code: String::new(),
                ..paris.clone()
            },
        )
        .with_map_view([1.446_10, 48.120_30, 3.559_20, 49.241_20])
        .with_time_zone(central_european.0, central_european.1, central_european.2),
        PlaceRecord::new(
            "United States",
            PLACE_TYPE_COUNTRY,
            [-98.579_500, 39.828_300],
            united_states,
        )
        .with_map_view([-124.784_40, 24.396_30, -66.885_44, 49.384_36]),
        PlaceRecord::new(
            "France",
            PLACE_TYPE_COUNTRY,
            [2.209_667, 46.232_193],
            france,
        )
        .with_map_view([-5.142_22, 41.333_00, 9.561_56, 51.089_00]),
    ];

    // --- Washington DC: a second city with a coffee shop, so a category search
    // --- is not answered out of one city alone.
    places.push(
        PlaceRecord::new(
            "COMPASS COFFEE",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-77.021_100, 38.902_500],
            street_in(&washington, "1535", "7th St NW", "20001", Some("Shaw")),
        )
        .with_categories(&[("coffee_shop", "Coffee Shop")])
        .with_food_types(&["Coffee"])
        .with_business_chains(&["Compass Coffee"])
        .with_contacts(&["+1 202-838-3139"], &["https://www.compasscoffee.com"])
        .with_opening_hours(&["Mon-Sun: 07:00 - 19:00"])
        .in_time_zone(eastern),
    );

    // --- Seattle: density. Three records sit within 40 m of the Space Needle and
    // --- two more between 40 m and 85 m, so widening a `QueryRadius` selects
    // --- between them; four coffee shops sit inside downtown and two outside it,
    // --- so a `BoundingBox` does too; and the chain has several stores, so a page
    // --- of N results can hold N.
    places.extend([
        PlaceRecord::new(
            "Seattle Center Monorail",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.349_780, 47.620_600],
            street_in(
                &seattle,
                "370",
                "Thomas St",
                "98109",
                Some("Lower Queen Anne"),
            ),
        )
        .with_alternate_names(&["Seattle Center Monorail Station"])
        .with_categories(&[("transit_station", "Transit Station")])
        .with_opening_hours(&["Mon-Sun: 07:30 - 23:00"])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "Chihuly Garden and Glass",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.350_245, 47.620_400],
            street_in(
                &seattle,
                "305",
                "Harrison St",
                "98109",
                Some("Lower Queen Anne"),
            ),
        )
        .with_categories(&[("museum", "Museum"), ("tourist_attraction", "Tourist Attraction")])
        .with_contacts(&["+1 206-753-4940"], &["https://www.chihulygardenandglass.com"])
        .with_opening_hours(&["Mon-Sun: 10:00 - 19:00"])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "Seattle Center Armory",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.349_600, 47.621_200],
            street_in(
                &seattle,
                "305",
                "Harrison St",
                "98109",
                Some("Lower Queen Anne"),
            ),
        )
        .with_categories(&[("food_court", "Food Court")])
        .with_opening_hours(&["Mon-Sun: 07:00 - 21:00"])
        .in_time_zone(pacific),
        // The chain's stores around the Seattle Center, each under the BRAND NAME
        // the place data writes, with the ZIP+4 of the delivery segment its street
        // address sits on where the data carries one.
        PlaceRecord::new(
            "STARBUCKS",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.347_26, 47.624_72],
            street_in(
                &seattle,
                "500",
                "Mercer St",
                "98109-4654",
                Some("Lower Queen Anne"),
            ),
        )
        .with_categories(&[("coffee_shop", "Coffee Shop")])
        .with_food_types(&["Coffee"])
        .with_business_chains(&["Starbucks"])
        .with_contacts(&["+1 206-298-3034"], &["https://www.starbucks.com"])
        .with_opening_hours(&["Mon-Sun: 05:00 - 20:30"])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "STARBUCKS",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.335_81, 47.622_76],
            street_in(
                &seattle,
                "442",
                "Terry Ave N",
                "98109-5210",
                Some("South Lake Union"),
            ),
        )
        .with_categories(&[("coffee_shop", "Coffee Shop")])
        .with_food_types(&["Coffee"])
        .with_business_chains(&["Starbucks"])
        .with_contacts(&["+1 206-624-4521"], &["https://www.starbucks.com"])
        .with_opening_hours(&["Mon-Fri: 05:30 - 18:00"])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "STARBUCKS",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.340_622, 47.610_100],
            street_in(&seattle, "1912", "Pike Pl", "98101", Some("Downtown")),
        )
        .with_categories(&[("coffee_shop", "Coffee Shop")])
        .with_food_types(&["Coffee"])
        .with_business_chains(&["Starbucks"])
        .with_contacts(&["+1 206-448-8762"], &["https://www.starbucks.com"])
        .with_opening_hours(&["Mon-Sun: 06:00 - 21:00"])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "STARBUCKS",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.339_500, 47.608_200],
            street_in(&seattle, "102", "Pike St", "98101", Some("Downtown")),
        )
        .with_categories(&[("coffee_shop", "Coffee Shop")])
        .with_food_types(&["Coffee"])
        .with_business_chains(&["Starbucks"])
        .with_opening_hours(&["Mon-Sun: 05:30 - 20:00"])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "STARBUCKS",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.330_600, 47.604_400],
            street_in(&seattle, "701", "5th Ave", "98104", Some("Downtown")),
        )
        .with_categories(&[("coffee_shop", "Coffee Shop")])
        .with_food_types(&["Coffee"])
        .with_business_chains(&["Starbucks"])
        .with_opening_hours(&["Mon-Fri: 05:00 - 18:00"])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "Seattle Coffee Works",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.341_900, 47.609_600],
            street_in(&seattle, "107", "Pike St", "98101", Some("Downtown")),
        )
        .with_categories(&[("coffee_shop", "Coffee Shop")])
        .with_food_types(&["Coffee"])
        .with_contacts(&["+1 206-340-8867"], &["https://seattlecoffeeworks.com"])
        .with_opening_hours(&["Mon-Sun: 07:00 - 18:00"])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "Victrola Coffee Roasters",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.312_900, 47.614_900],
            street_in(&seattle, "310", "E Pike St", "98122", Some("Capitol Hill")),
        )
        .with_categories(&[("coffee_shop", "Coffee Shop")])
        .with_food_types(&["Coffee"])
        .with_opening_hours(&["Mon-Sun: 06:30 - 19:00"])
        .in_time_zone(pacific),
    ]);

    // --- San Francisco, California: a landmark, its city and the levels above it.
    places.extend([
        PlaceRecord::new(
            "Golden Gate Bridge",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.478_255, 37.819_929],
            street_in(
                &san_francisco,
                "",
                "Golden Gate Brg",
                "94129",
                Some("Presidio"),
            ),
        )
        .with_categories(&[("tourist_attraction", "Tourist Attraction"), ("bridge", "Bridge")])
        .with_contacts(&["+1 415-921-5858"], &["https://www.goldengate.org"])
        .with_access_points(&[[-122.477_700, 37.807_700]])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "Pier 39",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.409_600, 37.808_600],
            street_in(
                &san_francisco,
                "",
                "The Embarcadero",
                "94133",
                Some("Fisherman's Wharf"),
            ),
        )
        .with_categories(&[("tourist_attraction", "Tourist Attraction"), ("market", "Market")])
        .with_opening_hours(&["Mon-Sun: 10:00 - 21:00"])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "BLUE BOTTLE COFFEE",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.402_000, 37.776_400],
            street_in(
                &san_francisco,
                "66",
                "Mint St",
                "94103",
                Some("South of Market"),
            ),
        )
        .with_categories(&[("coffee_shop", "Coffee Shop")])
        .with_food_types(&["Coffee"])
        .with_business_chains(&["Blue Bottle Coffee"])
        .with_opening_hours(&["Mon-Sun: 07:00 - 17:00"])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "San Francisco",
            PLACE_TYPE_LOCALITY,
            [-122.419_416, 37.774_929],
            san_francisco.clone(),
        )
        .with_map_view([-122.514_93, 37.708_08, -122.357_03, 37.833_24])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "San Francisco County",
            PLACE_TYPE_SUB_REGION,
            [-122.431_297, 37.773_972],
            sub_region_of(&san_francisco),
        )
        .with_map_view([-123.173_83, 37.639_83, -122.281_78, 37.929_82])
        .in_time_zone(pacific),
        PlaceRecord::new(
            "California",
            PLACE_TYPE_REGION,
            [-119.681_564, 36.116_203],
            above_locality(&san_francisco),
        )
        .with_map_view([-124.409_59, 32.534_16, -114.131_21, 42.009_52])
        .in_time_zone(pacific),
    ]);

    // --- New York: the positions a reverse geocode of this coast resolves to.
    places.extend([
        PlaceRecord::new(
            "Statue of Liberty",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-74.044_500, 40.689_200],
            street_in(&new_york, "", "Liberty Island", "10004", None),
        )
        .with_categories(&[("monument", "Monument"), ("tourist_attraction", "Tourist Attraction")])
        .with_contacts(&["+1 212-363-3200"], &["https://www.nps.gov/stli"])
        .with_opening_hours(&["Mon-Sun: 09:00 - 17:00"])
        .with_access_points(&[[-74.044_300, 40.690_200]])
        .in_time_zone(eastern),
        PlaceRecord::new(
            "Empire State Building",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-73.985_664, 40.748_817],
            street_in(&new_york, "20", "W 34th St", "10001", Some("Manhattan")),
        )
        .with_categories(&[
            ("tourist_attraction", "Tourist Attraction"),
            ("observation_deck", "Observation Deck"),
        ])
        .with_contacts(&["+1 212-736-3100"], &["https://www.esbnyc.com"])
        .with_opening_hours(&["Mon-Sun: 09:00 - 24:00"])
        .in_time_zone(eastern),
        PlaceRecord::new(
            "Manhattan",
            PLACE_TYPE_DISTRICT,
            [-73.971_200, 40.783_100],
            AddressRecord {
                district: Some("Manhattan".to_string()),
                postal_code: "10024".to_string(),
                ..new_york.clone()
            },
        )
        .with_map_view([-74.047_29, 40.679_55, -73.907_00, 40.882_21])
        .in_time_zone(eastern),
        PlaceRecord::new(
            "New York",
            PLACE_TYPE_LOCALITY,
            [-74.006_000, 40.712_800],
            new_york.clone(),
        )
        .with_map_view([-74.259_09, 40.477_40, -73.700_27, 40.917_58])
        .in_time_zone(eastern),
        PlaceRecord::new(
            "New York County",
            PLACE_TYPE_SUB_REGION,
            [-73.971_200, 40.776_700],
            sub_region_of(&new_york),
        )
        .with_map_view([-74.047_29, 40.679_55, -73.907_00, 40.882_21])
        .in_time_zone(eastern),
        PlaceRecord::new(
            "New York",
            PLACE_TYPE_REGION,
            [-75.421_000, 42.945_900],
            above_locality(&new_york),
        )
        .with_map_view([-79.762_15, 40.496_10, -71.856_21, 45.015_87])
        .in_time_zone(eastern),
    ]);

    // --- Paris, Texas: the US homonym of Paris, France, and the levels above it.
    places.extend([
        PlaceRecord::new(
            "Paris",
            PLACE_TYPE_LOCALITY,
            [-95.555_513, 33.660_942],
            paris_texas.clone(),
        )
        .with_map_view([-95.626_70, 33.616_10, -95.466_10, 33.731_80])
        .in_time_zone(CENTRAL),
        PlaceRecord::new(
            "Lamar County",
            PLACE_TYPE_SUB_REGION,
            [-95.571_000, 33.666_000],
            sub_region_of(&paris_texas),
        )
        .with_map_view([-95.838_70, 33.436_50, -95.306_40, 33.878_70])
        .in_time_zone(CENTRAL),
        PlaceRecord::new(
            "Texas",
            PLACE_TYPE_REGION,
            [-99.901_813, 31.968_599],
            above_locality(&paris_texas),
        )
        .with_map_view([-106.645_65, 25.837_38, -93.508_29, 36.500_70])
        .in_time_zone(CENTRAL),
    ]);

    // --- France: a landmark known by another name, and a category in a second
    // --- country.
    places.extend([
        PlaceRecord::new(
            "Musee du Louvre",
            PLACE_TYPE_POINT_OF_INTEREST,
            [2.337_644, 48.860_611],
            street_in(&paris, "", "Rue de Rivoli", "75001", Some("1er Arrondissement")),
        )
        .with_alternate_names(&["The Louvre"])
        .with_localized_titles(&[("en", "Louvre Museum"), ("es", "Museo del Louvre")])
        .with_categories(&[("museum", "Museum"), ("tourist_attraction", "Tourist Attraction")])
        .with_contacts(&["+33 1 40 20 53 17"], &["https://www.louvre.fr"])
        .with_opening_hours(&["Wed-Mon: 09:00 - 18:00"])
        .in_time_zone(central_european),
        PlaceRecord::new(
            "Cafe de Flore",
            PLACE_TYPE_POINT_OF_INTEREST,
            [2.332_900, 48.854_100],
            street_in(
                &paris,
                "172",
                "Bd Saint-Germain",
                "75006",
                Some("6e Arrondissement"),
            ),
        )
        .with_categories(&[("coffee_shop", "Coffee Shop"), ("restaurant", "Restaurant")])
        .with_food_types(&["Coffee", "French"])
        .with_contacts(&["+33 1 45 48 55 26"], &["https://cafedeflore.fr"])
        .with_opening_hours(&["Mon-Sun: 07:30 - 01:30"])
        .in_time_zone(central_european),
    ]);

    // --- A disputed territory: the records whose attribution a `PoliticalView`
    // --- changes, plus the country the default view attributes them to.
    //
    // The territory is held at the SUB-REGION level, which is the division its
    // address component fills (`Address.SubRegion`), with the settlements inside it
    // at locality level: a position there resolves to the finest of them that
    // covers it.
    places.extend([
        PlaceRecord::new(
            "Автономна Республіка Крим",
            PLACE_TYPE_SUB_REGION,
            [34.100_000, 45.300_000],
            crimea_ukraine,
        )
        .with_map_view([32.492_10, 44.386_50, 36.648_50, 46.219_10])
        .with_alternate_names(&["Crimea", "Krym", "Крым"])
        .with_political_attribution("RUS", crimea_russia)
        .in_time_zone(EASTERN_EUROPEAN),
        PlaceRecord::new(
            "Сімферополь",
            PLACE_TYPE_LOCALITY,
            [34.102_417, 44.952_117],
            simferopol_ukraine,
        )
        .with_map_view([34.027_50, 44.899_10, 34.188_80, 45.019_90])
        .with_alternate_names(&["Simferopol", "Симферополь"])
        .with_political_attribution("RUS", simferopol_russia)
        .in_time_zone(EASTERN_EUROPEAN),
        PlaceRecord::new(
            "Первомайський район",
            PLACE_TYPE_LOCALITY,
            [33.859_39, 45.714_2],
            pervomaiske_ukraine,
        )
        .with_map_view([33.526_92, 45.343_03, 34.122_77, 45.809_53])
        .with_alternate_names(&["Pervomaiske", "Первомайский район"])
        .with_political_attribution("RUS", pervomaiske_russia)
        .in_time_zone(EASTERN_EUROPEAN),
        PlaceRecord::new(
            "Україна",
            PLACE_TYPE_COUNTRY,
            [31.165_581, 48.379_433],
            ukraine,
        )
        .with_map_view([22.137_16, 44.386_46, 40.227_17, 52.379_19])
        .with_alternate_names(&["Ukraine", "Украина"])
        .in_time_zone(EASTERN_EUROPEAN),
    ]);

    // The derived fields: what each record is FOUND by, and how much of the
    // catalog it covers (both computed once, from the records themselves).
    let positions: Vec<[f64; 2]> = places.iter().map(|place| place.position).collect();
    for (index, place) in places.iter_mut().enumerate() {
        place.search_terms = index_terms(place);
        place.completion_terms = text_terms(place);
        place.places_covered = positions
            .iter()
            .enumerate()
            .filter(|(other, position)| *other != index && place.covers(**position))
            .count();
    }

    let place_index = places
        .iter()
        .enumerate()
        .map(|(index, place)| (place.place_id.clone(), index))
        .collect();

    let mut pricing_buckets = BTreeMap::new();
    for operation in PRICED_OPERATIONS {
        pricing_buckets.insert(operation.to_string(), PRICING_BUCKET_CORE.to_string());
        pricing_buckets.insert(
            format!("{operation}/{INTENDED_USE_STORAGE}"),
            PRICING_BUCKET_STORAGE.to_string(),
        );
    }

    ComAmazonawsGeoplacesPlacesserviceState {
        places,
        place_index,
        pricing_buckets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every seeded record is reachable by its own `PlaceId`, and no two records
    /// share one: GetPlace resolves what the search operations hand out.
    #[test]
    fn every_seeded_place_is_indexed_under_a_unique_id() {
        let state = ComAmazonawsGeoplacesPlacesserviceState::default();
        assert_eq!(
            state.place_index.len(),
            state.places.len(),
            "case index-size: expected one index entry per place, got {} for {} places",
            state.place_index.len(),
            state.places.len()
        );
        for place in &state.places {
            let found = state.place(&place.place_id);
            assert!(
                found.is_some_and(|record| record.title == place.title),
                "case {}: the index must resolve a place's own id back to it, got {:?}",
                place.title,
                found.map(|record| record.title.clone())
            );
        }
    }

    /// A distance is symmetric, zero at the same point, and correct to within a
    /// percent on a known separation.
    #[test]
    fn distances_are_symmetric_and_calibrated() {
        let white_house = [-77.036_560, 38.897_957];
        let lincoln_memorial = [-77.050_143, 38.889_248];
        let there = distance_metres(white_house, lincoln_memorial);
        let back = distance_metres(lincoln_memorial, white_house);
        assert_eq!(
            there, back,
            "case symmetry: expected equal distances, got {there} and {back}"
        );
        assert_eq!(
            distance_metres(white_house, white_house),
            0,
            "case identity: a point is zero metres from itself"
        );
        // One degree of latitude is ~111.2 km on the sphere used above.
        let degree = distance_metres([0.0, 0.0], [0.0, 1.0]);
        assert!(
            (111_000..=111_400).contains(&degree),
            "case one-degree-of-latitude: expected ~111.2 km, got {degree} m"
        );
    }

    /// Every entry of the hand-carried ISO 3166-1 register is well formed, listed
    /// once, and resolvable from either of its forms; a code the register assigns
    /// to no country resolves to nothing.
    ///
    /// The checked entries are ones no request in this service's issues names, so
    /// the table is a register and not a list fitted to a recorded value.
    #[test]
    fn the_country_code_register_is_well_formed() {
        let mut alpha2: Vec<&str> = ISO_3166_1_COUNTRIES.iter().map(|(code, _)| *code).collect();
        let mut alpha3: Vec<&str> = ISO_3166_1_COUNTRIES.iter().map(|(_, code)| *code).collect();
        for (code, length) in [(&mut alpha2, 2_usize), (&mut alpha3, 3)] {
            for entry in code.iter() {
                assert!(
                    entry.len() == length
                        && entry.chars().all(|character| character.is_ascii_uppercase()),
                    "case {entry}: every code is {length} uppercase ASCII letters"
                );
            }
            let listed = code.len();
            code.sort_unstable();
            code.dedup();
            assert_eq!(
                code.len(),
                listed,
                "case duplicates: a country is listed once, got {listed} entries and \
                 {} distinct codes",
                code.len()
            );
        }

        for (alpha2, alpha3) in [("NZ", "NZL"), ("MA", "MAR"), ("IN", "IND"), ("KM", "COM")] {
            assert_eq!(
                (country_alpha3(alpha2), country_alpha3(alpha3)),
                (Some(alpha3), Some(alpha3)),
                "case {alpha2}: both forms of a country resolve to its alpha-3 code"
            );
        }
        for unassigned in ["ZZ", "ZZZ", "XX", "QQQ"] {
            assert!(
                country_alpha3(unassigned).is_none(),
                "case {unassigned}: a code the register assigns to no country resolves \
                 to nothing, got {:?}",
                country_alpha3(unassigned)
            );
        }
    }

    /// A position outside the World Geodetic System range the modeled `Position`
    /// documents is no position at all, whatever its length.
    #[test]
    fn a_position_outside_the_wgs84_range_is_not_a_position() {
        for inside in [
            [0.0, 0.0],
            [180.0, 90.0],
            [-180.0, -90.0],
            [33.859_39, 45.714_2],
        ] {
            assert!(
                is_wgs84(inside),
                "case {inside:?}: a position on or inside the published bounds is one"
            );
        }
        for outside in [
            [180.000_001, 0.0],
            [-180.000_001, 0.0],
            [0.0, 90.000_001],
            [0.0, -90.000_001],
            [1.0e9, 0.0],
            [f64::NAN, 0.0],
            [0.0, f64::INFINITY],
        ] {
            assert!(
                !is_wgs84(outside),
                "case {outside:?}: a pair outside the WGS 84 range is no position"
            );
        }
    }

    /// A page token resumes only the operation and query that issued it.
    #[test]
    fn a_page_token_is_bound_to_its_operation_and_query() {
        let token = encode_page_token("SearchText", "fingerprint", 3);
        assert_eq!(
            decode_page_token(&token, "SearchText", "fingerprint"),
            Some(3),
            "case own-token: a token must resume at the offset it carries"
        );
        assert!(
            decode_page_token(&token, "SearchNearby", "fingerprint").is_none(),
            "case foreign-operation: a SearchText token must not resume SearchNearby"
        );
        assert!(
            decode_page_token(&token, "SearchText", "other").is_none(),
            "case foreign-query: a token must not resume a different query"
        );
        assert!(
            decode_page_token("not-a-token", "SearchText", "fingerprint").is_none(),
            "case garbage: an unparseable token must be refused"
        );
    }
}
