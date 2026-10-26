Issue-Severity: LOW
Issue-Gap-Class: event-propagation
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# IncludeEc2InstanceShutdown is recorded but never shuts down the registered EC2 instance

_Filed: 2026-10-24T06:28:49Z_
_Operation: StopApplication | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StopApplicationInput$IncludeEc2InstanceShutdown` is documented as: "Boolean. If included and if set to True, the StopApplication operation will shut down the associated Amazon EC2 instance in addition to the application." The handler only copies the flag (and `StopConnectedEntity`) into the stored operation's `Properties` map for read-back; it never acts on the EC2 instance ids the registration recorded (`StoredApplication.instances`), and no EC2 state in the emulator changes.

Expected: StopApplication with `IncludeEc2InstanceShutdown: true` stops the application AND transitions the EC2 instance(s) named at registration to a stopped state, observable through the EC2 emulator (DescribeInstances). Actual: the application reaches STOPPED and the operation record echoes the flag, while the instance named by `Instances` at registration is untouched — so a workflow that stops an SAP application and then checks its EC2 instance sees it still running.

## Evidence

Smithy model doc for `StopApplicationInput$IncludeEc2InstanceShutdown` (quoted above) ties the flag to shutting down the associated Amazon EC2 instance. Handler: services/service-com-amazonaws-ssmsap-ssmsap/src/ops/stop_application.rs lines ~205-223 inserts `IncludeEc2InstanceShutdown` into the operation `Properties` and does nothing else with it; `StoredApplication.instances` (src/state.rs) is stored but never read by any handler. Tests: tests/all/application_lifecycle.rs only asserts the flag appears in the operation record's `Properties`, not any instance-level effect.

## Steps to Reproduce

1. Register an application with Instances = ["i-1234567890abcdef0"] (an instance that exists in the EC2 emulator) and let it settle to ACTIVATED
2. POST /stop-application with {"ApplicationId":"<id>","IncludeEc2InstanceShutdown":true}
3. Poll GetOperation until SUCCESS and GetApplication until Status=STOPPED
4. Call EC2 DescribeInstances for i-1234567890abcdef0 and observe the instance is still `running` instead of `stopping`/`stopped`
