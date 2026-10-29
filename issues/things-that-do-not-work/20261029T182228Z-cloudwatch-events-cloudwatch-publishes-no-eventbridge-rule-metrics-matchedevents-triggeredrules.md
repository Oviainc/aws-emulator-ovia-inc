Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule
Issue-Key: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule|_poll_until_stable#25|Timeout|time.sleep(interval_s)
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# CloudWatch publishes no EventBridge rule metrics (MatchedEvents/TriggeredRules), so the per-rule counters are never observable

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the AWS/Events namespace carries no per-rule MatchedEvents / TriggeredRules datapoints.
AWS: EventBridge publishes to the AWS/Events namespace the metrics MatchedEvents and TriggeredRules, dimensioned by RuleName, incrementing only for the rule whose pattern matched the event; a sibling rule that did not match keeps a zero sum over the same period. recorded behavior only for the exact period alignment.
Emulator: the metric read never converges — the check ends "Check timed out after 60.0s" waiting for a non-zero sum on the matching rule's RuleName dimension, so neither the positive counter nor the sibling's zero can be observed.
Why: a client that alarms on MatchedEvents/TriggeredRules (to detect a rule that stopped matching, or a target that stopped firing) gets no data and either alarms on INSUFFICIENT_DATA forever or treats a dead bus as healthy.
Fix: the EventBridge rule-matching path must emit the AWS/Events MatchedEvents and TriggeredRules metrics with the RuleName dimension into the CloudWatch metric store.
Traces: cloudwatch__events.matched-events-and-triggered-rules-count-only-the-matching-rule

## Evidence
"Error: Check timed out after 60.0s" — GetMetricStatistics/GetMetricData on namespace AWS/Events, metric MatchedEvents and TriggeredRules, dimension RuleName=<matching rule>, never returns a non-zero Sum after a matching PutEvents. Expected: a non-zero Sum for the matching rule and a zero/absent Sum for the non-matching sibling rule over the same period.

## Steps to Reproduce
Create two rules with different patterns on the same bus, PutEvents an entry matching exactly one of them, then read AWS/Events MatchedEvents and TriggeredRules with dimension RuleName for both rules. Neither rule ever reports a datapoint.
