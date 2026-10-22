Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: support-app.delete-slack-workspace-configuration-malformed-team-id-invalid
Issue-Shape-Id: com.amazonaws.supportapp#SupportApp
Issue-Key: support-app.delete-slack-workspace-configuration-malformed-team-id-invalid|run#4|AssertionError|assert control_code == "ResourceNotFoundException", (
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# Implement support-app DeleteSlackWorkspaceConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSlackWorkspaceConfiguration

## Description
`DeleteSlackWorkspaceConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-supportapp-supportapp/src/ops/delete_slack_workspace_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteSlackWorkspaceConfiguration` call, so each one unblocks only when `DeleteSlackWorkspaceConfiguration` works: `support-app.delete-slack-workspace-configuration-malformed-team-id-invalid`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-supportapp-supportapp/src/ops/delete_slack_workspace_configuration.rs`. Replaying the corpus, the emulator refused the `DeleteSlackWorkspaceConfiguration` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_support_app_delete_slack_workspace_configuration_malformed_team_id_invalid.py", line 27, in run
    assert control_code == "ResourceNotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: positive control: a syntactically valid, never-registered teamId must reach the lookup and get ResourceNotFoundException, got NotImplemented: operation DeleteSlackWorkspaceConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the support-app emulator
Call DeleteSlackWorkspaceConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
