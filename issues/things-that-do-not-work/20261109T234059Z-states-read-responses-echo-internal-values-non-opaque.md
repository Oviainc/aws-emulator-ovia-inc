Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-cfode-tenant-workflow-execution-lookup-mistakes
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-cfode-tenant-workflow-execution-lookup-mistakes states-kms-execution-read-errors
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeStateMachineForExecution|2xx->2xx|$.encryptionConfiguration.kmsKeyId
Issue-Related: 20261109T234059Z-states-getexecutionhistory-omits-the-task-lambdafunction-started-event-shifting.md
Issue-Commit: 0a7cae03cb1d4210d38997f5682a54b52a42bce2

# states read responses echo internal values: non-opaque GetExecutionHistory nextToken and bare KMS key id (2 causes)

> _Note: this issue was identified against the code base at commit `0a7cae03cb1d4210d38997f5682a54b52a42bce2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionHistory

## Description
Two read operations return the emulator's internal representation of a value where AWS returns the value in its public, AWS-formatted form: a pagination cursor is emitted as a bare positional index, and a KMS key reference is emitted as a bare key id instead of the key ARN. 2 causes.

Cause 1 of 2 — GetExecutionHistory.nextToken is a bare decimal index rather than an opaque pagination token.
AWS: Smithy: GetExecutionHistory.output.nextToken targets PageToken (@length min 1, max 1024) and the operation is @paginated with inputToken/outputToken nextToken; GetExecutionHistory models InvalidToken at HTTP 400 for a token it cannot decode. Recorded behavior for the value: the token AWS hands out is an opaque string encoding the cursor, not the ordinal position of the next event.
Emulator: nextToken is the string '2' — the index of the next event in the stored history. (The same bare-index form appears on the other history reads in this corpus, e.g. '3' and '2'.)
Why: the token is contract surface. A client that persists a token obtained from the emulator and replays it against AWS sends something AWS rejects with InvalidToken, and one that moves the other way hands the emulator a real opaque token it cannot parse — so paging code that works against one does not work against the other. The index also leaks the result's absolute position, which an opaque token deliberately hides.
Fix: the pagination-token encoder GetExecutionHistory shares with the other states list/read operations — it must mint an opaque token that carries the cursor, and decode it on input.
Traces: sfn-cfode-tenant-workflow-execution-lookup-mistakes#task#8

Cause 2 of 2 — DescribeStateMachineForExecution returns encryptionConfiguration.kmsKeyId as a bare key id instead of the key ARN.
AWS: Smithy: DescribeStateMachineForExecution.output.encryptionConfiguration targets EncryptionConfiguration, whose kmsKeyId member (@length 1..2048) is documented "An alias, alias ARN, key ID, or key ARN of a symmetric encryption KMS key" — i.e. any of those forms may be SUPPLIED. Recorded behavior for the read-back: AWS reports the configured key in its full key-ARN form, arn:aws:kms:<region>:<account-id>:key/<key-id>, with the region and account of the key and the key id taken from the state machine's encryption configuration.
Emulator: kmsKeyId is only the key id portion (the bare uuid), with no arn:aws:kms:<region>:<account>:key/ prefix.
Why: a client that reads the configuration back and uses kmsKeyId directly in a KMS call, an IAM/key policy condition, or a cross-account grant needs the ARN — a bare key id resolves only within the caller's own account and region, so configuration read back from the emulator cannot be round-tripped the way AWS's can. A client comparing the configured value to the reported one also sees a spurious difference.
Fix: DescribeStateMachineForExecution's response construction for encryptionConfiguration (or the state that stores the key reference) — normalize the stored key reference to its key ARN when serializing the read.
Traces: states-kms-execution-read-errors#task#7

Note on already-filed issues: the open 20261103T052008Z-states-request-validation-gaps-and-non-opaque-pagination-tokens.md describes the same bare-index token form for ListStateMachines and for GetExecutionHistory's output member ordering, but it does not carry the replay key for the GetExecutionHistory nextToken cluster recorded here (the replay reports it unowned), and it says nothing about the KMS key reference.

## Evidence
Both causes are mismatch clusters from the replay of simparity-wire-aws-v1 against the emulator built from this checkout; both steps returned HTTP 200 as expected.

Cause 1 — sfn-cfode-tenant-workflow-execution-lookup-mistakes#task#8, GetExecutionHistory, 200 -> 200:
  $.nextToken: expected an opaque token (shaped, not literally asserted), got '2'.
  The same form recurs on sfn-failing-lambda-task-execution-diagnosis#task#11 ('3') and sfn-runtime-error-bypasses-catch-and-redrive#task#16 ('2'), where it is reported alongside the event-stream divergences filed in the companion issues.

Cause 2 — states-kms-execution-read-errors#task#7, DescribeStateMachineForExecution, 200 -> 200:
  $.encryptionConfiguration.kmsKeyId: expected 'arn:aws:kms:us-west-2:000000000000:key/d1bb033a-2ac5-42a6-bb7e-9b0b02548165', got 'd1bb033a-2ac5-42a6-bb7e-9b0b02548165'.
  The region, account id and key uuid in that value are environment values; the requirement is the key-ARN FORM, arn:aws:kms:<region>:<account-id>:key/<key-id>, built from the key's own region/account and the configured key id.

## Steps to Reproduce
1. Build the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Fixture sfn-cfode-tenant-workflow-execution-lookup-mistakes, step task#8 (the representative): call GetExecutionHistory with a maxResults smaller than the number of events and inspect $.nextToken — the emulator returns the decimal index '2' where AWS returns an opaque PageToken.
3. Fixture states-kms-execution-read-errors, step task#7: create a state machine with an encryptionConfiguration naming a customer-managed KMS key, start an execution, then call DescribeStateMachineForExecution and inspect $.encryptionConfiguration.kmsKeyId — the emulator returns the bare key id where AWS returns the full key ARN.
