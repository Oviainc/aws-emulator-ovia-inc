Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: osis.delete-pipeline-not-found
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis.delete-pipeline-not-found|run#11|AssertionError|assert malformed.get("Message") == expected_malformed, (
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# OSIS pipelineName ValidationException quotes the raw Smithy regex instead of the pattern AWS reports

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePipeline

## Description
Cause 1 of 1: a pattern-invalid `PipelineName` is correctly refused as `ValidationException` (HTTP 400), but the message renders a different constraint than AWS does. Failing check id: osis.delete-pipeline-not-found.

AWS: the refusal is the standard AWS input-validation message, built from the request: "1 validation error detected: Value '<value>' at 'pipelineName' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", where the pattern AWS reports for pipelineName is `[a-z][a-z0-9-]{2,27}` — the service's published pattern, which folds in the member's length bound. Smithy (com.amazonaws.osis#PipelineName) states @length(min 3, max 28) and @pattern "^[a-z][a-z0-9\\-]+$"; com.amazonaws.osis#ValidationException is a client error with httpError 400 and a single `message` member. The exact reported pattern text is recorded behavior (the check asserts it from real-AWS replays); the known rule is that the message names the member and the constraint the service publishes for it, not the model's internal regex literal.

Emulator: same code (`ValidationException`) and same message template, but it substitutes the Smithy regex literal verbatim: "... Member must satisfy regular expression pattern: ^[a-z][a-z0-9\\-]+$" instead of "... pattern: [a-z][a-z0-9-]{2,27}". The first half of the check passes, so the not-found classification and its message are correct; only the constraint rendering diverges.

Why: the pattern string is the only machine-readable part of the message, and it is what a client echoes to a user or matches on to explain why a name was rejected. A client told the rule is `^[a-z][a-z0-9\\-]+$` concludes any lowercase name of length >= 2 is acceptable and will retry names of 2 or of 29+ characters that the service still refuses, because the emulator's rendering drops the 3-28 length bound AWS encodes in the quoted pattern. Hypothesis: the validation layer formats the message straight from the member's @pattern trait and has no notion of the service's published constraint string.

Fix: the input-validation/message-formatting layer for OSIS member-constraint refusals — render the pattern AWS publishes for pipelineName (charset plus the length bound) rather than the model's @pattern literal, and keep the existing code (ValidationException) and HTTP 400. A previously resolved issue, "OSIS error messages diverge: VpcOptions validation message shape and generic pipeline-not-found template" (20261021T234042Z), fixed the not-found template; this residual pattern rendering is not covered by it.

Traces: osis.delete-pipeline-not-found (second assertion, after the ResourceNotFoundException half passes).

## Evidence
Expected (AWS, per the check's recorded behavior): Code=ValidationException, HTTP 400, Message="1 validation error detected: Value 'Sim2Real-Behav-Invalid' at 'pipelineName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z][a-z0-9-]{2,27}"
Actual (emulator): Code=ValidationException, HTTP 400, Message="1 validation error detected: Value 'Sim2Real-Behav-Invalid' at 'pipelineName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z][a-z0-9\\-]+$"
Suite verdict: [mismatch] osis.delete-pipeline-not-found — AssertionError at check_osis_delete_pipeline_not_found.py line 41.

## Steps to Reproduce
1. Boot the emulator and point an OSIS client at it.
2. Call osis.delete_pipeline(PipelineName="Sim2Real-Behav-Invalid") (length 22, so botocore's client-side length check does not fire; the leading uppercase letter violates the pattern).
3. Inspect the ClientError: Code is ValidationException as expected, but the Message quotes `^[a-z][a-z0-9\\-]+$` rather than `[a-z][a-z0-9-]{2,27}`.
