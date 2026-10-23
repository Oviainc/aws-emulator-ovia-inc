Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect
Issue-Key: apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect|run#6|AssertionError|assert endpoint.startswith(("wss://", "ws://")), f"a WEBSOCKET ApiEndpoint must be ws(s)://, got {endpoint!r}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# apigatewayv2 CreateApi: a WEBSOCKET API's ApiEndpoint is returned with the https:// scheme instead of wss:// (1 cause)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
One net-new root cause in the apigatewayv2 emulator, grounded in the single apigatewayv2 check that got past CreateApi rather than dying on a scaffold stub.

Cause 1 of 1: CreateApi serializes the same https:// endpoint for every protocol type, ignoring ProtocolType=WEBSOCKET.
AWS: CreateApi with ProtocolType=WEBSOCKET returns an ApiEndpoint whose scheme is the WebSocket scheme — wss://{api-id}.execute-api.{region}.amazonaws.com — because that value is what a client hands to a WebSocket client library to open the connection; an HTTP API's ApiEndpoint is https://. Smithy (apigatewayv2): CreateApi -> CreateApiResponse$ApiEndpoint (target __string, jsonName apiEndpoint), documented as 'The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage.' The model constrains the host form but not the scheme, so for the scheme this is recorded behavior only: the check asserts ApiEndpoint.startswith(('wss://','ws://')) for ProtocolType=WEBSOCKET.
Emulator: ApiEndpoint comes back with an https:// scheme for a WEBSOCKET API — recorded as 'https://8sh3tqaaaa.execute-api.us-west-2.amazonaws.com' (the api id and host are environment-specific; the scheme is the defect). The check fails with AssertionError: a WEBSOCKET ApiEndpoint must be ws(s)://.
Why: a client that builds its connect URL from the CreateApi/GetApi response — the documented way to learn it — gets an https:// URL and either refuses to open a WebSocket or performs a plain HTTP request instead of an RFC 6455 upgrade. The protocol type is then not observable from the API representation at all, so generic tooling cannot tell a WebSocket API from an HTTP one.
Fix: in the apigatewayv2 crate, make the ApiEndpoint value the CreateApi/GetApi/GetApis/UpdateApi serializer emits depend on the API's ProtocolType (wss:// for WEBSOCKET, https:// for HTTP), rather than hardcoding https://.
Traces: apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect (the suite's only non-stub apigatewayv2 failure; the other two apigatewayv2 failures in this run are marked ALREADY FILED against the CreateRouteResponse and CreateStage stubs and are not part of this issue).

## Evidence
Suite report, [mismatch] apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect:

  File "check_apigatewayv2_websocket_post_to_connection_delivers_then_gone_after_disconnect.py", line 61, in run
    assert endpoint.startswith(("wss://", "ws://")), f"a WEBSOCKET ApiEndpoint must be ws(s)://, got {endpoint!r}"
  AssertionError: a WEBSOCKET ApiEndpoint must be ws(s)://, got 'https://8sh3tqaaaa.execute-api.us-west-2.amazonaws.com'

Expected: CreateApi(ProtocolType='WEBSOCKET') -> ApiEndpoint with a wss:// (or ws://) scheme on the {api-id}.execute-api.{region}.amazonaws.com host. Actual: an https:// scheme.

## Steps to Reproduce
aws apigatewayv2 create-api --name t --protocol-type WEBSOCKET --route-selection-expression '$request.body.action'
Inspect ApiEndpoint in the 201 response (and in a following get-api): the emulator returns 'https://<api-id>.execute-api.<region>.amazonaws.com'; AWS returns 'wss://<api-id>.execute-api.<region>.amazonaws.com'.
