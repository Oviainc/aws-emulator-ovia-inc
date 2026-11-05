Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code s3control-outposts-bucket-mutations-reject-a-plain-bucket-name
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|PutAccessPointPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|PutAccessPointPolicy|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|PutAccessPointPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|PutAccessPointPolicy|4xx->2xx|$
Issue-Key: s3control|PutAccessPointPolicy|4xx->2xx|status
Issue-Key: s3control|GetAccessPointPolicy|2xx->2xx|$.GetAccessPointPolicyResult.Policy
Issue-Key: s3control|PutBucketReplication|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|PutBucketReplication|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|PutBucketReplication|4xx->4xx|$.ErrorResponse.Error.URI
Issue-Key: s3control|PutBucketReplication|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|PutBucketReplication|4xx->4xx|$.ErrorResponse.RequestId
Issue-Related: 20261029T192817Z-s3control-ignores-the-accountid-host-prefix-so-cross.md
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control access point policy validation accepts malformed policies and reformats stored documents

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccessPointPolicy

## Description
5 net-new root causes: the access-point policy path neither refuses what AWS refuses nor returns what it stored, and two bucket-scoped operations miss their refusals. Smithy declares no errors for these operations (errors: []); every code and message below is recorded behavior only.

--- Cause 1 of 5: the non-JSON policy refusal uses the wrong message ---
AWS: recorded behavior only — a policy document that is not JSON is refused with HTTP 400 and Message exactly `Policies must be valid JSON and the first byte must be '{'` (fixed text; nothing from the request is interpolated), plus RequestId/HostId in the envelope.
Emulator: answers 400 with `The specified policy is not a policy this service can store: it is not a well-formed JSON document` and omits RequestId/HostId.
Why: the message is not AWS's, so a client surfacing or matching it reports a different cause, and there is no request id to correlate.
Fix: the PutAccessPointPolicy validation's refusal message plus the shared s3control envelope serializer.
Traces: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#4 (2 steps in cluster).

--- Cause 2 of 5: a syntactically valid policy with an unknown field is accepted ---
AWS: recorded behavior only — a policy document carrying a field the policy grammar does not define is refused with HTTP 400, Code `MalformedPolicy` and Message `Unknown field <fieldName>` — fixed text `Unknown field ` plus the offending field name taken from the submitted document — with RequestId/HostId.
Emulator: answers 200 with an empty body and stores the policy.
Why: a client gets confirmation that a policy AWS would reject is in effect; the access point's effective permissions then differ between the emulator and AWS, and policy-linting flows pass where they should fail.
Fix: PutAccessPointPolicy policy-document validation — validate the document against the policy grammar and refuse unknown fields with MalformedPolicy.
Traces: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#6 (7 steps in cluster).

--- Cause 3 of 5: GetAccessPointPolicy returns a re-serialized document, not the stored bytes ---
AWS: recorded behavior only — the policy is returned exactly as it was put: the same bytes, the same (compact) formatting.
Emulator: returns the document pretty-printed with newlines and four-space indentation after a parse/re-serialize round trip.
Why: a client that compares the policy it put with the policy it reads back, hashes it, or signs it sees a difference that AWS would not produce; byte-sensitive drift detection always reports a change.
Fix: the access-point policy state — store and return the document verbatim instead of round-tripping it through a JSON model.
Traces: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#16.

--- Cause 4 of 5: ListAccessPoints scoped to a bucket with no access points answers 200 ---
AWS: recorded behavior only — listing access points scoped to a bucket that has none attached is refused with HTTP 400, Code `InvalidRequest` and Message `No access point attached to this bucket` (fixed text), plus RequestId/HostId.
Emulator: answers HTTP 200 with no error body.
Why: a client probing whether a bucket has an access point gets a success where AWS reports a request it must change, so the bucket-scoping contract is invisible to it.
Fix: the ListAccessPoints bucket-scoping branch in the s3control crate.
Traces: s3control-outposts-bucket-mutations-reject-a-plain-bucket-name#task#1.

--- Cause 5 of 5: PutBucketReplication against a plain bucket name yields a validation error instead of InvalidURI ---
AWS: recorded behavior only — the s3control bucket operations address Outposts buckets by ARN, so a plain bucket name in the path fails URI parsing first: HTTP 400, Code `InvalidURI`, Message `Couldn't parse the specified URI.` and `<URI>` carrying the offending path from the request (`bucket/<name>/replication`), plus RequestId/HostId.
Emulator: answers 400 with Code `ValidationException` and Message `1 validation error detected: Value null at 'replicationConfiguration.rules' failed to satisfy constraint: Member must not be null`, i.e. it parses the path, accepts the bucket and then complains about the body.
Why: the client is told its body is wrong when AWS is telling it the resource it addressed cannot be parsed; it will keep retrying with a corrected body against an address that can never work.
Fix: the s3control bucket-path routing/parsing stage ahead of PutBucketReplication's body validation.
Traces: s3control-outposts-bucket-mutations-reject-a-plain-bucket-name#task#6.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator:
- PutAccessPointPolicy (non-JSON policy): `$.ErrorResponse.Error.Message: value_mismatch (expected "Policies must be valid JSON and the first byte must be '{'", got 'The specified policy is not a policy this service can store: it is not a well-formed JSON document')` plus RequestId/HostId missing — s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#4 (2 affected steps).
- PutAccessPointPolicy (unknown field): `status: expected 400, got 200`; `$: empty_body (expected {'ErrorResponse': {'Error': {'Code': 'MalformedPolicy', 'Message': 'Unknown field hello'}, ...}}, got {})` — same fixture task#6 (7 affected steps, i.e. seven distinct malformed documents AWS refuses and the emulator stores).
- GetAccessPointPolicy: `$.GetAccessPointPolicyResult.Policy: value_mismatch` — expected the compact document as submitted, got the same JSON pretty-printed with newlines and 4-space indent — same fixture task#16.
- ListAccessPoints (bucket scoping): `status: expected 400, got 200`; expected `{'Error': {'Code': 'InvalidRequest', 'Message': 'No access point attached to this bucket'}, ...}` — s3control-outposts-bucket-mutations-reject-a-plain-bucket-name#task#1.
- PutBucketReplication: `Code: expected 'InvalidURI', got 'ValidationException'`; `Message: expected "Couldn't parse the specified URI.", got "1 validation error detected: Value null at 'replicationConfiguration.rules' failed to satisfy constraint: Member must not be null"`; `Error.URI: missing (expected 'bucket/<name>/replication')` — same fixture task#6.

## Steps to Reproduce
1. Replay s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#4: PUT /v20180820/accesspoint/{Name}/policy with a body that is not JSON — the message differs from AWS's.
2. Same fixture task#6: PUT a syntactically valid JSON policy containing an undefined field (e.g. `hello`) — the emulator answers 200 and stores it instead of 400 MalformedPolicy / `Unknown field hello`.
3. Same fixture task#16: PUT a compact policy document, then GET /v20180820/accesspoint/{Name}/policy — the document comes back pretty-printed.
4. Replay s3control-outposts-bucket-mutations-reject-a-plain-bucket-name#task#1: GET /v20180820/accesspoint?bucket=<bucket with no access point> — 200 instead of 400 InvalidRequest.
5. Same fixture task#6: PUT /v20180820/bucket/<plain-bucket-name>/replication — the emulator answers ValidationException about the body instead of 400 InvalidURI naming the path.
