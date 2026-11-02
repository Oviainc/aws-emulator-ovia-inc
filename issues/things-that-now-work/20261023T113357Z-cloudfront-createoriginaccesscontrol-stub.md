Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group cloudfront__logs.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery
Issue-Key: cloudfront|CreateOriginAccessControl|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement cloudfront CreateOriginAccessControl: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOriginAccessControl

## Description
`CreateOriginAccessControl` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_origin_access_control.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateOriginAccessControl` call, so each one unblocks only when `CreateOriginAccessControl` works: `cloudfront__logs.xsvc-cloudfront-3-standard-logging-v2-access-log-reaches-log-group`, `logs__cloudfront.obs-cloudfront-3-vended-access-logs-match-requests-and-stop-after-delete-delivery`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_origin_access_control.rs`. Replaying the corpus, the emulator refused the `CreateOriginAccessControl` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateOriginAccessControl operation: operation CreateOriginAccessControl is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudfront emulator
Call CreateOriginAccessControl and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
