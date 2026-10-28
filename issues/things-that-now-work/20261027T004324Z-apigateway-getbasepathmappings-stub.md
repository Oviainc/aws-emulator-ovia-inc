Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.create-base-path-mapping-unknown-restapi-returns-badrequest apigateway.delete-rest-api-racing-base-path-mapping-leaves-orphaned-mapping apigateway.e2e-custom-domain-base-path-routing
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|GetBasePathMappings|NotImplemented
Issue-Commit: df009dd79b3a670d3c0c53c12bb5e37a115b862c

# Implement apigateway GetBasePathMappings: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `df009dd79b3a670d3c0c53c12bb5e37a115b862c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBasePathMappings

## Description
`GetBasePathMappings` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_base_path_mappings.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `GetBasePathMappings` call, so each one unblocks only when `GetBasePathMappings` works: `apigateway.create-base-path-mapping-unknown-restapi-returns-badrequest`, `apigateway.delete-rest-api-racing-base-path-mapping-leaves-orphaned-mapping`, `apigateway.e2e-custom-domain-base-path-routing`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_base_path_mappings.rs`. Replaying the corpus, the emulator refused the `GetBasePathMappings` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetBasePathMappings operation: operation GetBasePathMappings is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigateway emulator
Call GetBasePathMappings and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
