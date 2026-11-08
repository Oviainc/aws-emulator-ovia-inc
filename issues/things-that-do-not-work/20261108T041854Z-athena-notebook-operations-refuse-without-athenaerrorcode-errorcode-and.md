Issue-Source: trace-sweep-divergence
Issue-Trace: athena-notebook-reference-and-workgroup-type-errors
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-notebook-reference-and-workgroup-type-errors
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|ImportNotebook|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|ImportNotebook|4xx->4xx|$.ErrorCode
Issue-Key: athena|ImportNotebook|4xx->4xx|$.Message
Issue-Key: athena|ExportNotebook|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|ExportNotebook|4xx->4xx|$.ErrorCode
Issue-Commit: 8c5329059e752b9fa65691305eadc8c02e100ead

# Athena notebook operations refuse without AthenaErrorCode/ErrorCode and with emulator-authored message text (2 causes)

> _Note: this issue was identified against the code base at commit `8c5329059e752b9fa65691305eadc8c02e100ead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportNotebook

## Description
TWO net-new causes on Athena's notebook operations. Both refusals return the right HTTP status but strip the error-identity fields Athena puts in its InvalidRequestException body, and one also substitutes the emulator's own message wording.

Cause 1 of 2 — ImportNotebook against a non-Spark workgroup omits AthenaErrorCode/ErrorCode and rewords the message (fixture athena-notebook-reference-and-workgroup-type-errors step task#4).
AWS: ImportNotebook is modeled with error InvalidRequestException (client, HTTP 400); Smithy com.amazonaws.athena#ImportNotebook input requires WorkGroup, Name and Type. Importing a notebook into a workgroup that is not Spark-enabled is refused with HTTP 400, __type 'InvalidRequestException', AthenaErrorCode 'INVALID_INPUT' and ErrorCode 'INVALID_INPUT' (Athena's InvalidRequestException carries the error code in both members), and the message is the fixed text 'Workgroup EngineVersion is not Spark.' — that string has NO variable parts: AWS does not interpolate the workgroup name for this error class.
Emulator: HTTP 400 and __type InvalidRequestException are correct, but AthenaErrorCode and ErrorCode are both absent (None) and the message is the emulator-authored 'WorkGroup <workGroupName> is not a Spark enabled workgroup.' — different fixed wording, plus a workgroup name interpolated from the request that AWS's message does not contain.
Why: a client branches on AthenaErrorCode/ErrorCode to tell INVALID_INPUT from the other InvalidRequestException causes; with both absent it can only match on message text, and the message it would match does not exist on AWS. A test or a user-facing error renderer written against AWS fails on both fields.
Fix: the ImportNotebook handler's error construction — populate AthenaErrorCode and ErrorCode with INVALID_INPUT and emit AWS's fixed message for the non-Spark-workgroup refusal.
Traces: athena-notebook-reference-and-workgroup-type-errors#task#4.

Cause 2 of 2 — ExportNotebook for an unknown notebook id omits AthenaErrorCode/ErrorCode (fixture athena-notebook-reference-and-workgroup-type-errors step task#8).
AWS: ExportNotebook is modeled with error InvalidRequestException (client, HTTP 400) and requires NotebookId (length 1-36, uuid pattern). Exporting a notebook id that does not exist is refused with HTTP 400, __type 'InvalidRequestException', and AthenaErrorCode 'NOTEBOOK_NOT_FOUND' and ErrorCode 'NOTEBOOK_NOT_FOUND'. The message's variable part is the notebook id taken from the request; the recorded message itself matched, only the code members diverged.
Emulator: HTTP 400, __type and message are correct, but AthenaErrorCode and ErrorCode are both absent (None).
Why: NOTEBOOK_NOT_FOUND is the only machine-readable signal that distinguishes 'this notebook id does not exist' from a malformed-input refusal on the same status and the same exception type. A client that creates-or-exports, or that retries on anything other than not-found, cannot make that decision.
Fix: the ExportNotebook handler's error construction — populate AthenaErrorCode and ErrorCode with NOTEBOOK_NOT_FOUND.
Traces: athena-notebook-reference-and-workgroup-type-errors#task#8.

The open issues 20261104T063259Z-athena-refusals-omit-athenaerrorcode-errorcode-and-carry-emulator.md and 20261104T163600Z-athena-interactive-session-calculation-notebook-and-tagging-operations.md describe the same family of defect on other operations, but neither carries either of these clusters' replay keys, so both causes are filed here and those issues are named for the fixer.

## Evidence
Replay of trace set simparity-wire-aws-v1 (36 fixtures, 384 steps, score 35.9%).

Cause 1 — athena-notebook-reference-and-workgroup-type-errors#task#4, ImportNotebook, status 400 vs 400:
  $.AthenaErrorCode: missing (expected 'INVALID_INPUT', got None)
  $.ErrorCode: missing (expected 'INVALID_INPUT', got None)
  $.Message: value_mismatch (expected 'Workgroup EngineVersion is not Spark.', got 'WorkGroup <workGroupName> is not a Spark enabled workgroup.')
  Fixed text required: 'Workgroup EngineVersion is not Spark.' — no variable parts. The emulator's message both rewords it and interpolates the request's WorkGroup value.

Cause 2 — athena-notebook-reference-and-workgroup-type-errors#task#8, ExportNotebook, status 400 vs 400:
  $.AthenaErrorCode: missing (expected 'NOTEBOOK_NOT_FOUND', got None)
  $.ErrorCode: missing (expected 'NOTEBOOK_NOT_FOUND', got None)
  (status, __type and Message matched.)

Smithy cited: com.amazonaws.athena#ImportNotebook — input requires WorkGroup (pattern ^[a-zA-Z0-9._-]{1,128}$), Name, Type; errors InvalidRequestException (client, httpError 400), TooManyRequestsException, InternalServerException. com.amazonaws.athena#ExportNotebook — input requires NotebookId (length 1-36, uuid pattern); errors InvalidRequestException (client, httpError 400), TooManyRequestsException, InternalServerException. The AthenaErrorCode/ErrorCode values INVALID_INPUT and NOTEBOOK_NOT_FOUND are recorded behavior.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: fixture athena-notebook-reference-and-workgroup-type-errors, step task#4 — create a non-Spark (SQL engine) workgroup, then call ImportNotebook with that WorkGroup. Inspect the 400 body for AthenaErrorCode, ErrorCode and Message.
4. Cause 2: same fixture, step task#8 — call ExportNotebook with a well-formed but unknown NotebookId. Inspect the 400 body for AthenaErrorCode and ErrorCode.
