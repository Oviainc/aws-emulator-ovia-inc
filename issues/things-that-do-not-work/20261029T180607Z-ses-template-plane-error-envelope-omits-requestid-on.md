Issue-Source: trace-sweep-divergence
Issue-Trace: email-email-template-lifecycle-and-rendering
Issue-Step: task#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-email-template-lifecycle-and-rendering
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses|UpdateTemplate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: ses|GetTemplate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# SES template-plane error envelope omits RequestId on UpdateTemplate and GetTemplate

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTemplate

## Description
2 net-new wire causes on the SES email-template plane (these two clusters are reported under the `ses` service token rather than the `email` endpoint-prefix token the rest of the SES corpus uses; the service is the same SimpleEmailService 2010-12-01 / awsQuery surface — sigv4Name and arnNamespace are both `ses`, endpointPrefix is `email`). Both are the SAME root cause as the configuration-set/receipt-rule issue 'SES v1 Query error envelope omits the RequestId element on every refusal': the awsQuery error serializer never emits the `RequestId` sibling of `Error`. They are filed separately because an issue carries ONE service token and these clusters report `ses`; a fixer should expect one serializer change to close all of them, but must re-replay these two bindings specifically.

AWS (both causes): Smithy services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json — SimpleEmailService, version 2010-12-01, protocols [awsQuery], xmlNamespace http://ses.amazonaws.com/doc/2010-12-01/, sdkId SES, endpointPrefix email, sigv4Name ses. The model does not spell the envelope out, so this is recorded behavior only: every recorded refusal body is `<ErrorResponse xmlns="http://ses.amazonaws.com/doc/2010-12-01/"><Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error><RequestId>…</RequestId></ErrorResponse>`. The RequestId VALUE is a per-request id (a run-specific value, never a requirement); its PRESENCE, carrying the id of the request being refused, is the requirement — the same id this service puts in `<ResponseMetadata><RequestId>` on success.

Emulator (both causes): the refusal body is `<ErrorResponse><Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error></ErrorResponse>` — no RequestId element. Status (400) and Error.Code match on both clusters; the body differs only by that missing key.

Why (both causes): the request id is the caller's only handle on a failed template call. An SDK surfaces nothing for the error's request id, so a client cannot correlate a template refusal with a service-side record, retry/idempotency bookkeeping keyed on it degrades, and any assertion that the error envelope is well formed fails even though the refusal is otherwise correct. Success responses on this plane pass, so refusals are strictly less diagnosable than successes.

Fix (both causes): the SES crate's shared awsQuery error-envelope serializer — emit the `RequestId` element after `Error`, populated from the same per-request id source the success-path `ResponseMetadata` writer already uses. No template handler, gate order or error selection changes.

Cause 1 of 2: UpdateTemplate on a template the account does not hold answers 400 with the right code and no RequestId. Traces: email-email-template-lifecycle-and-rendering#task#13.

Cause 2 of 2: GetTemplate on a deleted/absent template answers 400 with the right code and no RequestId. Traces: email-email-template-lifecycle-and-rendering#teardown#15.

## Evidence
Replayed corpus simparity-wire-aws-v1 against the booted emulator (emulator_up=true). Both clusters are verdict=mismatch with expected_status == actual_status == 400 and one diverging key.

Cause 1 — UpdateTemplate, fixture email-email-template-lifecycle-and-rendering, step task#13, service token 'ses':
  $.ErrorResponse.RequestId: missing (expected a request-id value, got None)

Cause 2 — GetTemplate, fixture email-email-template-lifecycle-and-rendering, step teardown#15, service token 'ses':
  $.ErrorResponse.RequestId: missing (expected a request-id value, got None)

Expected (AWS) body shape for both:
  <ErrorResponse xmlns="http://ses.amazonaws.com/doc/2010-12-01/">
    <Error><Type>Sender</Type><Code>TemplateDoesNotExist</Code><Message>…</Message></Error>
    <RequestId>…</RequestId>        <-- PRESENT
  </ErrorResponse>
Actual (emulator): identical except <RequestId> is absent.

Note on values: the recorded RequestId values in these two steps are fixture placeholders / per-run ids and are NOT part of the requirement — only the presence of the element carrying the refused request's id is.

Smithy grounding: service_info for services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json reports sdkId SES, version 2010-12-01, protocols ['awsQuery'], sigv4Name 'ses', awsService.endpointPrefix 'email', xmlNamespace http://ses.amazonaws.com/doc/2010-12-01/ — which is why the same service appears in the replay under both the 'email' and the 'ses' token. The template-absence errors are errorKind client with httpError 400 and an awsQueryError code; the status and code already match.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an SES v1 (Query) client at the data listener, region us-west-2.
2. Cause 1: call UpdateTemplate with Template.TemplateName set to a name that was never created (or was already deleted), with a subject and a text/HTML part.
3. Capture the RAW HTTP response body, not just the SDK-parsed error.
4. Observe HTTP 400 and `<ErrorResponse><Error><Type>Sender</Type><Code>TemplateDoesNotExist</Code><Message>…</Message></Error></ErrorResponse>` with NO `<RequestId>` element.
5. Cause 2: DeleteTemplate that name (or use a never-created name) and call GetTemplate on it; observe the same 400 refusal with no `<RequestId>`.
6. For contrast, call a SUCCEEDING template operation (e.g. CreateTemplate then GetTemplate) and note that the success body does carry ResponseMetadata/RequestId — the gap is on the error path only.
7. AWS answers the same status, type, code and message plus a `<RequestId>` sibling of `<Error>` carrying that request's id.
8. Re-replay email-email-template-lifecycle-and-rendering#task#13 and email-email-template-lifecycle-and-rendering#teardown#15 to confirm the fix.
