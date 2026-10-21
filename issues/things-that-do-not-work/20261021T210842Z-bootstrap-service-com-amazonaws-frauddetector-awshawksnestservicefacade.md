Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the frauddetector-awshawksnestservicefacade emulator

_Filed: 2026-10-21T21:08:42Z_
_Operation: service-com-amazonaws-frauddetector-awshawksnestservicefacade | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 73 operations in `services/service-com-amazonaws-frauddetector-awshawksnestservicefacade/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. This issue defines the FIRST, SMALL slice to implement: the detector lifecycle of Amazon Fraud Detector, end-to-end and statefully, over `awsJson1_1` (`POST /`, `X-Amz-Target: AWSHawksNestServiceFacade.<Op>`).

### Happy path (implement these 6, in this order)

1. **PutDetector** — in: `detectorId` (required, 1..64, `^[0-9a-z_-]+$`), `eventTypeName` (required), `description`, `tags`. Out: no members. Must INSERT (or update in place, it is an upsert) a detector record keyed by `detectorId`, stamping `arn`, `createdTime` and `lastUpdatedTime`. A second PutDetector for the same id must update `description`/`eventTypeName`, not duplicate.
2. **CreateRule** — in: `ruleId` (required), `detectorId` (required), `expression` (required, 1..4096), `language` (required, `DETECTORPL`), `outcomes` (required, len>=1), `description`, `tags`. Out: `rule` (Rule). Must attach the rule to the stored detector and assign `ruleVersion` starting at `"1"`, incrementing per (detectorId, ruleId); the returned `rule` must carry the assigned `{detectorId, ruleId, ruleVersion}` read back from state, not echoed input alone. Unknown `detectorId` → `ValidationException` (400).
3. **CreateDetectorVersion** — in: `detectorId` (required), `rules` (required, list<Rule> referencing stored rules), `description`, `externalModelEndpoints`, `modelVersions`, `ruleExecutionMode` (`ALL_MATCHED|FIRST_MATCHED`), `tags`. Out: `detectorId`, `detectorVersionId` (`^([1-9][0-9]*)$`), `status`. Must allocate the next sequential `detectorVersionId` per detector (`"1"`, `"2"`, …), persist the version with `status = DRAFT`, default `ruleExecutionMode = FIRST_MATCHED` when absent, and resolve each referenced rule against stored rules (unknown rule/version → `ValidationException`; unknown detector → `ResourceNotFoundException` 404).
4. **GetDetectorVersion** — in: `detectorId` + `detectorVersionId` (both required). Out must be reconstructed from the stored version: `detectorId`, `detectorVersionId`, `description`, `rules`, `modelVersions`, `externalModelEndpoints`, `status`, `ruleExecutionMode`, `createdTime`, `lastUpdatedTime`, `arn`. Unknown id → `ResourceNotFoundException` (404).
5. **GetDetectors** — in: `detectorId` (optional filter), `maxResults` (range 5..10), `nextToken`. Out: `detectors` (list<Detector> with `detectorId`, `eventTypeName`, `description`, `createdTime`, `lastUpdatedTime`, `arn`), `nextToken`. Must list the detectors actually created, honour the `detectorId` filter (unknown id → `ResourceNotFoundException`), and implement `maxResults`/`nextToken` paging over stored order — `nextToken` present only when more remain.
6. **DeleteDetector** — in: `detectorId` (required). Out: no members. Must remove the detector from state so a following GetDetectors/DescribeDetector no longer reports it; a detector that still has detector versions must fail with `ConflictException` (409), matching the documented "conflict during a delete operation" semantics.

Supporting read, cheap to add from the same records: **DescribeDetector** (`detectorId` → `detectorVersionSummaries`, `arn`), implement it if the version summaries are already stored.

### Hard requirement: no canned responses
Every one of the above must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)`. Hardcoded outputs, pure echo of the input, and generating an id without storing it are all failures for this issue: a `PutDetector` followed by `GetDetectors` must return the detector that was created, and `CreateDetectorVersion` followed by `GetDetectorVersion` must return the rules and status that were stored.

## State Model

In `src/state.rs`, hung off the service state struct:

