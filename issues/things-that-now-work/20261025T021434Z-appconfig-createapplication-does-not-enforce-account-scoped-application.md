Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__appconfigdata.duplicate-application-name-rejected
Issue-Key: appconfig__appconfigdata.duplicate-application-name-rejected|_error#6|AssertionError|raise AssertionError(
Issue-Commit: 5d61882ef55489b75c810fe19d4c281a0249999c

# AppConfig CreateApplication does not enforce account-scoped application-name uniqueness, so a duplicate name is accepted

> _Note: this issue was identified against the code base at commit `5d61882ef55489b75c810fe19d4c281a0249999c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
One net-new root cause in the bootstrapped appconfig emulator, grounded in the behavioral check appconfig__appconfigdata.duplicate-application-name-rejected (its check id carries the composite leading token `appconfig__appconfigdata`; the failing call is CreateApplication, an appconfig control-plane operation, so the issue is filed under service `appconfig`). It is not an unimplemented-operation failure — CreateApplication is implemented and answered 2xx — and it is not covered by any open issue: 20261024T131534Z-appconfig-accepts-three-classes-of-invalid-configuration-experiment.md bundles the experiment-profile-type, feature-flag-constraint and JSON_SCHEMA-validator over-permissiveness causes but says nothing about application-name uniqueness, and 20261024T131534Z-appconfig-refusal-messages-on-the-tagging-pagination-hosted.md covers the WORDING of refusals that do happen, not a refusal that never happens.

=== Cause 1 of 1: a second application with an already-taken Name is created instead of refused ===
AWS: an AppConfig application Name is unique per account. CreateApplication with a Name already owned by an existing application in the account is refused with BadRequestException, HTTP 400, and no second application is created (a ListApplications filter on that name still returns exactly the original Id). The conflict is on the NAME alone: re-sending the same Name with a different Description is refused identically, and it is BadRequestException, not ConflictException. Smithy: CreateApplication POST /applications, success code 201; input Name @required @length(1,64), Description @length(0,1024), Tags @length(0,50); modeled errors BadRequestException (client, httpError 400), InternalServerException (500), ServiceQuotaExceededException (402) — the model constrains Name's length but states no uniqueness rule, so the uniqueness constraint itself is recorded behavior only. The recording's message is built from the request and from state — it names the submitted name, the existing application's Id and the account, e.g. 'Application with name <name> already exists with id <id> in account <account>' — so that FORM (naming the colliding name and the existing id) is the requirement; the literal text is an example.
Emulator: the duplicate CreateApplication SUCCEEDS (2xx, a new application document returned) instead of raising; the check's _error helper aborts with 'the call under test SUCCEEDED where real AWS refuses it — this is an over-permissiveness failure, not an error-code mismatch'. The emulator therefore allows two applications to own one name, and the later assertions (the identical refusal for a differing Description, and ListApplications still holding exactly one Id for that name) are never reached.
Why: every by-name lookup becomes ambiguous once two applications share a name — a client resolving an application by name gets a coin flip, and the idempotent-redeploy pattern that depends on this refusal (an IaC template that pins applicationName and is deployed twice after a partial failure) silently forks the estate into two applications instead of failing fast with the 400 the client handles. Negative tests asserting the 400 pass against AWS and fail against the emulator.
Fix: CreateApplication's validation path in the appconfig handler — before any state is written, look up the account's existing applications by Name and refuse with BadRequestException / HTTP 400 (message naming the submitted name and the existing application id) when the name is taken, leaving the application store unchanged.
Traces: appconfig__appconfigdata.duplicate-application-name-rejected.

## Evidence
Behavioral replay (simparity-behavioral-ga-prod-v1) against the booted emulator: 0/13 passed, bootstrap_signal not set (the emulator served the calls — applications were created, ListApplications answered, and other checks returned modeled ClientErrors).

[mismatch] appconfig__appconfigdata.duplicate-application-name-rejected:
  File "check_appconfig__appconfigdata_duplicate_application_name_rejected.py", line 188, in run
    code, message = _error(lambda: ac.create_application(Name=taken, Tags=_TAGS))
  File "check_appconfig__appconfigdata_duplicate_application_name_rejected.py", line 42, in _error
    raise AssertionError(...)
  AssertionError: the call under test SUCCEEDED where real AWS refuses it — this is an over-permissiveness failure, not an error-code mismatch

The check first established the precondition that exactly one application owned the name (ListApplications filtered on it returned exactly the newly created Id) — that assertion PASSED — and then re-sent CreateApplication with the same Name.

Expected (real AWS): ClientError with Error.Code 'BadRequestException', HTTP status 400, message naming the submitted name and the existing application's id/account; no new application; the same refusal when only Description differs.
Actual (emulator): CreateApplication returned success, so a second application with the taken name exists.

Generated names, application ids and the account number in the recording are per-run values and are examples, not requirements.

## Steps to Reproduce
Point an appconfig client at the emulator's data listener (region/credentials irrelevant).

1. name = <any unused name>; app = create_application(Name=name, Description='...', Tags={...}) -> note app['Id'].
2. Confirm the precondition: list_applications (paginated) filtered to Name == name returns exactly [app['Id']].
3. create_application(Name=name, Tags={...}) again.
   Emulator: succeeds (2xx, a second application document).
   AWS: ClientError BadRequestException, HTTP 400, message naming the existing application id for that name.
4. create_application(Name=name, Description='a different description entirely', Tags={...}).
   AWS: refused identically with BadRequestException 400 (the conflict is on the name alone).
5. list_applications filtered to Name == name must still return exactly [app['Id']] — on AWS it does; on the emulator step 3 added a second id.
6. Positive control: create_application with a FREE name must still succeed and get_application must describe it as itself, so the fix must refuse only the duplicate, not every create.

Re-replay appconfig__appconfigdata.duplicate-application-name-rejected to confirm the fix.
