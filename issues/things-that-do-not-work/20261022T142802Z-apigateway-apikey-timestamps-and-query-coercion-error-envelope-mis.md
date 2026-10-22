Issue-Source: trace-sweep-divergence
Issue-Trace: apigw-envelope-api-key-get-include-value-masking
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigw-envelope-api-key-get-include-value-masking apigw-envelope-api-keys-list-include-values-masking
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetApiKeys|4xx->4xx|$.Message
Issue-Key: apigateway|GetApiKeys|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261022T142802Z-apigateway-patch-update-operations-are-accepted-but-never-applied.md
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# ApiKey timestamps and query coercion error envelope mis-serialized

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApiKey

## Description
Two causes, one shared fault area: the emulator's API Gateway response envelope is serialized with the wrong value shapes — a timestamp emitted with sub-second precision, and a request-coercion failure emitted in the wrong error envelope.

Cause 1 of 2: ApiKey.createdDate / lastUpdatedDate are serialized as fractional epoch values.
AWS: ApiKey.createdDate and ApiKey.lastUpdatedDate target the Timestamp shape (Smithy: GetApiKey output {... createdDate: Timestamp, lastUpdatedDate: Timestamp ...}). On this REST-JSON surface AWS emits them as whole-second epoch numbers; the fixture asserts their SHAPE rather than a literal instant, and the emulator's value fails that shape check. (The specific instant is environment-dependent and is not a requirement — the serialized form is.)
Emulator: emits a float carrying microseconds, e.g. `1792678925.0145848`, for both members, so the shaped assertion fails on both. HTTP 200 matches.
Why: clients that parse these members as integer epoch seconds (or into a fixed-width integer field) either reject the body or truncate silently; anything that compares or hashes the serialized value diverges from AWS, and the sub-second digits expose emulator wall-clock internals. The same step also omits `stageKeys: []`, which is the separate serializer gap filed as the "Empty stageKeys and tags members omitted from ApiKey responses" issue — this step will not replay clean until both are fixed.
Fix: the ApiKey timestamp serializer (the Timestamp output formatting for this service's REST-JSON responses) — emit whole-second epoch values.
Traces: fixture `apigw-envelope-api-key-get-include-value-masking`, step `task#2`; 3 affected steps in that fixture.

Cause 2 of 2: a non-boolean `includeValues` query value produces the wrong error envelope.
AWS: GetApiKeys is GET /apikeys with `includeValues` bound as a NullableBoolean @httpQuery member (Smithy: GetApiKeys input {position, limit, nameQuery, customerId, includeValues: NullableBoolean @httpQuery}). A value that cannot be coerced to a boolean fails at the protocol/deserialization layer, not in the operation's own validation, so AWS answers HTTP 400 with the body field capital-`Message` (recorded example: `'maybe' can not be converted to Boolean`) and an `x-amzn-errortype` whose value is NOT `ValidationException`.
Emulator: HTTP 400 (status matches) but the body carries no `Message` member at all and the header reports `x-amzn-errortype: ValidationException`, which is the wrong error code and the wrong envelope shape for a query-parameter coercion failure.
Why: the error code is what SDKs map to an exception class and what error-handling branches switch on, so a client gets a modeled validation exception where AWS reports a protocol-level coercion failure; and because the message lands under a differently-cased key, clients that read `Message` on this envelope see nothing.
Fix: the request deserializer for boolean-typed @httpQuery members — fail coercion in the protocol layer and serialize that failure with the recorded error type and the capital-`Message` body field, rather than routing it through the operation's ValidationException path.
Traces: fixture `apigw-envelope-api-keys-list-include-values-masking`, step `task#5`.

## Evidence
Representative (Cause 1): fixture apigw-envelope-api-key-get-include-value-masking step task#2, operation GetApiKey, expected HTTP 200 / actual HTTP 200, divergence_key `service=apigateway|GetApiKey|status=200->200|keys=$.createdDate,$.lastUpdatedDate,$.stageKeys`, detail `$.createdDate: value_mismatch (expected '<shaped-not-asserted>', got 1792678925.0145848)` / `$.lastUpdatedDate: value_mismatch (expected '<shaped-not-asserted>', got 1792678925.0145848)` / `$.stageKeys: missing (expected [], got None)`; affected_count=3.
Cause 2: apigw-envelope-api-keys-list-include-values-masking#task#5, operation GetApiKeys, expected HTTP 400 / actual HTTP 400, divergence_key `service=apigateway|GetApiKeys|status=400->400|keys=$.Message,$headers.x-amzn-errortype`, detail `$.Message: missing (expected "'maybe' can not be converted to Boolean", got None)` / `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')`.
Smithy evidence: GetApiKey GET /apikeys/{apiKey} code 200 returns ApiKey with createdDate/lastUpdatedDate targeting Timestamp; GetApiKeys GET /apikeys code 200 with input member includeValues targeting NullableBoolean bound via @httpQuery `includeValues`.
The recorded message text in Cause 2 is cited as an example; the requirements asserted are the body member name `Message` and the `x-amzn-errortype` value, which the emulator reports as `ValidationException` where the recording's shape check rejects it.
Run summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, state_reset_failures=0.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay wire fixture `apigw-envelope-api-key-get-include-value-masking` and inspect step `task#2`: the GetApiKey 200 body reports createdDate and lastUpdatedDate as fractional epoch floats (microsecond digits) instead of whole-second epoch numbers.
3. Replay `apigw-envelope-api-keys-list-include-values-masking` step `task#5` (GET /apikeys?includeValues=maybe): the emulator answers 400 with `x-amzn-errortype: ValidationException` and no `Message` body member, where the recording has a protocol coercion error carrying `Message`.
4. Both units must match the recorded serialization before this issue is closed.
