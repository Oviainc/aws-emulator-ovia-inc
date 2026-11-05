Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-ad-configuration-error-paths
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-ad-configuration-error-paths ivs-recording-configuration-error-paths
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|CreateAdConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivs|GetAdConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivs|DeleteAdConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivs|GetRecordingConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivs|DeleteRecordingConfiguration|4xx->4xx|$.RequestID
Issue-Commit: 4c560b92ea61305d4a9f12f94c4898dc6d4f1637

# IVS error-envelope RequestID is a UUID instead of an AWS request-id token

> _Note: this issue was identified against the code base at commit `4c560b92ea61305d4a9f12f94c4898dc6d4f1637`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAdConfiguration

## Description
Cause 1 of 1: the request identifier the IVS error envelope carries has the wrong shape, on every refusal the corpus exercises. The five replay clusters bundled here (CreateAdConfiguration 400, GetAdConfiguration 404, DeleteAdConfiguration 404, GetRecordingConfiguration 404, DeleteRecordingConfiguration 404) differ from AWS in nothing but this one field: in each of them the HTTP status, the $.Code value and the $.Message text all matched, and $.RequestID was the only diverging key. That makes it one defect in the shared id generator, not five per-operation defects.

AWS: recorded behavior only - the Smithy model does not model a RequestID member in the IVS error envelope (com.amazonaws.ivs#ValidationException and com.amazonaws.ivs#ResourceNotFoundException model only exceptionMessage plus the httpHeader-bound CORS/security members and x-amzn-ErrorType), so the envelope's RequestID is known from the recordings alone. Every recorded IVS refusal body carries a RequestID that is a fixed-width 16-character base64-style token: recorded examples are 'DzhHOENgvHcEUjA=' (CreateAdConfiguration 400 ValidationException), 'DzhHOGYAPHcEoEw=' (GetAdConfiguration 404 ResourceNotFoundException), 'DzhHQE79vHcEIFA=' (DeleteAdConfiguration 404), 'D0UwTHEvvHcEGYA=' (GetRecordingConfiguration 404) and 'D0UwUEdqPHcEBjQ=' (DeleteRecordingConfiguration 404). In the error message itself the fixed text is the error class's sentence (e.g. ResourceNotFoundException's "Request references a resource which does not exist.", or for the ad-configuration path the recorded "Resource: mediaTailorPlaybackConfiguration '<name>' not found" whose only variable part is the requested name) - the RequestID is not part of the message, it is a sibling member of the envelope. The VALUE of a request id is per-request and no client may depend on a particular one; its SHAPE is the wire contract, which is why other steps of the same corpus assert it as '<shaped-not-asserted>' rather than as a literal.

Emulator: it emits a lowercase RFC-4122 UUID in $.RequestID on every refusal - '0d76b681-ba4c-40b8-beef-975d24c75fde', '2ecb7ff2-338c-4827-b24c-18da0aec1c71', 'ca27a22e-b3a7-4968-82f0-0179de748e8d', '85b54f26-1995-4096-8ff7-83ed381e7b03', '579d8be6-59d7-432c-98de-a048cc433d3e' in the five clusters - i.e. a 36-character hyphenated UUID where AWS returns a 16-character base64-style token. The divergence is structural and reproducible in every environment, not a per-run value difference: no run of AWS ever produces a 36-character hyphenated id here, and no run of the emulator ever produces a 16-character token.

Why: client, proxy and support tooling that parses, length-checks, logs or quotes an IVS request id mis-handles a 36-character UUID where the service contract is a 16-character token; and traffic captured against the emulator is distinguishable from AWS by the id shape alone, so recordings made through it cannot be replayed as AWS recordings.

Fix: the shared response-id generator in the IVS crate's error-response plumbing (the component that fills the error envelope's RequestID and the paired x-amzn-RequestId header), not any single operation handler - every bundled cluster is a different operation reaching the same generator. It should mint an AWS-shaped 16-character base64-style token.

