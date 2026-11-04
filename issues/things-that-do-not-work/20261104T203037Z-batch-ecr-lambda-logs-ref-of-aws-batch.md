Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__ecr__lambda__logs.e2e-diagnose-batch-job-no-logs-lifecycle
Issue-Key: batch__ecr__lambda__logs.e2e-diagnose-batch-job-no-logs-lifecycle|phase#5|AssertionError|raise AssertionError(f'[phase {index}/{len(self.phases)} {name}] {exc}') from exc
Issue-Commit: a52cf8398fdb2537e1d726dcb152539e10f26618

# batch__ecr__lambda__logs: Ref of AWS::Batch::JobDefinition resolves to the job definition NAME, not its ARN, so an IAM policy that references it is refused and the stack rolls back (1 cause)

> _Note: this issue was identified against the code base at commit `a52cf8398fdb2537e1d726dcb152539e10f26618`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterJobDefinition

## Description
Cause 1 of 1: CloudFormation's AWS::Batch::JobDefinition resource resolves `Ref` (its physical resource id) to the bare job-definition NAME instead of the job definition ARN. Any template that feeds `{"Ref": "<JobDefinition>"}` into a place that requires an ARN therefore fails; here it is an AWS::IAM::Role inline policy statement whose `Resource` list is `[{"Ref": "<JobQueue>"}, {"Ref": "<JobDefinition>"}]` for action `batch:SubmitJob`. The queue Ref resolves to an ARN (only the job definition is reported as the offender), so the defect is specific to the job-definition resource.

AWS: `Ref` on AWS::Batch::JobDefinition returns the job definition ARN, of the form `arn:<partition>:batch:<region>:<account>:job-definition/<jobDefinitionName>:<revision>`. Smithy (batch): RegisterJobDefinition's output carries both `jobDefinitionName` and a separate required `jobDefinitionArn` — the ARN is the identifier the API vends for policy and cross-service use, and the name is not an ARN. IAM is then correct to refuse a non-ARN, non-`*` Resource element with HTTP 400 and the message `Resource <value> must be in ARN format or "*".` (fixed text `Resource ` + the rejected Resource element verbatim from the request + ` must be in ARN format or "*".`); on real AWS that refusal never happens for this template because the resolved value IS an ARN, and the stack reaches CREATE_COMPLETE.

Emulator: `Ref` yields the plain job-definition name (the check's parameterised name, i.e. the value passed as the JobDefinitionName parameter), so CreateRole/PutRolePolicy for the logical resource `SubmitJobFunctionRole` is refused 400 by the emulator's IAM with `Resource <jobDefinitionName> must be in ARN format or "*".`, CloudFormation marks that resource CREATE_FAILED (`HandlerErrorCode: InvalidRequest`) and the whole stack ends in ROLLBACK_COMPLETE with `The following resource(s) failed to create: [SubmitJobFunctionRole].` instead of CREATE_COMPLETE.

Why: a client that follows the documented AWS return value of `Ref` for AWS::Batch::JobDefinition — the standard, CDK-generated way to scope `batch:SubmitJob` to one job definition, and the standard way to pass the definition to a submitting Lambda via an environment variable — cannot deploy at all against the emulator. A template that is valid on AWS rolls back, and the stack Output built from the same Ref (`JobDefinitionArn`) would be a name rather than an ARN even if the role were removed, so downstream SubmitJob/DescribeJobDefinitions calls built from it would also be wrong. This blocks the whole six-phase deploy/diagnose lifecycle at phase 1 of 6 (deploy), so nothing past the stack create is graded.

Fix: the CloudFormation AWS::Batch::JobDefinition resource provisioner's physical-resource-id / Ref assignment (it must store the ARN that RegisterJobDefinition returned in `jobDefinitionArn`, not the name), which also feeds intrinsic resolution for Outputs and for other resources' properties. Related open issue 20261103T171935Z-cloudformation-reports-resource-names-where-aws-reports-resource.md records the same name-for-ARN substitution for AWS::S3Tables::TableBucket, AWS::Lambda::Alias and AWS::StepFunctions::StateMachine, but it does not carry AWS::Batch::JobDefinition and does not own this check's replay key, so this cause must be closed here.

Traces: batch__ecr__lambda__logs.e2e-diagnose-batch-job-no-logs-lifecycle (phase 1/6 deploy).

## Evidence
Failing check batch__ecr__lambda__logs.e2e-diagnose-batch-job-no-logs-lifecycle, phase 1/6 (deploy):

AssertionError: [phase 1/6 deploy] stack <stack-name> reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE — the resources that failed were [('<stack-name>', 'The following resource(s) failed to create: [SubmitJobFunctionRole]. '), ('SubmitJobFunctionRole', 'Resource handler returned message: "Resource <jobDefinitionName> must be in ARN format or "*". (Service: Iam, Status Code: 400, ...)" (... HandlerErrorCode: InvalidRequest)')]

Expected (AWS): DescribeStacks reports StackStatus CREATE_COMPLETE; the SubmitJobFunctionRole policy statement's Resource list contains the job queue ARN and the job definition ARN `arn:<partition>:batch:<region>:<account>:job-definition/<name>:<revision>`; stack Output `JobDefinitionArn` (Value = Ref of the AWS::Batch::JobDefinition) is that ARN.
Actual (emulator): Ref of the AWS::Batch::JobDefinition resolves to `<jobDefinitionName>` (a bare name); IAM refuses the resulting policy with HTTP 400 and message `Resource <jobDefinitionName> must be in ARN format or "*".` — fixed text `Resource ` ... ` must be in ARN format or "*".`, variable part = the Resource element from the request; CloudFormation rolls the stack back to ROLLBACK_COMPLETE.

The neighbouring `{"Ref": "OpenMPJobQueue"}` in the same Resource list is not reported as an offender, so AWS::Batch::JobQueue's Ref is ARN-valued and only the job-definition resource diverges.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Run check batch__ecr__lambda__logs.e2e-diagnose-batch-job-no-logs-lifecycle (or reproduce by hand with the steps below).
3. CreateStack with a template containing an AWS::Batch::JobDefinition (Type container, PlatformCapabilities [FARGATE]) and an AWS::IAM::Role whose inline policy has: Action `batch:SubmitJob`, Resource `[{"Ref": "<JobQueueLogicalId>"}, {"Ref": "<JobDefinitionLogicalId>"}]`, plus an Output whose Value is `{"Ref": "<JobDefinitionLogicalId>"}`.
4. Poll DescribeStacks for the stack status and DescribeStackEvents for the role resource.
5. Observe ROLLBACK_COMPLETE with the role's ResourceStatusReason `Resource <jobDefinitionName> must be in ARN format or "*".`; on AWS the stack reaches CREATE_COMPLETE and the Output value is the job definition ARN.
