Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the AWSBillingAndCostManagementDataExports emulator

_Filed: 2026-10-21T21:18:48Z_
_Operation: service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 12 operations in `src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and the service holds no state. This issue defines the FIRST buildable slice: the export lifecycle, implemented statefully over the awsJson1_1 wire protocol (`POST /`, `X-Amz-Target: AWSBillingAndCostManagementDataExports.<Op>`).

### Happy path to implement (in order)

1. **CreateExport** — input `Export` (required: `Name` [1..128, `^[0-9A-Za-z\-_]+$`], `DataQuery.QueryStatement`, `DestinationConfigurations.S3Destination` {`S3Bucket`, `S3Prefix`, `S3Region`, `S3OutputConfigurations`}, `RefreshCadence.Frequency`; optional `Description`), optional `ResourceTags`. Must mint and PERSIST an ARN of the modeled form `arn:aws:bcm-data-exports:<region>:<123456789012>:export/<Name>` (len 20..2048, must match the modeled pattern) and return it as `ExportArn`. Store the full submitted `Export` verbatim plus a fresh `ExportStatus{StatusCode, CreatedAt, LastUpdatedAt}` and the supplied tags. A second CreateExport with a name already in use should be refused (ValidationException 400).
2. **GetExport** — input `ExportArn` (required). Must return the stored `Export` (including the server-assigned `Export.ExportArn`, the exact `Name`, `Description`, `DataQuery`, `DestinationConfigurations`, `RefreshCadence` as created) and the stored `ExportStatus` with the real `CreatedAt`/`LastUpdatedAt` timestamps. Unknown/absent ARN → `ResourceNotFoundException` (HTTP 404), never a canned 200.
3. **ListExports** — input `MaxResults` (1..300), `NextToken` (0..8192). Must return one `ExportReference{ExportArn, ExportName, ExportStatus}` per persisted export — no hardcoded list. Honour `MaxResults` by truncating and emitting an opaque `NextToken` that resumes exactly where the page ended; omit `NextToken` on the last page.
4. **UpdateExport** — input `ExportArn` (required), `Export` (required). Must mutate the stored export in place (replace `DataQuery`, `DestinationConfigurations`, `RefreshCadence`, `Description`), bump `ExportStatus.LastUpdatedAt`, and return `ExportArn`. A subsequent GetExport must observe the new values. Unknown ARN → ResourceNotFoundException 404.
5. **DeleteExport** — input `ExportArn` (required), `@idempotent`. Must remove the export from state and return `ExportArn`. After deletion GetExport → 404 and the export no longer appears in ListExports.

Optional stretch inside the same slice (only if cheap): **ListTagsForResource** returning the `ResourceTags` persisted by CreateExport for that ARN.

All five must compute their responses from persisted state. No canned bodies, no echoing the request back, no minting an ARN that is never stored. A create followed by a get/list must return exactly what was created.

## State Model

```text
ServiceState {
    // primary store, keyed by the minted export ARN
    exports: BTreeMap<String /* ExportArn */, StoredExport>,
    // name -> ARN, to enforce name uniqueness and build deterministic ARNs
    names:   BTreeMap<String /* Export.Name */, String /* ExportArn */>,
}

