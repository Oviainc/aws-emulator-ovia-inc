Issue-Source: trace-sweep-divergence
Issue-Trace: route53-traffic-policy-lifecycle-and-list
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-traffic-policy-lifecycle-and-list
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|GetTrafficPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 7c73ea6d7a96c997ecb38a88ea3442295b1a24d6

# Route 53 NoSuchTrafficPolicy refusal is serialized without an Error.Message element

> _Note: this issue was identified against the code base at commit `7c73ea6d7a96c997ecb38a88ea3442295b1a24d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrafficPolicy

## Description
One cause, found by replaying the wire fixture corpus simparity-wire-aws-v1 against the emulator.

Cause 1 of 1: the modeled NoSuchTrafficPolicy refusal is emitted with no Message child, where Route 53 sends a fixed sentence.

AWS: Smithy (services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json) models GetTrafficPolicy with input members Id (TrafficPolicyId, @httpLabel, required, @length 1..36) and Version (TrafficPolicyVersion, @httpLabel, required, @range 1..1000), http GET /2013-04-01/trafficpolicy/{Id}/{Version}, and binds the client errors InvalidInput (httpError 400) and NoSuchTrafficPolicy (httpError 404). NoSuchTrafficPolicy is a structure with a single member `message` (target ErrorMessage); its shape doc is "No traffic policy exists with the specified ID.", which states the RULE, not the wire sentence. The recorded response supplies the wire sentence: HTTP 404, ErrorResponse/Error with Message = "The policy does not exist." That message is entirely FIXED text for this error class -- it has no variable part at all: it never echoes the policy Id, the Version, or the operation name, so nothing in it is taken from the request or from emulator state.

Emulator: the HTTP status (404) and the error Code are right, but the Error element carries no Message child at all -- the replay reports `$.ErrorResponse.Error.Message: missing (expected 'The policy does not exist.', got None)`. The omission is deliberate in the source: the shared constructor `no_such_traffic_policy()` in services/service-com-amazonaws-route53-awsdnsv20130401/src/state.rs builds `NoSuchTrafficPolicy { message: None }` and its doc comment declares the shape "MESSAGELESS" on the stated premise that Route 53 reports this refusal "as the error's identity alone". The recording contradicts that premise: Route 53 does send a Message for this error class.

Why: the Message is the only human-readable part of a refusal. A client that logs or surfaces Error.Message gets an empty reason for a 404 that AWS explains, and any caller that branches on, asserts on, or string-matches the fixed sentence "The policy does not exist." diverges even though the status and code are correct -- so not-found handling written against AWS breaks against the emulator. Because the refusal is minted in one shared place, every operation that raises NoSuchTrafficPolicy (GetTrafficPolicy, UpdateTrafficPolicyComment, CreateTrafficPolicyVersion, DeleteTrafficPolicy, ListTrafficPolicyVersions) is messageless, not just the replayed one.

Fix: belongs in the service's shared refusal constructor / state module rather than in the operation handler, so all five bound operations word the refusal identically -- `no_such_traffic_policy()` must populate `message` with AWS's fixed sentence and its MESSAGELESS doc premise must be corrected. The sibling `no_such_traffic_policy_instance()` rests on the same premise and should be re-checked against a recording, but only the traffic-policy path is grounded by this replay.

Related but NOT covering this: open issue 20261109T063005Z-route53-route-53-nosuchtrafficpolicy-is-answered-with-an-empty.md describes the same empty-Message defect from the BEHAVIORAL corpus (simparity-behavioral-aws-v1) and carries only behavioral check keys -- it holds no wire replay key, so this wire fixture step is not covered by it and the fix for this issue must be confirmed by re-replaying the fixture named below. Also related but different: 20261029T220729Z-route53-route-53-refusal-messages-are-emulator-authored-instead.md covers CreateTrafficPolicy's InvalidTrafficPolicyDocument clause wording, a different error shape.

Traces: route53-traffic-policy-lifecycle-and-list#task#4 (GetTrafficPolicy for a traffic policy version the store does not hold). Source of the omission: services/service-com-amazonaws-route53-awsdnsv20130401/src/state.rs around line 2886 (`no_such_traffic_policy`), reached from services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/get_traffic_policy.rs.

## Evidence
Wire corpus simparity-wire-aws-v1, fixture route53-traffic-policy-lifecycle-and-list, step task#4, operation GetTrafficPolicy, verdict mismatch (cluster size 1).

Expected status: 404. Actual status: 404 (status and error Code agree).
Divergence detail, verbatim from the replay:
  $.ErrorResponse.Error.Message: missing (expected 'The policy does not exist.', got None)

Expected (AWS): HTTP 404, ErrorResponse/Error/Code = NoSuchTrafficPolicy, ErrorResponse/Error/Message = "The policy does not exist."  -- the whole sentence is fixed text; there are no request- or state-derived values in it.
Actual (emulator): HTTP 404, the same error Code, and NO Message element serialized inside Error (botocore normalizes the absent child to the empty string).

Model evidence: NoSuchTrafficPolicy = structure, errorKind client, httpError 404, members { message: ErrorMessage }; bound as an error of GetTrafficPolicy alongside InvalidInput (400).
Code evidence: state.rs `pub fn no_such_traffic_policy() -> NoSuchTrafficPolicy { NoSuchTrafficPolicy { message: None } }`, preceded by a doc comment asserting the error is "MESSAGELESS" and that emitting the documentation text "would put an element on the wire the service never sends" -- the recording shows the service does send it.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay the wire fixture route53-traffic-policy-lifecycle-and-list and inspect step task#4 (GetTrafficPolicy against a traffic policy version that does not exist).
3. Or by hand, with a route53 client pointed at the emulator:
   a. create_traffic_policy(Name=<unique>, Document='{"AWSPolicyFormatVersion":"2015-10-01","RecordType":"A","Endpoints":{"e1":{"Type":"value","Value":"192.0.2.1"}},"StartEndpoint":"e1"}') and note Id and Version.
   b. get_traffic_policy(Id=<id>, Version=<Version>+99) and inspect the raised ClientError's raw XML body and response['Error'].
4. Observe HTTP 404 with Code=NoSuchTrafficPolicy but no <Message> element under <Error> (Error['Message'] == ''), where AWS returns <Message>The policy does not exist.</Message>.
