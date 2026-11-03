Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm.intrinsic-deletion-policy-if-honoured-on-delete cloudformation__ssm.intrinsic-deletion-policy-if-resolves-without-transform cloudformation__ssm.intrinsic-novalue-on-update-does-not-clear-ssm-property cloudformation__ssm.stack-update-retain-except-on-create-deletes-new-retained-resource cloudformation__ssm.stack-create-stack-policy-body-denies-modify-until-overridden cloudformation__ssm.stack-replacement-update-cleanup-deletes-old-physical-resource cloudformation__ssm.parameter-ssm-list-resolved-value-until-update cloudformation__ssm.stack-create-disable-validation-defers-name-conflict-to-provisioning cloudformation__ssm.cdk-deploy-parameters-are-reused-until-no-previous-parameters
Issue-Key: cloudformation__ssm.intrinsic-deletion-policy-if-honoured-on-delete|run#12|AssertionError|assert policies == {"Kept": "Retain", "Dropped": "Delete"}, (
Issue-Key: cloudformation__ssm.intrinsic-deletion-policy-if-resolves-without-transform|run#18|AssertionError|assert kept == "v", (
Issue-Key: cloudformation__ssm.intrinsic-novalue-on-update-does-not-clear-ssm-property|run#14|AssertionError|assert _described(ssm, name) == {"Description": _DESCRIPTION, "AllowedPattern": _PATTERN}, (
Issue-Key: cloudformation__ssm.stack-update-retain-except-on-create-deletes-new-retained-resource|run#18|AssertionError|assert kept_rows == ["CREATE_IN_PROGRESS", "CREATE_IN_PROGRESS", "CREATE_COMPLETE", "DELETE_SKIPPED"], (
Issue-Key: cloudformation__ssm.stack-create-stack-policy-body-denies-modify-until-overridden|run#15|AssertionError|assert denied == [("UPDATE_FAILED", "Action denied by stack policy: Statement [#1] does not allow "
Issue-Key: cloudformation__ssm.stack-replacement-update-cleanup-deletes-old-physical-resource|run#11|AssertionError|assert [r[:3] for r in rows] == [
Issue-Key: cloudformation__ssm.parameter-ssm-list-resolved-value-until-update|_view#10|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__ssm.stack-create-disable-validation-defers-name-conflict-to-provisioning|run#7|AssertionError|assert _of(_rows(_events(cfn, validated)), "Param") == [], (
Issue-Key: cloudformation__ssm.cdk-deploy-parameters-are-reused-until-no-previous-parameters|run#11|AssertionError|assert declared == {"Greeting": greeting}, (
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# cloudformation__ssm provisioning and update path: Fn::If DeletionPolicy left unresolved, AWS::NoValue wipes live properties, Retain ignored on rollback, wrong stack-policy denial events, wrong replacement event stream, Outputs without OutputValue, no pre-deployment validation, and stack Parameters lost (8 causes)

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Eight distinct net-new root causes in the emulator's CloudFormation provisioning/update path, exercised through AWS::SSM::Parameter resources. Two of them destroy data a client told CloudFormation to keep. They are independent defects and must each be closed; the numbering below is the count a fixer must satisfy.

=== Cause 1 of 8: a DeletionPolicy written as Fn::If is never resolved, so a resource the condition retains is deleted ===
AWS: documented in the DeletionPolicy attribute reference and the AWS::LanguageExtensions transform reference. A DeletionPolicy given as {"Fn::If": [<condition>, "Retain", "Delete"]} is accepted by CreateStack with or without the AWS::LanguageExtensions transform and resolves per stack. GetTemplate(TemplateStage=Processed) carries the resolved literal (`"DeletionPolicy": "Retain"` / `"Delete"`), and DeleteStack then emits DELETE_SKIPPED as the last event of the resource resolved to Retain (which survives with its value) and DELETE_COMPLETE for the one resolved to Delete (which is removed). Declaring the transform changes neither acceptance nor the resolved policy.
Emulator: the processed template still carries the intrinsic verbatim — got {'Dropped': {'Fn::If': ['Keep', 'Delete', 'Retain']}, 'Kept': {'Fn::If': ['Keep', 'Retain', 'Delete']}} — and DeleteStack deletes the resource that resolves to Retain (the parameter is gone afterwards; the probe read None).
Why: DATA LOSS. A client that protects a resource with a conditional DeletionPolicy has it destroyed on DeleteStack, with no refusal and no DELETE_SKIPPED event to warn them.
Fix: the template processing stage (intrinsic resolution over resource attributes, not just Properties) and the delete path's DeletionPolicy evaluation; the GetTemplate(Processed) serializer must emit the resolved value.
Traces: cloudformation__ssm.intrinsic-deletion-policy-if-honoured-on-delete, cloudformation__ssm.intrinsic-deletion-policy-if-resolves-without-transform

