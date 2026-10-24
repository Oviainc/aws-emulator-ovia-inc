Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# Recommended-action inventory is unreachable over the wire, so ListRecommendedActions can only ever answer an empty list

_Filed: 2026-10-24T02:29:20Z_
_Operation: ListRecommendedActions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRecommendedActions` pages over `ServiceState::recommended_actions`, but NOTHING in the emulator ever puts an entry into that store: no operation writes it and `src/state.rs` has no seeded catalog. Over HTTP the operation therefore answers `{"recommendedActions": []}` forever, no matter how long the account has been enrolled.

The consequence is not confined to this one operation — the whole automation workflow the service exists for is unreachable from a client:
* `ListRecommendedActionSummaries` always reports one summary with `recommendedActionCount: 0` and zero savings.
* `ListAutomationRulePreview` / `ListAutomationRulePreviewSummaries` always answer an empty page, so a rule's criteria can never be previewed against anything.
* `StartAutomationEvent` resolves its required `recommendedActionId` against the same empty store, so EVERY call answers `ResourceNotFoundException` (404) and no automation event can ever be created.
* Because no event can be created, `GetAutomationEvent` always 404s and `ListAutomationEvents` always answers an empty page.

Expected: against the real service, an enrolled account with EBS volumes has recommended actions, and a client can list them, preview a rule against them, start an automation event for one, and read that event back. In the emulator this needs a deterministic, service-generated inventory recorded in state at store construction (the established pattern elsewhere in this repo: `braket` seeds its device catalog via `seeded_devices()`, `artifact` via `seed_catalog()`, `arczonalshift` via `seed_managed_resources()`, `workspaces` via `seed_amazon_bundle_catalog()`), so that the read paths and `StartAutomationEvent` have something real to compute from. Actual: the inventory exists only as a field that tests populate by reaching into the store in-process.

## Evidence

Spec/model: `ListRecommendedActions` is @paginated(items=recommendedActions) and `StartAutomationEvent` takes a required `recommendedActionId` and declares `ResourceNotFoundException` — i.e. the model assumes an inventory of recommended actions exists for an enrolled account (OPERATIONS.md, "ListRecommendedActions" / "StartAutomationEvent").

Handler/state: `src/state.rs:1495` declares `pub recommended_actions: RecommendedActionStore` and its own doc says "nothing in this API populates it"; `RecommendedActionStore::insert` (src/state.rs:587) has no caller in `src/`. `src/ops/list_recommended_actions.rs:240-247` pages over that store; `src/ops/start_automation_event.rs:251-257` 404s when the id is absent from it; `src/ops/list_automation_rule_preview.rs:292` and `list_automation_rule_preview_summaries.rs:307` preview over it.

Test coverage: the only writers are the test fixtures, which bypass HTTP — `tests/all/recommended_actions.rs:33-43` (`record(...)` takes `service_state(&**engine)` and calls `guard.recommended_actions.insert(...)`), plus `tests/all/automation_rule_preview.rs:221` and `automation_rule_preview_summaries.rs:184`. Its own module doc (tests/all/recommended_actions.rs:9-12) states "no operation of THIS API creates one, so the fixtures below record actions into the store the way that analysis would". No test drives the chain from a cold HTTP-only start, so nothing asserts that a client can ever see an action.

## Steps to Reproduce

1. Start the emulator and point an SDK/CLI client for `aco-automation` (awsJson1_0, POST / with `X-Amz-Target: ComputeOptimizerAutomationService.<Op>`) at it.
2. Call `UpdateEnrollmentConfiguration` with `status=Active`, then `GetEnrollmentConfiguration` — the account is enrolled.
3. Call `ListRecommendedActions` with no filters — expect at least one recommended action for the enrolled account; actual: `{"recommendedActions": []}` with no `nextToken`.
4. Call `ListRecommendedActionSummaries` — actual: a single summary whose `total.recommendedActionCount` is 0 and whose savings are 0.
5. Call `ListAutomationRulePreview` with `ruleType=AccountRule` and `recommendedActionTypes=["UpgradeEbsVolumeType"]` — actual: `{"previewResults": []}`.
6. Take any `recommendedActionId` matching `^[0-9A-Za-z]{16}$` and call `StartAutomationEvent` — actual: HTTP 404 `ResourceNotFoundException` for every possible id, so `GetAutomationEvent` and `ListAutomationEvents` can never return an event either.
