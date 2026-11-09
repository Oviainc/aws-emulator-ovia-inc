Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ssm.cfn-ssm-parameter-missing-required-properties-rejected cloudformation__ssm.cfn-ssm-parameter-getatt-unknown-attribute-rejected-rolls-back cloudformation__ssm.output-getatt-on-writeonly-ssm-parameter-property-rejected cloudformation__ssm.deploy-ssm-maintenancewindowtask-via-cloudformation-then-lifecycle cloudformation__ssm.deploy-ssm-opsitem-via-cloudformation-then-lifecycle cloudformation__ssm.deploy-ssm-resourcepolicy-via-cloudformation-then-lifecycle cloudformation__ssm.deploy-ssm-service-setting-unknown-setting-id-rejected-then-round-trip-and-reset cloudformation__ssm.deploy-ssmquicksetup-lifecycleautomation-missing-automation-document-rejected cloudformation__ssm.deploy-ssmquicksetup-lifecycleautomation-round-trip-then-update-and-delete cloudformation__ssm.deploy-ssm-document-invalid-content-rejected cloudformation__ssm.cfn-nested-three-level-grandchild-update-failure-rolls-back-every-level cloudformation__ssm.cdk-deploy-outputs-file-matches-describe-stacks-outputs-for-every-deployed-stack
Issue-Key: cloudformation__ssm.cfn-ssm-parameter-missing-required-properties-rejected|run#12|AssertionError|assert found == want, f"{label}: validation errors expected {want!r}, got {found!r}"
Issue-Key: cloudformation__ssm.cfn-ssm-parameter-getatt-unknown-attribute-rejected-rolls-back|run#15|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__ssm.output-getatt-on-writeonly-ssm-parameter-property-rejected|run#7|AssertionError|assert bad["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__ssm|CreateStack|ValidationError
Issue-Key: cloudformation__ssm.deploy-ssm-document-invalid-content-rejected|run#8|AssertionError|assert control_before["Status"] == "Active", (
Issue-Key: cloudformation__ssm.cfn-nested-three-level-grandchild-update-failure-rolls-back-every-level|run#30|AssertionError|assert root_after["StackStatus"] == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__ssm.cdk-deploy-outputs-file-matches-describe-stacks-outputs-for-every-deployed-stack|run#30|AssertionError|assert live_values["ParamValue"] == value_a2, (
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# cloudformation+ssm: 5 net-new causes — no resource-schema property validation, five AWS::SSM*/SSMQuickSetup resource types unrecognized, SSM documents stuck Creating, nested-stack failures not rolled back to the root, and a stack update that does not apply the new parameter value

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five net-new root causes, each grounded in its own failing checks under this heading.

Cause 1 of 5: CloudFormation performs no resource-schema property validation on AWS::SSM::Parameter — a missing required property, an unknown GetAtt attribute and a GetAtt on a write-only property are all accepted and the stack reaches CREATE_COMPLETE.
AWS: CreateStack validates properties and Fn::GetAtt targets against the registered resource schema before provisioning. A missing required property fails with a PROPERTY_VALIDATION row ('<LogicalId>','PROPERTY_VALIDATION','FAILED','FAIL','Required property [<Name>] not found','/Resources/<LogicalId>/Properties'); an unknown attribute fails with reason `Requested attribute <Attr> does not exist in schema for AWS::SSM::Parameter`; a GetAtt naming a write-only property (Value/Description/AllowedPattern/Tier on AWS::SSM::Parameter) is likewise refused. In each case the stack ends ROLLBACK_COMPLETE. Fixed text: `Required property [` ... `] not found`, `Requested attribute ` ... ` does not exist in schema for ` ... ; variable parts are the property/attribute name and the resource type from the template. Recorded behavior only for the validation-row layout.
Emulator: all three templates reach `CREATE_COMPLETE`; DescribeStackEvents carries no validation rows (`validation errors expected [...], got []`).
Why: a client loses CloudFormation's pre-provision contract check — templates with typos, missing required properties or unreadable attributes deploy "successfully" and produce resources and outputs that do not match the template, with no event explaining why.
Fix: resource-schema validation in the CloudFormation provisioning path (required-property check, GetAtt attribute resolution against the schema, write-only-property guard) and the PROPERTY_VALIDATION stack events it must emit.
Traces: cfn-ssm-parameter-missing-required-properties-rejected, cfn-ssm-parameter-getatt-unknown-attribute-rejected-rolls-back, output-getatt-on-writeonly-ssm-parameter-property-rejected.

