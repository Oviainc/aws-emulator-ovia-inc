Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists
Issue-Key: cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists|run#20|AssertionError|assert refused == "CREATE_COMPLETE", (
Issue-Commit: 6ef54a59763e785e9d514d65e30cf6b5980831b9

# CloudFormation DeleteStack: an in-use export does not cancel the delete — the exporting stack ends DELETE_FAILED instead of being restored to CREATE_COMPLETE with a 'Delete canceled' reason

> _Note: this issue was identified against the code base at commit `6ef54a59763e785e9d514d65e30cf6b5980831b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
Cause 1 of 1: CloudFormation does not honour the export/import pin on DeleteStack. Failing check: cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists. (Service attributed as cloudformation: the check id's service segment list is cloudformation__sqs__ssm and the diverging behavior is entirely CloudFormation's export-pin handling; the SQS and SSM legs of the check pass.)

AWS: a stack Output declared with an Export whose name is consumed by another stack's Fn::ImportValue is PINNED. DeleteStack on the exporting stack is accepted (HTTP 200, output shape Unit) and the stack enters DELETE_IN_PROGRESS; CloudFormation then CANCELS the delete, returns the stack to the healthy status it held before the attempt (CREATE_COMPLETE here), destroys nothing, and records the refusal in the stack's event stream / StackStatusReason. The recorded reason begins with the fixed text "Delete canceled" and names both the export and the importing stack — AWS's wording for this class is "Delete canceled. Cannot delete export <ExportName> as it is in use by <ImportingStackName>": the fixed text is "Delete canceled. Cannot delete export " / " as it is in use by ", and the variable parts are the export name taken from the exporting stack's Output Export.Name and the importing stack name(s) taken from the same pin ListImports reports. The exported resource keeps working after the cancelled delete, and the identical DeleteStack succeeds cleanly once the importing stack is gone. Smithy (services/service-com-amazonaws-cloudformation-cloudformation): DeleteStack returns Unit and models only TokenAlreadyExistsException, so this is NOT a synchronous refusal — it is asynchronous stack-state behavior, recorded behavior only for the status transition and the reason text; ListImports (which correctly reports the importing stack in the emulator) is the model surface that records the pin.

Emulator: DeleteStack is accepted and the delete is then actually attempted in spite of the live import. The delete attempt resolves with the stack in DELETE_FAILED, not back at CREATE_COMPLETE, and no 'Delete canceled' reason naming the export or the importing stack is recorded in DescribeStackEvents ResourceStatusReason or in DescribeStacks StackStatusReason. ListImports does report the importing stack, so the pin is known to the emulator but not enforced by the delete path.

Why: a client relying on CloudFormation's export pin to protect a shared resource gets the opposite signal. Instead of a cancelled delete and a healthy stack, it sees a terminal DELETE_FAILED — which conventionally means 'resources are stranded, retry the delete, optionally with RetainResources or DeletionMode=FORCE_DELETE_STACK'. Automation that follows that convention will force the delete through and destroy a resource another live stack still imports. The absent reason text also means the operator is never told which export or which stack blocked the delete, so the cross-stack dependency is undiagnosable from CloudFormation's own output.

Fix: the DeleteStack handler's dependency evaluation plus the stack state machine in the cloudformation service — before/while executing a delete, the handler must resolve the stack's declared exports against the live import set (the same data ListImports serves) and, on a live import, cancel the delete, restore the stack's previous status, and emit a stack event carrying AWS's 'Delete canceled. Cannot delete export <name> as it is in use by <stack>' reason, instead of running the delete to DELETE_FAILED. Related OPEN issues that do NOT cover this cause: 20261027T205424Z-cloudformation-2-net-new-behavioral-root-causes-an-513ffe.md (UpdateStack changing an exported Output value while imported), 20261025T133035Z-cloudformation-7-net-new-root-causes-cli-deploy.md (export-name collision / Fn::ImportValue resolution), and 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md (rollback of failed create/update) — none addresses DeleteStack's in-use-export cancellation.

Traces: cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists (behavioral check).

## Evidence
Failing check cloudformation__sqs__ssm.cfn-delete-exporting-stack-rejected-while-import-exists (verdict: mismatch).

Assertion from the replay:
  assert refused == "CREATE_COMPLETE", (
      "an in-use export must CANCEL the delete and leave the stack healthy, got ...")
  AssertionError: an in-use export must CANCEL the delete and leave the stack healthy, got 'DELETE_FAILED'

Expected (AWS):
  - DescribeStacks(stack A).StackStatus == 'CREATE_COMPLETE' once the delete attempt resolves.
  - A reason recorded for stack A (DescribeStackEvents ResourceStatusReason or DescribeStacks StackStatusReason) containing the fixed text 'Delete canceled' and naming both the export name and the importing stack: 'Delete canceled. Cannot delete export <ExportName> as it is in use by <ImportingStackName>'.
  - Nothing destroyed: the exported SQS queue still accepts SendMessage and ReceiveMessage.
  - After the importing stack is deleted, the identical DeleteStack on stack A reaches DELETE_COMPLETE and the export disappears.

Actual (emulator):
  - DescribeStacks(stack A).StackStatus == 'DELETE_FAILED' after the delete attempt resolves.
  - No 'Delete canceled' reason naming the export or the importing stack.

What already works in the emulator (so the pin is known but unenforced): CreateStack of both stacks reaches CREATE_COMPLETE, Fn::ImportValue resolves to the real exported queue URL, and ListImports(ExportName) returns exactly the importing stack.

Model reference: Smithy DeleteStack -> output Unit, errors: [TokenAlreadyExistsException]; the cancellation is asynchronous stack state, not a synchronous error. ListImports reports the pin.

## Steps to Reproduce
1. CreateStack A with one AWS::SQS::Queue resource and an Output {"Value": {"Ref": "Exported"}, "Export": {"Name": <exportName>}}; wait for CREATE_COMPLETE and read the QueueUrl output.
2. CreateStack B with an AWS::SSM::Parameter whose Value is {"Fn::ImportValue": <exportName>}; wait for CREATE_COMPLETE.
3. Confirm the pin is visible: cfn.list_imports(ExportName=<exportName>)['Imports'] == [<stack B>] (this already holds).
4. cfn.delete_stack(StackName=<stack A>) — returns 200 — then poll until the delete attempt is no longer DELETE_IN_PROGRESS.
5. cfn.describe_stacks(StackName=<stack A>)['Stacks'][0]['StackStatus'].
   AWS: 'CREATE_COMPLETE' (delete cancelled). Emulator: 'DELETE_FAILED'.
6. Join every DescribeStackEvents ResourceStatusReason for stack A plus DescribeStacks StackStatusReason and look for 'Delete canceled' together with <exportName> and <stack B>.
   AWS: present. Emulator: absent.
7. SendMessage/ReceiveMessage on the exported queue url — must still work.
8. Delete stack B, then repeat delete_stack on stack A: it must reach DELETE_COMPLETE and <exportName> must disappear from ListExports.
