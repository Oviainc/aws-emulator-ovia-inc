Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.lambda-subscription-invocation-carries-the-sns-records-envelope
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.lambda-subscription-invocation-carries-the-sns-records-envelope|run#21|AssertionError|assert {"SigningCertUrl", "UnsubscribeUrl"} <= set(msg) and not (
Issue-Commit: dcaafa445ca563c5947bd284ba4541134d1f4f9e

# sns: the lambda-protocol notification record omits SignatureVersion, Signature and SigningCertUrl, so a function cannot verify the notification it was invoked with

> _Note: this issue was identified against the code base at commit `dcaafa445ca563c5947bd284ba4541134d1f4f9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: the Sns object of the one-record aws:sns event SNS hands a Lambda subscription carries no signature members.

AWS: recorded behavior only (the Lambda event document is out-of-band and not in the Smithy model; Publish's modeled output is only MessageId/SequenceNumber). The recording shows the record's Sns object carrying exactly these members: Type ('Notification'), MessageId, TopicArn, Subject (null when none was published), Message, Timestamp, SignatureVersion ('1'), Signature (non-empty), SigningCertUrl, UnsubscribeUrl and MessageAttributes — and in the Lambda event the two URL members use the '...Url' spelling (SigningCertUrl/UnsubscribeUrl), never the '...URL' spelling the HTTP/SQS envelope uses. Those member names are the fixed part of the requirement; their values come from the publish, the topic and the subscription.

Emulator: the Sns object's keys are ['Message', 'MessageAttributes', 'MessageId', 'Subject', 'Timestamp', 'TopicArn', 'Type', 'UnsubscribeUrl'] — SigningCertUrl, Signature and SignatureVersion are absent entirely. Everything else the check asserted up to that point matched (Records wrapper, one record, EventSource 'aws:sns'/EventVersion '1.0', EventSubscriptionArn, Type/MessageId/TopicArn/Message/Subject, and the Url casing of UnsubscribeUrl).

Why: the documented way for a subscribed function to authenticate what invoked it is to fetch SigningCertUrl, rebuild the canonical string and verify Signature with the digest selected by SignatureVersion. On this emulator that code cannot run at all: record['Sns']['SignatureVersion'] raises KeyError, so a handler that verifies before acting either crashes on every invocation or (if it fails open) accepts unauthenticated input.

Fix: the SNS lambda-protocol record serializer in the subscription delivery path — it must emit the signature members (SignatureVersion, Signature, SigningCertUrl) alongside the existing ones, using the same signing material the notification-envelope serializer needs.

Related open issues that do NOT cover this (neither carries this replay key; both describe the SQS/HTTP envelope's '...URL'-spelled members, not the Lambda record): 20261025T064350Z-sns-9-behavioral-divergences-unsigned-notification-envelopes-unvalidated.md (cause 1) and 20261029T183635Z-sns-sqs-the-delivered-notification-envelope-omits-signatureversion.md.

Traces: sns.lambda-subscription-invocation-carries-the-sns-records-envelope.

## Evidence
Check sns.lambda-subscription-invocation-carries-the-sns-records-envelope [mismatch]:

Expected (AWS): {'SigningCertUrl','UnsubscribeUrl'} <= set(record['Sns']) and no '...URL'-spelled member; record['Sns']['SignatureVersion'] == '1' with a non-empty Signature.

Actual (emulator): AssertionError: "the Lambda event spells SigningCertUrl/UnsubscribeUrl (not the ...URL envelope form); keys were ['Message', 'MessageAttributes', 'MessageId', 'Subject', 'Timestamp', 'TopicArn', 'Type', 'UnsubscribeUrl']" — the failure is the three missing members (SigningCertUrl, Signature, SignatureVersion), not the casing, which the emulator already gets right.

## Steps to Reproduce
1. git_clone the repo, then run_behavioral_suite and re-replay sns.lambda-subscription-invocation-carries-the-sns-records-envelope.
2. CreateTopic; create a python3.12 function that forwards its event to an SQS capture queue; AddPermission(lambda:InvokeFunction, Principal=sns.amazonaws.com, SourceArn=topic); Subscribe with Protocol='lambda'.
3. Publish with a Subject and String/Number/Binary MessageAttributes, then Publish a bare message.
4. Drain the capture queue and inspect event['Records'][0]['Sns']: SignatureVersion, Signature and SigningCertUrl are absent.
