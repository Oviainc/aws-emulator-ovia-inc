Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch.cloudwatch-dashboard-follows-template-through-lifecycle
Issue-Key: cloudformation__cloudwatch|GetDashboard|ResourceNotFound
Issue-Commit: d2e421bd781628705310382509b1712cb5427402

# cloudformation__cloudwatch: AWS::CloudWatch::Dashboard is never provisioned — the stack reaches CREATE_COMPLETE and Ref resolves to the dashboard name, but cloudwatch:GetDashboard answers ResourceNotFound (1 cause)

> _Note: this issue was identified against the code base at commit `d2e421bd781628705310382509b1712cb5427402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack declaring AWS::CloudWatch::Dashboard reports CREATE_COMPLETE but no dashboard exists at CloudWatch.

AWS: recorded behavior only (AWS::CloudWatch::Dashboard is a CloudFormation resource type, not a Smithy-modelled operation; the provisioning call it maps to is cloudwatch PutDashboard, and the read-back is cloudwatch GetDashboard, whose modelled error for an absent dashboard is ResourceNotFound). A stack whose template declares Dashboard{Type: AWS::CloudWatch::Dashboard, Properties:{DashboardName:{Ref:DashboardName}, DashboardBody:<json widgets>, Tags:[{Key:team,Value:{Ref:Team}}]}} must, on reaching CREATE_COMPLETE, have actually created the dashboard: cloudwatch:GetDashboard(DashboardName=<the Ref'd name>) returns 200 with DashboardBody carrying the declared widget and DashboardArn set, and cloudwatch:ListTagsForResource(ResourceARN=<that arn>) returns the declared tag. A body-or-tag-only change must update the SAME dashboard in place (Ref unchanged); a DashboardName change must create the new dashboard and DELETE the displaced one; DeleteStack must remove whichever dashboard currently exists.
Emulator: the stack reaches CREATE_COMPLETE and the stack output Ref resolves correctly to the requested dashboard name, but the very next call fails: cloudwatch:GetDashboard(DashboardName=<that name>) answers error code `ResourceNotFound` with message `Dashboard <name> does not exist`. The CloudFormation resource handler for AWS::CloudWatch::Dashboard never issues the PutDashboard that would create it, so none of the update/replacement/delete lifecycle can be reached either.
Why: a client that deploys its dashboards as part of a stack is told the deployment succeeded while the dashboards do not exist; every subsequent read, tag or update against the deployed dashboard 404s, and the replacement-deletes-the-displaced-dashboard contract can never be observed. CREATE_COMPLETE on a stack whose resources were never created is the strongest possible false positive.
Fix: the CloudFormation resource provisioner for AWS::CloudWatch::Dashboard (create/update/delete handlers) must drive cloudwatch PutDashboard / DeleteDashboards and the tag API, and the stack engine must only report CREATE_COMPLETE once that call succeeded. Note this is NOT the already-filed stub pair 20261025T020208Z-cloudwatch-getdashboard-stub.md / 20261025T015306Z-cloudwatch-getdashboard-is-an-unimplemented-scaffold-stub-so.md: GetDashboard now answers a real modelled ResourceNotFound rather than NotImplemented, so the surviving defect is the missing CFN provisioning, not the read op.
Traces: cloudformation__cloudwatch.cloudwatch-dashboard-follows-template-through-lifecycle.

## Evidence
Check cloudformation__cloudwatch.cloudwatch-dashboard-follows-template-through-lifecycle
  Passed: cfn:DescribeStacks StackStatus == CREATE_COMPLETE, and Outputs['DashboardRef'] == the requested dashboard name.
  Expected next: cloudwatch:GetDashboard(DashboardName=<that name>) -> 200 with DashboardBody whose widgets[0].properties.markdown == "Requests" and a DashboardArn, then ListTagsForResource -> team=platform.
  Actual: botocore.errorfactory.ResourceNotFound: An error occurred (ResourceNotFound) when calling the GetDashboard operation: Dashboard sim2real-xsvc-cwdash-v1-<id> does not exist
  (the -<id> suffix is a per-run generated name; the divergence is that no dashboard exists for a CREATE_COMPLETE stack.)

## Steps to Reproduce
1. cfn:CreateStack(StackName=S, Parameters=[{DashboardName: D},{Team: platform}], TemplateBody={Parameters:{DashboardName:{Type:String},Team:{Type:String}}, Resources:{Dashboard:{Type:"AWS::CloudWatch::Dashboard", Properties:{DashboardName:{Ref:DashboardName}, DashboardBody:"{\"widgets\":[{\"type\":\"text\",\"properties\":{\"markdown\":\"Requests\"}}]}", Tags:[{Key:team,Value:{Ref:Team}}]}}}, Outputs:{DashboardRef:{Value:{Ref:Dashboard}}}}).
2. Poll cfn:DescribeStacks until terminal — observe CREATE_COMPLETE and Outputs.DashboardRef == D.
3. Call cloudwatch:GetDashboard(DashboardName=D).
4. Observe ResourceNotFound "Dashboard D does not exist"; AWS returns the dashboard body and ARN.
