Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: notifications.list-notification-hubs-maxresults-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.notifications#Notifications
Issue-Key: notifications.list-notification-hubs-maxresults-out-of-range-rejected|run#6|AssertionError|assert message == expected_high, f"expected message {expected_high!r}, got {message!r}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# notifications: ValidationException message for ListNotificationHubs maxResults range includes the offending value, diverging from AWS's message format

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNotificationHubs

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: the range-constraint ValidationException message rendered for ListNotificationHubs' `maxResults` embeds the rejected value, where AWS's message for this member names only the member and the constraint. Failing check: notifications.list-notification-hubs-maxresults-out-of-range-rejected.

AWS: Smithy (services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json) models ListNotificationHubsRequest.maxResults as Integer with @range{min:3, max:3}, bound as httpQuery `maxResults`, and binds ValidationException (errorKind client, httpError 400) to the operation. The model fixes the code (ValidationException) and the status (400) but not the message text; the message rule here is request-derived — it is the standard "N validation error detected: ..." validation rendering that names the offending member and the violated constraint. The recorded AWS behavior for this member is, for both bounds:
  high: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 3"
  low:  "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 3"
i.e. the segment is "Value at '<member>'", with no rendering of the submitted value. maxResults=3 (the single permitted value) is accepted and returns 200 with `notificationHubs`.

Emulator: for maxResults=5 the emulator returns the right error code (ValidationException) but the message "1 validation error detected: Value '5' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 3" — it interpolates the rejected value as "Value '5' at ..." instead of the bare "Value at ..." form. The check asserts the high-bound message first and fails there, so the symmetric low-bound (maxResults=1) message and the maxResults=3 positive control were not reached and are presumably affected by the same renderer.

Why: a client that asserts on, matches, or logs/classifies the validation message for this member — including the behavioral contract tests and any parity harness that diffs validation output — sees a different message shape than AWS produces for the same rejected request, so a request real AWS rejects with one rendering is rejected with another. The code and status are right, so the blast radius is message-shape fidelity only.

Fix: the range-constraint validation message renderer used by the notifications ListNotificationHubs input validation (the operation's validation path in services/service-com-amazonaws-notifications-notifications/src/ops/list_notification_hubs.rs and whatever shared validation-message helper it calls) must emit the "Value at '<member>'" form for this member's @range violations rather than interpolating the submitted value, for both the greater-than-or-equal and less-than-or-equal bounds.

Traces: notifications.list-notification-hubs-maxresults-out-of-range-rejected.

## Evidence
Check: notifications.list-notification-hubs-maxresults-out-of-range-rejected [mismatch]

AssertionError at check_notifications_list_notification_hubs_maxresults_out_of_range_rejected.py line 28:
  expected message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 3"
  got      message "1 validation error detected: Value '5' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 3"

Matching (not a divergence): error code ValidationException, HTTP 400.
Diverging: the message's leading segment — AWS "Value at '<member>'" vs emulator "Value '<submitted>' at '<member>'".

Smithy evidence: ListNotificationHubs.maxResults @range{min:3,max:3}; errors include ValidationException (client, httpError 400); http GET /notification-hubs, 200 on success; paginated pageSize=maxResults.

## Steps to Reproduce
1. Boot the emulator and build a notifications client pinned to us-east-1 with botocore parameter validation DISABLED (botocore.config.Config(parameter_validation=False)) so the out-of-range value reaches the wire instead of being rejected client-side by the SDK's own @range enforcement.
2. Call ListNotificationHubs with maxResults=5. Observe ClientError: code ValidationException (correct), message "1 validation error detected: Value '5' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 3"; AWS returns the same code with "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 3".
3. Call ListNotificationHubs with maxResults=1 and compare against AWS's "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 3".
4. Positive control: ListNotificationHubs with maxResults=3 must succeed and return a `notificationHubs` key.
