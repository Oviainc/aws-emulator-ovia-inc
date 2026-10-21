Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the SageMaker FeatureStore Runtime emulator

_Filed: 2026-10-21T21:28:21Z_
_Operation: service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime | Class: stub_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 7 operations in `src/ops/` are unmodified scaffolds answering HTTP 501 `NotImplemented` and holding zero state, so no client workflow works at all. Implement a FIRST, SMALL stateful slice: the online-store record lifecycle of the primary resource (a record inside a feature group).

Happy path to implement, in order (restJson1 wire protocol, sigv4 signing name `sagemaker`, endpoint prefix `featurestore-runtime.sagemaker`):

1. **PutRecord** — `PUT /FeatureGroup/{FeatureGroupName}`. Input: `FeatureGroupName` (@httpLabel, required), `Record` (list<FeatureValue>, len 1.., required), optional `TargetStores`, optional `TtlDuration` {Unit, Value}. Output has no members (200 empty). Must UPSERT the record into persisted state: the record identifier is the `ValueAsString` of the FeatureValue whose `FeatureName` matches the feature group's record-identifier feature name (for the emulator: remember the identifier feature name on first write to that group, or treat the first `FeatureValue` of the posted record as identifier if nothing is registered). Store every FeatureValue (`FeatureName` + `ValueAsString` or `ValueAsStringList`), the EventTime feature value, and, when `TtlDuration` is given, compute and persist `ExpiresAt = EventTime + TtlDuration` as an ISO-8601 string. A second PutRecord with the same identifier must fully overwrite the previous feature values.

2. **GetRecord** — `GET /FeatureGroup/{FeatureGroupName}?RecordIdentifierValueAsString=...`. Output: `Record` (list<FeatureValue>, len 1..), `ExpiresAt`. Must read from the state written by PutRecord and return exactly the stored feature values — never a canned or echoed record. Honour `FeatureNames` (@httpQuery `FeatureName`, repeated) by projecting only the named features (plus the identifier/event-time features as AWS does) and `ExpirationTimeResponse=Enabled` by populating `ExpiresAt`. An identifier that was never written (or whose `ExpiresAt` has passed) must yield `ResourceNotFound` (HTTP 404) — note AWS returns 200 with NO `Record` member for an unknown identifier in an existing group, so prefer an empty 200 for "group known, identifier absent" and 404 for an unknown feature group.

3. **BatchGetRecord** — `POST /BatchGetRecord`. Input `Identifiers` (list<BatchGetRecordIdentifier> len 1..100: `FeatureGroupName`, `RecordIdentifiersValueAsString` list, optional `FeatureNames`). Output `Records` (list<BatchGetRecordResultDetail>: `FeatureGroupName`, `RecordIdentifierValueAsString`, `Record`, `ExpiresAt`), `Errors` (list<BatchGetRecordError>), `UnprocessedIdentifiers` — all three required and must always be present (empty lists when nothing to report). Must fan out over the SAME store GetRecord reads: every identifier previously written appears in `Records`; identifiers in an unknown feature group appear in `Errors` with a `ResourceNotFound`-style code/message; identifiers absent from a known group are simply omitted.

4. **DeleteRecord** — `DELETE /FeatureGroup/{FeatureGroupName}?RecordIdentifierValueAsString=...&EventTime=...`, optional `DeletionMode` (`SoftDelete` default | `HardDelete`), optional `TargetStores`. No output members (200 empty). `HardDelete` removes the record entry from state entirely; `SoftDelete` (the default) marks it deleted so a subsequent GetRecord no longer returns it, while keeping the tombstone with the supplied `EventTime`.

5. **ListRecords** — `POST /FeatureGroup/{FeatureGroupName}/ListRecords`. Input `MaxResults` (1..100), `NextToken` (1..8192), `IncludeSoftDeletedRecords`. Output `RecordIdentifiers` (required), `NextToken`. Must enumerate the identifiers actually present in state for that feature group, excluding soft-deleted ones unless `IncludeSoftDeletedRecords=true`, truncating to `MaxResults` and emitting an opaque `NextToken` that resumes exactly where the previous page stopped (and omitting `NextToken` on the last page). An unknown feature group → `ResourceNotFound` (404).

Every one of these five MUST compute its response from persisted state held in the shared engine (reached via `crate::state::service_state(&*self.engine)` per README.md). No hardcoded output, no echoing of the request, no generated identifier that is never stored. Validation should reject what the model rejects: `FeatureGroupName` length 1..150 / pattern `^(arn:...feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$`, `Identifiers` len 1..100, `Record` len >= 1, `MaxResults` range 1..100 → `ValidationError` (HTTP 400).

