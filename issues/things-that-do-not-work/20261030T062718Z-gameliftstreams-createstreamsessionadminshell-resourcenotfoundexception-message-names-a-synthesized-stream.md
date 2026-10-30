Issue-Source: trace-sweep-divergence
Issue-Trace: gameliftstreams-session-and-url-error-paths
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gameliftstreams-session-and-url-error-paths
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|CreateStreamSessionAdminShell|4xx->4xx|$.Message
Issue-Commit: cb1771c3c2a1df51218899fe99979bb9da68f0b3

# gameliftstreams CreateStreamSessionAdminShell ResourceNotFoundException message names a synthesized stream-session ARN instead of the missing resource

> _Note: this issue was identified against the code base at commit `cb1771c3c2a1df51218899fe99979bb9da68f0b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStreamSessionAdminShell

## Description
One cause.

Cause 1 of 1: the 404 ResourceNotFoundException body served by CreateStreamSessionAdminShell for a non-existent resource carries the wrong message — wrong fixed text and the wrong resource identity.

AWS: Smithy model (services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json) — CreateStreamSessionAdminShell is POST /streamgroups/{Identifier}/streamsessions/{StreamSessionIdentifier}/access; both path labels are @required Identifier (length 1..128, pattern '^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^:]*):([0-9]{12}):([^:]*)$)$'); Identifier is documented as "the stream group that runs this stream session" and StreamSessionIdentifier as the stream session. The operation models ResourceNotFoundException (errorKind client, httpError 404, required member Message documented as "Description of the error"), whose own doc is "The resource specified in the request was not found. Correct the request before you try again." The model does not dictate the message text, so the message is recorded behavior only: AWS answers 404 ResourceNotFoundException with Message of the form
  Resource '<arn-of-the-resource-that-was-not-found>' does not exist (Internal ID: <internal-id>)
Fixed text, exactly: "Resource '", then "' does not exist (Internal ID: ", then ")". Variable parts: the ARN is the resource AWS actually failed to resolve — here the stream group named by the {Identifier} path label, rendered as its stream-group ARN (arn:aws:gameliftstreams:<region>:<account>:streamgroup/<groupId>); the internal id is a per-run server-generated correlation id and is NOT part of the contract. Resolution is outside-in: the stream group must resolve before the stream session is looked up, so a request whose {Identifier} group does not exist is reported against the GROUP, not against a session under it.

Emulator: answers the same 404 with the correct error code but a message of the form "Stream session '<arn:aws:gameliftstreams:<region>:<account>:streamsession/<groupId>/<sessionId>>' does not exist (Internal ID: <internal-id>)". Two divergences from AWS: (a) the fixed text is "Stream session '...' does not exist" instead of "Resource '...' does not exist"; (b) the identity reported is a stream-session ARN the emulator composed from the two path labels, i.e. it skips resolving the stream group entirely and reports the un-resolvable child, so the message blames a resource the client never named directly. In the replayed step the only diverging field was $.Message (expected status 404, actual 404); the recorded expected value named the streamgroup ARN while the actual named a streamsession ARN under that same group id.

Why: the message is the only place AWS says WHICH resource was missing, and clients and operators branch on it. A caller that gets "Resource '<...streamgroup/sg-...>' does not exist" knows to create or fix the stream group; against the emulator it is told the stream SESSION is missing, so it retries session creation against a group that will never exist, or reports a misleading failure to its user. Tooling and tests that match the documented "Resource '...' does not exist" wording (or parse the ARN out of it to identify the failed resource type) do not match the emulator's text at all.

Fix: the CreateStreamSessionAdminShell handler's not-found path — it must resolve {Identifier} (the stream group) first and, when that fails, raise ResourceNotFoundException with the generic "Resource '<arn>' does not exist (Internal ID: <id>)" template filled from the request's own identifier expanded to the stream-group ARN; only a request whose group resolves but whose StreamSessionIdentifier does not should report the session. The shared ResourceNotFoundException message builder for this service is the right place for the fixed text.

Traces: gameliftstreams-session-and-url-error-paths#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture gameliftstreams-session-and-url-error-paths, step task#9, operation CreateStreamSessionAdminShell, verdict mismatch.

- Status: expected 404, actual 404 (status and error class agree).
- $.Message: value_mismatch.
  expected: "Resource 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-1AB2C3De9' does not exist (Internal ID: af760f6e-b3a4-4696-974c-6b20a2586f99)"
  actual:   "Stream session 'arn:aws:gameliftstreams:us-west-2:000000000000:streamsession/sg-1AB2C3De9/ABC123def4568' does not exist (Internal ID: 5aeba704-bb4b-20a5-3ade-f64be50e2056)"

Which parts of that message are requirements: the fixed text "Resource '" ... "' does not exist (Internal ID: " ... ")" is required verbatim; the ARN is a value derived from the request's {Identifier} path label plus the emulator's region/account and the resource type of the resource that failed to resolve (streamgroup here); the region, account id, group id (sg-1AB2C3De9) and session id (ABC123def4568) shown above are this recording's request values, and the Internal ID (af760f6e-... vs 5aeba704-...) is a per-run correlation id and is explicitly NOT part of the requirement — the divergence filed here is the fixed text and the resource type/identity in the ARN, not the differing internal ids.

Model evidence (smithy_query): CreateStreamSessionAdminShell POST /streamgroups/{Identifier}/streamsessions/{StreamSessionIdentifier}/access, errors include ResourceNotFoundException (client, httpError 404); ResourceNotFoundException.Message is @required String, doc "Description of the error"; shape doc "The resource specified in the request was not found. Correct the request before you try again."

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay fixture gameliftstreams-session-and-url-error-paths from trace set simparity-wire-aws-v1.
3. Step task#9 issues POST /streamgroups/{Identifier}/streamsessions/{StreamSessionIdentifier}/access where the stream group named by {Identifier} does not exist.
4. Observe the 404 ResourceNotFoundException body: $.Message reads "Stream session '<streamsession ARN composed from both path labels>' does not exist (Internal ID: ...)" where AWS answers "Resource '<streamgroup ARN for {Identifier}>' does not exist (Internal ID: ...)".
