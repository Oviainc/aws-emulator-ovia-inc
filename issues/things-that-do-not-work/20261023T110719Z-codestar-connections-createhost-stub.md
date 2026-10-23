Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codestar-connections.create-connection-on-deleted-host-rejected codestar-connections.create-host-duplicate-names-allowed-distinct-arns codestar-connections.create-host-with-tags-and-list-hosts-round-trip codestar-connections.create-host-with-tags-persisted codestar-connections.delete-host-removes-from-list codestar-connections.delete-host-with-active-connection-rejected codestar-connections.get-connection-reflects-host-association codestar-connections.host-appears-in-list-hosts codestar-connections.host-created-in-pending-status codestar-connections.host-status-message-present-when-pending codestar-connections.list-connections-host-arn-filter-matches codestar-connections.update-host-provider-endpoint-persisted
Issue-Shape-Id: com.amazonaws.codestarconnections#CodeStar_connections_20191201
Issue-Key: codestar-connections|CreateHost|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement codestar-connections CreateHost: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHost

## Description
`CreateHost` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/create_host.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `CreateHost` call, so each one unblocks only when `CreateHost` works: `codestar-connections.create-connection-on-deleted-host-rejected`, `codestar-connections.create-host-duplicate-names-allowed-distinct-arns`, `codestar-connections.create-host-with-tags-and-list-hosts-round-trip`, `codestar-connections.create-host-with-tags-persisted`, `codestar-connections.delete-host-removes-from-list`, `codestar-connections.delete-host-with-active-connection-rejected`, `codestar-connections.get-connection-reflects-host-association`, `codestar-connections.host-appears-in-list-hosts`, `codestar-connections.host-created-in-pending-status`, `codestar-connections.host-status-message-present-when-pending`, `codestar-connections.list-connections-host-arn-filter-matches`, `codestar-connections.update-host-provider-endpoint-persisted`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/create_host.rs`. Replaying the corpus, the emulator refused the `CreateHost` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateHost operation: operation amazonaws.codestar.connections.CodeStar_connections_20191201.CreateHost is not implemented in this emulator

## Steps to Reproduce
Boot the codestar-connections emulator
Call CreateHost and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
