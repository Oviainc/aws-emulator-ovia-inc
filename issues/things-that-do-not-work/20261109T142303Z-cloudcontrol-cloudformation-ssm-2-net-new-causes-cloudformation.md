Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__cloudformation__ssm.cfn-ssm-parameter-unknown-property-rejected-with-positive-control cloudcontrol__cloudformation__ssm.ssm-parameter-writeonly-properties-update-hidden-from-cloudcontrol
Issue-Key: cloudcontrol__cloudformation__ssm.cfn-ssm-parameter-unknown-property-rejected-with-positive-control|run#9|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", f"expected ROLLBACK_COMPLETE, got {bad_status!r}"
Issue-Key: cloudcontrol__cloudformation__ssm|GetResource|ResourceNotFoundException
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# cloudcontrol+cloudformation+ssm: 2 net-new causes — CloudFormation accepts an unsupported resource property instead of failing schema validation, and Cloud Control GetResource reports a CFN-managed SSM parameter as NotFound

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes, both grounded in failing checks under this heading.

Cause 1 of 2: CloudFormation performs no resource-schema property validation, so a template property the resource type does not declare is accepted and the stack reaches CREATE_COMPLETE.
AWS: CreateStack validates each resource's properties against the registered resource schema before provisioning. An undeclared property fails the resource with the schema-validation reason, the stack-level CREATE_FAILED reason is `Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.`, DescribeStackEvents carries a PROPERTY_VALIDATION operation row (status FAILED, mode FAIL) whose reason is `Unsupported property [<PropertyName>]` at path `/Resources/<LogicalId>/Properties/<PropertyName>`, and the stack ends ROLLBACK_COMPLETE. Fixed text: `Validation failed with 1 error(s). Call DescribeEvents...`, `Unsupported property [` ... `]`, and the literal `PROPERTY_VALIDATION` / `FAILED` / `FAIL` row fields; the variable parts are the logical id, the property name and the template path, all taken from the request. Recorded behavior only for the row layout (the Smithy model does not model validation rows).
Emulator: the stack with an undeclared AWS::SSM::Parameter property reaches `CREATE_COMPLETE`; no PROPERTY_VALIDATION event is emitted and no rollback occurs.
Why: a client that relies on CloudFormation to catch typos and unsupported properties gets a stack that "succeeded" while silently dropping the property, so the deployed resource does not match the template and the error surfaces only much later (or never).
Fix: template/resource-schema validation in the CloudFormation provisioning path (the per-resource-type schema check that runs before the handler), plus the PROPERTY_VALIDATION stack-event rows it must record.
Traces: cloudcontrol__cloudformation__ssm.cfn-ssm-parameter-unknown-property-rejected-with-positive-control.

Cause 2 of 2: Cloud Control GetResource answers ResourceNotFoundException for an AWS::SSM::Parameter that exists and is managed by a CloudFormation stack.
AWS: Cloud Control's GetResource reads the live resource through the resource type's READ handler, so a parameter that exists in SSM (whatever created it, and after a stack update that changes only write-only properties such as Description/AllowedPattern/Tier) is returned with its Identifier and Properties. ResourceNotFoundException is reserved for an identifier that does not exist. Smithy (cloudcontrol): GetResource errors include ResourceNotFoundException and HandlerFailureException; the handler status FAILED/NotFound path is the not-found path only.
Emulator: GetResource answers `ResourceNotFoundException`: `AWS::SSM::Parameter Handler returned status FAILED: Resource of type 'AWS::SSM::Parameter' with identifier '<parameter-name>' was not found. (HandlerErrorCode: NotFound, RequestToken: <uuid>)` for a parameter the same emulator still serves through SSM. The fixed text here is AWS's not-found wording; the defect is that it is returned at all — the variable parts (type name, identifier) come from the request and the RequestToken is a per-run value.
Why: a client using Cloud Control as the uniform read API cannot see resources it (or CloudFormation) just created, so drift detection, write-only-property verification and any Cloud-Control-based inventory report resources as deleted.
Fix: the Cloud Control GetResource read path for AWS::SSM::Parameter — the READ handler must look the parameter up in SSM's parameter state rather than in a Cloud-Control-local registry of resources it created itself.
Traces: cloudcontrol__cloudformation__ssm.ssm-parameter-writeonly-properties-update-hidden-from-cloudcontrol.

Related but not covering: open issue 20261104T172234Z-cloudcontrol-desiredstate-tags-are-never-written-to-the.md concerns Cloud Control CreateResource tags, not the read path or CFN property validation.

## Evidence
Cause 1 — expected: stack status `ROLLBACK_COMPLETE`; stack-level CREATE_FAILED reason `Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.`; a validation row ('Param','PROPERTY_VALIDATION','FAILED','FAIL','Unsupported property [ThisPropertyDoesNotExist]','/Resources/Param/Properties/ThisPropertyDoesNotExist').
Actual: `AssertionError: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'` — no validation row, no rollback.

Cause 2 — expected: GetResource(TypeName='AWS::SSM::Parameter', Identifier=<name>) returns 200 with ProgressEvent/ResourceDescription carrying that Identifier and the parameter's properties.
Actual: `botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetResource operation: AWS::SSM::Parameter Handler returned status FAILED: Resource of type 'AWS::SSM::Parameter' with identifier '<parameter-name>' was not found. (HandlerErrorCode: NotFound, RequestToken: <uuid>)` while SSM GetParameter still serves the same name.

## Steps to Reproduce
Cause 1:
1. CreateStack with a template whose only resource is {"Param": {"Type":"AWS::SSM::Parameter","Properties":{"Name":...,"Type":"String","Value":...,"ThisPropertyDoesNotExist":"x"}}}.
2. Poll DescribeStacks: observe CREATE_COMPLETE instead of ROLLBACK_COMPLETE, and DescribeStackEvents carries no PROPERTY_VALIDATION row.

Cause 2:
1. CreateStack an AWS::SSM::Parameter with Name/Type/Value plus Description, AllowedPattern and Tier; wait for CREATE_COMPLETE.
2. UpdateStack changing only the write-only properties (Description/AllowedPattern/Tier); wait terminal.
3. Call cloudcontrol GetResource(TypeName='AWS::SSM::Parameter', Identifier=<the parameter name>) — it raises ResourceNotFoundException although ssm GetParameter(Name=<same>) succeeds.
