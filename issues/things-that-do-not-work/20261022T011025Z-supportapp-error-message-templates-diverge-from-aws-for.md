Issue-Source: trace-sweep-divergence
Issue-Trace: support-app-slack-channel-configuration-validation-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: support-app-slack-channel-configuration-validation-errors support-app-put-account-alias-validation-errors support-app-delete-slack-channel-configuration-not-found support-app-update-slack-channel-configuration-not-found
Issue-Shape-Id: com.amazonaws.supportapp#SupportApp
Issue-Key: supportapp|CreateSlackChannelConfiguration|4xx->4xx|$.message
Issue-Key: supportapp|PutAccountAlias|4xx->4xx|$.message
Issue-Key: supportapp|DeleteSlackChannelConfiguration|4xx->4xx|$.message
Issue-Key: supportapp|UpdateSlackChannelConfiguration|4xx->4xx|$.message
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# supportapp: error message templates diverge from AWS for validation and not-found

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSlackChannelConfiguration

## Description
Shared fault: the supportapp emulator raises the correct exception with the correct HTTP status and error type, but renders the exception's `message` member with its own internal phrasing instead of the template AWS produces. Each of the three message rules below is a known rule — one is stated in the Smithy documentation, one is the standard constraint-validation template built from the request, and one is a request-parameterized not-found template confirmed by two independent recordings — so the template, not the recorded literal text, is the requirement. 3 causes, all must be fixed.

Cause 1 of 3: CreateSlackChannelConfiguration severity/notification-flag validation message.
AWS: Smithy documents the rule on CreateSlackChannelConfigurationRequest.notifyOnCaseSeverity: "If you specify high or all, you must specify true for at least one of the following parameters: notifyOnAddCorrespondenceToCase, notifyOnCreateOrReopenCase, notifyOnResolveCase" (NotificationSeverityLevel values: none|all|high). Violating it is ValidationException (Smithy: ValidationException, httpError 400). AWS's message restates that documented rule in service prose and names the operation: "Your CreateSlackChannelConfiguration request isn't valid. If NotifyOnCaseSeverity is all or high, you must enter true for at least one of the following parameters: NotifyOnCreateOrReopenCase, NotifyOnAddCorrespondenceToCase or NotifyOnResolveCase. Try your request again."
Emulator: same status 400 and error type, but message "notifyOnCaseSeverity high requires at least one of notifyOnAddCorrespondenceToCase, notifyOnCreateOrReopenCase or notifyOnResolveCase to be true" — an internal paraphrase that does not name the operation and does not use the documented AWS wording.
Why: the message is the only part of a ValidationException that tells a caller which rule it broke; SDK/CLI users and log-scraping or message-asserting clients see text no AWS deployment produces, so a client developed against the emulator ships an error path that never matches production text.
Fix: the CreateSlackChannelConfiguration input validation's message construction — emit the documented AWS template (operation-named prefix, the three parameter names, trailing "Try your request again.") for both the all and high severity cases.
Traces: support-app-slack-channel-configuration-validation-errors#task#1 and #task#2 (both steps of the cluster, severity all and high).

Cause 2 of 3: PutAccountAlias constraint-violation message echoes the anchored model pattern.
AWS: Smithy PutAccountAlias.accountAlias is required, target awsAccountAlias with @length 1..30 and @pattern ^[\\w\\- ]+$; a violation is ValidationException (400). The message is the standard constraint-validation template built from the request: "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", and the pattern AWS echoes is the UNANCHORED form [\\w\\- ]+.
Emulator: produces the same template and the same status, but echoes the anchored pattern ^[\\w\\- ]+$ taken verbatim from the model, i.e. "...Member must satisfy regular expression pattern: ^[\\w\\- ]+$".
Why: the echoed pattern is the part of the template derived from the API contract, so a client that surfaces or re-uses it (to pre-validate input, or to show the user the accepted shape) gets a pattern string AWS never returns; message-equality assertions against the standard template fail.
Fix: the shared constraint-validation message renderer used by supportapp input validation — strip the leading ^ and trailing $ anchors from the model @pattern before interpolating it into the "Member must satisfy regular expression pattern:" clause.
Traces: support-app-put-account-alias-validation-errors#task#1.

