Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: c96a98ba4351b01bcc3b10297b18cb90664e8dd6

# AddNotificationChannels mints channel ids into the store before the checks that refuse the add, against this crate's own minting rule

_Filed: 2026-10-29T02:23:23Z_
_Operation: AddNotificationChannels | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c96a98ba4351b01bcc3b10297b18cb90664e8dd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AddNotificationChannels` resolves each requested channel first, and for a channel the caller named no `id` for it calls `guard.mint_channel_id(..)` — which WRITES the store (`self.channel_ids_minted = self.channel_ids_minted.wrapping_add(1)`). Only AFTER that loop does the handler run the two checks that refuse the request: the "id already in the configuration" ConflictException and the two-channel ServiceQuotaExceededException.

So a refused add still advances the store's channel-id mint counter, and the id a later SUCCESSFUL add receives depends on how many refused adds preceded it.

This crate states the opposite rule for its other minter, twice, in `crate::state`: the policy writers deliberately run every check under a shared borrow and mint only once the write is going to be applied. The permission path is the sibling that does it right; the notification path is the one that mints first and then decides whether to write at all. The handler's own leading comment also claims the pre-resolution exists so that "a request this operation refuses leaves the stored configuration exactly as it was" — the configuration is indeed untouched, but the store is not: the mint count it holds moved.

The fix is to order this handler the way `remove_permission` is ordered — resolve/validate, apply the duplicate-id and quota checks, then mint the ids for the channels that are actually being stored. That changes no response and no existing assertion: nothing reports `channel_ids_minted`, and the ids stay distinct (`mint_channel_id` is still the only minter and still bumps before deriving).

## Evidence

The crate's stated minting rule — services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/state.rs:1677-1678 (`remove_permission`):
```
        // The checks first, under a shared borrow, so the revision is minted only for a
        // removal that is actually applied.
```
and src/state.rs:1639-1641 (`put_permission`):
```
        // The write is applied from here on, so this is where the revision it carries is
        // minted.
        let revision_id = self.mint_revision_id(group_name, action_group);
```

The minter is a WRITE to the store — src/state.rs:1557-1560:
```
    pub fn mint_channel_id(&mut self, group_name: &str) -> String {
        self.channel_ids_minted = self.channel_ids_minted.wrapping_add(1);
        channel_id(self.channel_ids_minted, now(), group_name)
    }
```
over a documented store field — src/state.rs:1452-1455:
```
    /// How many notification-channel ids this store has minted. Bumped by
    /// [`Self::mint_channel_id`], the only minter, and mixed into every id it derives,
    /// so two channels added in the same instant never carry the same id.
    pub channel_ids_minted: u64,
```

The handler mints BEFORE its refusals — src/ops/add_notification_channels.rs:179-180 and :220-228:
```
        // Resolve every requested channel BEFORE writing any of them, so a request this
        // operation refuses leaves the stored configuration exactly as it was.
...
            let id = match channel.id.clone() {
                Some(named) => named,
                // `Channel$id`: "A random UUID for channelId is used when adding a
                // channel to the notification configuration if not specified in the
                // request." Minted by the STORE, ...
                None => guard.mint_channel_id(&input.profiling_group_name),
            };
```
and only then can refuse — src/ops/add_notification_channels.rs:263-277:
```
        // A channel id already in the configuration names a channel that exists: adding
        // it again would conflict with the stored state rather than add anything.
        if let Some(duplicate) = requested
            .iter()
            .find(|channel| group.has_notification_channel(&channel.id))
        {
            return Err(AddNotificationChannelsErrorModeled::ConflictException(
```
and src/ops/add_notification_channels.rs:283-287:
```
        if group.notification_channels.len() + requested.len()
            > crate::state::MAX_NOTIFICATION_CHANNELS_PER_GROUP
        {
            return Err(
                AddNotificationChannelsErrorModeled::ServiceQuotaExceededException(
```

Test coverage that must stay green and does (none of it observes the mint count): tests/all/notification_configuration.rs:519 `adding_past_the_two_channel_limit_is_refused_and_changes_nothing` and :613 `an_id_the_configuration_already_holds_is_a_conflict` both assert only that "a refused add must leave the configuration unchanged" (:586); tests/all/notification_channels.rs:6-12 asserts "each minted id is distinct from every other this store hands out", which minting after the checks preserves.

## Steps to Reproduce

1. Read src/state.rs:1639-1641 and :1677-1678 — this crate's rule is that an id/revision is minted only for a write that is actually applied.
2. Read src/state.rs:1557-1560 — `mint_channel_id` takes `&mut self` and bumps the stored `channel_ids_minted` (src/state.rs:1452-1455) before deriving the id.
3. Read src/ops/add_notification_channels.rs:220-228 — the mint happens inside the channel-resolution loop, which runs before the ConflictException at :263-277 and the ServiceQuotaExceededException at :283-301.
4. Concretely: create a group, add two channels without ids (succeeds, mint count = 2), then POST a third channel without an id — the add is refused with ServiceQuotaExceededException, yet `channel_ids_minted` is now 3, so the next successful add on any group gets the id derived from count 4 rather than 3.
5. Reorder the handler the way `remove_permission` is ordered (checks first, mint for the channels actually stored) and re-run tests/all/notification_configuration.rs and tests/all/notification_channels.rs — every response and every assertion is unchanged.
