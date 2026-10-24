Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# OpenProactiveInsights / OpenReactiveInsights are hardcoded to 0 instead of tallied from the insight store

_Filed: 2026-10-24T00:38:29Z_
_Operation: DescribeAccountHealth | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeAccountHealth returns literal zeros for `OpenReactiveInsights`, `OpenProactiveInsights` and `MetricsAnalyzed`, ignoring `state.insights` entirely, while computing only `ResourceHours`/`AnalyzedResourceCount` from the coverage. The service's own state module documents `insights` as the map the insight reads report, and the sibling read DescribeResourceCollectionHealth DOES tally open proactive/reactive insights (and MTTR) out of exactly that map. So with insights present in the store the two reads of one store contradict each other: ListInsights (StatusFilter.Ongoing) and DescribeResourceCollectionHealth report open insights of a given type while DescribeAccountHealth reports 0 open insights for the whole account. Expected: `OpenProactiveInsights` = number of stored PROACTIVE insights whose status is ONGOING, `OpenReactiveInsights` = the same for REACTIVE, both computed from the same store the other reads walk (0 only when the store holds none). Actual: constant 0 regardless of store contents.

## Evidence

Spec: smithy_query(query="doc", shape="DescribeAccountHealthResponse", member="OpenProactiveInsights") — "An integer that specifies the number of open proactive insights in your Amazon Web Services account" (same wording for OpenReactiveInsights). Handler: src/ops/describe_account_health.rs lines 191-199 — `open_reactive_insights: 0, open_proactive_insights: 0, metrics_analyzed: 0` with the comment "Nothing analyses metrics or produces insights here"; the handler reads only `resource_collection.monitored_resource_count()` and never touches `guard.insights`. Contrast src/ops/describe_resource_collection_health.rs `insight_health(insights.values(), ...)` (lines ~173-209, 313, 342, 382) which tallies open proactive/reactive insights from `guard.insights`, and src/ops/list_insights.rs which lists them. Insight records are a real, populated part of this store (tests/all/configuration_lifecycle.rs line ~104 `guard.insights.insert(...)`, used by insight_listing.rs / insight_reads.rs / resource_collection_health.rs). Tests: tests/all/account_health.rs asserts the three analysis members are 0 but only for accounts where no insight was seeded, so nothing asserts the tally over a non-empty insight store.

## Steps to Reproduce

1. Seed/have the insight store hold one ONGOING PROACTIVE insight and one ONGOING REACTIVE insight (the same path tests/all/insight_listing.rs uses, i.e. the map DescribeInsight/ListInsights read).
2. POST /insights with StatusFilter.Ongoing.Type=PROACTIVE and observe the proactive insight is listed.
3. GET /accounts/health/resource-collection/AWS_CLOUD_FORMATION (with matching coverage) and observe Insight.OpenProactiveInsights >= 1.
4. GET /accounts/health and observe OpenProactiveInsights=0 and OpenReactiveInsights=0, contradicting both reads above; expected the account-level counts of open insights.
