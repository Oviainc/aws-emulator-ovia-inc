Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.resource-config-history-chronological-ordering
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config-service|PutResourceConfig|NotImplemented
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Implement config PutResourceConfig: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourceConfig

## Description
`PutResourceConfig` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_resource_config.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutResourceConfig` call, so each one unblocks only when `PutResourceConfig` works: `config.resource-config-history-chronological-ordering`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_resource_config.rs`. Replaying the corpus, the emulator refused the `PutResourceConfig` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_config_resource_config_history_chronological_ordering.py", line 209, in run
    assert exc.response["Error"]["Code"] == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException for an unregistered resource type, got 'NotImplemented'

## Steps to Reproduce
Boot the config emulator
Call PutResourceConfig and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Status note (left by the GetDiscoveredResourceCounts fixer)
`PutResourceConfig` no longer answers 501: implementing `GetDiscoveredResourceCounts`
needed a writer for the discovered-resource inventory, so the handler was implemented
in that change (it records a custom resource type into
`state.discovered_resources`, refusing a reserved organization or a malformed registry
type name, a `Configuration` that is not valid JSON or over 64 KB, a request with no
running configuration recorder, and a new resource past the 100,000 active custom
resources limit), and its `//! SCAFFOLDED, NOT IMPLEMENTED:` census line was deleted.
This issue stays open for the scope that change did not cover:

* the resource type is not checked against anything registered with CloudFormation —
  only against the type-name form and the reserved organizations;
* nothing reads a recorded `Configuration` document back yet, because
  `GetResourceConfigHistory` and `BatchGetResourceConfig` are still scaffolds with
  issues of their own;
* `DeleteResourceConfig`, which records a resource's deletion, is still a scaffold.

Its recorded unit (`config.resource-config-history-chronological-ordering`) also needs
`GetResourceConfigHistory`, so it is still blocked.
