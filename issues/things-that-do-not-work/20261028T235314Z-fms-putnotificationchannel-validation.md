Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 964c2ed1f803ffeae6e06375ca6c19c7cce42b54

# PutNotificationChannel stores SnsTopicArn / SnsRoleName without the ARN-form check state.rs says every ResourceArn gets, so GetNotificationChannel reports back a string that is not an ARN

_Filed: 2026-10-28T23:53:14Z_
_Operation: PutNotificationChannel | Class: validation_

> _Note: this issue was identified against the code base at commit `964c2ed1f803ffeae6e06375ca6c19c7cce42b54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both members of `PutNotificationChannelRequest` target the model's `com.amazonaws.fms#ResourceArn` and are documented as ARNs. The handler stores them verbatim and applies no check at all, on the stated ground that "there is nothing here to validate beyond the modelled length and pattern the boundary already enforced".

That ground is contradicted by this crate's own `crate::state::arn_format_error`, whose doc says the opposite in as many words: the modelled `ResourceArn` pattern "accepts any string of letters, digits and punctuation", so Firewall Manager checks the FORM of a `ResourceArn` itself and answers a 400 `ValidationException` for a string that is not an ARN. Every other handler in the crate that takes a caller-supplied `ResourceArn` — TagResource, UntagResource and ListTagsForResource — calls that helper as its first act. PutNotificationChannel is the only door onto a caller-given `ResourceArn` that skips it.

The consequence is observable: `PutNotificationChannel {"SnsTopicArn": "not-an-arn", "SnsRoleName": "x"}` is answered 200 and persisted, and the following GetNotificationChannel reports `SnsTopicArn: "not-an-arn"` as "The Amazon Resource Name (ARN) of the SNS topic" — a stored configuration the crate's own ARN rule says could never have been accepted.

## Evidence

The skip, with the comment that states the (contradicted) rule — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_notification_channel.rs:190`:
```
        // Both members are modelled `ResourceArn`s and are stored exactly as the
        // caller sent them: Firewall Manager's own checks on them (that the topic
        // policy allows the role to publish, that the role trusts
        // fms.amazonaws.com) are about resources of OTHER services, which this
        // emulator does not hold, so there is nothing here to validate beyond the
        // modelled length and pattern the boundary already enforced.
        store.notification_channel = Some(crate::state::NotificationChannelRecord {
            sns_topic_arn: input.sns_topic_arn.clone(),
            sns_role_name: input.sns_role_name.clone(),
```

The crate's own rule about a `ResourceArn` — `services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:2166`:
```
/// The reason `arn` is not an Amazon Resource Name at all, or `None` when it is
/// shaped like one.
///
/// Firewall Manager checks the FORM of a `ResourceArn` before it looks anything
/// up, and the modelled `ResourceArn` pattern cannot express it (that pattern
/// accepts any string of letters, digits and punctuation), so this check lives
/// here and answers a 400 `ValidationException` ...
pub fn arn_format_error(arn: &str) -> Option<String> {
```

The three siblings that apply it — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/tag_resource.rs:178`:
```
        // The ARN's FORM FIRST: a string that is not an ARN at all is a caller
        // programming error, refused before anything is looked up or written.
        // The modelled `ResourceArn` pattern cannot express this, so the service
        // check does — as a 400 `ValidationException`, never a 501.
        if let Some(reason) = crate::state::arn_format_error(&input.resource_arn) {
            return Err(crate::scaffolded::Scaffolded::validation(reason));
        }
```
the identical block at `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/untag_resource.rs:171`, and `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/list_tags_for_resource.rs:181`.

The model side (smithy_query, operation=PutNotificationChannel): `"SnsTopicArn": {"target": "ResourceArn", "required": true, "pattern": "^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$"}` and `"SnsRoleName": {"target": "ResourceArn", "required": true, ...}`; member docs: "The Amazon Resource Name (ARN) of the SNS topic that collects notifications from Firewall Manager." and "The Amazon Resource Name (ARN) of the IAM role that allows Amazon SNS to record Firewall Manager activity."

The stored value is reported straight back — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_notification_channel.rs:173`:
```
        Ok(GetNotificationChannelOutput {
            sns_topic_arn: Some(channel.sns_topic_arn.clone()),
            sns_role_name: Some(channel.sns_role_name.clone()),
        })
```
No test in `tests/all/notification_channel_deletion.rs` or elsewhere in the suite pins a non-ARN topic value as acceptable.

## Steps to Reproduce

1. AssociateAdminAccount so the administrator gate passes.
2. Call PutNotificationChannel with `{"SnsTopicArn": "not-an-arn", "SnsRoleName": "also-not-an-arn"}` — both satisfy the modelled length 1..1024 and the `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` pattern, so the boundary validator passes them.
3. Observed: HTTP 200 and the channel is persisted. Expected (the rule state.rs:2166 states and tag_resource.rs:178 / untag_resource.rs:171 / list_tags_for_resource.rs:181 apply to the same `ResourceArn` shape): a 400 `ValidationException` worded by `arn_format_error` ("Incorrect arn format. error:ARNs must start with 'arn:': not-an-arn").
4. Call GetNotificationChannel: it reports `SnsTopicArn: "not-an-arn"` for a member documented as "The Amazon Resource Name (ARN) of the SNS topic".