Cause 3 of 3: Slack channel configuration not-found message is not AWS's request-parameterized template.
AWS: DeleteSlackChannelConfiguration and UpdateSlackChannelConfiguration both bind ResourceNotFoundException (Smithy: httpError 404, doc "The specified resource is missing or doesn't exist, such as an account alias, Slack channel configuration, or Slack workspace configuration"). The recordings show the message is built from the request with one fixed template across both operations: "Slack channel configuration for team <teamId> and channel <channelId> not found" — the same template with each fixture's own teamId/channelId substituted, which is what establishes it as a rule rather than one recorded string.
Emulator: returns the correct 404 and error type but inverts the template into its own phrasing, "no Slack channel configuration exists for channel <channelId> in workspace <teamId>" — different wording, different ordering, and it calls the team a "workspace".
Why: both operations are the point where a client reconciles configuration it believes exists; the message is what it logs or shows. Against the emulator that text differs from AWS for every account, and it mislabels the teamId as a workspace, which misdirects the operator fixing the condition.
Fix: the ResourceNotFoundException message construction shared by the DeleteSlackChannelConfiguration and UpdateSlackChannelConfiguration handlers — render AWS's template, interpolating the request's teamId and channelId in that order.
Traces: support-app-delete-slack-channel-configuration-not-found#task#1, support-app-update-slack-channel-configuration-not-found#task#1.

## Evidence
Cause 1 — fixture support-app-slack-channel-configuration-validation-errors, steps task#1 and task#2, CreateSlackChannelConfiguration, status 400 on both sides:
  expected message: "Your CreateSlackChannelConfiguration request isn't valid. If NotifyOnCaseSeverity is all or high, you must enter true for at least one of the following parameters: NotifyOnCreateOrReopenCase, NotifyOnAddCorrespondenceToCase or NotifyOnResolveCase. Try your request again."
  actual message:   "notifyOnCaseSeverity high requires at least one of notifyOnAddCorrespondenceToCase, notifyOnCreateOrReopenCase or notifyOnResolveCase to be true"
  rule grounding: Smithy doc on CreateSlackChannelConfigurationRequest.notifyOnCaseSeverity states the all/high requirement verbatim.

Cause 2 — fixture support-app-put-account-alias-validation-errors, step task#1, PutAccountAlias, status 400 on both sides:
  expected message: "1 validation error detected: Value 'bad alias!' at 'accountAlias' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w\\- ]+"
  actual message:   "...Member must satisfy regular expression pattern: ^[\\w\\- ]+$"
  rule grounding: standard constraint template built from the request member + its @pattern; model pattern is ^[\\w\\- ]+$, AWS echoes it unanchored.

Cause 3 — status 404 on both sides, same template divergence in two fixtures:
  support-app-delete-slack-channel-configuration-not-found#task#1 (DeleteSlackChannelConfiguration):
    expected "Slack channel configuration for team <teamId> and channel <channelId> not found"
    actual   "no Slack channel configuration exists for channel <channelId> in workspace <teamId>"
  support-app-update-slack-channel-configuration-not-found#task#1 (UpdateSlackChannelConfiguration): identical expected/actual templates with that fixture's own ids, which is what shows the AWS text is a request-parameterized template and not an incidental string.

In all three causes the HTTP status and the exception type already match; only the `message` rendering diverges.

## Steps to Reproduce
Cause 1: POST /control/create-slack-channel-configuration with notifyOnCaseSeverity="high" (then "all") and notifyOnCreateOrReopenCase/notifyOnAddCorrespondenceToCase/notifyOnResolveCase all false or absent. Compare the 400 body's message to AWS's documented template. Replay support-app-slack-channel-configuration-validation-errors (task#1, task#2).

Cause 2: POST /control/put-account-alias with accountAlias="bad alias!" (violates @pattern). Compare the echoed pattern in the 400 message: emulator prints ^[\\w\\- ]+$, AWS prints [\\w\\- ]+. Replay support-app-put-account-alias-validation-errors#task#1.

Cause 3: POST /control/delete-slack-channel-configuration (and /control/update-slack-channel-configuration) with a teamId/channelId pair that was never configured. Compare the 404 body's message to "Slack channel configuration for team <teamId> and channel <channelId> not found". Replay support-app-delete-slack-channel-configuration-not-found#task#1 and support-app-update-slack-channel-configuration-not-found#task#1.
