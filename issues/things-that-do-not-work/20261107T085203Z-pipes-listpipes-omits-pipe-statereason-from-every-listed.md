Issue-Source: trace-sweep-divergence
Issue-Trace: pipes-list-pipes-filtering
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pipes-list-pipes-filtering pipes-list-pipes-pagination pipes-sqs-to-sqs-lifecycle
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-a-102d0c16"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-b-c000db61"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-df2dd815-0"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-df2dd815-1"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-df2dd815-2"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-pipes-s2s-036df3f8"].StateReason
Issue-Commit: 2d393b0b08d4e0faded23ebb784005e509785021

# pipes ListPipes omits Pipe.StateReason from every listed pipe summary

> _Note: this issue was identified against the code base at commit `2d393b0b08d4e0faded23ebb784005e509785021`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipes

## Description
This bundle has 6 causes. Every one is a ListPipes 200 response whose body is structurally correct except that each element of $.Pipes is serialized WITHOUT the modeled member StateReason, so the member comes back absent (None) for every pipe in every listing shape exercised by the corpus: name-prefix filtering, paginated listings (first page, later page, and the whole-set listing), and a listing taken mid-lifecycle. A fixer must close all 6 before submitting.

Shared fault: the ListPipes summary projection does not carry StateReason from the stored pipe record onto the Pipe summary it emits. The six causes are the six distinct replay clusters that fault reproduces in; they differ only in which listing call and which pipes are in scope, which is what makes it clear the omission is unconditional rather than specific to one query shape.

NOTE on an adjacent open issue: 20261030T071334Z-pipes-responses-that-describe-a-pipe-drop-modeled.md discusses ListPipes StateReason in prose, but its replay key for ListPipes is recorded truncated (Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn) and the replay reports all six of these clusters as UNOWNED, so none of them is gated by that issue. This issue carries their replay keys.

Cause 1 of 6: ListPipes with a NamePrefix filter omits StateReason on the matched pipe.
  AWS: ListPipes (Smithy: GET /v1/pipes, 200, output ListPipesResponse.Pipes: PipeList of Pipe) returns Pipe.StateReason (target PipeStateReason, string @length min 0 max 512, pattern ^.*$, doc "The reason the pipe is in its current state."). For a pipe whose state was reached by an explicit user request (create/start/stop), the recorded StateReason is the fixed token "USER_INITIATED" — a fixed state-reason token, not a per-run value; only the surrounding pipe Name/Arn are per-run.
  Emulator: the Pipe summary in $.Pipes is emitted with StateReason absent (None).
  Why: a client that lists pipes to triage which ones are not in their desired state gets no reason at all, so it cannot distinguish a user-initiated stop from a service-initiated failure without a per-pipe DescribePipe (which, per the adjacent issue, also omits it). Strict SDK consumers that expect the member to be populated for non-trivial states see an empty optional and report "no reason available".
  Fix: the ListPipes handler's Pipe summary projection (services/service-com-amazonaws-pipes-pipes/src/ops/list_pipes.rs) must copy StateReason from the stored pipe record, alongside the state members it already projects.
  Traces: pipes-list-pipes-filtering#task#15.

Cause 2 of 6: the second filtered ListPipes call in the same fixture omits StateReason on its matched pipe.
  AWS: as cause 1 — StateReason is populated for every entry returned, independent of which NamePrefix matched.
  Emulator: StateReason absent again, confirming the omission is not specific to one pipe record.
  Why: as cause 1; a filtered listing is the normal way a client narrows to one pipe, and it is exactly as uninformative as the unfiltered one.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-filtering#task#16.

Cause 3 of 6: the first page of a paginated ListPipes omits StateReason on both entries of the page.
  AWS: as cause 1 — every Pipe on a page carries StateReason; pagination does not reduce the projected member set.
  Emulator: both page entries come back with StateReason absent.
  Why: a paging client accumulating pipe summaries ends up with a state-reason-free inventory no matter how many pages it walks.
  Fix: same component as cause 1 (the projection is shared by all pages).
  Traces: pipes-list-pipes-pagination#task#13.