BatchWriteRecord (`Entries` len 1..25) and UpdateRecord (partial `Features` merge, `ConflictException`) stay stubbed at 501 for this first slice and will be filed as individual gap issues later.

## State Model

```
FeatureStoreRuntimeState {
    // feature group name (lowercased ARN suffix accepted too) -> FeatureGroupStore
    groups: HashMap<String, FeatureGroupStore>,
}

FeatureGroupStore {
    record_identifier_feature_name: Option<String>, // learned on first PutRecord
    event_time_feature_name: Option<String>,        // learned on first PutRecord
    // record identifier value (as string) -> StoredRecord, insertion-ordered
    records: IndexMap<String, StoredRecord>,
}

StoredRecord {
    identifier: String,                       // RecordIdentifierValueAsString
    features: Vec<StoredFeatureValue>,        // full overwrite on PutRecord
    event_time: Option<String>,               // ISO-8601, from the EventTime feature
    expires_at: Option<String>,               // EventTime + TtlDuration, ISO-8601
    ttl: Option<TtlDuration{unit, value}>,
    soft_deleted: bool,                       // set by DeleteRecord SoftDelete
    deleted_event_time: Option<String>,
}

StoredFeatureValue {
    feature_name: String,
    value_as_string: Option<String>,
    value_as_string_list: Option<Vec<String>>,
}
```

ListRecords pagination token = opaque (e.g. base64) encoding of the next index/identifier in `records` iteration order.

Done = an integration test doing PutRecord → GetRecord (values identical to what was put, projection by FeatureNames honoured) → BatchGetRecord (record present, Errors/UnprocessedIdentifiers present and empty) → ListRecords (identifier listed, paginates with MaxResults=1) → DeleteRecord → GetRecord/ListRecords no longer return it, all with correct bodies rather than merely 2xx.

## Evidence

OPERATIONS.md for this crate lists all 7 operations with routes and members: PutRecord `PUT /FeatureGroup/{FeatureGroupName}` (Record required, TtlDuration), GetRecord `GET /FeatureGroup/{FeatureGroupName}` (output `Record`, `ExpiresAt`), BatchGetRecord `POST /BatchGetRecord` (output `Records`/`Errors`/`UnprocessedIdentifiers` all required), DeleteRecord `DELETE /FeatureGroup/{FeatureGroupName}` (DeletionMode HardDelete|SoftDelete), ListRecords `POST /FeatureGroup/{FeatureGroupName}/ListRecords` (@paginated inputToken=NextToken, outputToken=NextToken, items=RecordIdentifiers), BatchWriteRecord, UpdateRecord. smithy_query on FeatureValue/TtlDuration/BatchGetRecordIdentifier/BatchGetRecordResultDetail confirms field shapes (FeatureName+ValueAsString/ValueAsStringList; TtlDuration{Unit,Value} documented as `ExpiresAt = EventTime + TtlDuration`). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 7 handler files (batch_get_record.rs, batch_write_record.rs, delete_record.rs, get_record.rs, list_records.rs, put_record.rs, update_record.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. No tests exist beyond the generated `tests/state_roundtrip.rs` state-API example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client at it with service `sagemaker-featurestore-runtime`.
2. PutRecord FeatureGroupName="customers", Record=[{FeatureName:"customer_id",ValueAsString:"C1"},{FeatureName:"event_time",ValueAsString:"2024-01-01T00:00:00Z"},{FeatureName:"city",ValueAsString:"Seattle"}] → expect HTTP 200 empty body; actual HTTP 501 NotImplemented.
3. GetRecord FeatureGroupName="customers", RecordIdentifierValueAsString="C1" → expect 200 with Record containing exactly the three feature values just written; actual 501.
4. GetRecord again with FeatureNames=["city"] → expect only the `city` (plus identifier/event-time) feature returned.
5. BatchGetRecord Identifiers=[{FeatureGroupName:"customers",RecordIdentifiersValueAsString:["C1","C-missing"]}] → expect Records containing C1 with its stored values, and Errors/UnprocessedIdentifiers present (empty or with a not-found entry); actual 501.
6. ListRecords FeatureGroupName="customers", MaxResults=1 → expect RecordIdentifiers=["C1"] and NextToken absent once all identifiers are returned; actual 501.
7. DeleteRecord FeatureGroupName="customers", RecordIdentifierValueAsString="C1", EventTime="2024-01-02T00:00:00Z" → expect 200; then GetRecord for C1 returns no Record and ListRecords no longer lists "C1" (unless IncludeSoftDeletedRecords=true); actual 501 at every step.
