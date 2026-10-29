Issue-Source: trace-sweep-divergence
Issue-Trace: dataexchange-dataset-catalog-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dataexchange-dataset-catalog-lifecycle dataexchange-event-action-lifecycle dataexchange-job-error-paths dataexchange-job-import-signed-url-lifecycle dataexchange-list-jobs-across-multiple-revisions dataexchange-notification-and-api-asset-errors dataexchange-job-cancel-before-start
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange|CreateDataSet|2xx->2xx|$.CreatedAt
Issue-Key: dataexchange|CreateDataSet|2xx->2xx|$.UpdatedAt
Issue-Key: dataexchange|GetDataSet|2xx->2xx|$.CreatedAt
Issue-Key: dataexchange|GetDataSet|2xx->2xx|$.UpdatedAt
Issue-Key: dataexchange|UpdateDataSet|2xx->2xx|$.CreatedAt
Issue-Key: dataexchange|UpdateDataSet|2xx->2xx|$.UpdatedAt
Issue-Key: dataexchange|ListDataSets|2xx->2xx|$.DataSets[Arn="arn
Issue-Key: dataexchange|CreateJob|2xx->2xx|$.CreatedAt
Issue-Key: dataexchange|CreateJob|2xx->2xx|$.Details.ImportAssetFromSignedUrl.SignedUrlExpiresAt
Issue-Key: dataexchange|CreateJob|2xx->2xx|$.UpdatedAt
Issue-Key: dataexchange|GetJob|2xx->2xx|$.CreatedAt
Issue-Key: dataexchange|GetJob|2xx->2xx|$.Details.ImportAssetFromSignedUrl.SignedUrlExpiresAt
Issue-Key: dataexchange|GetJob|2xx->2xx|$.UpdatedAt
Issue-Key: dataexchange|CreateRevision|2xx->2xx|$.CreatedAt
Issue-Key: dataexchange|CreateRevision|2xx->2xx|$.UpdatedAt
Issue-Key: dataexchange|ListDataSetRevisions|2xx->2xx|$.Revisions[Arn="arn
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# DataExchange serializes every date-time member as an epoch number instead of an RFC3339 string

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSet

## Description
The emulator's dataexchange response serializer emits every timestamp member as a bare JSON number (epoch seconds with a fractional part) where AWS emits an RFC3339 date-time string. The defect is in one place — timestamp serialization — but it surfaces on eight distinct operations/shapes, so eight causes must be closed.

Shared AWS requirement: Smithy com.amazonaws.dataexchange#Timestamp is `type: timestamp` carrying @timestampFormat("date-time"), and every CreatedAt / UpdatedAt / RevokedAt / SignedUrlExpiresAt member targets it. In the restJson1 body these therefore serialize as RFC3339 strings of the form YYYY-MM-DDThh:mm:ss.sssZ (the instant itself is request/state-derived and is not part of the requirement; the string encoding is). A JSON number is not a valid value for a date-time-formatted member.

Cause 1 of 8: CreateDataSet (POST /v1/data-sets, 201). AWS: CreatedAt and UpdatedAt are date-time strings (Smithy: CreateDataSet output CreatedAt/UpdatedAt -> Timestamp @timestampFormat date-time). Emulator: both are numbers, e.g. 1793294437.176836 where the recording carries '2026-09-06T06:29:41.417Z'. Why: an SDK/CBOR-free restJson1 client parsing a date-time member off a number either throws a deserialization error or silently yields a wrong instant, so a client cannot read the creation time of a data set it just created. Fix: the dataexchange response serializer's Timestamp member encoding (not state — the stored instant is correct). Traces: dataexchange-dataset-catalog-lifecycle#task#2 (also dataexchange-event-action-lifecycle#setup#1, dataexchange-job-error-paths#setup#1, dataexchange-job-import-signed-url-lifecycle#task#1, dataexchange-list-jobs-across-multiple-revisions#setup#1, dataexchange-notification-and-api-asset-errors#task#1).

Cause 2 of 8: GetDataSet (GET /v1/data-sets/{DataSetId}, 200). AWS: CreatedAt/UpdatedAt date-time strings. Emulator: numbers (1793294437.176836). Why: reads of an existing data set are unparseable the same way. Fix: same serializer path, GetDataSet response. Traces: dataexchange-dataset-catalog-lifecycle#task#3, #task#5.

Cause 3 of 8: UpdateDataSet (200). AWS: CreatedAt is the original instant and UpdatedAt the mutation instant, both date-time strings. Emulator: both numbers. Why: a client cannot tell that its update advanced UpdatedAt. Fix: same serializer path, UpdateDataSet response. Traces: dataexchange-dataset-catalog-lifecycle#task#4.

Cause 4 of 8: ListDataSets (200). AWS: each DataSets[] entry's CreatedAt/UpdatedAt is a date-time string. Emulator: numbers inside the list entries, so the list projection shares the defect. Why: catalog listings cannot be sorted or displayed by date. Fix: same serializer path, applied inside ListOfDataSetEntry members. Traces: dataexchange-dataset-catalog-lifecycle#task#6.

Cause 5 of 8: CreateJob (POST /v1/jobs, 201). AWS: CreatedAt/UpdatedAt and Details.ImportAssetFromSignedUrl.SignedUrlExpiresAt are all date-time strings. Emulator: all three are numbers. Why: the signed-URL expiry is the one value a client must compare against the wall clock before uploading; delivered as a number it is misread or rejected. Fix: same serializer path, including nested ResponseDetails members. Traces: dataexchange-job-cancel-before-start#task#3 (also dataexchange-job-error-paths#task#7, dataexchange-job-import-signed-url-lifecycle#task#3, dataexchange-list-jobs-across-multiple-revisions#task#4, #task#5).

Cause 6 of 8: GetJob (GET /v1/jobs/{JobId}, 200). AWS: CreatedAt/UpdatedAt/Details.ImportAssetFromSignedUrl.SignedUrlExpiresAt date-time strings. Emulator: numbers. Fix: same serializer path, GetJob response. Traces: dataexchange-job-cancel-before-start#task#4, dataexchange-job-import-signed-url-lifecycle#task#4.

Cause 7 of 8: CreateRevision (POST /v1/data-sets/{DataSetId}/revisions, 201). AWS: CreatedAt/UpdatedAt (and RevokedAt when present) date-time strings. Emulator: numbers. Fix: same serializer path, CreateRevision response. Traces: dataexchange-job-error-paths#setup#2 (also dataexchange-job-import-signed-url-lifecycle#task#2, dataexchange-list-jobs-across-multiple-revisions#setup#2, #setup#3, dataexchange-notification-and-api-asset-errors#task#2).

Cause 8 of 8: ListDataSetRevisions (200). AWS: each Revisions[] entry's CreatedAt/UpdatedAt is a date-time string. Emulator: numbers inside the list entries. Fix: same serializer path, applied inside the revision list entries. Traces: dataexchange-job-import-signed-url-lifecycle#task#8, dataexchange-list-jobs-across-multiple-revisions#task#8.

## Evidence
Smithy: com.amazonaws.dataexchange#Timestamp = timestamp with @timestampFormat("date-time"); CreateDataSet/GetDataSet/UpdateDataSet/CreateJob/GetJob/CreateRevision outputs bind CreatedAt, UpdatedAt (and SignedUrlExpiresAt, RevokedAt) to it.

Status codes match throughout (201/200 expected, 201/200 actual) — only the value encoding diverges.

CreateDataSet 201: expected $.CreatedAt='2026-09-06T06:29:41.417Z', got 1793294437.176836; expected $.UpdatedAt='2026-09-06T06:29:41.417Z', got 1793294437.176836.
GetDataSet 200: same two members, same numeric form.
UpdateDataSet 200: expected $.CreatedAt='2026-09-06T06:29:41.417Z' got 1793294437.176836; expected $.UpdatedAt='2026-09-06T06:29:41.673Z' got 1793294437.2106993.
ListDataSets 200: $.DataSets[Arn=...].CreatedAt / .UpdatedAt numeric instead of date-time strings.
CreateJob 201: expected $.CreatedAt='2026-09-06T07:07:39.534Z' got 1793294438.6846073; expected $.Details.ImportAssetFromSignedUrl.SignedUrlExpiresAt='2026-09-06T08:07:39.533Z' got 1793298038.6846073.
GetJob 200: same three members numeric.
CreateRevision 201: expected $.CreatedAt='2026-09-06T07:07:42.776Z' got 1793294439.3987665; $.UpdatedAt likewise.
ListDataSetRevisions 200: $.Revisions[Arn=...].CreatedAt / .UpdatedAt numeric (recording asserts shape only).

The literal instants above are one run's values and are not the requirement; the requirement is that each of these members is a JSON string in RFC3339 date-time form rather than a JSON number.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: POST /v1/jobs-free path — CreateDataSet with a Name/Description (dataexchange-dataset-catalog-lifecycle#task#2) and read CreatedAt/UpdatedAt in the 201 body: both are JSON numbers.
3. Cause 2/3/4: GET /v1/data-sets/{DataSetId} (#task#3, #task#5), PATCH the data set (#task#4) and GET /v1/data-sets (#task#6) — same numeric encoding, including inside DataSets[].
4. Cause 5/6: CreateJob of Type IMPORT_ASSET_FROM_SIGNED_URL (dataexchange-job-cancel-before-start#task#3) then GET /v1/jobs/{JobId} (#task#4) — CreatedAt, UpdatedAt and Details.ImportAssetFromSignedUrl.SignedUrlExpiresAt are numbers.
5. Cause 7: POST /v1/data-sets/{DataSetId}/revisions (dataexchange-job-error-paths#setup#2) — CreatedAt/UpdatedAt numbers.
6. Cause 8: GET /v1/data-sets/{DataSetId}/revisions (dataexchange-job-import-signed-url-lifecycle#task#8) — Revisions[].CreatedAt/.UpdatedAt numbers.
7. Each body should carry RFC3339 date-time strings for those members.
