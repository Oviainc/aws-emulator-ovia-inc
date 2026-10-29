Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-application-ui-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-application-ui-error-paths elasticmapreduce-cluster-mutations-on-unknown-cluster elasticmapreduce-cluster-scaling-and-policy-errors elasticmapreduce-security-configuration-crud
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|CreatePersistentAppUI|4xx->4xx|$.message
Issue-Key: elasticmapreduce|DescribePersistentAppUI|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|AddJobFlowSteps|4xx->4xx|$.message
Issue-Key: elasticmapreduce|SetUnhealthyNodeReplacement|4xx->4xx|$.message
Issue-Key: elasticmapreduce|TerminateJobFlows|4xx->4xx|$.message
Issue-Key: elasticmapreduce|PutAutoScalingPolicy|4xx->4xx|$.message
Issue-Key: elasticmapreduce|RemoveAutoScalingPolicy|4xx->4xx|$.message
Issue-Key: elasticmapreduce|DescribeSecurityConfiguration|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|DeleteSecurityConfiguration|4xx->4xx|$.Message
Issue-Commit: 14bf84ae7f57c35d094a91bae03d265103651cef

# EMR unknown-id refusal messages are reworded or taken from the wrong operation's template

> _Note: this issue was identified against the code base at commit `14bf84ae7f57c35d094a91bae03d265103651cef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePersistentAppUI

## Description
This issue bundles 9 net-new root causes in the EMR (elasticmapreduce) crate that share ONE fault: the refusal for an identifier that does not resolve is returned with the right HTTP status (400) and the right exception type, but the `Message` is text the emulator authored rather than the message real EMR words for that operation. Three distinct sub-shapes of the same fault appear: (a) the emulator interpolates the offending id into a message AWS keeps CONSTANT, (b) it uses ANOTHER operation's sentence template, and (c) it quotes the variable part with the wrong quote characters. In every cause below the fixed text is the requirement and the variable parts come from the request/state, as marked; the specific ids in the recordings are per-run values, not part of the template.

Why (shared by all 9 causes): the `Message` is the only human- and CLI-visible discriminator on EMR's `InvalidRequestException`/`ValidationException` (and, per the companion issue, `ErrorCode` is missing too on many paths). A client, operator runbook or CLI wrapper that exact-matches or regex-parses AWS's sentence does not recognise the refusal; where the emulator borrows another operation's template, the two operations' refusals become indistinguishable from each other and both are wrong relative to AWS.

Fix (shared by all 9 causes): the unknown-id refusal branch of each named handler under the EMR crate's `src/ops/`. The underlying design defect is that several handlers share one id-not-found message helper; the message template must be per-operation, and the helper must stop appending the id where AWS keeps the sentence constant.

--- Cause 1 of 9: CreatePersistentAppUI appends the cluster id to a message AWS keeps constant ---
AWS: Smithy: CreatePersistentAppUI models `InvalidRequestException` (client, 400). Recorded behavior only for the text: an unresolvable target is refused 400 with the CONSTANT message `Cluster id is not valid.` — no variable part; AWS deliberately does NOT echo the id on this operation.
Emulator: 400 with `Cluster id '<ClusterId>' is not valid.` — the general-purpose cluster-id template, with the id interpolated.
Why: a caller matching the constant sentence fails; the echoed id also diverges from AWS's deliberate non-echo.
Fix: the unknown-cluster branch of the CreatePersistentAppUI handler — emit the constant, drop the interpolation.
Traces: elasticmapreduce-application-ui-error-paths#task#1.

--- Cause 2 of 9: DescribePersistentAppUI's not-found message is reworded and drops AWS's service-detail suffix ---
AWS: Smithy: DescribePersistentAppUI models `InvalidRequestException` (client, 400). Recorded behavior only: an unknown PersistentAppUI id is refused 400 with `PersistentAppUI with ID '<PersistentAppUIId>' does not exist (Service: EMRProfilers; Status Code: 400; Error Code: InvalidRequestException; Request ID: <requestId>; Proxy: null)`. Fixed text: `PersistentAppUI with ID '` / `' does not exist (Service: EMRProfilers; Status Code: 400; Error Code: InvalidRequestException; Request ID: ` / `; Proxy: null)`. Variable parts: the id from the request, and the per-request request id (that id differs every run and is NOT a requirement — the surrounding fixed text and the `Service: EMRProfilers` / `Error Code: InvalidRequestException` labels are).
Emulator: `Persistent application user interface id '<id>' is not valid.` — the noun phrase is spelled out as prose instead of the shape name, the verb phrase is `is not valid` instead of `does not exist`, and the whole service-detail suffix EMR's downstream profiler service appends is absent.
Why: nothing in AWS's sentence survives, so no message match works; clients that read the embedded `Service:`/`Error Code:` labels to attribute the failure to the EMRProfilers backend get nothing.
Fix: the not-found branch of the DescribePersistentAppUI handler.
Traces: elasticmapreduce-application-ui-error-paths#task#2.

--- Cause 3 of 9: AddJobFlowSteps appends the id to the constant job-flow sentence ---
AWS: Smithy: AddJobFlowSteps(JobFlowId required, Steps required) — recorded behavior only for the text: a nonexistent job flow is refused 400 with the CONSTANT message `Specified job flow ID not valid.` — no variable part, no id echoed.
Emulator: `Specified job flow ID not valid: '<JobFlowId>'` — the constant is mutated into a colon-prefixed form with the id appended.
Why: both the trailing period and the absence of an id are part of AWS's sentence; an exact match fails and a parser expecting no id gets one.
Fix: the unknown-job-flow branch of the AddJobFlowSteps handler.
Traces: elasticmapreduce-cluster-mutations-on-unknown-cluster#task#1.

--- Cause 4 of 9: SetUnhealthyNodeReplacement uses the job-flow sentence where AWS uses the cluster-id one ---
AWS: Smithy: SetUnhealthyNodeReplacement(JobFlowIds required, UnhealthyNodeReplacement required). Recorded behavior only: a well-formed but nonexistent id is refused 400 with `Cluster id '<JobFlowId>' is not valid.` — fixed text `Cluster id '` / `' is not valid.`; variable part: the id from JobFlowIds.
Emulator: `Specified job flow ID not valid: '<JobFlowId>'` — i.e. the template AWS reserves for the job-flow mutators (Cause 3), with an id appended AWS does not append there either.
Why: the per-operation message mapping is inverted against Cause 3, so neither operation matches AWS.
Fix: the unknown-id branch of the SetUnhealthyNodeReplacement handler — use the `Cluster id '<id>' is not valid.` template.
Traces: elasticmapreduce-cluster-mutations-on-unknown-cluster#task#9.

--- Cause 5 of 9: TerminateJobFlows appends the id to the constant job-flow sentence ---
AWS: Smithy: TerminateJobFlows(JobFlowIds required). Recorded behavior only: refused 400 with the CONSTANT `Specified job flow ID not valid.`
Emulator: `Specified job flow ID not valid: '<JobFlowId>'`.
Why / Fix: as Cause 3, in the TerminateJobFlows handler.
Traces: elasticmapreduce-cluster-mutations-on-unknown-cluster#task#10.

--- Cause 6 of 9: PutAutoScalingPolicy reports the cluster id where AWS reports the instance-group id ---
AWS: Smithy: PutAutoScalingPolicy(ClusterId required, InstanceGroupId required, AutoScalingPolicy required). Recorded behavior only: when the cluster does not resolve, AWS still refuses in terms of the INSTANCE GROUP — 400 with `Instance group id '<InstanceGroupId>' is not valid.` Fixed text `Instance group id '` / `' is not valid.`; variable part: the InstanceGroupId from the request.
Emulator: `Cluster id '<ClusterId>' is not valid.` — it resolves (and reports) the wrong input member.
Why: the message names a different identifier than the one AWS blames, so a caller is pointed at the wrong request member to fix.
Fix: the unknown-target branch of the PutAutoScalingPolicy handler — refuse in terms of InstanceGroupId.
Traces: elasticmapreduce-cluster-scaling-and-policy-errors#task#5.

--- Cause 7 of 9: RemoveAutoScalingPolicy reports the cluster id where AWS reports the instance-group id ---
AWS: Smithy: RemoveAutoScalingPolicy(ClusterId required, InstanceGroupId required). Recorded behavior only: 400 with `Instance group id '<InstanceGroupId>' is not valid.`
Emulator: `Cluster id '<ClusterId>' is not valid.`
Why / Fix: as Cause 6, in the RemoveAutoScalingPolicy handler.
Traces: elasticmapreduce-cluster-scaling-and-policy-errors#task#6.

--- Cause 8 of 9: DescribeSecurityConfiguration's not-found message single-quotes a name AWS double-quotes ---
AWS: Smithy: DescribeSecurityConfiguration(Name required), error `InvalidRequestException` (client, 400). Recorded behavior only: a name that does not exist is refused 400 with `Security configuration with name "<Name>" does not exist.` — fixed text `Security configuration with name "` / `" does not exist.` with the name in DOUBLE quotes; variable part: the Name from the request.
Emulator: identical sentence but SINGLE-quoted: `Security configuration with name '<Name>' does not exist.`
Why: an exact-match or regex assertion keyed on AWS's quoting fails, so callers that classify not-found by message mis-handle the refusal.
Fix: the shared not-found refusal used by the EMR security-configuration handlers (or the state-lookup helper they share) — emit double quotes.
Traces: elasticmapreduce-security-configuration-crud#task#8.

--- Cause 9 of 9: DeleteSecurityConfiguration's not-found message single-quotes a name AWS double-quotes ---
AWS: Smithy: DeleteSecurityConfiguration(Name required), error `InvalidRequestException` (client, 400). Recorded behavior only: 400 with `Security configuration with name "<Name>" does not exist.` (double-quoted name). This confirms Cause 8's template is shared across both operations, so the fix is one helper.
Emulator: single-quoted name.
Why / Fix: as Cause 8, reached through the DeleteSecurityConfiguration handler; fixing the shared helper closes both, but BOTH steps must be re-replayed.
Traces: elasticmapreduce-security-configuration-crud#task#9.

Note: the open behavioral issue 20261029T175551Z-emr-control-plane-refusals-use-emulator-authored-messages.md describes the same family of message defects from the BEHAVIORAL suite and overlaps on the security-configuration quoting and on SetUnhealthyNodeReplacement, but it carries no wire replay key for any of these nine fixture steps and does not cover Causes 1, 2, 3, 5, 6 or 7 at all.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator (service token `elasticmapreduce`). Every cluster below matched on status (400 expected, 400 actual); only the message member diverged.

Cause 1 — elasticmapreduce-application-ui-error-paths#task#1, CreatePersistentAppUI: `$.message: value_mismatch (expected 'Cluster id is not valid.', got "Cluster id 'j-3SD91U2P1PANK' is not valid.")`.
Cause 2 — elasticmapreduce-application-ui-error-paths#task#2, DescribePersistentAppUI: `$.Message: value_mismatch (expected "PersistentAppUI with ID 'p-3SD91U2P1PANK' does not exist (Service: EMRProfilers; Status Code: 400; Error Code: InvalidRequestException; Request ID: ffc28c5f-2866-4643-9699-2a40fa208d97; Proxy: null)", got "Persistent application user interface id 'p-3SD91U2P1PANK' is not valid.")`. (The `p-...` id and the Request ID are per-run values; the fixed text and the Service/Error Code labels are the requirement.)
Cause 3 — elasticmapreduce-cluster-mutations-on-unknown-cluster#task#1, AddJobFlowSteps: `$.message: value_mismatch (expected 'Specified job flow ID not valid.', got "Specified job flow ID not valid: 'j-3SD91U2P1PANK'")`.
Cause 4 — elasticmapreduce-cluster-mutations-on-unknown-cluster#task#9, SetUnhealthyNodeReplacement: `$.message: value_mismatch (expected "Cluster id 'j-3SD91U2P1PANK' is not valid.", got "Specified job flow ID not valid: 'j-3SD91U2P1PANK'")`.
Cause 5 — elasticmapreduce-cluster-mutations-on-unknown-cluster#task#10, TerminateJobFlows: `$.message: value_mismatch (expected 'Specified job flow ID not valid.', got "Specified job flow ID not valid: 'j-3SD91U2P1PANK'")`.
Cause 6 — elasticmapreduce-cluster-scaling-and-policy-errors#task#5, PutAutoScalingPolicy: `$.message: value_mismatch (expected "Instance group id 'ig-1PT4EYFX3F2NM' is not valid.", got "Cluster id 'j-3SD91U2P1PANK' is not valid.")`.
Cause 7 — elasticmapreduce-cluster-scaling-and-policy-errors#task#6, RemoveAutoScalingPolicy: same expected/actual pair as Cause 6.
Cause 8 — elasticmapreduce-security-configuration-crud#task#8, DescribeSecurityConfiguration: `$.Message: value_mismatch (expected 'Security configuration with name "sim2real-probe-emr-secconfig-primary-e9773f49" does not exist.', got "Security configuration with name 'sim2real-probe-emr-secconfig-primary-e9773f49' does not exist.")` — the name is a per-run value; the divergence is the quote characters.
Cause 9 — elasticmapreduce-security-configuration-crud#task#9, DeleteSecurityConfiguration: same expected/actual pair as Cause 8.

## Steps to Reproduce
Boot the emulator and replay these steps from wire trace set simparity-wire-aws-v1 (service token `elasticmapreduce`):
1. Fixture `elasticmapreduce-application-ui-error-paths`, step task#1 (CreatePersistentAppUI on a nonexistent cluster id) and step task#2 (DescribePersistentAppUI on a nonexistent `p-` id); compare `Error.Message` with the AWS text.
2. Fixture `elasticmapreduce-cluster-mutations-on-unknown-cluster`, step task#1 (AddJobFlowSteps), task#9 (SetUnhealthyNodeReplacement), task#10 (TerminateJobFlows), all against the same nonexistent cluster/job-flow id.
3. Fixture `elasticmapreduce-cluster-scaling-and-policy-errors`, step task#5 (PutAutoScalingPolicy) and task#6 (RemoveAutoScalingPolicy) with a nonexistent ClusterId plus an InstanceGroupId; assert the message blames the instance-group id.
4. Fixture `elasticmapreduce-security-configuration-crud`, step task#8 (DescribeSecurityConfiguration) and task#9 (DeleteSecurityConfiguration) on a security-configuration name that was created and then deleted; assert the name is DOUBLE-quoted in the message.
