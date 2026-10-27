Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.disassociate-source-unknown-identifier-rejected logs.s3-table-integration-associate-list-disassociate logs.s3-table-integration-source-association-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|AssociateSourceToS3TableIntegration|NotImplemented
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# Implement logs AssociateSourceToS3TableIntegration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateSourceToS3TableIntegration

## Description
`AssociateSourceToS3TableIntegration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/associate_source_to_s3_table_integration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `AssociateSourceToS3TableIntegration` call, so each one unblocks only when `AssociateSourceToS3TableIntegration` works: `logs.disassociate-source-unknown-identifier-rejected`, `logs.s3-table-integration-associate-list-disassociate`, `logs.s3-table-integration-source-association-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/associate_source_to_s3_table_integration.rs`. Replaying the corpus, the emulator refused the `AssociateSourceToS3TableIntegration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AssociateSourceToS3TableIntegration operation: operation AssociateSourceToS3TableIntegration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call AssociateSourceToS3TableIntegration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution (fixer)
`AssociateSourceToS3TableIntegration` is implemented and its SCAFFOLDED marker is gone.
An accepted associate stores ONE `S3TableIntegrationSourceRecord` (new in
`src/state.rs`, beside the integrations) keyed by the identifier it mints from the
store's own monotonic counter through the shared `mint_uuid_from_seq`, holding the
`integrationArn` the request named, the `dataSource` as it named it (as the
`DataSourceRecord` every data source of this service is held as), the creation instant
and the `ACTIVE` status — the model's `S3TableIntegrationSourceStatus` carries no
transient "associating" value, so the clock owes a new association nothing. Associating a
pair that already has query access returns the stored identifier and writes nothing: an
association IS one data source's access through one integration. `integrationArn` is held
only to being an ARN (the modelled `Arn`), refused with this service's unparsable-ARN
validation template, because `PutIntegration`'s `integrationType` admits only
`OPENSEARCH` and no operation of this service creates an S3 Table Integration — the
integration is another service's resource, named by the ARN alone. An associate past this
emulator's store cap (`MAX_S3_TABLE_INTEGRATION_SOURCES`, a resource guard rather than an
AWS quota) is refused with the only quota-shaped error the operation models,
`ThrottlingException`, before a record is allocated.

`ListSourcesForS3TableIntegration` is implemented in the same change: it was the
round-trip reader for this associate and the only reader of an association, and it
reports the associations of the integration it is asked about in the order they were
associated, paged by `maxResults` through an opaque cursor, writing nothing.
`DisassociateSourceFromS3TableIntegration` is implemented too, as the operation that ends
an association's life: it takes query access away and leaves the data deletion running
("deleting all associated data from the integration"), so the association is reported
`DATA_SOURCE_DELETE_IN_PROGRESS` until the instant the clock owes its removal
(`S3_TABLE_INTEGRATION_SOURCE_DELETION_DELAY_MS`, a chosen span) and is gone after it; an
identifier no association is stored under is the modelled `ResourceNotFoundException`
worded as CloudWatch Logs words it, naming the identifier the request asked for. Both
operations' own stub census lines are deleted with their scaffolds.

Covered by `tests/all/s3_table_integration_sources.rs`: a 6-row association table over
four integration ARNs (three services, two partitions, two Regions) and data sources from
one to 256 characters with and without a `type`, each round-tripped through the list; an
idempotence table; a disassociate lifecycle with the deletion instant pinned either side
of the span; a read-twice case; a pagination walk at both published bounds of
`maxResults` and unset; a refusal table over values that are not ARNs, `maxResults` one
past each bound, bad and foreign page tokens and unknown identifiers; and the store cap.
