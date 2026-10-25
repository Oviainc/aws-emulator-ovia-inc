Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected|run#16|AssertionError|assert stack.get("StackStatusReason") == expected_reason, (
Issue-Commit: 4cec19096fbcd381402fd64bdd024f07c05668df

# CloudFormation StackStatusReason for a failed AWS::CloudWatch::Dashboard appends the provider's error text to AWS's fixed summary (1 cause)

> _Note: this issue was identified against the code base at commit `4cec19096fbcd381402fd64bdd024f07c05668df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One net-new root cause in the CloudFormation stack-failure reporting path, reached only AFTER the dashboard resource provider started failing the stack at all.

Related, and NOT this cause: the open issue 20261025T163307Z-cloudwatch-dashboard-validation-putdashboard-accepts-a-widget-body.md carries this same check id in its Issue-Trace-Ids, but its recorded replay key and its cause-2 description are now stale — they describe the stack reaching CREATE_COMPLETE (`assert stack["StackStatus"] == "CREATE_FAILED"` failing with 'CREATE_COMPLETE'). That leg now PASSES: the stack does settle at CREATE_FAILED and the dashboard write path does refuse the malformed body. The report did not mark this check OWNED, and the failure has moved to the next assertion, so this entry records the CURRENT residual signature instead of letting the remaining gap be closed as fixed. Also related but distinct: 20261025T151319Z-cloudwatch-4-net-new-root-causes-cfn-deployed.md cause 1 and 20261025T150742Z-cloudformation-cloudwatch-aws-cloudwatch-dashboard-is-never-provisioned.md cover the POSITIVE CFN leg (a valid body must create a readable dashboard); this cause is purely the stack-level failure REASON text.

Cause 1 of 1: the stack-level StackStatusReason of a CREATE_FAILED stack concatenates the failing resource provider's error onto AWS's fixed resource-summary sentence, instead of being that sentence alone.
Failing check id: cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected.
AWS: recorded behavior only — CloudFormation stack provisioning is not expressible in the CloudWatch Smithy model, and the CloudFormation model types StackStatusReason as a free-form string with no documented template. The underlying refusal IS modeled: Smithy (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json) gives PutDashboard the error DashboardInvalidInputError (errorKind client, httpError 400, awsQueryError code InvalidParameterInput), and the recording pins its message for a body that is not valid JSON as 'The field DashboardBody must be a valid JSON object'. What the recording pins for the STACK is a different, narrower string: a CreateStack with OnFailure='DO_NOTHING' whose only resource fails to create settles at StackStatus CREATE_FAILED with StackStatusReason whose FIXED text is 'The following resource(s) failed to create: [' + ']. ' — including the trailing space after the period — and whose only VARIABLE part is the comma-separated list of the template's own LogicalResourceIds that failed (here the single id 'BadDashboard', taken from the request template, not from this one recording). Nothing about the provider's own error code or message appears in the stack-level reason; that per-resource diagnosis belongs to the failing resource's StackEvent ResourceStatusReason, which is a separate field read through DescribeStackEvents.
Emulator: DescribeStacks reports StackStatus CREATE_FAILED (correct) but StackStatusReason 'The following resource(s) failed to create: [BadDashboard]. DashboardInvalidInputError: The field DashboardBody must be a valid JSON object' — AWS's fixed summary with the provider's exception name and message appended after the trailing space. The fixed text and the logical-id substitution are right; the suffix is additional text AWS does not emit here.
Why: a client that reads the stack-level reason gets a string that does not match the documented summary form. IaC tooling and deployment wrappers parse this field to recover the set of failed logical resource ids (the bracketed list) and to decide whether a stack failure is resource-level at all; an unbounded provider-error suffix breaks that parse and, when several resources fail, makes the field's length and content depend on provider error text rather than on the resource list. It also hides the layering a client depends on: the operator is taught to find the per-resource cause in DescribeStackEvents' ResourceStatusReason, and an emulator that folds it into the stack summary lets a consumer that never reads stack events appear to work while one that does see the cause duplicated.
Fix: the CloudFormation stack create/rollback state machine's status-reason composition — the stack-level StackStatusReason must be rendered as the fixed 'The following resource(s) failed to create: [<comma-separated LogicalResourceIds>]. ' sentence with nothing appended, and the failing provider's error (DashboardInvalidInputError / InvalidParameterInput and its message) must be carried on that resource's StackEvent ResourceStatusReason instead.
Traces: cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the built emulator (108/412 passed). This check was marked neither ALREADY FILED nor OWNED by the report.

cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected [mismatch]:
  File "check_cloudwatch_deploy_cloudformation_invalid_dashboard_body_rejected.py", line 64, in run
    assert stack.get("StackStatusReason") == expected_reason, (
  AssertionError: expected StackStatusReason 'The following resource(s) failed to create: [BadDashboard]. ', got 'The following resource(s) failed to create: [BadDashboard]. DashboardInvalidInputError: The field DashboardBody must be a valid JSON object'

Expected (AWS, recorded): StackStatusReason == 'The following resource(s) failed to create: [BadDashboard]. ' — fixed text 'The following resource(s) failed to create: [' + ']. ' (trailing space included), variable part = the failing LogicalResourceIds from the submitted template.
Actual (emulator): the same string with 'DashboardInvalidInputError: The field DashboardBody must be a valid JSON object' appended.

Legs that PASSED earlier in the same check, and must keep passing:
  - cloudwatch:PutDashboard(DashboardName=<name>, DashboardBody='{ this is not valid dashboard json ]') -> error code 'InvalidParameterInput' with message exactly 'The field DashboardBody must be a valid JSON object'.
  - cloudformation:DescribeStacks(StackName=<bad stack>) settles at StackStatus 'CREATE_FAILED' under OnFailure='DO_NOTHING'.

Legs NOT reached because the assertion above aborted the check:
  - cloudwatch:GetDashboard(DashboardName=<name>) must answer ResourceNotFound with message 'Dashboard <DashboardName> does not exist' (fixed text 'Dashboard ' + ' does not exist' around the name from the request), i.e. the refused resource left nothing behind.
  - Positive control: the same flow with a VALID text-widget body must reach CREATE_COMPLETE and GetDashboard must round-trip that body.

## Steps to Reproduce
1. Boot the emulator and point botocore 'cloudwatch' and 'cloudformation' clients at it.
2. cloudwatch:PutDashboard(DashboardName=<fresh name>, DashboardBody='{ this is not valid dashboard json ]'). Expect 400 InvalidParameterInput, message 'The field DashboardBody must be a valid JSON object' (correct today).
3. cloudformation:CreateStack(StackName=<fresh stack>, OnFailure='DO_NOTHING', TemplateBody = a template whose only resource has LogicalResourceId 'BadDashboard', Type 'AWS::CloudWatch::Dashboard', Properties {DashboardName: <that name>, DashboardBody: '{ this is not valid dashboard json ]'}).
4. Poll cloudformation:DescribeStacks(StackName=<that stack>) until StackStatus is terminal.
   Expected: StackStatus 'CREATE_FAILED' and StackStatusReason exactly 'The following resource(s) failed to create: [BadDashboard]. ' (nothing after the trailing space).
   Observed: StackStatus 'CREATE_FAILED' but StackStatusReason 'The following resource(s) failed to create: [BadDashboard]. DashboardInvalidInputError: The field DashboardBody must be a valid JSON object'.
5. cloudformation:DescribeStackEvents(StackName=<that stack>): the provider error ('DashboardInvalidInputError: The field DashboardBody must be a valid JSON object') belongs on the BadDashboard resource event's ResourceStatusReason, which is where a client is expected to read it.
6. cloudwatch:GetDashboard(DashboardName=<that name>) must answer ResourceNotFound 'Dashboard <name> does not exist'.
7. Positive control: deploy the same resource type with a valid body (a single text widget) and confirm CREATE_COMPLETE plus a byte-equivalent GetDashboard round-trip.
8. Re-replay to confirm the fix: cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected.
