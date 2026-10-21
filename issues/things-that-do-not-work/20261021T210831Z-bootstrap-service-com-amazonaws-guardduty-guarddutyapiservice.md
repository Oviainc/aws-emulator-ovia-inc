Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the GuardDuty emulator

_Filed: 2026-10-21T21:08:31Z_
_Operation: service-com-amazonaws-guardduty-guarddutyapiservice | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, holding zero state. Nothing works end-to-end.

This issue scopes the FIRST buildable slice: the detector lifecycle plus the findings read loop, which is the core GuardDuty workflow (`restJson1`, sigv4 signing name `guardduty`). Implement these operations, in this order, all computing their responses from persisted state:

1. **CreateDetector** — `POST /detector`. Input: `Enable` (boolean, required), `ClientToken` (@idempotencyToken), `FindingPublishingFrequency` (FIFTEEN_MINUTES|ONE_HOUR|SIX_HOURS), `DataSources`, `Features`, `Tags`. Output: `DetectorId` (len 1..300) — must be a freshly minted id that is STORED, not echoed. Record `Status` = ENABLED/DISABLED from `Enable`, `CreatedAt`/`UpdatedAt` timestamps, a `ServiceRole` ARN, the publishing frequency, tags and features. AWS allows only ONE detector per account/region: a second CreateDetector must fail with `BadRequestException` (HTTP 400) rather than minting a second id. Reuse the stored detector for a repeated `ClientToken`.
2. **GetDetector** — `GET /detector/{DetectorId}`. Output must be read back from the stored detector: `Status` (required), `ServiceRole` (required), `CreatedAt`, `UpdatedAt`, `FindingPublishingFrequency`, `DataSources`, `Features`, `Tags`. An unknown/deleted `DetectorId` must return `BadRequestException` (400), never a hollow 200.
3. **ListDetectors** — `GET /detector`. Output: `DetectorIds` (required, len 0..50) + `NextToken`; must list exactly the ids currently persisted (empty list before any create, empty again after delete) and honour `maxResults`/`nextToken` query params (@paginated inputToken=NextToken, outputToken=NextToken, items=DetectorIds).
4. **UpdateDetector** — `POST /detector/{DetectorId}`. Input: `Enable`, `FindingPublishingFrequency`, `DataSources`, `Features`. Must MUTATE the stored detector (and bump `UpdatedAt`) so a following GetDetector observes the new `Status`/frequency; unknown id => `BadRequestException`.
5. **CreateSampleFindings** — `POST /detector/{DetectorId}/findings/create`. Input: `DetectorId`, `FindingTypes` (len 0..50). Must INSERT one Finding per requested type (or a small default set when `FindingTypes` is absent) into the detector's finding store, each with a generated `Id`, `Arn`, `AccountId`, `Region`, `Partition`, `SchemaVersion`, `Type` (prefixed `SAMPLE:` style type from input), `Severity`, `Title`/`Description`, `Resource`, `CreatedAt`/`UpdatedAt`, `Archived=false`.
6. **ListFindings / GetFindings** — `POST /detector/{DetectorId}/findings` returns `FindingIds` (required) for the stored findings of that detector (honouring `MaxResults`/`NextToken`); `POST /detector/{DetectorId}/findings/get` takes `FindingIds` (required, len 0..50) and returns the matching `Findings` (list<Finding>, required) built from stored records — ids not present are simply absent, and the returned `Finding.Id`/`Type`/`Severity` must match what CreateSampleFindings stored.
7. **DeleteDetector** — `DELETE /detector/{DetectorId}`. Must remove the detector AND its findings/filters; afterwards GetDetector => `BadRequestException` (400) and ListDetectors no longer contains the id. Deleting an unknown id => `BadRequestException`.

No canned, hardcoded or echoed responses: a create followed by a read must return what was created, and a delete must be observable by a later read. Random-id-without-storage counts as a failure.

## State Model

In `src/state.rs` (`ComAmazonawsGuarddutyGuarddutyapiserviceState`, reached via `crate::state::service_state(&*self.engine)`):

