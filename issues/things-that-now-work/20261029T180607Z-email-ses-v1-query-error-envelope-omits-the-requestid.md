Issue-Source: trace-sweep-divergence
Issue-Trace: email-configuration-set-full-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-configuration-set-full-lifecycle email-configuration-set-lifecycle-and-default-reputation email-configuration-set-tracking-options-without-verified-domain email-custom-verification-email-template-workflow email-delete-receipt-rule-missing-rule-is-noop email-receipt-rule-set-lifecycle-and-activation email-delivery-options-invalid-tls-policy-rejected email-identity-sending-authorization-policy
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: email|CreateConfigurationSet|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|CreateConfigurationSetEventDestination|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|DescribeConfigurationSet|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|DeleteConfigurationSetTrackingOptions|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|CreateCustomVerificationEmailTemplate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|DeleteReceiptRule|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|DescribeReceiptRule|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|PutConfigurationSetDeliveryOptions|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|PutIdentityPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: email|DeleteReceiptRuleSet|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# SES v1 Query error envelope omits the RequestId element on every refusal

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSet

## Description
10 net-new wire causes on the SES v1 (awsQuery, endpointPrefix `email`, apiVersion 2010-12-01) refusal path. They share ONE root cause: the emulator's awsQuery ERROR serializer builds `<ErrorResponse>` with only the `<Error>` sub-tree and never emits the sibling `<RequestId>` element. Every one of the ten clusters below replays as a status-and-code MATCH (400 and the right awsQueryError code) whose body is missing exactly `$.ErrorResponse.RequestId`. The causes are enumerated one per diverging operation because each is its own replay cluster and each must be re-replayed after the fix; a single correction in the shared error serializer is expected to close all ten (and the six more in the companion issue 'SES v1 refusal messages are emulator-authored and the send/clone refusal envelopes omit RequestId', plus the two in the `ses`-token issue 'SES template-plane error envelope omits RequestId on UpdateTemplate and GetTemplate').

AWS (common to all ten causes): Smithy services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json — service SimpleEmailService, version 2010-12-01, protocols [awsQuery], xmlNamespace http://ses.amazonaws.com/doc/2010-12-01/, endpointPrefix email, sigv4Name ses. Under awsQuery a client error is serialized as `<ErrorResponse xmlns="http://ses.amazonaws.com/doc/2010-12-01/"><Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error><RequestId>…</RequestId></ErrorResponse>`. The model does not spell the envelope out, so the envelope is recorded behavior only: in every recorded SES refusal in this corpus the `ErrorResponse` carries a `RequestId` sibling of `Error`. Its value is a per-request UUID (a run-specific value, never a requirement); what IS required is that the element is PRESENT and carries the id of the request being refused — the same id the SDK surfaces as the response metadata request id and the same id that appears in `<ResponseMetadata><RequestId>` on this service's SUCCESS responses.

Emulator (common to all ten): the refusal body is `<ErrorResponse><Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error></ErrorResponse>` — `RequestId` absent. Replay detail for each cluster is literally `$.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>' or a uuid, got None)`.

Why (common to all ten): the request id is the only handle a caller has on a failed SES call. An SDK reads it into the error's response metadata, so `error.response_metadata.request_id` / `RequestId` comes back empty or None on the emulator; retry/idempotency bookkeeping that keys on it degrades, structured logging and correlation of a refusal with a service-side record is impossible, and any client or test that asserts the error envelope is well formed fails even though the operation refused correctly with the right code. Success responses are unaffected (they pass in this corpus), so the gap is specific to the error path and silently makes refusals less diagnosable than successes.

Fix (common to all ten): the SES service crate's shared awsQuery error serializer / error-envelope writer — the one place that renders `ErrorResponse` from an error variant. It must emit the `RequestId` element after `Error`, populated from the same per-request id source the success-path `ResponseMetadata` writer already uses. No operation handler, gate order or error selection changes: all ten clusters already pick the right error and the right status.

Cause 1 of 10: CreateConfigurationSet refusal (duplicate/invalid configuration set) answers 400 with the correct code and no RequestId. Traces: email-configuration-set-full-lifecycle#task#3.

Cause 2 of 10: CreateConfigurationSetEventDestination refusal answers 400 with the correct code and no RequestId. Traces: email-configuration-set-full-lifecycle#task#6.

Cause 3 of 10: DescribeConfigurationSet on a configuration set the account does not hold answers 400 ConfigurationSetDoesNotExist with no RequestId (Smithy: DescribeConfigurationSet binds ConfigurationSetDoesNotExistException, client, httpError 400, awsQueryError code ConfigurationSetDoesNotExist). Traces: email-configuration-set-full-lifecycle#teardown#16, and the cluster also covers email-configuration-set-lifecycle-and-default-reputation#task#1 and #teardown#6.

Cause 4 of 10: DeleteConfigurationSetTrackingOptions refusal answers 400 with the correct code and no RequestId. Traces: email-configuration-set-tracking-options-without-verified-domain#task#4 (recorded RequestId was a uuid; the requirement is presence, not that value).

Cause 5 of 10: CreateCustomVerificationEmailTemplate refusal answers 400 with the correct code and no RequestId. Traces: email-custom-verification-email-template-workflow#task#2.

Cause 6 of 10: DeleteReceiptRule refusal answers 400 with the correct code and no RequestId. Traces: email-delete-receipt-rule-missing-rule-is-noop#task#3.

