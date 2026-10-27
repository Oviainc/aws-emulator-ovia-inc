Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# PutProfileOutboundRequestBatch stores eventTriggerContext.sourceEvent but silently drops eventTriggerContext.channelContext

_Filed: 2026-10-27T23:07:10Z_
_Operation: PutProfileOutboundRequestBatch | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ProfileOutboundRequest$eventTriggerContext` is modelled as `EventTriggerContext`, which has TWO members: `sourceEvent` and `channelContext` (a `ChannelContext` carrying a `WebNotificationContext` with `sessionId` and `browserId`). The handler reads only `sourceEvent` off the context and passes it to the store; `channelContext` is never read, never passed and has no field on the stored record, so everything the caller sent under it is discarded.

This contradicts the rule the crate states for the sibling member of the very same structure: `StoredProfileOutboundRequest::source_event` is documented as kept precisely because the record is meant to carry what was submitted ("Part of the record, not of any response"), and `take_profile_outbound_request` is documented as taking "the profile outbound request submitted under `client_token`". The record the store keeps is therefore not the request that was submitted: one half of `eventTriggerContext` is persisted under that rule and the other half is thrown away.

## Evidence

Model (smithy_query shape="EventTriggerContext"):
```
"EventTriggerContext": {"type": "structure", "members": {
    "sourceEvent": {"target": "SourceEvent", "length": {"min": 1}},
    "channelContext": {"target": "ChannelContext"}}}
"ChannelContext": {"type": "structure", "members": {"webNotificationContext": {"target": "WebNotificationContext"}}}
"WebNotificationContext": {"type": "structure", "members": {
    "sessionId": {"target": "SessionId", "length": {"min": 1, "max": 36}},
    "browserId": {"target": "BrowserId", "length": {"min": 1, "max": 36}}}}
```
The deserialized member exists in this crate — src/ops/types/put_profile_outbound_request_batch_types.rs:207,219-220:
```
    pub channel_context: Option<ChannelContext>,
...
    pub session_id: Option<String>,
    pub browser_id: Option<String>,
```

Handler side — src/ops/put_profile_outbound_request_batch.rs:230-246 reads `source_event` ONLY and never touches `channel_context`:
```
            // The event this request was triggered by, when the caller named one: part
            // of the record, so a later reader of the buffer has what was submitted.
            let source_event = request
                .event_trigger_context
                .as_ref()
                .and_then(|context| context.source_event.as_deref());
            let outcome = store.take_profile_outbound_request(
                crate::state::CampaignRef { region: &region, id: &input.id },
                &request.client_token,
                &request.profile_id,
                request.expiration_time,
                source_event,
                now,
            );
```

Store side — src/state.rs:696-716: `StoredProfileOutboundRequest` has fields `id`, `profile_id`, `expires_at`, `source_event` and no field for the channel context, with the rule the drop breaks stated on the sibling member:
```
    /// The `eventTriggerContext$sourceEvent` the request carried, when it carried one:
    /// the event this request was triggered by. Part of the record, not of any response
    /// — no modelled member of this operation's output carries it.
    pub source_event: Option<String>,
```

Test coverage: tests/all/profile_outbound_requests.rs:64-79 SENDS the member in every request body —
```
            "channelContext": {
                "webNotificationContext": {"sessionId": "s-1", "browserId": "b-1"}
            }
```
— but no assertion anywhere in that suite pins the drop, so nothing justifies it.

## Steps to Reproduce

1. Onboard a Connect instance, create a campaign and StartCampaign it so it is Running.
2. PUT /v2/campaigns/{id}/profile-outbound-requests with profileOutboundRequests[0].eventTriggerContext = {"sourceEvent": "Campaign-WebNotification", "channelContext": {"webNotificationContext": {"sessionId": "s-1", "browserId": "b-1"}}}.
3. The request is accepted and a ProfileOutboundRequestId is minted.
4. Inspect the stored record (src/state.rs StoredProfileOutboundRequest): `source_event` holds "Campaign-WebNotification" while sessionId/browserId were never stored — the buffered record is not the request that was submitted, contrary to the rule stated on `source_event`.
