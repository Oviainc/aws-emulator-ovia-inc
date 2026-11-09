Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda.create-function-requires-matching-code-signing-config-condition
Issue-Key: iam__lambda.create-function-requires-matching-code-signing-config-condition|run#38|AssertionError|raise AssertionError(
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# Lambda CreateFunction is not gated by the lambda:CodeSigningConfigArn request condition: a function with no code-signing config is created

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Cause 1 of 1: CreateFunction's authorization does not publish the request's code-signing condition key, so a policy that requires a specific CodeSigningConfigArn does not constrain it.

AWS: CreateFunction is authorized with lambda:CodeSigningConfigArn in the request context. Under an identity policy whose Allow for lambda:CreateFunction is conditioned on that key equalling the approved config ARN, a CreateFunction that supplies no CodeSigningConfigArn at all does not satisfy the condition and is refused with `AccessDeniedException` carrying AWS's standard no-matching-policy wording — `User: <principal arn> is not authorized to perform: lambda:CreateFunction on resource: <function arn> because no identity-based policy allows the lambda:CreateFunction action` — with principal, action and resource taken from the request. Smithy: lambda#CreateFunction carries the optional CodeSigningConfigArn member and declares AccessDeniedException-class refusals through the service's common errors; the condition-key set is a service fact — recorded behavior only, and the recording shows the unsigned call refused and the matching one allowed.

Emulator: the CreateFunction with no CodeSigningConfigArn SUCCEEDS.

Why: code-signing enforcement by IAM condition is the control that stops unsigned code from being deployed. Here it is inert: any principal that may create functions can create them unsigned, and because the compliant leg also succeeds the client cannot tell an enforced policy from an unenforced one.

Fix: the CreateFunction authorization step must publish lambda:CodeSigningConfigArn (present or absent) into the authorization context so conditioned statements, including the absent-key case, are evaluated.

Traces: iam__lambda.create-function-requires-matching-code-signing-config-condition.

## Evidence
Expected: CreateFunction with no CodeSigningConfigArn is refused under a policy conditioned on lambda:CodeSigningConfigArn.
Actual: AssertionError: CreateFunction with no CodeSigningConfigArn was expected to be refused but it SUCCEEDED

## Steps to Reproduce
1. Create a code-signing config and a role whose policy allows lambda:CreateFunction only when lambda:CodeSigningConfigArn equals it.
2. As that principal, call CreateFunction without CodeSigningConfigArn.
3. Observe the function is created.
