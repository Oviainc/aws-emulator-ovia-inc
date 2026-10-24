Issue-Severity: HIGH
Issue-Gap-Class: missing
Issue-Commit: 30b4aefb4b2ff0fa6acb9bcc02c838fcaca2cb1f

# Implement StartMedicalScribeListeningSession: no handler exists, so no listening session can ever be created

_Filed: 2026-10-24T02:57:47Z_
_Operation: StartMedicalScribeListeningSession | Class: missing_

> _Note: this issue was identified against the code base at commit `30b4aefb4b2ff0fa6acb9bcc02c838fcaca2cb1f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model binds `StartMedicalScribeListeningSession` (POST /medical-scribe-stream/, bidirectional event stream) as the only operation that creates a Medical Scribe listening session. The emulator has NO handler file for it: `src/ops/` contains no `start_medical_scribe_listening_session.rs`, the op is not in the service's operation allow-list, and `src/ops/SKIPPED_STUBS.md` records that the scaffold generator declined it because its input carries a `@streaming` event-stream member it cannot type. A call therefore falls through to the framework's unroutable-operation path instead of producing the modeled response.

Expected: `StartMedicalScribeListeningSession` validates the required header-bound inputs (`x-amzn-medscribe-session-id`, `x-amzn-medscribe-domain-id`, `x-amzn-medscribe-subscription-id`, `x-amzn-medscribe-language-code`, `x-amzn-medscribe-sample-rate`, `x-amzn-medscribe-media-encoding`), resolves the live domain + subscription (ResourceNotFoundException otherwise), persists a session record (`crate::state::ScribeSession` already exists and models every field), echoes the session/domain/subscription/language/sample-rate/encoding response headers plus `x-amzn-request-id`, and streams `MedicalScribeOutputStream` events.

Actual: nothing is routed; no session record can be created through the API at all. The knock-on effect is that the implemented `GetMedicalScribeListeningSession` can only ever answer `ResourceNotFoundException` for a client-driven workflow — the only way a session reaches the store today is a test writing `ScribeSession` into state directly (`tests/all/domain_account_cap_and_lookup_semantics.rs::seed_session`, whose comment says the starting operation "is not generated into this crate"). The whole Medical Scribe workflow (start stream -> read session details) is unreachable.

## Evidence

Spec: smithy_query(query="operation") — StartMedicalScribeListeningSession is bound to the service with http POST /medical-scribe-stream/, six @required @httpHeader inputs, `inputStream` (MedicalScribeInputStream, @httpPayload, @streaming) and a `responseStream` output, errors AccessDenied/InternalServer/ResourceNotFound/ServiceQuotaExceeded/Throttling/Validation. OPERATIONS.md lists it in the 16 bound operations.
Implementation: glob of services/service-com-amazonaws-connecthealth-connecthealth/src/ops/*.rs shows no handler file for it (unlike the three scaffolded 501 ops get_patient_insights_job.rs / start_patient_insights_job.rs / untag_resource.rs, which do exist). src/ops/SKIPPED_STUBS.md: "// SKIPPED: StartMedicalScribeListeningSession — unsupported shape construct: … @streaming event-stream input member". src/state.rs:289-295 states the operation "this emulator does not serve … so this crate has no handler for it".
Tests: no test exercises the operation; tests/all/domain_account_cap_and_lookup_semantics.rs:490-535 seeds a ScribeSession by locking the store directly precisely because no API path creates one. Nothing asserts the operation's behavior, so this gap is uncovered.

## Steps to Reproduce

1. POST /domain with {"name":"clinic"} and capture the returned domainId.
2. POST /domains/{domainId}/subscriptions and capture the returned subscriptionId.
3. POST /medical-scribe-stream/ with headers x-amzn-medscribe-session-id=<uuid>, x-amzn-medscribe-domain-id=<domainId>, x-amzn-medscribe-subscription-id=<subscriptionId>, x-amzn-medscribe-language-code=en-US, x-amzn-medscribe-sample-rate=16000, x-amzn-medscribe-media-encoding=pcm and an event-stream body.
4. Observe the request is not served by any handler (framework unroutable-operation answer) instead of starting a session and returning the session/request-id response headers.
5. GET /medical-scribe-stream/domain/{domainId}/subscription/{subscriptionId}/session/{uuid} and observe ResourceNotFoundException — no session exists because none can be created through the API.
