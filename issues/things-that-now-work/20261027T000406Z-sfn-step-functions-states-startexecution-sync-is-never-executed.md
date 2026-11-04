Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.deep-sfn-1-sfn-sync-parallel-sibling-fail-aborts-inflight-child stepfunctions.deep-sfn-3-parallel-retry-reruns-completed-sibling stepfunctions.describe-execution-kms-access-denied
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.deep-sfn-1-sfn-sync-parallel-sibling-fail-aborts-inflight-child|_start_parent_until_child#7|AssertionError|raise AssertionError(
Issue-Key: stepfunctions.deep-sfn-3-parallel-retry-reruns-completed-sibling|run#7|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.describe-execution-kms-access-denied|_poll_until#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 6402e991ee1ffb1f37749ece0148ecf3009a760a

# Step Functions: states:startExecution.sync is never executed, a JSONata state inside a Parallel branch is never evaluated, and an execution on a CMK-encrypted state machine never advances past ExecutionStarted (3 causes)

> _Note: this issue was identified against the code base at commit `6402e991ee1ffb1f37749ece0148ecf3009a760a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Three net-new root causes that each stop an execution dead. They are listed together because they share one component (the sfn ASL interpreter's state dispatcher) but they are three independent causes and each must be closed on its own. The open issue 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md covers Task states, JSONata, Distributed Map and callbacks in the general case and 20261025T112726Z-sfn-step-functions-does-not-execute-optimized-athena-service.md covers the Athena optimized integration; none of them carries these three replay keys, and cause 3 (the CMK stall) is a distinct code path from all of them.

Cause 1 of 3: the `arn:aws:states:::states:startExecution.sync` optimized integration is refused at run time instead of executed.
AWS: recorded behavior only. A Task whose Resource is `arn:aws:states:::states:startExecution.sync` starts a child execution, records TaskSubmitted with the child's executionArn, and parks until the child reaches a terminal state; the parent then continues. The child execution exists and is describable while the parent waits.
Emulator: the parent execution ends immediately `status='FAILED'`, `error='States.Runtime'`, `cause="state '<state name>': '<the Resource ARN>' asks for the 'sync' integration pattern, whose outcome arrives after the call that starts it, so nothing was run for it"`. The fixed text is the emulator's own admission that the `.sync` pattern is unimplemented; the state name and Resource ARN are the variable parts taken from the definition. No child execution is ever created.
Why: every `.sync` service-integration workflow — the single most common Step Functions orchestration shape — fails on its first state. A caller gets a States.Runtime failure for a definition AWS accepts and runs, and the `cause` leaks emulator implementation language that no AWS client can classify.
Fix: the optimized-integration dispatcher for the `states:startExecution.sync` resource in the sfn crate's ASL interpreter — start the child execution, emit TaskSubmitted, and resume the parent on the child's terminal state instead of failing the parent.
Traces: sfn.deep-sfn-1-sfn-sync-parallel-sibling-fail-aborts-inflight-child.

Cause 2 of 3: a JSONata-language state nested inside a Parallel branch is never evaluated, failing the whole execution.
AWS: recorded behavior only (ASL QueryLanguage). A state carrying `"QueryLanguage": "JSONata"` is evaluated with JSONata semantics wherever it appears, including inside a Parallel branch, and mixing it with JSONPath states in the same machine is legal.
Emulator: the execution ends `status='FAILED'`, `error='States.Runtime'`, `cause="state '<state name>' is written in 'JSONata', and this emulator evaluates only 'JSONPath' states, so nothing was run for it"`, with history `['ExecutionStarted','ParallelStateEntered','PassStateEntered','PassStateExited','ExecutionFailed']`. The fixed text is the emulator's own self-description; the state name and language name are taken from the definition.
Why: a Parallel whose Retry depends on a JSONata gate never retries — the first attempt fails the execution instead. More broadly, any workflow that mixes a JSONata state into a JSONPath machine fails at run time on a definition AWS accepts, and the cause text identifies the emulator rather than any AWS error condition.
Fix: the state dispatcher / expression evaluator in the sfn crate's ASL interpreter — honour a per-state `QueryLanguage` of JSONata (including inside Parallel and Map branches) instead of failing the execution.
Traces: sfn.deep-sfn-3-parallel-retry-reruns-completed-sibling.

Cause 3 of 3: an execution of a CUSTOMER_MANAGED_KMS_KEY-encrypted state machine never advances past ExecutionStarted.
AWS: recorded behavior only. A state machine created with `encryptionConfiguration = {type: CUSTOMER_MANAGED_KMS_KEY, kmsKeyId: <key arn>, kmsDataKeyReusePeriodSeconds: <n>}`, whose execution role holds kms:GenerateDataKey/kms:Decrypt on the key, runs exactly like an unencrypted one: the execution progresses into its states and GetExecutionHistory shows the state-entered events. The identical definition (a single long Wait) progresses normally on an unencrypted machine in this same suite.
Emulator: GetExecutionHistory for such an execution still returns only `['ExecutionStarted']` after 30 s of polling — the Wait state is never entered. The execution is stuck, not failed: no error, no cause, no terminal event.
Why: every CMK-encrypted state machine is inert. A caller sees StartExecution succeed and then an execution that never progresses and never terminates, so the whole encrypted-state-machine feature — and every refusal that depends on reading an encrypted execution — is unreachable.
Fix: the execution scheduler / state-dispatch path in the sfn crate that handles a state machine with an encryptionConfiguration — after negotiating the data key, continue dispatching states instead of leaving the execution parked at ExecutionStarted.
Traces: sfn.describe-execution-kms-access-denied.

## Evidence
Cause 1 — sfn.deep-sfn-1-sfn-sync-parallel-sibling-fail-aborts-inflight-child:
  AssertionError: parent ended 'FAILED' before starting its child: 'States.Runtime' "state 'CallChild': 'arn:aws:states:::states:startExecution.sync' asks for the 'sync' integration pattern, whose outcome arrives after the call that starts it, so nothing was run for it"
  (fixed text = the emulator's unimplemented-pattern sentence; 'CallChild' and the Resource ARN come from the definition)

Cause 2 — sfn.deep-sfn-3-parallel-retry-reruns-completed-sibling:
  AssertionError: the retried Parallel must SUCCEED on its second attempt; got 'FAILED' error='States.Runtime' cause="state 'Gate' is written in 'JSONata', and this emulator evaluates only 'JSONPath' states, so nothing was run for it" history=['ExecutionStarted','ParallelStateEntered','PassStateEntered','PassStateExited','ExecutionFailed']

Cause 3 — sfn.describe-execution-kms-access-denied:
  setup: CreateStateMachine with encryptionConfiguration {type: CUSTOMER_MANAGED_KMS_KEY, kmsKeyId: <key arn>, kmsDataKeyReusePeriodSeconds: <n>}, execution role granted kms:GenerateDataKey/kms:Decrypt/kms:DescribeKey via the key policy, definition = single Wait state of 900 s.
  expected: the execution enters its Wait state (WaitStateEntered appears in GetExecutionHistory)
  actual:   AssertionError: the execution has entered its Wait state not met within 30s. Last result: ['ExecutionStarted']
  The execution is neither progressing nor terminating; no error/cause is reported.

## Steps to Reproduce
Cause 1:
1. Create a child STANDARD state machine with a long Wait, and a parent STANDARD state machine with a Parallel whose branch A is a Task {"Resource": "arn:aws:states:::states:startExecution.sync", "Parameters": {"StateMachineArn": <child>, "Input": {...}}}.
2. StartExecution on the parent, then DescribeExecution.
3. The parent is already FAILED with States.Runtime and the "asks for the 'sync' integration pattern ... nothing was run for it" cause; ListExecutions on the child shows no execution.

Cause 2:
1. Create a STANDARD (JSONPath) state machine containing a Parallel with Retry, one of whose branch states carries "QueryLanguage": "JSONata" with a JSONata Condition/Assign.
2. StartExecution, DescribeExecution.
3. FAILED / States.Runtime with cause "state '<name>' is written in 'JSONata', and this emulator evaluates only 'JSONPath' states, so nothing was run for it"; the Parallel never retries.

Cause 3:
1. KMS CreateKey (symmetric, ENCRYPT_DECRYPT); PutKeyPolicy granting the state machine's execution role kms:GenerateDataKey, kms:Decrypt, kms:DescribeKey.
2. CreateStateMachine (STANDARD) with definition {"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":900,"End":true}}} and encryptionConfiguration {"type":"CUSTOMER_MANAGED_KMS_KEY","kmsKeyId":<key arn>,"kmsDataKeyReusePeriodSeconds":<n>}.
3. StartExecution, then poll GetExecutionHistory.
4. The history stays ['ExecutionStarted'] indefinitely — WaitStateEntered never appears. Repeat with the same definition on a state machine with no encryptionConfiguration and the Wait is entered immediately.
