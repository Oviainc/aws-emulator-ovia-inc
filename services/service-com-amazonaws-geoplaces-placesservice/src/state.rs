// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-geoplaces-placesservice service.
//!
//! Amazon Location Service Places (`geo-places`) has no customer-created resources:
//! every operation it models is `@readonly` and reports over AWS's own place data.
//! That data therefore lives in this service's state, seeded once (the engine builds
//! the state through `Default` on first touch, atomically) with a deterministic
//! catalog of places in a handful of cities.
//!
//! Every handler PROJECTS its response out of these records, so the results of
//! different operations agree: a `PlaceId` handed out by Geocode, SearchText or
//! SearchNearby resolves through GetPlace to the same `Title`, `Position` and
//! `Address`, and a `Position` handed out by any of them reverse-geocodes back to the
//! place it came from.
//!
//! Nothing here is mutated by a request. All five implemented operations are
//! `@readonly`, so even their pagination carries no server-side cursor: a `NextToken`
//! is a self-describing, opaque string (see [`encode_page_token`]) rather than an
//! entry written into state by a read.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AccessPoint, Address, BusinessChain, Category, ContactDetails, Contacts, Country, FoodType,
    OpeningHours, Region, StreetComponents, SubRegion, TimeZone, ValidationException,
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

/// `AdditionalFeatures` values this emulator can answer, from the per-operation
/// `*AdditionalFeature` enums in the model.
pub const FEATURE_ACCESS: &str = "Access";
/// See [`FEATURE_ACCESS`].
pub const FEATURE_CONTACT: &str = "Contact";
/// See [`FEATURE_ACCESS`].
pub const FEATURE_TIME_ZONE: &str = "TimeZone";

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

/// The inclusive maximum of every implemented operation's modeled `MaxResults`
/// range (`range 1..100`), which is also the maximum length of their
/// `*ResultItemList` outputs (`len 0..100`).
pub const MAX_RESULTS_CEILING: i32 = 100;

/// The inclusive bounds of the modeled `QueryRadius` (`DistanceMeters`,
/// `range 1..21000000`) on ReverseGeocode and SearchNearby.
pub const QUERY_RADIUS_MIN: i64 = 1;
/// See [`QUERY_RADIUS_MIN`].
pub const QUERY_RADIUS_MAX: i64 = 21_000_000;

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Mean Earth radius in metres (IUGG mean radius), used for the great-circle
/// distances the service reports in `Distance`.
const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// Metres in one degree of latitude on that sphere, used to size a `MapView` box
/// around a point.
const METRES_PER_DEGREE_LAT: f64 = EARTH_RADIUS_M * std::f64::consts::PI / 180.0;

