Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewaymanagementapi.agwmgmt-malformed-connection-id-rejected apigatewaymanagementapi.agwmgmt-post-oversize-payload-rejected-before-lookup apigatewaymanagementapi.agwmgmt-undeployed-stage-endpoint-forbidden
Issue-Shape-Id: com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi
Issue-Key: apigatewaymanagementapi.agwmgmt-malformed-connection-id-rejected|_poll_until#3|Timeout|time.sleep(interval_s)
Issue-Key: apigatewaymanagementapi.agwmgmt-post-oversize-payload-rejected-before-lookup|_poll_until#3|Timeout|time.sleep(interval_s)
Issue-Key: apigatewaymanagementapi.agwmgmt-undeployed-stage-endpoint-forbidden|_poll_until#3|Timeout|time.sleep(interval_s)
Issue-Commit: d9cf74aafe764d4a8806a821a1189d14d75bb9bb

# ApiGatewayManagementApi @connections splits 400/410 on "did this emulator mint the id", so a well-formed unknown ConnectionId is refused 400 BadRequestException and GoneException (410) is unreachable — three checks hang to the 60 s cap with no verdict

> _Note: this issue was identified against the code base at commit `d9cf74aafe764d4a8806a821a1189d14d75bb9bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnection

## Description
ONE net-new root cause on the apigatewaymanagementapi service token, from the three failing checks the replay reports as neither ALREADY FILED nor OWNED. It must be closed before this issue is submitted.

=== Cause 1 of 1: the @connections 400-vs-410 refusal split is keyed on whether THIS emulator issued the ConnectionId, not on the id's form, so a well-formed id the emulator never minted is answered 400 BadRequestException instead of 410 GoneException and no request can ever reach the connection-store miss ===

AWS: the 400/410 split itself is recorded behavior only — the Smithy model (ApiGatewayManagementApi, restJson1/sigv4) declares NO BadRequestException at all; GetConnection GET /@connections/{ConnectionId}, PostToConnection POST /@connections/{ConnectionId} (Data @httpPayload blob @length 0..131072) and DeleteConnection DELETE /@connections/{ConnectionId} (code 204) each declare only ForbiddenException (@httpError 403, no members), GoneException (@httpError 410, no members, doc "The connection with the provided id no longer exists.") and LimitExceededException (@httpError 429); ConnectionId is an @httpLabel @required __string with no pattern or length. Recorded: a WELL-FORMED connection id of the shape API Gateway mints (base64 of 11 bytes) that no live connection owns reaches the connection store and is refused HTTP 410, error code GoneException, with an EMPTY message — that is the readiness signal a deployed stage's @connections route is serving. A value whose FORM is not one API Gateway mints (a single character, characters outside the base64 alphabet, unpadded base64, a 200-character id) is refused HTTP 400, error code BadRequestException, message "Invalid connectionId: <ConnectionId>" — fixed text "Invalid connectionId: " plus the ConnectionId exactly as the request sent it — before the store is consulted. The decision is therefore a property of the VALUE, not of the server's own mint log: AWS answers 410 for a well-formed id it did not mint (one from another API, one from a connection that closed before this server saw it, one a client kept across a restart).

Emulator: the split is made against a per-process set of ids the emulator itself issued. services/.../src/state.rs documents and implements the rule "a ConnectionId the service never ISSUED identifies no connection, so it is refused as an invalid ConnectionId (HTTP 400 BadRequestException, invalid_connection_id) before any connection is looked up" (state.issued: BTreeSet<String>, identifies_a_connection, invalid_connection_id_message -> "Invalid connectionId: {connection_id}"), and each handler consults it first (get_connection.rs: `if !issued { return Err(invalid_connection_id(...)) }`). Since ids are only ever added by register_connection — the internal seam a WebSocket data plane would call, and no upgrade ever completes on this emulator — the issued set is permanently empty, so EVERY @connections call on a freshly deployed stage, with any id, answers HTTP 400 BadRequestException "Invalid connectionId: <id>". The three checks each poll GetConnection with the well-formed unknown id "Fabc1cWIPHcCJ7A=" waiting for GoneException as their readiness condition, never get it, and are cut by the 60 s harness cap ("Check timed out after 60.0s"), so their actual subjects — the malformed-id refusal on all three operations, PostToConnection's payload gate ordering ahead of id validation and lookup, and the undeployed-stage ForbiddenException — reach no verdict at all.

Why: 410 GoneException is the documented signal that a peer went away and the one a fan-out application keys its "prune this row from my connection table" branch on; 400 BadRequestException means "your id is malformed, retrying is pointless". An emulator that answers 400 for every id it did not personally mint inverts that for the common cases a client actually hits — ids persisted in DynamoDB across a restart, ids minted by a sibling process, ids whose connection closed — so prune logic never fires on the emulator and fires in production, and the entire GoneException path (plus everything gated behind it, including every check that first proves a deployed stage is serving) is unobservable. Over-permissiveness is not the fault here: the emulator is wrong in both directions, refusing 400 where AWS says 410.

Fix: the shared @connections ConnectionId validation and the refusal split in front of the three handlers (the issued-set check in GetConnection/PostToConnection/DeleteConnection and the helpers in the service's state module). Decide 400 BadRequestException from the FORM of the submitted ConnectionId alone and let any well-formed id fall through to the connection store, whose miss is 410 GoneException with an empty body/message; do not let a value's absence from the emulator's own mint log produce a 400.

Related (does NOT cover these three checks): 20261029T192401Z-apigatewaymanagementapi-connections-malformed-connection-ids-oversize-payloads-and.md asked for the malformed-id 400 and carries older replay keys for these same check ids (assertions inside run()); the implementation that answered it introduced the issued-set rule and now blocks those checks before any assertion, and the replay reports no owner for them. 20261103T054456Z-apigatewaymanagementapi-the-deployed-websocket-stage-still-never-yields.md covers the missing 101 upgrade and the undeployed-stage 403 for other keys; 20261029T214905Z-apigatewaymanagementapi-connections-error-layer-every-failure-answers-410.md is the wire-corpus view of the opposite, now-stale symptom (everything answering 410).

Traces: apigatewaymanagementapi.agwmgmt-malformed-connection-id-rejected, apigatewaymanagementapi.agwmgmt-post-oversize-payload-rejected-before-lookup, apigatewaymanagementapi.agwmgmt-undeployed-stage-endpoint-forbidden.

## Evidence
Suite: simparity-behavioral-aws-v1. Totals: 0/47 passed (0.0%); apigatewaymanagementapi 0/28 (0%). The three checks below are the apigatewaymanagementapi failures the report marks neither ALREADY FILED nor OWNED.

[mismatch] apigatewaymanagementapi.agwmgmt-malformed-connection-id-rejected — Error: Check timed out after 60.0s
[mismatch] apigatewaymanagementapi.agwmgmt-post-oversize-payload-rejected-before-lookup — Error: Check timed out after 60.0s
[mismatch] apigatewaymanagementapi.agwmgmt-undeployed-stage-endpoint-forbidden — Error: Check timed out after 60.0s

All three complete their control-plane setup (apigatewayv2 create_api ProtocolType=WEBSOCKET, create_integration MOCK, create_route $connect, create_deployment, create_stage 'dev') and then block in the same readiness helper:

    def _wait_management_ready(mgmt):
        return _poll_until(
            lambda: _error_code(lambda: mgmt.get_connection(ConnectionId="Fabc1cWIPHcCJ7A="))[0],
            lambda code: code == "GoneException",
            timeout_s=120.0, interval_s=4.0,
            description="the deployed stage's @connections route reaching the connection store")

with the check's own note: "GoneException for a well-formed unknown id is the readiness condition. Before the stage has propagated the same call answers ForbiddenException, so this distinguishes 'not deployed yet' from the validation behaviour under test." The 120 s poll never converges and the 60 s harness cap ends the check, so none of the three reaches its first assertion.

Expected (real AWS), per operation:
- GetConnection/PostToConnection/DeleteConnection with ConnectionId="Fabc1cWIPHcCJ7A=" (well formed, base64 of 11 bytes, unknown): HTTP 410, Error.Code GoneException, Error.Message "" (memberless shape, no body members to fill).
- Same operations with a malformed id: HTTP 400, Error.Code BadRequestException, Error.Message "Invalid connectionId: <the request's ConnectionId>" (fixed prefix + the submitted value).

Actual (emulator): the well-formed unknown id is refused as an invalid ConnectionId, i.e. HTTP 400 / BadRequestException / "Invalid connectionId: Fabc1cWIPHcCJ7A=", for every poll of the whole budget. The code path is explicit in the checkout:
- services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/src/state.rs lines 11-22 (the documented split), 131-159 (`pub issued: BTreeSet<String>`, `identifies_a_connection`), 161-183 (`invalid_connection_id_message` -> format!("Invalid connectionId: {connection_id}"), `invalid_connection_id` -> Scaffolded::bad_request), 214-251 (`register_connection` is the ONLY writer of `issued`).
- services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/src/ops/get_connection.rs lines 155-172: `issued` is tested first; `!issued` -> invalid_connection_id (400), and GoneException is returned only for an id present in `issued` but absent from `connections`.
Because no WebSocket upgrade ever completes on this emulator (see the related open issues), `register_connection` is never called, `issued` is empty, and the GoneException branch is dead code for every externally supplied id.

That the same 400 fires for well-formed ids is corroborated by a sibling check in another service group: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request asserts ('BadRequestException', 'Invalid connectionId: AAAAAAAAAAAAAAA=') for an id of the same 11-byte-base64 shape and PASSES that assertion against this emulator before failing later — i.e. the emulator answers 400 regardless of form.

Model facts used (smithy_query, model_path services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json):
- operation GetConnection: GET /@connections/{ConnectionId}, code 200; errors ForbiddenException(403), GoneException(410), LimitExceededException(429); input ConnectionId __string @httpLabel @required (no pattern/length).
- operation PostToConnection: POST /@connections/{ConnectionId}; Data @httpPayload blob @length{min 0,max 131072}; errors +PayloadTooLargeException(413).
- operation DeleteConnection: DELETE /@connections/{ConnectionId}, code 204; same three errors.
- shape GoneException: client error, httpError 410, members {} — doc "The connection with the provided id no longer exists."
- shape ForbiddenException: client error, httpError 403, members {}.
- No BadRequestException shape exists in this model, so its wording is recorded behavior only.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, and replay the three apigatewaymanagementapi check ids in trace_ids from simparity-behavioral-aws-v1.
2. Create an apigatewayv2 API with ProtocolType=WEBSOCKET and RouteSelectionExpression=$request.body.action, a MOCK integration, a $connect route, a deployment and a stage 'dev'.
3. Build an apigatewaymanagementapi client whose endpoint_url is that API id + '/dev' (botocore Config(retries={'max_attempts': 1})).
4. Call get_connection(ConnectionId='Fabc1cWIPHcCJ7A=') — a well-formed id of the shape API Gateway mints that no connection owns. Observe HTTP 400 with Error.Code 'BadRequestException' and Error.Message 'Invalid connectionId: Fabc1cWIPHcCJ7A=' instead of HTTP 410 with Error.Code 'GoneException' and an empty message. Repeat with post_to_connection(ConnectionId=..., Data=b'hi') and delete_connection(ConnectionId=...): same 400.
5. Note that no amount of waiting changes the answer (the emulator's issued-id set is only written by its internal register_connection seam), so any check that first polls for GoneException as a readiness condition hangs until the harness cap and reaches no verdict.
6. After fixing, re-replay exactly the three check ids in trace_ids: the well-formed unknown id must answer 410 GoneException with an empty message, a malformed id (single character, non-base64 characters, unpadded, 200 characters) must answer 400 BadRequestException 'Invalid connectionId: <id>' on all three operations, and the checks must then proceed to their own assertions (payload-gate ordering, undeployed-stage 403).
