Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsecuretunneling.rotate-access-token-on-closed-tunnel-rejected iotsecuretunneling.rotate-with-destination-config-and-source-mode-rejected iotsecuretunneling.describe-tunnel-unknown-id-rejected iotsecuretunneling.close-tunnel-delete-flag-removes-tunnel-metadata iotsecuretunneling.tag-resource-unknown-arn-rejected
Issue-Shape-Id: com.amazonaws.iotsecuretunneling#IoTSecuredTunneling
Issue-Key: iotsecuretunneling.rotate-access-token-on-closed-tunnel-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: iotsecuretunneling.rotate-with-destination-config-and-source-mode-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: iotsecuretunneling.describe-tunnel-unknown-id-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iotsecuretunneling.close-tunnel-delete-flag-removes-tunnel-metadata|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iotsecuretunneling.tag-resource-unknown-arn-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# IoT Secure Tunneling: RotateTunnelAccessToken accepts requests AWS rejects, and not-found errors use a non-AWS message form

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateTunnelAccessToken

## Description
Three distinct root causes in the emulator's iotsecuretunneling implementation, all grounded in failing behavioral checks. No open issue covers any of them (list_existing_issues for 'iotsecuretunneling' is empty).

Cause 1 of 3: RotateTunnelAccessToken succeeds on a CLOSED tunnel.
AWS: rotation of the client access tokens is only possible while the tunnel record is live and OPEN; recorded behavior only (the Smithy model declares only ResourceNotFoundException for RotateTunnelAccessToken and no ValidationException, so the model says nothing about this case). The recording shows a rejection with error code 'ValidationException' for a tunnel that is in status CLOSED but still describable, with a message built from the request's tunnelId, e.g. 'Tunnel <tunnelId> is close.' (that exact text is an example of the recorded message; the requirement is the error code plus a message naming the tunnelId).
Emulator: after CloseTunnel(tunnelId) (no delete) and DescribeTunnel reporting status CLOSED, RotateTunnelAccessToken(tunnelId, clientMode=SOURCE) returns HTTP 200 with a fresh sourceAccessToken and tunnelArn instead of rejecting the call.
Why: a client that relies on AWS refusing to reissue credentials for a closed tunnel will keep handing out usable-looking access tokens for a tunnel that can never be connected to, and its error-handling path for a closed tunnel is never exercised. Worse, the emulator mints credentials for a dead resource, so negative tests of credential lifecycle pass against the emulator and fail against AWS.
Fix: the lifecycle precondition in the RotateTunnelAccessToken handler / its validation step — reject when the resolved tunnel's status is not OPEN, before any token is generated or stored.
Traces: iotsecuretunneling.rotate-access-token-on-closed-tunnel-rejected.

Cause 2 of 3: RotateTunnelAccessToken accepts destinationConfig together with clientMode=SOURCE.
AWS: the RotateTunnelAccessToken API documentation states that DestinationConfig cannot be used with source clientMode; the Smithy model shows RotateTunnelAccessTokenRequest with required tunnelId, required clientMode (enum ClientMode) and optional destinationConfig, and declares no ValidationException, so the pairing rule is documentation/recorded behavior only. The recording shows the request rejected with error code 'ValidationException' and a message naming the illegal combination, e.g. 'DestinationConfig is not allowed with clientMode SOURCE' (example text; the requirement is the error code and that the combination is refused).
Emulator: RotateTunnelAccessToken(tunnelId, clientMode=SOURCE, destinationConfig={services:[SSH]}) on an OPEN tunnel returns HTTP 200 with a rotated sourceAccessToken instead of rejecting.
Why: a client that sends the illegal combination gets a success and believes a destination configuration was applied by the rotate call; against real AWS the same call fails, so the emulator hides a client bug and silently diverges on what the tunnel's destinationConfig is afterwards.
Fix: input validation in the RotateTunnelAccessToken handler — cross-member check of clientMode against the presence of destinationConfig, performed before mutating tunnel state or issuing tokens.
Traces: iotsecuretunneling.rotate-with-destination-config-and-source-mode-rejected.

Cause 3 of 3: the tunnel-not-found error carries a non-AWS message form on every operation that raises it.
AWS: ResourceNotFoundException (Smithy: ResourceNotFoundException, client error, httpError 404) is modeled on DescribeTunnel, RotateTunnelAccessToken, CloseTunnel, TagResource and ListTagsForResource. Its message is built from the request: AWS names the TUNNEL by the tunnelId from the request (or, for the tag operations, the tunnel id component of the supplied resourceArn) in the form 'Tunnel <tunnelId> does not exist.' That construction rule — a message identifying the tunnel by its id, identical across read, mutate and tag paths — is the requirement; the recorded sentence is the example of it.
Emulator: the error code is correct (ResourceNotFoundException) but the message is produced by a different, path-specific rule: DescribeTunnel and RotateTunnelAccessToken on a never-created or deleted tunnelId return 'Tunnel <tunnelId> not found', and TagResource / ListTagsForResource on a well-formed but non-existent tunnel ARN return a generic resource form, 'Resource arn:aws:iot:<region>:<account>:tunnel/<tunnelId> not found', which names the ARN instead of the tunnel.
Why: clients and tooling that parse or assert on the not-found message (and tag-path consumers that expect the tunnel-scoped wording rather than a generic 'Resource <arn>' form) diverge from AWS; the tag operations additionally expose an inconsistent error vocabulary across operations of the same service.
Fix: the shared tunnel-not-found error construction used by the iotsecuretunneling operation handlers (describe/rotate/close and the tag/list-tags resource-ARN lookup) — build one tunnel-scoped message from the request's tunnelId, including on the ARN-addressed tag paths.
Traces: iotsecuretunneling.describe-tunnel-unknown-id-rejected, iotsecuretunneling.close-tunnel-delete-flag-removes-tunnel-metadata, iotsecuretunneling.tag-resource-unknown-arn-rejected.

