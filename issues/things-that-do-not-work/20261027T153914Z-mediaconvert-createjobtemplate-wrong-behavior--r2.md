Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0d4ec79976a0b91eeb5a23059b2beecdecae287a

# CreateJobTemplate/UpdateJobTemplate trim the Queue reference before resolving it, unlike every other queue reference in the crate

_Filed: 2026-10-27T15:39:14Z_
_Operation: CreateJobTemplate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0d4ec79976a0b91eeb5a23059b2beecdecae287a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateJobTemplate` and `UpdateJobTemplate` are the only two paths in this crate that normalise a queue REFERENCE before resolving it: both call `queue_name_from_reference(reference.trim())`. Every other queue reference in the crate resolves the caller's bytes verbatim — `CreateJob$Queue`, `HopDestination$Queue` on all three operations that accept a hop list, and `ListJobs`/`SearchJobs`' `queue` filter — because a queue's `Name` is its key and is stored byte for byte (which is why `CreateQueue` was fixed to stop trimming its own `Name`).

The result is that the same string resolves to two different things depending on which door it arrives at, in both directions:

* a template cannot reference a queue whose stored name carries surrounding whitespace — the reference is trimmed, the trimmed spelling is not a stored key, and the request is answered `NotFoundException` even though the queue exists and `CreateJob` accepts the identical reference;
* a template CAN be created against a reference whose trimmed form happens to name a stored queue, and the stored reference is then the trimmed string rather than what the caller sent — while `CreateJob` with that same reference answers `NotFoundException`.

So a job template's `Queue` is the one queue reference that is silently rewritten, and the queue a job created from that template is sent to is not the queue the template's request named.

## Evidence

The two handlers that trim:

`services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job_template.rs:217-219`
```rust
            Some(reference) => {
                Some(crate::state::queue_name_from_reference(reference.trim()).to_string())
            }
```
immediately under its own comment, `create_job_template.rs:202-205`: "A queue the caller DID name is a reference to a queue resource — by name or by ARN, as every queue reference in this service is" — but it is not resolved as every other queue reference is.

`services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/update_job_template.rs:192-194`
```rust
            Some(reference) => {
                Some(crate::state::queue_name_from_reference(reference.trim()).to_string())
            }
```

The siblings that do NOT trim:

`services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job.rs:339-343`
```rust
            let queue = queue_reference
                .as_deref()
                .map(crate::state::queue_name_from_reference)
                .map(str::to_string)
                .unwrap_or_else(|| crate::state::DEFAULT_QUEUE_NAME.to_string());
```

`services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:4497-4500`
```rust
    pub fn holds_queue(&self, reference: &str) -> bool {
        self.queues
            .contains_key(queue_name_from_reference(reference))
    }
```

The rule the crate states for itself, `services/service-com-amazonaws-mediaconvert-mediaconvert/tests/all/queue_name_identity.rs:8-11`:
"Every queue-addressing path (`GetQueue`, `UpdateQueue`, `DeleteQueue`, and a job's `Queue` reference) therefore resolves the string the create was given, and a name that differs only by surrounding whitespace is a DIFFERENT queue rather than the same one."

And the existing test that pins it for the nested queue reference, `services/service-com-amazonaws-mediaconvert-mediaconvert/tests/all/blank_resource_name.rs:444` and `:509-518`:
```rust
const PADDED_HOP_QUEUE: &str = "  padded hop  ";
...
    rows.push((
        "an entry naming the stored queue whose name is padded".to_string(),
        json!([{ "queue": PADDED_HOP_QUEUE, "waitMinutes": 10 }]),
        Expect::Accepted,
    ));
    rows.push((
        "an entry naming that queue's trimmed spelling".to_string(),
        json!([{ "queue": PADDED_HOP_QUEUE.trim(), "waitMinutes": 10 }]),
        Expect::NotFound,
    ));
```
No test in the crate exercises the TOP-LEVEL `Queue` member of `CreateJobTemplate`/`UpdateJobTemplate` with a padded reference, so the trim is unpinned: `blank_resource_name.rs:394-418` only drives blank values through it, and `queue_name_identity.rs:142-160` only drives padded names through `CreateJob$Queue`.

## Steps to Reproduce

1. CreateQueue with `{"name": "  padded  "}` → 201; the queue is stored and readable under the padded name (pinned by tests/all/queue_name_identity.rs).
2. CreateJob with `{"role": ..., "queue": "  padded  ", "settings": <valid>}` → 201, and the job's `queue` ARN carries the padded name verbatim.
3. CreateJobTemplate with `{"name": "t1", "queue": "  padded  ", "settings": <valid>}` → 404 NotFoundException ("queue"), because create_job_template.rs:218 trims the reference to "padded", which is not a stored key — the same reference CreateJob just accepted.
4. The other direction: on a fresh emulator, CreateQueue with `{"name": "padded"}`, then CreateJobTemplate with `{"name": "t2", "queue": "  padded  ", "settings": <valid>}` → 201 and GetJobTemplate reports `queue: "padded"`, while CreateJob with `"queue": "  padded  "` answers 404 for the identical value.
5. Same two outcomes through UpdateJobTemplate with `{"queue": "  padded  "}` (update_job_template.rs:193).
