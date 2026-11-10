Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: nova-act.update-act-invalid-status-enum-rejected
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act.update-act-invalid-status-enum-rejected|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 92e1207f9957e93edc6c957f01f4316de742e856

# nova-act UpdateAct ValidationException lists the ActStatus enum value set in model-declaration order, not AWS's order

> _Note: this issue was identified against the code base at commit `92e1207f9957e93edc6c957f01f4316de742e856`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAct

## Description
Cause 1 of 1: UpdateAct's refusal of a `status` value outside the ActStatus enum carries the right status (400) and the right error code (ValidationException), but it renders the enum value set in a different ORDER than AWS, so the message text does not match AWS's.

AWS: Smithy model services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/smithy-model/com-amazonaws-novaact-amazonnovaagentsdataplane.json — UpdateAct.status is required and targets ActStatus, an enum whose declared values are RUNNING, PENDING_CLIENT_ACTION, PENDING_HUMAN_ACTION, SUCCEEDED, FAILED, TIMED_OUT; UpdateAct models ValidationException (httpError 400). The recorded AWS refusal is HTTP 400, code ValidationException, message exactly "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [SUCCEEDED, PENDING_CLIENT_ACTION, TIMED_OUT, PENDING_HUMAN_ACTION, FAILED, RUNNING]". Fixed text of this error class: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", ", " between values, and "]". Variable parts: the member name taken from the request's rejected member (here `status`), and the value set taken from that member's modeled enum. The rejected value itself is NOT echoed. The ORDER AWS emits the value set in is NOT the model's declaration order — for ActStatus it is SUCCEEDED, PENDING_CLIENT_ACTION, TIMED_OUT, PENDING_HUMAN_ACTION, FAILED, RUNNING, and it is stable across runs (it is not a per-run value: no id, name, number or timestamp appears in this message). Hypothesis: AWS's validation layer iterates the service's internal hash-ordered set of permitted values rather than the declaration list; what is REQUIRED here is only that the emulator reproduce the same, stable order AWS emits for this member's enum.

Emulator: returns HTTP 400 / ValidationException with message "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [RUNNING, PENDING_CLIENT_ACTION, PENDING_HUMAN_ACTION, SUCCEEDED, FAILED, TIMED_OUT]" — the member name, the fixed text and the absence of the echoed value are all correct, but the value set is emitted in the model's declaration order. The handler builds the list from crate::state::ActStatus::ALL in declared order and hands it to service_common::validation_message::must_be_one_of, and its own doc comment states the choice to order "the values in the order the model declares them", so the divergence is deliberate rather than incidental.

Why: a client that asserts on the refusal text — a conformance test, a CLI golden-output test, an SDK-level error matcher, or any log/alarm rule keyed on the message AWS emits for an out-of-enum status — compares a message AWS never produces and treats the emulator's correct 400 as a mismatch. Because the ordering choice lives in the SHARED enum-message helper, every other member of this service validated against an enum will diverge the same way, not just UpdateAct.status.

Fix: the shared enum-value-set message renderer used by this service's validation (service_common::validation_message::must_be_one_of) and the ordering the UpdateAct handler feeds it: the enum value set must be rendered in the order AWS emits for that enum, not the model's declaration order. The boundary-walk clause in crate::service::validation makes the same declaration-order choice and must be changed with it, so an out-of-enum value refused at the boundary and one refused in the handler read identically.

Note: open issue 20261029T174545Z-nova-act-validationexception-message-text-diverges-from-aws.md covers a DIFFERENT cause in the same message family (pattern-violation messages echoing the rejected value and keeping the @pattern anchors). It does not mention the enum value set or its ordering, so it does not cover this cause.

Traces: behavioral check nova-act.update-act-invalid-status-enum-rejected.

## Evidence
Behavioral check nova-act.update-act-invalid-status-enum-rejected [mismatch].

The check creates a workflow definition, a workflow run and a session, proves the positive control (UpdateAct with status=SUCCEEDED is accepted and ListActs reports SUCCEEDED), then calls UpdateAct on a fresh act with status="NOT_A_REAL_STATUS".

Error code: ValidationException expected, ValidationException actual (code matches).
HTTP status: 400 expected, 400 actual (status matches).
Message:
  Expected: "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [SUCCEEDED, PENDING_CLIENT_ACTION, TIMED_OUT, PENDING_HUMAN_ACTION, FAILED, RUNNING]"
  Actual:   "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [RUNNING, PENDING_CLIENT_ACTION, PENDING_HUMAN_ACTION, SUCCEEDED, FAILED, TIMED_OUT]"

Assertion raised at check_nova_act_update_act_invalid_status_enum_rejected.py line 85:
  AssertionError: expected message "...[SUCCEEDED, PENDING_CLIENT_ACTION, TIMED_OUT, PENDING_HUMAN_ACTION, FAILED, RUNNING]", got "...[RUNNING, PENDING_CLIENT_ACTION, PENDING_HUMAN_ACTION, SUCCEEDED, FAILED, TIMED_OUT]"

The two sets contain exactly the same six values; only the order differs. The emulator's order is identical to the model's declaration order, confirmed via smithy_query(query="shape", shape="ActStatus"): ["RUNNING","PENDING_CLIENT_ACTION","PENDING_HUMAN_ACTION","SUCCEEDED","FAILED","TIMED_OUT"].

Code citations (ordering source):
  services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/src/ops/update_act.rs lines 184-203 — builds `declared` from crate::state::ActStatus::ALL in declaration order and passes it to service_common::validation_message::must_be_one_of("status", &declared); the comment at lines 188-191 states the declaration-order choice explicitly and points at crate::service::validation making the same choice for the boundary walk.

The act's stored status is left untouched by the rejected update (the check's follow-up ListActs assertion is not what fails), so only the message text diverges.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite simparity-behavioral-aws-v1, check nova-act.update-act-invalid-status-enum-rejected.
2. Against a nova-act client pinned to us-east-1: CreateWorkflowDefinition with a name matching ^[a-zA-Z0-9_-]{1,40}$; CreateWorkflowRun under it with a modelId from ListModels; CreateSession under that run; CreateAct in that session.
3. Call UpdateAct with that workflowDefinitionName / workflowRunId / sessionId / actId and status="NOT_A_REAL_STATUS".
4. Observe HTTP 400 with Error.Code ValidationException (both correct) and read Error.Message. The emulator emits the enum value set as [RUNNING, PENDING_CLIENT_ACTION, PENDING_HUMAN_ACTION, SUCCEEDED, FAILED, TIMED_OUT].
5. The check passes only when the message equals "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [SUCCEEDED, PENDING_CLIENT_ACTION, TIMED_OUT, PENDING_HUMAN_ACTION, FAILED, RUNNING]" — same six values, AWS's order.
6. Re-run the positive control in the same check (UpdateAct with status="SUCCEEDED") to confirm the fix did not turn a valid status into a refusal.
