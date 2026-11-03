// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the geolocation catalog: `ListGeoLocations` and
//! `GetGeoLocation`.
//!
//! The rules under test:
//!
//! 1. The catalog is the SERVICE-owned set of locations Route 53 supports for
//!    geolocation, and every entry of it is exactly one of the three kinds the
//!    API has — a continent (`ContinentCode`/`ContinentName`), a country
//!    (`CountryCode`/`CountryName`) or a subdivision of a country (its country's
//!    code and name plus `SubdivisionCode`/`SubdivisionName`) — with no code
//!    listed twice. Because it belongs to the service and not to the account, the
//!    same catalog answers every call, whatever the account does and whichever
//!    store the service is mounted on.
//! 2. It is reported in the order `ListGeoLocations` documents: "Countries are
//!    listed first, and continents are listed last. If Amazon Route 53 supports
//!    subdivisions for a country …, the subdivisions for that country are listed
//!    in alphabetical order immediately after the corresponding country."
//! 3. A page carries at most `maxitems` locations, `IsTruncated` says whether the
//!    catalog holds a location after it, and the response's `Next*Code`s are that
//!    location's own codes — so handing them back as the `start*code`s resumes
//!    exactly there, and walking the pages of any size visits the catalog once, in
//!    one order.
//! 4. `GetGeoLocation` resolves a location out of that SAME catalog ("For a list
//!    of all supported subdivision codes, use the ListGeoLocations API"), so every
//!    listed location is resolved to the very entry the listing carried, a
//!    location the catalog does not hold is refused `NoSuchGeoLocation` (404), and
//!    a request naming no single location — the three documented forms are
//!    `continentcode`, `countrycode`, and `countrycode` with `subdivisioncode` —
//!    is refused `InvalidInput` (400).
//!
//! One call is one row of those classes, so the tables below span them: the whole
//! catalog is walked rather than sampled for the structural rules, the page bound
//! is driven across its published range (1, 100 and one past it) and the kinds are
//! each resolved and each refused.
//!
//! Every test drives the service through the real router, using the same harness
//! the hosted-zone suites use, so what is asserted is the HTTP/restXml behaviour a
//! Route 53 client sees.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_ok, router, unescape_xml, Reply};

/// The seven continents the API supports, as `(code, name)`.
///
/// Written out here rather than read from the crate under test, from the source
/// the model names: `GeoLocation$ContinentCode`'s own documentation, "AF: Africa
/// AN: Antarctica AS: Asia EU: Europe OC: Oceania NA: North America SA: South
/// America". Compared as a SET, so this row order says nothing.
const PUBLISHED_CONTINENTS: &[(&str, &str)] = &[
    ("AF", "Africa"),
    ("AN", "Antarctica"),
    ("AS", "Asia"),
    ("EU", "Europe"),
    ("NA", "North America"),
    ("OC", "Oceania"),
    ("SA", "South America"),
];

/// Locations the catalog has to hold, none of them the one the recorded case
/// used, as `(country code, subdivision code)` with an empty subdivision for a
/// country.
///
/// Source: ISO 3166-1 alpha-2, which `GeoLocation$CountryCode` names ("the
/// two-letter country codes that are specified in ISO standard 3166-1 alpha-2"),
/// plus the `*` default location the same member documents and two of the United
/// States Postal Service's two-letter state abbreviations, which
/// `GeoLocationDetails$SubdivisionCode` points at.
const PUBLISHED_LOCATIONS: &[(&str, &str)] = &[
    ("*", ""),
    ("AD", ""),
    ("BR", ""),
    ("DE", ""),
    ("JP", ""),
    ("NZ", ""),
    ("US", ""),
    ("ZW", ""),
    ("US", "WY"),
    ("US", "RI"),
];

/// The page bound Route 53 documents on the listings of its CALLER-OWNED
/// resources: `ListHostedZones$MaxItems` ("If you specify a value greater than
/// 100, Route 53 returns only the first 100").
///
/// This operation's own `MaxItems` documents no such bound, so 100 is just one
/// row of the `maxitems` tables below; the rows past it are what assert that a
/// larger `maxitems` really is honoured here.
const PAGE_BOUND: i32 = 100;

