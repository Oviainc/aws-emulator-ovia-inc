Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists
Issue-Key: cloudformation__sqs__ssm|DeleteStack|ValidationError
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudformation__sqs__ssm: DeleteStack on a stack whose export is in use is refused synchronously with ValidationError instead of being accepted and then cancelled, so the stack is never left healthy with a 'Delete canceled' reason

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
Cause 1 of 1 — the in-use-export guard is applied as a synchronous refusal of the DeleteStack CALL, where AWS accepts the call and cancels the delete asynchronously.
AWS: Smithy: DeleteStack has an empty output and models only TokenAlreadyExistsException — there is no modelled error for an in-use export, i.e. the call itself is not refused. Recorded behavior (measured by the check against real AWS): DeleteStack on the exporting stack is ACCEPTED; the stack enters DELETE_IN_PROGRESS ("User Initiated"), CloudFormation then cancels the delete, and the stack returns to CREATE_COMPLETE with StackStatusReason `Delete canceled. Cannot delete export <export-name> as it is in use by <importing-stack-name>.` — fixed text "Delete canceled. Cannot delete export" / "as it is in use by", with the export name and the importing stack's name taken from state. Nothing is destroyed, and once the importing stack is deleted the byte-identical DeleteStack completes.
Emulator: the DeleteStack API call itself raises `ValidationError: Export <export-name> cannot be deleted as it is in use by <importing-stack-name>` (a 400 refusal of the request), so the check aborts on an unexpected ClientError: the stack never enters DELETE_IN_PROGRESS, no delete attempt is recorded, and no 'Delete canceled' StackStatusReason is ever produced. Hypothesis: if the refusal instead came from the control leg (the identical delete issued after the importing stack was deleted), the second defect is that the export's import registration is never released when the importer goes away — either way the call is refused where AWS answers 200.
Why: a client/CLI that deletes an exporting stack sees a hard synchronous error instead of AWS's accept-then-cancel sequence, so it cannot read the cancellation reason from the stack (the documented way to learn which export and which importer blocked it), it cannot poll the delete to completion, and teardown automation that treats DeleteStack as always-accepted breaks. With the refusal raised at call time, the whole post-condition chain (stack still CREATE_COMPLETE, resources intact, then a clean delete after the importer is gone) is unobservable.
Fix: the DeleteStack handler in the cloudformation crate — the in-use-export check must move out of request validation into the delete workflow, which accepts the call, records a DELETE_IN_PROGRESS "User Initiated" event, cancels, returns the stack to its previous healthy status, and writes the 'Delete canceled. Cannot delete export ... as it is in use by ...' StackStatusReason; the import index must also be cleared when the importing stack is deleted so the retry succeeds.
Traces: cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists

## Evidence
cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the DeleteStack operation: Export sim2real-xsvc-exp-name-<id> cannot be deleted as it is in use by sim2real-xsvc-exp-b-<id>
  (raised out of the client call, not caught by the check — the check expects DeleteStack to be accepted and then polls for the delete attempt to resolve, asserting the stack is back at CREATE_COMPLETE with a 'Delete canceled' reason naming both the export and the importing stack)

Smithy consulted: DeleteStack — input StackName (required), output empty, errors [TokenAlreadyExistsException] only; no in-use-export error is modelled, so AWS does not refuse this call.

## Steps to Reproduce
1. CreateStack A: an AWS::SQS::Queue with Outputs.QueueUrl exported under a unique Export.Name.
2. CreateStack B: an AWS::SSM::Parameter whose Value is {"Fn::ImportValue": <that export name>}; confirm ListImports(ExportName=...) names stack B.
3. Call DeleteStack(StackName=A): it must be ACCEPTED (200). Poll DescribeStacks/DescribeStackEvents — the stack must show a delete attempt and then return to CREATE_COMPLETE with StackStatusReason 'Delete canceled. Cannot delete export <export> as it is in use by <stack B>.'; the emulator raises 400 ValidationError from the DeleteStack call instead.
4. Delete stack B, then repeat DeleteStack(StackName=A): it must reach DELETE_COMPLETE and the export must disappear.
