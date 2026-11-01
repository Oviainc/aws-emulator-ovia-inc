Issue-Source: trace-sweep-divergence
Issue-Trace: xray-sampling-target-for-created-rule
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-sampling-target-for-created-rule xray-trace-ingestion-and-query
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|GetSamplingTargets|2xx->2xx|$.SamplingTargetDocuments[*].Interval
Issue-Key: xray|GetTraceSummaries|2xx->2xx|$.TracesProcessedCount
Issue-Commit: 54073c01592ce4e56a3cca58cb5387afed010108

# xray wire replay: GetSamplingTargets emits an Interval AWS omits and GetTraceSummaries over-counts TracesProcessedCount (2 causes)

> _Note: this issue was identified against the code base at commit `54073c01592ce4e56a3cca58cb5387afed010108`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSamplingTargets

## Description
Two net-new root causes where the emulator answers 200 with the right shape but serves a field value AWS does not serve.

Cause 1 of 2: GetSamplingTargets populates SamplingTargetDocument.Interval on every target document, where AWS leaves it absent.
AWS: Smithy SamplingTargetDocument members are RuleName, FixedRate (double, default 0), ReservoirQuota (NullableInteger), ReservoirQuotaTTL (timestamp), Interval (NullableInteger — "The number of seconds for the service to wait before getting sampling targets again") and SamplingBoost. Interval is nullable and is NOT required; recorded behavior only: for a normal target document computed for a freshly created rule, AWS omits Interval entirely, which tells the poller to keep its own default polling cadence. AWS only sends Interval when it wants to override that cadence.
Emulator: always serializes Interval with a hard-coded value (10 in the replay), so the member is present on a document where AWS sent none. Every other member of the document matched.
Why: an SDK sampling poller treats "Interval absent" and "Interval = N" as different instructions — the second is an explicit server-driven override of its poll period. Serving a constant override on every document means the emulator cannot be used to exercise the default-cadence path at all, and a client that records or asserts on the presence of a server-supplied interval sees one that AWS never sent.
Fix: the GetSamplingTargets handler / sampling-target computation in the X-Ray service crate (services/service-com-amazonaws-xray-awsxray) — leave Interval unset on the target document unless the service is actually overriding the poll cadence, and let the serializer omit the absent member rather than defaulting it.
Traces: xray-sampling-target-for-created-rule#task#2.

Cause 2 of 2: GetTraceSummaries reports a non-zero TracesProcessedCount for a query window in which AWS reports none processed.
AWS: Smithy GetTraceSummaries (POST /TraceSummaries) returns TraceSummaries, ApproximateTime, TracesProcessedCount (NullableLong) and NextToken. Recorded behavior only for the counting rule: immediately after PutTraceSegments, the query over the ingestion window returns TracesProcessedCount 0 — the count reflects traces the indexing pipeline has actually processed into the queryable store, and a segment submitted moments earlier has not been processed yet. The TraceSummaries list matched between AWS and the emulator for this step, so the divergence is confined to the count.
Emulator: reports TracesProcessedCount 1 for the same window, i.e. it counts the just-submitted segment as processed. Hypothesis: the handler derives TracesProcessedCount directly from the number of trace records held in state that fall in the window, with no notion of a trace having been processed/indexed, so it also ends up inconsistent with the TraceSummaries it actually returns.
Why: TracesProcessedCount is what a console or a client uses to tell "the window really had no traces" from "traces were scanned but none matched the filter expression", and to judge whether ingestion has caught up before polling again. An emulator that counts a segment the instant it is submitted makes a caller believe its trace is queryable, so retry/backoff loops around ingestion latency terminate early and the subsequent lookup of a trace that is counted but not returned fails.
Fix: the GetTraceSummaries handler and the trace-index state in the X-Ray service crate — TracesProcessedCount must count only traces the emulator considers processed/indexed for the requested window, and must stay consistent with the TraceSummaries served for that window.
Traces: xray-trace-ingestion-and-query#task#3.

## Evidence
Both from replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200).

Cause 1 — xray-sampling-target-for-created-rule#task#2, GetSamplingTargets, expected 200 / actual 200:
  $.SamplingTargetDocuments[0].Interval: type_mismatch (expected None — the member absent — got 10)
  All other members of the target document (RuleName, FixedRate, ReservoirQuota, ReservoirQuotaTTL) matched. The value 10 is the emulator's constant; the requirement is the member's ABSENCE, not any particular number.

Cause 2 — xray-trace-ingestion-and-query#task#3, GetTraceSummaries, expected 200 / actual 200:
  $.TracesProcessedCount: value_mismatch (expected 0, got 1)
  No other key of the response diverged, so the returned TraceSummaries list agreed with AWS while the processed count did not.

## Steps to Reproduce
Cause 1:
1. Replay fixture xray-sampling-target-for-created-rule through step task#2: CreateSamplingRule(...), then GetSamplingTargets with a SamplingStatisticsDocument for that rule -> 200, and SamplingTargetDocuments[0] must contain RuleName/FixedRate/ReservoirQuota/ReservoirQuotaTTL with NO Interval member serialized.

Cause 2:
2. Replay fixture xray-trace-ingestion-and-query through step task#3: PutTraceSegments(<segment document>), then GetTraceSummaries(StartTime, EndTime spanning the submission) -> 200 with TracesProcessedCount 0 for that window, consistent with the TraceSummaries returned.
