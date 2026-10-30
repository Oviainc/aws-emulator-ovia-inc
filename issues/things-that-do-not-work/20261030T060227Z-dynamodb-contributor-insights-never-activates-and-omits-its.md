Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-ttl-contributor-insights-and-replica-autoscaling-config
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-ttl-contributor-insights-and-replica-autoscaling-config dynamodb-contributor-insights-lifecycle
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DescribeContributorInsights|2xx->2xx|$
Issue-Key: dynamodb|DescribeContributorInsights|2xx->2xx|$.ContributorInsightsMode
Issue-Key: dynamodb|DescribeContributorInsights|2xx->2xx|$.ContributorInsightsRuleList
Issue-Key: dynamodb|DescribeContributorInsights|2xx->2xx|$.ContributorInsightsStatus
Issue-Key: dynamodb|ListContributorInsights|2xx->2xx|$.ContributorInsightsSummaries[*]
Issue-Key: dynamodb|UpdateContributorInsights|2xx->2xx|$.ContributorInsightsMode
Issue-Key: dynamodb|UpdateContributorInsights|2xx->2xx|$.ContributorInsightsStatus
Issue-Key: dynamodb|UpdateContributorInsights|4xx->2xx|$.__type
Issue-Key: dynamodb|UpdateContributorInsights|4xx->2xx|$.message
Issue-Key: dynamodb|UpdateContributorInsights|4xx->2xx|status
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# DynamoDB Contributor Insights never activates and omits its mode and rule list

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeContributorInsights

## Description
The whole Contributor Insights surface is a stub-grade implementation: an ENABLE is accepted but never takes effect, the mode a caller selected is dropped from every response, the generated rule list is never populated, the disable path parks in DISABLING, and the state guard on a redundant DISABLE is missing. 7 causes, all dynamodb.

