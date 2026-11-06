Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code s3control-account-public-access-block-lifecycle
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|PutAccessPointPolicy|4xx->2xx|$
Issue-Key: s3control|PutAccessPointPolicy|4xx->2xx|$headers.content-length
Issue-Key: s3control|PutAccessPointPolicy|4xx->2xx|status
Issue-Key: s3control|PutAccessPointPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|PutPublicAccessBlock|2xx->2xx|$headers.content-length
Issue-Key: s3control|PutPublicAccessBlock|2xx->2xx|status
Issue-Commit: 4274f29ee1bfe9df1e954a309f62e0b76d7a1961

# s3control put operations diverge: access-point policy validation misclassifies causes and empty successes answer 204 not 200

> _Note: this issue was identified against the code base at commit `4274f29ee1bfe9df1e954a309f62e0b76d7a1961`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccessPointPolicy

## Description
Three independent causes in the s3control PUT surface. The shared fault: the handlers for operations with an empty output (PutAccessPointPolicy, PutPublicAccessBlock) neither validate their payload the way AWS does nor serialize the success the model declares. Each cause must be closed separately.

Cause 1 of 3: PutAccessPointPolicy ACCEPTS malformed access-point policies that AWS refuses.
AWS: Smithy: PutAccessPointPolicy PUT /v20180820/accesspoint/{Name}/policy, success code 200, input Policy @required (plus AccountId @required @httpHeader x-amz-account-id + host label, Name @httpLabel). Recorded behavior: a policy document whose statement names an action that is not a real S3 action is refused with HTTP 400, REST-XML Error.Code "MalformedPolicy", Error.Message "Policy has invalid action" and an Error.Detail member naming the offending token. Message rule: "MalformedPolicy" and "Policy has invalid action" are fixed text; Error.Detail carries the offending value taken from the submitted policy document (in the recording the action string it rejected), not a literal from one run. The recorded cluster covers three distinct malformed documents, all refused by AWS.
Emulator: answers HTTP 204 with an empty body and no content-length header — i.e. it stores the malformed policy and reports success. (The 204 itself is Cause 3; here the defect is the missing refusal.)
Why: a client that submits a bad policy is told it succeeded, so the policy it believes is attached on AWS would in fact never have been accepted; subsequent authorization behavior and any negative test of policy validation diverge silently.
Fix: the PutAccessPointPolicy handler's policy-document validation (action/syntax/structure checks) and its MalformedPolicy error serialization, including the Error.Detail member.
Traces: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#9 (cluster also covers task#10 and task#11).

Cause 2 of 3: PutAccessPointPolicy collapses distinct malformed-policy causes onto one message.
AWS: recorded behavior only (the model declares no operation errors; s3control uses the common REST-XML error shapes): a policy document that is not valid policy syntax is refused with HTTP 400 Error.Code "MalformedPolicy" and Error.Message "Invalid policy syntax." — a different message from the invalid-action case ("Policy has invalid action"). Both messages are fixed text for their respective cause; the message distinguishes WHY the document was rejected.
Emulator: refuses with HTTP 400 but reports Error.Message "Policy has invalid action" for the invalid-syntax document, i.e. every rejection it does make carries the invalid-action message.
Why: a client (or an operator reading the error) is told the policy names a bad action when the document does not parse at all, so the remediation it derives from the message is wrong, and tests that assert on AWS's refusal reason fail.
Fix: the PutAccessPointPolicy validation path must classify the rejection cause and select the matching fixed message per cause instead of emitting one message for all MalformedPolicy rejections.
Traces: s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#12.

Cause 3 of 3: empty-output PUT operations answer 204 No Content instead of the modeled 200 with a content-length.
AWS: Smithy: PutPublicAccessBlock PUT /v20180820/configuration/publicAccessBlock declares http code 200 with output Unit (empty), and PutAccessPointPolicy likewise declares code 200; AWS answers 200 and sets a content-length header on these empty-body successes.
Emulator: answers HTTP 204 and emits no content-length header ('$headers.content-length: missing').
Why: a client or SDK/middleware that keys on the modeled success status (200) — or on the presence of content-length — sees a different status class than AWS for a successful put, so status assertions, response-handling branches and any proxy that distinguishes 200 from 204 behave differently against the emulator.
Fix: the response serializer for s3control operations with an empty output shape: use the @http code from the model (200) and emit the content-length header, rather than defaulting empty bodies to 204.
Traces: s3control-account-public-access-block-lifecycle#task#2 (PutPublicAccessBlock); the same 204 is visible on PutAccessPointPolicy in s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#9.

Related open issue that does NOT cover these causes: 20261024T210022Z-s3control-account-level-blockpublicpolicy-not-enforced-on-putbucketpolicy.md concerns enforcement of account-level BlockPublicPolicy on PutBucketPolicy, not access-point policy validation or the success status of these puts.

## Evidence
Replay of trace set simparity-wire-aws-v1 (s3control):

Cause 1 — s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#9, PutAccessPointPolicy (cluster also affects task#10, task#11): 'status: expected 400, got 204'; '$: empty_body (expected {"ErrorResponse": {"Error": {"Code": "MalformedPolicy", "Message": "Policy has invalid action", "Detail": "<offending action from the submitted policy>"}, "RequestId": ..., "HostId": ...}}, got {})'; '$headers.content-length: missing'.

Cause 2 — s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code#task#12, PutAccessPointPolicy: '$.ErrorResponse.Error.Message: value_mismatch (expected "Invalid policy syntax.", got "Policy has invalid action")' (status 400 on both sides, Code MalformedPolicy on both sides).

Cause 3 — s3control-account-public-access-block-lifecycle#task#2, PutPublicAccessBlock: 'status: expected 200, got 204'; '$headers.content-length: missing (expected a value, got None)'.

Model evidence (Smithy, com-amazonaws-s3control-awss3controlservicev20180820): PutAccessPointPolicy PUT /v20180820/accesspoint/{Name}/policy code 200, Policy @required, output Unit; PutPublicAccessBlock PUT /v20180820/configuration/publicAccessBlock code 200, PublicAccessBlockConfiguration @required @httpPayload, output Unit.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture s3c-access-point-policy-rejection-collapses-eight-causes-into-one-code, steps task#9/task#10/task#11 — PUT /v20180820/accesspoint/{Name}/policy with a policy document naming an action that is not a real S3 action; AWS answers 400 MalformedPolicy "Policy has invalid action" with Error.Detail naming the offending action, the emulator answers 204 with an empty body.
4. Cause 2: same fixture, step task#12 — PUT the policy with a document that is not valid policy syntax; AWS answers 400 MalformedPolicy "Invalid policy syntax.", the emulator answers 400 MalformedPolicy "Policy has invalid action".
5. Cause 3: fixture s3control-account-public-access-block-lifecycle, step task#2 — PUT /v20180820/configuration/publicAccessBlock with a valid configuration; AWS answers 200 with a content-length header, the emulator answers 204 with none.
