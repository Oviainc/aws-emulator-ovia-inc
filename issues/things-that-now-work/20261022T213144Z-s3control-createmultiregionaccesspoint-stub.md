Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.multi-region-access-point-create-and-delete-are-async-operations s3control.multi-region-access-point-duplicate-name-create-races-rejected
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3-control|CreateMultiRegionAccessPoint|NotImplemented
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# Implement s3control CreateMultiRegionAccessPoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMultiRegionAccessPoint

## Description
`CreateMultiRegionAccessPoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/create_multi_region_access_point.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateMultiRegionAccessPoint` call, so each one unblocks only when `CreateMultiRegionAccessPoint` works: `s3control.multi-region-access-point-create-and-delete-are-async-operations`, `s3control.multi-region-access-point-duplicate-name-create-races-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/create_multi_region_access_point.rs`. Replaying the corpus, the emulator refused the `CreateMultiRegionAccessPoint` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateMultiRegionAccessPoint operation: operation CreateMultiRegionAccessPoint is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3control emulator
Call CreateMultiRegionAccessPoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