Cause 2 of 5: CreateStack refuses five SSM/SSMQuickSetup resource types as unrecognized, so no template declaring them can be deployed.
AWS: AWS::SSM::MaintenanceWindowTarget, AWS::SSM::MaintenanceWindowTask, AWS::SSM::OpsItem, AWS::SSM::ResourcePolicy, AWS::SSM::ServiceSetting and AWS::SSMQuickSetup::LifecycleAutomation are all registered CloudFormation resource types; CreateStack accepts them and provisions the corresponding SSM resource (which is then readable through the SSM API). `Template format error: Unrecognized resource types: [...]` (400 ValidationError) is AWS's refusal for a type that does not exist at all — fixed text `Template format error: Unrecognized resource types: [` ... `]`, with the bracketed list taken from the template.
Emulator: CreateStack answers 400 `ValidationError: Template format error: Unrecognized resource types: [AWS::SSM::MaintenanceWindowTarget, AWS::SSM::MaintenanceWindowTask]`, `[AWS::SSM::OpsItem]`, `[AWS::SSM::ResourcePolicy]`, `[AWS::SSM::ServiceSetting]`, `[AWS::SSMQuickSetup::LifecycleAutomation]` respectively.
Why: a client cannot manage maintenance windows, OpsItems, parameter resource policies, service settings or Quick Setup lifecycle automations as infrastructure-as-code at all — the stack is rejected at submission, so there is no partial path and no workaround short of imperative API calls.
Fix: the CloudFormation resource-type registry and the per-type resource handlers for these five types.
Traces: deploy-ssm-maintenancewindowtask-via-cloudformation-then-lifecycle, deploy-ssm-opsitem-via-cloudformation-then-lifecycle, deploy-ssm-resourcepolicy-via-cloudformation-then-lifecycle, deploy-ssm-service-setting-unknown-setting-id-rejected-then-round-trip-and-reset, deploy-ssmquicksetup-lifecycleautomation-missing-automation-document-rejected, deploy-ssmquicksetup-lifecycleautomation-round-trip-then-update-and-delete.

Cause 3 of 5: an AWS::SSM::Document created by a CloudFormation stack never leaves Status 'Creating'.
AWS: CreateDocument puts the document in Creating and it becomes Active within seconds; a stack that reaches CREATE_COMPLETE has an Active document, and DescribeDocument/GetDocument report Status `Active`. Smithy (ssm): DescribeDocument output DocumentDescription.Status -> DocumentStatus enum (Creating|Active|Updating|Deleting|Failed); Active is the terminal success state.
Emulator: the control document is still `Creating` when the stack is complete, so the document never becomes usable and the negative case it controls for cannot be evaluated.
Why: a client that waits for Active (the documented readiness gate before SendCommand/StartAutomationExecution) waits forever, and any stack that creates a document and then uses it cannot proceed.
Fix: the AWS::SSM::Document resource handler / SSM document state machine — the document must transition Creating -> Active before the resource reports complete.
Traces: deploy-ssm-document-invalid-content-rejected.

Cause 4 of 5: a failed update inside a third-level nested stack does not roll the root stack back — the root reports UPDATE_COMPLETE.
AWS: a nested stack's failure propagates: the grandchild goes UPDATE_FAILED/UPDATE_ROLLBACK_COMPLETE, and every ancestor including the root ends `UPDATE_ROLLBACK_COMPLETE`. Recorded behavior only for the per-level status sequence; the rule that a nested-stack failure fails its parent is CloudFormation's documented nested-stack behaviour.
Emulator: the root ends `UPDATE_COMPLETE` although the grandchild's update failed.
Why: a client reads the root status as the verdict for the whole tree (that is the point of nesting). A green root over a failed grandchild means a partially-updated estate is reported as a successful deploy, and the rollback that should have restored it never ran.
Fix: nested-stack update orchestration in the CloudFormation service — a child stack's terminal failure must fail and roll back its parent, recursively to the root.
Traces: cfn-nested-three-level-grandchild-update-failure-rolls-back-every-level.

