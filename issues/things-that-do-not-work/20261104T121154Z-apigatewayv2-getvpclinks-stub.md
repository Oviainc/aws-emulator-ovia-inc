Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.get-vpc-link-after-delete-not-found apigatewayv2.list-vpc-links-reflects-created apigatewayv2.update-vpc-link-invalid-security-group-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|GetVpcLinks|NotImplemented
Issue-Commit: 08672d49945dcbdaeb30ca89916c398a9fb8e682

# Implement apigatewayv2 GetVpcLinks: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `08672d49945dcbdaeb30ca89916c398a9fb8e682`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVpcLinks

## Description
`GetVpcLinks` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/get_vpc_links.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `GetVpcLinks` call, so each one unblocks only when `GetVpcLinks` works: `apigatewayv2.get-vpc-link-after-delete-not-found`, `apigatewayv2.list-vpc-links-reflects-created`, `apigatewayv2.update-vpc-link-invalid-security-group-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/get_vpc_links.rs`. Replaying the corpus, the emulator refused the `GetVpcLinks` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetVpcLinks operation: operation GetVpcLinks is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigatewayv2 emulator
Call GetVpcLinks and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