Cause 1 of 7: UpdateContributorInsights(ENABLE) is accepted but DescribeContributorInsights never leaves DISABLED, so the fixture's wait runs out.
AWS: Smithy: ContributorInsightsStatus is an enum [ENABLING, ENABLED, DISABLING, DISABLED, FAILED]; DescribeContributorInsights output carries ContributorInsightsStatus, ContributorInsightsMode and ContributorInsightsRuleList. The recording shows the status reaching 'ENABLED' with ContributorInsightsMode 'ACCESSED_AND_THROTTLED_KEYS' and a two-entry ContributorInsightsRuleList.
Emulator: 200 with ContributorInsightsStatus 'DISABLED' indefinitely — the poll gave up after 61 attempts (the harness' 60 s cap) with the status still DISABLED, and both ContributorInsightsMode and ContributorInsightsRuleList absent.
Why: enabling Contributor Insights is unobservable; any client that confirms the feature is on before reading insight data waits forever, and the ENABLE it issued is silently a no-op.
Fix: the UpdateContributorInsights handler and the per-table/per-index Contributor Insights state — commit the requested action and settle the status to ENABLED.
Traces: dynamodb-ttl-contributor-insights-and-replica-autoscaling-config#task#7.

Cause 2 of 7: ListContributorInsights returns no summary for a table whose insights were enabled.
AWS: the recording shows ContributorInsightsSummaries containing an entry for the table with ContributorInsightsStatus 'ENABLED' and ContributorInsightsMode 'ACCESSED_AND_THROTTLED_KEYS'.
Emulator: ContributorInsightsSummaries[0] absent — the list is empty.
Why: a client cannot enumerate which tables/indexes have Contributor Insights on, so fleet-wide audits report none configured.
Fix: the ListContributorInsights handler must project the same state cause 1 fails to commit.
Traces: dynamodb-ttl-contributor-insights-and-replica-autoscaling-config#task#8.

Cause 3 of 7: DescribeContributorInsights omits ContributorInsightsRuleList entirely.
AWS: Smithy: DescribeContributorInsightsOutput.ContributorInsightsRuleList is a list of the CloudWatch Contributor Insights rule names AWS mints for the table/index. The recording shows a four-entry list (its element values are generated names and are not asserted).
Emulator: ContributorInsightsRuleList missing (null).
Why: the rule names are the handle a client uses to read the insight data out of CloudWatch; without them the feature cannot be consumed even when the status says enabled.
Fix: the DescribeContributorInsights projection and the state behind it — mint and store a rule name per enabled table/index and return them.
Traces: dynamodb-contributor-insights-lifecycle#task#6 (cluster of 4, also task#7, task#8, task#9 of the same fixture).

Cause 4 of 7: UpdateContributorInsights drops ContributorInsightsMode from its response.
AWS: Smithy: UpdateContributorInsightsInput accepts ContributorInsightsMode (enum ACCESSED_AND_THROTTLED_KEYS | THROTTLED_KEYS) and UpdateContributorInsightsOutput echoes ContributorInsightsMode back. The recording returns the mode the caller requested.
Emulator: ContributorInsightsMode missing from the 200 response.
Why: the caller cannot confirm which insight mode was applied, and SDK round-trips that re-submit the described configuration lose the mode.
Fix: the UpdateContributorInsights response serializer — echo the accepted ContributorInsightsMode.
Traces: dynamodb-ttl-contributor-insights-and-replica-autoscaling-config#task#12.

Cause 5 of 7: DescribeContributorInsights drops ContributorInsightsMode as well.
AWS: Smithy: DescribeContributorInsightsOutput.ContributorInsightsMode is the stored mode; the recording returns the mode selected earlier in the fixture.
Emulator: ContributorInsightsMode missing.
Why: the configured mode is unreadable, so a client cannot tell throttled-keys-only insights from accessed-and-throttled-keys insights.
Fix: store the mode on the Contributor Insights state and project it in DescribeContributorInsights.
Traces: dynamodb-ttl-contributor-insights-and-replica-autoscaling-config#task#13.

Cause 6 of 7: UpdateContributorInsights(DISABLE) returns the transient 'DISABLING' and again omits the mode.
AWS: the recording shows the disable response carrying ContributorInsightsStatus 'DISABLED' together with the ContributorInsightsMode that was in force.
Emulator: ContributorInsightsStatus 'DISABLING' and no ContributorInsightsMode.
Why: same unterminated-transient problem as the status-settling issue — a client polling for DISABLED after a disable never sees it.
Fix: settle the status to DISABLED on an accepted disable and echo the mode.
Traces: dynamodb-ttl-contributor-insights-and-replica-autoscaling-config#task#14.

Cause 7 of 7: UpdateContributorInsights(DISABLE) against a table whose insights are not ENABLED/FAILED is accepted instead of refused.
AWS: the recording shows HTTP 400 with __type 'com.amazon.coral.validate#ValidationException' and the message 'Table or Index is not in a valid state to update Key Access Insights: ContributorInsights Status must be ENABLED or FAILED to perform DISABLE operation.' The fixed text is the whole sentence; nothing in it is request-derived.
Emulator: HTTP 200 with no __type and no message — the illegal transition is accepted.
Why: a client gets no signal that its disable was a no-op on a resource that was never enabled, so retry/reconcile logic treats an unmanaged resource as managed.
Fix: the UpdateContributorInsights validation — gate the DISABLE action on the current ContributorInsightsStatus being ENABLED or FAILED and raise ValidationException (400) otherwise.
Traces: dynamodb-ttl-contributor-insights-and-replica-autoscaling-config#task#6.

Related open issues: '20261025T184159Z-cloudwatch-dynamodb-enabling-dynamodb-contributor-insights-mints-no.md' covers the missing CloudWatch insight RULES from the CloudWatch side; it does not cover the DynamoDB-side status, mode or rule-list members, nor the DISABLE state guard.

## Evidence
Divergence 43 (DescribeContributorInsights, 200->200): poll capped at 60s; $.ContributorInsightsStatus expected 'ENABLED', got 'DISABLED' after 61 attempts; $.ContributorInsightsMode expected 'ACCESSED_AND_THROTTLED_KEYS', got None; $.ContributorInsightsRuleList expected a 2-entry list, got None.
Divergence 44 (ListContributorInsights, 200->200): $.ContributorInsightsSummaries[0] missing (expected {ContributorInsightsMode: ACCESSED_AND_THROTTLED_KEYS, ContributorInsightsStatus: ENABLED, TableName: ...}).
Divergence 23 (DescribeContributorInsights, 200->200): $.ContributorInsightsRuleList missing, expected a 4-entry list (cluster of 4).
Divergence 45 (UpdateContributorInsights, 200->200): $.ContributorInsightsMode missing (expected 'ACCESSED_AND_THROTTLED_KEYS').
Divergence 46 (DescribeContributorInsights, 200->200): $.ContributorInsightsMode missing.
Divergence 47 (UpdateContributorInsights, 200->200): $.ContributorInsightsMode missing; $.ContributorInsightsStatus expected 'DISABLED', got 'DISABLING'.
Divergence 42 (UpdateContributorInsights, 400->200): status expected 400 got 200; $.__type expected 'com.amazon.coral.validate#ValidationException'; $.message expected 'Table or Index is not in a valid state to update Key Access Insights: ContributorInsights Status must be ENABLED or FAILED to perform DISABLE operation.'
Model: smithy_query(operation=UpdateContributorInsights,DescribeContributorInsights) -> input/output ContributorInsightsMode (enum), output ContributorInsightsStatus (enum), ContributorInsightsRuleList (list), errors ResourceNotFoundException(400), InternalServerError(500); shape=ContributorInsightsStatus -> [ENABLING, ENABLED, DISABLING, DISABLED, FAILED]; shape=ContributorInsightsMode -> [ACCESSED_AND_THROTTLED_KEYS, THROTTLED_KEYS].

## Steps to Reproduce
1. Replay dynamodb-ttl-contributor-insights-and-replica-autoscaling-config#task#6: UpdateContributorInsights(DISABLE) on a not-enabled table -> 200 instead of 400 ValidationException.
2. Replay #task#7 and #task#8: after UpdateContributorInsights(ENABLE, ACCESSED_AND_THROTTLED_KEYS), DescribeContributorInsights stays DISABLED to the 60 s cap and ListContributorInsights returns no summary.
3. Replay #task#12, #task#13, #task#14: Update/Describe/Update(DISABLE) all omit ContributorInsightsMode, and the disable answers 'DISABLING'.
4. Replay dynamodb-contributor-insights-lifecycle#task#6: DescribeContributorInsights returns no ContributorInsightsRuleList.
