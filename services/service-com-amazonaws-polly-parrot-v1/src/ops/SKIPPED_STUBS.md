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

`StartSpeechSynthesisStream` was skipped for that reason and is now HAND-AUTHORED,
so it is no longer unwired: it has its own file in `src/ops/`, its shapes are in
`src/ops/types/start_speech_synthesis_stream_types.rs`, the
`vnd.amazon.eventstream` framing and the request reader are the shared ones in
`service_common::event_stream`, and `src/service.rs` routes a request to it through
`Service::override_operation` — which the router consults BEFORE the generated
routing table, whose shim for a `@streaming`-output operation declines the request.

What it does NOT do is synthesise speech: there is no speech synthesiser in this
process, so each `AudioEvent` carries a SILENT signal in the requested encoding
lasting as long as this emulator's timeline model says the text takes to speak —
the same answer `SynthesizeSpeech` gives. Everything around the synthesis is real:
the engine and voice rules, the lexicon resolution, the sample-rate domain, the
billed character count and the modelled refusals.

// HAND-AUTHORED: StartSpeechSynthesisStream — src/ops/start_speech_synthesis_stream.rs