=== Cause 2 of 8: a property that resolves to AWS::NoValue on update clears the live resource instead of being omitted ===
AWS: documented for the pseudo parameter AWS::NoValue — a property whose value resolves to {"Ref": "AWS::NoValue"} is REMOVED from the template, so the update request simply does not carry that property and the live resource keeps whatever it was created with.
Emulator: turning Description and AllowedPattern into AWS::NoValue on an UpdateStack clears them on the live parameter — the read-back reports {'Description': None, 'AllowedPattern': None} instead of the values set at create time.
Why: DATA LOSS on a conditional template. Any template that drops optional properties behind a condition silently wipes those settings on the live resource on every update where the condition is false.
Fix: the UpdateStack handler's property-diff stage — a property resolving to AWS::NoValue must be omitted from the resource update, not sent as null.
Traces: cloudformation__ssm.intrinsic-novalue-on-update-does-not-clear-ssm-property

=== Cause 3 of 8: update rollback deletes a newly created DeletionPolicy:Retain resource instead of skipping it ===
AWS: Smithy: UpdateStack.RetainExceptOnCreate (boolean). On a failed update, rollback honours DeletionPolicy Retain for a resource the update newly created UNLESS RetainExceptOnCreate=true. Without the flag the resource's events are CREATE_IN_PROGRESS, CREATE_IN_PROGRESS, CREATE_COMPLETE, DELETE_SKIPPED and the resource survives with its value; with the flag they are CREATE_IN_PROGRESS, CREATE_IN_PROGRESS, CREATE_COMPLETE, DELETE_IN_PROGRESS, DELETE_COMPLETE and the resource is deleted.
Emulator: without the flag the rollback deletes it anyway — events ['CREATE_IN_PROGRESS', 'CREATE_IN_PROGRESS', 'CREATE_COMPLETE', 'DELETE_IN_PROGRESS', 'DELETE_COMPLETE'], no DELETE_SKIPPED. (The RetainExceptOnCreate=true leg behaves correctly, so the flag is simply never consulted — Retain is ignored on rollback.)
Why: DATA LOSS. The default behaviour and the opt-in behaviour are identical, so a client cannot keep a retained resource across a failed update, and RetainExceptOnCreate has no observable effect.
Fix: the update-rollback stage's DeletionPolicy/RetainExceptOnCreate decision and the DELETE_SKIPPED event it must record.
Traces: cloudformation__ssm.stack-update-retain-except-on-create-deletes-new-retained-resource

