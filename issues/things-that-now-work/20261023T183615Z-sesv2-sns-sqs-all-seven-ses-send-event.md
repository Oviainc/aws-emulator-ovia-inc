Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns__sqs.account-suppression-from-a-hard-bounce-blocks-the-next-send sesv2__sns__sqs.event-destination-disabled-publishes-no-events sesv2__sns__sqs.send-bulk-email-per-entry-replacement-headers-override-the-template-default sesv2__sns__sqs.send-email-to-success-simulator-returns-message-id-and-a-delivery-event sesv2__sns__sqs.send-raw-email-preserves-custom-headers-exactly-in-the-published-event sesv2__sns__sqs.send-to-bounce-and-complaint-simulator-publishes-documented-event-shapes sesv2__sns__sqs.templated-send-with-missing-variable-publishes-rendering-failure
Issue-Key: sesv2__sns__sqs|CreateEmailIdentity|AmbiguousService
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# sesv2__sns__sqs: all seven SES send/event-publication scenarios abort at setup — CreateEmailIdentity is refused 'AmbiguousService' by the sigv4-name router

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEmailIdentity

## Description
Attribution note: these check ids' leading token is the composite 'sesv2__sns__sqs'; all three constituent services (sesv2, sns, sqs) are stood up in this checkout, so the per-service bootstrap gate is FILE. The refused call in each is the SESv2 one.

Cause 1 of 1: every sigv4-'ses' request is refused with the non-AWS code AmbiguousService instead of being dispatched to SESv2.
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2): SimpleEmailService_v2, version 2019-09-27, protocols [restJson1], sigv4Name 'ses', awsService.endpointPrefix 'email', arnNamespace 'ses'. CreateEmailIdentity is POST /v2/email/identities with success code 200 and output {IdentityType, VerifiedForSendingStatus, DkimAttributes} (the DKIM tokens these checks publish in Route 53 to verify a stand-in sending domain); its modeled errors are AlreadyExistsException 400, BadRequestException 400, LimitExceededException 400, NotFoundException 404, ConcurrentModificationException 500, TooManyRequestsException 429. Three further on-disk models share sigv4Name 'ses' (SES v1 awsQuery, AmazonPinpointEmailService restJson1, MailManagerSvc awsJson1_0/rpcv2Cbor), so the signing name is not a service identity. AmbiguousService is in no AWS model.
Emulator: no SESv2 request reaches a handler. CreateEmailIdentity is answered with code 'AmbiguousService' and the message "4 emulated services all claim requests for 'ses': ses, ses, ses, ses; the request is rejected rather than served by an arbitrary one" (message quoted as an example; the code and its 5xx class are the requirement), retried to exhaustion by botocore ('reached max retries: 3'). No DKIM tokens are ever returned, so no sending identity can be established.
Why: the entire send-and-publish surface is unreachable for every client. Nothing these scenarios depend on can be set up or observed — no verified sending identity, no SendEmail/SendBulkEmail/raw-MIME send, no message id, and none of the SNS->SQS event publication (delivery, bounce, complaint, rendering-failure, suppression-on-hard-bounce, disabled-destination) that the configuration set's event destination would produce. The returned code is in no AWS model, so SDK error handling cannot classify it and clients spend their whole retry budget on a non-retryable condition.
Fix: the request router / service-registry resolution layer that selects a target service before operation dispatch; the four registrations claiming sigv4 name 'ses' must resolve to exactly one target per request using protocol + endpoint prefix + request shape (restJson1 /v2/email/... for SESv2, /v1/email/... for Pinpoint Email, awsQuery POST / with Action= for SES v1, awsJson1_0 X-Amz-Target with endpointPrefix 'mail-manager' for MailManager), never by signing name alone. This is the same router defect reported in 20261023T120410Z-ses-v1-and-v2-is-entirely-unreachable-the.md and 20261022T194351Z-firehose-s3-sesv2-ses-v2-is-unreachable-the-router-answers-ambiguousservice.md; neither carries these checks' replay keys.
Traces: all seven check ids in trace_ids.

## Evidence
Suite simparity-behavioral-aws-v1: sesv2__sns__sqs 0/7 passed, 7 failed — all seven on the same first call.

Expected: CreateEmailIdentity returns HTTP 200 with IdentityType, VerifiedForSendingStatus and DkimAttributes.Tokens so the check can publish the Easy DKIM CNAMEs and get the stand-in domain verified for sending.
Actual, in all seven: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateEmailIdentity operation (reached max retries: 3): 4 emulated services all claim requests for 'ses': ses, ses, ses, ses; the request is rejected rather than served by an arbitrary one.

## Steps to Reproduce
1. Point a SESv2 client at the emulator and call sesv2.create_email_identity(EmailIdentity=<domain>, Tags=[...]).
2. Expect HTTP 200 with DkimAttributes.Tokens; the emulator answers a 5xx with error code AmbiguousService and botocore retries it 3 times, so verification never starts.
3. Full grounding: replay the seven sesv2__sns__sqs check ids in trace_ids, which go on to create a configuration set with an SNS event destination bridged to SQS and assert the delivery, bounce, complaint, rendering-failure, suppression and disabled-destination event behavior.
