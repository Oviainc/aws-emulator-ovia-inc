Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.lambda-alias-update-to-absent-version-rejected-resourcenotfoundexception
Issue-Key: dynamodb__lambda.lambda-alias-update-to-absent-version-rejected-resourcenotfoundexception|run#24|AssertionError|assert (code, message) == ("ResourceNotFoundException", expected), (
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# dynamodb__lambda: UpdateAlias answers NotImplemented, so pointing an alias at an unpublished version is not refused ResourceNotFoundException (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAlias

## Description
Cause 1 of 1: UpdateAlias cannot refuse a bad version because the operation is a scaffold stub.

AWS: Smithy (lambda) UpdateAlias answers 200 with the AliasConfiguration and models ResourceNotFoundException (404) among its errors; pointing an alias at a FunctionVersion that was never published is refused with that error naming the qualified function ARN.

Emulator: the call is refused with code 'NotImplemented' and message 'operation UpdateAlias is scaffolded but not implemented in this emulator' (recorded message, an example), instead of the 404 ResourceNotFoundException the input deserves.

Why: NotImplemented is not in UpdateAlias's modeled error set, so a client cannot distinguish 'that version does not exist' (a request it must fix) from a platform gap, and cannot retarget an alias at all — which breaks every blue/green or canary flow built on aliases.

Fix: implement the UpdateAlias handler in the lambda service, including version resolution that refuses an unpublished version with ResourceNotFoundException. The scaffold stub itself is already filed for the lambda service ('Implement lambda UpdateAlias', the ALREADY-FILED entry naming services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/update_alias.rs); this issue records the refusal-taxonomy divergence it causes for this check, which that entry does not list.

Traces: dynamodb__lambda.lambda-alias-update-to-absent-version-rejected-resourcenotfoundexception.

## Evidence
Expected: ('ResourceNotFoundException', "Function not found: arn:aws:lambda:us-west-2:000000000000:function:<fn>:97") — HTTP 404.
Actual: ('NotImplemented', 'operation UpdateAlias is scaffolded but not implemented in this emulator').

## Steps to Reproduce
1. CreateFunction and CreateAlias pointing at $LATEST or version 1.
2. Call UpdateAlias with FunctionVersion='97' (never published).
3. Observe NotImplemented instead of ResourceNotFoundException.
