Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 409cf01b27185a74633e00014402d3a564807f11

# SynthesizeSpeech refuses a voice/engine mismatch with an unmodeled ValidationException instead of EngineNotSupportedException

_Filed: 2026-10-24T14:19:39Z_
_Operation: SynthesizeSpeech | Class: error-response_

> _Note: this issue was identified against the code base at commit `409cf01b27185a74633e00014402d3a564807f11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A request naming a VoiceId that does not support the requested Engine is exactly the condition the modeled `EngineNotSupportedException` describes ("This engine is not compatible with the voice that you have designated. Choose a new voice that is compatible with the engine or change the engine and restart the operation."), and the model lists that error on SynthesizeSpeech.

Expected: POST /v1/speech with e.g. VoiceId=Geraint + Engine=neural is refused with HTTP 400 and error code `EngineNotSupportedException`, the code the model declares for an engine the designated voice cannot use. An SDK generated from this model then surfaces a typed `EngineNotSupportedException` the caller can branch on.

Actual: the handler returns the scaffold wrapper's synthetic refusal — HTTP 400 with error code `ValidationException` and the message "This voice does not support the selected engine: <engine>". `ValidationException` is NOT among SynthesizeSpeech's modeled errors (the model binds `com.amazonaws.polly#ValidationException` only to StartSpeechSynthesisStream), so a generated client cannot map the code to a typed exception for this operation and reports a generic client error instead. `EngineNotSupportedException` is unreachable anywhere in the crate — `crate::service::errors` has a constructor for every other modeled Polly error but none for this code, so no request can ever produce it.

Secondary symptom of the same class: the document the emulator writes for that `ValidationException` code is the synthetic `smithy.framework#ValidationException` (message only), while this service's own `ValidationException` shape declares `message` AND `reason` (a `ValidationExceptionReason` enum) as @required plus an optional `fields` list. So even on the reading that Polly answers a validation-shaped refusal here, the body is missing members the model marks required and a client reading `reason`/`fields` gets nothing.

## Evidence

Spec: smithy_query(operation="SynthesizeSpeech") lists EngineNotSupportedException (HTTP 400) among its errors; smithy_query(query="shape", shape="EngineNotSupportedException") documents it as "This engine is not compatible with the voice that you have designated...", i.e. the error is specific to this failure mode. smithy_query(query="shape", shape="ValidationException") shows the service's own ValidationException requires `message` and `reason` and carries `fields`, and that shape is bound only to StartSpeechSynthesisStream, not to SynthesizeSpeech.

Handler: services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs — inside `handle`, after resolving the voice from the catalog, `if !voice.supports_engine(engine) { return Err(crate::scaffolded::Scaffolded::validation(format!("This voice does not support the selected engine: {engine}"))) }` (the branch commented "Amazon Polly refuses this combination as a REQUEST-SHAPE failure ... rather than with the `EngineNotSupportedException` the model also declares"). src/scaffolded.rs renders `Scaffolded::ValidationException` as HTTP 400 with shape_name() == "ValidationException" and a body carrying only `message`.

Proof the modeled code is dead: src/service.rs's `errors` module (constructors at lines ~268-366) has a constructor for every other modeled Polly error but no `engine_not_supported()`, and src/service.rs lines ~187-188 state the deliberate substitution. Grep for EngineNotSupported over src/ matches only doc comments.

Test coverage: tests/all/error_semantics.rs lines ~509-528 and tests/all/lexicons_and_speech.rs lines ~909-922 currently assert the (400, "ValidationException") answer for Geraint/neural, Kevin/standard and Nicole/generative — the divergence is locked in by the emulator's own tests rather than uncovered, the same situation as the already-filed PutLexicon unsupported-alphabet substitution.

## Steps to Reproduce

1. Boot the emulator and call SynthesizeSpeech with Text="hello world", OutputFormat="mp3", VoiceId="Geraint", Engine="neural" (Geraint is a standard-only voice in the seeded catalog)
2. Observe HTTP 400 with error code `ValidationException` and a body carrying only a `message` key
3. Expected per the model: HTTP 400 with error code `EngineNotSupportedException`, which a generated SDK surfaces as a typed exception for this operation; `ValidationException` is not a modeled error of SynthesizeSpeech at all
4. Repeat with VoiceId="Kevin", Engine="standard" (neural-only voice) for the mirror case
5. Confirm the modeled code is unreachable: no request shape makes the emulator emit `EngineNotSupportedException` (no constructor exists in src/service.rs's errors module)