=== Cause 4 of 8: the stack-policy denial event numbers statements from 0 and the rollback skip event is missing ===
AWS: documented in "Protect CloudFormation stack resources". A StackPolicyBody given to CreateStack is stored (GetStackPolicy returns it verbatim) and enforced. An update that modifies a denied resource produces exactly two events for that resource: UPDATE_FAILED with ResourceStatusReason `Action denied by stack policy: Statement [#1] does not allow [Update:Modify] for resource [LogicalResourceId/Protected];` — fixed text `Action denied by stack policy: Statement [#`, `] does not allow [`, `] for resource [`, `];`, with the statement NUMBER (statements are numbered from 1, so the first statement of the document is #1), the denied action and the resource pattern taken from the stack policy document — followed by UPDATE_COMPLETE with ResourceStatusReason `Resource skipped during UpdateRollback`; the stack ends UPDATE_ROLLBACK_COMPLETE and the resource's live value is untouched.
Emulator: the reason numbers the first statement `Statement [#0]` instead of `[#1]`, and only one event is emitted — the trailing ('UPDATE_COMPLETE', 'Resource skipped during UpdateRollback') event is absent.
Why: a client parsing the denial reason to report which policy statement blocked the update is told the wrong statement, and a client waiting for the resource's terminal rollback event never sees it.
Fix: the stack-policy evaluation in the UpdateStack handler (statement indexing in the reason) and the update-rollback stage's per-resource skip event.
Traces: cloudformation__ssm.stack-create-stack-policy-body-denies-modify-until-overridden

=== Cause 5 of 8: a replacing update emits CREATE_* events with no replacement reason and no cleanup-phase stack event ===
AWS: documented in "Update behaviors of stack resources". Changing a create-only property replaces the resource, and the event stream of the update is exactly: (<stack>, UPDATE_IN_PROGRESS, 'User Initiated'), (<logical id>, UPDATE_IN_PROGRESS, 'Requested update requires the creation of a new physical resource; hence creating one.'), (<logical id>, UPDATE_IN_PROGRESS, 'Resource creation Initiated'), (<logical id>, UPDATE_COMPLETE, None) carrying the NEW physical id, (<stack>, UPDATE_COMPLETE_CLEANUP_IN_PROGRESS, None), (<logical id>, DELETE_IN_PROGRESS, None) carrying the OLD physical id, (<logical id>, DELETE_COMPLETE, None) carrying the OLD physical id, (<stack>, UPDATE_COMPLETE, None).
Emulator: the replaced resource reports CREATE_IN_PROGRESS / CREATE_IN_PROGRESS('Resource creation Initiated') / CREATE_COMPLETE instead of UPDATE_* with the replacement reason, the first event carries an empty PhysicalResourceId, DELETE_IN_PROGRESS carries PhysicalResourceId None instead of the old id, and the stack-level UPDATE_COMPLETE_CLEANUP_IN_PROGRESS event is never emitted.
Why: clients and the CDK CLI drive progress display and replacement detection off exactly these statuses and reasons; a replacement is indistinguishable from a fresh create, and the cleanup phase is invisible.
Fix: the UpdateStack handler's replacement path and the stack-event recorder (statuses, ResourceStatusReason and PhysicalResourceId per phase).
Traces: cloudformation__ssm.stack-replacement-update-cleanup-deletes-old-physical-resource

=== Cause 6 of 8: DescribeStacks Outputs entries omit OutputValue when the output resolves SSM-typed list parameters ===
AWS: Smithy: DescribeStacks output Stacks[].Outputs[] is a list of Output{OutputKey, OutputValue, Description, ExportName}. A resolved output always carries OutputValue — here an output built with Fn::Select / Fn::Join over a parameter of type AWS::SSM::Parameter::Value<List<String>> and over a CommaDelimitedList.
Emulator: the Outputs entries are returned with no OutputValue member at all, so reading the output raises KeyError: 'OutputValue'.
Why: the stack's outputs cannot be read back, so any consumer of a resolved output (including a cross-stack or parameter-driven value) fails.
Fix: the DescribeStacks output serializer / output-resolution stage. Several open issues record this same omission under other service headings (20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md, 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md); none carries this check's replay key, and this one additionally covers SSM-typed list parameter resolution.
Traces: cloudformation__ssm.parameter-ssm-list-resolved-value-until-update

=== Cause 7 of 8: CreateStack runs no pre-deployment validation and DescribeEvents reports no VALIDATION_ERROR operation events ===
AWS: Smithy: CreateStack.DisableValidation (boolean); DescribeEvents returns OperationEvents of OperationEvent, whose members include EventType, ValidationName, ValidationStatus (enum FAILED|SKIPPED), ValidationFailureMode and ValidationStatusReason. Documented in "Validate stack deployments". A CreateStack whose template names an existing resource is stopped by pre-deployment validation BEFORE provisioning: the stack reaches ROLLBACK_COMPLETE with NO resource events for that logical id, and DescribeEvents reports one event with EventType VALIDATION_ERROR and (LogicalResourceId, ResourceType, ValidationName, ValidationStatus, ValidationFailureMode, ValidationStatusReason) = (<logical id>, 'AWS::SSM::Parameter', 'NAME_CONFLICT_VALIDATION', 'FAILED', 'FAIL', "Resource of type 'AWS::SSM::Parameter' with identifier '<name>' already exists."). With DisableValidation=true the handler is allowed to attempt it and the resource's CREATE_FAILED reason is `Resource handler returned message: "Resource of type 'AWS::SSM::Parameter' with identifier '<name>' already exists." (RequestToken: <uuid>, HandlerErrorCode: AlreadyExists)` — fixed text around the resource type name and the identifier, which come from the request; the RequestToken is per-run. Neither create may touch the existing resource.
Emulator: validation never runs — the validated create provisions the resource and records ('Param','CREATE_IN_PROGRESS'), ('Param','CREATE_FAILED', 'Resource handler returned message: "Parameter <name> already exists; set Overwrite to update it" (RequestToken: <uuid>, HandlerErrorCode: AlreadyExists)'), then rolls back; DescribeEvents reports no VALIDATION_ERROR event. The handler message text is also wrong: it must be `Resource of type 'AWS::SSM::Parameter' with identifier '<name>' already exists.`, not `Parameter <name> already exists; set Overwrite to update it`.
Why: a conflicting create is only discovered after CloudFormation has started mutating the account, and DisableValidation has no observable effect; clients that read DescribeEvents VALIDATION_ERROR to explain a pre-flight failure get nothing.
Fix: the CreateStack handler's pre-deployment validation stage, the DescribeEvents OperationEvents serializer, and the AWS::SSM::Parameter resource handler's AlreadyExists message. Related open issue 20261102T101202Z-cloudformation-10-net-new-root-causes-stack-level.md records the update-side DisableValidation divergence under the `cloudformation` heading but does not carry this check.
Traces: cloudformation__ssm.stack-create-disable-validation-defers-name-conflict-to-provisioning

=== Cause 8 of 8: after an update that supplies no Parameters, DescribeStacks reports an empty Parameters list instead of the effective defaults ===
AWS: Smithy: DescribeStacks output Stacks[].Parameters[] is a list of Parameter{ParameterKey, ParameterValue, UsePreviousValue, ResolvedValue}. Every parameter the template declares is reported with its EFFECTIVE value: when an update supplies no value and no previous value is reused (`cdk deploy --no-previous-parameters`), the template default is the effective value and is reported, e.g. ParameterKey `Greeting` with the declared default as ParameterValue.
Emulator: DescribeStacks returns an empty Parameters list ({}) for that stack, although the deploy succeeded and the resources were provisioned from the default.
Why: a client reading a stack's parameters to decide what to re-submit — exactly what the CDK CLI does to reuse or reset parameters — sees no parameters at all and loses the stack's configuration.
Fix: the UpdateStack handler's parameter-resolution stage (record declared parameters with their effective value) and the DescribeStacks serializer.
Traces: cloudformation__ssm.cdk-deploy-parameters-are-reused-until-no-previous-parameters

## Evidence
Cause 1 — cloudformation__ssm.intrinsic-deletion-policy-if-honoured-on-delete: expected the processed template's resolved policies {'Kept': 'Retain', 'Dropped': 'Delete'}; got AssertionError: the processed template must carry the resolved policies; got {'Dropped': OrderedDict({'Fn::If': ['Keep', 'Delete', 'Retain']}), 'Kept': OrderedDict({'Fn::If': ['Keep', 'Retain', 'Delete']})}. cloudformation__ssm.intrinsic-deletion-policy-if-resolves-without-transform: expected the Retain-resolved parameter to survive DeleteStack with value 'v'; got AssertionError: Mode=keep must resolve Fn::If to Retain and survive DeleteStack with its original value; got None.

Cause 2 — cloudformation__ssm.intrinsic-novalue-on-update-does-not-clear-ssm-property: expected {'Description': <create-time description>, 'AllowedPattern': <create-time pattern>}; got AssertionError: turning both properties into AWS::NoValue must not clear the live parameter: it must still carry the values set at create time; got {'Description': None, 'AllowedPattern': None}.

Cause 3 — cloudformation__ssm.stack-update-retain-except-on-create-deletes-new-retained-resource: expected ['CREATE_IN_PROGRESS', 'CREATE_IN_PROGRESS', 'CREATE_COMPLETE', 'DELETE_SKIPPED']; got AssertionError: Kept without the flag: ['CREATE_IN_PROGRESS', 'CREATE_IN_PROGRESS', 'CREATE_COMPLETE', 'DELETE_IN_PROGRESS', 'DELETE_COMPLETE'].

Cause 4 — cloudformation__ssm.stack-create-stack-policy-body-denies-modify-until-overridden: expected [('UPDATE_FAILED', 'Action denied by stack policy: Statement [#1] does not allow [Update:Modify] for resource [LogicalResourceId/Protected];'), ('UPDATE_COMPLETE', 'Resource skipped during UpdateRollback')]; got AssertionError: events of the denied resource: [('UPDATE_IN_PROGRESS', None), ('UPDATE_FAILED', 'Action denied by stack policy: Statement [#0] does not allow [Update:Modify] for resource [LogicalResourceId/Protected];')].

Cause 5 — cloudformation__ssm.stack-replacement-update-cleanup-deletes-old-physical-resource: expected [(<stack>,'UPDATE_IN_PROGRESS','User Initiated'), ('Param','UPDATE_IN_PROGRESS','Requested update requires the creation of a new physical resource; hence creating one.'), ('Param','UPDATE_IN_PROGRESS','Resource creation Initiated'), ('Param','UPDATE_COMPLETE',None), (<stack>,'UPDATE_COMPLETE_CLEANUP_IN_PROGRESS',None), ('Param','DELETE_IN_PROGRESS',None), ('Param','DELETE_COMPLETE',None), (<stack>,'UPDATE_COMPLETE',None)]; got [(<stack>,'UPDATE_IN_PROGRESS','User Initiated',<arn>), ('Param','CREATE_IN_PROGRESS',None,''), ('Param','CREATE_IN_PROGRESS','Resource creation Initiated',<new id>), ('Param','CREATE_COMPLETE',None,<new id>), ('Param','DELETE_IN_PROGRESS',None,None), ('Param','DELETE_COMPLETE',None,<old id>), (<stack>,'UPDATE_COMPLETE',None,<arn>)].

Cause 6 — cloudformation__ssm.parameter-ssm-list-resolved-value-until-update: reading DescribeStacks Stacks[0].Outputs raised KeyError: 'OutputValue'.

Cause 7 — cloudformation__ssm.stack-create-disable-validation-defers-name-conflict-to-provisioning: expected no Param resource events for the validated create plus one DescribeEvents VALIDATION_ERROR event ('Param','AWS::SSM::Parameter','NAME_CONFLICT_VALIDATION','FAILED','FAIL',"Resource of type 'AWS::SSM::Parameter' with identifier '<name>' already exists."); got AssertionError: validation must stop the create before Param is provisioned: [(<stack>,'CREATE_IN_PROGRESS','User Initiated'), ('Param','CREATE_IN_PROGRESS',None), ('Param','CREATE_FAILED','Resource handler returned message: "Parameter <name> already exists; set Overwrite to update it" (RequestToken: <uuid>, HandlerErrorCode: AlreadyExists)'), (<stack>,'CREATE_FAILED','The following resource(s) failed to create: [Param]. '), (<stack>,'ROLLBACK_IN_PROGRESS',...), (<stack>,'ROLLBACK_COMPLETE',...)].

Cause 8 — cloudformation__ssm.cdk-deploy-parameters-are-reused-until-no-previous-parameters: expected {'Greeting': '<template default>'} after the --no-previous-parameters deploy; got AssertionError: after deploy 3 ['--no-previous-parameters'] the stack's parameters are {}.

## Steps to Reproduce
Replay the behavioral suite and re-run the listed checks against the emulator:

1. Cause 1: CreateStack a template (with and without Transform: AWS::LanguageExtensions) whose AWS::SSM::Parameter resources carry DeletionPolicy {"Fn::If": ["Keep", "Retain", "Delete"]}; read GetTemplate(TemplateStage='Processed'), then DeleteStack and check the resources' last events and whether the retained parameter survives.
2. Cause 2: create a stack whose AWS::SSM::Parameter carries Description and AllowedPattern, then UpdateStack with both properties resolving to {"Ref": "AWS::NoValue"}; read the live parameter back.
3. Cause 3: create a stack, then UpdateStack with a template adding a DeletionPolicy:Retain AWS::SSM::Parameter plus a WaitCondition that times out — once with RetainExceptOnCreate=True, once without — and compare the new resource's events.
4. Cause 4: CreateStack with StackPolicyBody denying Update:Modify on one logical id, then UpdateStack modifying it; read the two events of the denied resource.
5. Cause 5: create a stack with a named AWS::SSM::Parameter, then UpdateStack changing the create-only Name; read the update's events with PhysicalResourceId.
6. Cause 6: create a stack with a parameter of type AWS::SSM::Parameter::Value<List<String>> and Outputs using Fn::Select/Fn::Join over it; read DescribeStacks Outputs.
7. Cause 7: PutParameter out of band, then CreateStack naming it (once plain, once with DisableValidation=True); read the stack events and DescribeEvents OperationEvents.
8. Cause 8: `cdk deploy --parameters <Stack>:Greeting=first`, then a plain `cdk deploy`, then `cdk deploy --no-previous-parameters`; read DescribeStacks Parameters after each.
