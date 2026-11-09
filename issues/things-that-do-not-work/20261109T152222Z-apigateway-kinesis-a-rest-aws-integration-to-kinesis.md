Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__kinesis.rest-api-aws-integration-kinesis-putrecord-with-credentials-role-puts-the-record
Issue-Key: apigateway__kinesis.rest-api-aws-integration-kinesis-putrecord-with-credentials-role-puts-the-record|_poll_http#24|TimeoutError|raise TimeoutError(
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__kinesis: a REST AWS integration to Kinesis PutRecord with a credentials role answers 500 instead of putting the record or returning the role's AccessDeniedException

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
Cause 1 of 1: an AWS-service integration to Kinesis never performs the service call; the stage answers 500 for both the permitted and the denied leg.
AWS: a REST API AWS integration whose uri is arn:aws:apigateway:<region>:kinesis:action/PutRecord and whose `credentials` names a role with kinesis:PutRecord performs the PutRecord under that role and returns Kinesis's response (ShardId/SequenceNumber) with HTTP 200. When the credentials role does NOT grant kinesis:PutRecord, API Gateway surfaces Kinesis's refusal: the integration response carries AccessDeniedException and the method answers the mapped error status deterministically.
Emulator: every request to the stage is answered HTTP 500 with the default gateway response {"message":"Internal server error"} — the denied leg never converges on AccessDeniedException within 47s, and the permitted leg never puts a record.
Why: an API Gateway AWS-service integration to Kinesis is unusable, and the two outcomes a caller must distinguish — record written vs. role-denied — are collapsed into the same opaque 500, so no authorization contract can be observed.
Fix: the execute-api AWS-service integration path (arn:aws:apigateway:<region>:kinesis:action/<Action>) — perform the signed service call under the integration's credentials role and map the service's refusal (AccessDeniedException) onto the integration/method response instead of answering 500.
Traces: apigateway__kinesis.rest-api-aws-integration-kinesis-putrecord-with-credentials-role-puts-the-record

## Evidence
"TimeoutError: POST /put-denied converging on the deterministic AccessDeniedException (https://<host>/_aws/execute-api/<apiId>/sim2real/put-denied) never reached an accepted status within 47s. Last: 500 '{\"message\":\"Internal server error\"}'". Expected: the permitted leg returns 200 with Kinesis's PutRecord result and the record visible on the stream; the denied leg returns the AccessDeniedException-derived status. Host/port/api id are per-run values.

## Steps to Reproduce
1. CreateStream (kinesis). Create two IAM roles API Gateway can assume: one with kinesis:PutRecord on the stream, one without.
2. Create a REST API with POST /put and POST /put-denied; PutIntegration type AWS, httpMethod POST, uri arn:aws:apigateway:<region>:kinesis:action/PutRecord, credentials=<the respective role>, with a request template building the PutRecord payload.
3. Deploy and POST both paths.
Expected: /put returns 200 with ShardId/SequenceNumber and the record is on the stream; /put-denied surfaces AccessDeniedException. Actual: both answer 500 {"message":"Internal server error"}.
