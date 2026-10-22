Issue-Source: trace-sweep-divergence
Issue-Trace: dsdata-request-layer-ordering-and-http-surface
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsdata-request-layer-ordering-and-http-surface dsdata-sdk-unreachable-request-envelopes
Issue-Shape-Id: com.amazonaws.directoryservicedata#DirectoryServiceData
Issue-Key: ds-data|ListUsers|2xx->5xx|$.Output
Issue-Key: ds-data|ListUsers|2xx->5xx|$.Version
Issue-Key: ds-data|ListUsers|2xx->5xx|status
Issue-Key: ds-data|ListUsers|4xx->5xx|$.UnknownOperationException
Issue-Key: ds-data|ListUsers|4xx->5xx|status
Issue-Key: ds-data|ListUsers|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: ds-data|ListUsers|4xx->5xx|$.Message
Issue-Key: ds-data|ListUsers|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: ds-data|ListGroups|4xx->4xx|$
Issue-Key: ds-data|ListGroups|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261022T000042Z-ds-data-validationexception-body-omits-message-for-every-member-constraint.md
Issue-Commit: 668d747e62a7a896ef2863c29b158e984b097a40

# Request-layer errors answer 501 NotImplemented instead of the modeled unknown-operation and serialization envelopes

> _Note: this issue was identified against the code base at commit `668d747e62a7a896ef2863c29b158e984b097a40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListUsers

## Description
Everything Directory Service Data rejects BEFORE an operation handler runs — an unresolvable operation, an absent required member bound to the request, a body that cannot be deserialized — diverges in status and/or error type. The emulator falls back to the framework's rest default (501 NotImplemented) or to a generic ValidationException, where the recording shows the service's own pre-handler envelopes. 5 distinct causes, all ds-data; all 5 must be closed.

Cause 1 of 5 — unresolvable operation on a served route returns 501 instead of the recorded unknown-operation envelope.
AWS: recorded behavior only (the Smithy model binds 17 operations over restJson1, POST /Users/ListUsers etc., and models no envelope for an unnamed operation). The recording shows HTTP 200 with a body carrying `Output` = {"__type": "com.amazon.coral.service#UnknownOperationException"} and `Version` = "1.0".
Emulator: HTTP 501, and neither `$.Output` nor `$.Version` present.
Why: a client (or an SDK probing the surface) sees a transport-level "not implemented" for a request the service does answer, so it classifies a routing problem as an emulator/feature gap and may retry or abort rather than reading the unknown-operation marker.
Fix: the service's routing-error seam — `override_routing_error` in the ds-data service file, which currently returns None and so renders the framework's rest default (501).
Traces: dsdata-request-layer-ordering-and-http-surface#task#3.

Cause 2 of 5 — unknown request path returns 501 instead of 404 UnknownOperationException.
AWS: recorded behavior only. The recording shows HTTP 404 with an `UnknownOperationException` key in the body (recorded value empty).
Emulator: HTTP 501 and no `UnknownOperationException` key.
Why: 404 is the client-visible signal that the URI names no operation; 501 tells a client the operation exists but is unimplemented, which is a different remediation.
Fix: same routing-error seam, path-not-matched branch.
Traces: dsdata-request-layer-ordering-and-http-surface#task#4.

Cause 3 of 5 — a body value of the wrong JSON type is reported as ValidationException instead of SerializationException.
AWS: recorded behavior only (deserialization precedes constraint validation). The recording shows HTTP 400 with header x-amzn-errortype `SerializationException:http://internal.amazon.com/coral/com.amazon.coral.service/` and a Message describing the failed coercion (recording: "STRING_VALUE can not be converted to an Integer").
Emulator: HTTP 400 but x-amzn-errortype `ValidationException`, and no `Message` at all.
Why: the error CODE is what a client branches on. A deserialization failure reported as ValidationException tells the client its value broke a modeled constraint (retry with a different value) when in fact the request was not parseable into the modeled shape; the layering of the two error classes is observable and clients classify on it.
Fix: the ds-data request deserializer — a type-coercion failure must raise SerializationException before the constraint-validation walk, and must carry a Message.
Traces: dsdata-sdk-unreachable-request-envelopes#task#3.

