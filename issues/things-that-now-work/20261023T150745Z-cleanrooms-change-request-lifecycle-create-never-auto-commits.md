Issue-Source: trace-sweep-divergence
Issue-Trace: cleanrooms-change-request-cancel-workflow
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanrooms-change-request-cancel-workflow
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms|CreateCollaborationChangeRequest|2xx->2xx|$.collaborationChangeRequest.isAutoApproved
Issue-Key: cleanrooms|CreateCollaborationChangeRequest|2xx->2xx|$.collaborationChangeRequest.status
Issue-Key: cleanrooms|GetCollaborationChangeRequest|2xx->2xx|$.collaborationChangeRequest.isAutoApproved
Issue-Key: cleanrooms|GetCollaborationChangeRequest|2xx->2xx|$.collaborationChangeRequest.status
Issue-Key: cleanrooms|UpdateCollaborationChangeRequest|4xx->2xx|$.message
Issue-Key: cleanrooms|UpdateCollaborationChangeRequest|4xx->2xx|$.reason
Issue-Key: cleanrooms|UpdateCollaborationChangeRequest|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cleanrooms|UpdateCollaborationChangeRequest|4xx->2xx|status
Issue-Commit: 326899db14d8647fedf8ea1301a88431f0a1aba6

# cleanrooms change-request lifecycle: create never auto-commits and a terminal request stays mutable

