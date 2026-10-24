Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# Seed the idle-recommendation store so GetIdleRecommendations can report a recommendation

_Filed: 2026-10-24T02:29:52Z_
_Operation: GetIdleRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Over the wire `GetIdleRecommendations` can never report a single idle resource recommendation: it always answers HTTP 200 with `idleRecommendations: []` (plus one `errors` entry per requested ARN), for every request, on every freshly booted emulator.

The handler itself is written and correct in structure (it selects by `resourceArns`/`accountIds`, applies `filters`, honours `orderBy`, pages with an opaque token and reports `errors` for unknown identifiers), but the store it reads — `ComAmazonawsComputeoptimizerComputeoptimizerserviceState::idle_recommendations` — is initialised EMPTY in `Default` and no wire-reachable operation of this API ever writes to it. Every other recommendation reader of this service is given a fixture inventory as part of the store's initial value (`observed_volumes()` for EBS, `seed_auto_scaling_group_analyses()` for Auto Scaling groups, the observed Lambda functions, `seed_analysed_resources()` for the findings catalog), so each of them reports real, computed rows; the idle store alone has no seed.

Expected: an enrolled account reading `GetIdleRecommendations` sees the idle resources Compute Optimizer analysed for it, each with `resourceArn`, `resourceType`, `finding`, `findingDescription`, `savingsOpportunity`/`savingsOpportunityAfterDiscounts`, `utilizationMetrics`, `lookBackPeriodInDays` and `lastRefreshTimestamp` — exactly like the seeded EBS/ASG/Lambda readers do — and filtering, ordering and paging visibly act on those rows.
Actual: an empty list, so filters, `orderBy`, `maxResults`/`nextToken` are all unobservable over the wire, and the knock-on readers are wrong too: `GetRDSDatabaseRecommendations` always reports `idle: "False"` (it derives that member from `store.idle_recommendations.get(..)`), and `GetRecommendationSummaries` never has an idle resource to summarise.

## Evidence

Spec: `GetIdleRecommendations` ("Returns idle resource recommendations... for the specified account(s)") and the modeled output `idleRecommendations: list<IdleRecommendation>` — the operation's whole purpose is reporting analysis output the service holds. Handler: `src/ops/get_idle_recommendations.rs` lines 291-312 read `guard.idle_recommendations.iter_in_order()`. State: `src/state.rs` — `IdleRecommendationStore` doc says "NO operation of this API creates one", and `impl Default for ComAmazonawsComputeoptimizerComputeoptimizerserviceState` initialises it with "no idle resource recommendation (only an analysis records one)", while the same `Default` seeds `volumes`, `functions`, `auto_scaling_group_analyses` and `analysed_resources`. Knock-on: `src/ops/get_rds_database_recommendations.rs` lines 478-485 derive `idle` from the empty store. Tests: `tests/all/idle_recommendations.rs` only exercises the handler after `record(&engine, ..)` reaches into `service_state(..).lock().idle_recommendations.insert(..)` directly (lines 163-177) — no test drives the operation on a freshly booted emulator because there is nothing to report.

## Steps to Reproduce

1. Boot the compute-optimizer emulator
2. Call UpdateEnrollmentStatus with status=Active and wait out the short analysis delay
3. Call GetEBSVolumeRecommendations and GetAutoScalingGroupRecommendations — both report the seeded fixture inventory
4. Call GetIdleRecommendations with no parameters — observe HTTP 200 with idleRecommendations: [] on every call, whatever filters/orderBy/maxResults are supplied
5. Call GetRDSDatabaseRecommendations — observe every database reports idle: "False", because the idle store it consults is empty
