Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-agent.asset-file-content-round-trips-through-zip-export
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|GetAssetContent|NotImplemented
Issue-Commit: 435579c3f4f417d2ee68a6d86543c2b3ca676fd8

# Implement devops-agent GetAssetContent: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `435579c3f4f417d2ee68a6d86543c2b3ca676fd8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAssetContent

## Description
`GetAssetContent` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_asset_content.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetAssetContent` call, so each one unblocks only when `GetAssetContent` works: `devops-agent.asset-file-content-round-trips-through-zip-export`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_asset_content.rs`. Replaying the corpus, the emulator refused the `GetAssetContent` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetAssetContent operation: operation GetAssetContent is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the devops-agent emulator
Call GetAssetContent and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
