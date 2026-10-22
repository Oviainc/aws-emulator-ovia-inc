Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 0761ec9a912f1ad457725ac2bfa14369ca92ad04

# Bootstrap the Amazon Location Service Places (geo-places) emulator

_Filed: 2026-10-21T23:12:55Z_
_Operation: service-com-amazonaws-geoplaces-placesservice | Class: missing_

> _Note: this issue was identified against the code base at commit `0761ec9a912f1ad457725ac2bfa14369ca92ad04`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 7 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` (marker `//! SCAFFOLDED, NOT IMPLEMENTED:` present in all 7 files), so no geocoding or place-lookup client works at all. Implement a FIRST, small slice: a seeded place catalog that all reads resolve against.

This service has no customer-created resources (all 7 operations are `@readonly` searches over AWS's own place data), so the happy path is `search -> get details -> reverse lookup`, backed by a catalog that the emulator seeds at startup. The critical property is CONSISTENCY: a PlaceId returned by Geocode/SearchText MUST resolve through GetPlace to the same Title/Position/Address, and a Position returned by Geocode MUST reverse-geocode back to that same place. That is only achievable if every response is computed from one persisted catalog — no canned bodies, no echoing the query back as the result, no random PlaceIds that nothing can later look up.

Happy path to implement (5 operations, in order):

1. **Geocode** — `POST /v2/geocode`. Input `QueryText` (free-form address) or `QueryComponents`, plus `MaxResults`, `BiasPosition` ([lon,lat]), `Filter`, `Language`, `IntendedUse`. Output: required header `PricingBucket` (`x-amz-geo-pricing-bucket`) and `ResultItems` (`list<GeocodeResultItem>`, 0..100) where each item carries `PlaceId`, `PlaceType`, `Title`, `Address`, `Position`, `MapView`, and a `Distance`/relevance ordering. Must match catalog records by normalized text (and by component when `QueryComponents` is given), honor `MaxResults` as a cap, and order results nearest-first when `BiasPosition` is supplied. Returning an empty `ResultItems` for a non-matching query is correct behavior; fabricating a result is not.
2. **GetPlace** — `GET /v2/place/{PlaceId}`. Input `PlaceId` (@httpLabel, required) plus query params `AdditionalFeatures` (additional-features), `Language`, `PoliticalView`, `IntendedUse`. Output required `PlaceId`, `PlaceType`, `Title`, `PricingBucket`, plus `Address`, `Position`, `MapView`, `Categories`, `TimeZone`, `Contacts`, `OpeningHours`, `AccessPoints`. MUST look the id up in the catalog and return the SAME record Geocode/SearchText/SearchNearby handed out; an unknown `PlaceId` must be a `ValidationException` (HTTP 400), never a hollow 200.
3. **ReverseGeocode** — `POST /v2/reverse-geocode`. Input `QueryPosition` ([lon,lat], required, len 2), `QueryRadius` (default 0), `MaxResults`, `Filter`, `Heading`. Output `PricingBucket` + `ResultItems` (`list<ReverseGeocodeResultItem>` with `PlaceId`, `PlaceType`, `Title`, `Address`, `Position`, `Distance`). Must select catalog records whose `Position` is within `QueryRadius` metres of `QueryPosition` (and the nearest record when the radius is 0/absent), sorted ascending by computed `Distance`. Feeding back a `Position` from step 1 must yield that place's `PlaceId`.
4. **SearchText** — `POST /v2/search-text`. Input `QueryText` or `QueryId`, `MaxResults`, `BiasPosition`, `Filter`, `NextToken` (1..2000). Output `PricingBucket`, `ResultItems` (`list<SearchTextResultItem>`), `NextToken`. Must match POI/business-name text in the catalog, cap each page at `MaxResults`, and emit a real opaque `NextToken` when more matches remain — a token that, passed back, returns the NEXT page and eventually a response with no `NextToken`.
5. **SearchNearby** — `POST /v2/search-nearby`. Input `QueryPosition` (required), `QueryRadius`, `MaxResults`, `Filter` (e.g. `IncludeCategories`), `NextToken`. Output `PricingBucket`, `ResultItems` (`list<SearchNearbyResultItem>` with `Distance`), `NextToken`. Must return catalog places inside the radius ordered by distance, apply the category filter, and paginate with the same token scheme as SearchText.

Cross-cutting requirements for the slice:
- `PricingBucket` is `@required` on all five outputs and is bound to the `x-amz-geo-pricing-bucket` RESPONSE HEADER — it must be set on every success, not placed in the JSON body.
- Spec-required input validation must produce `ValidationException` (HTTP 400): missing `QueryText` on Autocomplete/Suggest, missing `QueryPosition`/wrong list length (must be exactly 2) on ReverseGeocode/SearchNearby, `PoliticalView` not matching `^([A-Z]{2}|[A-Z]{3})$`, `Language` outside 2..35, `QueryRadius` outside 0..4294967295, `MaxResults` out of range, unparseable/foreign `NextToken`.
- No handler may mutate the catalog; all five are `@readonly`.

## State Model

One service-state struct reached via `crate::state::service_state(&*self.engine)` (never a fresh engine), seeded once at construction with a handful of well-known places (e.g. a few addresses and POIs in a couple of cities) so results are deterministic and assertable:

- `places: Vec<PlaceRecord>` — the seeded catalog, the single source of truth for all five operations.
- `place_index: HashMap<String /*PlaceId*/, usize>` — O(1) GetPlace lookup into `places`.
- `PlaceRecord { place_id: String, place_type: PlaceType, title: String, address: Address { Label, Country, Region, SubRegion, Locality, District, PostalCode, Street, AddressNumber }, position: [f64; 2] /*[lon,lat]*/, map_view: [f64; 4], categories: Vec<Category>, food_types: Vec<FoodType>, business_chains: Vec<BusinessChain>, contacts: Option<Contacts>, opening_hours: Vec<OpeningHours>, access_points: Vec<AccessPoint>, time_zone: Option<TimeZone>, political_view: Option<String>, search_terms: Vec<String> /*normalized tokens for text matching*/ }`.
- `pagination: HashMap<String /*NextToken*/, PageCursor { query_fingerprint: String, offset: usize, expires_at: Instant }>` — backs SearchText/SearchNearby `NextToken`; a token is only valid for the query that produced it, and the final page omits the token.
- `pricing_bucket: String` — the constant returned in `x-amz-geo-pricing-bucket` (per-operation value, e.g. the geocode/search bucket name).

All remaining operations (Autocomplete, Suggest) stay 501 for now and are tracked as separate gap issues; Suggest/Autocomplete should later reuse `places.search_terms` and the same catalog.

## Evidence

OPERATIONS.md (crate root) lists all 7 operations with routes, inputs, outputs and errors: Geocode `POST /v2/geocode`, GetPlace `GET /v2/place/{PlaceId}`, ReverseGeocode `POST /v2/reverse-geocode`, SearchText `POST /v2/search-text`, SearchNearby `POST /v2/search-nearby` (protocol `restJson1`, sigv4 name `geo-places`). It documents `PricingBucket` as `@required` with `@httpHeader(x-amz-geo-pricing-bucket)` on every output, GetPlace's required `PlaceId`/`PlaceType`/`Title`, the `NextToken` (len 1..2000) in/out on SearchText and SearchNearby, the 2-element `QueryPosition`/`BiasPosition` constraints, the `^([A-Z]{2}|[A-Z]{3})$` PoliticalView pattern, and `ValidationException (HTTP 400)` on all five. grep over `services/service-com-amazonaws-geoplaces-placesservice/src/ops/` shows the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker in all 7 handler files (autocomplete.rs, geocode.rs, get_place.rs, reverse_geocode.rs, search_nearby.rs, search_text.rs, suggest.rs), i.e. every operation currently answers HTTP 501 and the service holds zero state. No test exercises any operation beyond the generated `tests/state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `geo-places` client at it.
2. Call Geocode with QueryText set to a seeded address (e.g. "1600 Pennsylvania Ave NW, Washington DC") — expect HTTP 200, an `x-amz-geo-pricing-bucket` response header, and at least one ResultItem with PlaceId, PlaceType, Title, Address and Position. Today: HTTP 501 NotImplemented.
3. Take the PlaceId from step 2 and call GetPlace on it — expect 200 with the SAME Title/Position/Address as the Geocode result item.
4. Call GetPlace with a bogus PlaceId — expect ValidationException (HTTP 400), not 200 and not 501.
5. Call ReverseGeocode with QueryPosition set to the Position from step 2 and QueryRadius 50 — expect the same PlaceId back, with a Distance near 0.
6. Call SearchText with QueryText matching several seeded POIs and MaxResults=1 — expect exactly 1 ResultItem plus a NextToken; pass that NextToken back and expect the next distinct result, and no NextToken on the final page.
7. Call SearchNearby with the step-2 Position, QueryRadius=5000 and a Filter.IncludeCategories of a seeded category — expect only places of that category inside the radius, ordered ascending by Distance.
8. Call ReverseGeocode with a 3-element QueryPosition and SearchText with a garbage NextToken — expect ValidationException (HTTP 400) in both cases.
