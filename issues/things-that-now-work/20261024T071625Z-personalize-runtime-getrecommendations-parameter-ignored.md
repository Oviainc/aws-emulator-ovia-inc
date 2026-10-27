Issue-Severity: HIGH
Issue-Gap-Class: parameter-ignored
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# filterValues in the documented escaped-double-quote form is never matched, so INCLUDE filters return nothing and EXCLUDE filters remove nothing

_Filed: 2026-10-24T07:16:25Z_
_Operation: GetRecommendations | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Amazon Personalize requires string filter values supplied through `filterValues` (and a promotion's own `filterValues`) to be enclosed in escaped double quotes, e.g. `"filterValues": {"GENRE": "\"Comedy\""}` for the expression `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`, with several values separated by commas (`"\"Comedy\",\"Drama\""`). The emulator binds a `$placeholder` by splitting the raw value on commas and trimming whitespace ONLY — it never strips the surrounding double quotes — and then compares the resulting token to the stored Items-dataset attribute with string equality. A value in the documented form therefore binds to the token `"Comedy"` (quote characters included), which can never equal the stored attribute `Comedy`.

Observable consequences for a client that follows the documented request format:
- An `INCLUDE` expression matches no catalog entry at all, so GetRecommendations answers HTTP 200 with an EMPTY `itemList` (and a fresh `recommendationId`) instead of the matching items.
- An `EXCLUDE` expression matches no catalog entry, so `retains()` keeps everything and the filter silently removes NOTHING — the response is the unfiltered ranking. A filter whose whole purpose is to suppress items (already-purchased, unavailable, age-restricted) is thus quietly a no-op while the call still reports success.
- The same mis-binding applies to `promotions[0].filterValues`, so a promotion whose filter uses a placeholder promotes no item (or, for an EXCLUDE promotion filter, promotes every item).

Expected: a `filterValues` entry whose value carries escaped double quotes is parsed as a comma-separated list of quoted string literals, each matched against the stored attribute without the quotes (numeric/timestamp values stay unquoted), so the filter selects exactly the entries its expression names. Actual: the quotes are treated as part of the value, and the filter either selects nothing (INCLUDE) or filters nothing (EXCLUDE).

## Evidence

Spec/well-established behavior: `GetRecommendationsRequest$filterValues` (Smithy model, map<FilterAttributeName,FilterAttributeValue>, len 0..25) documents "provide the parameter name (in matching case) as a key and the filter value(s) as the corresponding value. Separate multiple values for one parameter with a comma", and the AWS Personalize guide the member points at ("Filtering recommendations and user segments") requires each STRING filter value to be enclosed in escaped double quotes in the request (`"GENRES": "\"Comedy\",\"Drama\""`); unquoted string values are not the documented form. The same wording appears on `Promotion$filterValues`.
Handler/shared code: `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state/filter.rs` lines 598-616 (`resolve()`: `raw.split(',').map(str::trim)` with no quote stripping) and lines 656-677 (`evaluate()`: `values.iter().any(|value| value == held)` against the stored attribute split on '|'), reached from `src/ops/get_recommendations.rs` lines 242-249 (request filter) and lines 446-455 (promotion filter) via `state.rs` `bind_filter`.
Test coverage: none for the documented form. `tests/all/inference.rs` lines 576-613 (`a_filter_expression_removes_exactly_the_items_it_names`) and lines 682-685, 751-755 only ever send UNQUOTED values (`{"GENRE": "Comedy"}`, `{"GENRE": "Comedy,Drama"}`), so the quoted form AWS documents is untested and currently broken.

## Steps to Reproduce

1. Register (via the state seam `register_filter`, or the control plane once it is wired) a filter `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)` and a USER_PERSONALIZATION campaign whose Items catalog has entries with GENRE=Comedy.
2. POST /recommendations with {"campaignArn": <campaign>, "userId": "user-1", "numResults": 10, "filterArn": <filter>, "filterValues": {"GENRE": "\"Comedy\""}} — i.e. the value in the documented escaped-quote form.
3. Observed: HTTP 200 with an empty `itemList`. Expected: the Comedy items from the catalog, descending by score.
4. Repeat with a registered filter `EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)` and the same `filterValues`. Observed: the full unfiltered ranking (the excluded genre is still present). Expected: every Comedy item removed.
5. Repeat with the quoted value inside `promotions[0].filterValues` for a placeholder promotion filter: observed no item carries `promotionName`/reason "Promoted item" for an INCLUDE promotion filter.
