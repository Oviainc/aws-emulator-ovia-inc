Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# In-transit messages are not region-scoped: a message is readable and updatable through every region's endpoint

_Filed: 2026-10-24T20:05:54Z_
_Operation: GetRawMessageContent | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

WorkMail Message Flow is a regional data plane: the client talks to `workmailmessageflow.<region>.amazonaws.com` and the messageId it is handed belongs to a WorkMail organization that lives in exactly one region. A messageId that is in transit in us-east-1 therefore does not exist as far as the eu-west-1 endpoint is concerned.

Expected (AWS): GET /messages/{messageId} signed for a region other than the region in which the message is in transit answers HTTP 404 with `x-amzn-errortype: ResourceNotFoundException` (the message simply is not in that region's message flow). The same request signed for the message's own region answers HTTP 200 with the MIME bytes.

Actual (emulator): the store keys messages by `messageId` alone and records no region for them at all, so the handler answers HTTP 200 with the stored MIME bytes for a request signed for ANY region. `crate::state::InTransitMessage` (src/state.rs lines 126-142: `message_id`, `content`, `frozen`, `last_s3_reference`, `updated_at`) has no region field, and `GetRawMessageContent::handle` looks the message up with `state.messages.get(&input.message_id)` without consulting the request's region — even though the crate already computes the request region (`crate::state::request_region`, src/state.rs lines 218-222) and already uses it for the S3-object location rule in PutRawMessageContent.

The same single-namespace lookup makes PutRawMessageContent cross-region too: `eligibility()` (put_raw_message_content.rs lines 289-298) keys on messageId only, so an update signed for eu-west-1 installs content into a message that is in transit in us-east-1 (where AWS would answer ResourceNotFoundException). That symptom is the same gap and is reported here rather than as a second issue.

The fix is to make the message namespace regional: record the region a message is in transit in when it is put in transit (today the in-process seed helper, tomorrow whatever cross-service producer the open producer issue adds) and resolve both operations' lookups against `request_region(ctx.parts())`, answering ResourceNotFoundException with the existing `unknown_message_id` wording when the id is not in transit in the request's region.

## Evidence

Smithy model: the service's `awsService` endpointPrefix/sigv4 signing name is `workmailmessageflow` with sigv4 auth, i.e. a regional endpoint with the region in the credential scope; both bound operations address a message only by the `messageId` @httpLabel and model `ResourceNotFoundException` (HTTP 404, "The requested email message is not found.") as the answer for a message that is not there. Well-established AWS semantics: a regional data-plane resource (a WorkMail organization's in-transit message) is not addressable through another region's endpoint.

Handler/state: services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/src/ops/get_raw_message_content.rs lines 136-156 — the request region is never read (only the AKID is extracted) and the lookup is `state.messages.get(&input.message_id)`. src/ops/put_raw_message_content.rs line 160 computes `request_region` but uses it only in `resolve_content` for the S3 object's bucket region (lines 351-376); `eligibility()` (lines 289-298) and `state.replace_message_content` (line 253) are region-blind. src/state.rs lines 126-165 — `InTransitMessage` carries no region and `messages: HashMap<String, InTransitMessage>` is a single global namespace, while the peer record `S3Object` (lines 92-97) deliberately does carry `region`.

Test coverage: tests/all/raw_message_content.rs signs every Get/Put with `REGION = "us-east-1"` except `an_object_resolves_only_in_the_region_the_request_was_made_in` (lines 804-860), which varies the region solely to exercise the S3 bucket-region rule on PutRawMessageContent. No test seeds a message "in" one region and reads it from another, so the cross-region read is neither asserted nor pinned.

Not covered by the open issues: 20261024T101352Z (no emulator-reachable producer) is about nothing being able to create a message at all; 20261024T015715Z (resolve the S3Reference against the shared S3 state) and 20261024T045353Z (bucket policy) are about the S3 object; 20261024T170349Z is about MIME criteria; the IAM issue is about authorization. None of them makes the message namespace regional.

## Steps to Reproduce

1. Mount the WorkMailMessageFlow service on the shared engine and put a message in transit under messageId `00000000-0000-0000-0000-000000000001` with well-formed MIME content, as the us-east-1 message flow would (today only the in-process seed helper can do this).
2. Call GET /messages/00000000-0000-0000-0000-000000000001 with SigV4 credentials scoped to `us-east-1/workmailmessageflow` — expect HTTP 200 with the MIME bytes (this works today).
3. Repeat the same GET with credentials scoped to `eu-west-1/workmailmessageflow` (host workmailmessageflow.eu-west-1.amazonaws.com).
4. Expected (AWS): HTTP 404, x-amzn-errortype ResourceNotFoundException, body {"message":"Unknown messageId: 00000000-0000-0000-0000-000000000001"} — the message is not in transit in that region.
5. Actual: HTTP 200 with the full MIME payload, because the lookup ignores the request region entirely.
6. Now POST /messages/00000000-0000-0000-0000-000000000001 with eu-west-1 credentials and a resolvable s3Reference whose object is in eu-west-1: expected HTTP 404 ResourceNotFoundException (no such message in eu-west-1); actual HTTP 200 and a following us-east-1 GET reports the content installed from the other region.
