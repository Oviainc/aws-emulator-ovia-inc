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
    OpeningHours, Region, SubRegion, TimeZone, ValidationException, ValidationExceptionField,
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

/// The `IntendedUse` value that says the caller will persist the results. The
/// per-operation docs state such requests "will be charged at a higher rate", i.e.
/// they fall in a different pricing bucket than the default `SingleUse`.
pub const INTENDED_USE_STORAGE: &str = "Storage";

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

/// How well an indexed term answers a piece of a query: 1 when they are the same,
/// half when one abbreviates the other (either is a prefix of the other, which is
/// what makes "Pennsylvania Ave" and "Pennsylvania Avenue" the same street), and
/// `None` when they are unrelated.
fn affinity(term: &str, wanted: &str) -> Option<f64> {
    if term == wanted {
        Some(1.0)
    } else if term.starts_with(wanted) || wanted.starts_with(term) {
        Some(0.5)
    } else {
        None
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
#[derive(Debug, Clone)]
pub struct AddressRecord {
    pub label: String,
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
}

impl AddressRecord {
    /// This address as the modeled `Address` structure.
    pub fn to_modeled(&self) -> Address {
        Address {
            label: Some(self.label.clone()),
            country: Some(Country {
                code2: Some(self.country_code2.clone()),
                code3: Some(self.country_code3.clone()),
                name: Some(self.country_name.clone()),
            }),
            region: Some(Region {
                code: Some(self.region_code.clone()),
                name: Some(self.region_name.clone()),
            }),
            sub_region: Some(SubRegion {
                code: None,
                name: Some(self.sub_region_name.clone()),
            }),
            locality: Some(self.locality.clone()),
            district: self.district.clone(),
            postal_code: Some(self.postal_code.clone()),
            street: self.street.clone(),
            address_number: self.address_number.clone(),
            ..Address::default()
        }
    }

    /// The texts a `GeocodeQueryComponents` member may match against, named by that
    /// member. A component the catalog does not carry matches nothing.
    pub fn component_texts(&self, component: &str) -> Vec<&str> {
        match component {
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
        }
    }
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
        Some(
            self.business_chains
                .iter()
                .map(|name| BusinessChain {
                    name: Some(name.clone()),
                    id: Some(normalize_tokens(name).join("_")),
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
            let wanted = normalize_tokens(value).join(" ");
            if wanted.is_empty() {
                return None;
            }
            matched += self
                .address
                .component_texts(component)
                .iter()
                .filter_map(|text| affinity(&normalize_tokens(text).join(" "), &wanted))
                .fold(f64::NAN, f64::max);
            if matched.is_nan() {
                return None;
            }
        }
        Some(matched / components.len() as f64)
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
    let mut sources: Vec<&str> = vec![
        place.title.as_str(),
        place.address.label.as_str(),
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
    /// for the storage-priced variant — `"<operation>/Storage"`.
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
    /// Each operation has its own bucket, and a request that declares
    /// `IntendedUse: Storage` falls in that operation's storage bucket, because the
    /// model documents such requests as "charged at a higher rate".
    pub fn pricing_bucket(&self, operation: &str, intended_use: Option<&str>) -> String {
        let storage = intended_use.is_some_and(|value| value == INTENDED_USE_STORAGE);
        let key = if storage {
            format!("{operation}/{INTENDED_USE_STORAGE}")
        } else {
            operation.to_string()
        };
        self.pricing_buckets
            .get(&key)
            .cloned()
            .unwrap_or_else(|| operation.to_string())
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

/// `MapView` extent for a locality, which covers a whole settlement.
const LOCALITY_EXTENT_M: f64 = 12_000.0;

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
/// Paris, spanning point addresses, points of interest and localities, in two
/// countries, with and without categories, food types, business chains, contacts,
/// opening hours, access points and time zones.
///
/// The category ids and names are this catalog's own: the model carries no
/// enumeration of AWS's place categories, only the `Category` structure shape.
fn seed_catalog() -> ComAmazonawsGeoplacesPlacesserviceState {
    let washington = AddressRecord {
        label: String::new(),
        country_code2: "US".to_string(),
        country_code3: "USA".to_string(),
        country_name: "United States".to_string(),
        region_code: "DC".to_string(),
        region_name: "District of Columbia".to_string(),
        sub_region_name: "District of Columbia".to_string(),
        locality: "Washington".to_string(),
        district: None,
        postal_code: String::new(),
        street: None,
        address_number: None,
    };
    let seattle = AddressRecord {
        region_code: "WA".to_string(),
        region_name: "Washington".to_string(),
        sub_region_name: "King County".to_string(),
        locality: "Seattle".to_string(),
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
        ..washington.clone()
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
                label: "1600 Pennsylvania Ave NW, Washington, DC 20500, United States".to_string(),
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
                label: "1600 Pennsylvania Ave NW, Washington, DC 20500, United States".to_string(),
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
                label: "600 Independence Ave SW, Washington, DC 20560, United States".to_string(),
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
                label: "2 Lincoln Memorial Cir NW, Washington, DC 20037, United States".to_string(),
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
                label: "1924 Pennsylvania Ave NW, Washington, DC 20006, United States".to_string(),
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
            AddressRecord {
                label: "Washington, DC, United States".to_string(),
                ..washington.clone()
            },
            LOCALITY_EXTENT_M,
        )
        .with_time_zone(eastern.0, eastern.1, eastern.2),
        PlaceRecord::new(
            "Space Needle",
            PLACE_TYPE_POINT_OF_INTEREST,
            [-122.349_358, 47.620_506],
            AddressRecord {
                label: "400 Broad St, Seattle, WA 98109, United States".to_string(),
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
                label: "400 Broad St, Seattle, WA 98109, United States".to_string(),
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
                label: "85 Pike St, Seattle, WA 98101, United States".to_string(),
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
                label: "1124 Pike St, Seattle, WA 98101, United States".to_string(),
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
            AddressRecord {
                label: "Seattle, WA, United States".to_string(),
                ..seattle.clone()
            },
            LOCALITY_EXTENT_M,
        )
        .with_time_zone(pacific.0, pacific.1, pacific.2),
        PlaceRecord::new(
            "Tour Eiffel",
            PLACE_TYPE_POINT_OF_INTEREST,
            [2.294_481, 48.858_370],
            AddressRecord {
                label: "5 Av. Anatole France, 75007 Paris, France".to_string(),
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
            AddressRecord {
                label: "Paris, Ile-de-France, France".to_string(),
                ..paris.clone()
            },
            LOCALITY_EXTENT_M,
        )
        .with_time_zone(central_european.0, central_european.1, central_european.2),
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
        pricing_buckets.insert(operation.to_string(), operation.to_string());
        pricing_buckets.insert(
            format!("{operation}/{INTENDED_USE_STORAGE}"),
            format!("{operation}{INTENDED_USE_STORAGE}"),
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
