Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f6f033d1876a0cb978cad56e1292384a0bfb4098

# The SNS notification channel is stored once per account, not per account/Region, although its own doc and all three handlers say per account/Region

_Filed: 2026-10-28T19:45:36Z_
_Operation: PutNotificationChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f6f033d1876a0cb978cad56e1292384a0bfb4098`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsFmsAwsfms20180101State::notification_channel` is a single `Option<NotificationChannelRecord>` and the record carries NO Region field, so the one channel is shared by every Region the emulator serves. The three notification-channel handlers all read the request's Region (`crate::state::request_region(ctx.parts())`) but only to build the authorization refusal — none of them uses it to select or key the channel.

That contradicts what this crate says about the resource in three places: the record's own doc calls it "ONE per account/Region", and the `GetNotificationChannel` / `DeleteNotificationChannel` handlers both say the channel they act on is "the one the account/Region has". Since `GetNotificationChannelRequest` and `DeleteNotificationChannelRequest` model NO members at all, the request's account/Region is the ONLY thing that can select which channel a call acts on — so storing one channel for the whole account is exactly the case the comments rule out. The justification the record's doc gives is also backwards: a resource that is one per account/Region is what WOULD need a map keyed by Region; an `Option` can only ever hold one per account.

Observable consequences, all with no second channel ever designated:

* `PutNotificationChannel` signed for `us-east-1` makes `GetNotificationChannel` signed for `eu-central-1` answer 200 with the `us-east-1` topic and role, where that Region has designated nothing and the modelled `ResourceNotFoundException` is the only answer the handler has for "no channel".
* `DeleteNotificationChannel` signed for `eu-central-1` removes the channel designated in `us-east-1`, so a later `GetNotificationChannel` in `us-east-1` answers `ResourceNotFoundException` — state lost in a Region the caller never addressed.
* `PutNotificationChannel` in a second Region silently overwrites the first Region's channel instead of designating its own.

The same crate takes the opposite approach for its other regional resources: `PolicyRecord` and `ResourceSetRecord` each carry a `region` field, documented as held "because it is state in its own right: a policy is a regional resource". The notification channel is the one regional configuration that skips it.

## Evidence

The rule the crate states for this record — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:383-394:
```
/// The Firewall Manager SNS notification channel: the topic Firewall Manager
/// records its activity to, and the role that lets it publish there. ONE per
/// account/Region, which is why it is an `Option` rather than a map.
#[derive(Debug, Clone)]
pub struct NotificationChannelRecord {
    pub sns_topic_arn: String,
    pub sns_role_name: String,
    /// When `PutNotificationChannel` last wrote the channel. Recorded because
    /// the channel is a configuration with a history, though no modelled member
    /// of `GetNotificationChannelResponse` reports it.
    pub configured_at: SystemTime,
}
```
(no `region` member)

The storage that breaks it — src/state.rs:1113-1114:
```
    /// The single SNS notification channel, if one was configured.
    pub notification_channel: Option<NotificationChannelRecord>,
```

The writer reads the Region only for the gate and stores nothing of it — src/ops/put_notification_channel.rs:186-200:
```
        let mut store = state.lock();
        // The authorization gate, BEFORE the payload is stored.
        crate::state::require_administrator(&store, &crate::state::request_region(ctx.parts()))?;
...
        store.notification_channel = Some(crate::state::NotificationChannelRecord {
            sns_topic_arn: input.sns_topic_arn.clone(),
            sns_role_name: input.sns_role_name.clone(),
            configured_at: ::std::time::SystemTime::now(),
        });
```

The readers claim the Region selects the channel while reading the single slot — src/ops/get_notification_channel.rs:149-151 and 165-172:
```
        // `GetNotificationChannel` takes no members: the channel is the one the
        // account/Region has.
        let _ = &input;
...
        let Some(channel) = store.notification_channel.as_ref() else {
            return Err(
                GetNotificationChannelErrorModeled::ResourceNotFoundException(
                    crate::state::resource_not_found(),
                )
                .into(),
            );
        };
```
src/ops/delete_notification_channel.rs:169-171 and 185-190:
```
        // `DeleteNotificationChannel` takes no members: the channel it removes is
        // the one the account/Region has.
        let _ = &input;
...
        // Taking the channel out of the store IS the delete: that record is the
        // only copy, so a following `GetNotificationChannel` finds nothing. With
        // no channel stored there is no channel resource to remove, which is
        // `ResourceNotFoundException` ...
        if store.notification_channel.take().is_none() {
```

The sibling pattern this one skips — src/state.rs:328-332:
```
/// `region` is the Region the policy was created in — the one its ARN names.
/// It is held as a field rather than read back out of the ARN because it is
/// state in its own right: a policy is a regional resource, and whether it is
/// in an administrator's Region scope
/// ([`ComAmazonawsFmsAwsfms20180101State::policy_status`]) is decided from it.
```
and src/state.rs:404-409 for `ResourceSetRecord::region` ("for the same reason [`PolicyRecord::region`] is: a resource set is a regional resource").

Test coverage today: tests/all/notification_channel_deletion.rs drives both `REGION` and `OTHER_REGION` (line 272) but builds a FRESH emulator per row (line 274), so no test ever designates a channel in one Region and reads or deletes it from another — nothing pins the cross-Region sharing, and the suite's own assertions (a designated channel reads back, a deleted one is gone, a refused delete leaves it stored) all stay true under a Region-keyed store.

## Steps to Reproduce

1. Boot the emulator and call `AssociateAdminAccount` with `{"AdminAccount": "123456789012"}` so the gate is open.
2. Call `PutNotificationChannel` signed for `us-east-1` with `{"SnsTopicArn": "arn:aws:sns:us-east-1:123456789012:fms-alerts", "SnsRoleName": "arn:aws:iam::123456789012:role/fms-sns"}` → 200.
3. Call `GetNotificationChannel` (no members) signed for `eu-central-1`. Expected: HTTP 400 `ResourceNotFoundException`, because that Region has designated no channel. Actual: HTTP 200 reporting the `us-east-1` topic and role.
4. Call `DeleteNotificationChannel` (no members) signed for `eu-central-1` → 200.
5. Call `GetNotificationChannel` signed for `us-east-1` again. Expected: 200 with the channel designated in step 2. Actual: HTTP 400 `ResourceNotFoundException` — the delete in a Region that had no channel removed another Region's channel.
6. Variant: after step 2, call `PutNotificationChannel` signed for `eu-central-1` with a different topic, then `GetNotificationChannel` signed for `us-east-1` — it reports the `eu-central-1` topic, the first Region's designation having been overwritten.
