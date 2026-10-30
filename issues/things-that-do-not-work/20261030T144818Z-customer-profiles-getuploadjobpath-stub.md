Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.start-upload-job-wrong-state-rejected customer-profiles.upload-job-state-transitions
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|GetUploadJobPath|NotImplemented
Issue-Commit: 43252aeb825d83cca4ce2cc37a2476b8a6fd6233

# Implement customer-profiles GetUploadJobPath: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `43252aeb825d83cca4ce2cc37a2476b8a6fd6233`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUploadJobPath

## Description
`GetUploadJobPath` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/get_upload_job_path.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetUploadJobPath` call, so each one unblocks only when `GetUploadJobPath` works: `customer-profiles.start-upload-job-wrong-state-rejected`, `customer-profiles.upload-job-state-transitions`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/get_upload_job_path.rs`. Replaying the corpus, the emulator refused the `GetUploadJobPath` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetUploadJobPath operation: operation GetUploadJobPath is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the customer-profiles emulator
Call GetUploadJobPath and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