> _Note: this issue was identified against the code base at commit `326899db14d8647fedf8ea1301a88431f0a1aba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCollaborationChangeRequest

## Description
This bundle carries 3 distinct causes found by replaying the wire corpus (trace set simparity-wire-aws-v1) against the cleanrooms emulator. All three are in the collaboration change-request lifecycle and all must be closed separately before this issue is submitted; the bundle is only for filing economy. Affected operations: CreateCollaborationChangeRequest, GetCollaborationChangeRequest, UpdateCollaborationChangeRequest. state_reset_failures was 0 for this replay, so no fixture ran against a previous fixture's state and these results are not suspect.

Related open issues, none of which cover these steps: 20261023T101042Z-cleanrooms-five-behavioral-divergences-change-request-never-auto describes the same auto-commit fault from the BEHAVIORAL suite but carries no wire replay key (the replay reports owned_by=[] for all three clusters below), so Causes 1 and 2 are filed here to bind that fault to the wire fixture steps that must be re-replayed; the two stub issues (20261022T200707Z-cleanrooms-createanalysistemplate-stub, -listtagsforresource-stub) are unrelated unimplemented-operation work items.

=== Cause 1 of 3: CreateCollaborationChangeRequest returns status=PENDING / isAutoApproved=false where AWS returns COMMITTED / true ===
Fixture cleanrooms-change-request-cancel-workflow, step task#3.
AWS: recorded behavior only for the commit rule. Smithy (com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda): CreateCollaborationChangeRequest POST /collaborations/{collaborationIdentifier}/changeRequests, 200, output member collaborationChangeRequest -> CollaborationChangeRequest (required). On that shape, status -> ChangeRequestStatus (required enum PENDING|APPROVED|CANCELLED|DENIED|COMMITTED, documented as "The current status of the change request") and isAutoApproved -> Boolean (required, documented as "Whether the change request was automatically approved based on the collaboration's auto-approval settings"); the model states no rule for WHEN a request collapses straight to COMMITTED. The recording shows that for this collaboration the create response already carries status=COMMITTED and isAutoApproved=true in the 200 body, i.e. the request is auto-approved and applied synchronously with no outstanding approval from any other party.
Emulator: HTTP status matches (200), but the body diverges on two members: $.collaborationChangeRequest.isAutoApproved is false (expected true) and $.collaborationChangeRequest.status is 'PENDING' (expected 'COMMITTED'). The change is therefore never applied to the collaboration.
Why: a client that creates a change request and then drives a state machine off status/isAutoApproved waits for an approval that can never arrive — against the emulator there is no other party to approve, so the request is stuck PENDING forever and the collaboration modification it requested never takes effect. Everything downstream of the commit (the requested change being visible on the collaboration, and the terminal-state rules in Cause 3) is unreachable.
Fix: the auto-approval/commit decision in this operation's handler and the state predicate it calls — the decision must account for there being no outstanding third-party approval, not only for the collaboration's declared auto-approved change types.
Traces: cleanrooms-change-request-cancel-workflow#task#3.

=== Cause 2 of 3: GetCollaborationChangeRequest reads back the same wrong status/isAutoApproved ===
Fixture cleanrooms-change-request-cancel-workflow, step task#4.
AWS: Smithy: GetCollaborationChangeRequest GET /collaborations/{collaborationIdentifier}/changeRequests/{changeRequestIdentifier}, readonly, 200, output collaborationChangeRequest -> CollaborationChangeRequest (required). The recorded read-back returns the same committed request: status=COMMITTED, isAutoApproved=true — the committed state is durable and observable on a subsequent read, not just a create-time response field.
Emulator: 200 as expected, but $.collaborationChangeRequest.isAutoApproved is false (expected true) and $.collaborationChangeRequest.status is 'PENDING' (expected 'COMMITTED').
Why: this is the persisted-state half of the fault, and it is what a polling client sees. Even a client that ignores the create response and re-reads the change request — the normal pattern for an asynchronous approval workflow — never observes the terminal state, so a poll loop on status never terminates.
Fix: the stored change-request record's status/isAutoApproved (this service's state) and this operation's serializer path that projects them — the read must reflect a committed request, so if Cause 1 is fixed by correcting the record at creation this read follows; verify it independently rather than assuming it does.
Traces: cleanrooms-change-request-cancel-workflow#task#4.

=== Cause 3 of 3: UpdateCollaborationChangeRequest accepts an action on a terminal change request with 200 instead of refusing it with 400 ValidationException ===
Fixture cleanrooms-change-request-cancel-workflow, step task#5.
AWS: recorded behavior only; the message rule IS known because the message is composed from the request. Smithy: UpdateCollaborationChangeRequest PATCH /collaborations/{collaborationIdentifier}/changeRequests/{changeRequestIdentifier}, required input action -> ChangeRequestAction (enum APPROVE|DENY|CANCEL|COMMIT), modeled errors include ValidationException (HTTP 400, members message/reason/fieldList with reason -> ValidationExceptionReason = FIELD_VALIDATION_FAILED|INVALID_CONFIGURATION|INVALID_QUERY|IAM_SYNCHRONIZATION_DELAY) and ConflictException (409). The recording shows that once a change request is in a terminal state, any action on it is refused with HTTP 400, header x-amzn-errortype: ValidationException, and body reason='INVALID_CONFIGURATION'. The message rule is: it names the change request id from the request path and the terminal status it is in, and states that requests in COMMITTED or CANCELLED status cannot be modified — recorded example: "You can't update change request 069fb2f8-596e-4974-b79c-231d53b97226 because it's in COMMITTED status. Change requests in COMMITTED or CANCELLED status can't be modified." (the id is an example, the rule is the requirement). COMMITTED and CANCELLED are the two terminal statuses; PENDING/APPROVED/DENIED remain actionable.
Emulator: returns HTTP 200 with a success body — the action is applied. No error is produced at all: $headers.x-amzn-errortype is absent (expected 'ValidationException'), $.reason is absent (expected 'INVALID_CONFIGURATION'), $.message is absent.
Why: the terminal-state guard is the API's guarantee that an approved-and-applied collaboration change cannot be retroactively cancelled, denied or re-approved. A client that relies on the 400 to detect a lost race (two operators acting on the same request) instead gets a 200 and believes its mutation won, so the collaboration's effective configuration and the request's audit trail silently disagree. A client that treats 400/INVALID_CONFIGURATION as "already settled, stop retrying" also loses its loop exit.
Note for the fixer: this step's request targets a request that AWS had already committed at task#3, so against the current emulator the request is still PENDING (Causes 1-2) and the action is legitimately applicable. Fixing Causes 1-2 is therefore necessary but NOT sufficient — the guard itself must exist, or the same call against a request made terminal some other way (CANCEL, DENY) will still wrongly return 200. Close this cause by adding the terminal-status check, then re-replay task#5.
Fix: this operation's handler/validation — reject an action whose target change request is in COMMITTED or CANCELLED status with ValidationException (400), reason INVALID_CONFIGURATION, and a message built from the request's change-request id and its current terminal status.
Traces: cleanrooms-change-request-cancel-workflow#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, /__admin/health 200; 2 fixtures replayed, 14 steps, 5 passed, 9 mismatched clustering into 3 divergences, 0 errored, 0 blocked, score 35.7%, state_reset_failures=0, bootstrap_signal=false).

Cause 1 - fixture cleanrooms-change-request-cancel-workflow step task#3, operation CreateCollaborationChangeRequest, verdict mismatch, status 200 -> 200:
  $.collaborationChangeRequest.isAutoApproved: value_mismatch (expected True, got False)
  $.collaborationChangeRequest.status: value_mismatch (expected 'COMMITTED', got 'PENDING')
  divergence_key: service=cleanrooms|CreateCollaborationChangeRequest|status=200->200|keys=$.collaborationChangeRequest.isAutoApproved,$.collaborationChangeRequest.status

Cause 2 - fixture cleanrooms-change-request-cancel-workflow step task#4, operation GetCollaborationChangeRequest, verdict mismatch, status 200 -> 200:
  $.collaborationChangeRequest.isAutoApproved: value_mismatch (expected True, got False)
  $.collaborationChangeRequest.status: value_mismatch (expected 'COMMITTED', got 'PENDING')
  divergence_key: service=cleanrooms|GetCollaborationChangeRequest|status=200->200|keys=$.collaborationChangeRequest.isAutoApproved,$.collaborationChangeRequest.status

Cause 3 - fixture cleanrooms-change-request-cancel-workflow step task#5, operation UpdateCollaborationChangeRequest, verdict mismatch, status 400 -> 200:
  status: expected 400, got 200
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)
  $.reason: missing (expected 'INVALID_CONFIGURATION', got None)
  $.message: missing (expected "You can't update change request 069fb2f8-596e-4974-b79c-231d53b97226 because it's in COMMITTED status. Change requests in COMMITTED or CANCELLED status can't be modified.", got None) - the id is an example; the requirement is the status, errortype, reason and the message RULE (names the change-request id and its terminal status).
  divergence_key: service=cleanrooms|UpdateCollaborationChangeRequest|status=400->200|keys=$.message,$.reason,$headers.x-amzn-errortype,status

Model evidence (services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/smithy-model/com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda.json): CollaborationChangeRequest.status required enum ChangeRequestStatus [PENDING, APPROVED, CANCELLED, DENIED, COMMITTED]; CollaborationChangeRequest.isAutoApproved required Boolean; ChangeRequestAction [APPROVE, DENY, CANCEL, COMMIT]; ValidationException httpError 400 with members message/reason/fieldList; ValidationExceptionReason [FIELD_VALIDATION_FAILED, INVALID_CONFIGURATION, INVALID_QUERY, IAM_SYNCHRONIZATION_DELAY]; UpdateCollaborationChangeRequest errors include ValidationException (400) and ConflictException (409).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture cleanrooms-change-request-cancel-workflow from trace set simparity-wire-aws-v1 (replay_wire_corpus); steps task#3, task#4 and task#5 are the three failing steps.
3. Or reproduce directly with a botocore cleanrooms client pointed at the emulator:
   a. create_collaboration + create_membership to get a collaboration, then create_collaboration_change_request(collaborationIdentifier=<collab>, changes=[<one change>]). Cause 1: inspect collaborationChangeRequest.status and .isAutoApproved - AWS returns COMMITTED/true, emulator returns PENDING/false.
   b. get_collaboration_change_request(collaborationIdentifier=<collab>, changeRequestIdentifier=<id from (a)>). Cause 2: same two members diverge on read-back.
   c. Cause 3: with that change request in a terminal status (COMMITTED per AWS, or reach CANCELLED via update_collaboration_change_request(action='CANCEL')), call update_collaboration_change_request(collaborationIdentifier=<collab>, changeRequestIdentifier=<id>, action='CANCEL'). AWS: HTTP 400, x-amzn-errortype ValidationException, reason INVALID_CONFIGURATION, message naming the change-request id and its terminal status. Emulator: HTTP 200 with the action applied and no error members at all.
4. Re-replay all three step keys (task#3, task#4, task#5) after fixing; task#5 needs its own terminal-status guard and does not pass merely because task#3 was corrected.
