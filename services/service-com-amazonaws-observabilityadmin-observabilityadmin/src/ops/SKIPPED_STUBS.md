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

// SKIPPED: GetTelemetryEnrichmentStatus — unsupported shape construct: operation 'GetTelemetryEnrichmentStatus' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GetTelemetryEvaluationStatus — unsupported shape construct: operation 'GetTelemetryEvaluationStatus' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: GetTelemetryEvaluationStatusForOrganization — unsupported shape construct: operation 'GetTelemetryEvaluationStatusForOrganization' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: StartTelemetryEnrichment — unsupported shape construct: operation 'StartTelemetryEnrichment' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: StopTelemetryEnrichment — unsupported shape construct: operation 'StopTelemetryEnrichment' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: StopTelemetryEvaluation — unsupported shape construct: operation 'StopTelemetryEvaluation' has a Unit/absent input, which the generator does not emit yet
// SKIPPED: StopTelemetryEvaluationForOrganization — unsupported shape construct: operation 'StopTelemetryEvaluationForOrganization' has a Unit/absent input, which the generator does not emit yet
