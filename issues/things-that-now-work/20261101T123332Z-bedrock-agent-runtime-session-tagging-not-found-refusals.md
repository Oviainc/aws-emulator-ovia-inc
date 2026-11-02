Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-runtime-session-delete-then-operations-fail
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-runtime-session-delete-then-operations-fail
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|TagResource|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|ListTagsForResource|4xx->4xx|$.message
Issue-Commit: d0b7c7f06678596bb74c7aa80db04e65e20adb2f

# bedrock-agent-runtime session-tagging not-found refusals use generic cross-service wording and echo the ARN

> _Note: this issue was identified against the code base at commit `d0b7c7f06678596bb74c7aa80db04e65e20adb2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
TWO net-new root causes, both reached by replaying bedrock-agent-runtime wire fixtures from simparity-wire-aws-v1. They share one fault: the two session-tagging operations resolve a well-formed session ARN that names no existing session, correctly answer HTTP 404 ResourceNotFoundException, but serve a generic cross-service refusal message that also echoes the whole request ARN, instead of the bedrock-agent-runtime session-specific fixed text. Smithy model consulted: services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json.

Relationship to already-open issues: the open issue 20261029T214059Z-bedrock-agent-runtime-answers-successful-and-failed-session.md narrates overlapping wording faults as its Causes 4 and 5, but its front-matter Issue-Key set carries ONLY bedrock-agent-runtime|ListTagsForResource|2xx->2xx|$ for the tagging surface — it carries NEITHER bedrock-agent-runtime|TagResource|4xx->4xx|$.message NOR bedrock-agent-runtime|ListTagsForResource|4xx->4xx|$.message. The replay therefore reports both steps below as unowned (owned_by: []), so neither 404 refusal is gated by any open issue and a fix closing that issue would never re-run them. The open behavioral issue 20261029T171944Z-bedrock-agent-runtime-ten-net-new-causes-session.md likewise carries no wire replay key for these steps.

=== Cause 1 of 2: TagResource's not-found refusal is generic and echoes the request ARN ===
AWS: Smithy: TagResource is POST /tags/{resourceArn}; resourceArn targets TaggableResourcesArn (@httpLabel, required, @length 20..1011, @pattern "(^arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session)/[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$)" — so the only taggable resource kind is a session), and the operation models ResourceNotFoundException with @httpError 404. The recorded response for a syntactically valid session ARN whose session does not exist (for example after DeleteSession) is HTTP 404 with message "Session resource not found." — recorded behavior only for the message text, and that text is ENTIRELY fixed, trailing period included: there is no variable part, the ARN is NOT echoed, and the resource kind is named in words rather than by identifier.
Emulator: HTTP 404 (status matches) with message "Resource <resourceArn> was not found. Check the Amazon Resource Name (ARN) and try your request again." — different fixed text borrowed from another service family, naming no resource kind, and interpolating the full request ARN as a variable part where AWS has none.
Why: a client that distinguishes a deleted/unknown session from other 404s by matching the refusal text never matches, so retry and cleanup branches keyed on the message do not fire; and the emulator reflects the complete request ARN (account id, region, session uuid) back into an error body AWS deliberately keeps identifier-free, which is a disclosure difference a caller can come to depend on.
Fix: the TagResource handler's resource-resolution / not-found branch in the bedrock-agent-runtime service crate — raise the session-specific fixed-text ResourceNotFoundException rather than a shared generic ARN-echoing not-found constructor.
Traces: bedrock-agent-runtime-session-delete-then-operations-fail#task#4.

=== Cause 2 of 2: ListTagsForResource's not-found refusal carries the same generic wording on the read path ===
AWS: Smithy: ListTagsForResource is GET /tags/{resourceArn}, @readonly, with the same required TaggableResourcesArn @httpLabel member, and models ResourceNotFoundException with @httpError 404. The recorded response for a session ARN that names no existing session is HTTP 404 with the same entirely fixed message "Session resource not found." (recorded behavior only; no variable parts, trailing period included).
Emulator: HTTP 404 (status matches) with the same generic "Resource <resourceArn> was not found. Check the Amazon Resource Name (ARN) and try your request again."
Why: as Cause 1, on the read path. It is listed separately because it is a separate handler and a separate replay key: correcting only TagResource leaves tag reads against a deleted session answering the wrong message, and the replay step for this operation would still diverge.
Fix: the ListTagsForResource handler's resource-resolution / not-found branch in the bedrock-agent-runtime service crate — the same session-specific fixed-text refusal.
Traces: bedrock-agent-runtime-session-delete-then-operations-fail#task#5.

## Evidence
Both causes returned the EXPECTED status (404); the divergence is entirely the response body's message. The ARN below is a per-run value produced by the emulator's own state and is not part of any requirement — the requirement is that AWS emits no identifier at all in this message.

Cause 1 — fixture bedrock-agent-runtime-session-delete-then-operations-fail, step task#4, operation TagResource, verdict mismatch, expected_status 404 / actual_status 404:
  $.message: value_mismatch — expected 'Session resource not found.', got 'Resource arn:aws:bedrock:us-west-2:000000000000:session/33888853-9c21-4a6e-8892-d33613d88a5b was not found. Check the Amazon Resource Name (ARN) and try your request again.'
  Fixed text required: the whole string 'Session resource not found.' Variable parts required: none.
  Fixed text served: 'Resource ' + ' was not found. Check the Amazon Resource Name (ARN) and try your request again.'; the interpolated session ARN is taken from the request/state and is a variable part AWS's message does not have.

Cause 2 — fixture bedrock-agent-runtime-session-delete-then-operations-fail, step task#5, operation ListTagsForResource, verdict mismatch, expected_status 404 / actual_status 404:
  $.message: value_mismatch — expected 'Session resource not found.', got 'Resource arn:aws:bedrock:us-west-2:000000000000:session/33888853-9c21-4a6e-8892-d33613d88a5b was not found. Check the Amazon Resource Name (ARN) and try your request again.' (same expected/actual pair as Cause 1).

Neither cluster carried a stub_path, and both carried owned_by: [] in the replay report, i.e. no open issue holds their replay keys.

## Steps to Reproduce
Replay the wire corpus simparity-wire-aws-v1 against the emulator; both causes live in the fixture bedrock-agent-runtime-session-delete-then-operations-fail.

Cause 1 (bedrock-agent-runtime-session-delete-then-operations-fail#task#4): CreateSession to obtain a session identifier and its ARN; DeleteSession on it; then POST /tags/<the url-encoded session ARN> with a non-empty tags map. Expect HTTP 404 ResourceNotFoundException with the body message exactly 'Session resource not found.' The emulator answers 404 with 'Resource <session ARN> was not found. Check the Amazon Resource Name (ARN) and try your request again.'

Cause 2 (bedrock-agent-runtime-session-delete-then-operations-fail#task#5): with the same deleted session, GET /tags/<the url-encoded session ARN>. Expect HTTP 404 ResourceNotFoundException with the body message exactly 'Session resource not found.' The emulator answers 404 with the same generic ARN-echoing message.

Both steps must be re-replayed to confirm a fix: the two refusals come from two different handlers.