/// One `GeoLocationDetails` as the wire carried it.
///
/// Parsed here, out of the response body, rather than read off the crate's own
/// types: what a client sees is the XML, and the kinds below are decided by which
/// members that XML carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) continent_code: Option<String>,
    pub(crate) continent_name: Option<String>,
    pub(crate) country_code: Option<String>,
    pub(crate) country_name: Option<String>,
    pub(crate) subdivision_code: Option<String>,
    pub(crate) subdivision_name: Option<String>,
}

impl Entry {
    /// The codes that name this location, as a case the assertions below can be
    /// read with.
    fn codes(&self) -> String {
        format!(
            "continent={:?} country={:?} subdivision={:?}",
            self.continent_code, self.country_code, self.subdivision_code
        )
    }
}

/// Which kind of location an entry is, decided by the members it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Continent,
    Country,
    Subdivision,
}

/// The kind `entry` is, or `None` when the members it carries name no kind at all
/// — a continent code beside a country code, a subdivision code with no country,
/// a code with no name.
fn kind(entry: &Entry) -> Option<Kind> {
    let continent = entry.continent_code.is_some() || entry.continent_name.is_some();
    let country = entry.country_code.is_some() || entry.country_name.is_some();
    let subdivision = entry.subdivision_code.is_some() || entry.subdivision_name.is_some();
    match (continent, country, subdivision) {
        (true, false, false) => entry
            .continent_code
            .is_some()
            .then_some(Kind::Continent)
            .filter(|_| entry.continent_name.is_some()),
        (false, true, false) => entry
            .country_code
            .is_some()
            .then_some(Kind::Country)
            .filter(|_| entry.country_name.is_some()),
        (false, true, true) => (entry.country_code.is_some()
            && entry.country_name.is_some()
            && entry.subdivision_code.is_some()
            && entry.subdivision_name.is_some())
        .then_some(Kind::Subdivision),
        _ => None,
    }
}

/// The text of the first `<name>` element inside `block`, or `None` when it holds
/// none.
fn element(block: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    Some(unescape_xml(block[start..end].trim()))
}

/// Every `<GeoLocationDetails>` element of a response body, in document order.
///
/// A hand-written scan rather than an XML crate, for the reason the harness's own
/// flattener gives: the only documents read here are the emulator's own restXml
/// responses. `<GeoLocationDetails>` cannot be confused with the
/// `<GeoLocationDetailsList>` that wraps it, because the split is on the closing
/// angle bracket too.
/// `pub(crate)` so the catalog-contents suite reads one response the same way
/// rather than growing a second parser.
pub(crate) fn entries(body: &str) -> Vec<Entry> {
    body.split("<GeoLocationDetails>")
        .skip(1)
        .map(|rest| {
            let block = rest
                .split("</GeoLocationDetails>")
                .next()
                .unwrap_or_default();
            Entry {
                continent_code: element(block, "ContinentCode"),
                continent_name: element(block, "ContinentName"),
                country_code: element(block, "CountryCode"),
                country_name: element(block, "CountryName"),
                subdivision_code: element(block, "SubdivisionCode"),
                subdivision_name: element(block, "SubdivisionName"),
            }
        })
        .collect()
}

/// One `ListGeoLocations` call with the query string `query`, asserting a 200.
///
/// A 501 here means the operation is still a scaffold, which is exactly what the
/// message says, so a regression to the stub reads as one.
/// `pub(crate)` so the catalog-contents suite calls the operation through the
/// same reader.
pub(crate) async fn list(router: &MultiServiceRouter, query: &str) -> Reply {
    let uri = if query.is_empty() {
        "/2013-04-01/geolocations".to_string()
    } else {
        format!("/2013-04-01/geolocations?{query}")
    };
    let reply = call(router, "GET", &uri, "").await;
    assert_eq!(
        reply.status, 200,
        "case {query:?}: expected 200 from ListGeoLocations (501 = still scaffolded), got {} with \
         body {}",
        reply.status, reply.body
    );
    reply
}

