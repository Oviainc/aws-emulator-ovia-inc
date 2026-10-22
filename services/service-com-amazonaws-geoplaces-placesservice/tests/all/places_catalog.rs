// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Geo Places emulator's first slice: Geocode,
//! GetPlace, ReverseGeocode, SearchText and SearchNearby, driven as real HTTP
//! requests through the same `MultiServiceRouter` the production front door serves.
//!
//! The rules under test are catalog-wide rather than per-place, so each test is a
//! TABLE whose rows either span the whole seeded catalog (every place, read out of
//! the engine's own state) or differ along what the rule generalises over (query
//! form, page size, radius, filter, requested feature, intended use). Where a bound
//! or a vocabulary comes from the Smithy model, the comment beside it names the
//! member it belongs to.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_geoplaces_placesservice::ComAmazonawsGeoplacesPlacesserviceState as Catalog;
use service_com_amazonaws_geoplaces_placesservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A host-style endpoint for the service, so a request routes even if the SigV4
/// scope below ever changes.
const ENDPOINT: &str = "https://geo-places.us-east-1.amazonaws.com";

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name (`geo-places`, from `OPERATIONS.md`): that scope is how the router
/// picks the service which claims a request. Request authentication is off in these
/// tests, so the signature itself is never verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATOR000000000/20260101/us-east-1/geo-places/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The response header `PricingBucket` is bound to on every output
/// (`@httpHeader(x-amz-geo-pricing-bucket)`).
pub(crate) const PRICING_BUCKET_HEADER: &str = "x-amz-geo-pricing-bucket";

/// One HTTP exchange.
pub(crate) struct Call {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: bytes::Bytes,
}

