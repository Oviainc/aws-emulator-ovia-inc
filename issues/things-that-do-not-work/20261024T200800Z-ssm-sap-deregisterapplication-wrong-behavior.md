Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# Deregistration deletes the application synchronously, so the DELETING status is never observable

_Filed: 2026-10-24T20:08:00Z_
_Operation: DeregisterApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: DeregisterApplication is an asynchronous control-plane call in AWS Systems Manager for SAP. The call returns 200 and the application enters the DELETING state while the deregistration runs: a GetApplication / ListApplications issued immediately afterwards still resolves the application and reports `Application.Status = DELETING` (and `DiscoveryStatus = DELETING`), and only once the deregistration completes does the application stop resolving. The emulator models every other asynchronous application transition this way already (REGISTERING->ACTIVATED, STARTING->ACTIVATED, STOPPING->STOPPED via crate::state::PendingTransition / TRANSITION_SPAN).

Actual: the handler removes the StoredApplication, its tags and every operation record filed against it inside the same request, and drops the Region partition if it becomes empty. The very next GetApplication answers 400 ValidationException ("No registered application found for account: ..., and application: ...") and ListApplications omits the application immediately. As a result the modeled ApplicationStatus$DELETING and ApplicationDiscoveryStatus$DELETING values are unreachable — no response this emulator can ever produce carries them — and the transient window a client polling for completion observes against the real service does not exist here.

Secondary symptom of the same class: unlike RegisterApplication / StartApplication / StopApplication, the deregistration files no StoredOperation at all, so nothing records that the application was deregistered (and the records of the application's earlier operations are destroyed, so an OperationId handed out moments earlier by StartApplication stops resolving through GetOperation after a deregister).

## Evidence

Smithy model: `ApplicationStatus` = [ACTIVATED, STARTING, STOPPED, STOPPING, FAILED, REGISTERING, DELETING, UNKNOWN] and `ApplicationDiscoveryStatus` = [SUCCESS, REGISTRATION_FAILED, REFRESH_FAILED, REGISTERING, DELETING] — DELETING in both enums can only be produced by a deregistration, so the model documents a deregistering application as something GetApplication/ListApplications still report. Handler: services/service-com-amazonaws-ssmsap-ssmsap/src/ops/deregister_application.rs lines ~162-179 — `region_state.applications.remove(...)`, `tags_by_arn.remove(...)`, `forget_operations_for(...)`, `forget_region_if_empty(...)` all inside the request, with no `PendingTransition` written and no `record_operation` call (compare src/ops/start_application.rs / stop_application.rs, which both set `pending` and file an operation record). src/state.rs defines APPLICATION_STATUS_* constants for REGISTERING/ACTIVATED/STARTING/STOPPING/STOPPED only — no DELETING constant exists. Tests: tests/all/application_lifecycle.rs::deregistering_removes_the_application_from_every_reader and tests/all/configuration_checks.rs::deregistering_an_application_forgets_its_configuration_checks pin the CURRENT immediate-removal behavior ("a deregistered application is gone" asserted in the request right after the deregister), so no test covers the DELETING window.

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"HDB","ApplicationType":"HANA","Instances":["i-1234567890abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"sap/HDB"}]} and let the registration settle to ACTIVATED
2. POST /start-application with {"ApplicationId":"HDB"} and keep the returned OperationId
3. POST /deregister-application with {"ApplicationId":"HDB"} (observe 200)
4. Immediately POST /get-application with {"ApplicationId":"HDB"}: the emulator answers 400 ValidationException, where the real service still returns the application with Status DELETING until the deregistration completes
5. POST /list-applications: the application is already absent rather than listed with DiscoveryStatus DELETING
6. POST /get-operation with the OperationId kept in step 2: the record has been destroyed and the call is refused with 400
