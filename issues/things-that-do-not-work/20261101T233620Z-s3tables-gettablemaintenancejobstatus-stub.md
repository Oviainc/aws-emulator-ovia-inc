Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.table-maintenance-job-status-progresses-to-complete s3tables.table-maintenance-job-status-reflects-execution
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|GetTableMaintenanceJobStatus|NotImplemented
Issue-Commit: 2483fdb6af8e19e16b529c88e6a2504bdf5a7762

# Implement s3tables GetTableMaintenanceJobStatus: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2483fdb6af8e19e16b529c88e6a2504bdf5a7762`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTableMaintenanceJobStatus

## Description
`GetTableMaintenanceJobStatus` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_maintenance_job_status.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetTableMaintenanceJobStatus` call, so each one unblocks only when `GetTableMaintenanceJobStatus` works: `s3tables.table-maintenance-job-status-progresses-to-complete`, `s3tables.table-maintenance-job-status-reflects-execution`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_maintenance_job_status.rs`. Replaying the corpus, the emulator refused the `GetTableMaintenanceJobStatus` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetTableMaintenanceJobStatus operation: operation GetTableMaintenanceJobStatus is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3tables emulator
Call GetTableMaintenanceJobStatus and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