Cause 4 of 6: the follow-up page of the same paginated ListPipes omits StateReason on its entry.
  AWS: as cause 1.
  Emulator: StateReason absent on the final-page entry.
  Why: as cause 3.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-pagination#task#14.

Cause 5 of 6: an unpaginated ListPipes over the whole set omits StateReason on all three entries.
  AWS: as cause 1.
  Emulator: all three entries have StateReason absent.
  Why: as cause 1; the omission scales with the listing, so every pipe in an account-wide listing is affected.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-pagination#task#15.

Cause 6 of 6: ListPipes taken after a start/stop lifecycle sequence omits StateReason on the pipe whose state just changed.
  AWS: as cause 1 — this is precisely the case where StateReason is informative, since the recorded reason for a user-driven transition is "USER_INITIATED".
  Emulator: StateReason absent on the just-transitioned pipe.
  Why: a client polling ListPipes to observe a start/stop converge cannot see why the pipe is in its current state, so it cannot tell a requested stop from a failure-induced one and may retry or alarm incorrectly.
  Fix: same component as cause 1; the state record written by the start/stop handlers must also expose the reason the projection reads.
  Traces: pipes-sqs-to-sqs-lifecycle#task#11.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 6 unowned mismatch clusters, all ListPipes, all expected_status == actual_status == 200 (body-only divergence), stub_path empty and owned_by empty for each.
- pipes-list-pipes-filtering#task#15: "$.Pipes[Arn=\"arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-a-102d0c16\"].StateReason: missing (expected 'USER_INITIATED', got None)".
- pipes-list-pipes-filtering#task#16: same detail for the ...pipes-list-b-c000db61 pipe.
- pipes-list-pipes-pagination#task#13: the same "missing (expected 'USER_INITIATED', got None)" for two entries (...listp-df2dd815-0 and -1).
- pipes-list-pipes-pagination#task#14: same for ...listp-df2dd815-2.
- pipes-list-pipes-pagination#task#15: same for all three ...listp-df2dd815-0/-1/-2 entries.
- pipes-sqs-to-sqs-lifecycle#task#11: same for ...pipes-s2s-036df3f8.
Model evidence: Smithy com.amazonaws.pipes#Pipe member StateReason -> PipeStateReason (string, @length 0..512, pattern ^.*$), doc "The reason the pipe is in its current state."; ListPipes GET /v1/pipes, 200, output member Pipes -> PipeList of Pipe.
The ARNs, pipe names and account/region above are per-run fillers and are not requirements. The requirement is that every Pipe summary ListPipes returns carries StateReason populated from the pipe's stored state reason, which for a state reached by an explicit user request is the fixed token "USER_INITIATED".

## Steps to Reproduce
1. Replay fixture pipes-list-pipes-filtering, step task#15: create two pipes, then call ListPipes with a NamePrefix matching one of them. Response is 200 and the matched $.Pipes[0] has no StateReason member (AWS returns StateReason "USER_INITIATED").
2. Replay pipes-list-pipes-filtering#task#16 — the second filtered ListPipes: same omission on the other pipe.
3. Replay pipes-list-pipes-pagination#task#13 and #task#14 — paginated ListPipes (Limit set, then the returned NextToken): StateReason is absent on every entry of both pages.
4. Replay pipes-list-pipes-pagination#task#15 — unpaginated ListPipes over all three pipes: StateReason absent on all three entries.
5. Replay pipes-sqs-to-sqs-lifecycle#task#11 — ListPipes after StartPipe/StopPipe on the pipe: StateReason absent on the just-transitioned pipe.
In each case compare the emulator's $.Pipes[*] against the recorded response, which carries StateReason on every entry.
