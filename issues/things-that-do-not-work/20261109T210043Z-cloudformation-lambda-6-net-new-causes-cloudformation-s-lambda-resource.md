Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cfn-lambda-eventinvokeconfig-and-resourcepolicy-read-back-and-removed-on-delete cloudformation__lambda.cfn-lambda-url-duplicate-on-same-function-rejected cloudformation__lambda.cfn-lambda-url-reads-back-and-its-removal-leaves-the-function-without-one cloudformation__lambda.cfn-lambda-layer-version-reads-back-through-lambda-then-not-found-after-delete cloudformation__lambda.deploy-lambda-layerversionpermission-round-trip cloudformation__lambda.cdk-deploy-lambda-version-alias-invoke-reads-back-configuration cloudformation__lambda.cfn-lambda-alias-read-back-and-removed-on-delete cloudformation__lambda.cfn-lambda-version-read-back-and-removed-on-delete cloudformation__lambda.cfn-lambda-eventinvokeconfig-maximumretryattempts-above-schema-max-rejected cloudformation__lambda.deploy-lambda-function-recursive-loop-enum-rejected cloudformation__lambda.deploy-lambda-layerversionpermission-principal-malformed-rejected cloudformation__lambda.output-getatt-on-writeonly-lambda-code-rejected cloudformation__lambda.cdk-watch-lambda-code-edit-hotswaps-then-timeout-edit-falls-back
Issue-Key: cloudformation__lambda|CreateStack|ValidationError
Issue-Key: lambda|GetFunctionUrlConfig|ResourceNotFoundException
Issue-Key: cloudformation__lambda.cfn-lambda-url-reads-back-and-its-removal-leaves-the-function-without-one|run#11|KeyError|outputs = {o["OutputKey"]: o["OutputValue"]
Issue-Key: cloudformation__lambda.cfn-lambda-layer-version-reads-back-through-lambda-then-not-found-after-delete|run#24|AssertionError|assert getatt_arn == physical_id, (
Issue-Key: cloudformation__lambda.deploy-lambda-layerversionpermission-round-trip|run#13|AssertionError|assert _versions(lam, layer_name) == [(layer_arn, 1)] and out["LayerArnOut"] == layer_arn, (
Issue-Key: cloudformation__lambda.cdk-deploy-lambda-version-alias-invoke-reads-back-configuration|run#12|AssertionError|assert absent == want, f"GetFunction on {qualifier_name!r} after `cdk destroy` returned {absent!r}"
Issue-Key: cloudformation__lambda.cfn-lambda-alias-read-back-and-removed-on-delete|run#19|AssertionError|assert message == want, f"unexpected message {message!r}, wanted {want!r}"
Issue-Key: cloudformation__lambda.cfn-lambda-version-read-back-and-removed-on-delete|run#20|AssertionError|assert message == want, f"unexpected message {message!r}, wanted {want!r}"
Issue-Key: cloudformation__lambda.cfn-lambda-eventinvokeconfig-maximumretryattempts-above-schema-max-rejected|run#7|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected ROLLBACK_COMPLETE, stack ended in {status!r}"
Issue-Key: cloudformation__lambda.deploy-lambda-function-recursive-loop-enum-rejected|run#11|AssertionError|assert stack_failed == [_STACK_REASON], (
Issue-Key: cloudformation__lambda.deploy-lambda-layerversionpermission-principal-malformed-rejected|run#12|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Key: cloudformation__lambda.output-getatt-on-writeonly-lambda-code-rejected|run#13|AssertionError|assert bad["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__lambda.cdk-watch-lambda-code-edit-hotswaps-then-timeout-edit-falls-back|run#54|AssertionError|assert new_fn_statuses, (
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# 6 net-new causes: CloudFormation's Lambda resource types do not validate, do not provision Url/ResourcePolicy, and mint malformed layer ARNs

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Six distinct root causes, all in CloudFormation's handling of AWS::Lambda::* resource types. They are separate work items; all six must be closed.

Cause 1 of 6 — AWS::Lambda::ResourcePolicy is an unrecognized resource type.
AWS: a template declaring AWS::Lambda::ResourcePolicy is accepted and the resource is provisioned; CreateStack returns a StackId. A genuinely unknown type is refused with ValidationError `Template format error: Unrecognized resource types: [<type>]` — the fixed text is `Template format error: Unrecognized resource types: ` and the bracketed list is the offending type names from the template.
Emulator: CreateStack answers ValidationError `Template format error: Unrecognized resource types: [AWS::Lambda::ResourcePolicy]`, so the stack never exists.
Why: any stack that declares a Lambda resource policy cannot be created at all.
Fix: the CloudFormation resource-type registry plus a provisioner for AWS::Lambda::ResourcePolicy (create/read/delete mapped onto Lambda PutResourcePolicy/GetResourcePolicy/DeleteResourcePolicy).
Traces: cloudformation__lambda.cfn-lambda-eventinvokeconfig-and-resourcepolicy-read-back-and-removed-on-delete.

