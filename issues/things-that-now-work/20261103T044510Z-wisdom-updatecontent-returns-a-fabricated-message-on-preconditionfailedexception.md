Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wisdom.content-update-stale-revision-precondition-failed-rejected
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom.content-update-stale-revision-precondition-failed-rejected|run#16|AssertionError|assert message == "", f"expected an empty PreconditionFailedException message, got {message!r}"
Issue-Commit: 2df6d794dcac4d826ad28dfabdbc81c1503508b3

# Wisdom UpdateContent returns a fabricated message on PreconditionFailedException (AWS sends it empty)

> _Note: this issue was identified against the code base at commit `2df6d794dcac4d826ad28dfabdbc81c1503508b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateContent

## Description
Cause 1 of 1: UpdateContent's stale-revision refusal carries a message body that AWS does not send.

AWS: Smithy (services/service-com-amazonaws-wisdom-wisdomservice/smithy-model/com-amazonaws-wisdom-wisdomservice.json) models UpdateContent with error PreconditionFailedException (client, HTTP 412), documented "The provided revisionId does not match, indicating the content has been modified since it was last read", whose only member is an OPTIONAL `message` (target String). Recorded behavior only: on a real wisdom endpoint an UpdateContent naming a revisionId that is no longer the content's current revision is refused with HTTP 412, error code `PreconditionFailedException`, and an EMPTY message — the error carries no `message` text at all, so botocore surfaces Error.Message as "". There is no fixed text and no variable part for this error class; the whole refusal is the 412 + the code.

Emulator: the handler refuses the stale revision with code PreconditionFailedException but populates `message` from a hard-coded constant, so the client sees Error.Message == 'Request precondition failed' instead of ''. The replayed check fails at `assert message == ""` with: AssertionError: expected an empty PreconditionFailedException message, got 'Request precondition failed'. The 412 status, the error code, the write-nothing semantics and the positive control (update with the CURRENT revisionId succeeds and bumps revisionId) all behave correctly — only the message text diverges.

Why: a client that matches or asserts on the refusal's message — error-equality checks, log/alert matching, test fixtures recorded against AWS, or any code that treats a non-empty Message as additional server detail — sees a sentence AWS never emits. Parity harnesses comparing emulator and AWS error payloads for this operation report a spurious difference, and a client that echoes the Message to a user shows wording that does not exist in AWS.

Fix: the UpdateContent handler's optimistic-lock refusal path — it must construct PreconditionFailedException with no message (message: None) rather than from the local stale-revision constant. The same fabricated-text question applies to any other wisdom path that raises PreconditionFailedException; only this one is grounded in a failing check.

Note: the open issue 20261030T000858Z-wisdom-updatecontent-stub.md ("Implement wisdom UpdateContent: unimplemented scaffold stub") lists this same check id, but it is about UpdateContent answering 501; UpdateContent is now implemented in this checkout and answers the stale-revision case with a 412, so this message divergence is NOT covered by it.

Traces: wisdom.content-update-stale-revision-precondition-failed-rejected; handler at services/service-com-amazonaws-wisdom-wisdomservice/src/ops/update_content.rs lines 133-141 (MESSAGE_STALE_REVISION) and 228-239 (the refusal that sets it).

## Evidence
Check wisdom.content-update-stale-revision-precondition-failed-rejected [mismatch]:

  File "check_wisdom_content_update_stale_revision_precondition_failed_rejected.py", line 65, in run
    assert message == "", f"expected an empty PreconditionFailedException message, got {message!r}"
  AssertionError: expected an empty PreconditionFailedException message, got 'Request precondition failed'

Expected (AWS, recorded): HTTP 412, Error.Code == "PreconditionFailedException", Error.Message == "" (member absent).
Actual (emulator): HTTP 412, Error.Code == "PreconditionFailedException", Error.Message == "Request precondition failed".

Source: services/service-com-amazonaws-wisdom-wisdomservice/src/ops/update_content.rs
  line 141: const MESSAGE_STALE_REVISION: &str = "Request precondition failed";
  lines 233-238: PreconditionFailedException { message: Some(MESSAGE_STALE_REVISION.to_string()) }

Model: PreconditionFailedException { message: String (optional) }, errorKind client, httpError 412; bound to UpdateContent.

## Steps to Reproduce
1. Boot the wisdom emulator.
2. CreateKnowledgeBase(name=<unique>, knowledgeBaseType="CUSTOM") and take knowledgeBaseId.
3. StartContentUpload(knowledgeBaseId, contentType="text/plain"), PUT some bytes to the returned url, take uploadId.
4. CreateContent(knowledgeBaseId, name=<unique>, uploadId, title="Title V0"); poll GetContent until status == ACTIVE and record revisionId R0.
5. UpdateContent(knowledgeBaseId, contentId, revisionId=R0, title="Title V1") — succeeds and returns a new revisionId R1.
6. UpdateContent(knowledgeBaseId, contentId, revisionId=R0, title="Title V1 Stale Attempt") — refused with HTTP 412 PreconditionFailedException.
7. Inspect the error payload: AWS sends no message (Error.Message == ""); the emulator sends Error.Message == "Request precondition failed".
