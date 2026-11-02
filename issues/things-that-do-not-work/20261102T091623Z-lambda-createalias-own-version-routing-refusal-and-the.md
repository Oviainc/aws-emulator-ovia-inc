Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda.cfnuf-alias-routing-to-a-missing-or-its-own-version-rejected iam__lambda.cfnuf-unqualified-delete-function-ignores-the-alias-and-removes-versions-and-aliases
Issue-Key: iam__lambda.cfnuf-alias-routing-to-a-missing-or-its-own-version-rejected|run#12|AssertionError|assert msg == expected_own, f"own-version refusal message: {msg!r}"
Issue-Key: iam__lambda.cfnuf-unqualified-delete-function-ignores-the-alias-and-removes-versions-and-aliases|run#9|AssertionError|assert got == version_gone, f"version 1 of the deleted function answered {got!r}, expected {version_gone!r}"
Issue-Commit: 3879dd05c1f9087ace2750498c91b002c8086d49

# lambda: CreateAlias own-version routing refusal and the deleted-function not-found message omit AWS's wording and the version qualifier

> _Note: this issue was identified against the code base at commit `3879dd05c1f9087ace2750498c91b002c8086d49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
This issue carries 2 root causes in Lambda.

Cause 1 of 2: CreateAlias refuses routing to the alias's own version with the wrong message.
AWS: CreateAlias whose RoutingConfig.AdditionalVersionWeights names the same version as FunctionVersion is refused InvalidParameterValueException (Smithy: CreateAlias declares InvalidParameterValueException, HTTP 400) with 'Invalid function version <version>. Function version <version> is already included in routing configuration.' — fixed text with the requested version substituted twice.
Emulator: returns the right code with 'RoutingConfig.AdditionalVersionWeights names version <version>, which is the version this alias already invokes'.
Why: a client that matches AWS's wording (or shows it to an operator) sees a message AWS never emits, and cannot distinguish it from other InvalidParameterValue refusals of the same call.
Fix: the CreateAlias routing-configuration validation message.
Traces: iam__lambda.cfnuf-alias-routing-to-a-missing-or-its-own-version-rejected.

Cause 2 of 2: a qualified read of a deleted function reports the UNqualified ARN.
AWS: after an unqualified DeleteFunction, GetFunction (and UpdateAlias) with Qualifier='1' is refused ResourceNotFoundException (Smithy: GetFunction declares ResourceNotFoundException, HTTP 404; Qualifier is an httpQuery member) with 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>:1' — the message names the QUALIFIED ARN, i.e. the ARN carries the ':<qualifier>' suffix that was requested.
Emulator: answers the same code with 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>' — the qualifier is dropped.
Why: a caller cleaning up versions and aliases cannot tell from the message whether the version or the whole function is gone, and ARN-parsing clients mis-key the failure.
Fix: the not-found message construction in the GetFunction/UpdateAlias handlers must echo the qualified ARN that was resolved.
Traces: iam__lambda.cfnuf-unqualified-delete-function-ignores-the-alias-and-removes-versions-and-aliases.

## Evidence
Cause 1: "own-version refusal message: 'RoutingConfig.AdditionalVersionWeights names version 1, which is the version this alias already invokes'" where 'Invalid function version 1. Function version 1 is already included in routing configuration.' was required.
Cause 2: "answered ('ResourceNotFoundException', 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-cfnuf-...-fn'), expected ('ResourceNotFoundException', 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-cfnuf-...-fn:1')".

## Steps to Reproduce
1. Cause 1: publish version 1 of a function and call CreateAlias(FunctionVersion='1', RoutingConfig={'AdditionalVersionWeights': {'1': 0.1}}).
2. Cause 2: publish version 1, add an alias, DeleteFunction unqualified, then GetFunction(FunctionName=<name>, Qualifier='1') and UpdateAlias on the same name.
3. Or replay the check ids in trace_ids.
