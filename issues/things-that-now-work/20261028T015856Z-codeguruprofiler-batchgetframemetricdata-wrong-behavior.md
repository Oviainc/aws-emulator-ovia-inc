Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 198aaba66ae988b1c226897aad5a72b32428db18

# Frame metric values are computed as 0 for a stored profile whose declared Content-Type is not the representation its bytes are a document in

_Filed: 2026-10-28T01:58:56Z_
_Operation: BatchGetFrameMetricData | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `198aaba66ae988b1c226897aad5a72b32428db18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`frame_metric_data` reads each stored profile through `crate::ops::get_profile::profile_document`, which decodes the bytes ONLY in the representation the stored `Content-Type` names. A profile that `PostAgentProfile` accepted under a media type that does not describe its bytes — a JSON profile posted under `Content-Type: text/plain` (which this crate's own test table pins as accepted), a JSON profile posted as `application/x-amzn-ion`, or binary Ion posted as `application/json` — therefore yields `None`, is dropped from `documents`, and contributes no counts at all.

The handler then answers 200 with `endTimes` naming the step as PROCESSED (the profile is aggregated, so it is not reported in `unprocessedEndTimes`) and every requested `frameMetricData[].values` entry equal to `0.0`, because `whole[step]` stayed 0. So a caller is told "this step was processed and the frame accounts for none of the profile's samples" for a profile whose frames and thread-state counts are plainly present in the stored bytes — a wrong computed metric reported as a successful one, not as an unprocessed step.

The crate already owns the reader that would find those frames: `get_profile::submitted_document` reads bytes "in WHICHEVER representation they are one in", and it is the reader `PostAgentProfile` used to decide the payload WAS a profile. The sibling comment on `profile_document` even states the invariant this breaks — "`BatchGetFrameMetricData` derives a frame metric from the same documents, and both must read a profile the same way" — while the two paths disagree about what "the same way" is: the writer reads whichever representation the bytes are, the reader reads only the declared one.

## Evidence

The reader this operation uses — src/ops/batch_get_frame_metric_data.rs:443-451:
```
    let documents: Vec<(usize, Element)> = profiles
        .iter()
        .filter_map(|(step, profile)| {
            // Bytes that are no document in the representation they were posted under
            // hold no frames this service can see, so they record no counts — the same
            // answer `GetProfile` gives such a profile when it is asked to re-read it.
            crate::ops::get_profile::profile_document(profile).map(|document| (*step, document))
        })
        .collect();
```
and the 0-valued series that follows — src/ops/batch_get_frame_metric_data.rs:463-485:
```
        let mut named = vec![0.0f64; step_count];
        let mut whole = vec![0.0f64; step_count];
        for (step, document) in &documents {
...
                    if whole[*step] > 0.0 {
                        named[*step] / whole[*step]
                    } else {
                        0.0
                    }
```

What `profile_document` does, and the invariant stated on it — src/ops/get_profile.rs:155-164:
```
/// The stored profile `profile` read as a document, or `None` when its bytes are not one
/// in the representation it was posted under (so it holds no frames this service can
/// see).
///
/// `pub(crate)` because reading a stored profile as a document is not only this
/// operation's business: `BatchGetFrameMetricData` derives a frame metric from the same
/// documents, and both must read a profile the same way.
pub(crate) fn profile_document(profile: &crate::state::AgentProfile) -> Option<Element> {
    decoded(&profile.bytes, Representation::of(&profile.content_type)?)
}
```

The reader the WRITER used on the very same bytes — src/ops/get_profile.rs:166-178:
```
/// `bytes` read as a document in WHICHEVER representation they are one in, or `None` when
/// they are one in neither.
...
/// exactly the ones read here — so a document this service accepts is a document it can
/// also serve.
pub(crate) fn submitted_document(bytes: &[u8]) -> Option<Element> {
```
called from src/ops/post_agent_profile.rs:151:
```
    let Some(document) = super::get_profile::submitted_document(bytes) else {
```
with the writer's rule at src/ops/post_agent_profile.rs:140-142:
```
/// The data is read as a document and judged on that document, NOT on the media type it
/// was announced under: the `Content-Type` says how to read the bytes, it cannot make
/// bytes that carry no profile into one.
```

The mismatched submission is accepted, pinned by this crate — tests/all/posted_profile_validation.rs:124-129 (`TEXT` = `"text/plain"`, line 34):
```
        Case {
            label: "a profile under a media type that names no representation",
            content_type: TEXT,
            bytes: json(&serde_json::json!({ START: PROFILE_START_MILLIS })),
            outcome: Outcome::Stored,
        },
```

Existing coverage does NOT pin this: the only 0.0-for-unreadable row uses bytes that are no document at all and is seeded into the store because it cannot be posted — tests/all/frame_metric_data.rs:472-478 and :489-493:
```
        ValueCase {
            label: "bytes that are no document hold no frames",
            body: b"this is not a profile document".to_vec(),
...
            expected: 0.0,
        },
...
        // A row whose bytes are no document at all cannot be POSTED — `PostAgentProfile`
        // parses the data it is handed — so it is placed in the store directly, which is
        // the only way such bytes get there; every other row is submitted the way an
        // agent submits one.
```
Every other row posts under `"application/json"` (tests/all/frame_metric_data.rs:275, :708), so no test exercises a document stored under a media type that is not its representation.

## Steps to Reproduce

1. CreateProfilingGroup(profilingGroupName="pg").
2. PostAgentProfile for "pg" with body {"start":1600000000000,"callgraph":{"A":{"RUNNABLE":3}}} and Content-Type: text/plain — accepted with 204 (tests/all/posted_profile_validation.rs:124-129), or post the same JSON body under Content-Type: application/x-amzn-ion.
3. Wait for the profile's aggregation to fall due, then POST /profilingGroups/pg/frames/-/metrics?targetResolution=PT5M&startTime=<window start>&endTime=<window end> with frameMetrics=[{frameName:"A", type:"AggregatedRelativeTotalTime", threadStates:["RUNNABLE"]}].
4. Observe 200 with the step listed in endTimes (not in unprocessedEndTimes) and frameMetricData[0].values == [0.0], although the stored bytes record 3 RUNNABLE samples under frame A.
5. Post the same body under Content-Type: application/json into a second group and repeat: the value is non-zero, so the only difference is which representation the declared media type named.
6. Read src/ops/batch_get_frame_metric_data.rs:443-451 with src/ops/get_profile.rs:162-164 and src/ops/get_profile.rs:174-178 — the writer accepted the bytes with the representation-agnostic reader, the metric path uses the declared-type-only reader.
