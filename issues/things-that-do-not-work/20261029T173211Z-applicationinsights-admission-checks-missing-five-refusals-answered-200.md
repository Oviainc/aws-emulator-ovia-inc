Issue-Source: trace-sweep-divergence
Issue-Trace: application-insights-application-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-insights-application-error-paths application-insights-component-configuration-lifecycle application-insights-list-applications-reads
Issue-Shape-Id: com.amazonaws.applicationinsights#EC2WindowsBarleyService
Issue-Key: applicationinsights|CreateApplication|4xx->2xx|$.Message
Issue-Key: applicationinsights|CreateApplication|4xx->2xx|$.__type
Issue-Key: applicationinsights|CreateApplication|4xx->2xx|status
Issue-Key: applicationinsights|DeleteApplication|4xx->2xx|$
Issue-Key: applicationinsights|DeleteApplication|4xx->2xx|status
Issue-Key: applicationinsights|CreateComponent|4xx->2xx|$
Issue-Key: applicationinsights|CreateComponent|4xx->2xx|status
Issue-Key: applicationinsights|DescribeComponent|4xx->2xx|$.Message
Issue-Key: applicationinsights|DescribeComponent|4xx->2xx|$.__type
Issue-Key: applicationinsights|DescribeComponent|4xx->2xx|status
Issue-Key: applicationinsights|ListApplications|4xx->2xx|$.Message
Issue-Key: applicationinsights|ListApplications|4xx->2xx|$.__type
Issue-Key: applicationinsights|ListApplications|4xx->2xx|status
Issue-Commit: 04aee419340dca63f4b07449b9fd0b6ed9270cf9

# applicationinsights admission checks missing: five refusals answered 200 success

