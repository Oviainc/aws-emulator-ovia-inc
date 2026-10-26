Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9b93cbc54d897386c30bfd66ea5bd8f85f03a356

# UpdateApplicationComponentConfig rewrites the analysis inputs without materialising an assessment completion the clock already owes, so the analysis a later read produces is not the one that was due

_Filed: 2026-10-26T01:05:34Z_
_Operation: UpdateApplicationComponentConfig | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9b93cbc54d897386c30bfd66ea5bd8f85f03a356`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every read handler of this crate materialises an assessment completion the clock already owes before it answers (`portfolio.settle_all(now)`), and the completion is what records the analysis on every component (`RegionState::analyze_portfolio`). `analyze_portfolio` computes each component's recommendation from the component record's CURRENT `inclusion_status` and `app_type` (`state/analysis.rs:363`, `:366`).

`UpdateApplicationComponentConfig` writes exactly those two members, and it is the one handler touching analysis inputs that neither settles nor refuses: it takes the lock and mutates the record directly (`src/ops/update_application_component_config.rs:294-330`). So an update that lands AFTER an assessment's completion instant has fallen due but BEFORE any read materialises it silently changes the analysis that completion will produce:

1. StartAssessment -> completion due at T.
2. At T+epsilon (nothing has read yet, so the completion is still owed) UpdateApplicationComponentConfig sets `inclusionStatus=excludeFromAssessment` (or changes `appType`).
3. The next read calls `settle_all`, the assessment completes with `completionTime = T`, and `component_recommendation` now returns `None` for that component (or a recommendation computed from the new `appType`).

The component therefore ends up with NO `recommendationSet` and no `lastAnalyzedTimestamp` even though the assessment it was covered by reports `COMPLETE` with a completion time that precedes the update — and a caller that had merely issued one read before the update would have got the recommendation. That contradicts the rule the crate states for this very write-on-read: the analysis is recorded at `at`, the instant the assessment completed, "so the same analysis is reported however late it is first read".

The crate already protects the OTHER half of the analysis inputs from exactly this: `PutPortfolioPreferences` runs `ongoing_task(now)` — which calls `settle_all` first — and refuses the save while any task is still owed its terminal status, precisely because "These preferences are the inputs the service's recommendations are computed from, so they cannot be replaced underneath work that consumes them". The component record's `inclusionStatus`/`appType` are inputs of the same computation, and this handler replaces them underneath the pending analysis with neither the settle nor the guard.

## Evidence

Rule the crate states for the clock-owed analysis — src/state.rs:1708-1711 (`analyze_portfolio` doc):
  "([`analysis::server_recommendation`], [`analysis::component_recommendation`]) and
   the instant it was analysed — `at`, the instant the assessment completed, so the
   same analysis is reported however late it is first read."
and src/state/analysis.rs:34-37:
  "Nothing here looks at a request: the inputs are the stored [`super::PortfolioPreferences`]
   and the stored [`super::ServerRecord`] / [`super::ApplicationComponentRecord`], which
   is why a second assessment over an unchanged portfolio and unchanged preferences
   recomputes the same recommendation."

The inputs this handler rewrites are read by the analysis — src/state/analysis.rs:363-367:
  "    if component.inclusion_status.as_deref() == Some(EXCLUDE_FROM_ASSESSMENT) {
           return None;
       }
       let app_type = component.app_type.as_deref();"

The sibling write that DOES settle/guard — src/ops/put_portfolio_preferences.rs:289-298:
  "// These preferences are the inputs the service's recommendations are computed
   // from, so they cannot be replaced underneath work that consumes them: while any
   // task of this Region's portfolio is still running the save is refused with the
   // modelled conflict ...
   let ongoing = store
       .region_mut(&region)
       .and_then(|portfolio| portfolio.ongoing_task(now));"
and src/state.rs:1804-1805:
  "    pub fn ongoing_task(&mut self, now: SystemTime) -> Option<OngoingTask> {
           self.settle_all(now);"

The handler that skips both — src/ops/update_application_component_config.rs:294-321:
  "        let region = crate::state::request_region(ctx.parts());
           let mut store = state.lock();
           ...
           if let Some(app_type) = input.app_type {
               component.app_type = Some(app_type);
           }
           if let Some(inclusion_status) = input.inclusion_status {
               component.inclusion_status = Some(inclusion_status);
           }"
(no `settle_all`, no `ongoing_task` — grep of this file finds neither.)

Contrast with every read of the same record — src/ops/get_application_component_details.rs:173-183:
  "// A read: the only write it makes is materialising what the clock already owed
   // this portfolio — an assessment whose completion was due at this same instant
   // even if nobody had called, and the analysis that completion stands for.
   ...
   if let Some(portfolio) = store.region_mut(&region) { portfolio.settle_all(now); }"

Test coverage: tests/all/application_component_config.rs exercises the update only against portfolios with no pending assessment (`a_queued_analysis_never_becomes_a_result`, line 977, is the only case that sleeps past COMPLETION_SPAN and it starts no assessment), and tests/all/assessment_analysis.rs seeds `inclusionStatus`/`appType` BEFORE the assessment (`the_application_mode_selects_the_components_the_analysis_covers`, line 866). No test pins an update issued between the due instant and the first read, so nothing justifies the divergence.

## Steps to Reproduce

1. Seed/ingest one application component with `appType` set and `inclusionStatus=includeInAssessment` (tests do this through the shared engine's `application_components` map).
2. POST /put-portfolio-preferences with an `applicationPreferences.managementPreference.awsManagedResources.targetDestination` of ["Amazon Elastic Cloud Compute (EC2)"] -> 200.
3. POST /start-assessment with a `migrationhub-strategy-` analysis bucket -> 200; the completion falls due at T = now + COMPLETION_SPAN (500 ms).
4. Sleep past T WITHOUT issuing any read (so nothing has called `settle_all` and the completion is still only owed).
5. POST /update-applicationcomponent-config/ with {"applicationComponentId": <id>, "inclusionStatus": "excludeFromAssessment"} -> 200.
6. GET /get-applicationcomponent-details/<id> -> the assessment is now COMPLETE with completionTime == T, yet the component reports no `recommendationSet` and no `lastAnalyzedTimestamp`.
7. Repeat steps 1-3, but issue the GET once before sleeping: the same component now carries the EC2/Rehost recommendation and a `lastAnalyzedTimestamp` of T — the analysis a read produces depends on when it was first read, which the crate's own rule (state.rs:1710-1711) says it must not.
