Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__ssm.cdk-custom-resource-lifecycle-wires-response-field-and-rejects-conflict cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly cloudformation__lambda__ssm.sam-ssm-parameter-read-policy-other-parameter-denied
Issue-Key: cloudformation__lambda__ssm.cdk-custom-resource-lifecycle-wires-response-field-and-rejects-conflict|_cdk#18|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly|run#14|AssertionError|assert rc != 0, (
Issue-Key: cloudformation__lambda__ssm.sam-ssm-parameter-read-policy-other-parameter-denied|run#14|KeyError|policies = _processed(cfn, stack_id)["Resources"]["FnRole"]["Properties"]["Policies"]
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# cloudformation__lambda__ssm: 3 net-new root causes — custom-resource response data is not wired into Fn::GetAtt, an onEvent handler that raises still reaches CREATE_COMPLETE, and the SAM transform drops the generated role's Policies

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three distinct net-new root causes in the emulator's CloudFormation custom-resource and SAM-transform handling. They are independent defects and must each be closed; the numbering below is the count a fixer must satisfy.

=== Cause 1 of 3: a custom resource's response Data is not available to Fn::GetAtt, so a dependent resource is created with a null property ===
AWS: documented for AWS::CloudFormation::CustomResource and the custom-resource response object. The provider's SUCCESS response Data map becomes the custom resource's attributes, and Fn::GetAtt <LogicalId>.<field> resolves to the value the provider returned, before any resource that references it is created. A dependent AWS::SSM::Parameter therefore receives a concrete Value and reaches CREATE_COMPLETE.
Emulator: the GetAtt resolves to nothing, so the dependent resource is created with Value null and the SSM handler refuses it: `Resource handler returned message: "1 validation error detected: Value null at 'value' failed to satisfy constraint: Member must not be null" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`; the resource ends CREATE_FAILED and the stack ROLLBACK_COMPLETE, so `cdk deploy` exits 1.
Why: every CDK construct that reads a value back out of a custom resource (the whole `Provider`/`AwsCustomResource` pattern) is unusable: the value a client computed in its provider never reaches the resources that consume it.
Fix: the custom-resource provisioning stage — record the provider response's Data map as the resource's attributes and resolve Fn::GetAtt against it before dependants are created. Related open issues 20261102T093047Z-cloudformation-sns-sqs-6-net-new-root-causes.md and 20261101T212352Z-cloudformation-sns-sqs-a-custom-resource-whose-servicetoken.md record custom-resource GetAtt causes under the cloudformation__sns__sqs heading; neither carries this check's replay key.
Traces: cloudformation__lambda__ssm.cdk-custom-resource-lifecycle-wires-response-field-and-rejects-conflict

=== Cause 2 of 3: a custom resource whose onEvent handler always raises on Create reaches CREATE_COMPLETE instead of failing the stack ===
AWS: documented for the custom-resource request/response protocol. A provider that raises instead of signalling SUCCESS produces a FAILED response (or times out), so the custom resource ends CREATE_FAILED with the provider's reason, the stack rolls back to ROLLBACK_COMPLETE, and `cdk deploy` exits non-zero.
Emulator: the custom resource is reported CREATE_COMPLETE and the stack CREATE_COMPLETE — the provider's failure is never observed — so `cdk deploy` exits 0 and prints a successful deployment. Combined with Cause 1 this shows the custom-resource response is not consulted at all: neither its Data nor its Status reaches CloudFormation.
Why: a client's provisioning logic can fail completely while CloudFormation reports the stack created, which is the worst possible fidelity failure for a deploy gate: nothing was provisioned and nothing was rolled back.
Fix: the custom-resource provisioning stage — invoke the provider, wait for its response, and map a FAILED/absent response to CREATE_FAILED plus stack rollback.
Traces: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly

=== Cause 3 of 3: the SAM transform emits the generated execution role without its Policies ===
AWS: documented for AWS::Serverless::Function Policies (SAM policy templates such as SSMParameterReadPolicy). The SAM transform expands a function's Policies into the generated AWS::IAM::Role, so GetTemplate(TemplateStage='Processed') shows the role resource carrying Properties.Policies with the expanded policy document scoped to the named parameter.
Emulator: the processed template's generated role resource has no `Policies` member at all — reading Resources['FnRole']['Properties']['Policies'] raises KeyError: 'Policies'.
Why: the role a SAM function runs under is generated without any of the permissions the template declared, so permission-scoping behaviour (what the function may and may not read) cannot be expressed or observed at all.
Fix: the SAM (AWS::Serverless-2016-10-31) transform's policy-template expansion and the processed-template serializer.
Traces: cloudformation__lambda__ssm.sam-ssm-parameter-read-policy-other-parameter-denied

## Evidence
Cause 1 — cloudformation__lambda__ssm.cdk-custom-resource-lifecycle-wires-response-field-and-rejects-conflict: expected `cdk deploy <stack> --require-approval never` to exit 0; got AssertionError: ... exited 1, expected 0; output tail shows ROLLBACK_COMPLETE with `AWS::SSM::Parameter | Mirror Resource handler returned message: "1 validation error detected: Value null at 'value' failed to satisfy constraint: Member must not be null" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)` — the Fn::GetAtt of the custom resource's response field resolved to null.

Cause 2 — cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly: expected a non-zero `cdk deploy` for an app whose onEvent handler always raises on Create; got AssertionError: `cdk deploy` exited 0 ...; output tail shows `CREATE_COMPLETE | AWS::CloudFormation::CustomResource | Widget/Default (Widget)` and `CREATE_COMPLETE | AWS::CloudFormation::Stack`.

Cause 3 — cloudformation__lambda__ssm.sam-ssm-parameter-read-policy-other-parameter-denied: reading _processed(cfn, stack_id)['Resources']['FnRole']['Properties']['Policies'] raised KeyError: 'Policies'.

## Steps to Reproduce
Replay the behavioral suite and re-run the listed checks against the emulator:

1. Cause 1: `cdk deploy` an app with a custom resource (CDK `Provider` + AWS::CloudFormation::CustomResource) whose onEvent handler returns Data, and an AWS::SSM::Parameter whose Value is Fn::GetAtt <CustomResource>.<field>; observe the dependent resource's CREATE_FAILED reason.
2. Cause 2: `cdk deploy` an app whose custom-resource onEvent handler raises on Create; check the CLI exit code, the custom resource's events and the stack status.
3. Cause 3: CreateStack a template with Transform AWS::Serverless-2016-10-31 declaring an AWS::Serverless::Function with Policies: [{SSMParameterReadPolicy: ...}], then GetTemplate(TemplateStage='Processed') and read the generated role's Properties.Policies.