Cause 5 of 5: a second `cdk deploy` that changes a stack parameter/property does not apply it — DescribeStacks still reports the first deploy's value.
AWS: an UpdateStack (or an executed CDK deploy) that changes an AWS::SSM::Parameter's Value updates the resource and the stack's Outputs; DescribeStacks Outputs and the SSM parameter itself both report the new value after the update completes. Smithy (cloudformation): DescribeStacks output Stacks[].Outputs[].OutputValue. Recorded behavior only for the CDK `--outputs-file` equality; the underlying requirement is that an applied update is reflected in the stack's outputs.
Emulator: after the second deploy the live `ParamValue` output is still the first deploy's value, so the outputs file and DescribeStacks disagree with what was deployed.
Why: a client cannot trust stack outputs after an update — downstream stacks and scripts that consume an output (or the `--outputs-file`) keep wiring the stale value, which silently points a whole estate at the previous configuration.
Fix: the UpdateStack apply path and the output re-resolution that follows it (the resource update must be applied to state and the stack's Outputs recomputed from the updated resources).
Traces: cdk-deploy-outputs-file-matches-describe-stacks-outputs-for-every-deployed-stack.

## Evidence
Cause 1:
- `AssertionError: missing Value: validation errors expected [('P','PROPERTY_VALIDATION','FAILED','FAIL','Required property [Value] not found','/Resources/P/Properties')], got []`
- `AssertionError: an unknown GetAtt attribute must fail the stack and roll it back; got 'CREATE_COMPLETE'` (expected reason `Requested attribute NoSuchAttr does not exist in schema for AWS::SSM::Parameter`)
- `AssertionError: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'` for Fn::GetAtt on a write-only property

Cause 2: `botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::SSM::MaintenanceWindowTarget, AWS::SSM::MaintenanceWindowTask]` — and the same refusal with `[AWS::SSM::OpsItem]`, `[AWS::SSM::ResourcePolicy]`, `[AWS::SSM::ServiceSetting]`, `[AWS::SSMQuickSetup::LifecycleAutomation]`. Expected: 200 CreateStack and provisioning.

Cause 3: `AssertionError: expected the control document to be Active, got 'Creating'`.

Cause 4: `AssertionError: the root must roll back to UPDATE_ROLLBACK_COMPLETE, got 'UPDATE_COMPLETE'`.

Cause 5: `AssertionError: stack '<name>-a''s live ParamValue output is 'va1-<id>', expected the second deploy's updated value 'va2-<id>'` — the values are per-run generated markers; the requirement is that the output equals the value the latest deploy applied.

## Steps to Reproduce
Cause 1:
 a) CreateStack {"Resources":{"P":{"Type":"AWS::SSM::Parameter","Properties":{"Name":...,"Type":"String"}}}} (no Value) — observe CREATE_COMPLETE and no PROPERTY_VALIDATION event.
 b) CreateStack a template with Outputs using Fn::GetAtt [Param, NoSuchAttr] — observe CREATE_COMPLETE.
 c) Same with Fn::GetAtt on a write-only property (e.g. Value/Description) — observe CREATE_COMPLETE.

Cause 2: CreateStack a template declaring each of AWS::SSM::MaintenanceWindowTarget, AWS::SSM::MaintenanceWindowTask, AWS::SSM::OpsItem, AWS::SSM::ResourcePolicy, AWS::SSM::ServiceSetting, AWS::SSMQuickSetup::LifecycleAutomation — each call raises 400 ValidationError "Template format error: Unrecognized resource types: [...]".

Cause 3: CreateStack an AWS::SSM::Document with valid content; after CREATE_COMPLETE call ssm DescribeDocument(Name=<doc>) — Status is 'Creating', never 'Active'.

Cause 4: Deploy a three-level nested stack; UpdateStack so the grandchild's resource update fails; poll DescribeStacks on the root — it reports UPDATE_COMPLETE instead of UPDATE_ROLLBACK_COMPLETE.

Cause 5: `cdk deploy --outputs-file` an app whose stack outputs an SSM parameter value; change the value and deploy again; DescribeStacks Outputs still carries the first value.
