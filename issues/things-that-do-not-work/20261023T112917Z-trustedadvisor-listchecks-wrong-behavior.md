Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 269503db875a05d28919ba6675d6ea40b332967b

# Freshly booted emulator answers 403 AccessDeniedException to every Trusted Advisor call (seeded support plan is Basic, with no wire-reachable way to become entitled)

_Filed: 2026-10-23T11:29:17Z_
_Operation: ListChecks | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `269503db875a05d28919ba6675d6ea40b332967b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a client pointed at the emulator can call the Trusted Advisor API and get data. Every real-world caller of this API necessarily holds a Business / Enterprise On-Ramp / Enterprise Support plan (that is the entitlement AWS requires to use the API at all), so every real request/response recording of ListChecks, ListRecommendations, GetRecommendation, ListRecommendationResources, UpdateRecommendationLifecycle and BatchUpdateRecommendationResourceExclusion is a 2xx with a body computed from the account's checks and recommendations.

Actual: the service's seeded state sets `support_plan: SupportPlan::Basic` (src/state.rs, `impl Default`), and every handler applies `crate::state::support_level_refusal(..)` as its first step. So on a freshly booted emulator EVERY operation of this service — all six implemented ones, and the six scaffolds too — answers HTTP 403 `AccessDeniedException` with message "Access denied due to support level", and no 200 is reachable. The seeded check catalog, the recommendations, the aggregates and the lifecycle/exclusion writes are all unobservable over HTTP.

There is no way for a client to change this: the Trusted Advisor model declares no operation that reads, buys or changes a support plan, and this crate exposes no header, query parameter, environment variable or admin endpoint for it — the only mutator is the in-process Rust accessor `ComAmazonawsTrustedadvisorTrustedadvisorState::set_support_plan`, which only the crate's own tests can call (`tests/all/recommendation_loop.rs::set_support_plan`). The gate's logic itself is correct and worth keeping (a Basic/Developer account must be refused); what diverges is the account the emulator presents by default. Like every other emulated AWS service in this repo, the single emulated account should behave as an entitled account (plan Business or better) unless something explicitly moves it to an unentitled plan, so that the gate stays observable via `set_support_plan` while black-box clients and trace replays see the 200s real AWS returns.

Severity HIGH: it is not an edge case — it makes the whole implemented surface of this service unusable and makes every recorded AWS trace for it mismatch on status code, error type header and body.

## Evidence

Spec/semantics: AWS requires a Business, Enterprise On-Ramp or Enterprise Support plan to use the Trusted Advisor API; traffic recorded from a real account therefore always comes from an entitled account and answers 200 (`ListChecks` -> `checkSummaries`, `GetRecommendation` -> `recommendation`, etc. per the Smithy model's 200 outputs). The model declares NO operation that reads or changes a support subscription (12 operations, none plan-related), so entitlement can never be established on the wire.

Handler/state: services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs `impl Default for ComAmazonawsTrustedadvisorTrustedadvisorState` sets `support_plan: SupportPlan::Basic` (~line 695); `SupportPlan::includes_trusted_advisor_api` returns false for Basic/Developer (~line 91); `support_level_refusal` (~line 784) therefore returns `Some(AccessDeniedException{message: ACCESS_DENIED_SUPPORT_LEVEL})` for the seeded store. Every implemented handler calls it first — e.g. src/ops/list_checks.rs lines 189-191, src/ops/get_recommendation.rs lines 201-203, src/ops/list_recommendations.rs lines 215-217, src/ops/list_recommendation_resources.rs lines 191-195, src/ops/update_recommendation_lifecycle.rs lines 200-204, src/ops/batch_update_recommendation_resource_exclusion.rs lines 184-191.

Test coverage: no test exercises the DEFAULT store — every test first calls `set_support_plan(..)` (tests/all/recommendation_loop.rs lines 61-85 uses `SupportPlan::Business`; tests/all/support_level_gate.rs always passes an explicit plan via `emulator_on_plan`). `grep -rn 'set_support_plan|SupportPlan::' services/service-com-amazonaws-trustedadvisor-trustedadvisor` shows the only non-test writer is the state accessor itself, i.e. nothing on the wire can reach it. The existing (fixed) issue 20261022T030737Z asserted the gate must EXIST for unentitled plans; it does not cover which plan a fresh store starts on, so this is net-new.

## Steps to Reproduce

1. Boot the emulator with the Trusted Advisor service mounted, exactly as a user would (no in-process Rust setup).
2. GET /v1/checks with a SigV4 Authorization header whose credential scope names `trustedadvisor`.
3. Observe HTTP 403 with `x-amzn-errortype: AccessDeniedException` and body {"message":"Access denied due to support level"}, instead of the 200 + `checkSummaries` real AWS returns for an API-entitled account.
4. Repeat for GET /v1/recommendations, GET /v1/recommendations/{arn}, GET /v1/recommendations/{arn}/resources, PUT /v1/recommendations/{arn}/lifecycle and PUT /v1/batch-update-recommendation-resource-exclusion: all six implemented operations answer the same 403.
5. Search the crate for any wire-reachable way to become entitled (header, query parameter, env var, admin route): none exists — only the in-process `set_support_plan` accessor used by tests, so the emulator can never return a 200 to an external client.
