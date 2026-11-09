Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__lambda__logs.lambda-metadata-endpoint-requires-bearer-token-and-reports-a-real-az
Issue-Key: ec2__iam__lambda__logs.lambda-metadata-endpoint-requires-bearer-token-and-reports-a-real-az|run#10|AssertionError|assert "FunctionError" not in resp, f"function raised instead of returning: {resp.get('FunctionError')!r}"
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# A Lambda handler reading the instance-metadata endpoint raises Unhandled instead of returning the token-gated identity document

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the metadata endpoint the emulator exposes to a function does not satisfy the IMDSv2 contract, so the handler faults.
AWS: a function that PUTs to the token endpoint and then GETs the metadata with X-aws-ec2-metadata-token receives the token and a document naming a real Availability Zone of the function's region; the handler returns that document and Invoke reports no FunctionError.
Emulator: the invoke comes back with FunctionError 'Unhandled' — 'function raised instead of returning: 'Unhandled'' — so the bearer-token requirement and the reported AZ cannot be observed.
Why: a client whose code reads instance/function metadata (a very common pattern for region and AZ discovery) crashes instead of receiving a document.
Fix: the metadata endpoint served to in-process functions in the Lambda crate (token issuance and the identity document, including a real AZ of the region).
Traces: ec2__iam__lambda__logs.lambda-metadata-endpoint-requires-bearer-token-and-reports-a-real-az.

## Evidence
Expected: the handler returns the metadata document (and the unauthenticated GET is refused). Actual: 'AssertionError: function raised instead of returning: 'Unhandled''.

## Steps to Reproduce
Replay ec2__iam__lambda__logs.lambda-metadata-endpoint-requires-bearer-token-and-reports-a-real-az: invoke the function, which requests an IMDSv2 token and then the metadata document.
