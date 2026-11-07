Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__glue__kinesis__s3.kds-iceberg-input-mints-a-metadata-json-and-a-metadata-location firehose__glue__kinesis__s3.kds-glue-iceberg-input-maps-glue-column-types-to-iceberg-types firehose__glue__kinesis__s3.kds-glue-iceberg-input-without-a-version-writes-format-version-2 firehose__glue__kinesis__s3.kds-glue-deleted-iceberg-table-keeps-its-metadata-and-a-recreate-mints-a-new-table firehose__glue__kinesis__s3.kds-table-type-parameter-alone-materialises-no-iceberg-metadata
Issue-Key: firehose__glue__kinesis__s3.kds-iceberg-input-mints-a-metadata-json-and-a-metadata-location|_poll#12|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: firehose__glue__kinesis__s3.kds-glue-iceberg-input-maps-glue-column-types-to-iceberg-types|_poll#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: firehose__glue__kinesis__s3.kds-glue-iceberg-input-without-a-version-writes-format-version-2|_poll#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: firehose__glue__kinesis__s3.kds-glue-deleted-iceberg-table-keeps-its-metadata-and-a-recreate-mints-a-new-table|_poll#8|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: firehose__glue__kinesis__s3.kds-table-type-parameter-alone-materialises-no-iceberg-metadata|_poll#7|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# A Firehose Iceberg destination writes no table metadata object, so no Iceberg table is ever materialised

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
One root cause, grounded in five failing checks this report does NOT mark ALREADY FILED or OWNED. Open issues 20261102T100157Z-firehose-glue-kinesis-s3-5-net-new-root.md and 20261028T122732Z-firehose-glue-kinesis-s3-the-apache-iceberg-path.md own OTHER checks of this service and do not carry these replay keys.

Cause 1 of 1: no Iceberg metadata JSON is written under the destination table's location, so the table has no metadata_location and nothing downstream can read it.
AWS: when a delivery stream is created with an Iceberg destination over a Glue table marked as Iceberg, the first metadata file is materialised in S3 at <table-location>/metadata/00000-<uuid>.metadata.json, the Glue table's metadata_location parameter points at it, its format-version defaults to 2 when the input names no version, and the Glue column types are mapped into the Iceberg schema it contains. A control route whose table only carries a table_type parameter (no Iceberg marker) materialises nothing — that asymmetry is the behaviour under test.
Emulator: the metadata object never appears for any route. All five checks poll the destination prefix for 60 s and report an empty listing — e.g. "the metadata object 'iceberg/typed_columns/metadata/00000-<uuid>.metadata.json' landed not met within 60.0s. Last result: []" and "the control route wrote its metadata object not met within 60.0s. Last result: []". The object names in those messages are this run's generated values; the empty listing is the defect. Because even the control route writes nothing, the negative assertion (table_type alone materialises nothing) is also unfalsifiable.
Why: a client building an Iceberg lake on Firehose gets a delivery stream that reports ACTIVE and a Glue table with no metadata_location — nothing can query the table, and the format-version and type-mapping contracts cannot be relied on.
Fix: the Firehose Iceberg destination writer — the step that, on stream creation/first commit, mints the initial metadata JSON (schema mapped from the Glue columns, format-version defaulted to 2) at <table-location>/metadata/ and sets the Glue table's metadata_location, only for tables actually marked as Iceberg.
Traces: firehose__glue__kinesis__s3.kds-iceberg-input-mints-a-metadata-json-and-a-metadata-location, .kds-glue-iceberg-input-maps-glue-column-types-to-iceberg-types, .kds-glue-iceberg-input-without-a-version-writes-format-version-2, .kds-glue-deleted-iceberg-table-keeps-its-metadata-and-a-recreate-mints-a-new-table, .kds-table-type-parameter-alone-materialises-no-iceberg-metadata.

## Evidence
Expected: an Iceberg metadata object at <table-location>/metadata/00000-<uuid>.metadata.json (with format-version 2 by default and the Glue column types mapped into its schema) plus a matching metadata_location on the Glue table. Actual: the destination prefix stays empty for every route — 'the metadata object ... landed not met within 60.0s. Last result: []' (x5, including the control route).

## Steps to Reproduce
1) Create a Glue database and a table marked as Iceberg with a known column set and an S3 location; create a sibling table carrying only a table_type parameter.
2) CreateDeliveryStream with an Iceberg destination over those tables.
3) ListObjectsV2 under <table-location>/metadata/ and read the Glue table's metadata_location.
