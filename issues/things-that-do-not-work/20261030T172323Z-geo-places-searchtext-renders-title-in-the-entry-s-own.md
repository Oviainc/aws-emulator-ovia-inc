Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: geo-places.search-text-finds-known-landmark
Issue-Shape-Id: com.amazonaws.geoplaces#PlacesService
Issue-Key: geo-places.search-text-finds-known-landmark|run#6|AssertionError|assert matches, f"no result titled 'Eiffel Tower' among {[i['Title'] for i in items]}"
Issue-Commit: 64e9c42c82622051de00b3d4417c73d8b9c2c0a0

# SearchText renders Title in the entry's own language, so a query for a landmark's English name answers with the local-language title

> _Note: this issue was identified against the code base at commit `64e9c42c82622051de00b3d4417c73d8b9c2c0a0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchText

## Description
One root cause, in the geo-places result-item title projection.

Cause 1 of 1: a SearchText request that supplies no `Language` gets `ResultItems[*].Title` rendered in the catalog entry's OWN language, even when the query matched the place by its English (localized) name. A query for "Eiffel Tower" matches the Paris landmark record and returns it at the right coordinates, but titled "Tour Eiffel".
Failing check: geo-places.search-text-finds-known-landmark.

AWS: Smithy `SearchText` (POST /v2/search-text, 200) -> `SearchTextResultItem$Title`, documented only as "The item's title"; `SearchTextRequest$Language` (LanguageTag, @length 2..35) is documented as "A list of BCP 47 compliant language codes for the results to be rendered in. If there is no data for the result in the requested language, data will be returned in the default language for the entry." The model therefore does not state what happens when `Language` is OMITTED - that part is recorded behavior only. The recording shows that a free-form `QueryText` naming a globally unique landmark by its English name, with no `Language` member in the request, comes back with at least one `ResultItem` whose `Title` is the name the query used ("Eiffel Tower", matched case-insensitively) and whose `Position` is the landmark's real coordinates (within 0.01 degree of [2.2945, 48.8584]). The general requirement is that `Title` is the localized display name for the language the result was resolved in - for a request with no `Language`, the name the query matched the place by - not unconditionally the entry's native-language name. The response status is 200 in both the recording and the emulator; nothing about the status or the error surface diverges, only the rendered `Title` text.

Emulator: HTTP 200 with one result whose Title is 'Tour Eiffel'. The replayed assertion is `no result titled 'Eiffel Tower' among ['Tour Eiffel']`. The catalog record is seeded with the native title "Tour Eiffel", alternate name "La Tour Eiffel" and a localized title ('en' -> "Eiffel Tower") (services/service-com-amazonaws-geoplaces-placesservice/src/state.rs, seed_catalog, the France block). The record is INDEXED by all of those names (`PlaceRecord::names`, state.rs:1507-1511, feeding `index_terms`), which is why the English query finds it - but the projection of `Title` resolves the language from the REQUEST only: `PlaceRecord::title_modeled_in` -> `name_in(language)` -> `lookup_language(&self.titles_by_language, language)` (state.rs:1492-1498, 1576-1585), and with `language = None` that lookup misses and falls back to `self.title`, the native name. So the localized title is reachable only by a caller that explicitly passes `Language: "en"`; a caller that passes no `Language` can never see it, whatever name it searched by.

Why: Title is the single display line a client shows for a place and the value a client asserts on or de-duplicates by. A client that searches for a landmark by its English name - the overwhelmingly common case, with no `Language` set - gets a title in a different language than the service returns, so result lists render unexpected text, and any client logic that matches the returned title against the query (highlighting, "did you mean", picking the result that corresponds to what the user typed) fails to match at all. The same projection feeds every operation that returns a result item (Geocode, Autocomplete, SearchNearby, Suggest, GetPlace), so every one of them under-localizes the same way: the Louvre record ('Musee du Louvre' with localized 'en' -> 'Louvre Museum') behaves identically.

Fix: the result-item title projection in the service's state - `PlaceRecord::title_modeled_in` / `name_in` (and the label assembly that reuses `name_in`) must resolve the response language when the request supplies none, from the language in which the query matched the record (the matched entry in `titles_by_language` / `alternate_names`), instead of defaulting straight to the record's native `title`. The seeded record already holds the right text; only the language-selection step is wrong.

Traces (examples, not requirements in themselves): geo-places.search-text-finds-known-landmark.

The one open geo-places issue, 20261029T182749Z-geo-places-result-items-diverge-from-the-recorded.md, does NOT cover this: its three causes are the hex PlaceId mint, the computed symmetric MapView, and the substituted Seattle POI records' values (title CASING, ZIP+4, districts). This defect is the language selection applied to an already-correct localized title and is independent of all three.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 22/23 passed; the single failure is geo-places.search-text-finds-known-landmark, verdict=mismatch, emulator up, bootstrap_signal false.

Request replayed (check source): geo.search_text(QueryText="Eiffel Tower", BiasPosition=[2.2945, 48.8584], MaxResults=5) - no `Language` member.

Expected (recording): ResultItems non-empty, and at least one item whose Title contains "Eiffel Tower" case-insensitively, with that item's Position within 0.01 degree of [2.2945, 48.8584].
Actual (emulator): HTTP 200, ResultItems titles = ['Tour Eiffel'].
AssertionError: no result titled 'Eiffel Tower' among ['Tour Eiffel']

Emulator code the value comes from:
- services/service-com-amazonaws-geoplaces-placesservice/src/state.rs, seed_catalog France block: PlaceRecord::new("Tour Eiffel", PointOfInterest, [2.294481, 48.858370], ...).with_alternate_names(&["La Tour Eiffel"]).with_localized_titles(&[("en", "Eiffel Tower")]) - the English title IS present in the record.
- state.rs:1492-1498 PlaceRecord::name_in: lookup_language(&self.titles_by_language, language).map_or(name, ...) - with language=None this falls back to the native title.
- state.rs:1576-1585 PlaceRecord::title_modeled_in / 1568-1570 title_modeled() (which passes None, None).
- state.rs:1507-1511 PlaceRecord::names: includes titles_by_language values, so the English name is searchable even though it is never rendered.

Status and error surface agree with the recording (200, no error); the divergence is confined to the rendered Title text.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a geo-places client at it.
2. Call SearchText (POST /v2/search-text) with QueryText="Eiffel Tower", BiasPosition=[2.2945, 48.8584], MaxResults=5 and NO Language member.
3. Observe HTTP 200 with one ResultItem whose Position is the real landmark ([2.294481, 48.858370], within 0.01 of [2.2945, 48.8584]) but whose Title is "Tour Eiffel"; no item's Title contains "Eiffel Tower". The recording returns an item titled "Eiffel Tower" for the same request.
4. Confirm the cause is language selection, not missing data: repeat the same call with Language="en" and the Title comes back "Eiffel Tower". The same holds for the Louvre record (QueryText="Louvre Museum" returns Title "Musee du Louvre" without Language, "Louvre Museum" with Language="en").
