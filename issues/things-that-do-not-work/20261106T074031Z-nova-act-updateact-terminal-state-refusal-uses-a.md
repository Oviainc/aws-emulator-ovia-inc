Issue-Source: trace-sweep-divergence
Issue-Trace: nova-act-act-terminal-state-conflict
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: nova-act-act-terminal-state-conflict
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act|UpdateAct|4xx->4xx|$.message
Issue-Commit: e11062c5c03ce94fd3623f23516eeaf1cfc9993a

# nova-act UpdateAct terminal-state refusal uses a fixed non-AWS message instead of naming the admitted statuses

> _Note: this issue was identified against the code base at commit `e11062c5c03ce94fd3623f23516eeaf1cfc9993a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAct

## Description
Cause 1 of 1: UpdateAct refuses a status transition that the act's current (finished) status does not admit with the right HTTP status (400), the right error code (ValidationException) and the right ValidationException.reason (InvalidStatus), but the message text is a fixed sentence AWS never emits for this error class. Fixture nova-act-act-terminal-state-conflict, step task#7.

AWS: recorded behavior only — the Smithy model (services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/smithy-model/com-amazonaws-novaact-amazonnovaagentsdataplane.json) documents UpdateActRequest.status only as "The new status to set for the act." (smithy_query doc=UpdateActRequest/status) and says nothing about which statuses a given act state admits; UpdateAct models ValidationException (httpError 400), whose required members are message and reason. The recording for this step is HTTP 400, x-amzn-errortype ValidationException, reason InvalidStatus (the emulator's status, code and reason all match — only $.message diverged), with message exactly "Status must be one of [FAILED, SUCCEEDED]". Fixed text of this error class: "Status must be one of [", ", " between values, and "]". Variable part: the value set, which is the set of statuses the operation admits for the act in its current state, taken from service state and from the operation's transition rule — in the recorded case the two terminal statuses FAILED and SUCCEEDED. The message does NOT echo the rejected value, the act id, the session id or any other per-run value, so nothing in it is environment- or run-specific. Hypothesis: AWS treats UpdateAct's permitted-status set as a property of the act's current state (a finished act admits only the finishing statuses) and renders that set in the refusal; what is REQUIRED here is that the emulator emit this sentence with the statuses it actually admits, not a sentence with no value set at all.

Emulator: answers HTTP 400 / ValidationException / reason InvalidStatus with message "Act in unexpected state" — a constant with nothing interpolated. It is produced by the terminal-transition guard in the UpdateAct handler, which returns a private const sentence (ACT_IN_UNEXPECTED_STATE) whenever the stored act is already terminal and the requested status differs from the one it holds. The sentence shares no fixed text with AWS's and names none of the statuses the act admits, so no client can recover the admitted set from it.

Why: a client relying on AWS's behavior cannot tell which statuses the update would have accepted. SDK error matchers, conformance tests, CLI golden-output tests and log/alarm rules keyed on AWS's refusal text for an inadmissible act status all compare against "Status must be one of [...]" and see a message AWS never produces, so a correct 400 reads as an unexpected failure; retry logic that parses the admitted set to pick a legal status has nothing to parse and cannot make progress.

Fix: the UpdateAct handler's terminal-transition guard and the message it renders — the refusal must build the admitted-status set from the act's current state and render AWS's sentence, rather than returning a fixed const with no value set. If the sentence is to be shared with other state-transition refusals in this service it belongs with the service's other validation-message renderers rather than as an operation-private constant.

Related open issues, neither of which covers this cause: 20261106T045214Z-nova-act-updateact-validationexception-lists-the-actstatus-enum.md is about the OUT-OF-ENUM refusal on the same member ("1 validation error detected: ... Member must satisfy enum value set: [...]") and its value ordering — a different message, raised on a different branch (an unparseable status value) than this state-transition guard; 20261029T174545Z-nova-act-validationexception-message-text-diverges-from-aws.md is about pattern-violation message text. Neither mentions the admitted-status sentence for an inadmissible transition.

Traces: fixture nova-act-act-terminal-state-conflict step task#7 (the cluster's only member).

## Evidence
Wire replay of simparity-wire-aws-v1, fixture nova-act-act-terminal-state-conflict, step task#7, verdict mismatch (divergence_key service=nova-act|UpdateAct|status=400->400|keys=$.message, affected_count 1).

HTTP status: 400 expected, 400 actual (matches).
Error code / x-amzn-errortype: ValidationException expected and actual (matches; not reported as a diverging key).
ValidationException.reason: InvalidStatus (matches; not reported as a diverging key).
Message:
  Expected: "Status must be one of [FAILED, SUCCEEDED]"
  Actual:   "Act in unexpected state"
Replay detail verbatim: "$.message: value_mismatch (expected 'Status must be one of [FAILED, SUCCEEDED]', got 'Act in unexpected state')"

Fixed vs variable parts of the expected message: fixed text is "Status must be one of [", the ", " separator and "]"; the only variable part is the set of statuses admitted for the act in its current state (FAILED, SUCCEEDED in this recording). No request id, timestamp, act id or other per-run value appears in it, so the divergence is not a per-run difference.

Code citations (source of the diverging text):
  services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/src/ops/update_act.rs line 94 — const ACT_IN_UNEXPECTED_STATE: &str = "Act in unexpected state", documented as "Fixed text with nothing interpolated, so the refusal reads the same whichever act and whichever status a caller named".
  services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/src/ops/update_act.rs lines 224-234 — the guard `if act.status.is_terminal() && act.status != requested` returns ValidationException { message: ACT_IN_UNEXPECTED_STATE, reason: "InvalidStatus", field_list: None }.

Same-step context: setup#5 of this fixture (CreateAct) also diverges, but that cluster is owned by open issue 20261104T064305Z-nova-act-returns-static-placeholder-state-instead-of.md and is not re-filed here.

## Steps to Reproduce
1. Clone this checkout, boot the emulator from it and replay the wire corpus simparity-wire-aws-v1.
2. Replay fixture nova-act-act-terminal-state-conflict and stop at step task#7 (UpdateAct).
3. Equivalently, by hand against a nova-act client pinned to the fixture's region: CreateWorkflowDefinition with a name matching ^[a-zA-Z0-9_-]{1,40}$, CreateWorkflowRun under it, CreateSession under that run, CreateAct in that session, then UpdateAct to drive the act to a finishing status (e.g. SUCCEEDED).
4. Call UpdateAct again on the same workflowDefinitionName / workflowRunId / sessionId / actId with a DIFFERENT status than the one the act now holds.
5. Observe HTTP 400 with x-amzn-errortype ValidationException and reason InvalidStatus (all three correct) and read Error.Message: the emulator emits "Act in unexpected state".
6. The step passes only when the message is AWS's sentence naming the admitted statuses — for this recording "Status must be one of [FAILED, SUCCEEDED]".
7. Re-send the status the act already holds to confirm the fix keeps the @idempotent retry succeeding with 200 and an empty body.
