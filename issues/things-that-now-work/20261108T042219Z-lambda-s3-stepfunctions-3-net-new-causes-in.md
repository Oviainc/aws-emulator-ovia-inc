Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__stepfunctions.lambda-describe-sfn-missing-permission-negative lambda__s3__stepfunctions.lambda-list-sfn-executions-wrong-arn-negative lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative
Issue-Key: lambda__s3__stepfunctions.lambda-describe-sfn-missing-permission-negative|_poll_until#24|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.lambda-list-sfn-executions-wrong-arn-negative|run#21|AssertionError|assert denied.get("ok") is False, (
Issue-Key: lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative|run#29|AssertionError|assert consecutive_denials >= 3, (
Issue-Commit: 742d95e483c6290726dac54edb7cc16a76e76b19

# lambda__s3__stepfunctions: 3 net-new causes — in-Lambda DescribeExecution, ListExecutions and StartExecution are all served under an execution role that does not grant them

> _Note: this issue was identified against the code base at commit `742d95e483c6290726dac54edb7cc16a76e76b19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Three net-new root causes, one per Step Functions operation, all from the same missing gate: the emulator never evaluates the Lambda execution role's identity policy for Step Functions calls signed with that role, and in particular never re-evaluates it after a grant is revoked or when the request names a resource the policy does not cover.

Shared AWS behavior for all three: Step Functions evaluates the signing principal's identity-based policy before dispatch, and the refusal is the standard IAM one — HTTP 400 with error code `AccessDeniedException` and message `User: <assumed-role session ARN> is not authorized to perform: <action> on resource: <resource ARN> because no identity-based policy allows the <action> action`. Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`. Variable parts come from the request and the emulator's IAM state: the function execution role's assumed-role session ARN, the action, and the ARN the request names. Smithy (com-amazonaws-sfn-awsstepfunctions) models no AccessDeniedException on these operations — DescribeExecution lists only ExecutionDoesNotExist/InvalidArn/Kms*, ListExecutions only InvalidArn/InvalidToken/ResourceNotFound/StateMachineDoesNotExist/StateMachineTypeNotSupported/ValidationException, StartExecution only ExecutionAlreadyExists/ExecutionLimitExceeded/InvalidArn/InvalidExecutionInput/InvalidName/Kms*/StateMachineDeleting/StateMachineDoesNotExist/ValidationException — so authorization is the framework refusal and this is recorded behavior only.

Cause 1 of 3: states:DescribeExecution stays allowed after the grant is revoked.
  Emulator: after revoking `states:DescribeExecution` from the function's role, repeated Invokes keep returning `{'status_code': 200, 'payload': {'status': 'SUCCEEDED'}}` for the whole 45 s convergence budget — the revocation is never observed.
  Why: a client cannot verify that a revoked read permission actually stops reads; a stale or over-broad policy looks correct locally.
  Fix: the Step Functions DescribeExecution handler's authorization step, and the IAM state lookup it uses, so a policy change is read per request rather than cached or skipped.
  Traces: lambda__s3__stepfunctions.lambda-describe-sfn-missing-permission-negative.

Cause 2 of 3: states:ListExecutions is served for a state machine the role's policy does not name (resource scoping ignored).
  Emulator: with the role granted ListExecutions only on machine A, an in-handler list_executions(stateMachineArn=<machine B>) returns `{'ok': True, 'count': 0}` instead of being refused. The empty list is indistinguishable from a successful authorized read of an idle machine.
  Why: resource-level scoping is unenforced, so a policy that should confine a function to one state machine appears to work while actually permitting every machine; and the caller is handed an empty result where AWS hands it an error.
  Fix: the ListExecutions handler's authorization step — it must match the request's `stateMachineArn` against the policy's resource element, not just the action.
  Traces: lambda__s3__stepfunctions.lambda-list-sfn-executions-wrong-arn-negative.

Cause 3 of 3: states:StartExecution stays allowed after the grant is revoked.
  Emulator: after revoking `states:StartExecution`, the handler keeps starting executions and returning an executionArn; the check never sees three consecutive denials within 90 s.
  Why: an execution is created that AWS would refuse — a real side effect produced under a policy that forbids it, so the emulator both hides the permission bug and diverges in state.
  Fix: the StartExecution handler's authorization step and the credential/principal plumbing that carries the invoked function's execution-role session identity into it.
  Traces: lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative.

Related open issue that does NOT cover these (it reports the same scenario being killed at a deadline, not these three refusal gaps): 20261105T112044Z-lambda-s3-stepfunctions-the-under-permissioned-startexecution-from.md.

## Evidence
Cause 1 — lambda__s3__stepfunctions.lambda-describe-sfn-missing-permission-negative:
Expected: after revoking states:DescribeExecution, the in-handler DescribeExecution is refused HTTP 400 AccessDeniedException.
Actual: AssertionError: negative: DescribeExecution denied after revoking states:DescribeExecution not met within 45.0s. Last result: {'status_code': 200, 'payload': {'status': 'SUCCEEDED'}}

Cause 2 — lambda__s3__stepfunctions.lambda-list-sfn-executions-wrong-arn-negative:
Expected: list_executions on the state machine the policy does not grant fails (AccessDeniedException).
Actual: AssertionError: Expected list_executions on the ungranted machine B to fail; got: {'ok': True, 'count': 0}

Cause 3 — lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative:
Expected: after revoking states:StartExecution, the call is refused (3 consecutive denials).
Actual: AssertionError: Denial of states:StartExecution never stabilized within 90s; last result: {'executionArn': 'arn:aws:states:us-west-2:000000000000:execution:sfn-noperm2-sm477ecc6fe9fa:68423ea8-ede7-4353-8b29-1b3b9319e6c0'}
(ARNs, account id and generated names are per-run values; the requirement is the 400 status, the AccessDeniedException code and the message template.)

## Steps to Reproduce
1. git_clone(); run_behavioral_suite().
2. Cause 1: create a state machine and start an execution; create a Lambda whose role grants states:DescribeExecution; confirm the in-handler describe_execution succeeds; revoke that statement; re-Invoke. The emulator keeps returning status SUCCEEDED instead of AccessDeniedException.
3. Cause 2: create state machines A and B; grant the function role states:ListExecutions on A only; have the handler call list_executions(stateMachineArn=<B>). The emulator returns {'ok': True, 'count': 0} instead of refusing.
4. Cause 3: grant the function role states:StartExecution, confirm success, revoke it, then re-Invoke repeatedly. The emulator keeps returning executionArns.
5. Re-replay the three check ids in trace_ids to confirm.
