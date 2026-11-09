Issue-Source: trace-sweep-divergence
Issue-Trace: dataexchange-job-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dataexchange-job-error-paths
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange|StartJob|4xx->4xx|$.Message
Issue-Key: dataexchange|CancelJob|4xx->4xx|$.Message
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# DataExchange CancelJob and StartJob client errors use emulator prose instead of AWS fixed messages

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelJob

## Description
Three net-new causes, one shared rule: on the job-plane operations CancelJob and StartJob the emulator returns the correct HTTP status and the correct error code, and diverges only in the error body's $.Message, which it composes itself out of request and state values instead of emitting AWS's fixed sentence. DataExchange's client errors carry the identity of the resource in the exception's own ResourceId/ResourceType members, never interpolated into the prose (Smithy: ResourceNotFoundException { Message: __string required, ResourceId: __string "The unique identifier for the resource that couldn't be found.", ResourceType: ResourceType } @error client @httpError 404; ConflictException { Message: __string required, ResourceId: __string "The unique identifier for the resource with the conflict.", ResourceType: ResourceType } @error client @httpError 409).

Related open issues, none of which covers any cause below. 20261029T172356Z-dataexchange-client-error-messages-are-emulator-prose-instead.md states the same wording rule from the WIRE corpus but names only GetDataSet, GetEventAction, UpdateEventAction, DeleteEventAction, GetJob and CreateJob — not StartJob's or CancelJob's lookup, and not ConflictException at all; the replay report confirms it does not own these steps. 20261029T183418Z-dataexchange-resourcenotfoundexception-message-is-emulator-composed-prose-with.md covers ResourceNotFoundException for GetDataSet/GetJob/EventAction/CreateEventAction/SendDataSetNotification only. 20261029T205114Z-dataexchange-job-state-outcome-fidelity-conflictexception-refusal-wording.md describes the CancelJob ConflictException wording of cause 3, but it is a BEHAVIORAL issue carrying only behavioral check keys, so no wire fixture step is bound to it and cause 3's recorded step is re-replayed by nothing until it is filed here.

