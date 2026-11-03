Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.start-sandbox-connection-absent-sandbox-not-found
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.start-sandbox-connection-absent-sandbox-not-found|run#11|AssertionError|assert malformed_message == expected, f"expected message {expected!r}, got {malformed_message!r}"
Issue-Commit: 286c0019b2a6d6ba24e260ebac56f0f3f96c1e4a

# CodeBuild StartSandboxConnection answers the not-found message for a MALFORMED sandboxId instead of 'Invalid sandbox ID provided'

> _Note: this issue was identified against the code base at commit `286c0019b2a6d6ba24e260ebac56f0f3f96c1e4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSandboxConnection

## Description
One net-new root cause.

Cause 1 of 1: StartSandboxConnection collapses two distinct AWS refusals of `sandboxId` into a single message. Failing check: codebuild.start-sandbox-connection-absent-sandbox-not-found.

AWS: Smithy (services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json) models StartSandboxConnection.sandboxId as NonEmptyString @length min 1, required, documented as "A sandboxId or sandboxArn", with modelled errors InvalidInputException (HTTP 400) and ResourceNotFoundException (HTTP 400). The model does not state the message text, so for the wording this is recorded behavior only: the recording shows AWS answering InvalidInputException with TWO different messages depending on the SHAPE of the value, not on whether a sandbox exists. (a) A WELL-FORMED but absent id — the `<projectName>:<uuid>` form — is refused with the fixed text "No sandbox found with specified ID" (no variable parts; the id is not echoed). (b) A MALFORMED id — a value that is neither a sandboxId of that form nor a sandboxArn (in the recording, a bare name with no `:`+uuid) — is refused with the fixed text "Invalid sandbox ID provided" (again entirely fixed text, no variable parts). Both are InvalidInputException at HTTP 400. BatchGetSandboxes on the same well-formed absent id is NOT an error: it returns sandboxes=[] and lists the id in sandboxesNotFound — so the two surfaces are deliberately not collapsed.

Emulator: both inputs take the same path. The handler resolves the value through a single store lookup and, on any miss, returns the one constant message: the malformed id is refused InvalidInputException / "No sandbox found with specified ID" where AWS says "Invalid sandbox ID provided". The error code (InvalidInputException) and status (400) are correct; only the message diverges, and only for the malformed-shape class. There is no syntax check of `sandboxId` ahead of the lookup, so the emulator has no notion of a value that is not a valid sandbox identifier at all.

Why: a client that branches on the refusal message cannot tell a typo/garbled identifier (never retryable, fix the caller) from a correctly formed id whose sandbox is gone (expected after a sandbox terminates, often a benign race). Retry/alerting logic keyed to "Invalid sandbox ID provided" never fires against the emulator, and tests asserting AWS's wording fail.

Fix: StartSandboxConnection's validation, ahead of the state lookup — services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_sandbox_connection.rs. The handler needs to classify `sandboxId` by FORM first (sandboxId `<project>:<uuid>` or a sandbox ARN) and refuse an unparseable value with InvalidInputException carrying the fixed text "Invalid sandbox ID provided", keeping the existing not-found constant only for a well-formed id that resolves to no stored sandbox. The shared constant crate::state::UNRESOLVABLE_SANDBOX_ID_MESSAGE therefore needs a sibling for the malformed class rather than serving both.

Traces: codebuild.start-sandbox-connection-absent-sandbox-not-found. Handler and the single message it returns on every miss: services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/start_sandbox_connection.rs lines 151-161 (store lookup with no form check) and lines 231-236 (unresolvable_sandbox_id() returning crate::state::UNRESOLVABLE_SANDBOX_ID_MESSAGE).

## Evidence
Check codebuild.start-sandbox-connection-absent-sandbox-not-found (verdict: mismatch).

Step 1 (passes): start_sandbox_connection(sandboxId="<project>:<uuid>") for a project/uuid that does not exist -> InvalidInputException, message "No sandbox found with specified ID". Matches AWS.

Step 2 (fails): start_sandbox_connection(sandboxId="<a bare name with no ':<uuid>' suffix>") ->
  expected (AWS): InvalidInputException, HTTP 400, message "Invalid sandbox ID provided"
  actual (emulator): InvalidInputException, HTTP 400, message "No sandbox found with specified ID"
Replay assertion: AssertionError: expected message 'Invalid sandbox ID provided', got 'No sandbox found with specified ID' (check_codebuild_start_sandbox_connection_absent_sandbox_not_found.py line 36).

Both expected messages are entirely FIXED text — neither embeds the submitted id, a project name, a uuid or any other request/state value — so the requirement is the literal wording per class, selected by whether `sandboxId` parses as a sandboxId/sandboxArn.

Note: this issue is NOT covered by the open CodeBuild sandbox issues. 20261025T062553Z-codebuild-6-further-divergences-startbuildbatch-takes-no-input.md and 20261026T234503Z-codebuild-3-net-new-divergences-sandbox-command-stdout.md concern sandbox/command-execution lifecycle and stdout, not StartSandboxConnection's refusal wording; the replay report lists this check with no owning issue.

## Steps to Reproduce
1. Boot the emulator and point a botocore codebuild client at it.
2. Negative control (already correct): codebuild.start_sandbox_connection(sandboxId="someproject:11111111-2222-3333-4444-555555555555") -> observe InvalidInputException / "No sandbox found with specified ID".
3. The divergence: codebuild.start_sandbox_connection(sandboxId="not-a-sandbox-id") (no ':<uuid>' suffix, not an ARN) -> emulator answers InvalidInputException / "No sandbox found with specified ID"; AWS answers InvalidInputException / "Invalid sandbox ID provided".
4. Confirm the surfaces are not meant to be collapsed: codebuild.batch_get_sandboxes(ids=["someproject:11111111-2222-3333-4444-555555555555"]) returns sandboxes=[] with the id in sandboxesNotFound (no exception).
5. Re-replay codebuild.start-sandbox-connection-absent-sandbox-not-found to confirm the fix.
