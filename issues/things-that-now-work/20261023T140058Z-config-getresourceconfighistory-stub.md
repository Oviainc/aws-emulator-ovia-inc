Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.config-rule-scoped-by-tag-evaluates-only-the-tagged-resource config.discovered-resource-counts-and-config-history config.get-resource-config-history-window-and-forward-order-select-real-items config.list-discovered-resources-omits-a-deleted-resource-until-include-deleted-requested
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.discovered-resource-counts-and-config-history|run#9|AssertionError|assert code == "ResourceNotDiscoveredException", (code, message)
Issue-Related: 20261022T190340Z-config-service-getresourceconfighistory-stub.md
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Implement config GetResourceConfigHistory: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
`GetResourceConfigHistory` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_resource_config_history.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `GetResourceConfigHistory` call, so each one unblocks only when `GetResourceConfigHistory` works: `config.config-rule-scoped-by-tag-evaluates-only-the-tagged-resource`, `config.discovered-resource-counts-and-config-history`, `config.get-resource-config-history-window-and-forward-order-select-real-items`, `config.list-discovered-resources-omits-a-deleted-resource-until-include-deleted-requested`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/get_resource_config_history.rs`. Replaying the corpus, the emulator refused the `GetResourceConfigHistory` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetResourceConfigHistory operation: operation GetResourceConfigHistory is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the config emulator
Call GetResourceConfigHistory and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
