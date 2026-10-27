Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected
Issue-Key: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected|run#16|AssertionError|assert reason1 is not None and "ssm-secure prefix was used for non-secure parameter" in reason1, (
Issue-Commit: 70d60110d97a6c21bc790d7df8d6a76c7ca8ef4a

# cloudformation__rds__ssm: an ssm-secure dynamic-reference misuse fails the stack through the resource provider instead of CloudFormation's own resolver gate, so the stack carries the generic 'resource(s) failed to create' reason and never the ssm-secure diagnostic (1 cause)

> _Note: this issue was identified against the code base at commit `70d60110d97a6c21bc790d7df8d6a76c7ca8ef4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause in CloudFormation's `{{resolve:ssm-secure:...}}` dynamic-reference resolver. The check is marked neither ALREADY FILED nor OWNED by the replay report.

Related, and NOT this cause: the open issue 20261026T092336Z-cloudformation-rds-ssm-a-resource-that-starts-provisioning.md carries this same check id, but its replay key is the check's LAST assertion (the positive control's missing CREATE_IN_PROGRESS / 'Resource creation Initiated' event). The check no longer reaches that assertion: it now fails much earlier, on the FIRST negative leg, so that issue's cause is not the cause below. The resolved issue 20261024T030130Z-cloudformation-rds-ssm-cloudformation-resolves-resolve-ssm-secure-against-a-plain.md fixed only the terminal stack status (the stack does now reach ROLLBACK_COMPLETE); the diagnostic half of the same behavior is still missing, and that issue is resolved so it covers nothing. The generic-reason-bundling open issues 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md and 20261025T050651Z-cloudformation-records-no-per-resource-stack-events-a.md assert there are no per-resource events at all, which is not the signature here (a stack-level reason IS returned).

Cause 1 of 1: an ssm-secure dynamic-reference misuse is not refused by CloudFormation's own resolver before provisioning; the stack is failed by way of the resource and the only stack-level ResourceStatusReason is the generic aggregate 'The following resource(s) failed to create: [<LogicalResourceId>]. '
Failing check id: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected.
AWS: recorded behavior only for the TEXT — a dynamic reference is a CloudFormation template feature, not a modeled API member; the STRUCTURE it is carried in is modeled. Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): StackEvent/StackResource carry LogicalResourceId, ResourceStatus and ResourceStatusReason ('Success/failure message associated with the resource'). What the recording pins is that `{{resolve:ssm-secure:<name>}}` is evaluated by CloudFormation itself, BEFORE any resource provider is called, and that a misuse is reported as the stack's own failure with the resolver's diagnostic as the stack-level ResourceStatusReason: (a) when the referenced SSM parameter is Type String rather than SecureString, the reason contains the fixed text 'ssm-secure prefix was used for non-secure parameter' followed by the parameter name taken from the request; (b) when the prefix is used on a property that is not on CloudFormation's ssm-secure allow-list (here AWS::RDS::DBInstance MasterUsername, even against a genuine SecureString), the reason contains the fixed text 'SSM Secure reference is not supported' and names the offending property ('MasterUsername') — the resource type and property path in the message come from the template, the rest is fixed text. Only the allow-listed property (MasterUserPassword) is let through to the provider. In both cases the stack settles at ROLLBACK_COMPLETE.
Emulator: the stack does settle at ROLLBACK_COMPLETE, but the first stack-level event carrying a ResourceStatusReason is the generic aggregate 'The following resource(s) failed to create: [Database]. ' — i.e. the failure is attributed to the resource provider having tried and failed, not to CloudFormation's ssm-secure gate, and neither resolver diagnostic ('ssm-secure prefix was used for non-secure parameter' / 'SSM Secure reference is not supported') is ever emitted. Replay: AssertionError: expected the ssm-secure/non-secure-parameter reason, got 'The following resource(s) failed to create: [Database]. '
Why: the diagnostic is the only thing that tells a client WHICH misuse it committed. A template that points ssm-secure at a plain String parameter, and one that puts ssm-secure on a property the resolver does not support, are two different authoring mistakes with two different fixes; collapsing both into 'a resource failed to create' means a client validating a template against the emulator learns that the deploy failed but not that it failed on the secret-resolution rule — and cannot distinguish either from a genuine RDS provisioning failure (bad engine version, quota, subnet group). It also hides that the secret was never resolved from a store offering an encryption guarantee, which is the exact confusion the ssm-secure prefix exists to prevent.
Fix: the CloudFormation dynamic-reference resolver and the stack create state machine that records its outcome — ssm-secure resolution must run as CloudFormation's own pre-provisioning gate, must check the referenced SSM parameter's Type and the property allow-list, and must record its failure as the stack's ResourceStatusReason with AWS's wording (fixed text + the parameter name / property path from the template) instead of letting the resource provider fail and falling back to the generic 'The following resource(s) failed to create: [...]' aggregate.
Traces: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the built emulator (41/242 passed). This check was marked neither ALREADY FILED nor OWNED by the report.

cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected [mismatch]:
  File "check_cloudformation__rds__ssm_rds_ssm_secure_dynamic_reference_misuse_rejected.py", line 78, in run
    assert reason1 is not None and "ssm-secure prefix was used for non-secure parameter" in reason1, (
  AssertionError: expected the ssm-secure/non-secure-parameter reason, got 'The following resource(s) failed to create: [Database]. '

Expected (AWS, recorded): after CreateStack with MasterUserPassword = '{{resolve:ssm-secure:<a Type=String parameter>}}', DescribeStackEvents for the stack's own LogicalResourceId yields a ResourceStatusReason containing the fixed text 'ssm-secure prefix was used for non-secure parameter' plus the parameter name from the request; and for MasterUsername = '{{resolve:ssm-secure:<a real SecureString>}}' a reason containing both the fixed text 'SSM Secure reference is not supported' and the property name 'MasterUsername'.
Actual (emulator): ROLLBACK_COMPLETE is reached (that leg passes), but the stack-level reason is the generic aggregate 'The following resource(s) failed to create: [Database]. ' — fixed text 'The following resource(s) failed to create: ' plus the template's logical id, with no resolver diagnostic anywhere in the event stream.

Leg that PASSED and must keep passing: StackStatus settles at ROLLBACK_COMPLETE for the plain-String-parameter misuse (this was the subject of the resolved issue 20261024T030130Z-...-resolve-ssm-secure-against-a-plain.md; the status is fixed, the message is not).

## Steps to Reproduce
1. Boot the emulator and point botocore 'cloudformation', 'rds' and 'ssm' clients at it.
2. ssm:PutParameter(Name=<plain-name>, Value='plainTextPassword123', Type='String') and ssm:PutParameter(Name=<secure-name>, Value='produser', Type='SecureString').
3. cloudformation:CreateStack(StackName=<fresh>, TemplateBody = one resource, LogicalResourceId 'Database', Type 'AWS::RDS::DBInstance', Properties {DBInstanceClass 'db.t3.micro', AllocatedStorage '20', Engine 'postgres', MasterUsername 'produser', MasterUserPassword '{{resolve:ssm-secure:<plain-name>}}'}, DeletionPolicy Delete).
4. Poll DescribeStacks until terminal: ROLLBACK_COMPLETE (correct). Then DescribeStackEvents(StackName=<that stack>) and take the newest event whose LogicalResourceId is the stack's own and which carries a ResourceStatusReason.
   Expected: a reason containing 'ssm-secure prefix was used for non-secure parameter' and the parameter name.
   Observed: 'The following resource(s) failed to create: [Database]. '
5. Repeat with MasterUsername = '{{resolve:ssm-secure:<secure-name>}}' and MasterUserPassword = 'PlainPassw0rd123'.
   Expected: ROLLBACK_COMPLETE with a stack-level reason containing 'SSM Secure reference is not supported' and 'MasterUsername'.
6. Control that must keep working: the same template with MasterUserPassword = '{{resolve:ssm-secure:<secure-name>}}' must clear the gate and reach real RDS provisioning (that leg is the subject of open issue 20261026T092336Z-cloudformation-rds-ssm-a-resource-that-starts-provisioning.md).
7. Re-replay to confirm the fix: cloudformation__rds__ssm.rds-ssm-secure-dynamic-reference-misuse-rejected.