/// A `[longitude, latitude]` pair taken from a modeled `Position` list, or `None`
/// when the list is not exactly two elements (the modeled `len 2..2` is refused at
/// the boundary, so this only guards an in-process caller).
pub fn position(values: &[f64]) -> Option<[f64; 2]> {
    match values {
        [longitude, latitude] => Some([*longitude, *latitude]),
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

/// The `MapView` box enclosing a point place: `extent_metres` either side of the
/// position, rendered as `[west, south, east, north]` the way the modeled
/// `BoundingBox` documents.
fn map_view_around(position: [f64; 2], extent_metres: f64) -> [f64; 4] {
    let latitude_delta = extent_metres / METRES_PER_DEGREE_LAT;
    let cosine = position[1].to_radians().cos().abs().max(1e-6);
    let longitude_delta = extent_metres / (METRES_PER_DEGREE_LAT * cosine);
    [
        position[0] - longitude_delta,
        position[1] - latitude_delta,
        position[0] + longitude_delta,
        position[1] + latitude_delta,
    ]
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
/// is a standard abbreviation of the word) while "King" answers the sub-region
/// "King County" by half: it names part of it.
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
    pub distance: i64,
}

// ---------------------------------------------------------------------------
// The catalog records
// ---------------------------------------------------------------------------

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
    pub sub_region_name: String,
    pub locality: String,
    pub district: Option<String>,
    pub postal_code: String,
    pub street: Option<String>,
    pub address_number: Option<String>,
    /// The BCP 47 language this record's text is written in, reported as the
    /// street components' `Language`: "If there is no data for the result in the
    /// requested language, data will be returned in the default language for the
    /// entry" (`StreetComponents$Language`), and this catalog holds one language
    /// per entry.
    pub language: String,
}

impl AddressRecord {
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
    pub fn component_texts(&self, component: &str) -> Vec<&str> {
        let texts = match component {
            "Country" => vec![
                self.country_code2.as_str(),
                self.country_code3.as_str(),
                self.country_name.as_str(),
            ],
            "Region" => vec![self.region_code.as_str(), self.region_name.as_str()],
            "SubRegion" => vec![self.sub_region_name.as_str()],
            "Locality" => vec![self.locality.as_str()],
            "District" => self.district.as_deref().into_iter().collect(),
            "Street" => self.street.as_deref().into_iter().collect(),
            "AddressNumber" => self.address_number.as_deref().into_iter().collect(),
            "PostalCode" => vec![self.postal_code.as_str()],
            _ => Vec::new(),
        };
        texts.into_iter().filter(|text| !text.is_empty()).collect()
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
/// sub-region as "King County, WA, United States". The district is part of the
/// label only for a place that IS a district — for a business or an address it is
/// finer detail than the postal address needs.
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
    let mut settlement: Vec<&str> = Vec::new();
    if place_type == PLACE_TYPE_DISTRICT {
        settlement.extend(address.district.as_deref());
    }
    settlement.extend(
        [address.locality.as_str(), address.sub_region_name.as_str()]
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
            .copied()
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
            .copied()
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
    pub address: AddressRecord,
    /// `[longitude, latitude]`, WGS 84, as the modeled `Position` orders them.
    pub position: [f64; 2],
    /// `[west, south, east, north]`, as the modeled `BoundingBox` orders them.
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
}

impl PlaceRecord {
    /// A catalog record for `title` at `position`, with a `MapView` extending
    /// `extent_metres` either side of that position.
    fn new(
        title: &str,
        place_type: &str,
        position: [f64; 2],
        address: AddressRecord,
        extent_metres: f64,
    ) -> Self {
        Self {
            place_id: synthesize_place_id(title, position),
            place_type: place_type.to_string(),
            title: title.to_string(),
            address,
            position,
            map_view: map_view_around(position, extent_metres),
            categories: Vec::new(),
            food_types: Vec::new(),
            business_chains: Vec::new(),
            phones: Vec::new(),
            websites: Vec::new(),
            opening_hours: Vec::new(),
            access_points: Vec::new(),
            time_zone: None,
            search_terms: Vec::new(),
        }
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
        self.access_points = access_points.to_vec();
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

    /// The place's OWN name, for a place that has one: a point of interest is a
    /// named business or landmark. An address or an administrative area has no
    /// name of its own — what names it is its assembled address label.
    pub fn name(&self) -> Option<&str> {
        (self.place_type == PLACE_TYPE_POINT_OF_INTEREST).then_some(self.title.as_str())
    }

    /// This place's assembled address label: see [`assemble_label`].
    pub fn assembled_label(&self) -> String {
        assemble_label(&self.address, self.name(), &self.place_type)
    }

    /// `Title`: "The localized display name of this result item"
    /// (`GeocodeResultItem$Title`) — the place's own name, or, for an address or an
    /// administrative area, the assembled address label that names it. A client
    /// shows this as the single display line for the place, so a bare address
    /// component is not one.
    pub fn title_modeled(&self) -> String {
        self.name()
            .map_or_else(|| self.assembled_label(), str::to_string)
    }

    /// `Address`, with the assembled `Label` this place renders.
    pub fn address_modeled(&self) -> Address {
        self.address.to_modeled(self.assembled_label())
    }

    /// `Position`, as a modeled `Position` list.
    pub fn position_modeled(&self) -> Vec<f64> {
        self.position.to_vec()
    }

    /// `MapView`, as a modeled `BoundingBox` list.
    pub fn map_view_modeled(&self) -> Vec<f64> {
        self.map_view.to_vec()
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

    /// `AccessPoints`, or `None` when this place has none recorded.
    pub fn access_points_modeled(&self) -> Option<Vec<AccessPoint>> {
        if self.access_points.is_empty() {
            return None;
        }
        Some(
            self.access_points
                .iter()
                .enumerate()
                .map(|(rank, point)| AccessPoint {
                    position: Some(point.to_vec()),
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
        if query_tokens.is_empty() {
            return None;
        }
        let mut matched = 0.0;
        for token in query_tokens {
            matched += self
                .search_terms
                .iter()
                .filter_map(|term| affinity(term, token))
                .fold(f64::NAN, f64::max);
            if matched.is_nan() {
                return None;
            }
        }
        Some(matched / query_tokens.len() as f64)
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
            if let Some(found) = self.name().and_then(|name| locate(query, name)) {
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
                    .filter_map(|text| locate(query, text))
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

    /// Is this place's position inside `[west, south, east, north]`? A box whose
    /// west edge is east of its east edge crosses the antimeridian.
    pub fn within_bounding_box(&self, box_: [f64; 4]) -> bool {
        let [west, south, east, north] = box_;
        let [longitude, latitude] = self.position;
        let within_longitude = if west <= east {
            longitude >= west && longitude <= east
        } else {
            longitude >= west || longitude <= east
        };
        within_longitude && latitude >= south && latitude <= north
    }
}

/// The opaque `PlaceId` of a record: an uppercase hex digest of what the record IS
/// (its title and its position to six decimal places), so the same catalog always
/// hands out the same ids and an id from one operation resolves in another.
fn synthesize_place_id(title: &str, position: [f64; 2]) -> String {
    /// Characters of the digest a `PlaceId` carries. The modeled `PlaceId` allows
    /// up to 500.
    const PLACE_ID_LEN: usize = 40;

    let digest = Sha256::digest(
        format!("{title}|{:.6}|{:.6}", position[0], position[1]).as_bytes(),
    );
    hex::encode_upper(digest)[..PLACE_ID_LEN].to_string()
}

/// The normalized tokens a record is found by: its title, its address, its
/// categories, its food types and its business chains.
fn index_terms(place: &PlaceRecord) -> Vec<String> {
    let label = place.assembled_label();
    let mut sources: Vec<&str> = vec![
        place.title.as_str(),
        label.as_str(),
        place.address.locality.as_str(),
        place.address.region_code.as_str(),
        place.address.region_name.as_str(),
        place.address.sub_region_name.as_str(),
        place.address.country_name.as_str(),
        place.address.country_code2.as_str(),
        place.address.country_code3.as_str(),
        place.address.postal_code.as_str(),
    ];
    sources.extend(place.address.street.as_deref());
    sources.extend(place.address.district.as_deref());
    sources.extend(
        place
            .categories
            .iter()
            .flat_map(|category| [category.id.as_str(), category.name.as_str()]),
    );
    sources.extend(place.food_types.iter().map(String::as_str));
    sources.extend(place.business_chains.iter().map(String::as_str));

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
    let message = message.into();
    ValidationException {
        message: message.clone(),
        reason: reason.to_string(),
        field_list: vec![ValidationExceptionField {
            name: field.to_string(),
            message,
        }],
    }
}

/// A filter list the caller set and left non-empty, or `None` — an absent filter
/// list and an empty one both mean "no constraint on this dimension". Shared by
/// every operation that declares a filter.
pub fn non_empty(values: Option<&[String]>) -> Option<&[String]> {
    values.filter(|values| !values.is_empty())
}

/// Refuse a `MaxResults` outside the modeled `range 1..100`.
///
/// Every implemented operation declares that range, so a value outside it is
/// outside the contract whether or not the boundary validator caught it first:
/// refusing here means an in-process caller gets the same answer as a client,
/// rather than a page quietly sized differently from the one it asked for.
pub fn check_max_results(max_results: Option<i32>) -> Result<(), ValidationException> {
    match max_results {
        Some(requested) if !(1..=MAX_RESULTS_CEILING).contains(&requested) => {
            Err(validation_exception(
                REASON_FIELD_VALIDATION_FAILED,
                "MaxResults",
                format!(
                    "MaxResults must be between 1 and {MAX_RESULTS_CEILING}; \
                     {requested} was supplied"
                ),
            ))
        }
        _ => Ok(()),
    }
}

/// Refuse a `QueryRadius` outside the modeled `DistanceMeters` range
/// (`range 1..21000000`).
pub fn check_query_radius(radius: Option<i64>) -> Result<(), ValidationException> {
    match radius {
        Some(requested) if !(QUERY_RADIUS_MIN..=QUERY_RADIUS_MAX).contains(&requested) => {
            Err(validation_exception(
                REASON_FIELD_VALIDATION_FAILED,
                "QueryRadius",
                format!(
                    "QueryRadius must be between {QUERY_RADIUS_MIN} and {QUERY_RADIUS_MAX} \
                     metres; {requested} was supplied"
                ),
            ))
        }
        _ => Ok(()),
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
            format!("{member} must be at most {QUERY_TEXT_MAX} characters"),
        )),
        _ => Ok(()),
    }
}

/// Refuse a `Language` outside the modeled `LanguageTag` length (`len 2..35`).
pub fn check_language(language: Option<&str>) -> Result<(), ValidationException> {
    match language {
        Some(requested) if !(2..=35).contains(&requested.chars().count()) => {
            Err(validation_exception(
                REASON_FIELD_VALIDATION_FAILED,
                "Language",
                "Language must be a BCP 47 language code of 2 to 35 characters",
            ))
        }
        _ => Ok(()),
    }
}

/// Refuse a `PoliticalView` that does not satisfy the modeled `CountryCode`
/// pattern `^([A-Z]{2}|[A-Z]{3})$`.
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
            "PoliticalView must be an alpha-2 or alpha-3 country code matching \
             ^([A-Z]{2}|[A-Z]{3})$",
        ))
    }
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

    /// The `PoliticalView` to report, in the alpha-3 form the result items model
    /// (`pattern ^[A-Z]{3}$`), for the alpha-2 or alpha-3 code the caller asked
    /// with; `Ok(None)` when the caller asked for no political view.
    ///
    /// A view this service cannot resolve is REFUSED rather than dropped: a caller
    /// whose political view was not applied must not be answered as if it had been.
    /// The code is `@sensitive`, so the refusal names the member, not the value.
    pub fn political_view(
        &self,
        requested: Option<&str>,
    ) -> Result<Option<String>, ValidationException> {
        let Some(requested) = requested else {
            return Ok(None);
        };
        self.places
            .iter()
            .find(|place| {
                place.address.country_code2.eq_ignore_ascii_case(requested)
                    || place.address.country_code3.eq_ignore_ascii_case(requested)
            })
            .map(|place| Some(place.address.country_code3.clone()))
            .ok_or_else(|| {
                validation_exception(
                    REASON_FIELD_VALIDATION_FAILED,
                    "PoliticalView",
                    "PoliticalView must name a country this service holds place data for",
                )
            })
    }

    /// Every record a free-form query matches, in no particular order.
    pub fn text_candidates(&self, query: &str) -> Vec<Scored> {
        let tokens = normalize_tokens(query);
        self.places
            .iter()
            .enumerate()
            .filter_map(|(index, place)| {
                place.text_score(&tokens).map(|score| Scored {
                    index,
                    score,
                    distance: 0,
                })
            })
            .collect()
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
                distance: distance_metres(position, place.position),
            })
            .filter(|candidate| radius.is_none_or(|radius| candidate.distance <= radius))
            .collect();
        self.order_by_distance(&mut candidates);
        candidates
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

    /// Order candidates by relevance first, then nearest first, then by title and
    /// `PlaceId`.
    pub fn order_by_relevance(&self, candidates: &mut [Scored]) {
        candidates.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then(left.distance.cmp(&right.distance))
                .then_with(|| self.tie_break(left, right))
        });
    }

    fn tie_break(&self, left: &Scored, right: &Scored) -> std::cmp::Ordering {
        let left = &self.places[left.index];
        let right = &self.places[right.index];
        left.title
            .cmp(&right.title)
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

/// `MapView` extent for a point place (an address or a point of interest).
const POINT_EXTENT_M: f64 = 150.0;

/// `MapView` extent for a district, which covers a neighbourhood.
const DISTRICT_EXTENT_M: f64 = 3_000.0;

/// `MapView` extent for a locality, which covers a whole settlement.
const LOCALITY_EXTENT_M: f64 = 12_000.0;

/// `MapView` extent for a sub-region (a county).
const SUB_REGION_EXTENT_M: f64 = 60_000.0;

/// `MapView` extent for a region (a state).
const REGION_EXTENT_M: f64 = 350_000.0;

/// `MapView` extent for a country.
const COUNTRY_EXTENT_M: f64 = 2_000_000.0;

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

/// Build the catalog: a deterministic set of places in Washington DC, Seattle and
/// Paris, spanning the administrative hierarchy (country, region, sub-region,
/// locality, district) down to points of interest and point addresses, in two
/// countries, with and without categories, food types, business chains, contacts,
/// opening hours, access points and time zones.
///
/// The administrative levels are seeded because a structured query resolves at the
/// granularity of the component it supplies ([`COMPONENT_GRANULARITY`]): a query
/// for a country must be able to answer with that country. The catalog holds no
/// place at postal-code granularity, so a structured postal-code query answers with
/// no items rather than with a place of the wrong granularity.
///
/// The category ids and names are this catalog's own: the model carries no
/// enumeration of AWS's place categories, only the `Category` structure shape.
fn seed_catalog() -> ComAmazonawsGeoplacesPlacesserviceState {
    let washington = AddressRecord {
        country_code2: "US".to_string(),
        country_code3: "USA".to_string(),
        country_name: "United States".to_string(),
        region_code: "DC".to_string(),
        region_name: "District of Columbia".to_string(),
        sub_region_name: "District of Columbia".to_string(),
        locality: "Washington".to_string(),
        district: None,
        // The locality's primary postal code: the central ZIP of the city.
        postal_code: "20001".to_string(),
        street: None,
        address_number: None,
        language: "en".to_string(),
    };
    let seattle = AddressRecord {
        region_code: "WA".to_string(),
        region_name: "Washington".to_string(),
        sub_region_name: "King County".to_string(),
        locality: "Seattle".to_string(),
        postal_code: "98104".to_string(),
        ..washington.clone()
    };
    let paris = AddressRecord {
        country_code2: "FR".to_string(),
        country_code3: "FRA".to_string(),
        country_name: "France".to_string(),
        region_code: "IDF".to_string(),
        region_name: "Ile-de-France".to_string(),
        sub_region_name: "Paris".to_string(),
        locality: "Paris".to_string(),
        postal_code: "75001".to_string(),
        language: "fr".to_string(),
        ..washington.clone()
    };
    // The administrative levels above a locality name no settlement of their own.
    let united_states = AddressRecord {
        region_code: String::new(),
        region_name: String::new(),
        sub_region_name: String::new(),
        locality: String::new(),
        postal_code: String::new(),
        ..washington.clone()
    };
    let france = AddressRecord {
        region_code: String::new(),
        region_name: String::new(),
        sub_region_name: String::new(),
        locality: String::new(),
        postal_code: String::new(),
        ..paris.clone()
    };

    let eastern = ("America/New_York", "-05:00", -18_000);
    let pacific = ("America/Los_Angeles", "-08:00", -28_800);
    let central_european = ("Europe/Paris", "+01:00", 3_600);

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
            POINT_EXTENT_M,
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
            POINT_EXTENT_M,
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
            POINT_EXTENT_M,
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
            POINT_EXTENT_M,
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
            POINT_EXTENT_M,
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
            LOCALITY_EXTENT_M,
        )
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
            POINT_EXTENT_M,
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
            POINT_EXTENT_M,
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
            POINT_EXTENT_M,
        )
        .with_categories(&[("market", "Market"), ("tourist_attraction", "Tourist Attraction")])
        .with_contacts(&["+1 206-682-7453"], &["https://www.pikeplacemarket.org"])
        .with_opening_hours(&["Mon-Sun: 09:00 - 18:00"])
        .with_time_zone(pacific.0, pacific.1, pacific.2),
        PlaceRecord::new(
            "Starbucks Reserve Roastery",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.327_290, 47.614_040],
            AddressRecord {
                postal_code: "98101".to_string(),
                street: Some("Pike St".to_string()),
                address_number: Some("1124".to_string()),
                district: Some("Capitol Hill".to_string()),
                ..seattle.clone()
            },
            POINT_EXTENT_M,
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
            [-122.330_062, 47.603_832],
            seattle.clone(),
            LOCALITY_EXTENT_M,
        )
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
            POINT_EXTENT_M,
        )
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
            LOCALITY_EXTENT_M,
        )
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
            DISTRICT_EXTENT_M,
        )
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
            SUB_REGION_EXTENT_M,
        )
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
            REGION_EXTENT_M,
        )
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
            LOCALITY_EXTENT_M,
        )
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
            REGION_EXTENT_M,
        )
        .with_time_zone(central_european.0, central_european.1, central_european.2),
        PlaceRecord::new(
            "United States",
            PLACE_TYPE_COUNTRY,
            [-98.579_500, 39.828_300],
            united_states,
            COUNTRY_EXTENT_M,
        ),
        PlaceRecord::new(
            "France",
            PLACE_TYPE_COUNTRY,
            [2.209_667, 46.232_193],
            france,
            COUNTRY_EXTENT_M,
        ),
    ];

    for place in &mut places {
        place.search_terms = index_terms(place);
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
