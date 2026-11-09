Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cli-lambda-create-function-missing-role-negative
Issue-Key: cloudformation__iam__lambda__s3.cli-lambda-create-function-missing-role-negative|run#5|AssertionError|assert proc.returncode != 0, "create-function unexpectedly succeeded with a nonexistent role ARN"
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# Lambda CreateFunction accepts a Role ARN that names no existing role

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Cause 1 of 1: CreateFunction does not check that the Role ARN resolves to an existing IAM role, so a function is created against a role that does not exist.

AWS: CreateFunction validates the execution role: a syntactically valid ARN that names no role is refused with HTTP 400 InvalidParameterValueException and a message of the form `The role defined for the function cannot be assumed by Lambda.` (or, while IAM is still converging, `The role defined for the function cannot be assumed by Lambda.` after retries) — the fixed text is that sentence; nothing in it is per-run. The CLI therefore exits non-zero and no function is created. Smithy: lambda#CreateFunction.Role is a required member constrained by the ARN pattern, and the operation declares InvalidParameterValueException and ResourceNotFoundException; the role-resolution check itself is a service behaviour — recorded behavior only, and the recording shows the call refused.

Emulator: CreateFunction answers 200/201 and creates the function; the CLI exits 0.

Why: a client's negative control (deploy must fail when the role is missing or misspelled) silently passes, so a function that can never be invoked is created and the failure is deferred to invocation time with a different, unrelated error. Automated deployment pipelines treat the misconfiguration as success.

Fix: role resolution in the CreateFunction (and UpdateFunctionConfiguration) validation path of the Lambda handler, which must consult IAM state and refuse with InvalidParameterValueException.

Traces: cloudformation__iam__lambda__s3.cli-lambda-create-function-missing-role-negative.

## Evidence
Expected: `aws lambda create-function --role arn:aws:iam::<account>:role/<nonexistent>` exits non-zero with InvalidParameterValueException.
Actual: AssertionError: create-function unexpectedly succeeded with a nonexistent role ARN (CLI returncode 0).

## Steps to Reproduce
1. Build a trivial deployment zip.
2. Call CreateFunction with Role set to arn:aws:iam::<account>:role/<a role that was never created>.
3. Observe the function is created and the call returns success.
