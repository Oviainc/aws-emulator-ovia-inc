Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.put-bucket-metrics-configuration-invalid-id-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.put-bucket-metrics-configuration-invalid-id-rejected|run#7|AssertionError|assert code == ("MalformedXML", "The XML you provided was not well-formed or did not validate against our published schema"), f"expected MalformedXML, got {code
Issue-Commit: a1aff8283ff7a4c3e09b44355afd91a57915fcd2

# S3 PutBucketMetricsConfiguration rejects an over-long metrics Id with InvalidArgument instead of MalformedXML (1 cause)

> _Note: this issue was identified against the code base at commit `a1aff8283ff7a4c3e09b44355afd91a57915fcd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketMetricsConfiguration

## Description
Cause 1 of 1: a metrics-configuration Id that violates the documented 64-character limit is refused with the wrong error code.
AWS: Smithy: s3#PutBucketMetricsConfiguration takes Id (httpQuery 'id') and MetricsConfiguration (httpPayload, xmlName MetricsConfiguration); MetricsConfiguration.Id targets s3#MetricsId, whose documentation states "The ID has a 64 character limit and can only contain letters, numbers, periods, dashes, and underscores". MetricsId carries no modeled @length or @pattern trait, so the constraint is enforced by the service, not by protocol-level shape validation. The operation models no errors of its own, and the recording shows the refusal arriving as the S3 common error code MalformedXML with HTTP 400 — i.e. S3 treats a body whose Id does not satisfy the published schema as a schema violation of the request document.
Emulator: the same request is refused with error code InvalidArgument and a bespoke message ('Id "xxxx..." is not a metrics configuration id: at most 64 characters, each a letter, a number, a period, a dash or an underscore'). The code is the divergence; the message text is quoted only as an example of what is returned.
Why: callers branch on the error code, not on prose. SDK and tooling error handling for S3 sub-resource PUTs routes MalformedXML to "my request document is wrong, do not retry, fix the body", while InvalidArgument is the code S3 uses for a bad query/header argument; a client that distinguishes the two mis-attributes the fault to the 'id' query parameter rather than to the MetricsConfiguration document, and any retry/validation layer keyed on MalformedXML does not fire at all.
Fix: the validation step in the PutBucketMetricsConfiguration handler that checks MetricsConfiguration.Id against the 64-character/charset rule — it must raise the MalformedXML common error with HTTP 400 rather than InvalidArgument. The constraint check itself is correct and should be kept.
Traces: s3.put-bucket-metrics-configuration-invalid-id-rejected.

## Evidence
Expected (error code and status are requirements): HTTP 400 with Code='MalformedXML'. Actual: Code='InvalidArgument'. Reported as AssertionError at check_s3_put_bucket_metrics_configuration_invalid_id_rejected.py line 31: "expected MalformedXML, got ('InvalidArgument', 'Id \"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\" is not a metrics configuration id: at most 64 characters, each a letter, a number, a period, a dash or an underscore')". The recorded AWS message ('The XML you provided was not well-formed or did not validate against our published schema') is shown as an example; only the code and status are asserted here as the contract.

## Steps to Reproduce
Create a bucket, then call PutBucketMetricsConfiguration with an Id longer than 64 characters (e.g. 65 'x' characters) and MetricsConfiguration={'Id': <that same id>}. The emulator answers 400 InvalidArgument; AWS answers 400 MalformedXML.

Replay: s3.put-bucket-metrics-configuration-invalid-id-rejected.
