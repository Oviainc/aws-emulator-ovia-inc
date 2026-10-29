Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 4db25e6bf87f4906c725859fd3714c6300682a7e

# PutOutboundRequestBatch drops every channelSubtypeParameters value: the buffered dial request keeps no destination, attributes or template

_Filed: 2026-10-28T15:02:29Z_
_Operation: PutOutboundRequestBatch | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `4db25e6bf87f4906c725859fd3714c6300682a7e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutOutboundRequestBatch` persists only three things per accepted request — the minted `DialRequestId`, a derived channel-subtype string and the `expirationTime` — and throws away the whole of `OutboundRequest$channelSubtypeParameters`, the member that carries WHO the request dials and WITH WHAT. The handler reads the union only to classify it into one of four channel names (`requested_channel_subtype`), then hands that string to the store; `destinationPhoneNumber`, `attributes`, `connectSourcePhoneNumber`, `answerMachineDetectionConfig`, `ringTimeout` (telephony), `destinationEmailAddress`/`connectSourceEmailAddress` (email), `templateArn`/`templateParameters`/`connectSourcePhoneNumberArn` (sms, whatsApp) are all deserialized, validated by the boundary walk, and then dropped on the floor.

This contradicts the sibling batch operation in the same crate. `StoredProfileOutboundRequest` deliberately keeps what its request carried — the `profileId` it is to reach AND `eventTriggerContext$sourceEvent`, the latter stored with the explicit reason that it is "Part of the record, not of any response" / "so a later reader of the buffer has what was submitted". `StoredOutboundRequest` keeps no equivalent: the record of a pending dial request holds nothing that identifies the contact it was submitted for, so the buffer that the crate documents as the campaign's record of its pending requests cannot say what any of them is. The two buffers are described as parallel (`take_profile_outbound_request` is documented as deciding "exactly as `take_outbound_request` decides a dial request"), yet one is a record of its requests and the other is a record of counts and deadlines only.

Nothing in the crate states that dropping the dial parameters is intended: the handler's module doc enumerates the operation's whole effect (buffer, idempotency, per-request failure codes) and never mentions the parameters, and no comment marks them as deliberately unstored.

## Evidence

The model's member that is dropped — `smithy_query(query="shape", shape="OutboundRequest")`:

```
"OutboundRequest": {"type": "structure", "members": {
  "clientToken": {..."required": true},
  "expirationTime": {..."required": true},
  "channelSubtypeParameters": {"target": "ChannelSubtypeParameters", "required": true}},
  "doc": "An outbound request for a campaign."}
```
and `smithy_query(query="shape", shape="TelephonyChannelSubtypeParameters")`:
```
"destinationPhoneNumber": {"target": "DestinationPhoneNumber", "required": true, ... "sensitive": true},
"attributes": {"target": "Attributes", "required": true, "sensitive": true, "type": "map"},
"connectSourcePhoneNumber": {...}, "answerMachineDetectionConfig": {...}, "ringTimeout": {...}
```
The deserialized type the crate builds, so the values DO reach the handler — src/ops/types/put_outbound_request_batch_types.rs:202-210:
```
#[smithy_macros::smithy(shape = "com.amazonaws.connectcampaignsv2#TelephonyChannelSubtypeParameters")]
pub struct TelephonyChannelSubtypeParameters {
    pub destination_phone_number: String,
    pub attributes: std::collections::HashMap<String, String>,
    pub connect_source_phone_number: Option<String>,
    pub answer_machine_detection_config: Option<AnswerMachineDetectionConfig>,
    pub ring_timeout: Option<i32>,
}
```
The only use the handler makes of the union — src/ops/put_outbound_request_batch.rs:156-163:
```
fn requested_channel_subtype(parameters: &ChannelSubtypeParameters) -> &'static str {
    match parameters {
        ChannelSubtypeParameters::Telephony(_) => "TELEPHONY",
```
and the store call that carries only the channel name forward — src/ops/put_outbound_request_batch.rs:259-268:
```
            let outcome = store.take_outbound_request(
                crate::state::CampaignRef { region: &region, id: &input.id },
                &request.client_token,
                channel_subtype,
                request.expiration_time,
                now,
            );
```
The record that results — src/state.rs:673-686 (three fields, no destination):
```
/// An outbound request a campaign has taken, as stored.
pub struct StoredOutboundRequest {
    pub id: String,
    /// The `ChannelSubtype` the request was submitted for.
    pub channel_subtype: String,
    /// The `expirationTime` the caller sent...
    pub expires_at: ::aws_smithy_types::DateTime,
}
```
The sibling record in the same file that DOES keep what was submitted — src/state.rs:698-716:
```
pub struct StoredProfileOutboundRequest {
    pub id: String,
    /// The `profileId` the request was submitted for: the Customer Profiles profile the
    /// campaign is to reach.
    pub profile_id: String,
    pub expires_at: Option<::aws_smithy_types::DateTime>,
    /// The `eventTriggerContext$sourceEvent` the request carried, when it carried one:
    /// the event this request was triggered by. Part of the record, not of any response
    /// — no modelled member of this operation's output carries it.
    pub source_event: Option<String>,
}
```
and the sibling handler stating the principle — src/ops/put_profile_outbound_request_batch.rs:230-231:
```
            // The event this request was triggered by, when the caller named one: part
            // of the record, so a later reader of the buffer has what was submitted.
```
Nothing else in the crate stores these values: `grep -n "destination_phone_number|attributes|ring_timeout|template_parameters|answer_machine" src/` matches only `src/ops/types/*.rs` (the wire structs) and `src/ops/types/shared_types.rs:95-96`, never `src/state.rs`.

Test coverage: tests/all/instance_onboarding_tags_and_outbound_requests.rs:845-899 submits batches that vary exactly these values (`telephony_request("token-two", "15555550101", json!({"CustomerId": "c-1"}))`, …) but asserts only `clientToken`/`id` pairs and failure codes (`pairs("successfulRequests", "id")`, line 840), so nothing pins the drop — persisting the parameters alongside the id changes no assertion in the suite.

## Steps to Reproduce

1. Create an Amazon Connect instance, then `PUT /v2/connect-instance/{id}/onboarding` and wait out `crate::state::ONBOARDING_JOB_DURATION` so the job reports SUCCEEDED.
2. `PUT /v2/campaigns` with that `connectInstanceId`, a name, and a `channelSubtypeConfig` holding a `telephony` block; keep the returned campaign `id`.
3. `POST /v2/campaigns/{id}/start` so the campaign is `Running`.
4. `PUT /v2/campaigns/{id}/outbound-requests` with `outboundRequests: [{"clientToken":"t1","expirationTime":"<future>","channelSubtypeParameters":{"telephony":{"destinationPhoneNumber":"+15555550100","attributes":{"CustomerId":"c-1"},"ringTimeout":30}}}]` — answered 200 with a minted `DialRequestId`.
5. Inspect what the call persisted (`crate::state::StoredCampaign::outbound_requests`, src/state.rs:656): the entry under `"t1"` is `StoredOutboundRequest { id, channel_subtype: "TELEPHONY", expires_at }` — the phone number, the attributes map and the ring timeout are nowhere in the store.
6. Contrast with `PUT /v2/campaigns/{id}/profile-outbound-requests` carrying `{"clientToken":"p1","profileId":"<32 hex>","eventTriggerContext":{"sourceEvent":"e-1"}}`: `StoredProfileOutboundRequest` (src/state.rs:698) keeps both the `profileId` and the `sourceEvent`, although no response member carries either.
