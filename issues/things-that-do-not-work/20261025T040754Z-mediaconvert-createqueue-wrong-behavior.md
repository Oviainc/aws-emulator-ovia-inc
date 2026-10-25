Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# CreateQueue trims the queue Name it stores, while the sibling creates store the resource key byte for byte

_Filed: 2026-10-25T04:07:54Z_
_Operation: CreateQueue | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateQueue` normalises the resource KEY: it stores `input.name.trim()` rather than the name the caller sent. The other two named creates in this crate deliberately do NOT normalise the key, and both say so in a comment: the trim there is only the test for a name carrying no characters, and the name itself is stored and echoed verbatim.

Because the stored key is the trimmed form while every other queue-addressing path reads the name exactly as sent (`GetQueue`/`UpdateQueue`/`DeleteQueue` use `input.name` unchanged, `CreateJob`/`ListJobs` resolve `Queue` through `queue_name_from_reference`, which only strips an ARN prefix), a caller who creates a queue with surrounding whitespace is handed a resource they cannot address with the string they used:
- `CreateQueue {"name": "q "}` answers 201 with `Queue.Name = "q"` and `Queue.Arn = ...:queues/q` — neither echoes the request;
- `GetQueue("q ")`, `UpdateQueue("q ", ...)` and `DeleteQueue("q ")` all answer the modelled 404;
- `CreateJob(Queue="q ")` answers NotFoundException for a queue the same account just created.

The divergence is also visible the other way round: a padded preset/job-template name is a distinct resource stored verbatim (pinned by a test), so the three creates of this one service disagree about whether a resource key is normalised.

## Evidence

This handler normalises the key — services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_queue.rs:181-187:
```
let name = input.name.trim().to_string();
if name.is_empty() { ... BadRequestException { message: Some("Name must not be empty.") } ... }
```
(the trimmed `name` is what is moved into `QueueRecord { name, ... }` at create_queue.rs:218-219 and inserted as the store key by `create_named`, src/state.rs:3058).

Sibling handlers refuse to normalise it, and state the rule — src/ops/create_job_template.rs:185-190:
```
// The name is the resource's KEY and is stored exactly as the caller sent it:
// nothing in the model or in AWS's documentation normalises it, so the template a
// later Get/Update/Delete addresses is the one whose name matches byte for byte.
let name = input.name.clone();
if name.is_empty() { ... }
```
and src/ops/create_preset.rs:162-167:
```
// The `Name` is the resource's identity, so it is stored and echoed back exactly
// as the caller sent it — never normalised. The trim is only the test for a name
// that carries no characters at all ...
let name = input.name;
if name.trim().is_empty() { ... }
```

The read paths that cannot see the trimmed key: src/ops/get_queue.rs:165-168 (`let name = input.name; ... state.queues.get(&name)`), src/ops/update_queue.rs:184-190, src/ops/delete_queue.rs:160-163, and src/state.rs:708-744 `queue_name_from_reference` → `resource_name_from_reference`, which returns the reference unchanged unless it is an `arn:...:queues/<name>`.

Test coverage: tests/all/preset_lifecycle.rs:408-429 pins the non-normalising rule for presets ("The name is the resource's identity and is never normalised, so a name that only differs by surrounding whitespace is a DIFFERENT preset ... echoed back exactly as sent"); no test in tests/all/queue_*.rs exercises a padded queue name, so nothing pins the trim.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_queue(Name='pad ') — observe HTTP 201 with Queue.Name == 'pad' and Queue.Arn ending in ':queues/pad' (neither is the name that was sent).
3. get_queue(Name='pad ') — observe NotFoundException (404) for the queue the previous call reported as created.
4. create_job(Queue='pad ', Role=..., Settings=...) — observe NotFoundException naming `queue`.
5. Positive control: create_preset(Name='pad ') then get_preset(Name='pad ') succeeds, because the preset path stores the key verbatim.
