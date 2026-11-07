Issue-Source: trace-sweep-divergence
Issue-Trace: pipes-list-pipes-filtering
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pipes-list-pipes-filtering pipes-list-pipes-pagination pipes-sqs-to-sqs-lifecycle
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-a-ca008e1f"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-b-2d8bc2b2"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-ba2897a7-0"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-ba2897a7-1"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-ba2897a7-2"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-pipes-s2s-23340ab1"].StateReason
Issue-Commit: f788794678f5d1e01179c3155755ea143e14c8e8

# pipes ListPipes Pipe-summary projection drops StateReason across filtered, paginated and post-lifecycle listings

> _Note: this issue was identified against the code base at commit `f788794678f5d1e01179c3155755ea143e14c8e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipes

## Description
This bundle has 6 causes and a fixer must close all 6 before submitting. Every one is a ListPipes 200 whose body is structurally right except that each element of $.Pipes is serialized WITHOUT the modeled member StateReason, so it comes back absent (None) for every pipe in every listing shape the corpus exercises: name-prefix filtering, paginated listings (first page, follow-up page, whole-set listing), and a listing taken right after a start/stop lifecycle.

Shared fault: the ListPipes summary projection does not carry the stored pipe record's state reason onto the Pipe summary it emits. The six causes are the six distinct replay clusters the fault reproduces in; they differ only in which listing call and which pipes are in scope, which is what shows the omission is unconditional rather than tied to one query shape.

NOTE on adjacent open issues: 20261107T085203Z-pipes-listpipes-omits-pipe-statereason-from-every-listed.md and 20261106T212616Z-pipes-listpipes-omits-statereason-from-every-returned-pipe.md describe this same omission, and 20261030T071334Z-pipes-responses-that-describe-a-pipe-drop-modeled.md mentions it in prose, but their recorded replay keys embed per-run pipe ARNs/names from an earlier run, so none of them joins this run's clusters: the replay reports all six clusters below with owned_by EMPTY and stub_path empty, i.e. ungated. This issue carries their replay keys for this corpus.

Cause 1 of 6: ListPipes with a NamePrefix filter omits StateReason on the matched pipe. AWS: Smithy com.amazonaws.pipes#ListPipes (GET /v1/pipes, 200, output member Pipes -> PipeList of com.amazonaws.pipes#Pipe) returns Pipe.StateReason -> PipeStateReason (string, @length min 0 max 512, pattern ^.*$), doc "The reason the pipe is in its current state."; for a state reached by an explicit user request (create/start/stop) the recorded reason is the fixed token "USER_INITIATED" — that token is the fixed text of the value, while the surrounding Pipe Name/Arn are per-run fillers. Emulator: the Pipe summary is emitted with StateReason absent (None), status 200 otherwise matching. Why: a client that lists pipes to triage which are not in their desired state gets no reason at all, so it cannot tell a user-initiated stop from a service-initiated failure without a per-pipe DescribePipe (which, per the adjacent issue, also omits it); strict SDK consumers read the empty optional as "no reason available". Fix: the ListPipes handler's Pipe summary projection (services/service-com-amazonaws-pipes-pipes/src/ops/list_pipes.rs) must copy the state reason from the stored pipe record alongside the state members it already projects. Traces: pipes-list-pipes-filtering#task#15.

Cause 2 of 6: the second filtered ListPipes call in the same fixture omits StateReason on the other matched pipe. AWS: as cause 1 — StateReason is populated for every returned entry, independent of which NamePrefix matched. Emulator: StateReason absent again, confirming the omission is not specific to one pipe record. Why: as cause 1; a filtered listing is the normal way a client narrows to one pipe and is just as uninformative as an unfiltered one. Fix: same component as cause 1. Traces: pipes-list-pipes-filtering#task#16.

Cause 3 of 6: the first page of a paginated ListPipes omits StateReason on both entries of the page. AWS: as cause 1 — every Pipe on a page carries StateReason; pagination does not reduce the projected member set. Emulator: both page entries come back with StateReason absent. Why: a paging client accumulating summaries ends up with a state-reason-free inventory however many pages it walks. Fix: same component as cause 1 (the projection is shared by all pages). Traces: pipes-list-pipes-pagination#task#13.

Cause 4 of 6: the follow-up page of the same paginated ListPipes omits StateReason on its entry. AWS: as cause 1. Emulator: StateReason absent on the final-page entry. Why: as cause 3. Fix: same component as cause 1. Traces: pipes-list-pipes-pagination#task#14.

Cause 5 of 6: an unpaginated ListPipes over the whole set omits StateReason on all three entries. AWS: as cause 1. Emulator: all three entries have StateReason absent. Why: as cause 1; the omission scales with the listing, so every pipe in an account-wide listing is affected. Fix: same component as cause 1. Traces: pipes-list-pipes-pagination#task#15.

Cause 6 of 6: ListPipes taken after a start/stop lifecycle sequence omits StateReason on the pipe whose state just changed. AWS: as cause 1 — this is exactly the case where the member is informative, since the recorded reason for a user-driven transition is "USER_INITIATED". Emulator: StateReason absent on the just-transitioned pipe. Why: a client polling ListPipes to watch a start/stop converge cannot see why the pipe is in its current state, so it cannot distinguish a requested stop from a failure-induced one and may retry or alarm wrongly. Fix: same component as cause 1; the state written by the start/stop handlers must expose the reason the projection reads. Traces: pipes-sqs-to-sqs-lifecycle#task#11.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 280 steps, 237 passed, 43 mismatched): 6 unowned mismatch clusters, all ListPipes, all expected_status == actual_status == 200 (body-only divergence), stub_path empty and owned_by empty for each.
- pipes-list-pipes-filtering#task#15: "$.Pipes[Arn=\"arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-a-ca008e1f\"].StateReason: missing (expected 'USER_INITIATED', got None)".
- pipes-list-pipes-filtering#task#16: same detail for the ...pipes-list-b-2d8bc2b2 pipe.
- pipes-list-pipes-pagination#task#13: same "missing (expected 'USER_INITIATED', got None)" for two entries (...listp-ba2897a7-0 and -1).
- pipes-list-pipes-pagination#task#14: same for ...listp-ba2897a7-2.
- pipes-list-pipes-pagination#task#15: same for all three ...listp-ba2897a7-0/-1/-2 entries.
- pipes-sqs-to-sqs-lifecycle#task#11: same for ...pipes-s2s-23340ab1.
Model evidence: Smithy com.amazonaws.pipes#Pipe member StateReason -> PipeStateReason (string, @length 0..512, pattern ^.*$), doc "The reason the pipe is in its current state."; ListPipes GET /v1/pipes, 200, output member Pipes -> PipeList of Pipe.
The ARNs, pipe names, account id and region above are per-run fillers and are not requirements. The requirement is that every Pipe summary ListPipes returns carries StateReason populated from the pipe's stored state reason, which for a state reached by an explicit user request is the fixed token "USER_INITIATED".

## Steps to Reproduce
1. Build the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Fixture pipes-list-pipes-filtering, step task#15: create two pipes, then ListPipes with a NamePrefix matching one. Response is 200 and the matched $.Pipes[0] has no StateReason member; the recording carries StateReason "USER_INITIATED".
3. pipes-list-pipes-filtering#task#16 — the second filtered ListPipes: same omission on the other pipe.
4. pipes-list-pipes-pagination#task#13 and #task#14 — paginated ListPipes (Limit set, then the returned NextToken): StateReason absent on every entry of both pages.
5. pipes-list-pipes-pagination#task#15 — unpaginated ListPipes over all three pipes: StateReason absent on all three entries.
6. pipes-sqs-to-sqs-lifecycle#task#11 — ListPipes after StartPipe/StopPipe: StateReason absent on the just-transitioned pipe.
In each case compare the emulator's $.Pipes[*] against the recorded response, which carries StateReason on every entry.
