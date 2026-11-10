Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1ecf6f83c17663b1cceb804b029a144947c92d6a

# Update applies the `none` severity clause from the STORED severity, refusing a flag change the request's own severity never triggers

_Filed: 2026-11-10T06:13:00Z_
_Operation: UpdateSlackChannelConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1ecf6f83c17663b1cceb804b029a144947c92d6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateSlackChannelConfiguration` evaluates the documented `notifyOnCaseSeverity` `none` clause against the MERGED severity — i.e. the value the stored record already carries when the request does not specify one — while scoping the flags the clause reads to only those the request carries. The model's sentence makes the clause conditional on the REQUEST specifying `none` ("If you specify none, any of the following parameters **that you specify in your request** must be false"), and the handler's own module doc applies exactly that reasoning to the flags ("a flag the request omits keeps its current value and is not something the request says, so it is not constrained") — but not to the severity, which it takes from the store.

Consequence: a configuration stored with `notifyOnCaseSeverity = none` and all flags false cannot be moved to notifying at all by a request that only turns a flag on. `UpdateSlackChannelConfiguration {teamId, channelId, notifyOnResolveCase: true}` (no `notifyOnCaseSeverity`) is refused 400 `ValidationException`, even though the request specifies no severity and therefore says nothing the `none` clause constrains.

The refusal is also inconsistent with the end state this same handler already admits: the crate's own test shows that an update specifying `notifyOnCaseSeverity: "none"` over a record whose `notifyOnCreateOrReopenCase` is true is ACCEPTED and persists `(none, [true,false,false])`. So a configuration with severity `none` and a true flag is a state this emulator reaches and reports; reaching the symmetric state from the other direction (flip the flag, leave severity alone) is refused. One of the two paths must be wrong, and it is the one the model's conditional does not cover.

## Evidence

Model (smithy_query doc, `UpdateSlackChannelConfigurationRequest`, member `notifyOnCaseSeverity`), also reproduced in the handler header at `src/ops/update_slack_channel_configuration.rs:81-88`:
  "If you specify none, any of the following parameters that you specify in your request must be false: notifyOnAddCorrespondenceToCase notifyOnCreateOrReopenCase notifyOnResolveCase If you don't specify these parameters in your request, the Amazon Web Services Support App uses the current values by default."

The handler's own stated rule — `services/service-com-amazonaws-supportapp-supportapp/src/ops/update_slack_channel_configuration.rs:11-14`:
  "the `none` clause over only the notification flags the request itself specifies — a flag the request omits keeps its current value and is not something the request says, so it is not constrained."

The code that breaks it for the severity — same file, `:314-316` and `:329-337`:
```
            notify_on_case_severity: input
                .notify_on_case_severity
                .unwrap_or_else(|| stored.notify_on_case_severity.clone()),
...
        if let Some(message) = crate::state::notification_rule_violation(
            Self::NAME,
            &merged.notify_on_case_severity,   // <- STORED severity when the request omits it
            ...
            requested_flags,
        ) {
```
and the clause itself, `src/state.rs:640-643`:
```
        "none" if in_scope_for_none.any_set() => format!(
            "If NotifyOnCaseSeverity is none, the following parameters must be null \
             or false: {create_or_reopen}, {add_correspondence} or {resolve}."
        ),
```

Both sides of the repo inconsistency — the same end state is admitted by a sibling path. `tests/all/update_severity_rule_scope.rs:14-16` states the scoping rule:
  "`none`: \"any of the following parameters **that you specify in your request** must be false\" — worded over the request, so a flag the request does not carry is not constrained by it at all"
yet the expectation it computes takes the severity from the store, `tests/all/update_severity_rule_scope.rs:113-118`:
```
    let effective_severity = severity.unwrap_or(stored.severity);
...
        "none" => requested.contains(&Some(true)),
```
and `tests/all/update_severity_rule_scope.rs:272` pins that `(none, [true,false,false])` is an accepted, persisted configuration:
```
    assert_configuration(case, &reply.body, "none", [true, false, false]);
```
Coverage: `tests/all/update_severity_rule_scope.rs::each_clause_of_the_severity_rule_is_applied_over_its_documented_scope` sweeps this row (stored fixture "none, notifying on nothing", request severity `None`, request flags `[None,None,Some(true)]`) and currently asserts the 400; its `refusal_expected` helper needs the same correction as the handler.

## Steps to Reproduce

1. Register the console-authorized workspace: POST /control/register-slack-workspace-for-organization with {"teamId":"T012ABCDEFG"} (account in an organization) -> 200.
2. POST /control/create-slack-channel-configuration with {"teamId":"T012ABCDEFG","channelId":"C01234A5BCD","notifyOnCaseSeverity":"none","channelRoleArn":"arn:aws:iam::<emulator-account>:role/SupportAppRole"} and no notification flags -> 200; the stored record is severity `none` with all three flags false.
3. POST /control/update-slack-channel-configuration with {"teamId":"T012ABCDEFG","channelId":"C01234A5BCD","notifyOnResolveCase":true} — no `notifyOnCaseSeverity` member at all.
4. Observe HTTP 400 ValidationException "If NotifyOnCaseSeverity is none, the following parameters must be null or false: ...", although the request specified no severity, so the clause's documented antecedent ("If you specify none") was never met.
5. Contrast the admitted path: create the same pair with {"notifyOnCaseSeverity":"high","notifyOnResolveCase":true}, then POST /control/update-slack-channel-configuration with {"teamId":...,"channelId":...,"notifyOnCaseSeverity":"none"} -> 200, and ListSlackChannelConfigurations reports severity `none` with notifyOnResolveCase true — exactly the state step 3 was refused for.