> _Note: this issue was identified against the code base at commit `04aee419340dca63f4b07449b9fd0b6ed9270cf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
This issue bundles 5 net-new causes that share ONE root fault: the applicationinsights handlers for these operations perform no admission/existence check at all. They mutate or read state unconditionally and serialize a success response, so a request AWS refuses with a modeled 400-class error is answered HTTP 200 (and in two cases with an entirely empty JSON body). A client therefore believes a resource exists, or was created/deleted, when nothing of the kind happened. All five causes must be closed before this issue is submitted.

Cause 1 of 5: CreateApplication against a resource group that does not exist.
AWS: Application Insights resolves ResourceGroupName against Resource Groups before creating the application and refuses an unknown group. Smithy: CreateApplication errors include ResourceNotFoundException (structure with a single Message member, doc 'The resource does not exist in the customer account.'). The recorded refusal is HTTP 400 with body {"__type":"ResourceNotFoundException","Message":"Resource Group with name <resourceGroupName> does not exist in <accountId>"}. Fixed text: 'Resource Group with name ' ... ' does not exist in ' — variable parts are the ResourceGroupName taken from the request and the caller's 12-digit account id; the recording's particular group name and 000000000000 are one run's values, not part of the template.
Emulator: answers HTTP 200 with a success payload; $.__type and $.Message are both absent.
Why: a client that creates an application for a group it never provisioned gets a success and proceeds to configure components against an application AWS would never have created; the error path is untestable.
Fix: CreateApplication's handler/validation in the applicationinsights service crate — resolve the resource group in state before the create and raise ResourceNotFoundException.
Traces: application-insights-application-error-paths#task#2.

Cause 2 of 5: DeleteApplication for an application that is not being monitored.
AWS: DeleteApplication removes an application from monitoring and refuses when there is no monitored application for the named resource group. Smithy: DeleteApplication errors include ResourceNotFoundException and BadRequestException. Recorded refusal is HTTP 400 with body {"__type":"ResourceNotFoundException","Message":"Application does not exist"} — the message is entirely fixed text with no variable part.
Emulator: answers HTTP 200 with an empty JSON object ({}) — no __type, no Message.
Why: delete looks idempotent/successful, so a client cannot distinguish 'removed from monitoring' from 'was never monitored', and cleanup logic that expects the refusal silently passes.
Fix: DeleteApplication's handler in the applicationinsights service crate — look the application up in state and refuse when absent.
Traces: application-insights-application-error-paths#task#4.

Cause 3 of 5: CreateComponent grouping resources that are not EC2 instances.
AWS: a custom component may only group EC2 instances; a resourceList naming any other resource type is refused. Smithy: CreateComponent.ResourceList is required, targets ResourceList whose member is an ARN (length 1..1011, pattern ^arn:aws(-\w+)*:[\w\d-]+:([\w\d-]*)?:[\w\d_-]*([:/].+)*$); the type restriction itself is recorded behavior only, enforced beyond the ARN syntax check. Recorded refusal is HTTP 400 with body {"__type":"ValidationException","Message":"Only EC2 Instance can be grouped."} — wholly fixed text, including the trailing period.
Emulator: answers HTTP 200 with an empty JSON object and records the component.
Why: the emulator admits custom components AWS rejects, so configuration flows that must handle this refusal never see it and the emulator's component inventory contains groupings real Application Insights cannot hold.
Fix: CreateComponent's validation in the applicationinsights service crate — classify each resourceList ARN and refuse non-EC2-instance members with ValidationException.
Traces: application-insights-component-configuration-lifecycle#task#10.

Cause 4 of 5: DescribeComponent for a custom component that does not exist in an existing application.
AWS: DescribeComponent resolves (ResourceGroupName, ComponentName) and refuses an unknown component even when the application exists. Smithy: DescribeComponent errors include ResourceNotFoundException. Recorded refusal is HTTP 400 with body {"__type":"ResourceNotFoundException","Message":"Custom component <componentName> does not exist for <resourceGroupName>"}. Fixed text: 'Custom component ' ... ' does not exist for ' — the component name and resource-group name are taken verbatim from the request.
Emulator: answers HTTP 200; $.__type and $.Message are both missing from the body.
Why: a describe of a component that was never created reports success, so a client treats a missing component as present and reads whatever defaults the emulator serialized.
Fix: DescribeComponent's handler in the applicationinsights service crate — fail the component lookup instead of falling through to a success response.
Traces: application-insights-component-configuration-lifecycle#task#11.

Cause 5 of 5: ListApplications with an AccountId that is not the caller's.
AWS: the optional AccountId selects the resource-group owner and must be an account the caller may read; a foreign account is refused. Smithy: ListApplications.AccountId has length 12..12 and pattern ^\d{12}$; ValidationException is a service-common error for every operation. Recorded refusal is HTTP 400 with body {"__type":"ValidationException","Message":"Account <callerAccountId> is not able to access applications that belong to account <requestedAccountId>"}. Fixed text: 'Account ' ... ' is not able to access applications that belong to account ' — the two ids are the caller's account and the request's AccountId respectively.
Emulator: answers HTTP 200 with an application list; $.__type and $.Message absent.
Why: cross-account isolation is not modeled — a client asking for another account's applications gets a 200 and the local account's data, which hides an authorization error it must handle.
Fix: ListApplications' validation in the applicationinsights service crate — compare the request AccountId against the caller's account and raise ValidationException on mismatch.
Traces: application-insights-list-applications-reads#task#2.

## Evidence
Replay of simparity-wire-aws-v1 (trace set bound to this run), 5 clusters:

1. application-insights-application-error-paths#task#2 CreateApplication — expected 400, got 200; $.__type missing (expected 'ResourceNotFoundException'); $.Message missing (expected 'Resource Group with name ... does not exist in <account>').
2. application-insights-application-error-paths#task#4 DeleteApplication — expected 400, got 200; empty_body (expected {'__type': 'ResourceNotFoundException', 'Message': 'Application does not exist'}, got {}).
3. application-insights-component-configuration-lifecycle#task#10 CreateComponent — expected 400, got 200; empty_body (expected {'__type': 'ValidationException', 'Message': 'Only EC2 Instance can be grouped.'}, got {}).
4. application-insights-component-configuration-lifecycle#task#11 DescribeComponent — expected 400, got 200; $.__type missing (expected 'ResourceNotFoundException'); $.Message missing (expected 'Custom component <name> does not exist for <resourceGroupName>').
5. application-insights-list-applications-reads#task#2 ListApplications — expected 400, got 200; $.__type missing (expected 'ValidationException'); $.Message missing (expected 'Account <caller> is not able to access applications that belong to account <requested>').

In every cause the expected response is a modeled 400-class refusal and the actual is a 2xx success, i.e. no check ran at all.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
2. Cause 1: fixture application-insights-application-error-paths, step task#2 — CreateApplication naming a resource group that was never created. Expect 400 ResourceNotFoundException; observe 200.
3. Cause 2: same fixture, step task#4 — DeleteApplication for that unmonitored application. Expect 400 ResourceNotFoundException 'Application does not exist'; observe 200 {}.
4. Cause 3: fixture application-insights-component-configuration-lifecycle, step task#10 — CreateComponent with a resourceList of non-EC2-instance ARNs. Expect 400 ValidationException 'Only EC2 Instance can be grouped.'; observe 200 {}.
5. Cause 4: same fixture, step task#11 — DescribeComponent for a custom component name that was never created in that existing application. Expect 400 ResourceNotFoundException; observe 200.
6. Cause 5: fixture application-insights-list-applications-reads, step task#2 — ListApplications with AccountId set to an account other than the caller's. Expect 400 ValidationException; observe 200.
