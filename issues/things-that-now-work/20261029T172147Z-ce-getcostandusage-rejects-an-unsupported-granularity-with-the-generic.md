Issue-Source: trace-sweep-divergence
Issue-Trace: cost-explorer-get-cost-and-usage-granularity-bucketing
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-explorer-get-cost-and-usage-granularity-bucketing
Issue-Shape-Id: com.amazonaws.costexplorer#AWSInsightsIndexService
Issue-Key: ce|GetCostAndUsage|4xx->4xx|$.message
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# GetCostAndUsage rejects an unsupported Granularity with the generic Smithy enum-constraint sentence instead of Cost Explorer's fixed Granularity message

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCostAndUsage

## Description
Single cause (Cause 1 of 1): the Granularity enum refusal in GetCostAndUsage is worded with the emulator's generic framework-level enum-constraint sentence rather than the sentence Cost Explorer actually serves for this member.

AWS: GetCostAndUsage.Granularity is a required member targeting the Granularity enum whose only values are DAILY, MONTHLY and HOURLY (Smithy: com.amazonaws.costexplorer#Granularity, values [DAILY, MONTHLY, HOURLY]; GetCostAndUsageRequest$Granularity required, documented as "Sets the Amazon Web Services cost granularity to MONTHLY or DAILY, or HOURLY"). The model does not name the exception used for an out-of-set Granularity, so the status and the wording here are recorded behavior only: the recording shows HTTP 400 carrying the message field exactly "Granularity can only be MONTHLY, DAILY or HOURLY". That whole sentence is FIXED text — it has no variable parts at all: it does not echo the rejected value, the member path or the accepted set, so it is identical for WEEKLY, for ANNUAL or for any other unsupported granularity.

Emulator: the same request is answered 400 (the status matches, and the error code/discriminator matches — the replay flags only $.message), but the message body is the generic constraint report "1 validation error detected: Value 'WEEKLY' at 'granularity' failed to satisfy constraint: Member must satisfy enum value set: [DAILY, MONTHLY, HOURLY]". That is the shape-level enum validator's sentence (variable parts: the rejected value, the lowercased member path, the enum value list), not Cost Explorer's.

Why: an error message is part of the contract. A client that asserts on, logs, or branches on the refusal text — a cost-reporting tool that surfaces the service's own explanation of which granularities exist, or a test that pins the refusal string — sees a sentence AWS never sends. It also leaks a different vocabulary ('1 validation error detected', a lowercased 'granularity' member path) that signals a generic constraint failure rather than Cost Explorer's own granularity rule, so clients that distinguish framework validation from service validation mis-route the failure.

Fix: the Granularity admission check in the GetCostAndUsage handler's validation for the cost-explorer service crate. The out-of-set Granularity case must be refused by the handler with the service's own fixed sentence, before (or instead of) falling through to the shared enum-constraint validator that currently composes the message; the status (400) and the error code already match and must not change.

Traces: fixture cost-explorer-get-cost-and-usage-granularity-bucketing, step task#3 (divergence key service=ce|GetCostAndUsage|status=400->400|keys=$.message).

## Evidence
Fixture cost-explorer-get-cost-and-usage-granularity-bucketing, step task#3, GetCostAndUsage with Granularity=WEEKLY.

Expected (recorded AWS): HTTP 400, body message == "Granularity can only be MONTHLY, DAILY or HOURLY" (entirely fixed text; no request- or state-derived parts).

Actual (emulator): HTTP 400, body message == "1 validation error detected: Value 'WEEKLY' at 'granularity' failed to satisfy constraint: Member must satisfy enum value set: [DAILY, MONTHLY, HOURLY]".

Replay verdict: mismatch; detail "$.message: value_mismatch". Status and error code agree; only the message text diverges. No stub_path and no owning issue — a search for GetCostAndUsage returns no existing issue.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture cost-explorer-get-cost-and-usage-granularity-bucketing, step task#3 issues GetCostAndUsage with a required TimePeriod and Metrics and Granularity set to a value outside the modeled enum (the recording uses WEEKLY).
3. Observe the 400 response body: the emulator serves the generic enum-constraint sentence; AWS serves "Granularity can only be MONTHLY, DAILY or HOURLY".
4. Repeat with any other out-of-set granularity value — AWS's sentence is unchanged, confirming the text is fixed and not value-derived.
