Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.list-speech-synthesis-tasks-ordered-by-creation-time-descending
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly.list-speech-synthesis-tasks-ordered-by-creation-time-descending|run#10|AssertionError|assert pos_b < pos_a, (
Issue-Commit: 560e7694040bdb3912f85c22ba90afbac643f510

# polly: ListSpeechSynthesisTasks returns tasks in ascending CreationTime order instead of most-recent-first

> _Note: this issue was identified against the code base at commit `560e7694040bdb3912f85c22ba90afbac643f510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSpeechSynthesisTasks

## Description
Cause 1 of 1: the unfiltered ListSpeechSynthesisTasks listing is not sorted by CreationTime descending.

AWS: recorded behavior only — Smithy (polly ListSpeechSynthesisTasks, GET /v1/synthesisTasks, output SynthesisTasks + NextToken, paginated inputToken/outputToken NextToken, pageSize MaxResults) states no ordering trait. The recording shows Polly returning the Region's synthesis tasks newest-first: a task whose SynthesisTask.CreationTime is strictly later than another's appears at a lower index in the concatenation of all SynthesisTasks pages, and that order agrees with the CreationTime each task itself reports via GetSpeechSynthesisTask. Pagination walks that same descending sequence.

Emulator: with two tasks created a second apart and both driven to TaskStatus=completed, the concatenated listing puts the OLDER task first. The check recorded positions a=0 (earlier CreationTime) and b=1 (later CreationTime) in the returned id sequence, i.e. ascending CreationTime — the exact reverse of Polly's order. The CreationTime values the tasks report are themselves correctly ordered (b > a), so this is an ordering defect in the listing, not a timestamp defect.

Why: a client that reads only the first page (or the first element) to find its most recent task gets the oldest one instead, and any consumer that treats the listing as a newest-first feed — dashboards, "latest task" lookups, incremental pagination that stops once it passes a known timestamp — silently processes stale tasks and can never see tasks created after the ones it already consumed.

Fix: the ListSpeechSynthesisTasks handler's listing/ordering step over the synthesis-task state — sort the task set by CreationTime descending before paginating, and derive the page cursor from that descending sequence so NextToken continues it.

Traces: polly.list-speech-synthesis-tasks-ordered-by-creation-time-descending.

## Evidence
Expected: after starting task A, waiting until the clock is past A.CreationTime + 1s, then starting task B (both reaching TaskStatus=completed), the ids collected across every ListSpeechSynthesisTasks page place B before A, consistent with B.CreationTime > A.CreationTime.

Actual: AssertionError: expected the more-recently-created task to list first (descending CreationTime), got positions a=0 b=1 in ['<task-a-id>', '<task-b-id>'] — the listing is ascending by CreationTime. (The two ids are per-run values; the requirement is the relative order, not the ids.)

The companion assertion that B.CreationTime > A.CreationTime was not reached, and the per-task CreationTime values are ordered correctly, so only the listing's sort order diverges.

## Steps to Reproduce
1. Create an S3 bucket for task output.
2. polly.start_speech_synthesis_task(Engine="standard", OutputFormat="mp3", OutputS3BucketName=<bucket>, Text="First task started for ordering check.", VoiceId="Joanna") and poll get_speech_synthesis_task until TaskStatus == completed; record its CreationTime (call it A).
3. Wait until the wall clock is past A.CreationTime + 1 second.
4. Start and complete a second task the same way with different Text (call it B), so B.CreationTime > A.CreationTime.
5. Walk every page of polly.list_speech_synthesis_tasks() (following NextToken) and collect TaskId values in order.
6. Observe A's id at a lower index than B's id; AWS lists B first.
