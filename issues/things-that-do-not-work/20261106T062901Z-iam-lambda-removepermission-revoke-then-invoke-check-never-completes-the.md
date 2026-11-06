Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda.xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied
Issue-Key: iam__lambda.xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied|call#16|Timeout|resp = client.invoke(FunctionName=fn, Payload=json.dumps({"marker": marker}).encode())
Issue-Commit: 5b7d66e77a060af6cd000fd4d7e09192c88c4502

# RemovePermission revoke-then-invoke check never completes: the invoke after the resource-policy statement is removed hangs past 60s

> _Note: this issue was identified against the code base at commit `5b7d66e77a060af6cd000fd4d7e09192c88c4502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RemovePermission

## Description
Cause 1 of 1: after RemovePermission deletes the statement that authorised a caller, the follow-up invoke neither succeeds nor is refused inside the check's deadline.
AWS: RemovePermission deletes the named statement from the function's resource policy, and the next Invoke by the de-authorised principal is refused promptly with AccessDeniedException (403); the positive control (before the removal) returns 200.
Emulator: "Error: Check timed out after 60.0s" - the sequence add-permission -> invoke -> remove-permission -> invoke never settles, so neither the grant nor the revocation can be observed. Recorded behavior only: the replay records no response for the hanging call.
Why: a client cannot verify that revoking an invoke grant takes effect - the most basic authorization property of a function's resource policy - and a hang is indistinguishable from an outage.
Fix: the RemovePermission handler and the resource-policy authorization path of the Invoke handler (the revoked state must produce a prompt refusal).
Traces: iam__lambda.xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied.

## Evidence
Expected (AWS): Invoke succeeds while the AddPermission statement exists; after RemovePermission the same Invoke is refused with AccessDeniedException (HTTP 403) - both answers return promptly.
Actual (emulator): Error: Check timed out after 60.0s (no answer to the post-revocation invoke).

## Steps to Reproduce
1. Replay the behavioral suite and re-run iam__lambda.xsvc-iam-2-lambda-remove-permission-revokes-invoke-denied.
2. CreateFunction, AddPermission for a principal, Invoke as that principal, RemovePermission(StatementId=...), then Invoke again as the same principal.
