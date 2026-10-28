Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected
Issue-Key: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected|run#28|AssertionError|assert initiated[1] == "Resource creation Initiated", (
Issue-Commit: 4f4ec5316b812872d2e4beba3bc651533b6eb906

# cloudformation__rds__ssm: a resource that starts provisioning emits no CREATE_IN_PROGRESS stack event with ResourceStatusReason 'Resource creation Initiated' (1 cause)

> _Note: this issue was identified against the code base at commit `4f4ec5316b812872d2e4beba3bc651533b6eb906`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
One net-new root cause in CloudFormation's per-resource stack-event stream, reached only AFTER both ssm-secure dynamic-reference rejections already behaved correctly.

Related, and NOT this cause: the open issues 20261025T050651Z-cloudformation-records-no-per-resource-stack-events-a.md, 20261025T053459Z-cloudformation-emits-no-per-resource-stack-events-and.md and 20261025T133035Z-cloudformation-7-net-new-root-causes-cli-deploy.md all state that there are NO per-resource stack events at all. That is no longer the signature here: DescribeStackEvents DOES return an event for the template's own LogicalResourceId ('Database'), with ResourceStatus 'CREATE_COMPLETE'. What is missing is specifically the in-progress step of that resource's event sequence and its fixed reason text, so this residual is not covered by those issues' replay keys and the report marked this check neither ALREADY FILED nor OWNED.

