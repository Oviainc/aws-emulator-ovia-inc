Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-cfnuf-alias-repair-attempts-in-lambda
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-cfnuf-alias-repair-attempts-in-lambda
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|DeleteFunction|4xx->2xx|$
Issue-Key: lambda|DeleteFunction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|PublishVersion|2xx->2xx|$.FunctionArn
Issue-Key: lambda|PublishVersion|2xx->2xx|$.Version
Issue-Related: 20261029T233239Z-lambda-code-signing-is-never-enforced-and-code.md
Issue-Related: 20261029T233239Z-lambda-function-configuration-and-version-listings-carry-the.md
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda DeleteFunction deletes a version an alias still references

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFunction

## Description
DeleteFunction does not check whether the qualified version it is asked to delete is still referenced by an alias, so it deletes it and the version counter moves on. 2 causes, both in the DeleteFunction handler's referential-integrity check.

Cause 1 of 2 - deleting a version referenced by an alias succeeds instead of being refused.
AWS: Smithy: DeleteFunction declares ResourceConflictException (409) and takes a Qualifier. A version that one or more aliases still point at cannot be deleted: AWS refuses with HTTP 409, Type 'User' and the message 'Unable to delete version because the following aliases reference it: [<alias-names>]' - fixed text with the referencing alias names, comma-separated, inside the brackets.
Emulator: answers HTTP 204 with an empty body and deletes the version; no Type, no message, no x-amzn-errortype.
Why: an alias is left pointing at a version that no longer exists, so every invocation through that alias - the standard blue/green and stage-pinning mechanism - breaks, and the operator gets no warning at delete time. The emulator's 204 also makes a destructive mistake look like a clean idempotent delete.
Fix: the DeleteFunction handler - it must enumerate aliases referencing the qualified version and refuse before removing state.
Traces: lambda-cfnuf-alias-repair-attempts-in-lambda#task#9 (and the same unchecked delete is visible as a 204-for-404 in lam-err-code-signing-warn-then-enforce#teardown#10).

Cause 2 of 2 - the next PublishVersion is numbered one too high.
AWS: Smithy: PublishVersion output FunctionConfiguration.Version (@pattern ^(\\$LATEST|[0-9]+)$) and FunctionArn. Because the delete in cause 1 is refused, the published version is numbered 1 and FunctionArn ends ':1'.
Emulator: returns Version='2' and FunctionArn ending ':2', because cause 1's delete consumed number 1 and the counter advanced past it. The same response also carries the short RuntimeVersionArn reported separately.
Why: a client that pins a deployment to a version number - the documented way to pin immutable code - pins a number that denotes different code than it does on AWS, and the whole subsequent version history of the function is shifted. The same shift is visible in that fixture's ListVersionsByFunction, which lists an extra version.
Fix: as cause 1 - once the aliased version is not deleted, the counter is correct; the version allocator must also never reuse or skip numbers on a refused delete.
Traces: lambda-cfnuf-alias-repair-attempts-in-lambda#task#10 (2 steps affected).

## Evidence
Cause 1: lambda-cfnuf-alias-repair-attempts-in-lambda#task#9 - expected status 409 with body {'Type': 'User', 'message': 'Unable to delete version because the following aliases reference it: [<aliases>]'} and an x-amzn-errortype header; got status 204 with an empty body and no header.
Cause 2: #task#10 (201 vs 201) - $.Version expected '1', got '2'; $.FunctionArn expected '...:function:<name>:1', got '...:function:<name>:2' (2 steps affected). The same fixture's task#16 ListVersionsByFunction then reports an extra Versions[2] entry.

## Steps to Reproduce
1. Replay lambda-cfnuf-alias-repair-attempts-in-lambda and issue task#9's DeleteFunction against the version an alias still points at - it must be refused 409.
2. Then compare task#10's PublishVersion response: Version must be 1 and the ARN must end ':1'.
3. Check task#16's ListVersionsByFunction for the extra version the unchecked delete produced.
