Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 7bcb53c96ea0ea6c32faf644dec1ae2940515247

# ListSpeechSynthesisTasks truncates at an emulator-chosen 20-task page and returns a NextToken when the request names no MaxResults

_Filed: 2026-10-24T13:38:56Z_
_Operation: ListSpeechSynthesisTasks | Class: pagination_

> _Note: this issue was identified against the code base at commit `7bcb53c96ea0ea6c32faf644dec1ae2940515247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`MaxResults` (range 1..100) is the only page bound the model documents for this operation; the emulator additionally imposes its own default page size of 20 when the caller sends no `MaxResults`.

Expected: a request with no `MaxResults` returns the caller's synthesis tasks without an emulator-invented 20-item cut, i.e. no `NextToken` appears for a task count real Polly serves in a single response.

Actual: `SYNTHESIS_TASK_PAGE_SIZE = 20` is used whenever `MaxResults` is absent, so with 25 seeded tasks an unbounded ListSpeechSynthesisTasks answers 20 tasks plus a `NextToken`, and a client that does not paginate silently sees a truncated report. This is the same class of divergence already filed for ListLexicons (20-lexicon page), but on a different operation and a different code path, so it needs its own fix.

## Evidence

Spec: smithy_query(operation="ListSpeechSynthesisTasks") — input is `MaxResults` (integer, range 1..100), `NextToken`, `Status`; `@paginated(inputToken=NextToken, outputToken=NextToken)`. The model documents no default page size, and nothing in the operation's documentation licenses truncating a report the caller did not bound. Handler: services/service-com-amazonaws-polly-parrot-v1/src/ops/list_speech_synthesis_tasks.rs lines 113-119 define `SYNTHESIS_TASK_PAGE_SIZE: usize = 20` with a comment conceding "CHOSEN: ... this is the emulator's own", and lines 227-234 apply it for `input.max_results == None`; lines 274-283 then mint a `NextToken` off that chosen bound. Test coverage: tests/all/synthesis_tasks.rs::pagination_visits_every_task_exactly_once (lines ~198-272) seeds 25 tasks and only asserts that following tokens visits each task once for MaxResults in {1,2,7,24,100,None}; it never asserts how many tasks an unbounded request returns, so the chosen default is untested and uncovered.

## Steps to Reproduce

1. Boot the emulator and create/seed 25 synthesis tasks
2. Call GET /v1/synthesisTasks with no query parameters
3. Observe exactly 20 SynthesisTasks plus a NextToken in the response, i.e. the report is cut at an emulator-chosen bound the caller never asked for
4. Compare with real Polly, where MaxResults is the only page bound and an unbounded ListSpeechSynthesisTasks over 25 tasks returns them in one response with no NextToken
