Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.put-dashboard-with-a-metric-widget-missing-metrics-rejected cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.put-dashboard-with-a-metric-widget-missing-metrics-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected|run#14|AssertionError|assert stack["StackStatus"] == "CREATE_FAILED", stack["StackStatus"]
Issue-Commit: 2efbe0db58fc669c0e9ee58e639ae2f2fc196447

# CloudWatch dashboard validation: PutDashboard accepts a widget body AWS refuses, and a CloudFormation AWS::CloudWatch::Dashboard carrying an invalid body reaches CREATE_COMPLETE (2 causes)

> _Note: this issue was identified against the code base at commit `2efbe0db58fc669c0e9ee58e639ae2f2fc196447`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDashboard

## Description
Two net-new root causes in the CloudWatch dashboard write path, both reached only AFTER PutDashboard stopped answering NotImplemented. The fixed issue 20261025T020208Z-cloudwatch-putdashboard-stub.md stood the operation up and the JSON-well-formedness leg now PASSES (a body that is not valid JSON is correctly refused InvalidParameterInput with the message 'The field DashboardBody must be a valid JSON object'); this entry records the CURRENT residual signature of the two legs that still diverge, so the remaining gap is not closed as fixed. The open issue 20261025T024908Z-cloudwatch-four-scaffolded-operations-answer-notimplemented-so-getmetricdata.md (cause 2) lists both check ids below in its Issue-Trace-Ids, but its recorded replay keys and its whole description are stale — it describes PutDashboard answering NotImplemented for both bodies, which is no longer what happens, and the report did not mark either check owned. Related, and NOT these causes: 20261025T151319Z-cloudwatch-4-net-new-root-causes-cfn-deployed.md cause 1 and 20261025T150742Z-cloudformation-cloudwatch-aws-cloudwatch-dashboard-is-never-provisioned.md cover the POSITIVE CFN leg (a valid template body must create a readable dashboard); cause 2 here is the REFUSAL leg (an invalid body must fail the resource and the stack). Both causes below must be closed before this issue is submitted.

Cause 1 of 2: PutDashboard does not validate DashboardBody against the widget schema, so a body that is valid JSON but structurally invalid is accepted and stored.
Failing check id: cloudwatch.put-dashboard-with-a-metric-widget-missing-metrics-rejected.
AWS: Smithy (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json) models PutDashboard with @required DashboardName and @required DashboardBody, output member DashboardValidationMessages (list), and the error DashboardInvalidInputError (errorKind client, httpError 400, awsQueryError code InvalidParameterInput). Recorded behavior pins the refusal: a metric widget whose `properties` omits `metrics` is refused 400 / InvalidParameterInput with a message whose fixed text is 'The dashboard body is invalid, there are <N> validation errors:' followed by a newline and a JSON array rendered with indent=2, one object per problem of the form {"dataPath": <path>, "message": <problem>}. The variable parts come from the submitted body: N is the number of problems found, each dataPath locates the offending node in the submitted document (for a single bad first widget: '/widgets/0/properties'), and the per-problem texts are the schema's own ('Should have required property \'metrics\'', 'The metric widget should have specified a region and a data source or an alarm annotation', 'Should have required property \'annotations\''). Nothing is stored for a refused body: GetDashboard on that name then answers ResourceNotFound with 'Dashboard <DashboardName> does not exist' (fixed text 'Dashboard ' + ' does not exist' around the name from the request). A well-formed body is a 200 whose DashboardValidationMessages is empty and which round-trips byte-equivalently through GetDashboard.
Emulator: the call SUCCEEDS. The check's error helper raises 'the call succeeded; real AWS rejects it, so there is no error code' at the put_dashboard of the body {"widgets": [{"type": "metric", "x": 0, "y": 0, "width": 24, "height": 6, "properties": {"title": "End-to-End Latency"}}]} — no error code, no validation message, and the dashboard is created under the refused name (the check registers a delete for it). Only JSON parsing is performed; the widget schema is never applied.
Why: a client that relies on CloudWatch to validate a dashboard document gets no diagnosis at all and believes a broken layout was accepted. Dashboard-authoring tools and IaC pipelines that surface the per-dataPath problems to the user show success, and the dashboard that is stored cannot render the widget. It also inverts the strongest signal available: an emulator ACCEPTING a call AWS refuses means downstream checks of 'the refused name is unknown to GetDashboard' can never hold.
Fix: the PutDashboard handler's body validation — a widget-schema validator over the parsed DashboardBody that raises DashboardInvalidInputError / InvalidParameterInput with the enumerated per-dataPath message, and which writes nothing to the dashboard state when it fires.
Traces: cloudwatch.put-dashboard-with-a-metric-widget-missing-metrics-rejected.

