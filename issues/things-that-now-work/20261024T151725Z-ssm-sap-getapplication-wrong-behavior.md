Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# Application.AppRegistryArn is never assigned, so it is absent from every response and GetApplication by AppRegistryArn can never resolve an application

_Filed: 2026-10-24T15:17:25Z_
_Operation: GetApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: AWS Systems Manager for SAP records each registered application in AWS Service Catalog AppRegistry, so Application.AppRegistryArn (pattern ^arn:aws:servicecatalog:[a-z0-9:\/-]+$) is populated on the RegisterApplication/GetApplication response, and GetApplication can be called with that AppRegistryArn as the only identifier to resolve the application.

Actual: RegisterApplication hard-codes `app_registry_arn: None` on the stored record and nothing else ever writes it, so (a) the member is omitted from every GetApplication/RegisterApplication response, and (b) the AppRegistryArn lookup path in RegionState::resolve_application_id can never match any stored application — every request that identifies an application only by AppRegistryArn is refused with ValidationException ("No registered application found for account: ..., and application: <arn>"), including the ARN of an application this emulator itself registered.

## Evidence

Smithy model: Application$AppRegistryArn — "The Amazon Resource Name (ARN) of the Application Registry"; GetApplication input accepts AppRegistryArn as one of its three identifiers (a lookup key that only makes sense because the service assigns one). Handler/state: src/ops/register_application.rs sets `app_registry_arn: None` with the comment "Nothing registers an AppRegistry application here"; src/state.rs StoredApplication::view passes it straight through, and resolve_application_id's app_registry_arn branch admits it can never match. Tests: tests/all/application_lifecycle.rs only asserts that an UNMATCHED AppRegistryArn is refused (400), and tests/all/application_reference_refusals.rs likewise; no test asserts that a registered application carries an AppRegistryArn or is resolvable by it.

## Steps to Reproduce

1. POST /register-application with a valid HANA registration (ApplicationId=HDB-TEST)
2. POST /get-application {"ApplicationId":"HDB-TEST"} and inspect Application — no AppRegistryArn member is present, where the real service reports an arn:aws:servicecatalog:...:/applications/... value
3. POST /get-application {"AppRegistryArn":"<the AppRegistry ARN the service would have assigned>"} — the emulator answers 400 ValidationException because no stored application carries any AppRegistryArn
