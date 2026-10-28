Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 198aaba66ae988b1c226897aad5a72b32428db18

# GetProfile drops maxDepth and the accepted representation for a profile whose stored Content-Type does not describe its bytes

_Filed: 2026-10-28T01:58:56Z_
_Operation: GetProfile | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `198aaba66ae988b1c226897aad5a72b32428db18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PostAgentProfile` deliberately judges a submitted profile on the DOCUMENT it is, not on the `Content-Type` it was announced under (post_agent_profile.rs:140-142), and it reads it with `get_profile::submitted_document`, which tries BOTH representations. The crate's own test table pins such a submission as accepted: a JSON profile posted under `Content-Type: text/plain` is `Outcome::Stored`.

But the READER, `served_profile`, resolves the stored profile's representation from the declared media type alone (`Representation::of(&profile.content_type)`). For every profile whose declared type is not the representation its bytes actually are a document in — `text/plain` with JSON bytes (test-pinned as accepted), JSON bytes declared `application/x-amzn-ion`, binary-Ion bytes declared `application/json` — `decoded(..)` returns `None` and the function falls into its `as_stored()` branch. The consequence is that two documented input members are SILENTLY DROPPED:

* `maxDepth` — "The maximum depth of the stacks in the code that is represented in the aggregated profile. … If the maxDepth is set to 2, then the aggregated profile contains representations of methods A and B." The response carries every frame at every depth, with no indication the bound was not applied.
* `accept` — the request's negotiated representation is computed (`wanted`) and then thrown away, so the bytes are served in the representation they are NOT announced in, and `contentType` reports the declared media type rather than the one the served bytes are in (e.g. JSON bytes reported as `application/x-amzn-ion`).

This contradicts the rule the crate states one function below the offending one: `submitted_document`'s own doc says "the representations it can be one in are exactly the ones read here — so a document this service accepts is a document it can also serve". `served_profile`'s justification ("A profile this service cannot read as a document in the representation it was posted under holds no frames it can see") only holds for bytes that are a document in NEITHER representation; for a mismatched-but-readable profile the frames are right there, and the crate already owns the reader that finds them.

(Distinct from the open GetProfile wrong-behavior issue, which is about the VALUE of `contentType` for a third media type; this is about the two input members the same branch discards, and it also fires for the two documented media types when they are swapped.)

## Evidence

The reader that trusts the declared type only — src/ops/get_profile.rs:310-332:
```
fn served_profile(
    profile: &crate::state::AgentProfile,
    accept: Option<&str>,
    max_depth: Option<i32>,
) -> (Vec<u8>, String) {
    let as_stored = || (profile.bytes.clone(), profile.content_type.clone());
    let stored = Representation::of(&profile.content_type);
...
    let Some(document) = decoded(&profile.bytes, stored) else {
        return as_stored();
    };
```

The rule it breaks, in the same file — src/ops/get_profile.rs:166-178:
```
/// `bytes` read as a document in WHICHEVER representation they are one in, or `None` when
/// they are one in neither.
///
/// `pub(crate)` because the bytes a caller SUBMITS are read without being told which
/// representation they are in: `PostAgentProfile` parses the profile it is handed to
/// decide whether it is a profile at all, and the representations it can be one in are
/// exactly the ones read here — so a document this service accepts is a document it can
/// also serve.
pub(crate) fn submitted_document(bytes: &[u8]) -> Option<Element> {
    Representation::ALL
        .into_iter()
        .find_map(|representation| decoded(bytes, representation))
}
```

The writer that uses that helper and ignores the media type — src/ops/post_agent_profile.rs:138-153:
```
/// Why `bytes` are not an agent profile, or `None` when they are one.
///
/// The data is read as a document and judged on that document, NOT on the media type it
/// was announced under: the `Content-Type` says how to read the bytes, it cannot make
/// bytes that carry no profile into one.
...
    let Some(document) = super::get_profile::submitted_document(bytes) else {
```
and it stores the header verbatim — src/ops/post_agent_profile.rs:321-328:
```
        match guard.post_profile(
            &input.profiling_group_name,
            input.profile_token.as_deref(),
            crate::state::AgentProfile {
                bytes: input.agent_profile.clone(),
                content_type: input.content_type.clone(),
```

The model sentence the dropped member carries — `smithy_query(query="doc", shape="GetProfileRequest", member="maxDepth")`, restated in src/ops/get_profile.rs:87-91:
```
//! * `maxDepth` (MaxDepth, range 1..10000) — The maximum depth of the stacks in the code that
//!   is represented in the aggregated profile. For example, if CodeGuru Profiler finds a
//!   method A, which calls method B, which calls method C, which calls method D, then the
//!   depth is 4. If the maxDepth is set to 2, then the aggregated profile contains
//!   representations of methods A and B.
```

The mismatched submission is ACCEPTED, pinned by this crate's own table — tests/all/posted_profile_validation.rs:124-129:
```
        Case {
            label: "a profile under a media type that names no representation",
            content_type: TEXT,
            bytes: json(&serde_json::json!({ START: PROFILE_START_MILLIS })),
            outcome: Outcome::Stored,
        },
```
(`TEXT` is `"text/plain"`, tests/all/posted_profile_validation.rs:34.)

No test pins the behaviour reported here: every row of tests/all/profile_representations.rs::a_payload_that_is_no_document_is_served_as_it_is_stored uses bytes that are a document in NEITHER representation — tests/all/profile_representations.rs:703-707:
```
    let rows: [(&str, &str, Vec<u8>); 3] = [
        ("every byte value, posted as Ion", ION, (0..=255u8).collect()),
        ("a truncated JSON text", JSON, b"{\"callgraph\":".to_vec()),
        ("one brace, posted as JSON", JSON, vec![b'{']),
    ];
```
and the depth/negotiation suites (lines 271, 417, 489) always post under the media type the bytes really are.

## Steps to Reproduce

1. CreateProfilingGroup(profilingGroupName="pg").
2. PostAgentProfile for "pg" with body {"start":1600000000000,"callgraph":{"A":{"B":{"RUNNABLE":2}}}} and Content-Type: text/plain — accepted with 204 (pinned by tests/all/posted_profile_validation.rs:124-129), or equivalently post the same JSON body under Content-Type: application/x-amzn-ion.
3. Wait for the posted profile's aggregation to fall due (crate::state::AGGREGATION_PASS_MILLIS).
4. GET /profilingGroups/pg/profile?period=PT5M&maxDepth=1 with Accept: application/json.
5. Observe the full, unbounded document is returned: src/ops/get_profile.rs:316 resolves no representation for the declared media type, src/ops/get_profile.rs:330-332 returns as_stored(), so maxDepth=1 and the negotiated representation are both discarded and contentType reports the declared media type rather than the representation of the bytes served.
6. Compare src/ops/get_profile.rs:169-173, which states that a document this service accepts is a document it can also serve, and src/ops/post_agent_profile.rs:151, which read the very same bytes as a document with submitted_document.
