Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.resource-policy-enforced-on-put-rum-events rum.e2e-resource-policy-access-lifecycle rum.update-app-monitor-cw-log-role-denied rum.server-side-cw-log-enablement-enforced rum.deploy-app-monitor-cfn-behaves
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum.resource-policy-enforced-on-put-rum-events|_poll_until#13|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Key: rum.e2e-resource-policy-access-lifecycle|_client_error#17|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: rum.update-app-monitor-cw-log-role-denied|_poll_refused#8|AssertionError|raise AssertionError(f"expected a settled refusal within {timeout_s}s, last={last!r}")
Issue-Key: rum.server-side-cw-log-enablement-enforced|run#14|AssertionError|assert matching, f"RUM advertised CwLogGroup {log_group!r} but it does not exist in CloudWatch Logs"
Issue-Key: rum.deploy-app-monitor-cfn-behaves|_deploy_cfn#3|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# RUM performs no authorization evaluation and does not provision managed side effects: unprivileged PutRumEvents/UpdateAppMonitor succeed, CwLogGroup is advertised but never created, and CFN stack outputs omit OutputValue

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRumEvents

## Description
Four distinct root causes in the rum emulator's server-side enforcement and managed side effects. All are in one service (rum). A fixer must close ALL FOUR before submitting.

=== Cause 1 of 4: PutRumEvents is accepted from a principal with no identity-based and no resource-policy permission ===
AWS: Smithy: PutRumEvents models AccessDeniedException (httpError 403). Recorded behavior: a same-account IAM role holding zero rum permissions, calling PutRumEvents against another principal's app monitor with no RUM resource policy granting it, is refused with an AccessDenied-family error (HTTP 403); it succeeds only after a resource policy on the app monitor grants that principal rum:PutRumEvents, and is refused again once the policy is deleted — i.e. the decision is evaluated server-side on every call.
Emulator: the call from the bare assumed role succeeds. The replay records no error at all within 45s of polling (the error-code probe keeps returning None, i.e. HTTP 200), both before any grant exists and in the resource-policy lifecycle check.
Why: the data plane is wide open. Any caller that can reach the endpoint can write events into any app monitor, so an authorization regression — a missing grant, a revoked policy, an over-broad policy — cannot be detected against the emulator; code that depends on RUM refusing an unauthorized unsigned/foreign-principal write has its negative path untested, and a revoked grant appears to still work.
Fix: authorization evaluation for the PutRumEvents handler — resolve the calling principal and evaluate identity-based permission plus the app monitor's resource policy, denying with AccessDeniedException/403 when neither allows rum:PutRumEvents, and re-evaluating per call rather than caching at grant time.
Traces: rum.resource-policy-enforced-on-put-rum-events, rum.e2e-resource-policy-access-lifecycle.

=== Cause 2 of 4: UpdateAppMonitor(CwLogEnabled=True) does not require the caller's logs:PutResourcePolicy permission ===
AWS: Smithy: UpdateAppMonitor (PATCH /appmonitor/{Name}) models AccessDeniedException (httpError 403). Recorded behavior: enabling CloudWatch Logs delivery on an app monitor requires the caller to hold logs:PutResourcePolicy, because RUM must install a log-group resource policy on the caller's behalf. A caller allowed rum:* but explicitly denied logs:PutResourcePolicy is refused with AccessDeniedException / HTTP 403, and CwLogEnabled does not persist; the recorded message is "Error updating the application <Name>" (built from the request's Name member). An unrestricted caller can still enable it on the same app monitor.
Emulator: the denied caller's UpdateAppMonitor(Name=..., CwLogEnabled=True) returns HTTP 200 with an empty body and the change takes effect — no refusal ever settles within 45s.
Why: a cross-service permission precondition is skipped, so the emulator accepts a configuration change real AWS rejects. Deployments that would fail in AWS for a missing logs permission appear to succeed, and the resulting state (CwLogEnabled=true) is unreachable in AWS for that principal.
Fix: the UpdateAppMonitor handler's CwLogEnabled transition — gate the enable path on the caller's logs:PutResourcePolicy permission and deny with AccessDeniedException/403 without mutating state.
Traces: rum.update-app-monitor-cw-log-role-denied.

=== Cause 3 of 4: enabling CwLogEnabled advertises a CwLogGroup that is never provisioned in CloudWatch Logs ===
AWS: recorded behavior only (the RUM model carries no cross-service binding). After UpdateAppMonitor(CwLogEnabled=True), GetAppMonitor reports DataStorage.CwLog.CwLogEnabled=true and a CwLogGroup name, and the managed service itself provisions that log group so it is a real CloudWatch Logs resource — logs:DescribeLogGroups with logGroupNamePrefix=<that name> returns a logGroups entry whose logGroupName equals it exactly, without the client ever calling logs:CreateLogGroup.
Emulator: GetAppMonitor returns a CwLogGroup name of the expected managed form (e.g. '/aws/vendedlogs/RUMService_<app monitor name>'), but logs.describe_log_groups finds no log group with that name — the string is config-only.
Why: the advertised name is a dangling reference. A client that reads CwLogGroup from GetAppMonitor and then queries/tails/sets retention on it gets a non-existent-resource error against the emulator, while the same code works against AWS; log-delivery wiring cannot be exercised end to end.
Fix: the UpdateAppMonitor handler's CwLogEnabled side effect (the RUM-managed state that backs DataStorage.CwLog) — create the vended log group in the CloudWatch Logs state when delivery is enabled, so DescribeLogGroups resolves the name RUM reports.
Traces: rum.server-side-cw-log-enablement-enforced.

