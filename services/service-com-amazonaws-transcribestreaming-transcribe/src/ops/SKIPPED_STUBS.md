# Skipped operation stubs

The scaffold op-stub generator declined to emit a stub for the operations
below. Every remaining decline class is one the generator cannot TYPE: a
Unit/absent input, a `@streaming` member (event-stream union or streaming
blob), a member whose target is a shape kind that has no Rust type, or a
malformed model. A stub that does not compile would break the tenant build,
so each is left modeled-but-unwired — hand-author it in `ops/<op>.rs`, using
a rendered sibling as the pattern.

Note the difference from a SCAFFOLDED operation. Both answer HTTP 501, but a
scaffolded op has a compile-checked typed skeleton and is in the service's
`operations(...)` allow-list; an op listed here has neither, and is answered
by the framework's unroutable-operation path instead.

An output that is a SCALAR, list or map is declined: only a structure or a
union can be declared as `{Op}Output`. A `@required` union member and a
`@required`-member reference cycle are NOT decline reasons — a scaffolded
handler returns an error, so it never has to construct a value.

// SKIPPED: StartCallAnalyticsStreamTranscription — unsupported shape construct: operation 'StartCallAnalyticsStreamTranscription' has a @streaming event-stream input member, which needs wire framing the generator does not emit

The three `Start*` operations below were skipped for the same reason and are now
HAND-AUTHORED, so they are no longer unwired: each has its own file in `src/ops/`,
the `vnd.amazon.eventstream` framing lives in `src/ops/types/event_stream.rs`, and
`src/service.rs` routes a request to it through `Service::override_operation` —
which the router consults BEFORE the generated routing table, whose shim for a
`@streaming`-output operation declines the request.

What they do NOT do is recognise speech: there is no recogniser in this process, so
each result stream carries the modeled `ServiceUnavailableException` member of its
`@streaming` union (framed as an in-band exception) instead of an invented
transcript. Everything around the recognition — the modeled refusals, the session
and its settings, the conflict rule, the capacity limits, and the HealthScribe
session `GetMedicalScribeStream` reports — is real.

`StartCallAnalyticsStreamTranscription` above is NOT implemented and still answers
the framework's "modeled but not served" response; it has its own gap issue.

// HAND-AUTHORED: StartMedicalScribeStream — src/ops/start_medical_scribe_stream.rs
// HAND-AUTHORED: StartMedicalStreamTranscription — src/ops/start_medical_stream_transcription.rs
// HAND-AUTHORED: StartStreamTranscription — src/ops/start_stream_transcription.rs