/// The whole catalog, walked a page of `page_size` at a time through the
/// `Next*Code`s the responses carry.
///
/// Returns the locations in the order the pages carried them, so a walk at one
/// page size can be compared against a walk at another.
async fn walk(router: &MultiServiceRouter, page_size: Option<i32>) -> Vec<Entry> {
    let mut walked: Vec<Entry> = Vec::new();
    let mut resume: Vec<String> = Vec::new();
    loop {
        let mut query = resume.clone();
        if let Some(page_size) = page_size {
            query.push(format!("maxitems={page_size}"));
        }
        let query = query.join("&");
        let reply = list(router, &query).await;
        let page = entries(&reply.body);
        // "The maximum number of geolocations to be included in the response body
        // for this request" — the request's own bound, and no bound at all when it
        // names none.
        let wanted = page_size.map_or(usize::MAX, |size| size.max(1) as usize);
        assert!(
            page.len() <= wanted,
            "case {query:?}: a page carries at most {wanted} location(s), got {}",
            page.len()
        );
        walked.extend(page);

        let truncated = reply.first("IsTruncated").unwrap_or("");
        assert!(
            truncated == "true" || truncated == "false",
            "case {query:?}: IsTruncated is a boolean, got {truncated:?}"
        );
        if truncated == "false" {
            assert_eq!(
                reply.first("NextContinentCode"),
                None,
                "case {query:?}: a listing that ends names no location to resume at, got body {}",
                reply.body
            );
            assert_eq!(
                reply.first("NextCountryCode"),
                None,
                "case {query:?}: a listing that ends names no location to resume at, got body {}",
                reply.body
            );
            return walked;
        }

        resume = [
            ("startcontinentcode", reply.first("NextContinentCode")),
            ("startcountrycode", reply.first("NextCountryCode")),
            ("startsubdivisioncode", reply.first("NextSubdivisionCode")),
        ]
        .into_iter()
        .filter_map(|(parameter, code)| code.map(|code| format!("{parameter}={code}")))
        .collect();
        assert!(
            !resume.is_empty(),
            "case {query:?}: a truncated listing names the location to resume at, got body {}",
            reply.body
        );
        // The catalog is finite and every page carries at least one location, so a
        // walk that reaches this many locations is not walking forward any more.
        assert!(
            walked.len() < 10_000,
            "case {query:?}: the walk does not terminate; {} location(s) so far",
            walked.len()
        );
    }
}

/// One `GetGeoLocation` call, with the query string `query`.
async fn get(router: &MultiServiceRouter, query: &str) -> Reply {
    call(router, "GET", &format!("/2013-04-01/geolocation?{query}"), "").await
}

/// The query string that asks about `(country code, subdivision code)`, with an
/// empty subdivision meaning the country itself.
fn location_query(country_code: &str, subdivision_code: &str) -> String {
    if subdivision_code.is_empty() {
        format!("countrycode={country_code}")
    } else {
        format!("countrycode={country_code}&subdivisioncode={subdivision_code}")
    }
}