Cause 1 of 3: StartJob on a well-formed but never-created JobId returns 404 ResourceNotFoundException with an emulator-composed Message.
AWS: 404 ResourceNotFoundException whose Message is the FIXED text 'The requested resource was not found.' — the sentence has no variable part; the missing id travels in ResourceId/ResourceType (Smithy: StartJob PATCH /v1/jobs/{JobId}, success code 202, errors include ResourceNotFoundException @httpError 404; JobId @httpLabel required, pattern ^[a-zA-Z0-9]{30,40}$). The exact sentence is recorded behavior; the model documents only that the resource couldn't be found.
Emulator: correct 404 and correct ResourceNotFoundException code, but Message is the emulator's own template 'The job <JobId> couldn't be found.', where <JobId> is the JobId taken verbatim from the request path. (One recorded run read "The job aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa couldn't be found."; that id is one request's value filling the template, not part of any requirement.)
Why: a client that matches, logs or asserts on AWS's documented refusal text sees text AWS never serves, and the missing resource's identity arrives only inside prose instead of in the ResourceId/ResourceType members a client reads programmatically.
Fix: the dataexchange crate's shared not-found error constructor, reached from StartJob's job lookup in StartJob's handler — the status and the code are already right, so only the message the constructor mints is at fault.
Traces: dataexchange-job-error-paths#task#4.

Cause 2 of 3: CancelJob on a well-formed but never-created JobId returns 404 ResourceNotFoundException with the same emulator-composed Message.
AWS: 404 ResourceNotFoundException, Message exactly 'The requested resource was not found.' (Smithy: CancelJob DELETE /v1/jobs/{JobId}, success code 204, errors include ResourceNotFoundException @httpError 404).
Emulator: correct 404 / ResourceNotFoundException, Message 'The job <JobId> couldn't be found.' — the same interpolated template as cause 1, raised from CancelJob's own lookup site.
Why: same as cause 1.
Fix: the same shared not-found constructor, reached from CancelJob's job lookup.
Traces: dataexchange-job-error-paths#task#5.

Cause 3 of 3: CancelJob on a job that is not in the WAITING state returns the right 409 ConflictException but the wrong Message.
AWS: 409 ConflictException with the FIXED message 'Cannot cancel the requested Job. To cancel a Job, it must be in the WAITING state.' — no job id and no observed state is interpolated; the conflicting resource is identified by ConflictException.ResourceId/ResourceType (Smithy: CancelJob errors include ConflictException @httpError 409; ConflictException.Message required). The model documents the rule that a job can be cancelled only from WAITING; the exact sentence is recorded behavior.
Emulator: correct 409 and correct ConflictException code, and the stored job state is correctly left unchanged, but Message is the emulator's own sentence of the form 'The job <JobId> is in state <State> and can't be cancelled. Jobs can be cancelled only when they are in the WAITING state.' Both <JobId> (from the request) and <State> (from the emulator's stored job) are interpolated values AWS never emits. (One recorded run read "The job eba3e6946b7ea341e460c8319d8010bb is in state CANCELLED and can't be cancelled. Jobs can be cancelled only when they are in the WAITING state.")
Why: the refusal is recognisable to AWS clients only by its fixed text, so a client keyed on it treats the conflict as an unknown failure; and the state/id that a client should read from ResourceId/ResourceType is available only by parsing prose.
Fix: the shared conflict-error constructor the dataexchange job handlers call, plus the per-operation verb text that feeds it — CancelJob's state machine itself is already correct.
Traces: dataexchange-job-error-paths#task#10.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator. In all three causes the HTTP status and the error code match the recording exactly; only $.Message diverges.

Cause 1 — fixture dataexchange-job-error-paths step task#4, StartJob. expected status 404 / actual status 404.
  $.Message expected: 'The requested resource was not found.'
  $.Message actual:   "The job aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa couldn't be found."
The required text is wholly fixed; the emulator's variable part is the request's JobId.

Cause 2 — fixture dataexchange-job-error-paths step task#5, CancelJob. expected status 404 / actual status 404.
  $.Message expected: 'The requested resource was not found.'
  $.Message actual:   "The job aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa couldn't be found."

Cause 3 — fixture dataexchange-job-error-paths step task#10, CancelJob on an already-CANCELLED job. expected status 409 / actual status 409.
  $.Message expected: 'Cannot cancel the requested Job. To cancel a Job, it must be in the WAITING state.'
  $.Message actual:   "The job eba3e6946b7ea341e460c8319d8010bb is in state CANCELLED and can't be cancelled. Jobs can be cancelled only when they are in the WAITING state."
Fixed text = the whole expected sentence. The emulator's template's variable parts are the request's JobId and the stored State; the id and state quoted above are one run's values and are not part of the requirement.

Smithy citations: CancelJob DELETE /v1/jobs/{JobId} code 204, errors ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400. StartJob PATCH /v1/jobs/{JobId} code 202, errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400. ResourceNotFoundException/ConflictException each have Message (required), ResourceId and ResourceType.

## Steps to Reproduce
1. Boot the emulator and replay the simparity-wire-aws-v1 corpus; the three steps are fixture dataexchange-job-error-paths steps task#4, task#5 and task#10.
2. Cause 1: PATCH /v1/jobs/{JobId} (StartJob) with a syntactically valid (pattern ^[a-zA-Z0-9]{30,40}$) but never-created JobId. A 404 ResourceNotFoundException comes back; compare $.Message against the exact string 'The requested resource was not found.' — the emulator instead interpolates the JobId into its own sentence. Check that ResourceId/ResourceType carry the identity instead.
3. Cause 2: DELETE /v1/jobs/{JobId} (CancelJob) with the same never-created JobId and make the same comparison.
4. Cause 3: CreateDataSet -> CreateRevision -> CreateJob(Type=IMPORT_ASSET_FROM_SIGNED_URL) to get a WAITING job, then CancelJob once (it succeeds, 204, and GetJob reports CANCELLED). Call CancelJob again on the same JobId: a 409 ConflictException comes back; compare $.Message against the exact string 'Cannot cancel the requested Job. To cancel a Job, it must be in the WAITING state.' The emulator instead emits its own sentence with the JobId and the stored state interpolated.
5. Repeat cause 3 with a job in another non-WAITING state to confirm the emulator varies the sentence with stored state, where AWS's text is invariant.

## Resolution

All three causes are closed, and the one divergence the fixture's job plane has left is a
transport boundary rather than behaviour this crate states.

Causes 1 and 2 (the `ResourceNotFoundException` message on `StartJob` and `CancelJob`): the
crate's ONE not-found constructor (`crate::state`'s `resource_not_found`, reached from every
lookup site including both job-plane handlers) mints the fixed sentence "The requested resource
was not found." and reports the identity in `ResourceId`/`ResourceType`. Nothing is interpolated
into the text. Pinned by `tests/all/not_found_message.rs`, whose table includes the `StartJob`
and `CancelJob` rows over several well-formed-but-absent ids.

Cause 3 (the `ConflictException` message on a job whose `State` does not admit the call): the
crate's ONE job-state conflict constructor (`crate::state::job_state_conflict`) composes
"Cannot cancel the requested Job. To cancel a Job, it must be in the WAITING state." from the
operation's own verb alone — neither the job's id nor the state it is actually in reaches the
sentence — and the identity travels in `ResourceId`/`ResourceType`. Pinned by
`tests/all/job_state_conflict_message.rs`, whose table spans both operations, every
non-`WAITING` state a stored job reaches and every job `Type`.

The step the fixture now diverges on is the `CancelJob` SUCCESS, on `$headers.content-length`:
the recorded empty `204` states `content-length: 0` and the emulator stated no length at all.
The service states that framing at the one place every modelled response is serialized
(`Service::override_protocol` in `src/service.rs`, through the shared
`service_common::no_content_responses`), so it holds for every operation of this model whose
success is `204` over `Unit` — `CancelJob`, the deletes and both tag mutations — rather than
for the one that prompted it. That half landed on mainline from
`20261029T215303Z-dataexchange-canceljob-204-no-content-response-omits-the.md` while this
report was being worked, which is why this report's own change is the record of its three
message causes; this branch merged that work rather than carrying a second copy of the rule.
CAVEAT: a client reached over HTTP/1.1 still does not see the header, because the HTTP writer
the vendored transport serves with removes `content-length` from a `204`; that writer is under
the read-only vendored tree, so the wire-level half is a platform limitation rather than
something a service crate can state its way out of. What the service states is pinned by
`tests/all/no_content_framing.rs`, and what a socket then delivers by
`tests/all/no_content_wire_probe.rs`.
