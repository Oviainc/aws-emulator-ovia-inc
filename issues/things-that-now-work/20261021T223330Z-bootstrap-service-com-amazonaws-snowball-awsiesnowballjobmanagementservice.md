Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 103148eb649c3bac8e519c843b1d31d8637412c4

# Bootstrap the snowball emulator

_Filed: 2026-10-21T22:33:30Z_
_Operation: service-com-amazonaws-snowball-awsiesnowballjobmanagementservice | Class: missing_

> _Note: this issue was identified against the code base at commit `103148eb649c3bac8e519c843b1d31d8637412c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 27 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Snowball client workflow can run at all. Implement a FIRST, SMALL stateful slice: the job lifecycle of the primary resource (a Snowball job, addressed by a shipping Address).

### Happy path to implement (in order)

1. **CreateAddress** — input `Address` (required; structure with `Name`, `Company`, `Street1..3`, `City`, `StateOrProvince`, `Country`, `PostalCode`, `PhoneNumber`, `IsRestricted`, `Type`). Must mint a NEW `AddressId` matching the modeled pattern `^ADID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` (exactly 40 chars), store the full submitted address under that id, and return `AddressId`.
2. **DescribeAddress** — input `AddressId` (required). Must return the stored `Address` (including the generated `AddressId` member) for the id created in step 1; an unknown id must fail with the modeled `InvalidResourceException` (HTTP 400), not an empty 200.
3. **CreateJob** — inputs include `JobType` (EXPORT|IMPORT|LOCAL_USE), `AddressId`, `SnowballType`, `ShippingOption`, `Resources`, `Description`, `RoleARN`, `KmsKeyARN`, `SnowballCapacityPreference`, `Notification`. Must reject an `AddressId` that was never created with `InvalidResourceException`, mint a new `JobId` matching `^(M|J)ID[0-9a-f]{8}-...$` (39 chars), persist every supplied input field against that job, set `JobState` to its initial value (`New`) and record `CreationDate`, and return `JobId`.
4. **DescribeJob** — input `JobId` (required). Must return `JobMetadata` reconstructed from the persisted job: `JobId`, `JobState`, `JobType`, `SnowballType`, `CreationDate`, `Description`, `Resources`, `AddressId`, `RoleARN`, `KmsKeyARN`, `SnowballCapacityPreference`, `ShippingOption`/`ShippingDetails`, `Notification`. Values must be exactly what CreateJob stored. Unknown `JobId` -> `InvalidResourceException`.
5. **ListJobs** — inputs `MaxResults` (0..100), `NextToken`. Must return one `JobListEntry` (`JobId`, `JobState`, `JobType`, `SnowballType`, `CreationDate`, `Description`, `IsMaster`) per persisted job, honour `MaxResults` as a page size, emit an opaque `NextToken` when more jobs remain, resume from it on the next call, and reject a malformed/stale token with `InvalidNextTokenException` (HTTP 400).
6. **CancelJob** — input `JobId` (required). Must transition the persisted job to `Cancelled` so a following DescribeJob/ListJobs observes the new `JobState`. Cancelling a job that is no longer cancellable (already `Cancelled`, or past `New`/`PreparingAppliance`) must fail with `InvalidJobStateException` (HTTP 400); unknown `JobId` -> `InvalidResourceException`.

No canned, hardcoded or echoed responses: each response field must be computed from the shared store reached via `crate::state::service_state(&*self.engine)` (see README "State"). Minting an id without storing the record, or returning a zeroed `JobMetadata`, is a failure of this issue.

## State Model

In `src/state.rs`, on the service state struct:

- `addresses: HashMap<String /*AddressId*/, StoredAddress>` where `StoredAddress { address_id, name, company, street1, street2, street3, city, state_or_province, country, postal_code, phone_number, is_restricted, address_type }` — enough to rebuild the modeled `Address` verbatim.
- `jobs: HashMap<String /*JobId*/, StoredJob>` where `StoredJob { job_id, job_state: JobState, job_type, snowball_type, snowball_capacity_preference, shipping_option, creation_date: SystemTime, description, address_id, forwarding_address_id, role_arn, kms_key_arn, resources: Option<JobResource>, notification, on_device_service_configuration, cluster_id: Option<String>, long_term_pricing_id: Option<String> }`.
- `job_order: Vec<String>` — insertion order of job ids so ListJobs paginates deterministically.
- an id counter / RNG source used only to mint `ADID…`/`JID…` ids in the modeled formats.

The remaining 21 operations (clusters, long-term pricing, manifests/unlock codes, return shipping labels, compatible images, service versions, pickup locations, usage) stay stubbed at 501 and will be filed as individual gap issues later.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/ops/` matches all 27 operation files, i.e. the whole service answers 501 and persists nothing. Contracts taken from the crate's OPERATIONS.md: CreateAddress (Address required -> AddressId), DescribeAddress (AddressId required -> Address, InvalidResourceException), CreateJob (-> JobId, InvalidResourceException/InvalidInputCombinationException), DescribeJob (-> JobMetadata/SubJobMetadata, InvalidResourceException), ListJobs (@paginated inputToken=NextToken outputToken=NextToken items=JobListEntries, MaxResults range 0..100, InvalidNextTokenException), CancelJob (JobId required, InvalidJobStateException/InvalidResourceException). Member lists for `Address`, `JobMetadata` and `JobListEntry` confirmed via smithy_query on those shapes. Protocol is awsJson1_1 with `X-Amz-Target: AWSIESnowballJobManagementService.<Operation>` per OPERATIONS.md "Wire protocol". Well-established Snowball behavior: a newly created job starts in `New` and CancelJob is only permitted before the device is prepared.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI snowball client at it (awsJson1_1, POST /, X-Amz-Target: AWSIESnowballJobManagementService.<Operation>).
2. CreateAddress with a full Address (Name, Street1, City, StateOrProvince, Country, PostalCode, PhoneNumber) -> expect 200 with a 40-char AddressId of the form ADID<uuid>; today 501.
3. DescribeAddress with that AddressId -> expect 200 whose Address matches every field submitted in step 2, with AddressId populated.
4. CreateJob with JobType=IMPORT, the AddressId from step 2, SnowballType=EDGE, ShippingOption=SECOND_DAY, Description='bootstrap job', RoleARN and Resources -> expect 200 with a 39-char JobId of the form JID<uuid>.
5. DescribeJob with that JobId -> expect JobMetadata.JobState=New and JobType/SnowballType/Description/AddressId/Resources equal to what was submitted in step 4.
6. ListJobs with MaxResults=1 after creating two jobs -> expect exactly one JobListEntry plus a NextToken; call again with that NextToken and expect the second job and no overlap. Calling ListJobs with NextToken='garbage' must return InvalidNextTokenException (HTTP 400).
7. CancelJob with the JobId -> expect 200, then DescribeJob shows JobState=Cancelled, and a second CancelJob on the same job returns InvalidJobStateException (HTTP 400).
8. DescribeJob / DescribeAddress / CancelJob with a well-formed but unknown id -> expect InvalidResourceException (HTTP 400), not a 501 and not an empty success.
