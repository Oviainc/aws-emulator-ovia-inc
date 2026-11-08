Issue-Source: trace-sweep-divergence
Issue-Trace: glue-ml-transform-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-ml-transform-error-paths
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CreateMLTransform|4xx->4xx|$.Message
Issue-Commit: 7a00204bd93c8ed9dd88a2629fdcaac9e0c865d8

# Glue CreateMLTransform validates the role before the input schema, so a too-narrow schema is refused with the wrong message

> _Note: this issue was identified against the code base at commit `7a00204bd93c8ed9dd88a2629fdcaac9e0c865d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMLTransform

## Description
1 cause, in CreateMLTransform's input validation.

Cause 1 of 1: a FindMatches CreateMLTransform whose input record table has fewer columns than the algorithm requires is refused with the emulator's role-assumption message instead of Glue's schema refusal.
  AWS: Smithy: CreateMLTransform input members InputRecordTables (@required, GlueTables), Parameters (@required, TransformParameters -> TransformType + FindMatchesParameters) and Role (@required, RoleString); modeled errors include InvalidInputException (client, HTTP 400). The model states no column-count rule, so the rule itself is recorded behavior only: the recording shows Glue evaluating the input record table's schema BEFORE it evaluates whether the supplied Role can be assumed, and refusing 400 InvalidInputException with the message 'Number of columns in schema less than the minimum allowed number'. That message is entirely fixed text — it interpolates nothing from the request, no column count, table name or limit — so it must be reproduced verbatim. The recorded request names a role that cannot be assumed, and Glue still answers the schema refusal, i.e. the schema check takes precedence.
  Emulator: the status (400) and error code (InvalidInputException) match, but the message is emulator-authored role text: 'Unable to assume role for arn: <the request's Role ARN>' (replay detail: `$.Message: value_mismatch (expected 'Number of columns in schema less than the minimum allowed number', got 'Unable to assume role for arn: arn:aws:iam::000000000000:role/sim2real-glue-unassumed-mltransform-role')`). Here the fixed part of the emulator's text is 'Unable to assume role for arn: ' and the variable part is the Role value copied from the request; neither the sentence nor the ordering is AWS's.
  Why: a client that drives CreateMLTransform error handling off the message — the only thing that distinguishes these two 400 InvalidInputException refusals, since the code is identical — is told its IAM role is unusable when the real defect is a source table with too few columns. It retries with a different role forever and never widens the schema; a negative test asserting on Glue's schema sentence fails against the emulator.
  Fix: CreateMLTransform's input validation — the FindMatches schema check (minimum column count of the resolved InputRecordTables schema) must run and refuse ahead of the role-assumability check, and must emit AWS's fixed sentence rather than an emulator-authored one.
  Traces: glue-ml-transform-error-paths#task#3.

The replay reports this step unowned. The open issue "Glue: 2 net-new root causes in the machine-learning transform task-run surface" (20261030T063753Z) covers ML task-run state and GetMLTaskRuns ordering, not CreateMLTransform's validation order, so it does not cover this.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout.

Fixture glue-ml-transform-error-paths, step task#3, operation CreateMLTransform, verdict mismatch, expected 400 / actual 400 (error code matches; message diverges), affected_count 1:
  $.Message: value_mismatch (expected 'Number of columns in schema less than the minimum allowed number', got 'Unable to assume role for arn: arn:aws:iam::000000000000:role/sim2real-glue-unassumed-mltransform-role')

Expected (AWS, recorded): HTTP 400, __type InvalidInputException, Message 'Number of columns in schema less than the minimum allowed number' — wholly fixed text.
Actual (emulator): HTTP 400, __type InvalidInputException, Message 'Unable to assume role for arn: ' + the Role ARN taken from the request (the ARN shown is this recording's value, not part of any requirement).

Model evidence: smithy_query(operation="CreateMLTransform") on services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json — InputRecordTables/Parameters/Role all @required, InvalidInputException modeled as a 400 client error; no column-count constraint is modeled, so the rule and its wording come from the recording.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it.
2. Replay fixture glue-ml-transform-error-paths up to step task#3: it creates a database and an input record table whose schema has fewer columns than FindMatches requires, then calls CreateMLTransform with TransformType FindMatches and a Role that cannot be assumed.
3. Read the 400 response. __type is InvalidInputException as AWS returns, but Message is 'Unable to assume role for arn: <role>' instead of AWS's 'Number of columns in schema less than the minimum allowed number'.
4. Re-replay glue-ml-transform-error-paths#task#3 to confirm a fix.
