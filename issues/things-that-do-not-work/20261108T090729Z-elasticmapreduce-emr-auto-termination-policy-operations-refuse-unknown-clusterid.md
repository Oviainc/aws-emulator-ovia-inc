Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-cluster-scaling-and-policy-errors
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-cluster-scaling-and-policy-errors
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|PutAutoTerminationPolicy|4xx->4xx|$.message
Issue-Key: elasticmapreduce|RemoveAutoTerminationPolicy|4xx->4xx|$.message
Issue-Commit: 970023a244fd093fa9114d7aedbe426a3c08f756

# EMR auto-termination policy operations refuse unknown ClusterId with the job-flow message template

> _Note: this issue was identified against the code base at commit `970023a244fd093fa9114d7aedbe426a3c08f756`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAutoTerminationPolicy

## Description
This issue bundles 2 net-new root causes in the EMR (elasticmapreduce) crate that share ONE fault: the two auto-termination-policy operations, both of which take a required `ClusterId` and no job-flow member at all, refuse an unresolvable cluster with the CONSTANT legacy job-flow sentence instead of the cluster-id sentence real EMR words for a ClusterId that does not resolve. The HTTP status (400) and the exception type are correct on both paths; only the message member diverges, and it diverges the same way on both.

In both causes the requirement is AWS's template, not one recording's values: the fixed text is `Cluster id '` ... `' is not valid.` and the single variable part is the `ClusterId` taken from the request. The `j-...` id that appears in the recording is a per-run value and is not part of the template.

Why (shared by both causes): the message is the only human- and CLI-visible discriminator on EMR's 400 refusal for these operations. A client, runbook or CLI wrapper that exact-matches or regex-parses AWS's `Cluster id '<id>' is not valid.` sentence does not recognise the refusal at all; worse, the emulator's substitute sentence blames a `job flow ID` that is not even a member of either input shape, so a caller is pointed at a request member it never sent and cannot correct. Because the substitute is a CONSTANT, the offending id is also lost from the message, so a caller handling several cluster ids in one workflow cannot tell which one was rejected.

Fix (shared by both causes): the unknown-cluster refusal branch of the PutAutoTerminationPolicy and RemoveAutoTerminationPolicy handlers under the EMR crate's operation handlers. The underlying design defect is the same one the companion issue describes: several EMR handlers reach a single shared id-not-found message helper, and these two are wired to the job-flow variant of it. The message template must be selected per operation from the input member that was actually resolved — here `ClusterId` — rather than inherited from the job-flow mutators.

--- Cause 1 of 2: PutAutoTerminationPolicy emits the constant job-flow sentence for an unresolvable ClusterId ---
AWS: Smithy: PutAutoTerminationPolicy(ClusterId required, @length min 0 max 256; AutoTerminationPolicy), no operation-specific errors modeled (service-common client errors apply). Recorded behavior only for the text: a well-formed but nonexistent cluster id is refused with HTTP 400 and the message `Cluster id '<ClusterId>' is not valid.` — fixed text `Cluster id '` / `' is not valid.`, variable part the ClusterId from the request.
Emulator: HTTP 400 with the constant message `Specified job flow ID not valid.` — the template AWS reserves for the legacy job-flow mutators (AddJobFlowSteps, TerminateJobFlows), with no id echoed.
Why: nothing in AWS's sentence survives, so no message match works, and the sentence names an identifier (`job flow ID`) that is not a member of PutAutoTerminationPolicyInput.
Fix: the unknown-cluster branch of the PutAutoTerminationPolicy handler — refuse in terms of ClusterId using the `Cluster id '<id>' is not valid.` template.
Traces: elasticmapreduce-cluster-scaling-and-policy-errors#task#10.

--- Cause 2 of 2: RemoveAutoTerminationPolicy emits the constant job-flow sentence for an unresolvable ClusterId ---
AWS: Smithy: RemoveAutoTerminationPolicy(ClusterId required, @length min 0 max 256), no operation-specific errors modeled. Recorded behavior only: refused with HTTP 400 and `Cluster id '<ClusterId>' is not valid.` — same fixed text and same single variable part as Cause 1, which confirms the template is the cluster-id one for both auto-termination operations.
Emulator: HTTP 400 with the constant `Specified job flow ID not valid.`
Why / Fix: as Cause 1, in the RemoveAutoTerminationPolicy handler. If the two handlers share one helper, fixing the helper can close both — but BOTH steps must be re-replayed to confirm, since each reaches the helper through its own handler.
Traces: elasticmapreduce-cluster-scaling-and-policy-errors#task#12.

Note: the open issue 20261029T212242Z-elasticmapreduce-emr-unknown-id-refusal-messages-are-reworded-or.md bundles nine causes of this same message-template family (including PutAutoScalingPolicy and RemoveAutoScalingPolicy in the SAME fixture) and names this fixture in its trace ids, but it carries NO replay key for task#10 or task#12 and does not describe the auto-termination operations at all, so neither cause here is covered by it. The open behavioral issue 20261029T175551Z-emr-control-plane-refusals-use-emulator-authored-messages.md describes the family from the behavioral suite and likewise carries no wire replay key for these two steps.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator (service token `elasticmapreduce`). Both clusters matched on status — 400 expected, 400 actual — and diverged only on `$.message`.

Cause 1 — elasticmapreduce-cluster-scaling-and-policy-errors#task#10, PutAutoTerminationPolicy: `$.message: value_mismatch (expected "Cluster id 'j-3SD91U2P1PANK' is not valid.", got 'Specified job flow ID not valid.')`. Expected status 400, actual status 400. Fixed text required: `Cluster id '` / `' is not valid.`; the `j-3SD91U2P1PANK` between them is the ClusterId from the request and is a per-run value, NOT part of the requirement.

Cause 2 — elasticmapreduce-cluster-scaling-and-policy-errors#task#12, RemoveAutoTerminationPolicy: `$.message: value_mismatch (expected "Cluster id 'j-3SD91U2P1PANK' is not valid.", got 'Specified job flow ID not valid.')`. Expected status 400, actual status 400. Same fixed text and same variable part as Cause 1.

In both cases the emulator's message is a constant that echoes no identifier, so the diverging text is wrong independently of which cluster id the request carried.

## Steps to Reproduce
Boot the emulator and replay these steps from wire trace set simparity-wire-aws-v1 (service token `elasticmapreduce`):
1. Fixture `elasticmapreduce-cluster-scaling-and-policy-errors`, step task#10: call PutAutoTerminationPolicy with a well-formed but nonexistent ClusterId (and an AutoTerminationPolicy body). Assert HTTP 400 and that `message` is `Cluster id '<the ClusterId you sent>' is not valid.` — the emulator returns the constant `Specified job flow ID not valid.` instead.
2. Fixture `elasticmapreduce-cluster-scaling-and-policy-errors`, step task#12: call RemoveAutoTerminationPolicy with the same nonexistent ClusterId. Assert HTTP 400 and the same `Cluster id '<ClusterId>' is not valid.` message; the emulator again returns `Specified job flow ID not valid.`
3. Re-replay BOTH steps after the fix: they are reached through two different handlers even if they share a message helper.
4. Note for the fixer: the sibling steps task#5 (PutAutoScalingPolicy) and task#6 (RemoveAutoScalingPolicy) in this same fixture are owned by issue 20261029T212242Z-elasticmapreduce-emr-unknown-id-refusal-messages-are-reworded-or.md and are NOT part of this issue.
