Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch.cloudwatch-dashboard-follows-template-through-lifecycle
Issue-Key: cloudformation__cloudwatch|GetDashboard|NotImplemented
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CloudWatch GetDashboard is an unimplemented scaffold stub, so a dashboard deployed by CloudFormation can never be read back (1 cause)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDashboard

## Description
Cause 1 of 1: cloudwatch GetDashboard answers NotImplemented for every request.

AWS: GetDashboard(DashboardName) returns the dashboard's DashboardArn, DashboardName and DashboardBody, and answers ResourceNotFound (404) for a name that does not exist. It is the read path that lets a caller confirm a PutDashboard — or a CloudFormation-deployed AWS::CloudWatch::Dashboard — landed, and see its body change as the template changes.
Emulator: every call is refused with HTTP 501-class 'NotImplemented' and the message 'operation GetDashboard is scaffolded but not implemented in this emulator', so the check aborts at its first read and the dashboard's whole lifecycle (create, template update, stack delete) is unobservable.
Why: a client cannot verify or diff a dashboard at all, and cannot distinguish 'dashboard absent' (ResourceNotFound) from 'service cannot answer' — the refusal is not one of the operation's modeled errors, so SDK error handling takes an unexpected branch.
Fix: implement the GetDashboard handler in the cloudwatch service crate (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/get_dashboard.rs) over the dashboard state that PutDashboard writes, including the ResourceNotFound refusal for an unknown name.
Traces: cloudformation__cloudwatch.cloudwatch-dashboard-follows-template-through-lifecycle.

## Evidence
Replayed suite, check cloudformation__cloudwatch.cloudwatch-dashboard-follows-template-through-lifecycle:
"botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetDashboard operation: operation GetDashboard is scaffolded but not implemented in this emulator"
The check never got past this call, so nothing downstream of it was exercised.

## Steps to Reproduce
1. Boot the emulator and replay the suite; re-run cloudformation__cloudwatch.cloudwatch-dashboard-follows-template-through-lifecycle.
2. Manual repro: cloudwatch PutDashboard(DashboardName='d', DashboardBody='{"widgets":[]}'), then GetDashboard(DashboardName='d') — AWS returns the body, the emulator answers NotImplemented.
3. Also call GetDashboard(DashboardName='no-such-dashboard') — AWS answers ResourceNotFound (404).
