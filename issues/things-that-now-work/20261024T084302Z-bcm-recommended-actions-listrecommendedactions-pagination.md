Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# nextToken is bound to maxResults, so resuming a listing with a different or absent maxResults is refused with 400

_Filed: 2026-10-24T08:43:02Z_
_Operation: ListRecommendedActions | Class: pagination_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator's pagination token encodes a fingerprint of the *whole effective query including the page size* (`query_fingerprint(&filters, input.max_results)` — the canonical string is `maxResults={..}|filters={..}`). Any subsequent page request that repeats the token but changes `maxResults`, or omits `maxResults` altogether, therefore fails the fingerprint comparison in `parse_token` and is answered with HTTP 400 `ValidationException` (reason `cannotParse`, message "The nextToken was issued for a different request...").

Expected (real service / established AWS pagination semantics): a `nextToken` identifies the position to resume at; `maxResults` is a per-request page-size hint that a caller may change (or drop) between pages. Passing a valid token with a different `maxResults` continues the listing from where the previous page stopped and simply returns a page of the newly requested size. Only a token that is malformed/expired — or, at most, one replayed against a different *filter* — is rejected.

Actual: client paginating with `maxResults=2` on page 1 and then sending just `{"nextToken": "<token>"}` (the most common manual-pagination idiom) gets a 400 instead of page 2, so the listing can never be completed and records are silently unreachable from the caller's perspective.

Secondary symptom of the same class: when `maxResults` is absent the handler sets `page_size = usize::MAX` and returns the ENTIRE matching set in a single page with no `nextToken`. The model caps `MaxResults` at 1000, i.e. the service never puts more than 1000 records in one page; an unbounded page is therefore not a response the real service can produce once the inventory exceeds that bound.

## Evidence

Spec/model: `smithy_query(query="operation", operation="ListRecommendedActions")` — `@paginated(inputToken=nextToken, outputToken=nextToken, pageSize=maxResults, items=recommendedActions)`; `MaxResults` is `range {min:1, max:1000}` and `maxResults` is documented only as "The maximum number of results to return in the response" (no statement that it is part of the token's identity, and no documented default). Well-established AWS pagination behavior: the opaque token encodes position, page size is a per-call parameter (e.g. S3 continuation-token + max-keys, EC2 next-token + max-results).

Handler: `services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/src/ops/list_recommended_actions.rs` — `query_fingerprint()` interpolates `maxResults` into the canonical query string; `parse_token()` returns `cannot_parse("The nextToken was issued for a different request. Repeat the original filter and maxResults when paginating.", ...)` when the fingerprints differ; `page_size = ... None => usize::MAX` in `handle`.

Test coverage: `tests/all/list_recommended_actions.rs` — `paging_reports_every_matching_record_exactly_once` and `a_token_resumes_the_same_query_it_was_issued_for` always re-send the SAME `maxResults` with the token, and `a_next_token_this_service_did_not_mint_is_refused` only asserts refusal for a token minted under a different FILTER. No test exercises resuming with a changed/omitted `maxResults`, so this behaviour is unasserted and net-new.

## Steps to Reproduce

1. Boot the emulator with this service mounted (the store boots with its computed inventory; seed more records through `RecommendedActionsStore::seed` so more than one page exists).
2. POST `/` with `X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions` and body `{"maxResults": 1}`; note the returned `nextToken`.
3. POST again with body `{"nextToken": "<token from step 2>"}` (maxResults omitted). Observed: HTTP 400 ValidationException (reason `cannotParse`, nextToken "issued for a different filter or maxResults"). Expected: HTTP 200 with the next page of records.
4. POST again with body `{"maxResults": 2, "nextToken": "<token from step 2>"}`. Observed: the same HTTP 400. Expected: HTTP 200 resuming at the token's offset with up to 2 records.
5. Seed more than 1000 matching records and POST `{}` (no maxResults). Observed: all records in a single page with no `nextToken`. Expected: at most the service page cap (1000, the modelled MaxResults maximum) plus a `nextToken`.