## Evidence
Cause 1 — iotsecuretunneling.rotate-access-token-on-closed-tunnel-rejected:
  Expected: the call is rejected; code == 'ValidationException', message naming the closed tunnel (example: "Tunnel <id> is close.").
  Actual: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'tunnelArn': 'arn:aws:iot:us-west-2:000000000000:tunnel/1c6b48d1-...', 'sourceAccessToken': 'c5bf48f7...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} — this after DescribeTunnel had confirmed status == 'CLOSED'.

Cause 2 — iotsecuretunneling.rotate-with-destination-config-and-source-mode-rejected:
  Expected: the call is rejected; code == 'ValidationException' (example message: "DestinationConfig is not allowed with clientMode SOURCE").
  Actual: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'tunnelArn': 'arn:aws:iot:us-west-2:000000000000:tunnel/526638d0-...', 'sourceAccessToken': 'def52590...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.

Cause 3 — message form of ResourceNotFoundException (code matched in all three checks, message did not):
  describe-tunnel-unknown-id-rejected: expected 'Tunnel sim2real-nonexistent-tunnel-b9e81498c53f does not exist.', got 'Tunnel sim2real-nonexistent-tunnel-b9e81498c53f not found'.
  close-tunnel-delete-flag-removes-tunnel-metadata: expected 'Tunnel c24970ee-e2d0-48aa-b84f-63bddb9c6465 does not exist.', got 'Tunnel c24970ee-e2d0-48aa-b84f-63bddb9c6465 not found' (DescribeTunnel after CloseTunnel(delete=True); the delete semantics themselves were correct).
  tag-resource-unknown-arn-rejected: expected 'Tunnel nonexistent-tunnel-661c815074cb does not exist.', got 'Resource arn:aws:iot:us-west-2:000000000000:tunnel/nonexistent-tunnel-661c815074cb not found'.

Model evidence (services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/smithy-model/com-amazonaws-iotsecuretunneling-iotsecuredtunneling.json): RotateTunnelAccessToken input {tunnelId (required, pattern ^[a-zA-Z0-9_\-+=:]{1,128}$), clientMode (required, enum ClientMode), destinationConfig (optional)}; errors: ResourceNotFoundException (client, httpError 404). Same ResourceNotFoundException bound to DescribeTunnel, CloseTunnel, TagResource, ListTagsForResource. No ValidationException is modeled, hence 'recorded behavior only' for causes 1 and 2.

## Steps to Reproduce
Boot the emulator and point a botocore iotsecuretunneling client at it (region us-west-2).

Cause 1:
  1. t = client.open_tunnel(description='x'); tid = t['tunnelId']
  2. client.close_tunnel(tunnelId=tid)  # no delete flag
  3. poll client.describe_tunnel(tunnelId=tid)['tunnel']['status'] until 'CLOSED'
  4. client.rotate_tunnel_access_token(tunnelId=tid, clientMode='SOURCE')
     -> emulator: HTTP 200 with a new sourceAccessToken. Expected: ValidationException.

Cause 2:
  1. t = client.open_tunnel(description='x'); tid = t['tunnelId']  (tunnel is OPEN)
  2. client.rotate_tunnel_access_token(tunnelId=tid, clientMode='SOURCE', destinationConfig={'services': ['SSH']})
     -> emulator: HTTP 200 with a new sourceAccessToken. Expected: ValidationException.

Cause 3:
  1. client.describe_tunnel(tunnelId='sim2real-nonexistent-tunnel-abc') -> ResourceNotFoundException with message 'Tunnel ... not found' instead of 'Tunnel ... does not exist.'
  2. t = client.open_tunnel(description='x'); client.close_tunnel(tunnelId=t['tunnelId'], delete=True); then client.describe_tunnel(tunnelId=t['tunnelId']) and client.rotate_tunnel_access_token(tunnelId=t['tunnelId'], clientMode='SOURCE') -> same message divergence.
  3. bogus = t['tunnelArn'].rsplit('/',1)[0] + '/nonexistent-tunnel-abc'; client.tag_resource(resourceArn=bogus, tags=[{'key':'k','value':'v'}]) and client.list_tags_for_resource(resourceArn=bogus) -> ResourceNotFoundException with message 'Resource <arn> not found' instead of the tunnel-scoped form.

Replay: run_behavioral_suite and re-run the five checks listed in trace_ids.
