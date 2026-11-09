Issue-Source: trace-sweep-divergence
Issue-Trace: pipes-list-pipes-filtering
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pipes-list-pipes-filtering pipes-list-pipes-pagination pipes-sqs-to-sqs-lifecycle
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-a-1c0db346"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-b-959c64e8"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-76cc9b5a-0"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-76cc9b5a-1"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-76cc9b5a-2"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-pipes-s2s-28d10c1c"].StateReason
Issue-Commit: 8bb4a17e1773d9e193916d4dc5d538985b40383b

# pipes lifecycle writes never set Pipe StateReason so every ListPipes entry omits it

> _Note: this issue was identified against the code base at commit `8bb4a17e1773d9e193916d4dc5d538985b40383b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipes

## Description
This bundle has 6 causes. A fixer MUST close all six before submitting: they share one root cause but replay as six independent clusters (different pipes, different query shapes, different pages, different fixtures), so each must be re-replayed.

Shared fault: the stored pipe record's state-reason is written in exactly ONE place -- PipeRecord::settle, which fires only when a DELAYED state transition becomes due (services/service-com-amazonaws-pipes-pipes/src/state.rs:947-955 sets self.state_reason = settled.reason). A pipe that reaches its resting state because the CALLER asked for it (CreatePipe with a DesiredState, StartPipe, StopPipe) leaves state_reason as None. PipeRecord::summary() (src/state.rs:988-1001) does copy state_reason through faithfully, so the projection is innocent: ListPipes serialises an absent member because nothing ever wrote the field. The defect belongs in the state writes of the create/start/stop handlers, not in the ListPipes serializer.

Related open issues that look similar but do NOT own these replay keys, so these clusters replay as UNOWNED and are re-filed here with intact bindings: 20261030T071334Z-pipes-responses-that-describe-a-pipe-drop-modeled.md (its ListPipes/StateReason cause carries a TRUNCATED Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn), 20261106T212616Z-pipes-listpipes-omits-statereason-from-every-returned-pipe.md, 20261107T085203Z-pipes-listpipes-omits-pipe-statereason-from-every-listed.md, 20261107T102546Z-pipes-listpipes-pipe-summary-projection-drops-statereason-across.md and 20261108T161708Z-pipes-listpipes-omits-statereason-on-every-listed-pipe.md. Each of those recorded its replay key with the per-run pipe ARN embedded in the JSON path, and the corpus mints a fresh ARN suffix every run, so the key never re-matches and the ownership link is lost. Whoever fixes this should ALSO re-record those keys without the per-run ARN, otherwise this cause will be re-filed again next sweep.

Cause 1 of 6: ListPipes with a NamePrefix filter returns the matching pipe with no StateReason.
  AWS: Smithy ListPipes -> 200, output ListPipesResponse.Pipes: PipeList of com.amazonaws.pipes#Pipe. Pipe.StateReason targets PipeStateReason, a string with @length(min 0, max 512) and @pattern ^.*$, documented "The reason the pipe is in its current state." The model does not enumerate the reason tokens; recorded behavior only for the value: for a pipe resting in the state its caller asked for, AWS reports the fixed token USER_INITIATED. That token is fixed text describing HOW the pipe reached its state -- it is not derived from the request and is not a per-run value.
  Emulator: the listed Pipe entry has StateReason absent (None, omitted from the JSON body). Status is correct (200) and every other member matches.
  Why: a client that lists pipes to triage which are unhealthy, or why one is stopped, gets no reason on any entry and cannot tell a user-stopped pipe from a failed one. Falling back to DescribePipe does not help: it omits the member too.
  Fix: the state writes of the create/lifecycle handlers (CreatePipe, StartPipe, StopPipe) must set the pipe record's state-reason to the user-initiated reason token when the caller drives the transition, in the pipe record's state rather than only on a settled delayed transition.
  Traces: pipes-list-pipes-filtering#task#15.

Cause 2 of 6: the same filtering fixture's SECOND filtered listing returns its pipe with no StateReason.
  AWS: as cause 1 -- every Pipe entry carries StateReason regardless of which pipe it is.
  Emulator: StateReason absent on that entry as well.
  Why: as cause 1; the omission is not specific to one pipe, so no entry in any listing is usable for state triage.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-filtering#task#16.

Cause 3 of 6: the FIRST page of a paginated ListPipes returns its entries with no StateReason.
  AWS: as cause 1 -- a paged listing's entries are the same Pipe shape with StateReason populated.
  Emulator: every entry on the page is missing StateReason.
  Why: a paging client accumulating pipe state across pages ends with a complete inventory in which no pipe has a reason.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-pagination#task#13.

Cause 4 of 6: the CONTINUATION page of the same paginated ListPipes returns its entry with no StateReason.
  AWS: as cause 1 -- resuming with a NextToken does not change which members an entry carries.
  Emulator: StateReason absent on the continuation page's entry.
  Why: as cause 3; the gap is uniform over pages, so it cannot be worked around by re-reading a page.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-pagination#task#14.

Cause 5 of 6: an UNPAGED ListPipes over the same pipes returns all entries with no StateReason.
  AWS: as cause 1.
  Emulator: StateReason absent on each listed pipe.
  Why: as cause 1; this confirms the omission lives in the stored record, not in the paging path.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-pagination#task#15.

Cause 6 of 6: ListPipes after a StartPipe/StopPipe run on an SQS-to-SQS pipe returns the pipe with no StateReason.
  AWS: as cause 1 -- after a caller-driven StartPipe/StopPipe the pipe's StateReason is the user-initiated reason token, which is precisely the case AWS populates.
  Emulator: StateReason absent, so the one transition that most clearly should have written a reason wrote none.
  Why: a client polling a start/stop by listing pipes never learns that the state it sees was caller-requested, and a lifecycle failure would be indistinguishable from a successful stop.
  Fix: same component as cause 1 -- specifically the StartPipe and StopPipe state writes alongside the pipe record's state-reason field.
  Traces: pipes-sqs-to-sqs-lifecycle#task#11.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 280 steps, 237 passed, 43 mismatched, 0 errored, 0 blocked, score 84.6%, emulator_up true, bootstrap_signal false. 6 of the 20 mismatch clusters are ListPipes clusters with an EMPTY owned_by and an EMPTY stub_path; all six are expected_status == actual_status == 200 and diverge in the body only.

- rep pipes-list-pipes-filtering#task#15 (affected_count 1): '$.Pipes[Arn="arn:aws:pipes:<region>:<account>:pipe/<name>"].StateReason: missing (expected \'USER_INITIATED\', got None)'.
- rep pipes-list-pipes-filtering#task#16 (affected_count 1): same detail for the fixture's second pipe.
- rep pipes-list-pipes-pagination#task#13 (affected_count 1): the same missing-StateReason detail for two entries on one page.
- rep pipes-list-pipes-pagination#task#14 (affected_count 1): same, continuation page, one entry.
- rep pipes-list-pipes-pagination#task#15 (affected_count 1): same, three entries in one unpaged listing.
- rep pipes-sqs-to-sqs-lifecycle#task#11 (affected_count 1): same, after StartPipe/StopPipe.

Model evidence (services/service-com-amazonaws-pipes-pipes/smithy-model/com-amazonaws-pipes-pipes.json): ListPipesResponse.Pipes is a PipeList of com.amazonaws.pipes#Pipe; Pipe.StateReason targets PipeStateReason (@length min 0 max 512, @pattern ^.*$), doc "The reason the pipe is in its current state."

Code evidence: services/service-com-amazonaws-pipes-pipes/src/state.rs:988-1001 (PipeRecord::summary) copies self.state_reason through unchanged; src/state.rs:947-955 (PipeRecord::settle) is the ONLY write of state_reason and fires only for a delayed transition that became due, so a caller-driven create/start/stop leaves it None.

The ARNs, pipe names and per-run suffixes in the recorded details are per-run fillers and are NOT part of the requirement. The requirement is that Pipe.StateReason is PRESENT on every listed pipe and carries the state-reason token AWS reports for how that pipe reached its current state -- the fixed token USER_INITIATED for a caller-driven transition.

## Steps to Reproduce
1. Clone the repo, build the emulator from the checkout, and replay the simparity-wire-aws-v1 wire corpus.
2. Replay fixture pipes-list-pipes-filtering, step task#15: create two pipes with an SQS source, then call ListPipes with a NamePrefix filter. The response is 200 and matches on every other member, but the matching Pipes[] entry has no StateReason (AWS: USER_INITIATED).
3. Replay pipes-list-pipes-filtering#task#16 (the fixture's second filtered listing): the same omission on the other pipe.
4. Replay pipes-list-pipes-pagination#task#13 and #task#14 (ListPipes with Limit set, then the returned NextToken): every entry on both the first and the continuation page is missing StateReason.
5. Replay pipes-list-pipes-pagination#task#15 (the unpaged listing of the same three pipes): all three entries are missing StateReason.
6. Replay pipes-sqs-to-sqs-lifecycle#task#11 (create an SQS-to-SQS pipe, run StartPipe/StopPipe, then ListPipes): the pipe is listed with no StateReason even though the state it rests in was caller-requested.
