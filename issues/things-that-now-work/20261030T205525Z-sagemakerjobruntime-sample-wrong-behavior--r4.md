Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 14cae1c0e7b9c9b79fa8777e5722a482560d4956

# The store-wide retained-byte ceiling is checked against the request half only, while the counter it guards sums request AND response bytes

_Filed: 2026-10-30T20:55:25Z_
_Operation: Sample | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `14cae1c0e7b9c9b79fa8777e5722a482560d4956`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`append_turn` is the one writer of a captured turn and the one maintainer of the store-wide retained-byte total. The counter it maintains is documented to sum BOTH halves of every turn ("summed over both the request and the response body of each turn"), and the ceiling it guards is documented as a real ceiling on that total: "a turn that would pass this ceiling is refused as a quota, with nothing written".

The admission check does not measure the quantity it guards. It compares `retained_body_bytes + request_bytes` against `MAX_RETAINED_BODY_BYTES` (state.rs:387), but once the turn is admitted the counter is advanced by `request_bytes + response_bytes` (state.rs:412-414). A turn whose request half fits under the ceiling but whose request+response pair does not is therefore ADMITTED and written, and the tracked total ends ABOVE the documented ceiling — the state the next `AppendRefusal::RetainedBytesQuota` message then reports as "This emulator already retains the maximum of {MAX_RETAINED_BODY_BYTES} bytes" while it in fact retains more.

The caller chooses body sizes freely (any size up to `MAX_TURN_BODY_BYTES` is admitted), so the boundary is reachable: a sampling call whose `Body` is exactly `MAX_RETAINED_BODY_BYTES - retained_body_bytes` bytes passes the check at equality and then pushes the counter past the ceiling by the whole response body. The overshoot is bounded (one served completion envelope, a few hundred bytes), so the practical impact is small — the defect is that the check and the counter measure different quantities, which is exactly what the field's own doc says it exists to prevent ("it is what makes `MAX_RETAINED_BODY_BYTES` a real ceiling rather than a comment"). The sibling per-turn check immediately above it does measure what it guards (`request_bytes > MAX_TURN_BODY_BYTES`, state.rs:382), which is why only this one can be passed.

`Sample` is named because its handler is the first caller of the shared path (`crate::state::serve_sampling_turn` -> `append_turn`); `SampleWithResponseStream` reaches the same code and the same counter.

## Evidence

The rule the crate states for itself, `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/state.rs:66-74`:

    /// How many bytes of payload the whole store may retain, across every turn of
    /// every trajectory.
    ///
    /// The store-wide half of the same bound: many in-bound bodies must not add up
    /// to an unbounded store either, so the retained total is tracked
    /// ([`...State::retained_body_bytes`])
    /// and a turn that would pass this ceiling is refused as a quota, with nothing
    /// written.
    pub const MAX_RETAINED_BODY_BYTES: usize = 64 << 20;

and what the counter is declared to hold, `src/state.rs:290-296`:

    /// How many payload bytes every captured turn of this store retains, summed over
    /// both the request and the response body of each turn.
    ///
    /// Maintained by [`Self::append_turn`], which is the only writer of a turn, and
    /// read through [`Self::retained_body_bytes`]; it is what makes
    /// [`MAX_RETAINED_BODY_BYTES`] a real ceiling rather than a comment.
    retained_body_bytes: usize,

The code under those comments, `src/state.rs:387-389` (admission — request half only):

        if self.retained_body_bytes.saturating_add(request_bytes) > MAX_RETAINED_BODY_BYTES {
            return Err(AppendRefusal::RetainedBytesQuota);
        }

and `src/state.rs:412-414` (accounting — both halves):

        self.retained_body_bytes = retained_before
            .saturating_add(request_bytes)
            .saturating_add(response_bytes);

The sentence the next refusal then states while the store holds more than it names, `src/state.rs:515-519`:

            Self::RetainedBytesQuota => format!(
                "This emulator already retains the maximum of {MAX_RETAINED_BODY_BYTES} bytes of \
                 captured payload, so no further turn can be appended to trajectory \
                 {trajectory_id}."

Test coverage: `tests/all/modelled_rejections.rs:281-347` (`a_payload_too_large_to_retain_is_refused_and_captured_bytes_are_counted`) exercises only the PER-TURN bound and asserts the counter loosely (`retained >= size`, line 312-316), so it neither pins nor would break on the store-wide accounting; `state.rs` has no unit test for the `RetainedBytesQuota` branch, and `retained_body_bytes` has a getter but no setter, so nothing in the crate pins the invariant either way.

## Steps to Reproduce

1. Register a job in the shared directory as the SageMaker control plane does (e.g. arn:aws:sagemaker:us-east-1:000000000000:job/AgentRFT/retain-run).
2. Drive POST /sample calls for that job with 1 MiB bodies (state::MAX_TURN_BODY_BYTES) until engine.state::<ServiceState>().lock().retained_body_bytes() is just under state::MAX_RETAINED_BODY_BYTES.
3. Read R = retained_body_bytes() and send one more POST /sample whose Body is exactly MAX_RETAINED_BODY_BYTES - R bytes.
4. Observe 200: the check at src/state.rs:387 passes at equality because it adds only the request half.
5. Read retained_body_bytes() again: it is now MAX_RETAINED_BODY_BYTES + <size of the served completion envelope>, i.e. above the ceiling the constant's own doc says no turn may pass, and the turn was written rather than refused with AppendRefusal::RetainedBytesQuota.