Cause 2 of 2: a CloudFormation stack whose AWS::CloudWatch::Dashboard carries a DashboardBody that CloudWatch itself refuses reaches CREATE_COMPLETE instead of failing the resource.
Failing check id: cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected.
AWS: recorded behavior (CloudFormation resource provisioning is not expressible in the CloudWatch Smithy model; the underlying refusal is the modeled DashboardInvalidInputError / InvalidParameterInput above). Recorded: a CreateStack with OnFailure='DO_NOTHING' whose only resource is AWS::CloudWatch::Dashboard with a malformed DashboardBody settles at StackStatus CREATE_FAILED with StackStatusReason whose fixed text is 'The following resource(s) failed to create: [<LogicalResourceId>]. ' — the bracketed id is the template's own logical resource name, the rest is fixed, including the trailing space. The effect is absent: GetDashboard on the declared DashboardName answers ResourceNotFound 'Dashboard <name> does not exist'. The positive control in the same check shows a VALID body deployed the same way does reach CREATE_COMPLETE and round-trips through GetDashboard.
Emulator: the same stack settles at StackStatus CREATE_COMPLETE ('AssertionError: CREATE_COMPLETE' against `assert stack["StackStatus"] == "CREATE_FAILED"`), even though the identical body submitted directly to PutDashboard in the same check IS refused InvalidParameterInput with AWS's exact message (that leg passes). So the CloudFormation resource provider for AWS::CloudWatch::Dashboard does not call the dashboard write path, or ignores its refusal, and reports the resource as created.
Why: an IaC pipeline cannot tell a bad template from a working one — the deployment reports success for a dashboard that was never validated and (per the related open issues) never provisioned, so the failure surfaces only when an operator opens the dashboard. A rollback-on-failure policy never engages, and the StackStatusReason that would name the offending logical resource is never written.
Fix: the CloudFormation resource provider for AWS::CloudWatch::Dashboard — it must invoke the CloudWatch dashboard write path and map its DashboardInvalidInputError into a resource-create failure, so the stack lands CREATE_FAILED (OnFailure=DO_NOTHING) / rolls back, with the 'The following resource(s) failed to create: [<LogicalResourceId>]. ' reason, and leaves no dashboard behind.
Traces: cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the built emulator (101/412 passed). Neither check was marked ALREADY FILED or OWNED by the report.

Cause 1 — cloudwatch.put-dashboard-with-a-metric-widget-missing-metrics-rejected [mismatch]:
  File "check_cloudwatch_put_dashboard_with_a_metric_widget_missing_metrics_rejected.py", line 43, in run
    code, message = _error(lambda: cw.put_dashboard(DashboardName=broken, DashboardBody=json.dumps(bad_body)))
  File ... line 122, in _error
    raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code
Expected (AWS): code 'InvalidParameterInput' and message exactly "The dashboard body is invalid, there are 3 validation errors:\n" + json.dumps([{"dataPath": "/widgets/0/properties", "message": "Should have required property 'metrics'"}, {"dataPath": "/widgets/0/properties", "message": "The metric widget should have specified a region and a data source or an alarm annotation"}, {"dataPath": "/widgets/0/properties", "message": "Should have required property 'annotations'"}], indent=2).
Never reached because the call succeeded: GetDashboard on the refused name must answer ResourceNotFound 'Dashboard <name> does not exist'; and the valid two-widget body must return DashboardValidationMessages == [] and round-trip through GetDashboard.

Cause 2 — cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected [mismatch]:
  File "check_cloudwatch_deploy_cloudformation_invalid_dashboard_body_rejected.py", line 62, in run
    assert stack["StackStatus"] == "CREATE_FAILED", stack["StackStatus"]
  AssertionError: CREATE_COMPLETE
Passed first, in the same check: cw.put_dashboard(DashboardName=<name>, DashboardBody='{ this is not valid dashboard json ]') was refused with code 'InvalidParameterInput' and message exactly 'The field DashboardBody must be a valid JSON object'.
Never reached: StackStatusReason == 'The following resource(s) failed to create: [BadDashboard]. '; GetDashboard(<name>) -> ResourceNotFound 'Dashboard <name> does not exist'; and the valid-template positive control's body round-trip.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and point a botocore cloudwatch client at it.
2. cloudwatch:PutDashboard(DashboardName=<fresh name>, DashboardBody='{"widgets":[{"type":"metric","x":0,"y":0,"width":24,"height":6,"properties":{"title":"End-to-End Latency"}}]}').
   Expected: 400 InvalidParameterInput, message 'The dashboard body is invalid, there are 3 validation errors:' + newline + the indent=2 JSON array of {dataPath:'/widgets/0/properties', message:<problem>} entries listed above. Observed: 200, no error, the dashboard is stored.
3. cloudwatch:GetDashboard(DashboardName=<that name>). Expected: ResourceNotFound 'Dashboard <name> does not exist' (nothing was stored). Observed: the dashboard exists.
4. Control (must keep passing): PutDashboard with a two-widget body whose metric widgets carry region/view/stat/period/metrics (+ annotations) -> 200 with DashboardValidationMessages == [], and GetDashboard round-trips the body.

Cause 2:
1. cloudwatch:PutDashboard(DashboardName=<name>, DashboardBody='{ this is not valid dashboard json ]') -> 400 InvalidParameterInput 'The field DashboardBody must be a valid JSON object' (correct today).
2. cloudformation:CreateStack(StackName=<s>, OnFailure='DO_NOTHING', TemplateBody of a template whose only resource is logical id BadDashboard of Type AWS::CloudWatch::Dashboard with Properties {DashboardName: <name>, DashboardBody: '{ this is not valid dashboard json ]'}).
3. Poll cloudformation:DescribeStacks until the status is terminal. Expected: CREATE_FAILED with StackStatusReason 'The following resource(s) failed to create: [BadDashboard]. '. Observed: CREATE_COMPLETE.
4. cloudwatch:GetDashboard(DashboardName=<name>) -> expected ResourceNotFound 'Dashboard <name> does not exist'.
5. Positive control: the same flow with a valid text-widget body must reach CREATE_COMPLETE and GetDashboard must return that body.
6. Re-replay: cloudwatch.put-dashboard-with-a-metric-widget-missing-metrics-rejected, cloudwatch.deploy-cloudformation-invalid-dashboard-body-rejected.