- `detectors: HashMap<String /*DetectorId*/, Detector>`
- `Detector { id: String, status: DetectorStatus /*ENABLED|DISABLED*/, service_role: String, finding_publishing_frequency: Option<FindingPublishingFrequency>, data_sources: Option<DataSourceConfigurationsResult>, features: Vec<DetectorFeatureConfigurationResult>, tags: BTreeMap<String,String>, created_at: String, updated_at: String, findings: BTreeMap<String /*FindingId*/, FindingRecord>, filters: BTreeMap<String /*Name*/, FilterRecord> }`
- `FindingRecord { id: String, arn: String, account_id: String, region: String, partition: String, schema_version: String, r#type: String, severity: f64, confidence: Option<f64>, title: Option<String>, description: Option<String>, resource: Resource, service: Option<Service>, created_at: String, updated_at: String, archived: bool }`
- `client_tokens: HashMap<String /*ClientToken*/, String /*DetectorId*/>` for idempotent CreateDetector
- a monotonic counter/uuid source for detector and finding ids (ids must be stable across calls once stored)

`FilterRecord` is listed because the detector owns filters and DeleteDetector must cascade; CreateFilter/GetFilter/ListFilters themselves may stay stubbed in this slice.

Everything outside this list (members/organization ops, malware protection, IP sets, threat entity/intel sets, custom detection rules, investigations, coverage/usage statistics, publishing destinations, tagging) stays 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-guardduty-guarddutyapiservice/OPERATIONS.md` documents the wire protocol (restJson1, sdkId GuardDuty, endpoint prefix `guardduty`) and every operation's contract used above: CreateDetector (line 62: `POST /detector`, in `Enable` required, out `DetectorId`), GetDetector (line 329: `GET /detector/{DetectorId}`, out `ServiceRole`/`Status` required), ListDetectors (line 503: `GET /detector`, @paginated items=DetectorIds, out `DetectorIds` required), UpdateDetector (line 686: `POST /detector/{DetectorId}`), CreateSampleFindings (line 111: `POST /detector/{DetectorId}/findings/create`, in `FindingTypes`), ListFindings (line 519: @paginated items=FindingIds), GetFindings (line 343: in `FindingIds` required, out `Findings` required), DeleteDetector (line 162: `DELETE /detector/{DetectorId}`). `smithy_query(shape="Finding")` gives the required Finding members (AccountId, Arn, CreatedAt, Id, Region, Resource, SchemaVersion, Severity, Type, UpdatedAt). The all-stub state is proven by `src/ops/create_detector.rs` lines 4 and 179: the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDetector"))`; the same marker is present in all ~104 files under `src/ops/`. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. aws guardduty create-detector --enable --finding-publishing-frequency SIX_HOURS --endpoint-url $EMU  -> expect 200 with a new detectorId (currently HTTP 501 NotImplemented)
2. aws guardduty list-detectors -> expect the detectorId returned above to be the single entry in DetectorIds
3. aws guardduty get-detector --detector-id <id> -> expect Status=ENABLED, FindingPublishingFrequency=SIX_HOURS, ServiceRole and CreatedAt populated from stored state
4. aws guardduty update-detector --detector-id <id> --no-enable -> then get-detector again -> expect Status=DISABLED and a bumped UpdatedAt
5. aws guardduty create-sample-findings --detector-id <id> --finding-types Recon:EC2/PortProbeUnprotectedPort -> expect 200
6. aws guardduty list-findings --detector-id <id> -> expect one or more FindingIds for the sample finding just created
7. aws guardduty get-findings --detector-id <id> --finding-ids <findingId> -> expect a Finding whose id matches the requested id and whose type/severity match the sample created in the previous step
8. aws guardduty create-detector --enable (second time) -> expect BadRequestException (HTTP 400), not a second detector id
9. aws guardduty delete-detector --detector-id <id> -> expect 200, then list-detectors returns an empty DetectorIds list and get-detector --detector-id <id> returns BadRequestException (HTTP 400)
