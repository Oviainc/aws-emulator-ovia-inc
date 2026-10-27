Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.receipt-rule-s3-action-kms-key-policy-is-evaluated-apart-from-the-bucket-grant
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.receipt-rule-s3-action-kms-key-policy-is-evaluated-apart-from-the-bucket-grant|run#20|ResponseParserError|ses.create_receipt_rule_set(RuleSetName=rule_set)
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# SES v1 (awsQuery) CreateReceiptRuleSet is never dispatched: the router refuses every 'ses'-signed request with AmbiguousService, framed as awsJson so an awsQuery client cannot parse it

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReceiptRuleSet

## Description
One root cause, in the pre-dispatch request router / service registry (not in any SES operation handler). This check's replay key is NOT carried by the existing open issue '20261023T120410Z-ses-v1-and-v2-is-entirely-unreachable-the.md' (its Issue-Trace-Ids list the other 137 checks but not this one), so it is filed separately here and must be re-replayed with the id in trace_ids before that work item is closed.

Cause 1 of 1: a sigv4-signing-name-only route resolution makes the whole SES v1 surface unreachable, and the resulting refusal is serialized with JSON framing on an XML-protocol service.
AWS: SES v1 is reached by an ordinary sigv4-signed call and answers with its modelled output or a modelled client error. Smithy (services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json): service SimpleEmailService, version 2010-12-01, protocols [awsQuery], sigv4Name 'ses', awsService.endpointPrefix 'email', xmlNamespace http://ses.amazonaws.com/doc/2010-12-01/. Smithy: CreateReceiptRuleSet(RuleSetName required) -> empty output on success (HTTP 200, XML CreateReceiptRuleSetResponse), and its only modelled errors are AlreadyExistsException (awsQueryError code 'AlreadyExists', 400) and LimitExceededException ('LimitExceeded', 400). Because the protocol is awsQuery, EVERY response, success or error, is XML: an error is an <ErrorResponse><Error><Code>/<Message></Error></ErrorResponse> envelope that the SDK surfaces as a ClientError with a Code. On AWS a client sees exactly one service behind the endpoint it addressed; the signing name is not an identity (SimpleEmailService, SimpleEmailService_v2, AmazonPinpointEmailService and MailManagerSvc all sign as 'ses'). 'AmbiguousService' is not a code in any AWS model.
Emulator: the call never reaches a handler. ses.create_receipt_rule_set(RuleSetName=...) is answered with the non-AWS body b'{"__type":"AmbiguousService","message":"4 emulated services all claim requests for 'ses': ses, ses, ses, ses; the request is rejected rather than served by an arbitrary one"}'. Two things are wrong at once: (a) the request is refused instead of dispatched, and (b) the refusal is framed as awsJson for a service whose protocol is awsQuery, so botocore cannot even parse it and raises botocore.parsers.ResponseParserError 'Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed' rather than any ClientError with a status/code pair. The body is also served with a retryable status, so the client burns its retry budget first.
Why: no receipt rule set can be created, so nothing that depends on one (receipt rules, S3Action/KmsKeyArn pre-flight validation, SNS/Lambda actions, inbound mail) can be configured or observed at all; and because the error body is not in the addressed service's protocol framing, an SDK client cannot classify the failure — it sees a parse error with no code, so even generic error handling and retry classification break.
Fix: the pre-dispatch route-resolution layer (service selection before operation dispatch) must disambiguate the four registrations that share sigv4 name 'ses' down to exactly one target per request, using protocol + endpoint prefix + request shape (awsQuery POST / with an Action= body for SES v1 vs restJson1 /v2/email/... for SESv2 vs /v1/email/... for Pinpoint Email vs awsJson1_0 X-Amz-Target for Mail Manager); and the gateway's error serializer for pre-dispatch failures must select the addressed service's protocol framing (an XML ErrorResponse envelope for awsQuery) instead of always emitting awsJson.
Traces: ses.receipt-rule-s3-action-kms-key-policy-is-evaluated-apart-from-the-bucket-grant (the check's first SES call, ses.create_receipt_rule_set, is the one refused; the KMS/S3/STS setup calls before it succeeded).

## Evidence
Expected: HTTP 200 and an XML CreateReceiptRuleSetResponse from ses.create_receipt_rule_set(RuleSetName='sim2real-behav-ses-rs-<unique>'), or — on a negative leg — one of the operation's modelled awsQuery errors (AlreadyExists / LimitExceeded, 400) in an <ErrorResponse> envelope parsed by botocore into a ClientError with that Code.

Actual (suite replay, check ses.receipt-rule-s3-action-kms-key-policy-is-evaluated-apart-from-the-bucket-grant, verdict [error]):
  File "parsers.py", line 600, in _do_error_parse -> botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received. Further retries may succeed:
  b'{"__type":"AmbiguousService","message":"4 emulated services all claim requests for \'ses\': ses, ses, ses, ses; the request is rejected rather than served by an arbitrary one"}'

No Code and no status could be read by the SDK at all — the response is JSON where the model fixes XML. The same pre-dispatch refusal accounts for every other failing check in the suite (0/139 passed; ses 0/132, s3__ses__sesv2 0/4, cloudwatch__ses 0/2, ses__sns 0/1); those 137 are carried by the open issue 20261023T120410Z-ses-v1-and-v2-is-entirely-unreachable-the.md, which does not list this check id.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (any region; credentials are irrelevant — the refusal is pre-dispatch).
2. Call client('ses').create_receipt_rule_set(RuleSetName='sim2real-behav-ses-rs-1') against the emulator.
3. Observe botocore.exceptions.ResponseParserError instead of a 200 XML CreateReceiptRuleSetResponse; capture the raw HTTP response and confirm the body is awsJson ({"__type":"AmbiguousService",...}) on a retryable status rather than an XML <ErrorResponse> envelope.
4. Re-replay check id ses.receipt-rule-s3-action-kms-key-policy-is-evaluated-apart-from-the-bucket-grant to confirm the fix: after the router resolves the 'ses' signing-name collision, the check must get past create_receipt_rule_set and exercise the S3Action KmsKeyArn pre-flight (expected refusal code InvalidS3Configuration, 400).
