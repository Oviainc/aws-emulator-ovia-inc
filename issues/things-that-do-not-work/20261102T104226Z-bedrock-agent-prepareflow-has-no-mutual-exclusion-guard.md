Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.concurrent-prepare-flow-mutual-exclusion
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent.concurrent-prepare-flow-mutual-exclusion|run#14|AssertionError|assert len(successes) == 1, f"expected exactly 1 racer to start preparation, got {len(successes)}: {results}"
Issue-Commit: fb0a9c1858b32d12ecab08653c5d3b6198801bbb

# bedrock-agent: PrepareFlow has no mutual-exclusion guard — a second concurrent PrepareFlow on a flow already Preparing is accepted instead of refused ConflictException

> _Note: this issue was identified against the code base at commit `fb0a9c1858b32d12ecab08653c5d3b6198801bbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PrepareFlow

## Description
ONE net-new root cause of service `bedrock-agent`, grounded in one failing behavioral check the replay report left unmarked (neither ALREADY FILED nor OWNED).

Cause 1 of 1: PrepareFlow does not treat a flow that is already being prepared as busy, so two PrepareFlow calls racing on the same flow are BOTH accepted and both report `Preparing`.

AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json) — PrepareFlow is POST /flows/{flowIdentifier}/ with success status 202, output members `id` (FlowId, pattern ^[0-9a-zA-Z]{10}$) and `status` (FlowStatus: Failed | NotPrepared | Prepared | Preparing), and it models `ConflictException` (errorKind client, httpError 409, doc "There was a conflict performing an operation."). The model does not state the concurrency rule itself, so the recording supplies it: of N PrepareFlow calls issued concurrently against one flow whose DRAFT is stale, exactly ONE is accepted (202, status `Preparing`) and every other is refused HTTP 409, Error.Code `ConflictException`, with the message `Couldn't perform PrepareFlow operation. The resource is being updated by a concurrent request. Retry your request after the other request has completed.` Every word of that message is FIXED text except the operation name `PrepareFlow`, which is the refused operation; nothing else in it is taken from the request or from state. The flow then settles at `Prepared` (one preparation ran, not two).

Emulator: both racers are accepted. The replay observed `[('ok', 'Preparing'), ('ok', 'Preparing')]` where exactly one 'ok' and one ConflictException are owed. The handler (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/prepare_flow.rs) reads the flow record and unconditionally writes `record.status = Preparing` and `record.pending_preparation = Some(...)`, overwriting any preparation already in flight; it never inspects the current status for the in-progress state, so it has no path that can produce the modeled ConflictException at all.

Why: a client relying on AWS's behavior uses the 409 as the serialization point — a CI pipeline, a CDK/console-style "prepare then version" flow or two operators pressing Prepare both expect that only one preparation can be in flight and that the loser is told to retry after the winner finishes. Against the emulator every racer believes it owns the preparation, so a caller that keys on ConflictException to back off never backs off, duplicate preparations clobber each other's pending outcome (the second call resets the timer and discards the first's computed validations), and a test or guard that asserts "exactly one winner" can never pass.

Fix: the PrepareFlow handler and the flow record's in-flight preparation state in the bedrock-agent crate — before accepting, reject with ConflictException and AWS's fixed sentence when the flow already carries an unfinished preparation (status `Preparing` / a pending preparation that is not yet due), and make the accept-and-claim a single atomic step under the state guard so exactly one of N concurrent callers claims it. NOTE: open issue 20261029T172151Z-bedrock-agent-prepareflow-leaves-the-flow-in-notprepared.md records a DIFFERENT PrepareFlow defect (the flow never leaving NotPrepared and empty validations) and does not carry this check's replay key; this cause is the concurrency pre-condition and must be closed separately.

Traces: bedrock-agent.concurrent-prepare-flow-mutual-exclusion (file/line citation: services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/prepare_flow.rs lines 197-249, the unconditional status/pending-preparation write).

## Evidence
Check bedrock-agent.concurrent-prepare-flow-mutual-exclusion [mismatch]:

AssertionError: expected exactly 1 racer to start preparation, got 2: [('ok', 'Preparing'), ('ok', 'Preparing')]
  (raised at check_bedrock_agent_concurrent_prepare_flow_mutual_exclusion.py line 83)

Expected: one call accepted 202 with status 'Preparing'; the other refused HTTP 409 Error.Code 'ConflictException' with message
  "Couldn't perform PrepareFlow operation. The resource is being updated by a concurrent request. Retry your request after the other request has completed."
Actual: both calls accepted 202 with status 'Preparing'; no ConflictException was produced.

The racer count (2) and the flow id are the check's own setup and per-run values, not part of the requirement: the requirement is that of N concurrent PrepareFlow calls on one flow, exactly one is accepted and the rest are refused ConflictException with that fixed sentence, after which the flow settles at 'Prepared'.

## Steps to Reproduce
0. git_clone, then run_behavioral_suite and read the `### bedrock-agent` section of the failing-check list.
1. iam.create_role with a trust policy for bedrock.amazonaws.com, and attach an inline policy allowing bedrock:InvokeModel.
2. bedrock-agent.create_flow(name=<unique>, executionRoleArn=<that role>, definition=<one Input node, one Output node, one Data connection>).
3. bedrock-agent.prepare_flow(flowIdentifier=<id>) and poll get_flow until the status leaves Preparing/NotPrepared (warm-up, so the race below starts from a stale DRAFT).
4. bedrock-agent.update_flow(same name/role/definition, description='v2') and poll get_flow until status == 'NotPrepared'.
5. Fire 2 (or more) bedrock-agent.prepare_flow(flowIdentifier=<id>) calls concurrently from a thread pool.
6. Observe every call returns 202 with status 'Preparing'. Required: exactly one 202, and each loser refused 409 ConflictException with "Couldn't perform PrepareFlow operation. The resource is being updated by a concurrent request. Retry your request after the other request has completed."
7. Re-replay: bedrock-agent.concurrent-prepare-flow-mutual-exclusion.
