Issue-Source: trace-sweep-divergence
Issue-Trace: s3-x2x-lambda-destination-unpermitted-then-granted
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-x2x-lambda-destination-unpermitted-then-granted
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.RuntimeVersionConfig.RuntimeVersionArn
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# lambda RuntimeVersionArn carries a short non-AWS identifier

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunctionConfiguration

## Description
Cause 1 of 1: the runtime version identifier the emulator mints is not AWS-shaped.

AWS: GetFunctionConfiguration reports RuntimeVersionConfig.RuntimeVersionArn as arn:aws:lambda:<region>::runtime:<id>, where <id> is a 64-character lowercase hex digest identifying the managed runtime build (recorded: ...:runtime:36aeb19f3326e129d9f096dea0a3c6a9ca8dce2610e62d2b2060747602d863ae). The digest value is AWS-internal, but its form - a 64-hex identifier in the runtime ARN - is the wire contract. Recorded behavior only.

Emulator: it returns arn:aws:lambda:us-west-2::runtime:a5e88e82e3fdc7cd - a 16-hex identifier, so the ARN's resource part is a quarter of the modelled length.

Why: tooling that pins, compares or validates runtime-version ARNs (runtime management configuration, drift checks, log correlation) rejects or mis-parses the value; the ARN cannot be matched against anything AWS would publish.

Fix: the runtime-version identifier generator behind the function-configuration projection (mint a 64-character lowercase hex id), not the GetFunctionConfiguration handler's shape.

Traces: s3-x2x-lambda-destination-unpermitted-then-granted#task#4.

## Evidence
Replay of simparity-wire-aws-v1, step s3-x2x-lambda-destination-unpermitted-then-granted#task#4 (GetFunctionConfiguration, expected 200 / actual 200): $.RuntimeVersionConfig.RuntimeVersionArn value_mismatch - expected 'arn:aws:lambda:us-west-2::runtime:36aeb19f3326e129d9f096dea0a3c6a9ca8dce2610e62d2b2060747602d863ae' (64 hex characters), got 'arn:aws:lambda:us-west-2::runtime:a5e88e82e3fdc7cd' (16 hex characters). Everything else in the response matched.

## Steps to Reproduce
1. Replay fixture s3-x2x-lambda-destination-unpermitted-then-granted. 2. At step task#4 read RuntimeVersionConfig.RuntimeVersionArn from the GetFunctionConfiguration response. 3. Observe the runtime resource id is 16 hex characters where AWS's is 64.