StoredExport {
    arn:         String,                  // arn:aws:bcm-data-exports:<region>:<account>:export/<name>
    name:        String,                  // Export.Name
    description:  Option<String>,         // Export.Description
    data_query:  DataQuery,               // QueryStatement + TableConfigurations map
    destinations: DestinationConfigurations, // S3Destination{bucket, owner, prefix, region, output_configurations}
    refresh_cadence: RefreshCadence,      // Frequency enum
    status:      ExportStatus,            // StatusCode, StatusReason, CreatedAt, LastUpdatedAt, LastRefreshedAt
    tags:        Vec<ResourceTag>,        // from CreateExport.ResourceTags
    executions:  Vec<StoredExecution>,    // reserved for ListExecutions/GetExecution (later slice)
}
```

Pagination tokens for ListExports should encode a stable position in the `exports` key order (e.g. last-returned ARN, base64-encoded) so repeated calls are consistent.

### Out of scope (stay 501, filed separately later)
GetExecution, ListExecutions, GetTable, ListTables, TagResource, UntagResource (and ListTagsForResource if not done above).

### Done when
A client script can CreateExport → GetExport (fields match) → ListExports (export present, paging works) → UpdateExport → GetExport (sees the update) → DeleteExport → GetExport returns 404, with every response derived from stored state.

## Evidence

OPERATIONS.md for this crate lists all 12 operations with their inputs/outputs/errors: CreateExport(`Export` required, `ResourceTags`) → `ExportArn`; GetExport(`ExportArn` required) → `Export`+`ExportStatus` with ResourceNotFoundException (HTTP 404); ListExports `@paginated(inputToken=NextToken, outputToken=NextToken, items=Exports)` → `Exports` (list<ExportReference>)+`NextToken`, MaxResults range 1..300; UpdateExport(`ExportArn`,`Export` required) → `ExportArn`; DeleteExport `@idempotent`(`ExportArn` required) → `ExportArn`. Wire protocol from OPERATIONS.md: awsJson1_1, X-Amz-Target `AWSBillingAndCostManagementDataExports.<Op>`, signing name `bcm-data-exports`. Shape detail from smithy_query: `Export{ExportArn, Name(required,1..128), Description, DataQuery(required), DestinationConfigurations(required), RefreshCadence(required)}`, `ExportReference{ExportArn, ExportName, ExportStatus}` (all required), `ExportStatus{StatusCode, StatusReason, CreatedAt, LastUpdatedAt, LastRefreshedAt}`, `S3Destination{S3Bucket, S3Prefix, S3Region, S3OutputConfigurations required; S3BucketOwner optional}`, `RefreshCadence{Frequency required}`. Stub proof: services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/ops/create_export.rs line 4 `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 179 `Err(crate::scaffolded::Scaffolded::not_implemented("CreateExport"))`; all 12 files in src/ops/ are the same generated stub. No tests exist for this crate.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSBillingAndCostManagementDataExports.CreateExport and body {"Export":{"Name":"my-export","Description":"demo","DataQuery":{"QueryStatement":"SELECT line_item_usage_account_id FROM COST_AND_USAGE_REPORT"},"DestinationConfigurations":{"S3Destination":{"S3Bucket":"my-bucket","S3Prefix":"exports","S3Region":"us-east-1","S3OutputConfigurations":{"Compression":"GZIP","Format":"TEXT_OR_CSV","OutputType":"CUSTOM","Overwrite":"OVERWRITE_REPORT"}}},"RefreshCadence":{"Frequency":"SYNCHRONOUS"}},"ResourceTags":[{"Key":"env","Value":"dev"}]} — expect 200 with an ExportArn matching ^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$ (currently HTTP 501 NotImplemented)
2. GetExport with that ExportArn — expect 200 whose Export.Name is "my-export" and whose DataQuery/DestinationConfigurations/RefreshCadence equal what was submitted, plus ExportStatus.CreatedAt
3. ListExports with {"MaxResults":1} — expect the export present as an ExportReference{ExportArn, ExportName, ExportStatus}; create a second export and verify MaxResults=1 returns one item plus a NextToken, and the token returns the remaining item
4. UpdateExport with the ARN and a modified Export (new Description and QueryStatement) — expect 200 with ExportArn; GetExport must then return the updated values and a later LastUpdatedAt
5. DeleteExport with the ARN — expect 200 with ExportArn; GetExport with the same ARN must now return ResourceNotFoundException (HTTP 404) and ListExports must no longer include it
6. GetExport with an ARN that was never created — expect ResourceNotFoundException (HTTP 404), not 501 and not a canned 200
