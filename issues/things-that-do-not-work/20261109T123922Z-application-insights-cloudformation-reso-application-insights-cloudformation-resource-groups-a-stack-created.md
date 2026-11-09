Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-round-trip-and-delete application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-missing-resource-group-rejected
Issue-Key: application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-round-trip-and-delete|run#13|AssertionError|assert listed == [group_arn], f"ListGroups found {listed!r}, expected {group_arn!r}"
Issue-Key: application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-missing-resource-group-rejected|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# application-insights__cloudformation__resource-groups: a stack-created Resource Groups group is never registered with Resource Groups, and an Application naming a missing resource group is provisioned instead of rolling the stack back

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two independent causes behind the 2 failing application-insights__cloudformation__resource-groups checks. The stacks reach a terminal status, so these are not registry gaps.

Cause 1 of 2: a Resource Groups group created by a stack is not visible through ListGroups.
AWS: recorded behavior only — once a stack declaring AWS::ResourceGroups::Group reaches CREATE_COMPLETE, ListGroups returns the group, with the ARN `arn:aws:resource-groups:<region>:<account>:group/<group-name>` built from the request's group name; the Application Insights application declared over it then resolves its member resources.
Emulator: ListGroups answers `[]` for the group the stack just created, so the group the Application Insights application points at does not exist from the Resource Groups API's point of view.
Why: a client cannot read, tag or query a resource group a template created, and nothing that resolves an application's membership through Resource Groups can work.
Fix: the AWS::ResourceGroups::Group resource handler's create path — it must call the resource-groups crate's CreateGroup (or write the same state) so ListGroups/GetGroup return it.
Traces: application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-round-trip-and-delete

Cause 2 of 2: an AWS::ApplicationInsights::Application naming a resource group that does not exist is created instead of failing the stack.
AWS: recorded behavior only — an application whose ResourceGroupName names no existing group fails to create and the stack settles ROLLBACK_COMPLETE with Application Insights' own not-found refusal as the resource failure reason.
Emulator: the stack settles 'CREATE_COMPLETE'.
Why: a client gets an application bound to no resource group — a resource AWS would never have created — and the dangling reference is never reported, so a template typo deploys silently.
Fix: the AWS::ApplicationInsights::Application resource handler's create path (resolve ResourceGroupName against resource-groups state) and the rollback that failure must drive.
Traces: application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-missing-resource-group-rejected

## Evidence
application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-round-trip-and-delete — AssertionError: ListGroups found [], expected 'arn:aws:resource-groups:<region>:<account>:group/<group-name>'

application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-missing-resource-group-rejected — AssertionError: rejected stack ended 'CREATE_COMPLETE'  (expected ROLLBACK_COMPLETE)

## Steps to Reproduce
1. Cause 1: CreateStack with a template declaring AWS::ResourceGroups::Group and AWS::ApplicationInsights::Application over it; wait for CREATE_COMPLETE; call ListGroups and observe an empty list.
2. Cause 2: CreateStack with an AWS::ApplicationInsights::Application whose ResourceGroupName names no existing group; expect ROLLBACK_COMPLETE, observe CREATE_COMPLETE.
3. Re-replay both checks in trace_ids.
