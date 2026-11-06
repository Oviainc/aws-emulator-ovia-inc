Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-cancel-import-task
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-cancel-import-task|run#19|AssertionError|assert code_message == expected_code_message, f"expected message {expected_code_message!r}, got {code_message!r}"
Issue-Commit: 09448fb928879ffb83c121d01adc49e65396319c

# ec2 CancelImportTask: the not-found refusal reuses the conversion-task message instead of naming the import-image-task ID

> _Note: this issue was identified against the code base at commit `09448fb928879ffb83c121d01adc49e65396319c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelImportTask

## Description
Cause 1 of 1: CancelImportTask refuses a well-formed but nonexistent import task id with the wrong message text.
AWS: CancelImportTask against a well-formed import task id that the account holds no task for is refused with HTTP 400 and the EC2 client error code InvalidConversionTaskId. The EC2 Smithy model declares no error shapes for CancelImportTask (ec2Query/awsQuery errors are not modelled per operation), so this is recorded behavior only: the recorded refusal carries code InvalidConversionTaskId and the message whose FIXED text is "The import-image-task ID '" ... "' does not exist", with the quoted id taken verbatim from the request's ImportTaskId. The message names the kind of id the request supplied — an import-image-task ID — not a conversion task.
Emulator: the status and the code are right (InvalidConversionTaskId), and the positive half of the path works (ImportSnapshot, CancelImportTask echoing the ImportTaskId, and the task reaching a cancelled-family status all pass). Only the refusal sentence diverges: it answers "The conversion task '<ImportTaskId>' does not exist" — AWS's "The import-image-task ID" is replaced with "The conversion task", and the trailing "ID" wording is lost. The handler's doc comment shows the message is produced by the shared helper crate::state::invalid_conversion_task_id_not_found, i.e. the legacy conversion-task sentence is reused for an import task id.
Why: a client that matches, logs or surfaces the refusal of a stale import task id does not recognise AWS's sentence, and the message misidentifies the resource class — it tells an operator the "conversion task" is missing when the request named an import-image-task ID (import-ami-...), which is how VM Import/Export distinguishes import image/snapshot tasks from legacy conversion tasks. Code-level retry/branching is unaffected; message-level handling and human diagnosis are wrong.
Fix: the CancelImportTask not-found refusal — the shared message helper invalid_conversion_task_id_not_found in services/service-com-amazonaws-ec2-amazonec2/src/state.rs as reached from services/service-com-amazonaws-ec2-amazonec2/src/ops/cancel_import_task.rs. CancelImportTask must word the refusal for the id class it was given (import-image-task ID) rather than reusing the conversion-task sentence that the legacy conversion-task operations need; keep the code InvalidConversionTaskId and the 400.
Traces: ec2.op-contract-cancel-import-task.

## Evidence
ec2.op-contract-cancel-import-task [mismatch]:
  File "check_ec2_op_contract_cancel_import_task.py", line 74, in run
    assert code_message == expected_code_message
  AssertionError: expected message "The import-image-task ID 'import-ami-0123456789abcdef0' does not exist", got "The conversion task 'import-ami-0123456789abcdef0' does not exist"

  Code: InvalidConversionTaskId (expected and observed — correct). Status: 400 (correct).
  Fixed text required: "The import-image-task ID '" + <ImportTaskId from the request> + "' does not exist".
  Variable part: only the quoted id, which is the ImportTaskId the caller sent (import-ami-0123456789abcdef0 in this run — a filler value, not part of the template).
  Emulator's fixed text: "The conversion task '" + <id> + "' does not exist".
  Preceding steps of the same check passed: ImportSnapshot minted a task, CancelImportTask echoed its ImportTaskId, and DescribeImportSnapshotTasks reported a cancelled-family status.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (run_behavioral_suite); the check is listed under '### ec2' as op-contract-cancel-import-task.
2. Call ec2 CancelImportTask(ImportTaskId="import-ami-0123456789abcdef0") — a well-formed import-image-task id no task exists for.
3. Inspect the ClientError: Error.Code is InvalidConversionTaskId (correct) but Error.Message is "The conversion task 'import-ami-0123456789abcdef0' does not exist" instead of AWS's "The import-image-task ID 'import-ami-0123456789abcdef0' does not exist".
