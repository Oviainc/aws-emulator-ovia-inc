Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__s3__sesv2.firehose-destination-delivers-two-send-events-within-the-buffering-interval firehose__s3__sesv2.firehose-destination-rejects-a-role-the-stream-cannot-be-assumed-by
Issue-Key: firehose__s3__sesv2|CreateEmailIdentity|AmbiguousService
Issue-Key: firehose__s3__sesv2|CreateConfigurationSet|AmbiguousService
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# SES v2 is unreachable: the router answers AmbiguousService for every request signed for the 'ses' endpoint prefix

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEmailIdentity

## Description
Cause 1 of 1: four emulated services all claim the 'ses' endpoint prefix, so the router refuses every SES/SESv2 request instead of serving it.
AWS: sesv2.CreateEmailIdentity and sesv2.CreateConfigurationSet are ordinary control-plane calls signed with the sigv4 service name 'ses'; they return 200 (CreateEmailIdentity with IdentityType/VerifiedForSendingStatus, CreateConfigurationSet with an empty body) and model client errors such as BadRequestException/AlreadyExistsException. A client sees one service behind that prefix.
Emulator: the request never reaches a handler — 'An error occurred (AmbiguousService) when calling the CreateEmailIdentity operation (reached max retries: 3): 4 emulated services all claim requests for "ses": ses, ses, ses, ses; the request is rejected rather than served by an arbitrary one' (same for CreateConfigurationSet). AmbiguousService is not an AWS error code, and botocore retries it to exhaustion first.
Why: the whole SESv2 surface is unreachable: an identity or configuration set cannot be created, so no event-destination wiring (SES send events delivered through a Firehose stream to S3, and the refusal of a Firehose role SES cannot assume) can be configured or observed. The emulator advertises an endpoint it then refuses to serve.
Fix: endpoint/service routing — the four registrations claiming 'ses' (SES, SESv2, Pinpoint Email and the SMS/Voice-era crates) must be disambiguated so the signed request is dispatched to exactly one, in the request-router/service-registry layer rather than in any operation handler.
Traces: firehose__s3__sesv2.firehose-destination-delivers-two-send-events-within-the-buffering-interval, firehose__s3__sesv2.firehose-destination-rejects-a-role-the-stream-cannot-be-assumed-by.

## Evidence
Expected: 200 from sesv2.create_email_identity / sesv2.create_configuration_set. Actual: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateEmailIdentity operation (reached max retries: 3): 4 emulated services all claim requests for 'ses': ses, ses, ses, ses; the request is rejected rather than served by an arbitrary one — and the same for CreateConfigurationSet in the second check.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. ses = client('sesv2'); ses.create_email_identity(EmailIdentity='sim2real@example.com') -> expect 200; observe AmbiguousService after retries.
3. ses.create_configuration_set(ConfigurationSetName='sim2real-cs') -> expect 200; observe the same AmbiguousService refusal.
4. Re-replay the check ids in trace_ids.