=== Cause 4 of 4: CloudFormation stack Outputs for an AWS::RUM::AppMonitor stack contain no OutputValue ===
AWS: recorded behavior only. A stack whose template declares Outputs: AppMonitorId: { Value: !GetAtt AppMonitor.Id } over a Type: AWS::RUM::AppMonitor resource reaches CREATE_COMPLETE, and DescribeStacks returns each Outputs entry with BOTH OutputKey and OutputValue, the OutputValue being the resolved attribute (the app monitor's Id, which equals GetAppMonitor(...).AppMonitor.Id).
Emulator: the Outputs entries are returned without an OutputValue member — reading o['OutputValue'] raises KeyError: 'OutputValue' — so the !GetAtt AppMonitor.Id attribute is never resolved into the stack output.
Why: any consumer of this stack's outputs (another stack's Fn::ImportValue, a deploy script, a CDK/CFN-driven test) cannot obtain the app monitor id, so IaC-driven provisioning of RUM breaks at the output-consumption step even though the stack reports success.
Fix: the AWS::RUM::AppMonitor resource provider's GetAtt attribute resolution and the stack-output serialization that carries it into DescribeStacks Outputs (the output must carry OutputKey AND OutputValue).
Traces: rum.deploy-app-monitor-cfn-behaves.

## Evidence
Behavioral suite: rum 2/77 passed (3%). Per-cause expected vs actual, verbatim from replay:

Cause 1 — rum.resource-policy-enforced-on-put-rum-events: expected an AccessDenied-family error from PutRumEvents by a bare assumed role before any grant; actual AssertionError: condition not met within 45.0s (last=None) — the error-code probe never saw any error, i.e. the call kept returning success. rum.e2e-resource-policy-access-lifecycle: AssertionError: expected a rejection, but the call succeeded and returned None (PutRumEvents by a zero-permission role with no resource policy in place).

Cause 2 — rum.update-app-monitor-cw-log-role-denied: expected AccessDeniedException with message 'Error updating the application <name>'; actual AssertionError: expected a settled refusal within 45.0s, last={'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}, 'RetryAttempts': 0}} — the role denied logs:PutResourcePolicy got HTTP 200.

Cause 3 — rum.server-side-cw-log-enablement-enforced: AssertionError: RUM advertised CwLogGroup '/aws/vendedlogs/RUMService_sim2real-rum-cwlog-b61d4bf7b0de' but it does not exist in CloudWatch Logs (logs.describe_log_groups(logGroupNamePrefix=<that name>) returned no matching logGroupName).

Cause 4 — rum.deploy-app-monitor-cfn-behaves: KeyError: 'OutputValue' raised while building {o['OutputKey']: o['OutputValue'] for o in stack.get('Outputs', [])} from DescribeStacks of a stack with Outputs: AppMonitorId: Value: !GetAtt AppMonitor.Id.

## Steps to Reproduce
Boot the emulator; use botocore clients for rum, iam, sts, logs and cloudformation.
1) Cause 1: iam.create_role with a trust policy allowing arn:aws:iam::<account>:root and NO permission policy; sts.assume_role it; build a rum client from those credentials; rum.create_app_monitor(Name=<n>, Domain='example.com') as the normal caller; then call put_rum_events(Id=<that monitor's Id>, BatchId=uuid4, AppMonitorDetails={}, UserDetails={...}, RumEvents=[{...}]) with the bare-role client -> succeeds (HTTP 200); expected an AccessDenied-family error/403.
2) Cause 2: create an app monitor; create a role allowing 'rum:*' and explicitly denying 'logs:PutResourcePolicy'; assume it; call update_app_monitor(Name=<n>, CwLogEnabled=True) with those credentials -> HTTP 200 and the change persists; expected AccessDeniedException/403 with CwLogEnabled unchanged.
3) Cause 3: create an app monitor; rum.update_app_monitor(Name=<n>, CwLogEnabled=True); poll get_app_monitor until DataStorage.CwLog.CwLogGroup is populated; then logs.describe_log_groups(logGroupNamePrefix=<that CwLogGroup>) -> no matching logGroupName; expected a real log group with exactly that name.
4) Cause 4: cloudformation.create_stack with a template declaring Type: AWS::RUM::AppMonitor (Name/Domain: localhost/CwLogEnabled: false) and Outputs: AppMonitorId: { Value: !GetAtt AppMonitor.Id }; wait for CREATE_COMPLETE; describe_stacks and read Stacks[0]['Outputs'][0] -> no 'OutputValue' key.
Or replay the checks listed in trace_ids.
