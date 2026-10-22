Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9e4273988d175f89e8a017aa0737ee7107d62a89

# Bootstrap the ssm-sap emulator

_Filed: 2026-10-21T23:47:47Z_
_Operation: service-com-amazonaws-ssmsap-ssmsap | Class: missing_

> _Note: this issue was identified against the code base at commit `9e4273988d175f89e8a017aa0737ee7107d62a89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 27 handlers in `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a first, small, stateful slice covering the primary resource (an SAP Application registered with AWS Systems Manager for SAP) so a realistic client script runs end-to-end.

### Happy path to implement (in order)

1. **RegisterApplication** (`POST /register-application`) — input `ApplicationId` (required, 1..60, `^[\w\d\.-]+$`), `ApplicationType` (required, `HANA|SAP_ABAP`), `Instances` (required, exactly 1 item), optional `Sid`, `SapInstanceNumber`, `Tags`, `Credentials`, `DatabaseArn`, `ComponentsInfo`. Must create and PERSIST the application, then return `Application` (with `Id`, `Type`, `Arn` = `arn:aws:ssm-sap:<region>:<account>:application/<ApplicationId>`, `Status`, `DiscoveryStatus`, `Components`, `LastUpdated`) and a freshly generated `OperationId` (UUID matching `^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$`) that is also recorded as a retrievable Operation record. Re-registering an existing `ApplicationId` must raise `ConflictException` (409); malformed id/type/Instances must raise `ValidationException` (400).

2. **GetApplication** (`POST /get-application`) — resolve by any of `ApplicationId`, `ApplicationArn`, or `AppRegistryArn` against stored state and return the SAME `Application` that was registered plus its `Tags`. Must not echo the request or return a canned body; an unknown id must not return a fabricated application.

3. **ListApplications** (`POST /list-applications`) — return one `ApplicationSummary` (`Id`, `Arn`, `Type`, `DiscoveryStatus`, `Tags`) per registered application, computed from state. Honour `MaxResults` (1..50, default 50) and round-trip `NextToken` (pattern `^.{16,2048}$`) so paging across the stored set terminates, and apply `Filters` when supplied.

4. **StartApplication** / **StopApplication** (`POST /start-application`, `POST /stop-application`) — input `ApplicationId` (required); `StopApplication` also takes `StopConnectedEntity` and `IncludeEc2InstanceShutdown`. Each must transition the stored application's `Status` (e.g. STARTING→ACTIVE, STOPPING→STOPPED), update `LastUpdated`, create a persisted Operation record (`Type` = START_APPLICATION / STOP_APPLICATION, `Status`, `ResourceType`, `ResourceId`, `ResourceArn`, `StartTime`, `EndTime`, `LastUpdatedTime`) and return its `OperationId`. Unknown `ApplicationId` → `ResourceNotFoundException` (404).

5. **GetOperation** (`POST /get-operation`) — input `OperationId` (required, UUID pattern). Must look the operation up in persisted state and return the `Operation` structure created by step 1/4, with the fields above populated consistently. An unknown but well-formed id must not yield a synthesized SUCCESS operation.

6. **DeregisterApplication** (`POST /deregister-application`) — input `ApplicationId` (required). Must remove the application (and its components/databases/tags/operations bookkeeping) from state so a subsequent `GetApplication` no longer returns it and `ListApplications` no longer lists it.

Every one of these must compute its response from persisted state — no canned payloads, no echoing the input back, no random `OperationId` that is not stored and retrievable.

## State Model

In `src/state.rs` (`ComAmazonawsSsmsapSsmsapState`), add:

- `applications: HashMap<String /*ApplicationId*/, StoredApplication>` where `StoredApplication { id, app_type: ApplicationType, arn, status: ApplicationStatus, discovery_status: ApplicationDiscoveryStatus, status_message: Option<String>, components: Vec<String>, associated_application_arns: Vec<String>, last_updated: DateTime, sid: Option<String>, sap_instance_number: Option<String>, instances: Vec<String>, credentials: Vec<ApplicationCredential>, database_arn: Option<String>, tags: HashMap<String,String> }`
- `operations: HashMap<String /*OperationId uuid*/, StoredOperation>` where `StoredOperation { id, op_type, status: OperationStatus, status_message, properties: HashMap<String, Option<String>>, resource_type, resource_id, resource_arn, start_time, end_time, last_updated_time }`
- `tags_by_arn: HashMap<String /*resourceArn*/, HashMap<String,String>>` (shared by RegisterApplication's `Tags` and later Tag/Untag/ListTagsForResource)
- optional for pagination: a deterministic token encoding (e.g. opaque base64 of the last-seen application id, padded to satisfy the 16..2048 token pattern).

All other operations (components, databases, configuration checks, resource permissions, tagging APIs, UpdateApplicationSettings, StartApplicationRefresh, ListOperations/ListOperationEvents) stay at 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-ssmsap-ssmsap/OPERATIONS.md` lists all 27 operations with their input/output members and errors (restJson1, sigv4 name `ssm-sap`): RegisterApplication → `Application` + `OperationId`; GetApplication → `Application` + `Tags`; ListApplications → `Applications` (paginated, inputToken/outputToken=NextToken, MaxResults 1..50) ; StartApplication/StopApplication → `OperationId`; GetOperation → `Operation`; DeregisterApplication → empty output. Smithy shapes `Application`, `ApplicationSummary` and `Operation` (queried from `smithy-model/com-amazonaws-ssmsap-ssmsap.json`) give the exact response fields cited above. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-ssmsap-ssmsap/src/ops/` matches all 27 handler files (register_application.rs, get_application.rs, list_applications.rs, start_application.rs, stop_application.rs, get_operation.rs, deregister_application.rs, ...), proving nothing is implemented and no test asserts any behavior.

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"HDB-TEST","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Tags":{"env":"dev"}} — expect 200 with Application.Id="HDB-TEST", Application.Arn ending in application/HDB-TEST, and a UUID OperationId. Actual: HTTP 501 NotImplemented.
2. POST /get-application with {"ApplicationId":"HDB-TEST"} — expect the same Application (Id, Type=HANA, Arn, Status) plus Tags {"env":"dev"}.
3. POST /list-applications with {"MaxResults":50} — expect Applications to contain exactly one ApplicationSummary for HDB-TEST with its Arn and Tags.
4. POST /get-operation with the OperationId returned by RegisterApplication — expect the stored Operation with matching ResourceId="HDB-TEST" and a terminal Status.
5. POST /stop-application {"ApplicationId":"HDB-TEST"} then POST /get-application — expect the application Status to reflect the stop and a new retrievable OperationId; then POST /start-application and observe the status transition back.
6. POST /get-application with {"ApplicationId":"NO-SUCH"} — expect no fabricated application (ValidationException/empty, never a synthesized record).
7. POST /deregister-application {"ApplicationId":"HDB-TEST"} then POST /list-applications — expect an empty Applications list and GetApplication to no longer return the record.
