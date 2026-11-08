Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-cloudfront-function-get-workflow
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-cloudfront-function-get-workflow cloudfront-fle-config-lifecycle-with-profile-reference cloudfront-response-headers-policy-precondition-and-notfound-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetFunction|4xx->2xx|$
Issue-Key: cloudfront|GetFunction|4xx->2xx|$headers.content-type
Issue-Key: cloudfront|GetFunction|4xx->2xx|status
Issue-Key: cloudfront|GetFieldLevelEncryptionProfile|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|GetFieldLevelEncryptionProfile|4xx->2xx|status
Issue-Key: cloudfront|GetPublicKey|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|GetPublicKey|4xx->2xx|status
Issue-Key: cloudfront|DeleteResponseHeadersPolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|DeleteResponseHeadersPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteResponseHeadersPolicy|4xx->4xx|status
Issue-Commit: 903d830d252bdd68b0796b6265be694baf73f62a

# CloudFront refusals diverge: deleted resources stay readable and If-Match is not validated before lookup

> _Note: this issue was identified against the code base at commit `903d830d252bdd68b0796b6265be694baf73f62a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunction

## Description
Four net-new wire divergences in the CloudFront control plane, all in the refusal path: three resource reads succeed after the resource was deleted (so the modeled NoSuch* refusal never happens), and one delete refuses on resource lookup before it validates the If-Match precondition header. No open issue carries any of these four replay keys (the replay report lists owned_by: [] for all four). The nearest open issues are 20261105T231732Z-cloudfront-path-identifiers-are-not-format-validated-before.md, 20261029T212223Z-cloudfront-if-match-etag-handling-wrong-precondition-message.md and 20261029T212223Z-cloudfront-refusal-codes-and-messages-diverge-from-aws.md; none of them covers the delete-then-read lifecycle faults or the refusal ORDERING below. This bundle has 4 causes and all 4 must be closed.

Cause 1 of 4 — GetFunction returns a deleted function's code instead of 404 NoSuchFunctionExists (fixture cloudfront-cloudfront-function-get-workflow, step teardown#7).
AWS: Smithy: GetFunction GET /2020-05-31/function/{Name}, output FunctionCode (@httpPayload, sensitive blob), errors NoSuchFunctionExists (@httpError 404, doc "The function does not exist."), UnsupportedOperation (400). Once DeleteFunction has removed the function, a subsequent GetFunction for that name is refused 404 with a REST-XML ErrorResponse body (Content-Type: text/xml) carrying Error/Type=Sender, Error/Code=NoSuchFunctionExists and the message AWS words for this class, "The specified function does not exist." — that sentence is fixed text, it interpolates nothing from the request; the function name travels only in the request URI. The recorded response confirms status 404, that body and text/xml.
Emulator: answers 200 with Content-Type: application/octet-stream and the previously created function source as the payload, i.e. the deleted function is still served. No ErrorResponse at all.
Why: a client that deletes a function and then polls GetFunction to confirm removal (or that relies on 404 to decide a name is free for re-create) sees the function as still existing forever. Deletion looks like a no-op, and a caller that treats 200 as "still live" loops or re-uses stale code.
Fix: DeleteFunction's state mutation / GetFunction's lookup in the cloudfront service state — the delete must remove the function from the store (all stages) so the read path falls through to the NoSuchFunctionExists refusal, and the error path must serialize REST-XML with Content-Type: text/xml rather than the payload serializer's octet-stream.
Traces: cloudfront-cloudfront-function-get-workflow#teardown#7.

Cause 2 of 4 — GetFieldLevelEncryptionProfile returns a deleted profile instead of 404 NoSuchFieldLevelEncryptionProfile (fixture cloudfront-fle-config-lifecycle-with-profile-reference, step teardown#10).
AWS: Smithy: GetFieldLevelEncryptionProfile GET /2020-05-31/field-level-encryption-profile/{Id}, errors NoSuchFieldLevelEncryptionProfile (@httpError 404, doc "The specified profile for field-level encryption doesn't exist."), AccessDenied (403). After the profile has been deleted, the read is refused 404 with ErrorResponse/Error Type=Sender, Code=NoSuchFieldLevelEncryptionProfile and the runtime message "The specified field level encryption profile does not exist." — fixed text, no interpolated values (the id appears only in the request URI). Status and code are taken from the recorded response.
Emulator: answers 200 and returns the profile document; the response has no ErrorResponse element at all.
Why: the FLE teardown contract is unobservable — a client cannot tell a deleted profile from a live one, and an orchestration that waits for 404 before deleting the public key it referenced never proceeds.
Fix: DeleteFieldLevelEncryptionProfile's state mutation / GetFieldLevelEncryptionProfile's lookup in the cloudfront service state, so a deleted profile is absent from the store and the read path reaches its modeled 404 refusal.
Traces: cloudfront-fle-config-lifecycle-with-profile-reference#teardown#10.

Cause 3 of 4 — GetPublicKey returns a deleted public key instead of 404 NoSuchPublicKey (fixture cloudfront-fle-config-lifecycle-with-profile-reference, step teardown#11).
AWS: Smithy: GetPublicKey GET /2020-05-31/public-key/{Id}, errors NoSuchPublicKey (@httpError 404, doc "The specified public key doesn't exist."), AccessDenied (403). After DeletePublicKey the read is refused 404 with ErrorResponse/Error Type=Sender, Code=NoSuchPublicKey, message "The specified CloudFront public key does not exist." — entirely fixed text for this error class; the key id is carried by the URI, not the message. Status/code/message per the recorded response.
Emulator: answers 200 with the public-key document and no ErrorResponse.
Why: same class of breakage as cause 2 — deletion is not observable, so key-rotation flows that confirm removal before re-registering a key id never converge, and a caller can keep reading a key it has revoked.
Fix: DeletePublicKey's state mutation / GetPublicKey's lookup in the cloudfront service state.
Traces: cloudfront-fle-config-lifecycle-with-profile-reference#teardown#11.

Cause 4 of 4 — DeleteResponseHeadersPolicy checks the id before the If-Match precondition, and answers a not-found code with a format-validation message (fixture cloudfront-response-headers-policy-precondition-and-notfound-errors, step task#4).
AWS: Smithy: DeleteResponseHeadersPolicy DELETE /2020-05-31/response-headers-policy/{Id}, input Id (@httpLabel) and IfMatch (@httpHeader If-Match), errors InvalidIfMatchVersion (@httpError 400, doc "The If-Match version is missing or not valid."), NoSuchResponseHeadersPolicy (404, doc "The response headers policy does not exist."), PreconditionFailed (412), IllegalDelete (400), ResponseHeadersPolicyInUse (409), AccessDenied (403). When the If-Match header is missing or malformed, AWS refuses the request FIRST on the precondition, before any resource lookup: 400 with ErrorResponse/Error Type=Sender, Code=InvalidIfMatchVersion and the message "The If-Match version is missing or not valid for the resource." — fixed text, no request-derived values. The recording confirms 400 / InvalidIfMatchVersion / that message.
Emulator: answers 404 with Code=NoSuchResponseHeadersPolicy and Message="Id format is invalid" — the wrong status (404 vs 400), the wrong error code, and a message that is emulator-authored (AWS's text for that code is "The response headers policy does not exist.", and it is not the applicable code here anyway).
Why: a client cannot distinguish "you forgot/garbled If-Match" from "the policy is gone". SDK retry/idempotency logic treats 404 NoSuchResponseHeadersPolicy as a successful teardown and stops supplying the ETag, so the real precondition bug is masked and a concurrent-update guard is silently lost.
Fix: DeleteResponseHeadersPolicy's handler/validation ordering in the cloudfront service — validate the If-Match header (present and well-formed) and emit InvalidIfMatchVersion at 400 before resolving the Id; the Id-format refusal must not borrow the NoSuchResponseHeadersPolicy code, and that code's message must be AWS's fixed text. Note the open stub issue 20261029T175823Z-cloudfront-deleteresponseheaderspolicy-stub.md: the operation is no longer answering 501, so that stub issue does not cover this refusal-ordering defect.
Traces: cloudfront-response-headers-policy-precondition-and-notfound-errors#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (192 fixtures, 1310 steps, score 83.7%, emulator_up=true, bootstrap_signal=false). Four clusters with owned_by: [] and stub_path: "":

1) cloudfront-cloudfront-function-get-workflow#teardown#7 GetFunction — expected 404, got 200. detail: $: type_mismatch (expected {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'NoSuchFunctionExists', 'Message': 'The specified function does not exist.'}, 'RequestId': '<shaped-not-asserted>'}}, got 'function handler(event) {\n    var response = event.response;\n    return response;\n}\n'); $headers.content-type: expected 'text/xml', got 'application/octet-stream'. divergence_key service=cloudfront|GetFunction|status=404->200|keys=$,$headers.content-type,status.

2) cloudfront-fle-config-lifecycle-with-profile-reference#teardown#10 GetFieldLevelEncryptionProfile — expected 404, got 200. detail: $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'NoSuchFieldLevelEncryptionProfile', 'Message': 'The specified field level encryption profile does not exist.'}, 'RequestId': '<shaped-not-asserted>'}, got None). divergence_key service=cloudfront|GetFieldLevelEncryptionProfile|status=404->200|keys=$.ErrorResponse,status.

3) cloudfront-fle-config-lifecycle-with-profile-reference#teardown#11 GetPublicKey — expected 404, got 200. detail: $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'NoSuchPublicKey', 'Message': 'The specified CloudFront public key does not exist.'}, 'RequestId': '<shaped-not-asserted>'}, got None). divergence_key service=cloudfront|GetPublicKey|status=404->200|keys=$.ErrorResponse,status.

4) cloudfront-response-headers-policy-precondition-and-notfound-errors#task#4 DeleteResponseHeadersPolicy — expected 400, got 404. detail: $.ErrorResponse.Error.Code: expected 'InvalidIfMatchVersion', got 'NoSuchResponseHeadersPolicy'; $.ErrorResponse.Error.Message: expected 'The If-Match version is missing or not valid for the resource.', got 'Id format is invalid'. divergence_key service=cloudfront|DeleteResponseHeadersPolicy|status=400->404|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status.

Model evidence (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31): GetFunction errors NoSuchFunctionExists@404; GetFieldLevelEncryptionProfile errors NoSuchFieldLevelEncryptionProfile@404; GetPublicKey errors NoSuchPublicKey@404; DeleteResponseHeadersPolicy errors InvalidIfMatchVersion@400, NoSuchResponseHeadersPolicy@404 (doc "The response headers policy does not exist."), PreconditionFailed@412, with IfMatch bound to the If-Match header.

## Steps to Reproduce
Cause 1: CreateFunction a CloudFront function, DeleteFunction it (with its ETag), then GET /2020-05-31/function/{Name} — AWS 404 NoSuchFunctionExists (text/xml ErrorResponse); emulator 200 + function code as application/octet-stream. Replay cloudfront-cloudfront-function-get-workflow step teardown#7.
Cause 2: create an FLE profile, delete it, then GET /2020-05-31/field-level-encryption-profile/{Id} — AWS 404 NoSuchFieldLevelEncryptionProfile; emulator 200 with the profile. Replay cloudfront-fle-config-lifecycle-with-profile-reference step teardown#10.
Cause 3: create a public key, delete it, then GET /2020-05-31/public-key/{Id} — AWS 404 NoSuchPublicKey; emulator 200 with the key. Replay cloudfront-fle-config-lifecycle-with-profile-reference step teardown#11.
Cause 4: DELETE /2020-05-31/response-headers-policy/{Id} with a missing/invalid If-Match header — AWS 400 InvalidIfMatchVersion "The If-Match version is missing or not valid for the resource."; emulator 404 NoSuchResponseHeadersPolicy "Id format is invalid". Replay cloudfront-response-headers-policy-precondition-and-notfound-errors step task#4.
