Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: nova-act.act-create-initial-status-running nova-act.act-update-lifecycle-running-to-succeeded nova-act.concurrent-update-workflow-run-terminal-status-race nova-act.create-session-idempotent-same-token nova-act.delete-workflow-definition-with-active-run-rejected nova-act.invoke-act-step-call-id-mismatch-rejected nova-act.list-acts-filters-by-session-id nova-act.list-models-returns-supported-models nova-act.list-workflow-runs-reflects-status-transition nova-act.session-create-and-list-under-workflow-run nova-act.session-cross-workflow-run-isolation-rejected nova-act.update-act-after-terminal-status-rejected nova-act.update-act-invalid-status-enum-rejected nova-act.workflow-run-update-to-succeeded-sets-ended-at
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act|ListModels|NotImplemented
Issue-Commit: e0f3b342c2995e24bb81f3397837b3776f1c553b

# Implement nova-act ListModels: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e0f3b342c2995e24bb81f3397837b3776f1c553b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListModels

## Description
`ListModels` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/src/ops/list_models.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `ListModels` call, so each one unblocks only when `ListModels` works: `nova-act.act-create-initial-status-running`, `nova-act.act-update-lifecycle-running-to-succeeded`, `nova-act.concurrent-update-workflow-run-terminal-status-race`, `nova-act.create-session-idempotent-same-token`, `nova-act.delete-workflow-definition-with-active-run-rejected`, `nova-act.invoke-act-step-call-id-mismatch-rejected`, `nova-act.list-acts-filters-by-session-id`, `nova-act.list-models-returns-supported-models`, `nova-act.list-workflow-runs-reflects-status-transition`, `nova-act.session-create-and-list-under-workflow-run`, `nova-act.session-cross-workflow-run-isolation-rejected`, `nova-act.update-act-after-terminal-status-rejected`, `nova-act.update-act-invalid-status-enum-rejected`, `nova-act.workflow-run-update-to-succeeded-sets-ended-at`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/src/ops/list_models.rs`. Replaying the corpus, the emulator refused the `ListModels` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListModels operation: operation ListModels is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the nova-act emulator
Call ListModels and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
