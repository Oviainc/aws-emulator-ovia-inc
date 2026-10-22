Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the AWSBillingAndCostManagementRecommendedActions emulator

_Filed: 2026-10-21T21:19:00Z_
_Operation: service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole service crate is a fresh scaffold: the only handler, `src/ops/list_recommended_actions.rs`, still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented("ListRecommendedActions"))` (HTTP 501), so no client can do anything with this service.

This service models exactly ONE operation, so the bootstrap happy path IS that operation, implemented statefully with its filter and pagination semantics working end to end (the awsJson1_0 wire: `POST /` with `X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions`).

## Happy path to implement

1. **ListRecommendedActions (unfiltered)** — input `filter` (RequestFilter, optional), `maxResults` (integer, 1..1000, optional), `nextToken` (string, len 1..8192, optional). Output `recommendedActions` (list<RecommendedAction>, **required** — always present, empty list when nothing matches) and `nextToken` (present ONLY when more pages remain). With no filter and no maxResults it must return every action held in state for the caller's account, each one a fully-populated `RecommendedAction{ id, type (ActionType), accountId (12 digits), severity (INFO|WARNING|CRITICAL), feature (ACCOUNT|BUDGETS|COST_ANOMALY_DETECTION|COST_OPTIMIZATION_HUB|FREE_TIER|IAM|PAYMENTS|RESERVATIONS|SAVINGS_PLANS|TAX_SETTINGS), context (map), nextSteps (list), lastUpdatedTimeStamp }` read out of the store — not a hardcoded literal built in the handler body.
2. **ListRecommendedActions with `filter.actions[]`** — each `ActionFilter` has required `key` (FilterName: `FEATURE` | `SEVERITY` | `TYPE`), required `matchOption` (`EQUALS` | `NOT_EQUALS`) and required `values` (len >= 1). The handler must actually evaluate the predicate against the stored records: `EQUALS` keeps records whose field is in `values`, `NOT_EQUALS` excludes them, and multiple filters in the list are ANDed. A filter that matches nothing returns `recommendedActions: []` with no `nextToken`, NOT the full list.
3. **ListRecommendedActions with `maxResults` + `nextToken` paging** — return at most `maxResults` items, emit an opaque `nextToken` when and only when more matching records remain, and resume exactly where the previous page stopped when that token is passed back. Order must be stable across pages so no record is duplicated or skipped. An unparseable/foreign token must be refused with `ValidationException` (HTTP 400), never a 500 or a silent restart from the beginning.
4. **Error channel** — `ValidationException` (400) for a bad `nextToken` or a filter value that is not a member of the enum named by `key`; `AccessDeniedException` (403), `ThrottlingException` (429) and `InternalServerException` (500) exist in the model and should be constructible but are not needed on the happy path.

Every response must be computed from persisted state. A canned list returned regardless of `filter`, a `maxResults` that is ignored, or a `nextToken` that is minted but never honoured are all failures of this bootstrap, not partial credit.

## State Model

In `crate::state` behind the existing `service_state(&*self.engine)` lock guard:

- `RecommendedActionsStore { actions: Vec<StoredAction> }` — insertion-ordered so pagination is deterministic.
- `StoredAction { id: String, action_type: ActionType, account_id: String /* ^[0-9]{12}$ */, severity: Severity, feature: Feature, context: BTreeMap<String, String>, next_steps: Vec<String>, last_updated_timestamp: String /* ISO8601 */ }` — one record per recommended action; every field is serialized straight into the wire `RecommendedAction`.
- A seeding seam (e.g. `RecommendedActionsStore::seed(..)` / an internal admin path) so a test can install a known fixture set spanning several `feature`/`severity`/`type` combinations and several accounts — this service has no public create operation, so without a seam there is nothing to list and the operation is untestable.
- `page_tokens` handling: either an opaque encoding of the offset plus a hash of the effective filter, or a `BTreeMap<String, Cursor>` of live cursors; a token issued for one filter/maxResults combination must not silently resume a different query.

## Done when

A client script can list all seeded actions, narrow them with a `FEATURE`/`EQUALS` filter and get only the matching subset, invert it with `NOT_EQUALS` and get the complement, and walk the full set with `maxResults: 1` collecting each page via `nextToken` until `nextToken` is absent — with the union of pages equal to the unpaginated result and no duplicates.

## Evidence

OPERATIONS.md (crate root) lists exactly 1 operation, `ListRecommendedActions`, with `@paginated(inputToken=nextToken, outputToken=nextToken, items=recommendedActions)`, input `filter` (RequestFilter) / `maxResults` (integer, range 1..1000) / `nextToken` (len 1..8192), output `recommendedActions` (list<RecommendedAction>, required) / `nextToken`, and errors AccessDeniedException (403), InternalServerException (500), ThrottlingException (429), ValidationException (400); wire protocol awsJson1_0, target prefix `AWSBillingAndCostManagementRecommendedActions`. The Smithy model (smithy_query on RequestFilter/ActionFilter/RecommendedAction/MatchOption/ActionType/Severity/Feature) gives ActionFilter's required `key`/`matchOption`/`values` members, MatchOption = EQUALS|NOT_EQUALS, FilterName = FEATURE|SEVERITY|TYPE, and RecommendedAction's members id/type/accountId/severity/feature/context/nextSteps/lastUpdatedTimeStamp. The handler `src/ops/list_recommended_actions.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 166 returns `Err(crate::scaffolded::Scaffolded::not_implemented("ListRecommendedActions"))` — the single operation of the service answers 501 and holds no state. No tests exist for this crate.

## Steps to Reproduce

1. Seed the store with several recommended actions across different feature/severity/type values for account 123456789012.
2. POST / with X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions and body {} — expect 200 with every seeded action fully populated (currently 501 NotImplemented).
3. POST the same target with {"filter":{"actions":[{"key":"FEATURE","matchOption":"EQUALS","values":["BUDGETS"]}]}} — expect only the BUDGETS actions.
4. Repeat with matchOption NOT_EQUALS — expect exactly the complement of the previous result.
5. POST with {"maxResults":1} and then repeatedly re-POST with the returned nextToken — expect one item per page, a nextToken only while more remain, and the concatenation of pages to equal the unfiltered list with no duplicates or omissions.
6. POST with {"nextToken":"not-a-real-token"} — expect ValidationException (HTTP 400), not 500 and not a silent full listing.
