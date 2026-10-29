Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-permissions-and-resource-policy
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-permissions-and-resource-policy lambda-resource-policy-conditional-update-and-stale-revision lambda-resource-policy-delete-then-recreate lambda-resource-policy-lifecycle lambda-resource-policy-on-alias lambda-resource-policy-precondition-conflict lambda-resource-policy-put-get-delete-lifecycle lambda-resource-policy-revision-mismatch lambda-resource-policy-statement-and-document lambda-resource-policy-error-paths lambda-resource-policy-nonexistent-function lambda-resource-policy-errors lambda-resource-policy-invalid-policy-json
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|PutResourcePolicy|2xx->2xx|$.Policy
Issue-Key: lambda|GetResourcePolicy|2xx->2xx|$.Policy
Issue-Key: lambda|GetResourcePolicy|4xx->4xx|$.Message
Issue-Key: lambda|DeleteResourcePolicy|2xx->4xx|status
Issue-Key: lambda|PutResourcePolicy|4xx->4xx|$.message
Issue-Related: 20261029T233239Z-lambda-permission-and-policy-refusals-use-the-wrong.md
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda resource-policy documents are re-serialized with whitespace and the not-found paths diverge

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
Shared fault: the resource-policy handlers re-render the policy document instead of returning AWS's compact form, and their missing-function paths use the wrong status and the wrong refusal text. Five net-new clusters.

AWS: PutResourcePolicy/GetResourcePolicy return the `Policy` member as a compact JSON document — no space after `:` or `,` — i.e. `{"Version":"2012-10-17","Id":"default","Statement":[{"Sid":...}]}`; the Sid, Principal, Action and Resource values are the caller's and the account/function identifiers come from state. A resource-policy read for a function that does not exist is refused 404 ResourceNotFoundException with Message `The resource you requested does not exist.` (wholly fixed text — it does not name the function). DeleteResourcePolicy for a function/policy that does not exist is recorded as a 204 no-op (recorded behavior only). A syntactically invalid policy document is refused 400 with message `Policy has syntax errors.` (wholly fixed text).

Emulator: the policy is re-serialized with a space after every `:` and `,`; the not-found Message is `Function not found: <arn>`; DeleteResourcePolicy answers 404; the invalid-document refusal says `The Policy parameter must be a policy document formatted as a JSON object.`

Why: a client that stores or hashes the returned policy document, or compares it byte-for-byte with what it sent, sees a different string on every round trip; a client that treats the delete as idempotent now gets a 404 and aborts; message-matching callers do not match.

Fix: the resource-policy serializer must emit the document compactly (no inserted whitespace); GetResourcePolicy's not-found message must be AWS's fixed text; DeleteResourcePolicy must be a 204 no-op when there is nothing to delete; PutResourcePolicy's document validation must use AWS's syntax-error text.

Cause 1 of 5: PutResourcePolicy, 200 — fixture lambda-permissions-and-resource-policy step task#9; returned `Policy` string carries inserted whitespace.
Cause 2 of 5: GetResourcePolicy, 200 — fixture lambda-permissions-and-resource-policy step task#10; same whitespace defect on read-back.
Cause 3 of 5: GetResourcePolicy, 404 — fixture lambda-resource-policy-error-paths step task#4; Message must be `The resource you requested does not exist.`, emulator returned `Function not found: <arn>`; `Type` also missing.
Cause 4 of 5: DeleteResourcePolicy — fixture lambda-resource-policy-error-paths step task#5; expected 204, emulator answered 404.
Cause 5 of 5: PutResourcePolicy, 400 — fixture lambda-resource-policy-errors step task#4; message must be `Policy has syntax errors.`, emulator returned `The Policy parameter must be a policy document formatted as a JSON object.`; `Type` also missing.

## Evidence
Cluster details: task#9/task#10 `$.Policy: value_mismatch` — expected `{"Version":"2012-10-17","Id":"default","Statement":[...]}` vs actual `{"Version": "2012-10-17", "Id": "default", "Statement": [...]}` (identical content, inserted whitespace); error-paths task#4 `$.Message: value_mismatch (expected 'The resource you requested does not exist.', got 'Function not found: <arn>')`; error-paths task#5 `status: expected 204, got 404`; resource-policy-errors task#4 `$.message: value_mismatch (expected 'Policy has syntax errors.', got 'The Policy parameter must be a policy document formatted as a JSON object.')`.

## Steps to Reproduce
Replay the corpus. Fixture lambda-permissions-and-resource-policy step task#9: PutResourcePolicy and compare the returned Policy string with the compact recorded form. Fixture lambda-resource-policy-error-paths step task#5: DeleteResourcePolicy for a function with no policy — expect 204, observe 404.
