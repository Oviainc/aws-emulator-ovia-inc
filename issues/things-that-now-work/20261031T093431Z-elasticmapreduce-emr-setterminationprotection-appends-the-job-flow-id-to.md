Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-cluster-mutations-on-unknown-cluster
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-cluster-mutations-on-unknown-cluster
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|SetTerminationProtection|4xx->4xx|$.message
Issue-Commit: 403232ac2ece2dfa5fd1232b496a3fd148521367

# EMR SetTerminationProtection appends the job-flow id to AWS's constant refusal message

> _Note: this issue was identified against the code base at commit `403232ac2ece2dfa5fd1232b496a3fd148521367`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetTerminationProtection

## Description
One net-new root cause in the EMR (elasticmapreduce) crate.

Cause 1 of 1: SetTerminationProtection on a job flow that does not resolve is refused with the right HTTP status (400) and the right exception type, but with an emulator-authored message that mutates AWS's CONSTANT sentence into a colon-prefixed form and appends the offending id.

AWS: Smithy (com.amazonaws.emr#ElasticMapReduce): SetTerminationProtection(JobFlowIds: XmlStringList, required; TerminationProtected: Boolean, required); the model declares only InternalServerError, so the client refusal shape and its text are recorded behavior only. The recording shows a well-formed but nonexistent job-flow id refused with HTTP 400 and the CONSTANT message `Specified job flow ID not valid.` — the whole string is fixed text, including the trailing period; there is NO variable part and AWS deliberately does not echo the id from JobFlowIds. (The `j-...` id in the recording is a per-run value and is not part of the template.)

Emulator: HTTP 400 (status matches) with `$.message` = `Specified job flow ID not valid: 'j-3SD91U2P1PANK'` — AWS's terminating period is replaced by `: ` and the requested JobFlowIds entry is interpolated in single quotes.

Why: `message` is the only human- and CLI-visible discriminator on this refusal, since the status is the same 400 as every other EMR client error. A client, operator runbook or CLI wrapper that exact-matches or regex-anchors AWS's sentence (`^Specified job flow ID not valid\.$`) does not recognise the refusal, and a parser that assumes no id is embedded is handed one, so it mis-classifies the failure (e.g. treats an unknown-cluster refusal as an unparsed/unknown error and retries or aborts instead of reporting the bad id).

Fix: the unknown-job-flow refusal branch of the SetTerminationProtection handler under the EMR crate's src/ops/ — emit the constant sentence and stop appending the id. The underlying design defect is the shared id-not-found message helper several EMR mutators call: the template must be per-operation, and for the job-flow mutators it must not interpolate the id.

Related open issue: 20261029T212242Z-elasticmapreduce-emr-unknown-id-refusal-messages-are-reworded-or.md bundles nine causes of exactly this fault family (AddJobFlowSteps, TerminateJobFlows, SetUnhealthyNodeReplacement, the PersistentAppUI and security-configuration paths) and names the same shared helper, but it does NOT cover SetTerminationProtection and carries no replay key for this fixture step, so fixing and closing that issue would leave this step unrun and still diverging.

Traces: elasticmapreduce-cluster-mutations-on-unknown-cluster#task#6.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator (service token `elasticmapreduce`, operation SetTerminationProtection). Status matched (expected 400, actual 400); only the message member diverged:

  $.message: value_mismatch (expected 'Specified job flow ID not valid.', got "Specified job flow ID not valid: 'j-3SD91U2P1PANK'")

Fixed text (the requirement): the entire string `Specified job flow ID not valid.` Variable parts: none — the `j-3SD91U2P1PANK` id the emulator appends comes from the request's JobFlowIds and is a per-run value, so the defect is that any id is appended at all and that the trailing period is replaced by `: `.

Divergence key: service=elasticmapreduce|SetTerminationProtection|status=400->400|keys=$.message
Cluster: 1 affected step — elasticmapreduce-cluster-mutations-on-unknown-cluster#task#6.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay wire fixture `elasticmapreduce-cluster-mutations-on-unknown-cluster` from trace set simparity-wire-aws-v1, step `task#6`: SetTerminationProtection with a single well-formed but nonexistent job-flow id (e.g. JobFlowIds=['j-3SD91U2P1PANK'], TerminationProtected=true).
3. Assert the response is HTTP 400 and that `message` equals exactly `Specified job flow ID not valid.` — no colon, no quoted id, trailing period present.
4. Observed instead: `Specified job flow ID not valid: '<the id from JobFlowIds>'`.
