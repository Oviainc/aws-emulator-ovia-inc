Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Registration never derives AppRegistryArn or discovered components, so Application is reported without them

_Filed: 2026-10-24T06:28:49Z_
_Operation: RegisterApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

RegisterApplication persists only what the request literally carried. Two pieces of state the real service derives at registration are never produced:

1. `AppRegistryArn`. AWS Systems Manager for SAP registers every SAP application in AWS Service Catalog AppRegistry, and the resulting `arn:aws:servicecatalog:<region>:<account>:/applications/<id>` is reported as `Application$AppRegistryArn` by RegisterApplication/GetApplication and is accepted as a lookup key by `GetApplicationInput$AppRegistryArn`. The emulator hardcodes `app_registry_arn: None` (src/ops/register_application.rs: `// Nothing registers an AppRegistry application here`), so the member is absent from every response and `GetApplication{AppRegistryArn=...}` can never resolve an application — it always answers 400 ValidationException ("No registered application found ..."), making that whole input member dead.

2. Discovered components. `Application$Components` is documented as "The components of the application" and the service discovers them from the registered EC2 instance(s) (a HANA registration yields the HANA database component; that is the point of the service's discovery phase, signalled by `DiscoveryStatus: REGISTERING -> SUCCESS`). The emulator derives component ids ONLY from the optional `ComponentsInfo` entries (`format!("{}-{}", component.sid, component.component_type)`), so a normal HANA registration (which carries no `ComponentsInfo`) settles to `DiscoveryStatus: SUCCESS` with `Components: []` — a successful discovery that discovered nothing.

Expected: after a HANA registration settles, `GetApplication` reports a non-empty `Components` list and an `AppRegistryArn`, and `GetApplication` by that AppRegistry ARN resolves the same application. Actual: `Components` is empty unless the caller described components itself, and `AppRegistryArn` is never present or resolvable.

## Evidence

Smithy model: `Application$AppRegistryArn` (pattern ^arn:aws:servicecatalog:[a-z0-9:\/-]+$) and `Application$Components` (ComponentIdList) exist, and `GetApplicationInput` accepts `AppRegistryArn` as one of three ways to name an application — a key the service can only honour if it assigns one. Handler: services/service-com-amazonaws-ssmsap-ssmsap/src/ops/register_application.rs lines ~249-256 (`app_registry_arn: None`, components built from `input.components_info` only); src/state.rs `resolve_application_id` comments that "Nothing in this slice registers an application through AppRegistry, so no stored application carries an AppRegistryArn to match". Tests: tests/all/application_lifecycle.rs asserts the CURRENT behavior (an AppRegistry ARN "resolves to nothing", components equal `expected_components()` derived from ComponentsInfo), so no test covers the real-service behavior.

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"HDB","ApplicationType":"HANA","Instances":["i-1234567890abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"sap/HDB"}]} (no ComponentsInfo)
2. Wait for the registration to settle, then POST /get-application with {"ApplicationId":"HDB"}
3. Observe `Application.Components` is `[]` and `Application.AppRegistryArn` is absent, while `DiscoveryStatus` is SUCCESS
4. POST /get-application with {"AppRegistryArn":"arn:aws:servicecatalog:us-east-1:<account>:/applications/<any>"} and observe it can never resolve any application (400 ValidationException)
