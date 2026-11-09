Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.s3t-record-expiration-reads-rejected-as-method-not-allowed
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.s3t-record-expiration-reads-rejected-as-method-not-allowed|run#12|AssertionError|assert (code, message) == ("MethodNotAllowedException", MSG_NOT_ALLOWED), (
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# Implement s3tables GetTableRecordExpirationConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTableRecordExpirationConfiguration

## Description
`GetTableRecordExpirationConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_record_expiration_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetTableRecordExpirationConfiguration` call, so each one unblocks only when `GetTableRecordExpirationConfiguration` works: `s3tables.s3t-record-expiration-reads-rejected-as-method-not-allowed`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_record_expiration_configuration.rs`. Replaying the corpus, the emulator refused the `GetTableRecordExpirationConfiguration` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_s3tables_s3t_record_expiration_reads_rejected_as_method_not_allowed.py", line 130, in run
    assert (code, message) == ("MethodNotAllowedException", MSG_NOT_ALLOWED), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: reading the record expiration configuration of a customer Iceberg table was refused with ('NotImplemented', 'operation GetTableRecordExpirationConfiguration is scaffolded but not implemented in this emulator'); real S3 Tables answers ('MethodNotAllowedException', 'Table record expiration configuration API calls to this table are not allowed'), because the surface does not apply to this kind of table rather than being missing or forbidden

## Steps to Reproduce
Boot the s3tables emulator
Call GetTableRecordExpirationConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
