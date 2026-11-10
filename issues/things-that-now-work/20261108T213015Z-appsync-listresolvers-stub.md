Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.list-child-resources-nonexistent-parent-not-found
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListResolvers|NotImplemented
Issue-Commit: 2233db37534078619f28c37e0e1db9dccbe7de65

# Implement appsync ListResolvers: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2233db37534078619f28c37e0e1db9dccbe7de65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResolvers

## Description
`ListResolvers` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/list_resolvers.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListResolvers` call, so each one unblocks only when `ListResolvers` works: `appsync.list-child-resources-nonexistent-parent-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/list_resolvers.rs`. Replaying the corpus, the emulator refused the `ListResolvers` call: File "check_appsync_list_child_resources_nonexistent_parent_not_found.py", line 80, in run
    _assert_ok(
  File "check_appsync_list_child_resources_nonexistent_parent_not_found.py", line 20, in _assert_ok
    raise AssertionError(
AssertionError: ListResolvers positive control: expected success against a live apiId, got NotImplemented

## Steps to Reproduce
Boot the appsync emulator
Call ListResolvers and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