Cause 1 of 1: a resource whose provisioning the stack has started never produces a CREATE_IN_PROGRESS StackEvent carrying ResourceStatusReason 'Resource creation Initiated'; the resource's event sequence jumps straight to a reasonless CREATE_COMPLETE, so there is no observable point at which the resource is provisioning.
Failing check id: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected.
AWS: recorded behavior only for the TEXT; the structure is modeled. Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): StackEvent carries LogicalResourceId, ResourceStatus and ResourceStatusReason ('Success/failure message associated with the resource'), and StackEvent.ResourceStatus is an enum whose members include CREATE_IN_PROGRESS as well as CREATE_COMPLETE — the in-progress state is a first-class resource status, not an internal detail. The RDS side is modeled too: CreateDBInstance is a long-running provisioning call whose output DBInstance reports a non-terminal status before it is usable. What the recording pins is the sequence: for each resource CloudFormation writes CREATE_IN_PROGRESS with no ResourceStatusReason, then a second CREATE_IN_PROGRESS whose ResourceStatusReason is the fixed string 'Resource creation Initiated' (no variable part at all), and only then CREATE_COMPLETE. For an AWS::RDS::DBInstance that in-progress window lasts minutes, so the 'Resource creation Initiated' event is observable for the whole of a 60 s poll.
Emulator: polling DescribeStackEvents for the 'Database' resource for 60 s never yields an event with ResourceStatusReason 'Resource creation Initiated'. The newest (and apparently only) 'Database' event is ResourceStatus 'CREATE_COMPLETE' with ResourceStatusReason absent — reported by the check as status='CREATE_COMPLETE' reason=None. The DBInstance is therefore presented as created with no initiating event and no in-progress phase.
Why: this is the signal a client uses to tell 'CloudFormation accepted my template and the provider is working' apart from 'CloudFormation rejected it before calling the provider'. That distinction is the whole point of a dynamic-reference or template gate: a consumer (and this check's positive control) reads the resource's ResourceStatusReason to prove the resource reached its provider at all, because a reasonless event cannot distinguish the two. Dropping the in-progress events also makes every resource look instantaneous, so progress reporting, CREATE_IN_PROGRESS-based waiters and any tooling that tracks which resource a stack is currently working on see nothing to track, and a stack whose provider would have taken minutes cannot be observed mid-flight.
Fix: the CloudFormation stack create/provisioning state machine's event recorder — each resource must append CREATE_IN_PROGRESS (no reason), then CREATE_IN_PROGRESS with ResourceStatusReason exactly 'Resource creation Initiated', then its terminal CREATE_COMPLETE/CREATE_FAILED event, and the DBInstance provider must not settle the resource before that initiating event is visible through DescribeStackEvents.
Traces: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the built emulator (38/242 passed). This check was marked neither ALREADY FILED nor OWNED by the report.

cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected [mismatch]:
  File "check_cloudformation__rds__ssm_rds_ssm_secure_dynamic_reference_misuse_rejected.py", line 115, in run
    assert initiated[1] == "Resource creation Initiated", (
  AssertionError: a correctly-used ssm-secure reference on MasterUserPassword must clear CloudFormation's own gate and reach real RDS creation, got status='CREATE_COMPLETE' reason=None

Expected (AWS, recorded): within 60 s of CreateStack, DescribeStackEvents contains an event for LogicalResourceId 'Database' with ResourceStatus 'CREATE_IN_PROGRESS' and ResourceStatusReason exactly 'Resource creation Initiated' (entirely fixed text; no values are substituted into it).
Actual (emulator): the only 'Database' event is ResourceStatus 'CREATE_COMPLETE' with ResourceStatusReason absent (None), for the whole poll window.

Legs that PASSED earlier in the same check, and must keep passing:
  - ssm:PutParameter of a String parameter and of a SecureString parameter.
  - Negative 1: a stack whose AWS::RDS::DBInstance MasterUserPassword is '{{resolve:ssm-secure:<plain String parameter>}}' settles at StackStatus 'ROLLBACK_COMPLETE', and the stack-level ResourceStatusReason contains 'ssm-secure prefix was used for non-secure parameter'.
  - Negative 2: a stack whose MasterUsername is '{{resolve:ssm-secure:<real SecureString>}}' settles at 'ROLLBACK_COMPLETE' with a reason containing both 'SSM Secure reference is not supported' and 'MasterUsername'.
So CloudFormation's ssm-secure gate itself is behaving; the divergence is only that the positive control's resource never reports an initiating event.

## Steps to Reproduce
1. Boot the emulator and point botocore 'cloudformation', 'rds' and 'ssm' clients at it.
2. ssm:PutParameter(Name=<secure-name>, Value='produser', Type='SecureString').
3. cloudformation:CreateStack(StackName=<fresh>, TemplateBody = a template with one resource, LogicalResourceId 'Database', Type 'AWS::RDS::DBInstance', Properties {DBInstanceClass 'db.t3.micro', AllocatedStorage '20', Engine 'postgres', MasterUsername 'produser', MasterUserPassword '{{resolve:ssm-secure:<secure-name>}}'}, DeletionPolicy Delete).
4. Immediately poll cloudformation:DescribeStackEvents(StackName=<that stack>) every 3 s for up to 60 s and look at the events whose LogicalResourceId is 'Database'.
   Expected: the sequence CREATE_IN_PROGRESS (no ResourceStatusReason), then CREATE_IN_PROGRESS with ResourceStatusReason exactly 'Resource creation Initiated', then CREATE_COMPLETE — with the 'Resource creation Initiated' event observable for the whole window, since a real DBInstance create takes minutes.
   Observed: a single 'Database' event, ResourceStatus 'CREATE_COMPLETE', ResourceStatusReason absent; no CREATE_IN_PROGRESS event and no 'Resource creation Initiated' reason ever appears.
5. Controls that must keep working while this is fixed: the same template with MasterUserPassword '{{resolve:ssm-secure:<plain String parameter>}}' must reach ROLLBACK_COMPLETE with a reason containing 'ssm-secure prefix was used for non-secure parameter', and with MasterUsername '{{resolve:ssm-secure:<SecureString>}}' must reach ROLLBACK_COMPLETE with a reason containing 'SSM Secure reference is not supported' and 'MasterUsername'.
6. Re-replay to confirm the fix: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected.
