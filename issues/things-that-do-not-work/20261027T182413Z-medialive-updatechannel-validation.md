Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f2ead5306d42c2c14ef865f57d355d9f40696e5a

# channelSecurityGroups is stored without resolving the ids, skipping the input-security-group resolution the crate's own input writers apply

_Filed: 2026-10-27T18:24:13Z_
_Operation: UpdateChannel | Class: validation_

> _Note: this issue was identified against the code base at commit `f2ead5306d42c2c14ef865f57d355d9f40696e5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateChannel` (and `CreateChannel`) writes the request's `ChannelSecurityGroups` list verbatim onto the channel record with no resolution against the account's input security groups. The model documents the member as "A list of IDs for all the Input Security Groups attached to the channel" — the same kind of reference as an input's `InputSecurityGroups` — and the crate already owns the resolution rule for that reference and applies it on the input side: `ComAmazonawsMedialiveMedialiveState::input_configuration_error` refuses any `inputSecurityGroups` entry that `live_input_security_group` does not resolve, and both `CreateInput` and `UpdateInput` run it before writing.

The stated rationale for that check applies verbatim to the channel side, because the channel side has the mirror derivation: `channels_using_security_group` derives `InputSecurityGroup$Channels` ("The list of channels currently using this Input Security Group as their channel security group") from `channel.channel_security_groups`. So a channel updated to name an id nothing minted (or a group that was deleted) is left "claiming a resource no read of the account can show" — exactly the condition the crate's own doc says the resolution exists to prevent — while the group side reports nothing.

The gap is not a missing feature: the helper (`live_input_security_group`), the shared refusal wording (`unknown_reference_message`) and the sibling call site all already exist in this crate; the channel writers simply do not call them.

## Evidence

The unresolved write — `services/service-com-amazonaws-medialive-medialive/src/ops/update_channel.rs:307-309`:
```
if let Some(security_groups) = input.channel_security_groups {
    channel.channel_security_groups = security_groups;
}
```
and `src/ops/create_channel.rs:299`: `channel.channel_security_groups = input.channel_security_groups.unwrap_or_default();`

The sibling path that resolves the same reference — `src/state.rs:3262-3269`:
```
for group in &input.security_groups {
    if self.live_input_security_group(group).is_none() {
        return Some(unknown_reference_message(
            "inputSecurityGroups",
            "input security group",
            group,
        ));
    }
}
```
called by `src/ops/update_input.rs:295-300` ("The same rule `CreateInput` applies, to the merged record: an update may no more leave an input naming a resource this account does not hold ... than a create may create one.") and by `CreateInput`.

The crate's own stated rationale — `src/state.rs:3233-3239`:
```
/// * every id the configuration names must resolve to a resource this
///   account holds — an input security group, an SdiSource, a claimed input
///   device. Those resources are what the input ingests through, and each
///   derives the inputs using it from this store
///   ([`Self::inputs_using_security_group`], [`Self::inputs_using_sdi_source`]),
///   so an id nothing minted would leave the input claiming a resource no
///   read of the account can show.
```
The mirror derivation that makes the channel side identical — `src/state.rs:2472-2492`:
```
/// The ids of the channels currently using `group_id` as one of their
/// channel security groups — `InputSecurityGroup$Channels` ("The list of
/// channels currently using this Input Security Group as their channel
/// security group"). Derived from the channel store ...
pub fn channels_using_security_group(&self, group_id: &str) -> Vec<String> {
```
Model text: `smithy_query(query="doc", shape="com.amazonaws.medialive#CreateChannelRequest", member="ChannelSecurityGroups")` -> "A list of IDs for all the Input Security Groups attached to the channel."

Test coverage: `tests/all/channel_update.rs:186-187` round-trips `{"channelSecurityGroups": ["3000000", "3000001"]}` under the rule "every member an update supplies is read back" — it pins persistence, not acceptance of unminted ids, so the fix needs those rows to name groups a `CreateInputSecurityGroup` actually minted (as `tests/all/input_security_group.rs:351` and `:1012` already do). No test asserts a refusal for an unknown `channelSecurityGroups` id.

## Steps to Reproduce

1. POST /prod/channels with {"name":"demo"}; poll GET /prod/channels/<id> until state is IDLE
2. PUT /prod/channels/<id> with {"channelSecurityGroups":["3000000"]} where no CreateInputSecurityGroup ever minted id 3000000 — expected the same refusal UpdateInput gives for an unknown inputSecurityGroups entry (400 BadRequestException, message from state::unknown_reference_message, nothing written); the emulator answers 200 and stores it
3. GET /prod/channels/<id> — the channel reports channelSecurityGroups ["3000000"]
4. GET /prod/inputSecurityGroups — no group with that id exists, so no InputSecurityGroup$Channels list can ever report this channel: the channel claims a group no read of the account can show
5. For comparison: PUT /prod/inputs/<input-id> with {"inputSecurityGroups":["3000000"]} is refused 400 by src/ops/update_input.rs:295-300