impl Call {
    pub(crate) fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "response body is not JSON ({error}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    pub(crate) fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// The wire error code: the `x-amzn-errortype` header restJson1 sets on a
    /// boundary refusal, or the body's `__type` for an error a handler returned.
    pub(crate) fn error_code(&self) -> String {
        let raw = if self.headers.contains_key("x-amzn-errortype") {
            self.header("x-amzn-errortype")
        } else {
            self.json()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        raw.split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// One place as the seeded catalog holds it: the rows of every catalog-wide table.
#[derive(Debug, Clone)]
pub(crate) struct Place {
    pub(crate) place_id: String,
    pub(crate) place_type: String,
    /// The catalog record's own name, which is how a test names the place it
    /// wants. For a point of interest this is also the reported `Title`; for an
    /// address or an administrative area the reported `Title` is the assembled
    /// `label` below.
    pub(crate) name: String,
    /// The `Title` the service reports for this place.
    pub(crate) title: String,
    pub(crate) label: String,
    pub(crate) position: [f64; 2],
}

/// A router with only this service mounted — one emulator "session".
pub(crate) struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// The catalog the emulator serves, read from the shared engine's own state —
    /// the table every "for every place" test iterates.
    pub(crate) fn catalog(&self) -> Vec<Place> {
        let state = self.engine.state::<Catalog>();
        let catalog = state.lock();
        catalog
            .places
            .iter()
            .map(|place| Place {
                place_id: place.place_id.clone(),
                place_type: place.place_type.clone(),
                name: place.title.clone(),
                title: place.title_modeled(),
                label: place.assembled_label(),
                position: place.position,
            })
            .collect()
    }

    async fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Call {
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("{ENDPOINT}{path}"))
            .header("authorization", AUTHORIZATION)
            .header("host", "geo-places.us-east-1.amazonaws.com")
            .header("content-type", "application/json");
        if method == "GET" {
            request = request.header("content-length", "0");
        }
        let payload = body
            .map(|body| serde_json::to_vec(body).expect("a serializable test body"))
            .unwrap_or_default();
        let request = request
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Call {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        }
    }

    pub(crate) async fn post(&self, path: &str, body: &Value) -> Call {
        self.send("POST", path, Some(body)).await
    }

    pub(crate) async fn get(&self, path: &str) -> Call {
        self.send("GET", path, None).await
    }

    /// POST and require a success: HTTP 200 plus the required pricing-bucket header.
    pub(crate) async fn post_ok(&self, case: &str, path: &str, body: &Value) -> Value {
        let call = self.post(path, body).await;
        assert_success(case, &call);
        call.json()
    }

    /// GET and require a success.
    pub(crate) async fn get_ok(&self, case: &str, path: &str) -> Value {
        let call = self.get(path).await;
        assert_success(case, &call);
        call.json()
    }
}

/// Every success carries HTTP 200 and the `@required` `PricingBucket` header.
pub(crate) fn assert_success(case: &str, call: &Call) {
    assert_eq!(
        call.status,
        200,
        "case {case}: expected 200, got {} with {}",
        call.status,
        String::from_utf8_lossy(&call.body)
    );
    let bucket = call.header(PRICING_BUCKET_HEADER);
    assert!(
        !bucket.is_empty(),
        "case {case}: expected a non-empty {PRICING_BUCKET_HEADER} header, got {:?}",
        call.headers
    );
}

/// `ResultItems` of a search response.
pub(crate) fn items(body: &Value) -> Vec<Value> {
    body.get("ResultItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A string member of a JSON object, or the empty string when absent.
pub(crate) fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `Distance`, which carries a modeled `@default` of 0, so an absent member reads
/// as zero.
pub(crate) fn distance(item: &Value) -> i64 {
    item.get("Distance").and_then(Value::as_i64).unwrap_or(0)
}

/// `Position`, as `[longitude, latitude]`.
fn position(item: &Value) -> Vec<f64> {
    item.get("Position")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// The `PlaceId`s of a response's items, in order.
fn place_ids(body: &Value) -> Vec<String> {
    items(body)
        .iter()
        .map(|item| text(item, "PlaceId"))
        .collect()
}

/// The category ids of one result item.
fn category_ids(item: &Value) -> Vec<String> {
    item.get("Categories")
        .and_then(Value::as_array)
        .map(|categories| {
            categories
                .iter()
                .map(|category| text(category, "Id"))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Geocode
// ---------------------------------------------------------------------------

/// A row of the geocode table: a query form and the catalog titles it must find.
struct GeocodeRow {
    case: &'static str,
    body: Value,
    /// Titles the response must contain. Empty means the query matches nothing,
    /// which is a 200 with an empty `ResultItems` — never a fabricated result.
    expected_titles: &'static [&'static str],
}

/// Geocode matches the catalog by free-form text and by address component, and a
/// query that matches nothing returns no items rather than an invented one.
///
/// The rows differ along the query FORM (full label, abbreviated label, place name,
/// postal code, structured components at several granularities, unmatchable text),
/// which is what the matching rule generalises over. A free-form query answers with
/// whatever its words name; a structured query answers at the granularity of the
/// finest component it supplies.
#[tokio::test]
async fn geocode_matches_the_catalog_by_text_and_by_component() {
    let emulator = Emulator::new();
    let rows = vec![
        GeocodeRow {
            case: "full-label",
            body: json!({"QueryText": "1600 Pennsylvania Ave NW, Washington, DC 20500"}),
            expected_titles: &[
                "White House",
                "1600 Pennsylvania Ave NW, Washington, DC 20500, United States",
            ],
        },
        GeocodeRow {
            case: "abbreviation-expanded",
            body: json!({"QueryText": "1600 Pennsylvania Avenue NW, Washington, DC"}),
            expected_titles: &[
                "White House",
                "1600 Pennsylvania Ave NW, Washington, DC 20500, United States",
            ],
        },
        GeocodeRow {
            case: "place-name",
            body: json!({"QueryText": "Space Needle"}),
            expected_titles: &["Space Needle"],
        },
        GeocodeRow {
            case: "postal-code",
            body: json!({"QueryText": "75007 Paris"}),
            expected_titles: &["Tour Eiffel"],
        },
        GeocodeRow {
            case: "components-locality-and-number",
            body: json!({"QueryComponents": {"Locality": "Seattle", "AddressNumber": "400"}}),
            // The finest component is an address number, so the answer is the
            // point address — not the point of interest that shares its number.
            expected_titles: &["400 Broad St, Seattle, WA 98109, United States"],
        },
        GeocodeRow {
            case: "components-country-alpha3",
            body: json!({"QueryComponents": {"Country": "FRA"}}),
            expected_titles: &["France"],
        },
        GeocodeRow {
            case: "components-region",
            body: json!({"QueryComponents": {"Country": "USA", "Region": "Washington"}}),
            expected_titles: &["Washington, United States"],
        },
        GeocodeRow {
            case: "components-sub-region",
            body: json!({"QueryComponents": {"Region": "WA", "SubRegion": "King County"}}),
            expected_titles: &["King County, WA, United States"],
        },
        GeocodeRow {
            case: "components-locality",
            body: json!({
                "QueryComponents": {
                    "Country": "USA",
                    "Region": "Washington",
                    "Locality": "Seattle",
                },
            }),
            expected_titles: &["Seattle, WA, United States"],
        },
        GeocodeRow {
            case: "components-district",
            body: json!({"QueryComponents": {"Locality": "Seattle", "District": "Capitol Hill"}}),
            expected_titles: &["Capitol Hill, Seattle, WA, United States"],
        },
        GeocodeRow {
            case: "components-street",
            body: json!({"QueryComponents": {"Locality": "Seattle", "Street": "Broad Street"}}),
            expected_titles: &["400 Broad St, Seattle, WA 98109, United States"],
        },
        GeocodeRow {
            case: "text-and-component-must-both-hold",
            body: json!({
                "QueryText": "Pike Place Market",
                "QueryComponents": {"Locality": "Seattle"},
            }),
            // The free-form text and the component must BOTH hold; the structured
            // granularity does not apply to a query that names a place in text.
            expected_titles: &["Pike Place Market"],
        },
        GeocodeRow {
            case: "no-match",
            body: json!({"QueryText": "Atlantis Undersea Resort"}),
            expected_titles: &[],
        },
        GeocodeRow {
            case: "component-no-match",
            body: json!({"QueryComponents": {"Locality": "Reykjavik"}}),
            expected_titles: &[],
        },
        GeocodeRow {
            case: "component-granularity-the-catalog-has-no-place-at",
            // The catalog holds no place at postal-code granularity, so the honest
            // answer is no items rather than a place of another granularity.
            body: json!({"QueryComponents": {"PostalCode": "98109"}}),
            expected_titles: &[],
        },
    ];

    for row in rows {
        let body = emulator.post_ok(row.case, "/v2/geocode", &row.body).await;
        let returned: Vec<String> = items(&body)
            .iter()
            .map(|item| text(item, "Title"))
            .collect();
        for expected in row.expected_titles {
            assert!(
                returned.iter().any(|title| title == expected),
                "case {}: expected {expected:?} among the results, got {returned:?}",
                row.case
            );
        }
        if row.expected_titles.is_empty() {
            assert!(
                returned.is_empty(),
                "case {}: a query matching nothing must return no items, got {returned:?}",
                row.case
            );
        }
        // Every item is a whole place: the members GetPlace also reports.
        for item in items(&body) {
            assert!(
                !text(&item, "PlaceId").is_empty()
                    && !text(&item, "PlaceType").is_empty()
                    && !text(&item, "Title").is_empty()
                    && position(&item).len() == 2
                    && item.get("Address").is_some(),
                "case {}: an item must carry PlaceId, PlaceType, Title, Position and \
                 Address, got {item:?}",
                row.case
            );
        }
    }
}

/// `MaxResults` caps a geocode page, at every value from the bottom of the modeled
/// range (`GeocodeRequest$MaxResults`, `range 1..100`) up past the number of matches.
#[tokio::test]
async fn geocode_honours_max_results_as_a_cap() {
    let emulator = Emulator::new();
    let unlimited = emulator
        .post_ok(
            "uncapped",
            "/v2/geocode",
            &json!({"QueryText": "Washington", "MaxResults": 100}),
        )
        .await;
    let all = place_ids(&unlimited);
    assert!(
        all.len() > 2,
        "case fixture: the catalog must hold several places in Washington, got {}",
        all.len()
    );

    for max_results in [1_usize, 2, 3, all.len(), all.len() + 1, 100] {
        let case = format!("max-results-{max_results}");
        let body = emulator
            .post_ok(
                &case,
                "/v2/geocode",
                &json!({"QueryText": "Washington", "MaxResults": max_results}),
            )
            .await;
        let returned = place_ids(&body);
        assert!(
            returned.len() <= max_results,
            "case {case}: a page must hold at most MaxResults items, got {}",
            returned.len()
        );
        assert_eq!(
            returned,
            all[..returned.len()].to_vec(),
            "case {case}: a capped page must be the prefix of the uncapped result"
        );
    }
}

/// With a `BiasPosition`, equally relevant results come back nearest first, and
/// `Distance` is measured from that position.
#[tokio::test]
async fn geocode_orders_by_distance_from_the_bias_position() {
    let emulator = Emulator::new();
    // Every row is a different bias position, so no single ordering can satisfy them
    // all: each must be ordered around its own bias.
    for place in emulator.catalog() {
        let case = format!("bias-at-{}", place.name);
        let body = emulator
            .post_ok(
                &case,
                "/v2/geocode",
                &json!({
                    "QueryText": place.label,
                    "BiasPosition": [place.position[0], place.position[1]],
                    "MaxResults": 100,
                }),
            )
            .await;
        let returned = items(&body);
        assert!(
            !returned.is_empty(),
            "case {case}: a place's own address label must geocode to at least one place"
        );
        let distances: Vec<i64> = returned.iter().map(distance).collect();
        assert!(
            distances.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case}: expected non-decreasing distances, got {distances:?}"
        );
        assert_eq!(
            distances[0], 0,
            "case {case}: the place at the bias position itself must come first, \
             at distance 0; got {distances:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// GetPlace: the round trip
// ---------------------------------------------------------------------------

/// EVERY `PlaceId` the search operations hand out resolves through GetPlace to the
/// same record. The table is the whole catalog, reached through three different
/// operations.
#[tokio::test]
async fn every_place_id_a_search_returns_resolves_through_get_place() {
    let emulator = Emulator::new();
    for place in emulator.catalog() {
        for (operation, path, body) in [
            (
                "Geocode",
                "/v2/geocode",
                json!({"QueryText": place.title, "MaxResults": 100}),
            ),
            (
                "SearchText",
                "/v2/search-text",
                json!({"QueryText": place.title, "MaxResults": 100}),
            ),
            (
                "ReverseGeocode",
                "/v2/reverse-geocode",
                json!({
                    "QueryPosition": [place.position[0], place.position[1]],
                    "QueryRadius": 1,
                    "MaxResults": 100,
                }),
            ),
        ] {
            let case = format!("{operation}-{}", place.name);
            let found = emulator.post_ok(&case, path, &body).await;
            let item = items(&found)
                .into_iter()
                .find(|item| text(item, "PlaceId") == place.place_id)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: expected {operation} to return the place's own id, \
                         got {:?}",
                        place_ids(&found)
                    )
                });

            let detail = emulator
                .get_ok(&case, &format!("/v2/place/{}", place.place_id))
                .await;
            for member in ["PlaceId", "PlaceType", "Title"] {
                assert_eq!(
                    text(&detail, member),
                    text(&item, member),
                    "case {case}: GetPlace must report the same {member} as {operation}"
                );
            }
            assert_eq!(
                position(&detail),
                position(&item),
                "case {case}: GetPlace must report the same Position as {operation}"
            );
            assert_eq!(
                detail.get("Address"),
                item.get("Address"),
                "case {case}: GetPlace must report the same Address as {operation}"
            );
            assert_eq!(
                text(&detail, "Title"),
                place.title,
                "case {case}: GetPlace must report the catalog's own Title"
            );
            assert_eq!(
                text(&detail, "PlaceType"),
                place.place_type,
                "case {case}: GetPlace must report the catalog's own PlaceType"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ReverseGeocode
// ---------------------------------------------------------------------------

/// A `Position` any operation returned reverse-geocodes back to the place it came
/// from, at a `Distance` of zero. The table is the whole catalog.
#[tokio::test]
async fn a_position_reverse_geocodes_to_the_place_it_came_from() {
    let emulator = Emulator::new();
    for place in emulator.catalog() {
        let case = format!("reverse-{}", place.name);
        let body = emulator
            .post_ok(
                &case,
                "/v2/reverse-geocode",
                &json!({
                    "QueryPosition": [place.position[0], place.position[1]],
                    "QueryRadius": 50,
                }),
            )
            .await;
        let returned = items(&body);
        assert_eq!(
            returned.len(),
            1,
            "case {case}: ReverseGeocode's modeled default MaxResults is 1, so one \
             item is expected; got {}",
            returned.len()
        );
        assert_eq!(
            text(&returned[0], "PlaceId"),
            place.place_id,
            "case {case}: expected the place at the queried position, got {:?}",
            text(&returned[0], "Title")
        );
        assert_eq!(
            distance(&returned[0]),
            0,
            "case {case}: a place at the queried position is zero metres away"
        );
    }
}

/// `QueryRadius` bounds the results, and a wider radius never drops a place a
/// narrower one returned. The rows span the modeled range
/// (`ReverseGeocodeRequest$QueryRadius`, `range 1..21000000`), including both bounds.
#[tokio::test]
async fn reverse_geocode_respects_the_query_radius() {
    let emulator = Emulator::new();
    let center = emulator
        .catalog()
        .into_iter()
        .find(|place| place.name == "Seattle" && place.place_type == "Locality")
        .expect("the catalog seeds the locality of Seattle");

    let mut previous: BTreeSet<String> = BTreeSet::new();
    for radius in [1_i64, 100, 2_000, 20_000, 5_000_000, 21_000_000] {
        let case = format!("radius-{radius}");
        let body = emulator
            .post_ok(
                &case,
                "/v2/reverse-geocode",
                &json!({
                    "QueryPosition": [center.position[0], center.position[1]],
                    "QueryRadius": radius,
                    "MaxResults": 100,
                }),
            )
            .await;
        let returned = items(&body);
        let distances: Vec<i64> = returned.iter().map(distance).collect();
        assert!(
            distances.iter().all(|metres| *metres <= radius),
            "case {case}: every result must lie within the radius, got {distances:?}"
        );
        assert!(
            distances.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case}: expected ascending distances, got {distances:?}"
        );
        let ids: BTreeSet<String> = returned
            .iter()
            .map(|item| text(item, "PlaceId"))
            .collect();
        assert!(
            previous.is_subset(&ids),
            "case {case}: a wider radius must keep every place a narrower one returned; \
             missing {:?}",
            previous.difference(&ids).collect::<Vec<_>>()
        );
        previous = ids;
    }
    assert!(
        previous.len() > 1,
        "case radius-21000000: the widest radius must reach more than one city, got {}",
        previous.len()
    );
}

// ---------------------------------------------------------------------------
// SearchText and SearchNearby: pagination
// ---------------------------------------------------------------------------

/// Walk a paginated search to its end, following `NextToken`, and return every
/// `PlaceId` in the order the service returned them.
///
/// Checks the pagination contract as it goes: no page exceeds the `MaxResults`
/// asked for, no place is returned twice, and the walk ends on a page with no token.
async fn drain(emulator: &Emulator, case: &str, path: &str, body: &Value, page_size: usize) -> Vec<String> {
    /// More pages than the catalog could ever produce: a token that fails to
    /// advance would otherwise loop forever.
    const PAGE_LIMIT: usize = 100;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut ordered: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;

    loop {
        let mut request = body.clone();
        request["MaxResults"] = json!(page_size);
        match token.as_deref() {
            Some(token) => request["NextToken"] = json!(token),
            None => {
                request.as_object_mut().expect("a JSON object").remove("NextToken");
            }
        }
        let response = emulator.post_ok(case, path, &request).await;
        let page = place_ids(&response);
        assert!(
            page.len() <= page_size,
            "case {case} page {pages}: expected at most {page_size} items, got {}",
            page.len()
        );
        for place_id in page {
            assert!(
                seen.insert(place_id.clone()),
                "case {case} page {pages}: {place_id} was already returned"
            );
            ordered.push(place_id);
        }
        token = response
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(token) = token.as_deref() {
            assert!(
                !ordered.is_empty(),
                "case {case} page {pages}: an empty page must not carry a NextToken ({token})"
            );
        } else {
            break ordered;
        }
        pages += 1;
        assert!(
            pages < PAGE_LIMIT,
            "case {case}: pagination did not terminate within {PAGE_LIMIT} pages"
        );
    }
}

/// SearchText and SearchNearby page with a real token: whatever the page size, the
/// walk returns the same places, in the same order, as one uncapped call, and the
/// final page carries no token.
///
/// The rows differ along operation, query and page size — what the pagination rule
/// generalises over.
#[tokio::test]
async fn paginated_searches_walk_the_whole_result_in_order() {
    let emulator = Emulator::new();
    let seattle = emulator
        .catalog()
        .into_iter()
        .find(|place| place.name == "Seattle" && place.place_type == "Locality")
        .expect("the catalog seeds the locality of Seattle");

    let queries: Vec<(&str, &str, Value)> = vec![
        (
            "search-text-city",
            "/v2/search-text",
            json!({"QueryText": "Seattle"}),
        ),
        (
            "search-text-category",
            "/v2/search-text",
            json!({"QueryText": "tourist attraction"}),
        ),
        (
            "search-text-chain",
            "/v2/search-text",
            json!({"QueryText": "Starbucks"}),
        ),
        (
            "search-nearby-wide",
            "/v2/search-nearby",
            json!({
                "QueryPosition": [seattle.position[0], seattle.position[1]],
                "QueryRadius": 20_000,
            }),
        ),
        (
            "search-nearby-global",
            "/v2/search-nearby",
            json!({
                "QueryPosition": [seattle.position[0], seattle.position[1]],
                "QueryRadius": 21_000_000,
            }),
        ),
    ];

    for (case, path, body) in queries {
        let mut uncapped = body.clone();
        uncapped["MaxResults"] = json!(100);
        let whole = emulator.post_ok(case, path, &uncapped).await;
        let expected = place_ids(&whole);
        assert!(
            !expected.is_empty(),
            "case {case}: the query must match at least one place"
        );
        assert!(
            whole.get("NextToken").is_none(),
            "case {case}: a page holding every match must carry no NextToken"
        );

        for page_size in [1_usize, 2, 3, expected.len(), expected.len() + 1] {
            let case = format!("{case}-page-{page_size}");
            let walked = drain(&emulator, &case, path, &body, page_size).await;
            assert_eq!(
                walked, expected,
                "case {case}: paging must yield the same places, in the same order, \
                 as one uncapped call"
            );
        }
    }
}

/// A `NextToken` belongs to the query that issued it: replayed against a different
/// query, or against the other operation, it is refused rather than answered from
/// the wrong sequence.
#[tokio::test]
async fn a_next_token_only_resumes_its_own_query() {
    let emulator = Emulator::new();
    let first = emulator
        .post_ok(
            "token-issuer",
            "/v2/search-text",
            &json!({"QueryText": "Seattle", "MaxResults": 1}),
        )
        .await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("several places match Seattle, so a first page of one leaves more")
        .to_string();

    let replays: Vec<(&str, &str, Value)> = vec![
        (
            "same-query",
            "/v2/search-text",
            json!({"QueryText": "Seattle", "MaxResults": 1, "NextToken": token.clone()}),
        ),
        (
            "other-query",
            "/v2/search-text",
            json!({"QueryText": "Washington", "MaxResults": 1, "NextToken": token.clone()}),
        ),
        (
            "other-operation",
            "/v2/search-nearby",
            json!({
                "QueryPosition": [-122.330_062, 47.603_832],
                "QueryRadius": 20_000,
                "MaxResults": 1,
                "NextToken": token.clone(),
            }),
        ),
    ];

    for (case, path, body) in replays {
        let call = emulator.post(path, &body).await;
        if case == "same-query" {
            assert_success(case, &call);
            let returned = place_ids(&call.json());
            assert_eq!(
                returned.len(),
                1,
                "case {case}: resuming must return the next page, got {returned:?}"
            );
            assert_ne!(
                returned,
                place_ids(&first),
                "case {case}: the next page must hold a different place"
            );
        } else {
            assert_eq!(
                call.status,
                400,
                "case {case}: a foreign token must be refused, got {} with {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ValidationException",
                "case {case}: expected ValidationException, got {:?}",
                call.error_code()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SearchNearby filters
// ---------------------------------------------------------------------------

/// A category filter keeps exactly the places inside the radius that carry the
/// category — which is what the same radius returns through ReverseGeocode, filtered
/// on the categories those results themselves report.
///
/// The rows are the catalog's own categories, each tested at several radii.
#[tokio::test]
async fn search_nearby_applies_the_category_filter_inside_the_radius() {
    let emulator = Emulator::new();
    let center = emulator
        .catalog()
        .into_iter()
        .find(|place| place.name == "Seattle" && place.place_type == "Locality")
        .expect("the catalog seeds the locality of Seattle");
    let query_position = json!([center.position[0], center.position[1]]);

    for radius in [2_000_i64, 5_000, 21_000_000] {
        // Everything inside the radius, from a different operation over the same
        // catalog: the reference set this filter is checked against.
        let inside = emulator
            .post_ok(
                "reference",
                "/v2/reverse-geocode",
                &json!({
                    "QueryPosition": query_position,
                    "QueryRadius": radius,
                    "MaxResults": 100,
                }),
            )
            .await;
        let categories: BTreeSet<String> = items(&inside)
            .iter()
            .flat_map(category_ids)
            .filter(|id| !id.is_empty())
            .collect();
        assert!(
            !categories.is_empty(),
            "case radius-{radius}: the catalog inside this radius must carry categories"
        );

        for category in categories {
            let case = format!("radius-{radius}-include-{category}");
            let expected: BTreeSet<String> = items(&inside)
                .iter()
                .filter(|item| category_ids(item).contains(&category))
                .map(|item| text(item, "PlaceId"))
                .collect();

            let body = emulator
                .post_ok(
                    &case,
                    "/v2/search-nearby",
                    &json!({
                        "QueryPosition": query_position,
                        "QueryRadius": radius,
                        "MaxResults": 100,
                        "Filter": {"IncludeCategories": [category.clone()]},
                    }),
                )
                .await;
            let returned = items(&body);
            let got: BTreeSet<String> = returned
                .iter()
                .map(|item| text(item, "PlaceId"))
                .collect();
            assert_eq!(
                got, expected,
                "case {case}: an included category must keep exactly the places inside \
                 the radius that carry it"
            );
            let distances: Vec<i64> = returned.iter().map(distance).collect();
            assert!(
                distances.windows(2).all(|pair| pair[0] <= pair[1]),
                "case {case}: expected ascending distances, got {distances:?}"
            );

            // The complementary rule: excluding the same category keeps the rest.
            let case = format!("radius-{radius}-exclude-{category}");
            let body = emulator
                .post_ok(
                    &case,
                    "/v2/search-nearby",
                    &json!({
                        "QueryPosition": query_position,
                        "QueryRadius": radius,
                        "MaxResults": 100,
                        "Filter": {"ExcludeCategories": [category.clone()]},
                    }),
                )
                .await;
            let excluded: BTreeSet<String> = items(&body)
                .iter()
                .map(|item| text(item, "PlaceId"))
                .collect();
            assert!(
                excluded.is_disjoint(&expected),
                "case {case}: an excluded category must drop every place carrying it, \
                 got {:?}",
                excluded.intersection(&expected).collect::<Vec<_>>()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// AdditionalFeatures
// ---------------------------------------------------------------------------

/// A row of the additional-features table: an operation, the feature asked for and
/// the response member that feature carries.
struct FeatureRow {
    case: &'static str,
    path: &'static str,
    /// The request without any `AdditionalFeatures`.
    body: Value,
    /// Where the item lives: `None` for GetPlace, which answers at the root.
    in_result_items: bool,
    feature: &'static str,
    member: &'static str,
}

/// An additional feature is reported only when the request asks for it — across
/// every operation that declares `AdditionalFeatures` and every feature this
/// catalog can answer (`Access`, `Contact`, `TimeZone`, from the per-operation
/// `*AdditionalFeature` enums).
#[tokio::test]
async fn an_additional_feature_is_reported_only_when_requested() {
    let emulator = Emulator::new();
    let needle = emulator
        .catalog()
        .into_iter()
        .find(|place| place.name == "Space Needle")
        .expect("the catalog seeds the Space Needle, which has all three features");
    let nearby = json!({
        "QueryPosition": [needle.position[0], needle.position[1]],
        "QueryRadius": 100,
        "MaxResults": 100,
    });

    let rows = vec![
        FeatureRow {
            case: "geocode-time-zone",
            path: "/v2/geocode",
            body: json!({"QueryText": needle.title.clone()}),
            in_result_items: true,
            feature: "TimeZone",
            member: "TimeZone",
        },
        FeatureRow {
            case: "geocode-access",
            path: "/v2/geocode",
            body: json!({"QueryText": needle.title.clone()}),
            in_result_items: true,
            feature: "Access",
            member: "AccessPoints",
        },
        FeatureRow {
            case: "reverse-geocode-time-zone",
            path: "/v2/reverse-geocode",
            body: nearby.clone(),
            in_result_items: true,
            feature: "TimeZone",
            member: "TimeZone",
        },
        FeatureRow {
            case: "search-text-contact",
            path: "/v2/search-text",
            body: json!({"QueryText": needle.title.clone()}),
            in_result_items: true,
            feature: "Contact",
            member: "Contacts",
        },
        FeatureRow {
            case: "search-text-time-zone",
            path: "/v2/search-text",
            body: json!({"QueryText": needle.title.clone()}),
            in_result_items: true,
            feature: "TimeZone",
            member: "TimeZone",
        },
        FeatureRow {
            case: "search-nearby-contact",
            path: "/v2/search-nearby",
            body: nearby.clone(),
            in_result_items: true,
            feature: "Contact",
            member: "Contacts",
        },
        FeatureRow {
            case: "search-nearby-access",
            path: "/v2/search-nearby",
            body: nearby,
            in_result_items: true,
            feature: "Access",
            member: "AccessPoints",
        },
    ];

    for row in rows {
        let without = emulator.post_ok(row.case, row.path, &row.body).await;
        let mut asked = row.body.clone();
        asked["AdditionalFeatures"] = json!([row.feature]);
        let with = emulator.post_ok(row.case, row.path, &asked).await;

        let pick = |body: &Value| -> Value {
            if row.in_result_items {
                items(body)
                    .into_iter()
                    .find(|item| text(item, "PlaceId") == needle.place_id)
                    .unwrap_or_else(|| panic!("case {}: the query must find the place", row.case))
            } else {
                body.clone()
            }
        };
        assert!(
            pick(&without).get(row.member).is_none(),
            "case {}: {} must be absent when AdditionalFeatures does not ask for {}",
            row.case,
            row.member,
            row.feature
        );
        assert!(
            pick(&with).get(row.member).is_some(),
            "case {}: {} must be present when AdditionalFeatures asks for {}, got {:?}",
            row.case,
            row.member,
            row.feature,
            pick(&with)
        );
    }

    // GetPlace takes its features as repeated query parameters
    // (`@httpQuery(additional-features)`).
    let bare = emulator
        .get_ok("get-place-bare", &format!("/v2/place/{}", needle.place_id))
        .await;
    for (feature, member) in [
        ("TimeZone", "TimeZone"),
        ("Contact", "Contacts"),
        ("Access", "AccessPoints"),
    ] {
        let case = format!("get-place-{feature}");
        assert!(
            bare.get(member).is_none(),
            "case {case}: {member} must be absent when no feature asks for it"
        );
        let asked = emulator
            .get_ok(
                &case,
                &format!(
                    "/v2/place/{}?additional-features={feature}",
                    needle.place_id
                ),
            )
            .await;
        assert!(
            asked.get(member).is_some(),
            "case {case}: {member} must be present when {feature} is requested, got {asked:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// PricingBucket
// ---------------------------------------------------------------------------

/// `PricingBucket` rides in the response HEADER on every success, never in the JSON
/// body, and a request that declares `IntendedUse: Storage` is charged in a
/// different bucket than the default `SingleUse` — the model documents storage
/// requests as "charged at a higher rate".
///
/// The rows are every implemented operation.
#[tokio::test]
async fn pricing_bucket_is_a_response_header_that_tracks_intended_use() {
    let emulator = Emulator::new();
    let needle = emulator
        .catalog()
        .into_iter()
        .find(|place| place.name == "Space Needle")
        .expect("the catalog seeds the Space Needle");
    let position = json!([needle.position[0], needle.position[1]]);

    let operations: Vec<(&str, &str, Value)> = vec![
        (
            "Geocode",
            "/v2/geocode",
            json!({"QueryText": needle.title.clone()}),
        ),
        (
            "SearchText",
            "/v2/search-text",
            json!({"QueryText": needle.title.clone()}),
        ),
        (
            "ReverseGeocode",
            "/v2/reverse-geocode",
            json!({"QueryPosition": position.clone(), "QueryRadius": 100}),
        ),
        (
            "SearchNearby",
            "/v2/search-nearby",
            json!({"QueryPosition": position, "QueryRadius": 100}),
        ),
    ];

    let mut buckets: Vec<String> = Vec::new();
    for (operation, path, body) in operations {
        let mut single_use = body.clone();
        single_use["IntendedUse"] = json!("SingleUse");
        let mut storage = body.clone();
        storage["IntendedUse"] = json!("Storage");

        let mut seen: Vec<String> = Vec::new();
        for (case, request) in [
            (format!("{operation}-default"), body),
            (format!("{operation}-single-use"), single_use),
            (format!("{operation}-storage"), storage),
        ] {
            let call = emulator.post(path, &request).await;
            assert_success(&case, &call);
            assert!(
                call.json().get("PricingBucket").is_none(),
                "case {case}: PricingBucket is bound to a header, so it must not appear \
                 in the body: {:?}",
                call.json()
            );
            seen.push(call.header(PRICING_BUCKET_HEADER));
        }
        assert_eq!(
            seen[0], seen[1],
            "case {operation}: an absent IntendedUse defaults to SingleUse, so both must \
             report the same bucket; got {:?} and {:?}",
            seen[0], seen[1]
        );
        assert_ne!(
            seen[1], seen[2],
            "case {operation}: a Storage request is charged at a higher rate, so its \
             bucket must differ from the SingleUse one; both were {:?}",
            seen[1]
        );
        buckets.push(seen[0].clone());
    }

    // GetPlace reports its own bucket too, through the same header.
    let call = emulator
        .get(&format!("/v2/place/{}", needle.place_id))
        .await;
    assert_success("GetPlace-default", &call);
    assert!(
        call.json().get("PricingBucket").is_none(),
        "case GetPlace-default: PricingBucket must not appear in the body"
    );
    buckets.push(call.header(PRICING_BUCKET_HEADER));
    assert!(
        buckets.iter().all(|bucket| !bucket.is_empty()),
        "case every-operation: every success must carry a bucket, got {buckets:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A row of the refusal table: a request outside the contract, and why.
struct RefusalRow {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// Every request outside the contract is refused with HTTP 400
/// `ValidationException` — never a 200 with a hollow body, and never a 501.
///
/// The rows span the constraint KINDS: an unknown opaque id, a missing required
/// query, mutually exclusive members, a list of the wrong length, a value outside a
/// modeled range (at both ends), a string outside a modeled length, a value failing
/// a modeled pattern, and an unparseable pagination token.
#[tokio::test]
async fn requests_outside_the_contract_are_refused_with_validation_exception() {
    let emulator = Emulator::new();
    let rows = vec![
        RefusalRow {
            case: "get-place-unknown-id",
            method: "GET",
            path: "/v2/place/NOTAPLACEIDTHISSERVICEEVERISSUED00000000".to_string(),
            body: None,
        },
        RefusalRow {
            case: "geocode-without-query",
            method: "POST",
            path: "/v2/geocode".to_string(),
            body: Some(json!({"MaxResults": 5})),
        },
        RefusalRow {
            case: "search-text-without-query",
            method: "POST",
            path: "/v2/search-text".to_string(),
            body: Some(json!({"MaxResults": 5})),
        },
        RefusalRow {
            case: "search-text-query-text-and-query-id",
            method: "POST",
            path: "/v2/search-text".to_string(),
            body: Some(json!({"QueryText": "Seattle", "QueryId": "abc"})),
        },
        RefusalRow {
            case: "search-text-garbage-next-token",
            method: "POST",
            path: "/v2/search-text".to_string(),
            body: Some(json!({"QueryText": "Seattle", "NextToken": "not-a-token"})),
        },
        RefusalRow {
            case: "search-nearby-garbage-next-token",
            method: "POST",
            path: "/v2/search-nearby".to_string(),
            body: Some(json!({
                "QueryPosition": [-122.330_062, 47.603_832],
                "NextToken": "not-a-token",
            })),
        },
        RefusalRow {
            // `QueryPosition` is `len 2..2`.
            case: "reverse-geocode-three-element-position",
            method: "POST",
            path: "/v2/reverse-geocode".to_string(),
            body: Some(json!({"QueryPosition": [-122.330_062, 47.603_832, 10.0]})),
        },
        RefusalRow {
            case: "reverse-geocode-one-element-position",
            method: "POST",
            path: "/v2/reverse-geocode".to_string(),
            body: Some(json!({"QueryPosition": [-122.330_062]})),
        },
        RefusalRow {
            // `QueryPosition` is `@required`.
            case: "search-nearby-without-position",
            method: "POST",
            path: "/v2/search-nearby".to_string(),
            body: Some(json!({"QueryRadius": 100})),
        },
        RefusalRow {
            // `QueryRadius` is `range 1..21000000`.
            case: "reverse-geocode-radius-below-range",
            method: "POST",
            path: "/v2/reverse-geocode".to_string(),
            body: Some(json!({
                "QueryPosition": [-122.330_062, 47.603_832],
                "QueryRadius": 0,
            })),
        },
        RefusalRow {
            case: "reverse-geocode-radius-above-range",
            method: "POST",
            path: "/v2/reverse-geocode".to_string(),
            body: Some(json!({
                "QueryPosition": [-122.330_062, 47.603_832],
                "QueryRadius": 21_000_001_i64,
            })),
        },
        RefusalRow {
            // `MaxResults` is `range 1..100`.
            case: "geocode-max-results-below-range",
            method: "POST",
            path: "/v2/geocode".to_string(),
            body: Some(json!({"QueryText": "Seattle", "MaxResults": 0})),
        },
        RefusalRow {
            case: "geocode-max-results-above-range",
            method: "POST",
            path: "/v2/geocode".to_string(),
            body: Some(json!({"QueryText": "Seattle", "MaxResults": 101})),
        },
        RefusalRow {
            // `PoliticalView` is `pattern ^([A-Z]{2}|[A-Z]{3})$`.
            case: "geocode-political-view-lowercase",
            method: "POST",
            path: "/v2/geocode".to_string(),
            body: Some(json!({"QueryText": "Seattle", "PoliticalView": "usa"})),
        },
        RefusalRow {
            case: "geocode-political-view-too-long",
            method: "POST",
            path: "/v2/geocode".to_string(),
            body: Some(json!({"QueryText": "Seattle", "PoliticalView": "USAX"})),
        },
        RefusalRow {
            // `Language` is `len 2..35`.
            case: "geocode-language-too-short",
            method: "POST",
            path: "/v2/geocode".to_string(),
            body: Some(json!({"QueryText": "Seattle", "Language": "e"})),
        },
        RefusalRow {
            // Well formed against the modeled pattern, but no country this service
            // holds place data for: refused rather than accepted and ignored.
            case: "geocode-political-view-unresolvable",
            method: "POST",
            path: "/v2/geocode".to_string(),
            body: Some(json!({"QueryText": "Seattle", "PoliticalView": "JPN"})),
        },
        RefusalRow {
            // `BiasPosition` is `len 2..2`.
            case: "geocode-three-element-bias-position",
            method: "POST",
            path: "/v2/geocode".to_string(),
            body: Some(json!({
                "QueryText": "Seattle",
                "BiasPosition": [-122.330_062, 47.603_832, 10.0],
            })),
        },
        RefusalRow {
            case: "search-text-one-element-bias-position",
            method: "POST",
            path: "/v2/search-text".to_string(),
            body: Some(json!({"QueryText": "Seattle", "BiasPosition": [-122.330_062]})),
        },
        RefusalRow {
            // "Exactly one of BiasPosition, Filter.BoundingBox, or Filter.Circle"
            // (`SearchTextRequest$BiasPosition`).
            case: "search-text-two-areas",
            method: "POST",
            path: "/v2/search-text".to_string(),
            body: Some(json!({
                "QueryText": "Seattle",
                "BiasPosition": [-122.330_062, 47.603_832],
                "Filter": {"BoundingBox": [-123.0, 47.0, -122.0, 48.0]},
            })),
        },
    ];

    for row in rows {
        let call = emulator.send(row.method, &row.path, row.body.as_ref()).await;
        assert_eq!(
            call.status,
            400,
            "case {}: expected HTTP 400, got {} with {}",
            row.case,
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationException",
            "case {}: expected the ValidationException error code, got {:?}",
            row.case,
            call.error_code()
        );
    }
}

/// A political view the caller asked for comes back in the alpha-3 form the result
/// items model (`pattern ^[A-Z]{3}$`), whichever accepted form was asked with.
#[tokio::test]
async fn a_requested_political_view_is_reported_in_alpha_three_form() {
    let emulator = Emulator::new();
    for (case, requested, expected) in [
        ("alpha-2-us", "US", "USA"),
        ("alpha-3-us", "USA", "USA"),
        ("alpha-2-fr", "FR", "FRA"),
        ("alpha-3-fr", "FRA", "FRA"),
    ] {
        let body = emulator
            .post_ok(
                case,
                "/v2/geocode",
                &json!({"QueryText": "Space Needle", "PoliticalView": requested}),
            )
            .await;
        let returned = items(&body);
        assert!(
            !returned.is_empty(),
            "case {case}: the query must match a place"
        );
        for item in returned {
            assert_eq!(
                text(&item, "PoliticalView"),
                expected,
                "case {case}: expected the alpha-3 political view"
            );
        }
    }

    // With no political view asked for, none is reported.
    let body = emulator
        .post_ok(
            "absent",
            "/v2/geocode",
            &json!({"QueryText": "Space Needle"}),
        )
        .await;
    for item in items(&body) {
        assert!(
            item.get("PoliticalView").is_none(),
            "case absent: no political view was requested, so none must be reported: \
             {item:?}"
        );
    }
}

/// Autocomplete and Suggest are not implemented yet, and say so with HTTP 501
/// rather than an empty success.
#[tokio::test]
async fn the_operations_this_slice_leaves_out_still_answer_501() {
    let emulator = Emulator::new();
    for (case, path) in [
        ("autocomplete", "/v2/autocomplete"),
        ("suggest", "/v2/suggest"),
    ] {
        let call = emulator
            .post(path, &json!({"QueryText": "Space Needle"}))
            .await;
        assert_eq!(
            call.status,
            501,
            "case {case}: an unimplemented operation must answer 501, got {} with {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
    }
}
