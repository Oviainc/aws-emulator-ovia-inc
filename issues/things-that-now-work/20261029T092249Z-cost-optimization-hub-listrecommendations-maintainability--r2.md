Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: feb204971e0c529b59922c31c571e9b68abdb6f7

# Consolidate the triplicated nextToken decode-or-refuse rule into one helper

_Filed: 2026-10-29T09:22:49Z_
_Operation: ListRecommendations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `feb204971e0c529b59922c31c571e9b68abdb6f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "an absent `nextToken` starts at offset 0; a token this service did not issue for THIS list is refused with the structured `ValidationException` attributed to `nextToken`" is implemented three times, once per paginated handler, instead of once beside `decode_page_token` in `state.rs` (or beside the other handler-enforced constraint helpers in `service.rs`).

The three copies are byte-for-byte identical except for the operation name embedded in the two message strings, so a single parameterised helper (`kind`, operation name) would produce the exact same 400 bodies. The copies have ALREADY drifted structurally: `list_recommendations.rs` and `list_enrollment_statuses.rs` each inline `crate::service::stated_validation_exception` in their own `invalid_next_token()`, while `list_recommendation_summaries.rs` routes the same refusal through a local `stated_refusal(statement, field_name, field_message)` wrapper. That is three places to keep in step when the token rule changes (e.g. accepting a foreign token, or re-wording the field entry), and the crate's own stated principle is the opposite: `service.rs:317-319` calls `stated_validation_exception` "The ONE place this service's `ValidationException` is built", and the sibling handler-enforced constraints (`out_of_range`, `not_in_enum`) are already centralised in `service.rs`.

The fix is a pure consolidation: every response byte and every existing test stays unchanged.

## Evidence

Copy 1 — services/.../src/ops/list_recommendations.rs:187-193:
```
        let offset = match &input.next_token {
            None => 0,
            Some(token) => match crate::state::decode_page_token(PAGE_KIND, token) {
                Some(offset) => offset,
                None => return Err(invalid_next_token()),
            },
        };
```
with its refusal at list_recommendations.rs:338-351:
```
fn invalid_next_token() -> ListRecommendationsError {
    ListRecommendationsErrorModeled::ValidationException(
        crate::service::stated_validation_exception(
            "Invalid nextToken specified. The token was not issued by ListRecommendations."
                .to_string(),
            vec![ValidationExceptionDetail {
                field_name: "nextToken".to_string(),
                message: "the value is not a pagination token issued by ListRecommendations"
                    .to_string(),
```

Copy 2 — services/.../src/ops/list_enrollment_statuses.rs:173-179 is the same `match` block, and its refusal at list_enrollment_statuses.rs:240-253:
```
fn invalid_next_token() -> ListEnrollmentStatusesError {
    ListEnrollmentStatusesErrorModeled::ValidationException(
        crate::service::stated_validation_exception(
            "Invalid nextToken specified. The token was not issued by ListEnrollmentStatuses."
```

Copy 3 — services/.../src/ops/list_recommendation_summaries.rs:258-264 is the same `match` block, but its refusal goes through a DIFFERENT local wrapper (list_recommendation_summaries.rs:465-472):
```
fn invalid_next_token() -> ListRecommendationSummariesError {
    stated_refusal(
        "Invalid nextToken specified. The token was not issued by ListRecommendationSummaries."
            .to_string(),
        "nextToken",
        "the value is not a pagination token issued by ListRecommendationSummaries".to_string(),
    )
}
```

The single existing shared half of the rule is services/.../src/state.rs:880 `pub fn decode_page_token(kind: &str, token: &str) -> Option<usize>`; the crate's stated single-place principle is services/.../src/service.rs:317-319: "The ONE place this service's `ValidationException` is built, so every refusal of this service — whichever layer raised it — fills `reason` and `fields` the same way".

Test coverage that pins all three responses (so a consolidation is verifiable as behaviour-preserving):
- tests/all/validation_exception_fields.rs:414-424 — "a nextToken this service never issued" (ListEnrollmentStatuses) and "a nextToken from another list" (ListRecommendations), both asserting field `nextToken`.
- tests/all/summary_metrics_and_refusals.rs:473-493 — the same two cases plus ListRecommendationSummaries.
- tests/all/cost_optimization_hub.rs:537-547 and :934-944 — the ListEnrollmentStatuses and ListRecommendations 400s.
- tests/all/recommendation_summaries.rs:520-538 — a ListRecommendations token rejected by ListRecommendationSummaries.

## Steps to Reproduce

1. Open services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_recommendations.rs and read lines 187-193 and 338-351.
2. Open src/ops/list_enrollment_statuses.rs and read lines 173-179 and 240-253 — the same branch and the same refusal envelope, with only the operation name differing.
3. Open src/ops/list_recommendation_summaries.rs and read lines 258-264 and 465-472 — the same branch again, but the refusal is built through a local `stated_refusal` wrapper instead of calling `crate::service::stated_validation_exception` directly, showing the copies have already drifted.
4. Note src/state.rs:880 already shares the decode half (`decode_page_token(kind, token)`) and src/service.rs:357/:391 already centralise the analogous handler-enforced `maxResults`/enum refusals, so the refusal half is the only part left duplicated per operation.
5. Run the existing tests listed in the evidence to confirm all three responses are pinned, so lifting the branch and the refusal into one helper parameterised by list kind and operation name changes no response.