Cause 7 of 10: DescribeReceiptRule on an absent rule answers 400 RuleDoesNotExist with no RequestId (Smithy: DescribeReceiptRule binds RuleDoesNotExistException and RuleSetDoesNotExistException, both client, httpError 400, awsQueryError codes RuleDoesNotExist / RuleSetDoesNotExist). Traces: email-delete-receipt-rule-missing-rule-is-noop#task#4, cluster also covers email-receipt-rule-set-lifecycle-and-activation#task#18.

Cause 8 of 10: PutConfigurationSetDeliveryOptions refusal (invalid TlsPolicy) answers 400 with the correct code and no RequestId. Traces: email-delivery-options-invalid-tls-policy-rejected#task#2.

Cause 9 of 10: PutIdentityPolicy refusal answers 400 with the correct code and no RequestId. Traces: email-identity-sending-authorization-policy#task#7.

Cause 10 of 10: DeleteReceiptRuleSet refusal (deleting the active rule set) answers 400 with the correct code and no RequestId. Traces: email-receipt-rule-set-lifecycle-and-activation#task#14.

## Evidence
Replayed corpus simparity-wire-aws-v1 against the booted emulator (emulator_up=true, /__admin/health 200). Each of the ten clusters is verdict=mismatch with expected_status == actual_status == 400 and a single diverging key.

Expected (AWS), shape of EVERY SES v1 refusal body:
  <ErrorResponse xmlns="http://ses.amazonaws.com/doc/2010-12-01/">
    <Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error>
    <RequestId>…</RequestId>          <-- PRESENT; value is the per-request id
  </ErrorResponse>

Actual (emulator): identical except the <RequestId> element is absent.

Per-cluster replay detail (verbatim):
  CreateConfigurationSet                     email-configuration-set-full-lifecycle#task#3
    $.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>', got None)
  CreateConfigurationSetEventDestination     email-configuration-set-full-lifecycle#task#6
    $.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>', got None)
  DescribeConfigurationSet                   email-configuration-set-full-lifecycle#teardown#16
    $.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>', got None)
    affected also: email-configuration-set-lifecycle-and-default-reputation#task#1, #teardown#6
  DeleteConfigurationSetTrackingOptions      email-configuration-set-tracking-options-without-verified-domain#task#4
    $.ErrorResponse.RequestId: missing (expected a uuid, got None)
  CreateCustomVerificationEmailTemplate      email-custom-verification-email-template-workflow#task#2
    $.ErrorResponse.RequestId: missing (expected a uuid, got None)
  DeleteReceiptRule                          email-delete-receipt-rule-missing-rule-is-noop#task#3
    $.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>', got None)
  DescribeReceiptRule                        email-delete-receipt-rule-missing-rule-is-noop#task#4
    $.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>', got None)
    affected also: email-receipt-rule-set-lifecycle-and-activation#task#18
  PutConfigurationSetDeliveryOptions         email-delivery-options-invalid-tls-policy-rejected#task#2
    $.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>', got None)
  PutIdentityPolicy                          email-identity-sending-authorization-policy#task#7
    $.ErrorResponse.RequestId: missing (expected a uuid, got None)
  DeleteReceiptRuleSet                       email-receipt-rule-set-lifecycle-and-activation#task#14
    $.ErrorResponse.RequestId: missing (expected '<shaped-not-asserted>', got None)

Note on values: the recorded RequestId values are per-run uuids (and several fixtures assert only that the element is shaped, '<shaped-not-asserted>'). The requirement asserted here is the PRESENCE of the element carrying the refused request's id — not any recorded uuid.

Smithy grounding: services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json — sdkId SES, endpointPrefix email, protocols [awsQuery], version 2010-12-01, xmlNamespace http://ses.amazonaws.com/doc/2010-12-01/. Errors involved are all errorKind client with httpError 400 and an awsQueryError code (e.g. ConfigurationSetDoesNotExist, RuleDoesNotExist, RuleSetDoesNotExist) — the codes and statuses already match; only the envelope's RequestId sibling is missing.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an SES v1 (Query) client at the data listener, region us-west-2.
2. Drive any ONE of the ten refusals, e.g. the simplest: call DescribeConfigurationSet with ConfigurationSetName set to a name that was never created.
3. Capture the raw HTTP response body (not just the SDK-parsed error).
4. Observe HTTP 400 and `<ErrorResponse><Error><Type>Sender</Type><Code>ConfigurationSetDoesNotExist</Code><Message>…</Message></Error></ErrorResponse>` with NO `<RequestId>` element.
5. AWS answers the same status, type, code and message plus a `<RequestId>` sibling of `<Error>` carrying the id of that request.
6. Repeat for the other nine operations to confirm the serializer, not a handler, is at fault: CreateConfigurationSet (duplicate name), CreateConfigurationSetEventDestination (invalid destination), DeleteConfigurationSetTrackingOptions (no tracking options set), CreateCustomVerificationEmailTemplate (unverified FromEmailAddress), DeleteReceiptRule / DescribeReceiptRule (absent rule), PutConfigurationSetDeliveryOptions (TlsPolicy outside the enum), PutIdentityPolicy (invalid policy), DeleteReceiptRuleSet (the active rule set).
7. Re-replay the ten fixture#step bindings named in this issue (issue_trace/issue_step plus trace_ids) to confirm the fix.
