Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: eba09938c5757f116b88f59710a766eb7f22de4f

# An update can store a channelSubtypeConfig holding no channel, the exact record state the deletion path is written to never leave behind

_Filed: 2026-10-28T16:15:36Z_
_Operation: UpdateCampaignChannelSubtypeConfig | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `eba09938c5757f116b88f59710a766eb7f22de4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCampaignChannelSubtypeConfig` writes the request's config onto the record verbatim (`campaign.channel_subtype_config = Some(input.channel_subtype_config);`). Every member of `ChannelSubtypeConfig` is optional (`telephony`, `sms`, `email`, `whatsApp`), and the crate's own Rust type makes all four `Option`, so a body of `{"channelSubtypeConfig": {}}` deserializes to an all-`None` struct and is stored as `Some(<empty config>)`.

That is precisely the record state the sibling deletion path in this same crate refuses to leave behind. `StoredCampaign::clear_channel_subtype_config` collapses the field to `None` the moment the last channel block goes, and its doc states the rule as a property of the RECORD, not of the delete call: "A config left holding no channel at all holds nothing, so the record carries no `channelSubtypeConfig` rather than an empty one". The deletion test asserts exactly that absence against `DescribeCampaign`.

So the two writers of one field disagree about how "configured for no channel" is represented: after the delete the member is absent, after an update carrying `{}` `DescribeCampaign` projects `"channelSubtypeConfig": {}`. A client that round-trips Describe -> Update, or one that branches on the member's presence to decide whether the campaign is configured, sees two different answers for the same stored meaning, and `CampaignSummary.channelSubtypes` is `[]` in both cases so it cannot tell them apart either. The same asymmetry exists for `communicationLimitsOverride` (`UpdateCampaignCommunicationLimits` stores an empty `CommunicationLimitsConfig` as-is while `clear_communication_limits` collapses it to `None`), and `CreateCampaign` stores an empty config the same unnormalised way, which is why the fix belongs in the write path's normalisation rather than in one handler's response.

## Evidence

The update stores the config verbatim — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/update_campaign_channel_subtype_config.rs:218:

```
        campaign.channel_subtype_config = Some(input.channel_subtype_config);
```

All four members are optional, so `{}` is a legal body — src/ops/types/shared_types.rs:16-21:

```
pub struct ChannelSubtypeConfig {
    pub telephony: Option<TelephonyChannelSubtypeConfig>,
    pub sms: Option<SmsChannelSubtypeConfig>,
    pub email: Option<EmailChannelSubtypeConfig>,
    pub whats_app: Option<WhatsAppChannelSubtypeConfig>,
```

The other side of the inconsistency — src/state.rs:886-890 (doc of `clear_channel_subtype_config`) states the rule about the stored record:

```
    /// Only the named channel's block goes: every other channel keeps the block it
    /// was configured with, so [`Self::channel_subtypes`] still reports those. A
    /// config left holding no channel at all holds nothing, so the record carries no
    /// `channelSubtypeConfig` rather than an empty one — the member is optional on
    /// both `Campaign` and `CampaignSummary`'s projection.
```

and src/state.rs:910-917 implements it:

```
        if removed
            && config.email.is_none()
            && config.sms.is_none()
            && config.telephony.is_none()
            && config.whats_app.is_none()
        {
            self.channel_subtype_config = None;
        }
```

The existing test pins the absence for the delete path — tests/all/campaign_channel_subtype_config_deletion.rs:617-624:

```
    // Every channel's block is gone, so the campaign carries no channel subtype
    // config at all rather than an empty one.
    let campaign = describe(&router, region, &id, "every channel deleted").await;
    assert!(
        campaign.get("channelSubtypeConfig").is_none(),
        "expected no channelSubtypeConfig after every channel went, got {:?}",
```

The sibling field shows the same split: src/state.rs:974-976 collapses an emptied override (`if config.all_channel_subtypes.is_none() && config.instance_limits_handling.is_none() { self.communication_limits_override = None; }`, with the doc at src/state.rs:960-963 — "An override left carrying no member at all carries nothing, so the record holds no override rather than an empty one"), while src/ops/update_campaign_communication_limits.rs:245 writes `campaign.communication_limits_override = Some(input.communication_limits_override);` unnormalised. No test in tests/all/campaign_channel_subtype_config_update.rs covers an empty `{}` config, so nothing pins the current behaviour.

## Steps to Reproduce

1. Onboard a Connect instance and create a campaign carrying a `channelSubtypeConfig` with a `telephony` block.
2. Call `POST /v2/campaigns/{id}/channel-subtype-config` with body `{"channelSubtypeConfig": {}}` — it is accepted (200), since every member of `ChannelSubtypeConfig` is optional.
3. Call `GET /v2/campaigns/{id}`: the response carries `"channelSubtypeConfig": {}` — a config holding no channel.
4. Now reach the same meaning through the other path: recreate the campaign with a single `telephony` block and call `DELETE /v2/campaigns/{id}/channel-subtype-config?channelSubtype=TELEPHONY`, then `GET /v2/campaigns/{id}`: the `channelSubtypeConfig` member is absent, as src/state.rs:888-890 and tests/all/campaign_channel_subtype_config_deletion.rs:620-623 require.
5. Repeat steps 2-3 against `POST /v2/campaigns/{id}/communication-limits` with `{"communicationLimitsOverride": {}}` versus `DELETE /v2/campaigns/{id}/communication-limits?config=ALL_CHANNEL_SUBTYPES` for the same split on `communicationLimitsOverride`.
