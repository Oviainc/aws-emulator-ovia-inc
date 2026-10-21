Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the Marketplace Commerce Analytics emulator

_Filed: 2026-10-21T22:08:16Z_
_Operation: service-com-amazonaws-marketplacecommerceanalytics-marketplacecommerceanalytics20150701 | Class: stub_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state (`src/ops/generate_data_set.rs`, `src/ops/start_support_data_export.rs`; `src/state.rs` has `// TODO: add service-specific state fields`). The service models exactly TWO operations, so the bootstrap slice is the whole surface and must be implemented statefully.

Wire protocol (from OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: MarketplaceCommerceAnalytics20150701.<OperationName>`, sigv4 signing name `marketplacecommerceanalytics`.

### Happy path to implement, in order

1. **GenerateDataSet** — input `dataSetType` (DataSetType enum, len 1..255, required), `dataSetPublicationDate` (timestamp, required), `roleNameArn` (string, len 1..2048, required), `destinationS3BucketName` (string, len 1..63, required), `destinationS3Prefix` (string, optional), `snsTopicArn` (string, len 1..256, required), `customerDefinedValues` (map<string,string>, len 1..5, optional). Output: `dataSetRequestId` (string). The handler must MINT a fresh unique request id (UUID-shaped), RECORD a request record in service state keyed by that id capturing every input member (including `customerDefinedValues` and `destinationS3Prefix` verbatim — they are documented as returned unmodified), and return that id. Two calls with identical input must yield two DIFFERENT ids, and every id returned must be findable in state.

2. **StartSupportDataExport** — input `dataSetType` (SupportDataSetType: `customer_support_contacts_data` | `test_customer_support_contacts_data`, required), `fromDate` (timestamp, required), `roleNameArn` (required), `destinationS3BucketName` (required), `destinationS3Prefix` (optional), `snsTopicArn` (required), `customerDefinedValues` (optional). Output: `dataSetRequestId`. Same contract: mint a unique id, persist a record of the request (distinguished by kind = support export, with `fromDate` instead of `dataSetPublicationDate`), return the id. The operation is `@deprecated` in the model but must still succeed on the happy path.

3. **Derived publication artifact (the "use" step that makes the workflow observable)** — because the real API is asynchronous and reports completion only via S3 + SNS, the emulator must at least record, per persisted request, the artifact it would publish: the CSV object key `{dataSetType}_YYYY-MM-DD.csv` (date taken from `dataSetPublicationDate` / `fromDate`), joined under `destinationS3Prefix` when one was supplied, the destination bucket, the `snsTopicArn` to notify, and the `customerDefinedValues` echoed into the notification/metadata. Re-requesting the SAME dataSetType + same publication date for the same bucket/prefix must OVERWRITE the recorded artifact for that key (documented: "the original file will be overwritten by the new file") while still returning a new `dataSetRequestId`.

4. **Error channel** — the only modeled error is `MarketplaceCommerceAnalyticsException` (HTTP 500), which must be returned as a real modeled fault (not 501) when the emulator cannot service the request, e.g. state unreachable. Requests that violate something the model does not cover should use the 400 `ValidationException` channel, never 501. Modeled constraints (enum values, string lengths, required members, `customerDefinedValues` size 1..5) are enforced before `handle` runs, so the handler must not re-refuse valid input.

No canned, hardcoded, or echoed-without-storage responses: a random id returned without a stored record is an explicit failure of this issue. Every response field must be computed from persisted state.

## State Model

```text
MarketplaceCommerceAnalyticsState {
    // dataSetRequestId -> the request as accepted
    requests: HashMap<String, DataSetRequest>,
    // insertion order of ids, so tests can assert "the id I got back is the newest"
    request_order: Vec<String>,
    // "{bucket}/{prefix}/{dataSetType}_YYYY-MM-DD.csv" -> PublishedArtifact
    // overwritten when the same data set/date/destination is requested again
    published: HashMap<String, PublishedArtifact>,
}

DataSetRequest {
    data_set_request_id: String,       // the value returned to the caller
    kind: RequestKind,                 // GenerateDataSet | StartSupportDataExport
    data_set_type: String,             // DataSetType or SupportDataSetType value
    publication_date: Option<DateTime>,// GenerateDataSet.dataSetPublicationDate
    from_date: Option<DateTime>,       // StartSupportDataExport.fromDate
    role_name_arn: String,
    destination_s3_bucket_name: String,
    destination_s3_prefix: Option<String>,
    sns_topic_arn: String,
    customer_defined_values: BTreeMap<String, String>, // stored verbatim
    created_at: DateTime,              // emulator clock
}

PublishedArtifact {
    s3_key: String,                    // {prefix/}{dataSetType}_YYYY-MM-DD.csv
    bucket: String,
    sns_topic_arn: String,
    customer_defined_values: BTreeMap<String, String>,
    last_request_id: String,           // id of the most recent request that wrote it
}

RequestKind = GenerateDataSet | StartSupportDataExport
```

Done = a client script can call both operations with valid input, receive distinct non-empty `dataSetRequestId` values over `awsJson1_1`, and the emulator's state holds a faithful record of each request (including the optional members) with the published-artifact key derived from the data set type and date — no 501s anywhere on the happy path.

## Evidence

OPERATIONS.md for this crate lists both operations with their exact input/output members and the single modeled error: GenerateDataSet (`dataSetType`, `dataSetPublicationDate`, `roleNameArn`, `destinationS3BucketName`, `destinationS3Prefix`, `snsTopicArn`, `customerDefinedValues` -> `dataSetRequestId`; `MarketplaceCommerceAnalyticsException` HTTP 500) and StartSupportDataExport (`dataSetType`, `fromDate`, ... -> `dataSetRequestId`), plus the wire protocol `awsJson1_1` with target prefix `MarketplaceCommerceAnalytics20150701`. The modeled docs reproduced in `src/ops/generate_data_set.rs` (lines 52-109) state that the operation "Returns a unique request identifier that can be used to correlate requests with notifications from the SNS topic", that data sets are published as `{data_set_type}_YYYY-MM-DD.csv`, that a same-named file "will be overwritten by the new file", and that `customerDefinedValues` are "returned, unmodified, in the Amazon SNS notification message and the data set metadata file" — grounding the uniqueness, key-derivation, overwrite and verbatim-storage requirements above. Both handler files carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and end in `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (generate_data_set.rs line 199), and `src/state.rs` declares an empty state struct, so no state exists. No tests exist for either operation.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `marketplacecommerceanalytics` at it.
2. POST / with `X-Amz-Target: MarketplaceCommerceAnalytics20150701.GenerateDataSet` and body {"dataSetType":"customer_profile_by_geography","dataSetPublicationDate":1.4e9,"roleNameArn":"arn:aws:iam::123456789012:role/MarketplaceAnalyticsRole","destinationS3BucketName":"my-bucket","destinationS3Prefix":"myprefix/mydatasets","snsTopicArn":"arn:aws:sns:us-east-1:123456789012:mca","customerDefinedValues":{"trackingId":"abc123"}}. Expect HTTP 200 with a non-empty unique `dataSetRequestId`; observed: HTTP 501 NotImplemented.
3. Repeat the identical GenerateDataSet call. Expect a SECOND, DIFFERENT `dataSetRequestId` (both ids recorded in state; the derived artifact key `myprefix/mydatasets/customer_profile_by_geography_YYYY-MM-DD.csv` overwritten by the newer request).
4. POST / with `X-Amz-Target: MarketplaceCommerceAnalytics20150701.StartSupportDataExport` and body {"dataSetType":"test_customer_support_contacts_data","fromDate":1.4e9,"roleNameArn":"arn:aws:iam::123456789012:role/MarketplaceAnalyticsRole","destinationS3BucketName":"my-bucket","snsTopicArn":"arn:aws:sns:us-east-1:123456789012:mca"}. Expect HTTP 200 with its own distinct `dataSetRequestId`; observed: HTTP 501 NotImplemented.
5. Verify every returned id is distinct and that the emulator's persisted request records reflect each call's inputs verbatim (including `customerDefinedValues` and the omitted `destinationS3Prefix`), rather than canned or echoed output.