#[tokio::test]
async fn every_listed_location_is_one_well_formed_kind_and_is_listed_once() {
    // One row per location the service publishes — every one of them, not the one
    // a recording happened to carry.
    let catalog = walk(&router(), None).await;
    assert!(
        catalog.len() > PUBLISHED_CONTINENTS.len() + PUBLISHED_LOCATIONS.len(),
        "the catalog must hold the supported locations, got {} entry/entries",
        catalog.len()
    );

    let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
    for entry in &catalog {
        let case = entry.codes();
        let kind = kind(entry).unwrap_or_else(|| {
            panic!("case {case}: every location is a continent, a country or a subdivision of one, got {entry:?}")
        });
        // The modeled code and name lengths: `GeoLocationContinentCode` is
        // `length 2..2`, `GeoLocationCountryCode` `1..2`,
        // `GeoLocationSubdivisionCode` `1..3`, and the names `1..32` (continent)
        // and `1..64` (country, subdivision).
        match kind {
            Kind::Continent => {
                let code = entry.continent_code.clone().unwrap_or_default();
                let name = entry.continent_name.clone().unwrap_or_default();
                assert_eq!(
                    code.len(),
                    2,
                    "case {case}: a continent code is two characters, got {code:?}"
                );
                assert!(
                    (1..=32).contains(&name.len()),
                    "case {case}: a continent name is 1..32 characters, got {name:?}"
                );
            }
            Kind::Country => {
                let code = entry.country_code.clone().unwrap_or_default();
                let name = entry.country_name.clone().unwrap_or_default();
                assert!(
                    (1..=2).contains(&code.len()),
                    "case {case}: a country code is 1..2 characters, got {code:?}"
                );
                assert!(
                    (1..=64).contains(&name.len()),
                    "case {case}: a country name is 1..64 characters, got {name:?}"
                );
            }
            Kind::Subdivision => {
                let code = entry.subdivision_code.clone().unwrap_or_default();
                let name = entry.subdivision_name.clone().unwrap_or_default();
                assert!(
                    (1..=3).contains(&code.len()),
                    "case {case}: a subdivision code is 1..3 characters, got {code:?}"
                );
                assert!(
                    (1..=64).contains(&name.len()),
                    "case {case}: a subdivision name is 1..64 characters, got {name:?}"
                );
            }
        }
        let codes = (
            entry.continent_code.clone().unwrap_or_default(),
            entry.country_code.clone().unwrap_or_default(),
            entry.subdivision_code.clone().unwrap_or_default(),
        );
        assert!(
            seen.insert(codes.clone()),
            "case {case}: a location is listed once, {codes:?} came twice"
        );
    }

    // The published sets reach the wire: the seven continents with their names,
    // and locations of each kind the recorded case did not name.
    let continents: BTreeSet<(String, String)> = catalog
        .iter()
        .filter(|entry| kind(entry) == Some(Kind::Continent))
        .map(|entry| {
            (
                entry.continent_code.clone().unwrap_or_default(),
                entry.continent_name.clone().unwrap_or_default(),
            )
        })
        .collect();
    let published: BTreeSet<(String, String)> = PUBLISHED_CONTINENTS
        .iter()
        .map(|(code, name)| ((*code).to_string(), (*name).to_string()))
        .collect();
    assert_eq!(
        continents, published,
        "the catalog must list the seven supported continents, got {continents:?}"
    );

    for (country_code, subdivision_code) in PUBLISHED_LOCATIONS {
        let case = location_query(country_code, subdivision_code);
        let found = catalog.iter().any(|entry| {
            entry.country_code.as_deref() == Some(*country_code)
                && entry.subdivision_code.as_deref()
                    == (!subdivision_code.is_empty()).then_some(*subdivision_code)
        });
        assert!(
            found,
            "case {case:?}: the catalog must list this supported location, got {} entry/entries",
            catalog.len()
        );
    }
}

#[tokio::test]
async fn countries_come_first_with_their_subdivisions_inside_them_and_continents_last() {
    let catalog = walk(&router(), None).await;

    // Countries first and continents last: no country-side entry follows a
    // continent.
    let first_continent = catalog
        .iter()
        .position(|entry| kind(entry) == Some(Kind::Continent))
        .unwrap_or_else(|| panic!("the catalog must list continents, got {} entries", catalog.len()));
    for (at, entry) in catalog.iter().enumerate().skip(first_continent) {
        assert_eq!(
            kind(entry),
            Some(Kind::Continent),
            "case {}: countries are listed first and continents last, so entry {at} after the \
             first continent at {first_continent} must be a continent",
            entry.codes()
        );
    }

    // Each country's subdivisions sit immediately after it, in ascending code
    // order, and the countries themselves ascend: so walking the country side
    // gives a strictly ascending country code, and inside one country a strictly
    // ascending subdivision code.
    let mut previous_country: Option<String> = None;
    let mut previous_subdivision: Option<String> = None;
    for entry in catalog.iter().take(first_continent) {
        let case = entry.codes();
        let country = entry.country_code.clone().unwrap_or_default();
        match entry.subdivision_code.clone() {
            None => {
                if let Some(previous) = &previous_country {
                    assert!(
                        country.as_str() > previous.as_str(),
                        "case {case}: countries are listed in ascending code order, {country:?} \
                         came after {previous:?}"
                    );
                }
                previous_country = Some(country);
                previous_subdivision = None;
            }
            Some(subdivision) => {
                assert_eq!(
                    Some(&country),
                    previous_country.as_ref(),
                    "case {case}: a subdivision is listed immediately after its own country, \
                     which was {previous_country:?}"
                );
                if let Some(previous) = &previous_subdivision {
                    assert!(
                        subdivision.as_str() > previous.as_str(),
                        "case {case}: a country's subdivisions are listed in ascending code \
                         order, {subdivision:?} came after {previous:?}"
                    );
                }
                previous_subdivision = Some(subdivision);
            }
        }
    }
}

