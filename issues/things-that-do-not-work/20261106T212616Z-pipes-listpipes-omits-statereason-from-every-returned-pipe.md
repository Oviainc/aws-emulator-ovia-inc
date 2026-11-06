Issue-Source: trace-sweep-divergence
Issue-Trace: pipes-list-pipes-filtering
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pipes-list-pipes-filtering pipes-list-pipes-pagination pipes-sqs-to-sqs-lifecycle
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-a-9eb9c38c"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-b-781359bd"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-4bcd448c-0"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-4bcd448c-1"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-4bcd448c-2"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-pipes-s2s-e0efea35"].StateReason
Issue-Commit: f5561e7f84a7bf80a6aaebfba5e3ac3d454e1388

# pipes ListPipes omits StateReason from every returned Pipe summary

> _Note: this issue was identified against the code base at commit `f5561e7f84a7bf80a6aaebfba5e3ac3d454e1388`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipes

## Description
This bundle has 6 causes. All six are the SAME emulator defect observed on six distinct replay keys: the ListPipes summary projection never carries the Pipe.StateReason member, so every entry of the returned Pipes list comes back with StateReason absent while the HTTP status is the expected 200. The six causes are listed separately because each is its own replay cluster (different fixtures/steps and different pipe sets: filtered listing, the three pages of a paginated listing, the unpaginated full listing, and a post-lifecycle listing), and each must be re-replayed to confirm the fix.

NOTE ON OVERLAP: the open issue 20261030T071334Z-pipes-responses-that-describe-a-pipe-drop-modeled.md describes this same member omission as its "Cause 3 of 8", but its replay keys do not own these clusters (the replay report returns owned_by=[] for all six, because its ListPipes Issue-Key is truncated at the per-run ARN: 'pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn'). These six clusters are therefore unowned and filed here so they are actually re-replayed; a fixer should treat them together with that issue's Cause 3.

Shared fault for all six causes:
  AWS: ListPipes (Smithy: GET /v1/pipes, 200, output ListPipesResponse{Pipes: PipeList of Pipe, NextToken}) returns, for each element, the modeled member Pipe.StateReason -> PipeStateReason (string, @length min 0 max 512, pattern ^.*$), documented "The reason the pipe is in its current state." For a pipe whose state was reached by an explicit user request (create/start/stop), the recorded value is the fixed token USER_INITIATED; the token is a value AWS derives from the state transition, not a per-run filler.
  Emulator: every Pipes[] element is serialized with StateReason absent (None). Status, ordering, pagination and all other summary members match.
  Why: a client that lists pipes to triage them (find the ones not in their desired state, or show why a pipe stopped) gets no reason at all and must fall back to DescribePipe per pipe — which, per the issue named above, also omits StateReason. A list-then-diff reconciler sees StateReason flip from a value on AWS to null here and treats every pipe as changed.
  Fix: the ListPipes handler's Pipe summary projection (services/service-com-amazonaws-pipes-pipes/src/ops/list_pipes.rs) must copy StateReason from the stored pipe record, and the state record must retain the state-reason token set on each transition (src/state.rs).

Cause 1 of 6: filtered ListPipes (NamePrefix-style filtered listing) returns a single pipe with StateReason absent (expected USER_INITIATED).
  AWS/Emulator/Why/Fix: as the shared fault above.
  Traces: pipes-list-pipes-filtering#task#15.

Cause 2 of 6: a second filtered ListPipes over a different pipe returns that entry with StateReason absent.
  AWS/Emulator/Why/Fix: as the shared fault above.
  Traces: pipes-list-pipes-filtering#task#16.

Cause 3 of 6: the first page of a paginated ListPipes (Limit set) returns both page entries with StateReason absent.
  AWS/Emulator/Why/Fix: as the shared fault above; the omission is in the per-element projection, so it affects every page.
  Traces: pipes-list-pipes-pagination#task#13.

Cause 4 of 6: the second/last page of the same paginated ListPipes returns its entry with StateReason absent.
  AWS/Emulator/Why/Fix: as the shared fault above.
  Traces: pipes-list-pipes-pagination#task#14.

Cause 5 of 6: the unpaginated ListPipes over the same three pipes returns all three entries with StateReason absent.
  AWS/Emulator/Why/Fix: as the shared fault above.
  Traces: pipes-list-pipes-pagination#task#15.

Cause 6 of 6: ListPipes after a create/start/stop lifecycle on an SQS-to-SQS pipe returns that pipe with StateReason absent, so the reason for the state the lifecycle calls produced is unreadable.
  AWS/Emulator/Why/Fix: as the shared fault above; the state-reason token must survive lifecycle transitions.
  Traces: pipes-sqs-to-sqs-lifecycle#task#11.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, 237/280 steps passed). Six mismatch clusters, all with expected_status == actual_status == 200 — body-only divergence, one JSONPath key per listed pipe:
- ListPipes, rep pipes-list-pipes-filtering#task#15 (affected 1): "$.Pipes[Arn=\"arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-a-9eb9c38c\"].StateReason: missing (expected 'USER_INITIATED', got None)".
- ListPipes, rep pipes-list-pipes-filtering#task#16 (affected 1): same detail for .../pipe/sim2real-probe-pipes-list-b-781359bd.
- ListPipes, rep pipes-list-pipes-pagination#task#13 (affected 1): same detail for both .../sim2real-probe-pipes-listp-4bcd448c-0 and ...-1.
- ListPipes, rep pipes-list-pipes-pagination#task#14 (affected 1): same detail for .../sim2real-probe-pipes-listp-4bcd448c-2.
- ListPipes, rep pipes-list-pipes-pagination#task#15 (affected 1): same detail for ...-0, ...-1 and ...-2 together.
- ListPipes, rep pipes-sqs-to-sqs-lifecycle#task#11 (affected 1): same detail for .../pipe/sim2real-pipes-s2s-e0efea35.
None of the six clusters carries a stub_path, and all six report owned_by=[] (no open issue carries their replay key).
The pipe names, ARNs and account id above are per-run fillers. The requirement is that each element of ListPipes' Pipes list carries the modeled StateReason member populated from the pipe's state record — the state-reason token AWS reports for how the pipe reached its current state (USER_INITIATED for a user-driven create/start/stop) — not that any particular recorded ARN appears.
Smithy cited: ListPipes GET /v1/pipes 200, output ListPipesResponse.Pipes: PipeList of Pipe; Pipe.StateReason -> PipeStateReason (string, @length 0..512, pattern ^.*$), doc "The reason the pipe is in its current state."

## Steps to Reproduce
1. Replay fixture pipes-list-pipes-filtering, step task#15: create pipes with an SQS source/target and call ListPipes with a name filter. Response is 200 but the returned Pipes[] entry has no StateReason member (AWS: StateReason = USER_INITIATED).
2. Replay pipes-list-pipes-filtering#task#16 — the second filtered listing: same omission.
3. Replay pipes-list-pipes-pagination#task#13 and #task#14 — paged ListPipes (Limit set): every entry on every page omits StateReason.
4. Replay pipes-list-pipes-pagination#task#15 — unpaginated ListPipes over the same three pipes: all three entries omit StateReason.
5. Replay pipes-sqs-to-sqs-lifecycle#task#11 — ListPipes after create/start/stop: the pipe's StateReason is still absent.
6. Compare against the recordings: each entry must carry StateReason with the state-reason token for the transition that produced the current state.
