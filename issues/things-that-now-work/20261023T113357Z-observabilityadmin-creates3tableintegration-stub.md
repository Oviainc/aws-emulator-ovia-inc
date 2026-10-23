Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.disassociate-source-unknown-identifier-rejected logs.s3-table-integration-associate-list-disassociate logs.s3-table-integration-lifecycle logs.s3-table-integration-source-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: observabilityadmin|CreateS3TableIntegration|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement observabilityadmin CreateS3TableIntegration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateS3TableIntegration

## Description
`CreateS3TableIntegration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-observabilityadmin-observabilityadmin/src/ops/create_s3_table_integration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `CreateS3TableIntegration` call, so each one unblocks only when `CreateS3TableIntegration` works: `logs.disassociate-source-unknown-identifier-rejected`, `logs.s3-table-integration-associate-list-disassociate`, `logs.s3-table-integration-lifecycle`, `logs.s3-table-integration-source-lifecycle-transitions`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-observabilityadmin-observabilityadmin/src/ops/create_s3_table_integration.rs`. Replaying the corpus, the emulator refused the `CreateS3TableIntegration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateS3TableIntegration operation: operation CreateS3TableIntegration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the observabilityadmin emulator
Call CreateS3TableIntegration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution (fixer)
`CreateS3TableIntegration` is implemented and its SCAFFOLDED marker is gone. The
operation carries no caller-supplied identifier, so the handler mints the integration
id (`state::new_s3_table_integration_id`, a v4 UUID through `emulator_telemetry`'s
`BatchId::random()`), stores ONE `StoredS3TableIntegration` record keyed by
`(region, id)` — the region coming from the request's own SigV4 credential scope, as
every other store in `src/state.rs` is keyed — and answers that record's ARN,
`arn:aws:observabilityadmin:<region>:<account>:s3-table-integration/<id>`. The record
holds the supplied `RoleArn`, `Encryption` and `Tags`, the creation instant, and the
table bucket the create NAMES
(`arn:aws:s3tables:<region>:<account>:bucket/aws-cloudwatch-telemetry-<id>`), because
the caller names no destination. That ARN is REPORTED-ONLY: the table bucket is the
`s3tables` service's own resource and its `CreateTableBucket` is itself a scaffold
stub, so nothing here claims the bucket exists.

`GetS3TableIntegration` is implemented in the same change (it was the round-trip
reader for this create and a stub of its own, `GetS3TableIntegration`'s own stub
issue): it reports every member from that one record and writes nothing. The modelled
`IntegrationStatus` carries no transient creating value — only `ACTIVE` and `DELETING`
— so a created integration is `ACTIVE` from the instant the create returns and the
clock owes it no transition. `ListS3TableIntegrations` and `DeleteS3TableIntegration`
remain stubs with their own issues.

Beyond the modelled constraints the centralized walk already enforces, the create
refuses with the modelled 400 `ValidationException`
(`state::s3_table_integration_rejection`) what the model states only in prose:
`SseAlgorithm` outside `SSEAlgorithm`'s own values, and a `KmsKeyArn` that rides on
`AES256` (S3 managed encryption takes no key; `aws:kms` with no key is the Amazon Web
Services managed key, which the member's own documentation admits). `RoleArn` and
`KmsKeyArn` are not read beyond that: both target `ResourceArn`, whose `@pattern` the
walk already enforces, and this operation resolves neither — it stores and reports
them verbatim. A create past either emulator store cap is the modelled 402, naming the
cap that refused it. Covered by `tests/all/s3_table_integrations.rs`: a 5-row table
over the encryption configurations and role forms the model admits (each round-tripped
through `GetS3TableIntegration`), a 6-row refusal table, a 6-row table of ARNs that
resolve to no stored integration, a read-twice case, and both store caps.
