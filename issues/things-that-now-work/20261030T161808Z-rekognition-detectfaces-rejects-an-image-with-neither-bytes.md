Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-image-analysis-detect-operations
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-image-analysis-detect-operations
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|DetectFaces|4xx->4xx|$.Code
Issue-Key: rekognition|DetectFaces|4xx->4xx|$.Message
Issue-Commit: 45d3476fddc0c303f86586399cfbfaaac87d2cc4

# Rekognition DetectFaces rejects an Image with neither Bytes nor S3Object using a non-AWS message and omits Code

> _Note: this issue was identified against the code base at commit `45d3476fddc0c303f86586399cfbfaaac87d2cc4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectFaces

## Description
One cause, in the Rekognition image-analysis input validation path.

Cause 1 of 1: DetectFaces called with an Image structure that carries neither Bytes nor S3Object.

AWS: Smithy (model services/service-com-amazonaws-rekognition-rekognitionservice/smithy-model/com-amazonaws-rekognition-rekognitionservice.json): DetectFaces.Image is @required and targets structure Image, whose only members are Bytes (ImageBlob, @length 1..5242880) and S3Object; neither member is required by the model, so the "exactly one source must be supplied" rule is enforced by the service, not by constraint traits. DetectFaces's modelled errors include InvalidParameterException (client, httpError 400), documented as "Input parameter violated a constraint. Validate your parameter before calling the API operation again." InvalidParameterException models three members: Message, Code and Logref. The recorded response for an Image with no source is HTTP 400 with body {"__type": "InvalidParameterException", "Code": "InvalidParameterException", "Message": "Requested image should either contain bytes or s3 object."}. That Message is entirely FIXED text for this error class -- there is no variable part, no request value and no member name interpolated into it, so it must be emitted verbatim including the trailing period and the lower-case "s3 object".

Emulator: HTTP 400 (status is right) with __type "InvalidParameterException" but NO top-level Code member, and Message "Request has invalid parameters: Image must carry either Bytes or S3Object" -- a generic validator string the service never produces (wrong prefix, wrong wording, member names spelled out, no trailing period).

Why: a client that classifies the failure from the body's Code member (the awsJson1_1 convention Rekognition's recorded errors follow, and what several SDKs and CLI-driven tools read) sees an unidentified error and falls through to its generic/unknown-error branch instead of treating it as a malformed-input error it can fix locally; and any consumer, test or log matcher that asserts on AWS's documented message for the missing-image-source class never matches, so a retry/repair path keyed on that text (upload the bytes to S3 and retry with S3Object) is never taken.

Fix: the DetectFaces handler's Image input validation (emit InvalidParameterException with the fixed message "Requested image should either contain bytes or s3 object." when Image has neither Bytes nor S3Object) plus the shared Rekognition awsJson1_1 error serializer, which must emit the Code member alongside __type for every modelled error. The serializer half is the same defect already reported for the collection/face error paths in open issue 20261029T191740Z-rekognition-error-bodies-omit-the-code-member-and.md; that issue does NOT cover this step or the DetectFaces image-source message, which is why this is filed separately.

Traces: rekognition-image-analysis-detect-operations#task#8.

## Evidence
Replay of simparity-wire-aws-v1 (trace_set_id simparity-wire-aws-v1), verdict=mismatch, expected_status 400 == actual_status 400 -- only the body diverges.

Divergence key service=rekognition|DetectFaces|status=400->400|keys=$.Code,$.Message at rekognition-image-analysis-detect-operations#task#8:
  $.Code: missing (expected 'InvalidParameterException', got None)
  $.Message: value_mismatch (expected 'Requested image should either contain bytes or s3 object.', got 'Request has invalid parameters: Image must carry either Bytes or S3Object')

affected_count=1, stub_path empty (the operation IS implemented), owned_by empty (no open issue carries this replay key).

Fixed vs variable text: the whole expected Message is fixed text for this error class; nothing in it comes from the request or from emulator state.

## Steps to Reproduce
1. Replay fixture rekognition-image-analysis-detect-operations against the emulator and stop at step task#8: a DetectFaces call whose Image structure supplies neither Bytes nor S3Object.
2. Assert HTTP 400 and that the awsJson1_1 body carries all three top-level members: __type == "InvalidParameterException", Code == "InvalidParameterException", Message == "Requested image should either contain bytes or s3 object." (exact text, trailing period included).
3. Observed instead: Code absent and Message == "Request has invalid parameters: Image must carry either Bytes or S3Object".
