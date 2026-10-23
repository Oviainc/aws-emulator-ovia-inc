Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.get-export-conflict apigateway.get-export-generates-openapi-definition apigateway.get-export-generates-valid-openapi-for-stage apigateway.get-export-oas30-vs-swagger-format-differs-server-side apigateway.get-export-produces-openapi-spec apigateway.get-export-reflects-live-api-state apigateway.get-export-server-generates-cors-configuration apigateway.op-contract-getExport
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|GetExport|NotImplemented
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# Implement apigateway GetExport: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExport

## Description
`GetExport` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_export.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `GetExport` call, so each one unblocks only when `GetExport` works: `apigateway.get-export-conflict`, `apigateway.get-export-generates-openapi-definition`, `apigateway.get-export-generates-valid-openapi-for-stage`, `apigateway.get-export-oas30-vs-swagger-format-differs-server-side`, `apigateway.get-export-produces-openapi-spec`, `apigateway.get-export-reflects-live-api-state`, `apigateway.get-export-server-generates-cors-configuration`, `apigateway.op-contract-getExport`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_export.rs`. Replaying the corpus, the emulator refused the `GetExport` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetExport operation: operation GetExport is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigateway emulator
Call GetExport and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
