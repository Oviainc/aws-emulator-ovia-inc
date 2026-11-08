Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle
Issue-Key: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle|phase#10|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e

# iam__lambda__s3: Lambda GetPolicy for a deleted function answers ResourceNotFoundException with the ARN-naming GetFunction message instead of AWS's generic 'The resource you requested does not exist.' (1 cause)

> _Note: this issue was identified against the code base at commit `3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy

## Description
Cause 1 of 1: after the function is deleted, lambda GetPolicy is correctly refused ResourceNotFoundException but with the wrong message — it reuses GetFunction's ARN-naming text rather than the generic not-found text AWS uses for the resource-policy surface. Failing check: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle (phase 5/7 post_invoke).

AWS: Smithy: GetPolicy (lambda, GET /2015-03-31/functions/{FunctionName}/policy) declares errors InvalidParameterValueException (400), ResourceNotFoundException (404), ServiceException (500), TooManyRequestsException (429). For a FunctionName that does not resolve, AWS answers HTTP 404 with code ResourceNotFoundException and the fixed message `The resource you requested does not exist.` — this message has no variable part at all; notably it does NOT name the function or its ARN, which is what distinguishes it from GetFunction's not-found text.

Emulator: the code is right and the message is wrong. GetPolicy on the deleted function answers ResourceNotFoundException with `Function not found: arn:aws:lambda:<region>:<account-id>:function:<function-name>` — the GetFunction-style message, whose variable parts are the region, account id and function name from the request.

Why: a client that branches on the refusal text — error-classification tables, log-based alerting, idempotent teardown that distinguishes 'no such function' from 'no policy on this function' — takes the wrong branch, and a message that leaks the resolved ARN of a resource AWS declines to name changes what the refusal tells the caller.

Fix: the GetPolicy handler's not-found path (its refusal construction), which must emit the generic ResourceNotFoundException message for this operation rather than deferring to the shared GetFunction not-found text.

Traces: iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle, which reaches this call in its post_invoke phase after delete_function.

## Evidence
Expected (AWS): HTTP 404, code `ResourceNotFoundException`, message exactly `The resource you requested does not exist.` (all fixed text, no variable part).

Actual (emulator): code `ResourceNotFoundException`, message `Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-scn-e2e-770d9b0c51f2-conv`. The fixed part of the emulator's text is `Function not found: `; the region, account id and function name are values from the request and this run's state and are not part of any requirement.

Replay line: `AssertionError: [phase 5/7 post_invoke] the resource policy for the deleted function 'sim2real-scn-e2e-770d9b0c51f2-conv' still resolves or reports ('ResourceNotFoundException', 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-scn-e2e-770d9b0c51f2-conv'), expected ('ResourceNotFoundException', 'The resource you requested does not exist.')`

## Steps to Reproduce
1. Create a Lambda function and add a resource policy statement (e.g. AddPermission for s3.amazonaws.com).
2. Delete the function.
3. Call lambda GetPolicy(FunctionName=<the deleted name>). Expect 404 ResourceNotFoundException with message 'The resource you requested does not exist.'; observe 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>'.
4. Replay iam__lambda__s3.e2e-csv-to-xlsx-pipeline-lifecycle.