#[tokio::test]
async fn a_page_is_bounded_by_maxitems_and_the_next_codes_resume_after_it() {
    // The rows differ along the page size, across the published range of
    // `maxitems`: its minimum, sizes that split the catalog inside a country's
    // subdivisions and at a kind boundary, the published maximum, and one past it.
    let router = router();
    let whole = walk(&router, None).await;

    for page_size in [1, 2, 7, 13, 51, 99, PAGE_BOUND, PAGE_BOUND + 1, 1_000] {
        let paged = walk(&router, Some(page_size)).await;
        assert_eq!(
            paged.len(),
            whole.len(),
            "case maxitems={page_size}: paging visits the catalog once, got {} of {} location(s)",
            paged.len(),
            whole.len()
        );
        assert_eq!(
            paged, whole,
            "case maxitems={page_size}: paging visits the catalog in one order"
        );
    }
}

#[tokio::test]
async fn the_response_echoes_the_maxitems_the_request_asked_for() {
    // "The value that you specified for MaxItems in the request" — echoed as
    // asked, and the page bounded by that same value and by nothing else: this
    // operation's `MaxItems` documents no bound of Route 53's own, so the rows
    // past `PAGE_BOUND` are answered in full rather than cut to 100.
    let router = router();
    let catalog = walk(&router, Some(50)).await.len();
    for asked in [1, 5, PAGE_BOUND, PAGE_BOUND + 1, 4_096] {
        let reply = list(&router, &format!("maxitems={asked}")).await;
        assert_eq!(
            reply.first("MaxItems"),
            Some(asked.to_string().as_str()),
            "case maxitems={asked}: the response echoes the value asked for, got {:?}",
            reply.first("MaxItems")
        );
        let page = entries(&reply.body);
        let expected = (asked as usize).min(catalog);
        assert_eq!(
            page.len(),
            expected,
            "case maxitems={asked}: a page carries what was asked for, bounded only by the \
             {catalog} location(s) left, got {} location(s)",
            page.len()
        );
        let truncated = reply.first("IsTruncated");
        assert_eq!(
            truncated,
            Some(if expected < catalog { "true" } else { "false" }),
            "case maxitems={asked}: IsTruncated says whether a location follows the page of \
             {expected} of {catalog}, got {truncated:?}"
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_maxitems_reads_the_whole_catalog_in_one_page() {
    // A `maxitems` the request does not name bounds nothing, so the one page
    // carries every location, nothing follows it, and the `MaxItems` read back is
    // the bound that did apply — the number of locations the catalog holds.
    let router = router();
    let whole = walk(&router, Some(50)).await;

    let reply = list(&router, "").await;
    let page = entries(&reply.body);
    assert_eq!(
        page.len(),
        whole.len(),
        "case {:?}: the one page carries every location, got {} of {}",
        "no maxitems",
        page.len(),
        whole.len()
    );
    assert_eq!(
        page, whole,
        "case {:?}: the one page carries the catalog in its one order",
        "no maxitems"
    );
    assert_eq!(
        reply.first("IsTruncated"),
        Some("false"),
        "case {:?}: nothing follows a page that carries the whole catalog, got body {}",
        "no maxitems",
        reply.body
    );
    assert_eq!(
        reply.first("MaxItems"),
        Some(whole.len().to_string().as_str()),
        "case {:?}: the bound that applied is the catalog's own size, got {:?}",
        "no maxitems",
        reply.first("MaxItems")
    );
}

#[tokio::test]
async fn every_listed_location_is_resolved_by_get_geo_location() {
    // The round trip: each kind of listed location is asked about on its own and
    // comes back as the very entry the listing carried. The rows span the catalog
    // — its first and last location and every seventeenth in between, so
    // continents, countries and subdivisions are all covered — plus every
    // published location named above.
    let router = router();
    let catalog = walk(&router, None).await;

    let mut sampled: Vec<Entry> = catalog
        .iter()
        .enumerate()
        .filter(|(at, _)| *at % 17 == 0 || *at + 1 == catalog.len())
        .map(|(_, entry)| entry.clone())
        .collect();
    for (country_code, subdivision_code) in PUBLISHED_LOCATIONS {
        if let Some(entry) = catalog.iter().find(|entry| {
            entry.country_code.as_deref() == Some(*country_code)
                && entry.subdivision_code.as_deref()
                    == (!subdivision_code.is_empty()).then_some(*subdivision_code)
        }) {
            sampled.push(entry.clone());
        }
    }

    for entry in sampled {
        let case = entry.codes();
        let query = match kind(&entry) {
            Some(Kind::Continent) => format!(
                "continentcode={}",
                entry.continent_code.clone().unwrap_or_default()
            ),
            Some(Kind::Country) => {
                location_query(&entry.country_code.clone().unwrap_or_default(), "")
            }
            Some(Kind::Subdivision) => location_query(
                &entry.country_code.clone().unwrap_or_default(),
                &entry.subdivision_code.clone().unwrap_or_default(),
            ),
            None => panic!("case {case}: a listed location names a kind"),
        };
        let reply = get(&router, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {query:?}: expected 200 from GetGeoLocation (501 = still scaffolded), got {} \
             with body {}",
            reply.status, reply.body
        );
        let resolved = entries(&reply.body);
        assert_eq!(
            resolved.len(),
            1,
            "case {query:?}: one location is asked about, so one is reported, got {resolved:?}"
        );
        assert_eq!(
            resolved.first(),
            Some(&entry),
            "case {query:?}: GetGeoLocation reports the listed location unchanged, got {resolved:?}"
        );
    }
}

#[tokio::test]
async fn a_code_selects_the_same_location_however_it_is_spelled() {
    // A geolocation code is an identifier, so the same location is selected
    // whichever case a client spells it in, and the response carries the
    // catalog's own spelling. The rows differ along the kind and the spelling.
    let router = router();
    let cases = [
        ("continentcode=eu", "continentcode=EU"),
        ("continentcode=Eu", "continentcode=EU"),
        ("countrycode=de", "countrycode=DE"),
        ("countrycode=us&subdivisioncode=ny", "countrycode=US&subdivisioncode=NY"),
        ("countrycode=US&subdivisioncode=Ca", "countrycode=US&subdivisioncode=CA"),
    ];
    for (spelled, canonical) in cases {
        let spelled_reply = get(&router, spelled).await;
        assert_eq!(
            spelled_reply.status, 200,
            "case {spelled:?}: expected 200 from GetGeoLocation, got {} with body {}",
            spelled_reply.status, spelled_reply.body
        );
        let canonical_reply = get(&router, canonical).await;
        assert_eq!(
            entries(&spelled_reply.body),
            entries(&canonical_reply.body),
            "case {spelled:?}: must resolve the location {canonical:?} names, got body {}",
            spelled_reply.body
        );
    }
}

#[tokio::test]
async fn a_location_the_catalog_does_not_hold_is_refused_as_no_such_geo_location() {
    // The rows differ along the kind of location asked about and along which part
    // of it the catalog does not hold: an unassigned country code, an unassigned
    // continent code, a subdivision code of a real country, and a subdivision of
    // a country that has none.
    let router = router();
    let cases = [
        ("countrycode=ZZ", "No geo location for location: [country = ZZ]"),
        ("countrycode=QQ", "No geo location for location: [country = QQ]"),
        ("continentcode=XX", "No geo location for location: [continent = XX]"),
        ("continentcode=ZZ", "No geo location for location: [continent = ZZ]"),
        (
            "countrycode=US&subdivisioncode=ZZ",
            "No geo location for location: [country = US, subdivision = ZZ]",
        ),
        (
            "countrycode=DE&subdivisioncode=BY",
            "No geo location for location: [country = DE, subdivision = BY]",
        ),
    ];
    for (query, message) in cases {
        let reply = get(&router, query).await;
        assert_eq!(
            reply.status, 404,
            "case {query:?}: an unsupported location is refused 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchGeoLocation",
            "case {query:?}: expected the NoSuchGeoLocation code, got body {}",
            reply.body
        );
        assert_eq!(
            reply.first("Error/Message"),
            Some(message),
            "case {query:?}: expected the refusal to name the location asked about, got {:?}",
            reply.first("Error/Message")
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_single_location_is_refused_as_invalid_input() {
    // The three documented forms are `continentcode`, `countrycode`, and
    // `countrycode` with `subdivisioncode`; the rows below are the combinations
    // that name no location — a continent beside a country, a continent beside a
    // subdivision, a subdivision with no country, and nothing at all — on both
    // the getter and the listing, whose start codes take the same three forms.
    let router = router();
    let getter = [
        "continentcode=EU&countrycode=DE",
        "continentcode=EU&subdivisioncode=NY",
        "continentcode=EU&countrycode=US&subdivisioncode=NY",
        "subdivisioncode=NY",
        "",
    ];
    for query in getter {
        let reply = call(
            &router,
            "GET",
            &format!("/2013-04-01/geolocation?{query}"),
            "",
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {query:?}: a request naming no single location is refused 400, got {} with \
             body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidInput",
            "case {query:?}: expected the InvalidInput code, got body {}",
            reply.body
        );
    }

    let listing = [
        "startcontinentcode=EU&startcountrycode=DE",
        "startcontinentcode=EU&startsubdivisioncode=NY",
        "startsubdivisioncode=NY",
        // A start code that names no supported location positions nothing, so the
        // caller is told so rather than handed a page starting somewhere else.
        "startcountrycode=ZZ",
        "startcontinentcode=XX",
        "startcountrycode=US&startsubdivisioncode=ZZ",
    ];
    for query in listing {
        let reply = call(
            &router,
            "GET",
            &format!("/2013-04-01/geolocations?{query}"),
            "",
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {query:?}: a start code naming no location is refused 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidInput",
            "case {query:?}: expected the InvalidInput code, got body {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn the_start_codes_of_a_truncated_page_name_the_next_location() {
    // The `Next*Code`s are the codes of the location AFTER the page, so a page of
    // n locations followed by a page resumed at them covers the first n+1
    // locations of the catalog, whichever kind the boundary falls on. The rows
    // differ along where that boundary falls.
    let router = router();
    let whole = walk(&router, None).await;

    for page_size in [1, 3, 52, PAGE_BOUND] {
        let first = list(&router, &format!("maxitems={page_size}")).await;
        let page = entries(&first.body);
        assert_eq!(
            first.first("IsTruncated"),
            Some("true"),
            "case maxitems={page_size}: the catalog is longer than one page, got body {}",
            first.body
        );
        let resumed: Vec<String> = [
            ("startcontinentcode", first.first("NextContinentCode")),
            ("startcountrycode", first.first("NextCountryCode")),
            ("startsubdivisioncode", first.first("NextSubdivisionCode")),
        ]
        .into_iter()
        .filter_map(|(parameter, code)| code.map(|code| format!("{parameter}={code}")))
        .collect();
        let query = format!("{}&maxitems=1", resumed.join("&"));
        let next = list(&router, &query).await;
        let next_page = entries(&next.body);
        assert_eq!(
            next_page.first(),
            whole.get(page.len()),
            "case {query:?}: the next codes name the location after a page of {}, got \
             {next_page:?}",
            page.len()
        );
    }
}

#[tokio::test]
async fn the_same_catalog_answers_every_call_whatever_the_account_does() {
    // The catalog is service-owned, so the account's own activity cannot move it,
    // and reading it creates and destroys nothing. The rows differ along what the
    // account does between two reads: nothing, a zone created, more zones created.
    let router = router();
    let first = walk(&router, None).await;

    let repeated = walk(&router, None).await;
    assert_eq!(
        repeated, first,
        "case {:?}: a repeated read must report the same catalog",
        "read twice with nothing in between"
    );

    for index in 0..3u32 {
        create_zone_ok(
            &router,
            &format!("zone-{index}.geo.example.com"),
            &format!("geo-catalog-ref-{index}"),
        )
        .await;
        let again = walk(&router, None).await;
        assert_eq!(
            again, first,
            "case {:?}: creating a hosted zone must not move the catalog",
            format!("{} hosted zone(s) created", index + 1)
        );
    }

    // Two routers, two engines, two stores: the catalog comes from the SERVICE, so
    // it does not depend on what any one store holds.
    let separate = crate::hosted_zone_records::router();
    let fresh = walk(&separate, None).await;
    assert_eq!(
        fresh, first,
        "a fresh store must report the same service-owned catalog"
    );
}
