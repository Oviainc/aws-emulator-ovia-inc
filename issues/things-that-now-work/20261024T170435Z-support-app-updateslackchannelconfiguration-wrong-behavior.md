Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ad9d9cbded3020216cba71d97cb223a025446b5b

# Apply the `none` severity rule only to notification flags the update request carries

_Filed: 2026-10-24T17:04:35Z_
_Operation: UpdateSlackChannelConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ad9d9cbded3020216cba71d97cb223a025446b5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateSlackChannelConfiguration` evaluates the documented `notifyOnCaseSeverity` / notification-flag consistency rule over the MERGED configuration (request values OR-ed with the stored record). For the `none` side of the rule the model documents a narrower scope on the UPDATE request than on the CREATE request, so the emulator refuses requests the spec allows.

Model doc for `UpdateSlackChannelConfigurationRequest$notifyOnCaseSeverity`:
- "If you specify `none`, **any of the following parameters that you specify in your request** must be `false`: notifyOnAddCorrespondenceToCase, notifyOnCreateOrReopenCase, notifyOnResolveCase"
- note: "If you don't specify these parameters in your request, the Amazon Web Services Support App uses the current values by default."

Compare `CreateSlackChannelConfigurationRequest$notifyOnCaseSeverity`, which is worded over the whole configuration ("the following parameters must be null or false") because a create has no prior values. The update wording is deliberately scoped to the flags the request carries.

Expected: an update carrying only `{teamId, channelId, notifyOnCaseSeverity: "none"}` for a configuration whose stored `notifyOnCreateOrReopenCase` is `true` names no flag in the request, so the `none` clause constrains nothing — the request is accepted (HTTP 200) and the severity is persisted.
Actual: HTTP 400 `ValidationException` ("Your UpdateSlackChannelConfiguration request isn't valid. If NotifyOnCaseSeverity is none, the following parameters must be null or false: ...") and nothing is persisted, because the handler merges the stored flags in before evaluating the rule.

The same merge-then-validate logic also refuses an update that only turns the single true flag off while the stored severity stays `high`/`all`; that side of the rule is worded without the "that you specify in your request" qualifier, so it is at least arguable — the `none` side is not.

## Evidence

Spec: `smithy_query(query="shape", shape="UpdateSlackChannelConfigurationRequest")` — member `notifyOnCaseSeverity` documents "If you specify none, any of the following parameters **that you specify in your request** must be false", plus the note that unspecified parameters keep their current values. The CREATE request's same member is worded differently ("must be null or false"), which is the wording the emulator implements for both.

Handler: `services/service-com-amazonaws-supportapp-supportapp/src/ops/update_slack_channel_configuration.rs` builds `merged` from request-or-stored values (lines ~224-242) and then calls `crate::state::notification_rule_violation(Self::NAME, &merged.notify_on_case_severity, merged.notify_on_create_or_reopen_case.unwrap_or(false), ...)` (lines ~246-259), i.e. the rule is evaluated over stored flags the request never mentioned. `src/state.rs::notification_rule_violation` is shared verbatim with the create handler, so update inherits create's wider rule.

Tests: the current behavior is asserted the other way round, so no test covers the documented scope — `tests/all/slack_channel_configurations.rs` row "severity to none while a stored flag is true" expects 400, and `tests/all/error_message_templates.rs::the_none_severity_rule_is_stated_in_the_same_service_prose` asserts the 400 message for the same request. Those assertions need to change with the fix.

## Steps to Reproduce

1. POST /control/register-slack-workspace-for-organization {"teamId":"T012ABCDEFG"}
2. POST /control/create-slack-channel-configuration {"teamId":"T012ABCDEFG","channelId":"C01234A5BCD","notifyOnCaseSeverity":"high","notifyOnCreateOrReopenCase":true,"channelRoleArn":"arn:aws:iam::123456789012:role/SupportAppRole"} -> 200
3. POST /control/update-slack-channel-configuration {"teamId":"T012ABCDEFG","channelId":"C01234A5BCD","notifyOnCaseSeverity":"none"} (no notification flag in the request)
4. Observe HTTP 400 ValidationException; the documented rule only constrains flags the request specifies, so the update should be accepted and the new severity reported by the update response and by a later ListSlackChannelConfigurations