- `detectors: BTreeMap<String /*detectorId*/, Detector>` where
  `Detector { detector_id: String, event_type_name: String, description: Option<String>, arn: String, created_time: String, last_updated_time: String, tags: Vec<Tag>, rules: BTreeMap<String /*ruleId*/, Vec<RuleRecord>>, versions: BTreeMap<String /*detectorVersionId*/, DetectorVersion>, next_version_id: u32 }`
- `RuleRecord { rule_id: String, rule_version: String, expression: String, language: String, outcomes: Vec<String>, description: Option<String>, created_time: String, last_updated_time: String, arn: String }` (index in the Vec = version-1, so `ruleVersion` allocation is `len()+1`)
- `DetectorVersion { detector_version_id: String, description: Option<String>, status: DetectorVersionStatus /*DRAFT on create*/, rule_refs: Vec<(String /*ruleId*/, String /*ruleVersion*/)>, model_versions: Vec<ModelVersion>, external_model_endpoints: Vec<String>, rule_execution_mode: RuleExecutionMode /*default FIRST_MATCHED*/, created_time: String, last_updated_time: String, arn: String }`

Ids are allocated from `next_version_id` / rule-version length held in this state — never random, never recomputed per request. Timestamps are ISO-8601 strings (the model types these as strings of len 11..30). ARNs follow the modeled pattern `arn:aws:frauddetector:<region>:<account12>:detector/<detectorId>`.

All other operations (variables, event types, models, batch jobs, lists, tags, predictions, event ingestion) stay at 501 and will be filed as individual gap issues later. Keeping this first slice to the detector lifecycle is intentional.

## Evidence

`services/service-com-amazonaws-frauddetector-awshawksnestservicefacade/OPERATIONS.md` lists all 73 operations with their input/output members and errors; the field names, constraints (`detectorId` 1..64 `^[0-9a-z_-]+$`, `detectorVersionId` `^([1-9][0-9]*)$`, `GetDetectors.maxResults` range 5..10), the `@paginated(inputToken=nextToken, outputToken=nextToken)` trait on GetDetectors, the `DetectorVersionStatus` (ACTIVE|DRAFT|INACTIVE) and `RuleExecutionMode` (ALL_MATCHED|FIRST_MATCHED) enums, and the error sets (`ResourceNotFoundException` 404 on GetDetectorVersion/CreateDetectorVersion/GetDetectors, `ConflictException` 409 on PutDetector/DeleteDetector, `ValidationException` 400 on CreateRule) are all quoted from that file. README.md documents the wire protocol as `awsJson1_1` with `X-Amz-Target: AWSHawksNestServiceFacade.<OperationName>` and the 501-until-written scaffold contract. `src/ops/put_detector.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 162 returns `Err(crate::scaffolded::Scaffolded::not_implemented("PutDetector"))` — representative of all 73 op files, so no operation currently stores or returns anything.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `frauddetector` client at it.
2. `PutDetector` with detectorId="my_detector", eventTypeName="my_event_type", description="demo" — expect 200 with an empty body (currently 501).
3. `CreateRule` with detectorId="my_detector", ruleId="high_risk", language="DETECTORPL", expression="$amount > 100", outcomes=["review"] — expect 200 and `rule.ruleVersion == "1"`.
4. `CreateDetectorVersion` with detectorId="my_detector" and rules=[{detectorId:"my_detector", ruleId:"high_risk", ruleVersion:"1"}] — expect `detectorVersionId == "1"` and `status == "DRAFT"`.
5. `GetDetectorVersion` detectorId="my_detector", detectorVersionId="1" — expect the stored rule reference, `ruleExecutionMode == "FIRST_MATCHED"`, status DRAFT, and non-empty createdTime/arn.
6. `GetDetectors` with no filter — expect "my_detector" in `detectors` with the eventTypeName and description supplied in step 2; with maxResults=5 and more than 5 detectors created, expect a `nextToken` that pages the remainder without repeats.
7. `GetDetectorVersion` for detectorVersionId="2" — expect `ResourceNotFoundException` (404), not 501 or an empty 200.
8. `DeleteDetector` detectorId="my_detector" while version "1" exists — expect `ConflictException` (409); after `DeleteDetectorVersion` (or on a detector with no versions) expect 200 and a subsequent `GetDetectors` that no longer lists it.
