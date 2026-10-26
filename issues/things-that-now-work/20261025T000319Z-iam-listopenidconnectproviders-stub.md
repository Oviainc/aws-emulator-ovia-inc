Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.create-open-id-connect-provider-duplicate-url-rejected iam.op-contract-list-open-id-connect-providers
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListOpenIDConnectProviders|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Implement iam ListOpenIDConnectProviders: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListOpenIDConnectProviders

## Description
`ListOpenIDConnectProviders` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_open_id_connect_providers.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListOpenIDConnectProviders` call, so each one unblocks only when `ListOpenIDConnectProviders` works: `iam.create-open-id-connect-provider-duplicate-url-rejected`, `iam.op-contract-list-open-id-connect-providers`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_open_id_connect_providers.rs`. Replaying the corpus, the emulator refused the `ListOpenIDConnectProviders` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListOpenIDConnectProviders operation: operation ListOpenIDConnectProviders is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call ListOpenIDConnectProviders and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
