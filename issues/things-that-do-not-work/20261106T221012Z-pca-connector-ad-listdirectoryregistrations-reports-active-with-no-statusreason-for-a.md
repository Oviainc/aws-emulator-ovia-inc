Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-ad-directory-registration-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-ad-directory-registration-lifecycle
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad|ListDirectoryRegistrations|2xx->2xx|$.DirectoryRegistrations[Arn="arn:aws:pca-connector-ad:us-west-2:000000000000:directory-registration/d-9067fedc21"].Status
Issue-Key: pca-connector-ad|ListDirectoryRegistrations|2xx->2xx|$.DirectoryRegistrations[Arn="arn:aws:pca-connector-ad:us-west-2:000000000000:directory-registration/d-9067fedc21"].StatusReason
Issue-Related: 20261030T160522Z-pca-connector-ad-directoryregistration-lifecycle-a-registration-for-a-nonexistent-directory.md
Issue-Commit: d35145ae6b4828dd610d0d9959197bb2d8dd734f

# ListDirectoryRegistrations reports ACTIVE with no StatusReason for a registration whose directory does not exist

> _Note: this issue was identified against the code base at commit `d35145ae6b4828dd610d0d9959197bb2d8dd734f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDirectoryRegistrations

## Description
One cause. Cause 1 of 1: the pca-connector-ad ListDirectoryRegistrations reporting path serves Status=ACTIVE and omits StatusReason for a DirectoryRegistration created against an Active Directory that does not exist, so the registration never reaches its terminal FAILED state on the collection read.

Service model: services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/smithy-model/com-amazonaws-pcaconnectorad-pcaconnectorad.json.

AWS: CreateDirectoryRegistration is asynchronous — Smithy: CreateDirectoryRegistration POST /directoryRegistrations, http code 202, input DirectoryId @required @pattern ^d-[0-9a-f]{10}$ — and does not verify inline that the named directory exists. The registration is created in CREATING and the service then tries to authorize AWS Private CA with that Active Directory. When the directory does not exist in Directory Service, the registration terminates in Status FAILED carrying a StatusReason, and EVERY reporting operation must show that same terminal state. Smithy: ListDirectoryRegistrations GET /directoryRegistrations, http code 200, output member DirectoryRegistrations -> DirectoryRegistrationList, paginated items=DirectoryRegistrations; DirectoryRegistration.Status -> DirectoryRegistrationStatus enum values [CREATING, ACTIVE, DELETING, FAILED]; DirectoryRegistration.StatusReason -> DirectoryRegistrationStatusReason enum values [DIRECTORY_ACCESS_DENIED, DIRECTORY_RESOURCE_NOT_FOUND, DIRECTORY_NOT_ACTIVE, DIRECTORY_NOT_REACHABLE, DIRECTORY_TYPE_NOT_SUPPORTED, INTERNAL_FAILURE], documented "Additional information about the directory registration status if the status is failed". The model enumerates DIRECTORY_RESOURCE_NOT_FOUND but does not bind it to a condition, so the specific pairing of an unknown directory id with StatusReason=DIRECTORY_RESOURCE_NOT_FOUND is recorded behavior only: the recording shows the listed entry settling to Status FAILED / StatusReason DIRECTORY_RESOURCE_NOT_FOUND. No error is involved — the call is a 200 on both sides, so there is no status, error code or error message in dispute here; only the reported resource state.

Emulator: the registration settles unconditionally from CREATING to ACTIVE and StatusReason is never populated. ListDirectoryRegistrations answered 200 with the registration's Status 'ACTIVE' and StatusReason absent, and the fixture's poll for a terminal FAILED gave up after 20 attempts still seeing 'ACTIVE'. The directory id is never checked against any directory the emulator knows, so the FAILED branch is unreachable for every input.

Why: a client that polls the collection to learn whether a registration became usable — wait until ACTIVE, or until FAILED and then surface StatusReason — is told the registration is healthy when the directory it named does not exist. On AWS the workflow fails fast with a diagnosable reason code; against the emulator it reports success and the caller proceeds to CreateConnector against a directory that can never be authorized, so the misconfiguration surfaces far later or not at all. Any client code that reads StatusReason, or that branches on FAILED, is dead code against the emulator and is never exercised.

Fix: the clock-owed state transition in the service's state store plus the create path that seeds it — services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/state.rs (the DirectoryRegistrationRecord settle routine, which only knows CREATING -> ACTIVE and defines no FAILED status constant) and services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/create_directory_registration.rs (the record it inserts always schedules an ACTIVE settle with status_reason unset). A registration whose DirectoryId names no directory the emulator knows must settle to FAILED with StatusReason DIRECTORY_RESOURCE_NOT_FOUND; the ListDirectoryRegistrations serializer itself then needs no change beyond emitting the corrected record, including the optional StatusReason member whenever it is set.

Related open issue, which does NOT own this replay key: 20261030T160522Z-pca-connector-ad-directoryregistration-lifecycle-a-registration-for-a-nonexistent-directory.md describes the same root cause and names this step in prose, but the replay report returned this cluster with an EMPTY owned_by, i.e. its replay key is not carried by that issue (that issue's recorded ListDirectoryRegistrations keys are $.DirectoryRegistrations[*].Status and a truncated $.DirectoryRegistrations[Arn="arn key, and do not cover this cluster's key set $.DirectoryRegistrations[0].Status + [Arn=...].Status + [Arn=...].StatusReason). This issue exists so the key is gated and re-replayed; whoever fixes either must close both.

Traces: pca-connector-ad-directory-registration-lifecycle#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, /__admin/health answered 200). Fixture pca-connector-ad-directory-registration-lifecycle, step task#2, operation ListDirectoryRegistrations, verdict mismatch. Expected status 200, actual status 200 — the divergence is entirely in the body.

Divergence detail as reported:
  $.DirectoryRegistrations[Arn="arn:aws:pca-connector-ad:us-west-2:000000000000:directory-registration/d-9067fedc21"].Status: value_mismatch (expected 'FAILED', got 'ACTIVE')
  $.DirectoryRegistrations[Arn="arn:aws:pca-connector-ad:us-west-2:000000000000:directory-registration/d-9067fedc21"].StatusReason: missing (expected 'DIRECTORY_RESOURCE_NOT_FOUND', got None)
  $.DirectoryRegistrations[0].Status: poll: not in ['FAILED'] after 20 attempt(s) (expected 'FAILED', got 'ACTIVE')

divergence_key: service=pca-connector-ad|ListDirectoryRegistrations|status=200->200|keys=$.DirectoryRegistrations[0].Status,$.DirectoryRegistrations[Arn="...directory-registration/d-9067fedc21"].Status,$.DirectoryRegistrations[Arn="..."].StatusReason
owned_by: (empty), stub_path: (empty) — this cluster is not covered by any open issue's replay key and is not blocked by an unimplemented operation.

The requirement is the pair of enum values (Status FAILED, StatusReason DIRECTORY_RESOURCE_NOT_FOUND) for a registration whose directory does not exist. The directory id d-9067fedc21, the account 000000000000, the region us-west-2, the ARN and the CreatedAt/UpdatedAt timestamps are this recording's values and are NOT part of the requirement.

Smithy evidence used: ListDirectoryRegistrations GET /directoryRegistrations http 200, output DirectoryRegistrations -> DirectoryRegistrationList, paginated items=DirectoryRegistrations; CreateDirectoryRegistration POST /directoryRegistrations http 202, DirectoryId @required @pattern ^d-[0-9a-f]{10}$; DirectoryRegistrationStatus enum [CREATING, ACTIVE, DELETING, FAILED]; DirectoryRegistrationStatusReason enum [DIRECTORY_ACCESS_DENIED, DIRECTORY_RESOURCE_NOT_FOUND, DIRECTORY_NOT_ACTIVE, DIRECTORY_NOT_REACHABLE, DIRECTORY_TYPE_NOT_SUPPORTED, INTERNAL_FAILURE]; DirectoryRegistration.StatusReason doc "Additional information about the directory registration status if the status is failed".

## Steps to Reproduce
Boot the emulator and point an AWS client at its pca-connector-ad endpoint (the recording used region us-west-2).

1. Call CreateDirectoryRegistration(DirectoryId="d-9067fedc21") — any id matching ^d-[0-9a-f]{10}$ that is NOT an Active Directory the environment knows. It returns HTTP 202 with DirectoryRegistrationArn arn:aws:pca-connector-ad:<region>:<account>:directory-registration/<that directory id>.
2. Poll ListDirectoryRegistrations() and read the entry whose Arn is the ARN from step 1, waiting for a terminal status.
   - AWS: the entry reaches Status=FAILED with StatusReason=DIRECTORY_RESOURCE_NOT_FOUND.
   - Emulator: the entry reaches Status=ACTIVE, omits StatusReason entirely, and never leaves ACTIVE (the fixture's poll exhausted 20 attempts).

Re-replay the recorded step directly to confirm a fix: pca-connector-ad-directory-registration-lifecycle#task#2.