Traces: ivs-ad-configuration-error-paths#task#1 (representative) plus ivs-ad-configuration-error-paths#task#2, ivs-ad-configuration-error-paths#task#4, ivs-recording-configuration-error-paths#task#2 and ivs-recording-configuration-error-paths#task#3 - each is a distinct replay cluster whose ONLY divergence is the request-id shape. Note: the other IVS clusters in this replay that also touch these fixtures (CreateAdConfiguration admitting a request AWS refuses, ListTagsForResource, TagResource/UntagResource 204 framing, failoverSlateConfigurationArn on channel reads, CreateChannel srt block) are already owned by 20261029T172005Z-ivs-tagging-and-ad-recording-configuration-admission-diverge.md, 20261102T071721Z-ivs-channel-projections-still-omit-failoverslateconfigurationarn-on-reads.md and 20261103T113752Z-ivs-createchannel-srt-block-mints-an-endpoint-and.md and are NOT part of this issue; none of those issues covers the request-id shape. The same defect was filed for S3 as 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md, so the IVS generator needs its own fix.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (score 65.5%, 24 mismatched steps).

Representative cluster - ivs-ad-configuration-error-paths#task#1, CreateAdConfiguration, expected 400 / actual 400, divergence key service=ivs|CreateAdConfiguration|status=400->400|keys=$.RequestID:
  $.RequestID: value_mismatch (expected 'DzhHOENgvHcEUjA=', got '0d76b681-ba4c-40b8-beef-975d24c75fde')
Status, $.Code and $.Message matched; $.RequestID was the only key reported.

The four other bundled clusters, same single key:
- ivs-ad-configuration-error-paths#task#2, GetAdConfiguration, 404 -> 404: expected 'DzhHOGYAPHcEoEw=', got '2ecb7ff2-338c-4827-b24c-18da0aec1c71'.
- ivs-ad-configuration-error-paths#task#4, DeleteAdConfiguration, 404 -> 404: expected 'DzhHQE79vHcEIFA=', got 'ca27a22e-b3a7-4968-82f0-0179de748e8d'.
- ivs-recording-configuration-error-paths#task#2, GetRecordingConfiguration, 404 -> 404: expected 'D0UwTHEvvHcEGYA=', got '85b54f26-1995-4096-8ff7-83ed381e7b03'.
- ivs-recording-configuration-error-paths#task#3, DeleteRecordingConfiguration, 404 -> 404: expected 'D0UwUEdqPHcEBjQ=', got '579d8be6-59d7-432c-98de-a048cc433d3e'.

Contrast showing the shape is the requirement and not the literal: elsewhere in the same corpus the comparator records the IVS RequestID as '<shaped-not-asserted>' (e.g. ivs-ad-configuration-lifecycle-attempt#task#1 expects '$.RequestID: <shaped-not-asserted>'), i.e. a value of the recorded 16-character token shape is accepted and only a differently-shaped id fails. Expected tokens are uniformly 16 characters of base64 alphabet ending in '='; actual ids are uniformly 36-character hyphenated lowercase UUIDs.

Smithy (services/service-com-amazonaws-ivs-amazoninteractivevideoservice): ValidationException @error client httpError 400 and ResourceNotFoundException @error client httpError 404 both model exceptionMessage plus httpHeader-bound members only - no RequestID member - so the envelope's request id is grounded in the recordings.

## Steps to Reproduce
1. Build the emulator from this checkout and replay fixture ivs-ad-configuration-error-paths from simparity-wire-aws-v1.
2. At step task#1, POST /CreateAdConfiguration with a mediaTailorPlaybackConfigurations entry that does not resolve, so the service refuses. Inspect the 400 ValidationException body's RequestID member (and the paired x-amzn-RequestId response header).
3. Observe a 36-character hyphenated lowercase UUID (e.g. '0d76b681-ba4c-40b8-beef-975d24c75fde') where AWS returns a 16-character base64-style token (recorded 'DzhHOENgvHcEUjA='). Status 400, Code 'ValidationException' and the message text match.
4. Repeat for task#2 (GetAdConfiguration, 404) and task#4 (DeleteAdConfiguration, 404) of the same fixture, and for task#2 (GetRecordingConfiguration, 404) and task#3 (DeleteRecordingConfiguration, 404) of ivs-recording-configuration-error-paths: every refusal shows the same UUID-shaped id, confirming one shared generator rather than per-operation behaviour.
5. After fixing the generator, re-replay all five fixture#step bindings listed in trace_ids plus the representative.