Cause 4 of 5 — an absent required member returns 501 NotImplemented instead of 400 ValidationException.
AWS: Smithy ListUsersRequest.DirectoryId is @required (target DirectoryId, httpQuery DirectoryId); ValidationException is a common error of every ds-data operation (client, httpError 400). The recording shows HTTP 400, x-amzn-errortype `ValidationException:http://directoryservicedata.amazonaws.com/doc/2023-05-31/`, Message "1 validation error detected: Value null at 'directoryId' failed to satisfy constraint: Member must not be null".
Emulator: HTTP 501 with x-amzn-errortype `NotImplemented` and no Message. Hypothesis: because DirectoryId is bound to the query string, omitting it makes route/operation resolution fail, so the request never reaches the required-member check and falls through to the same rest 501 default as causes 1-2.
Why: the client is told the operation does not exist rather than that it omitted a required parameter — a 501 is also commonly treated as non-retryable infrastructure failure, so a simple caller bug surfaces as an outage.
Fix: required-member validation for ds-data must run after routing and before any not-implemented fallback (the operation's validation layer plus the routing fallback in the service file).
Traces: dsdata-sdk-unreachable-request-envelopes#task#4.

Cause 5 of 5 — an unparseable JSON body returns a ValidationException envelope instead of the recorded empty SerializationException body.
AWS: recorded behavior only. The recording for ListGroups with a malformed body shows HTTP 400, x-amzn-errortype `SerializationException:http://internal.amazon.com/coral/com.amazon.coral.service/`, and an EMPTY JSON body ({}).
Emulator: HTTP 400 with x-amzn-errortype `ValidationException` and body {"__type": "ValidationException", "message": "json parse error in lookahead"}.
Why: same class confusion as cause 3, plus the emulator invents a body where the service sends none; a client that keys off __type/errortype to decide whether to re-serialize and retry takes the wrong branch.
Fix: the ds-data body deserializer / framework-error rendering — a parse failure must render the SerializationException error type with the empty body the recording shows.
Traces: dsdata-sdk-unreachable-request-envelopes#task#6.

All five clusters reported owned_by empty and stub_path empty, so no open issue carries these replay keys and no unimplemented operation blocks them.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up true, bootstrap_signal false, state_reset_failures 0, 19/19 steps mismatched). Five ds-data clusters, verdict=mismatch:
1. ListUsers, dsdata-request-layer-ordering-and-http-surface#task#3 — expected 200, got 501; `$.Output` missing (expected {'__type': 'com.amazon.coral.service#UnknownOperationException'}); `$.Version` missing (expected '1.0').
2. ListUsers, dsdata-request-layer-ordering-and-http-surface#task#4 — expected 404, got 501; `$.UnknownOperationException` missing (expected '').
3. ListUsers, dsdata-sdk-unreachable-request-envelopes#task#3 — status 400/400 but `$headers.x-amzn-errortype` expected 'SerializationException:http://internal.amazon.com/coral/com.amazon.coral.service/', got 'ValidationException'; `$.Message` missing (expected 'STRING_VALUE can not be converted to an Integer').
4. ListUsers, dsdata-sdk-unreachable-request-envelopes#task#4 — expected 400, got 501; `$headers.x-amzn-errortype` expected 'ValidationException:http://directoryservicedata.amazonaws.com/doc/2023-05-31/', got 'NotImplemented'; `$.Message` missing (expected "1 validation error detected: Value null at 'directoryId' failed to satisfy constraint: Member must not be null").
5. ListGroups, dsdata-sdk-unreachable-request-envelopes#task#6 — status 400/400; body expected {} , got {'__type': 'ValidationException', 'message': 'json parse error in lookahead'}; errortype expected 'SerializationException:http://internal.amazon.com/coral/com.amazon.coral.service/', got 'ValidationException'.
Model grounding: services/service-com-amazonaws-directoryservicedata-directoryservicedata/smithy-model/com-amazonaws-directoryservicedata-directoryservicedata.json — DirectoryServiceData, restJson1, 17 bound operations (ListUsers POST /Users/ListUsers), ListUsersRequest.DirectoryId @required httpQuery, common error ValidationException (client, httpError 400). Component: services/service-com-amazonaws-directoryservicedata-directoryservicedata/src/service.rs lines 112-120 — `override_routing_error` returns None, documented as rendering the protocol default "rest -> 501", which matches the three observed 501s.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay dsdata-request-layer-ordering-and-http-surface#task#3 (request on the ds-data endpoint naming no resolvable operation) — emulator answers 501; expected 200 with body {"Output": {"__type": "com.amazon.coral.service#UnknownOperationException"}, "Version": "1.0"}.
4. Cause 2: replay dsdata-request-layer-ordering-and-http-surface#task#4 (unknown URI path) — emulator answers 501; expected 404 with an `UnknownOperationException` body key.
5. Cause 3: replay dsdata-sdk-unreachable-request-envelopes#task#3 (ListUsers with a string where an integer is modeled) — emulator answers 400 ValidationException with no Message; expected 400 with x-amzn-errortype SerializationException:http://internal.amazon.com/coral/com.amazon.coral.service/ and a coercion Message.
6. Cause 4: replay dsdata-sdk-unreachable-request-envelopes#task#4 (ListUsers with DirectoryId null/absent) — emulator answers 501 NotImplemented; expected 400 ValidationException with the "Member must not be null" Message naming 'directoryId'.
7. Cause 5: replay dsdata-sdk-unreachable-request-envelopes#task#6 (ListGroups with an unparseable JSON body) — emulator answers 400 ValidationException with body {"__type":"ValidationException","message":"json parse error in lookahead"}; expected 400 with errortype SerializationException:... and an empty {} body.