Cause 2 of 6 — AWS::Lambda::Url is accepted but no function URL is created and its Fn::GetAtt output is empty.
AWS: a stack declaring AWS::Lambda::Url creates the URL; Lambda GetFunctionUrlConfig then returns the config, a second AWS::Lambda::Url on the same function+qualifier is refused with ResourceConflictException, and `Fn::GetAtt <Url>.FunctionUrl` resolves to the published URL so the stack Output carries an OutputValue.
Emulator: the stack reports success but GetFunctionUrlConfig answers ResourceNotFoundException `The resource you requested does not exist.`, and the stack Output for the URL has no OutputValue at all (the read raises KeyError 'OutputValue').
Why: a client that deploys a function URL by template gets a stack that claims success with no URL behind it, and templates whose Outputs export the URL export nothing.
Fix: the CloudFormation AWS::Lambda::Url provisioner — create/delete the URL config against Lambda state and resolve its FunctionUrl/FunctionArn attributes for Fn::GetAtt.
Traces: cloudformation__lambda.cfn-lambda-url-duplicate-on-same-function-rejected, cloudformation__lambda.cfn-lambda-url-reads-back-and-its-removal-leaves-the-function-without-one.

Cause 3 of 6 — Fn::GetAtt on AWS::Lambda::LayerVersion returns a synthetic ARN that is not a layer-version ARN.
AWS: Smithy GetLayerVersion.LayerVersionArn is constrained to `^arn:(aws[a-zA-Z-]*)?:lambda:<region>:<account>:layer:[a-zA-Z0-9-_]+:[0-9]+$`; `Fn::GetAtt <Layer>.LayerVersionArn` resolves to exactly the physical id Lambda minted, i.e. `arn:<partition>:lambda:<region>:<account>:layer:<layer name>:<version number>`.
Emulator: resolves it to `arn:<partition>:lambda:<region>:<account>:layer-version/<stack>-<logical id>-<suffix>` — an ARN whose resource segment is `layer-version/<CFN physical id>`, matching neither the Smithy pattern nor the layer version that ListLayerVersions reports.
Why: every consumer of that attribute (a function's Layers property, a LayerVersionPermission, a cross-stack export) is handed an ARN Lambda cannot resolve.
Fix: the AWS::Lambda::LayerVersion attribute resolver — return the layer-version ARN from Lambda state rather than deriving one from the CloudFormation physical id.
Traces: cloudformation__lambda.cfn-lambda-layer-version-reads-back-through-lambda-then-not-found-after-delete, cloudformation__lambda.deploy-lambda-layerversionpermission-round-trip.

Cause 4 of 6 — ResourceNotFoundException messages for version- and alias-qualified Lambda reads are not qualified.
AWS: after the owning stack is deleted, GetFunction with a version qualifier is refused with `Function not found: <function ARN>:<version>` and GetAlias with `Cannot find alias arn: <function ARN>:<alias name>`. The fixed texts are `Function not found: ` and `Cannot find alias arn: `; the variable part is the fully qualified ARN built from the request (function ARN plus the qualifier actually asked for).
Emulator: drops the qualifier and answers the unqualified `Function not found: <function ARN>` in all three cases — including where the request named an alias, so an alias lookup reports a missing function instead of a missing alias.
Why: a client cannot distinguish "the function is gone" from "this version/alias is gone", which is the whole point of the qualified read after a delete or a rollback.
Fix: Lambda's GetFunction/GetAlias error serializers — build the not-found message from the qualified resource the request named, and use the alias-specific template for alias lookups.
Traces: cloudformation__lambda.cdk-deploy-lambda-version-alias-invoke-reads-back-configuration, cloudformation__lambda.cfn-lambda-alias-read-back-and-removed-on-delete, cloudformation__lambda.cfn-lambda-version-read-back-and-removed-on-delete.

Cause 5 of 6 — property validation on AWS::Lambda::* resources is not enforced, so invalid templates reach CREATE_COMPLETE instead of rolling back.
AWS: a template whose property violates the resource schema fails the resource at create and rolls the stack back to ROLLBACK_COMPLETE, with the stack-level CREATE_FAILED reason `Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.` (fixed text; the error count is the number of schema violations found). This covers at least: EventInvokeConfig.MaximumRetryAttempts above the schema maximum, Function.RecursiveLoop outside its enum, LayerVersionPermission.Principal that is not a well-formed principal, and an Output whose Fn::GetAtt names the write-only Function.Code.
Emulator: every one of those stacks ends CREATE_COMPLETE, and the stack's CREATE_FAILED reasons list is empty.
Why: a client's negative deployments silently "succeed", so a bad template is promoted instead of being rejected, and nothing in DescribeStackEvents names the offending property.
Fix: the CloudFormation resource-schema validation pass for the AWS::Lambda::* types (and the stack-event writer that must record the validation reason), ahead of the provisioners.
Traces: cloudformation__lambda.cfn-lambda-eventinvokeconfig-maximumretryattempts-above-schema-max-rejected, cloudformation__lambda.deploy-lambda-function-recursive-loop-enum-rejected, cloudformation__lambda.deploy-lambda-layerversionpermission-principal-malformed-rejected, cloudformation__lambda.output-getatt-on-writeonly-lambda-code-rejected.

Cause 6 of 6 — a fallback deploy that changes only a function's Timeout emits no new resource event.
AWS: when a hotswap is not possible and the deploy falls back to a CloudFormation stack update, the AWS::Lambda::Function resource gets fresh UPDATE_IN_PROGRESS/UPDATE_COMPLETE events in DescribeStackEvents for that update.
Emulator: no new event appears on the AWS::Lambda::Function resource after the fallback deploy that changed Timeout.
Why: tooling that distinguishes a hotswap from a real deployment by watching stack events concludes nothing was deployed.
Fix: the stack-update path's event writer for AWS::Lambda::Function — record per-resource update events when a changed property forces a stack update.
Traces: cloudformation__lambda.cdk-watch-lambda-code-edit-hotswaps-then-timeout-edit-falls-back.

## Evidence
Cause 1: botocore ClientError (ValidationError) on CreateStack: 'Template format error: Unrecognized resource types: [AWS::Lambda::ResourcePolicy]'.
Cause 2: botocore.errorfactory.ResourceNotFoundException on GetFunctionUrlConfig: 'The resource you requested does not exist.'; and KeyError 'OutputValue' reading the stack Output for the URL.
Cause 3: AssertionError: Fn::GetAtt LayerVersionArn 'arn:aws:lambda:<region>:<account>:layer-version/<stack>-Layer-<suffix>' does not match the physical id 'arn:aws:lambda:<region>:<account>:layer:<layer>:1'; and listed versions [('arn:...:layer:<layer>:1', 1)] vs GetAtt 'arn:...:layer-version/<stack>-Layer-<suffix>'.
Cause 4: AssertionError: unexpected message 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>', wanted 'Cannot find alias arn: arn:...:function:<name>:live'; and wanted 'Function not found: arn:...:function:<name>:1'.
Cause 5: AssertionError: expected ROLLBACK_COMPLETE, stack ended in 'CREATE_COMPLETE'; AssertionError: the stack's CREATE_FAILED reasons were [], expected ['Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.']; 'rejected stack ended CREATE_COMPLETE'.
Cause 6: AssertionError: no new event on the AWS::Lambda::Function resource appeared after the fallback deploy that changed its Timeout.
(stack names, logical/physical ids, suffixes and request tokens in these lines are per-run values, not requirements.)

## Steps to Reproduce
Cause 1: CreateStack with a template declaring AWS::Lambda::ResourcePolicy -> ValidationError 'Unrecognized resource types'.
Cause 2: CreateStack with AWS::Lambda::Function + AWS::Lambda::Url and an Output exporting GetAtt Url.FunctionUrl; then GetFunctionUrlConfig on the function, and add a second AWS::Lambda::Url for the same function.
Cause 3: CreateStack with AWS::Lambda::LayerVersion and an Output exporting GetAtt Layer.LayerVersionArn; compare with lambda ListLayerVersions.
Cause 4: Deploy a stack with a Function, a Version and an Alias; delete the stack; call GetFunction(Qualifier=<version>) and GetAlias(Name=<alias>).
Cause 5: CreateStack with (a) EventInvokeConfig MaximumRetryAttempts above the schema max, (b) Function RecursiveLoop outside its enum, (c) LayerVersionPermission with a malformed Principal, (d) an Output with Fn::GetAtt on Function.Code; each must roll back.
Cause 6: Deploy a function, edit only its Timeout so a hotswap is impossible, redeploy, and read DescribeStackEvents for the function resource.
