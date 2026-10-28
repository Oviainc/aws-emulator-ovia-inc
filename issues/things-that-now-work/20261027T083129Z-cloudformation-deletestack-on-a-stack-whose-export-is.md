Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.export-in-use-blocks-stack-deletion
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.export-in-use-blocks-stack-deletion|_wait_terminal_or_stall#20|Timeout|time.sleep(3)
Issue-Commit: 7ce2b3eec7df75677d4abb942fccfdb7efd0ecba

# cloudformation: DeleteStack on a stack whose export is still imported is accepted and then silently reverted to CREATE_COMPLETE, so the deletion never reaches a terminal status and every delete waiter hangs

> _Note: this issue was identified against the code base at commit `7ce2b3eec7df75677d4abb942fccfdb7efd0ecba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack

## Description
One net-new root cause in service `cloudformation`, grounded in the only failing behavioral check the replay report marked neither ALREADY FILED nor OWNED. The emulator is serving (139/404 checks pass, bootstrap_signal false), so this is a fidelity divergence, not a bootstrap gap.

Cause 1 of 1: a DeleteStack that CloudFormation cannot carry out because the stack publishes an export another live stack still imports is taken (HTTP 200) and then UNDONE — the stack is put back into the status it held before the attempt — so no terminal deletion outcome is ever published and a client that polls for one waits forever.

AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): DeleteStack(StackName required; RetainResources, RoleARN, ClientRequestToken, DeletionMode, DeploymentConfig) has an EMPTY output and models exactly one error, TokenAlreadyExistsException (errorKind client, httpError 400, awsQueryError code `TokenAlreadyExistsException`); the only other refusal channel is the service-common ValidationError at HTTP 400. DeleteStack is documented as "Deletes a specified stack. Once the call completes successfully, stack deletion starts", and ListImports is documented as the read a caller uses to see "which stacks are using" an exported output value before it is modified or removed. Recorded behavior for this scenario: deleting the exporter while a live stack imports its export does NOT end in DELETE_COMPLETE; the observable outcome a client can poll is a TERMINAL one — either the call is refused synchronously (ValidationError, 400, whose message names the export and the importing stack) or the stack lands in the terminal stack status DELETE_FAILED whose ResourceStatusReason names the export and the importing stack. In the recorded form the fixed text is "Export " ... " cannot be deleted as it is in use by " ... and its variable parts are the export name from the exporting stack's Outputs[*].Export.Name and the name of the importing stack, both taken from state — never from any one run. Either way a client that waits on the deletion gets an answer: a 4xx, or a terminal status that standard SDK/CLI waiters (stack-delete-complete) treat as a failure state and stop on.

Emulator: the call is accepted with 200 and an empty body, the stack reports DELETE_IN_PROGRESS ("User Initiated"), and then the attempt is CANCELLED: the stack is restored to the status it settles to — CREATE_COMPLETE for a freshly created stack — carrying StackStatusReason `Delete canceled. Cannot delete export <export-name> as it is in use by <importing-stack-name>.` (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/delete_stack.rs calls state::export_in_use and then StoredStack::begin_canceled_deletion with state::delete_canceled_export_in_use_reason, which restores StoredStack::settled_status). No DELETE_FAILED is ever published and no refusal is returned. Replaying the check, the scenario therefore never converges: after ListImports correctly names the importer, the check polls the exporter for a terminal delete status and the emulator publishes none, so the check is cut off by the harness ('Error: Check timed out after 60.0s').

Why: a client cannot learn the outcome of a deletion it was told had started. DeleteStack answered success, so the client waits — `aws cloudformation wait stack-delete-complete` and every SDK waiter poll DescribeStacks for a terminal status and treat DELETE_FAILED as the stop-with-failure signal; against this emulator they never see one and hang indefinitely, while against AWS they either fail fast on the 400 or stop on DELETE_FAILED. The refusal is also unobservable to anything that keys on status rather than on prose: a stack sitting at CREATE_COMPLETE is indistinguishable from a stack nobody ever asked to delete, so an orchestration that retries or escalates on DELETE_FAILED never fires, and the emulator-authored sentence "Delete canceled. Cannot delete export X as it is in use by Y." does not match AWS's wording for this class either (AWS's fixed text is "Export <name> cannot be deleted as it is in use by <stack>"), so an error-matching layer keyed on AWS's sentence never matches. Hypothesis: the handler models the in-use export as a deletion CloudFormation declines and rolls back, rather than as a deletion that FAILS, which is why it reuses the pre-delete status instead of the terminal failed one.

Fix: the export-in-use branch of the DeleteStack handler and the deletion state machine it drives — the cancelled-deletion path in the cloudformation crate's stack state (the branch that currently restores the settled status) must instead land the stack in the terminal DELETE_FAILED status, leave its resources intact, and record the reason on the stack-level event in AWS's wording, OR refuse the call up front with the service-common ValidationError. Either shape converges; the current accept-then-revert shape does not.

Traces: cloudformation.export-in-use-blocks-stack-deletion. Related open issues that do NOT cover this: 20261025T133035Z-cloudformation-listimports-stub.md carries this check's replay key but describes ListImports as a 501 scaffold stub — ListImports is implemented now (it answers 200 and correctly names the importer in this replay), so that issue's cause is not this one; 20261025T133035Z-cloudformation-7-net-new-root-causes-cli-deploy.md (Cause 4) covers Fn::ImportValue not reaching a resource property, and 20261026T235900Z-cloudformation-7-net-new-root-causes-in-the.md (Cause 7) covers the missing in-place AWS::SNS::Topic update — neither touches the deletion outcome of an in-use export.

## Evidence
Failing check cloudformation.export-in-use-blocks-stack-deletion, verdict [mismatch]: 'Error: Check timed out after 60.0s'.

Sequence replayed (all steps before the delete succeeded):
  1. CreateStack <exporter> with {"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{}}},"Outputs":{"ExportedValue":{"Value":"<v>","Export":{"Name":<export>}}}} -> CREATE_COMPLETE; ListExports shows <export> with value <v>.
  2. CreateStack <importer> whose AWS::SNS::Topic carries Tags=[{"Key":"imported","Value":{"Fn::ImportValue":<export>}}] -> CREATE_COMPLETE.
  3. ListImports(ExportName=<export>) -> names <importer> (so the emulator's cross-stack index is populated and the in-use condition is detected).
  4. DeleteStack(StackName=<exporter>) -> HTTP 200, empty body, no exception raised.

Expected (AWS): either step 4 raises ValidationError (400) whose Message names the export and <importer> and the exporter stays CREATE_COMPLETE, or the exporter reaches the TERMINAL status DELETE_FAILED with a reason naming the export and <importer>. Then, after <importer> is deleted, DeleteStack(<exporter>) reaches DELETE_COMPLETE and <export> is gone from ListExports.

Actual (emulator): step 4 returns 200; the exporter reports DELETE_IN_PROGRESS and is then put back to CREATE_COMPLETE with StackStatusReason 'Delete canceled. Cannot delete export <export> as it is in use by <importer>.'. Polling DescribeStacks for DELETE_FAILED or DELETE_COMPLETE never terminates, and the check is killed by the harness at 60.0s before it can delete the importer, re-delete the exporter, or verify the export left ListExports.

Source of the emulator behavior: services/service-com-amazonaws-cloudformation-cloudformation/src/ops/delete_stack.rs (export-in-use branch -> StoredStack::begin_canceled_deletion), services/service-com-amazonaws-cloudformation-cloudformation/src/state.rs (export_in_use, begin_canceled_deletion / settled_status, delete_canceled_export_in_use_reason).

Smithy consulted: DeleteStack (input StackName required, RetainResources/RoleARN/ClientRequestToken/DeletionMode/DeploymentConfig; output Unit; errors: TokenAlreadyExistsException client/400/awsQueryError TokenAlreadyExistsException); ListImports (ExportName required, output Imports, paginated inputToken/outputToken NextToken, items Imports, NO modeled errors).

## Steps to Reproduce
1. Boot the emulator and point an AWS SDK/CLI client at its cloudformation endpoint.
2. CreateStack 'exporter' with TemplateBody {"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{}}},"Outputs":{"ExportedValue":{"Value":"v1","Export":{"Name":"my-export"}}}} and poll DescribeStacks until CREATE_COMPLETE.
3. CreateStack 'importer' with TemplateBody {"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"Tags":[{"Key":"imported","Value":{"Fn::ImportValue":"my-export"}}]}}}} and poll until CREATE_COMPLETE.
4. ListImports(ExportName='my-export') and confirm it names 'importer'.
5. DeleteStack(StackName='exporter'). Observe HTTP 200 with no error.
6. Poll DescribeStacks(StackName='exporter') for a terminal status, or run `aws cloudformation wait stack-delete-complete --stack-name exporter`. Observe the stack goes DELETE_IN_PROGRESS and then returns to CREATE_COMPLETE with StackStatusReason 'Delete canceled. Cannot delete export my-export as it is in use by importer.'; no DELETE_FAILED or DELETE_COMPLETE is ever published and the waiter never returns.
7. Expected instead: step 5 is refused with 400 ValidationError naming the export and the importing stack (exporter unchanged at CREATE_COMPLETE), or step 6 observes the terminal status DELETE_FAILED with a reason naming the export and the importing stack. After DeleteStack('importer') completes, DeleteStack('exporter') must then reach DELETE_COMPLETE and 'my-export' must disappear from ListExports.
