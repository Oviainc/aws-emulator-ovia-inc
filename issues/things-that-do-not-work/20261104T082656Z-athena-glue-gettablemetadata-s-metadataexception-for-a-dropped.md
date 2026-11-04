Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue.athena-table-metadata-follows-a-glue-table-drop-and-recreate
Issue-Key: athena__glue.athena-table-metadata-follows-a-glue-table-drop-and-recreate|run#13|AssertionError|assert got == expected, f"GetTableMetadata after the Glue drop: expected {expected!r}, got {got!r}"
Issue-Commit: ed67363a200a02a71118cd5e7b7585c85db4efeb

# athena__glue: GetTableMetadata's MetadataException for a dropped Glue table carries emulator-authored text instead of the wrapped AmazonDataCatalog EntityNotFoundException message

> _Note: this issue was identified against the code base at commit `ed67363a200a02a71118cd5e7b7585c85db4efeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTableMetadata

## Description
1 cause.

Cause 1 of 1: the not-found refusal message of GetTableMetadata is emulator-authored.
AWS: Smithy: GetTableMetadata input CatalogName (required), DatabaseName (required), TableName (required); modeled errors InternalServerException / InvalidRequestException / MetadataException (client, HTTP 400). After the table is dropped from the Glue Data Catalog, Athena's metadata read fails with error code MetadataException whose Message is the wrapped catalog exception, of the fixed form 'Entity Not Found (Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <request id>; Proxy: null)' — every token of that text is fixed except the per-request Request ID. Recreating the table makes the same call succeed again.
Emulator: the error code is right (MetadataException) but the Message is 'Table does not exist: <table name>' — fixed text the emulator authors, with only the table name taken from the request; it carries neither the Service/Status Code/Error Code/Proxy envelope nor the EntityNotFoundException classification.
Why: a client (or a human reading a stack trace) that parses the wrapped envelope to recover the underlying catalog error code EntityNotFoundException, or that matches on AWS's message, sees text AWS never emits, so a dropped-table condition is not recognised.
Fix: the athena crate's GetTableMetadata handler — the not-found branch must render the wrapped AmazonDataCatalog EntityNotFoundException envelope as the MetadataException message.
Traces: athena__glue.athena-table-metadata-follows-a-glue-table-drop-and-recreate.

Related open issue that does NOT cover this cause: 20261104T063259Z-athena-refusals-omit-athenaerrorcode-errorcode-and-carry-emulator.md is a wire-corpus issue filed under the athena service token and does not name GetTableMetadata's dropped-table refusal.

## Evidence
Replay of the simparity-behavioral-aws-v1 behavioral suite against the emulator built from this checkout (service heading athena__glue, 1/2 passed).
athena-table-metadata-follows-a-glue-table-drop-and-recreate: "GetTableMetadata after the Glue drop: expected ('MetadataException', 'Entity Not Found (Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <id>; Proxy: null)'), got ('MetadataException', 'Table does not exist: sample_data')" — the check normalises the per-request id to <id>, so only the fixed text diverges.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run the simparity-behavioral-aws-v1 suite and inspect athena__glue.athena-table-metadata-follows-a-glue-table-drop-and-recreate.
3. Minimal manual repro: create a Glue database and table, confirm athena.get_table_metadata(CatalogName='AwsDataCatalog', DatabaseName=<db>, TableName=<tbl>) succeeds, glue.delete_table(...), then repeat the call — expect MetadataException with 'Entity Not Found (Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <request id>; Proxy: null)', observe 'Table does not exist: <tbl>'.
