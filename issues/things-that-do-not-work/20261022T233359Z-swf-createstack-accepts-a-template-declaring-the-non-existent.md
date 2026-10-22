Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.deploy-domain-via-cloudformation
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf.deploy-domain-via-cloudformation|run#6|AssertionError|raise AssertionError(
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# CreateStack accepts a template declaring the non-existent resource type AWS::SWF::Domain and drives it to CREATE_COMPLETE instead of rejecting it as an unrecognized resource type

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause. Filed under service `swf` because that is the leading token of the failing check id (swf.deploy-domain-via-cloudformation); the defect itself lives in the CloudFormation crate, services/service-com-amazonaws-cloudformation-cloudformation.

Cause 1 of 1: CreateStack does not validate resource types against the set CloudFormation actually supports.
AWS: recorded behavior only — the Smithy model for CloudFormation does not enumerate resource types, and CloudFormation has no native AWS::SWF::Domain resource type (SWF domains are created only through the SWF API: RegisterDomain, then RegisterWorkflowType / StartWorkflowExecution against that domain). A template whose Resources section declares a type CloudFormation does not know is refused at CreateStack/validation time with HTTP 400 and error code ValidationError (recorded message example: "Unrecognized resource types: [AWS::SWF::Domain]"); the stack is never created, so it never reaches CREATE_IN_PROGRESS or CREATE_COMPLETE.
Emulator: CreateStack with that template is ACCEPTED — no error is returned at all — and the stack proceeds as if the resource type existed. The check fails with "CreateStack with AWS::SWF::Domain was accepted; real AWS rejects this resource type at validation time".
Why: this is an overly-permissive divergence, the most dangerous direction. Infrastructure-as-code pipelines validated against the emulator pass with templates that real AWS refuses outright, so the failure moves from a fast, local 400 to a deploy-time rejection in a real account. Worse, a stack that "succeeds" here produces no SWF domain, so any downstream step that assumes the domain exists (or reads the stack's Outputs for its name) silently operates against a resource that was never created.
Fix: resource-type validation in the CloudFormation crate's CreateStack handler (and the shared template-validation path it hands the template to) — reject templates declaring unknown resource types with a 400 ValidationError before the stack is created, instead of admitting them.
Traces: swf.deploy-domain-via-cloudformation. Note: the related check swf.deploy-domain-cfn-stack-behaves is separately blocked by the unimplemented CloudFormation ValidateTemplate stub, which the suite reports as already filed; that stub issue covers standing ValidateTemplate up, not CreateStack's admission of unknown resource types.

## Evidence
Behavioral check swf.deploy-domain-via-cloudformation (verdict: mismatch).

Request: CreateStack with a template whose Resources section is {"SwfDomain": {"Type": "AWS::SWF::Domain", "Properties": {"Name": {"Ref": "DomainName"}, "WorkflowExecutionRetentionPeriodInDays": "1"}}} and an Output referencing it.

Expected (real AWS): HTTP 400, error code ValidationError, stack not created (recorded message example: "Unrecognized resource types: [AWS::SWF::Domain]").

Actual (emulator): the CreateStack call returned successfully; no error was raised. Harness output: AssertionError: CreateStack with AWS::SWF::Domain was accepted; real AWS rejects this resource type at validation time.

## Steps to Reproduce
1. Boot the emulator.
2. Call cloudformation CreateStack with StackName=<any>, Parameters=[{ParameterKey: DomainName, ParameterValue: <any>}] and a TemplateBody of {"Parameters": {"DomainName": {"Type": "String"}}, "Resources": {"SwfDomain": {"Type": "AWS::SWF::Domain", "Properties": {"Name": {"Ref": "DomainName"}, "WorkflowExecutionRetentionPeriodInDays": "1"}}}, "Outputs": {"DomainName": {"Value": {"Ref": "SwfDomain"}}}}.
3. Observe the call is accepted (no ValidationError) and the stack is created; real AWS returns HTTP 400 ValidationError for the unrecognized resource type and creates nothing.
4. For contrast, the only real deployment path works: swf RegisterDomain, then RegisterWorkflowType and StartWorkflowExecution against that domain.
