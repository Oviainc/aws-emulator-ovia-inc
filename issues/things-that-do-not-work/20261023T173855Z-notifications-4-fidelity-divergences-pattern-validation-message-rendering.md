Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: notifications.associate-channel-invalid-arn-pattern-rejected notifications.create-event-rule-invalid-source-pattern-rejected notifications.create-notification-configuration-invalid-name-pattern-rejected notifications.get-notification-configuration-invalid-arn-validation notifications.notification-configuration-lifecycle-create-active-delete notifications.list-notification-configurations-filters-by-status notifications.tag-resource-exceeds-max-tags-rejected notifications.notification-hub-register-active-deregister-removes-it
Issue-Shape-Id: com.amazonaws.notifications#Notifications
Issue-Key: notifications.associate-channel-invalid-arn-pattern-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: notifications.create-event-rule-invalid-source-pattern-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: notifications.create-notification-configuration-invalid-name-pattern-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: notifications.get-notification-configuration-invalid-arn-validation|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: notifications.notification-configuration-lifecycle-create-active-delete|run#8|AssertionError|assert created["status"] == "INACTIVE", f"unexpected initial status {created['status']!r}"
Issue-Key: notifications.list-notification-configurations-filters-by-status|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: notifications.tag-resource-exceeds-max-tags-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: notifications|DeregisterNotificationHub|ResourceNotFoundException
Issue-Commit: a91eb31cf888327ed8ad916c03309e3893f84f70

# notifications: 4 fidelity divergences — pattern-validation message rendering, new-configuration status, TagResource quota message, hub not-found message

> _Note: this issue was identified against the code base at commit `a91eb31cf888327ed8ad916c03309e3893f84f70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNotificationConfiguration, GetNotificationConfiguration, ListNotificationConfigurations, AssociateChannel, CreateEventRule, TagResource, DeregisterNotificationHub

## Description
The notifications (AWS User Notifications) emulator is standing up and answering, but 4 distinct root causes make its responses diverge from real AWS. All 4 are in the same service and are reported here as one bundle; a fixer must close ALL FOUR numbered causes below before submitting.

=== Cause 1 of 4: @pattern ValidationException message is rendered with the offending value and with the regex anchors ===
AWS: Smithy (services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json) constrains AssociateChannel.arn @pattern ^arn:[a-z-]{3,10}:(chatbot|consoleapp|notifications-contacts):[a-zA-Z0-9-]*:[0-9]{12}:[a-zA-Z0-9-_.@]+/[a-zA-Z0-9/_.@:-]+$, CreateEventRule.source @pattern ^aws.([a-z0-9\-])+$, CreateNotificationConfiguration.name @pattern ^[A-Za-z0-9_\-]+$, GetNotificationConfiguration.arn @pattern ^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$; each op models ValidationException (httpError 400). The model does not state the message text, so the message rule here is recorded behavior only, but it is a deterministic template built from the request member and its modelled constraint: AWS answers HTTP 400 ValidationException with "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" where (a) the rejected VALUE is not echoed, and (b) the pattern is rendered WITHOUT its leading ^ and trailing $ anchors.
Emulator: returns the right code (ValidationException) but renders both differently — it echoes the rejected value and keeps the anchors, e.g. "1 validation error detected: Value 'not-a-valid-arn' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$" (and likewise "Value 'ec2' at 'source' ... ^aws.([a-z0-9\-])+$", "Value 'sim2real behav <id>!' at 'name' ... ^[A-Za-z0-9_\-]+$", "Value 'arn:aws:sns:...:not-a-supported-channel-type' at 'arn' ... ^arn:...(chatbot|consoleapp|notifications-contacts)...$"). Both deviations come from one renderer and are one fix, but BOTH must be corrected.
Why: a client that parses or asserts on the standard AWS validation-error text (console surfaces, SDK retry/classification wrappers, and test suites that pin the AWS message) gets text AWS never emits; echoing the submitted value also leaks request data into the error that AWS deliberately omits for these members.
Fix: the shared @pattern constraint-validation message formatter used by the notifications op handlers' input validation — strip ^/$ from the rendered pattern and omit the value segment.
Failing checks: notifications.associate-channel-invalid-arn-pattern-rejected, notifications.create-event-rule-invalid-source-pattern-rejected, notifications.create-notification-configuration-invalid-name-pattern-rejected, notifications.get-notification-configuration-invalid-arn-validation.

=== Cause 2 of 4: a freshly created NotificationConfiguration reports ACTIVE instead of INACTIVE, and is therefore invisible to the status=INACTIVE list filter ===
AWS: Smithy NotificationConfigurationStatus enumerates ACTIVE/PARTIALLY_ACTIVE/INACTIVE/DELETING and documents ACTIVE as "All of the EventRules are in ACTIVE Status" and INACTIVE as "All of the EventRules are in INACTIVE Status". A configuration created by CreateNotificationConfiguration owns zero EventRules, so the recorded AWS behavior is status INACTIVE in the CreateNotificationConfiguration response (HTTP 201) and in GetNotificationConfiguration; ListNotificationConfigurations?status=INACTIVE returns it and ?status=ACTIVE excludes it.
Emulator: CreateNotificationConfiguration returns status 'ACTIVE' for a brand-new, rule-less configuration, and the configuration consequently never appears under the status=INACTIVE list filter (the list check polled ListNotificationConfigurations?status=INACTIVE for the new ARN and timed out at 60s).
Why: status is the client's signal that a configuration is actually wired up to deliver notifications. A client sees a config that delivers nothing as ACTIVE, and status-based inventory/reconciliation queries (status=INACTIVE to find unfinished configs) return the wrong set, so automation skips configurations that still need EventRules.
Fix: the CreateNotificationConfiguration handler and the notifications state model — derive status from the configuration's EventRule set (zero rules => INACTIVE) rather than hard-coding ACTIVE, and make sure GetNotificationConfiguration and the ListNotificationConfigurations status filter read that derived value.
Failing checks: notifications.notification-configuration-lifecycle-create-active-delete (assert created['status'] == 'INACTIVE' -> got 'ACTIVE'), notifications.list-notification-configurations-filters-by-status (timed out waiting for the new config under status=INACTIVE).

=== Cause 3 of 4: TagResource over-quota rejection uses the generic length-constraint message instead of the properties-count message ===
AWS: Smithy TagResource.tags targets TagMap with @length min 0 max 200 and the op models ValidationException (httpError 400). Recorded behavior: submitting 201 tag entries is rejected HTTP 400 ValidationException with the JSON-schema-style message "[object has too many properties (found 201 but schema requires at most 200)]" — a template built from the request (the submitted entry count) and the modelled maximum. No tags are applied.
Emulator: returns ValidationException (code correct) but with the generic member-length text "1 validation error detected: Value '201 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 200".
Why: the tags map is validated by AWS as an object-property-count rule, not a member-length rule; a client (or a test/parity harness) matching on the AWS quota text gets a message AWS never produces for this member, and the synthetic value rendering "'201 entries'" is not an AWS construct at all.
Fix: the TagResource handler's input validation for the tags map — emit the properties-count message for a map that exceeds TagMap's maximum instead of routing it through the generic @length formatter.
Failing check: notifications.tag-resource-exceeds-max-tags-rejected.

=== Cause 4 of 4: DeregisterNotificationHub ResourceNotFoundException carries non-AWS message text ===
AWS: Smithy DeregisterNotificationHub takes notificationHubRegion as an httpLabel and models ResourceNotFoundException (httpError 404). Recorded behavior: deregistering a region with no registered hub returns 404 ResourceNotFoundException with the request-derived message "notification hub with region <notificationHubRegion> not found" (the model does not state the text — recorded behavior only — but it is templated on the request's region).
Emulator: returns ResourceNotFoundException (code and status correct) with "no NotificationHub is registered in region <region>" (observed: 'no NotificationHub is registered in region eu-west-1' where AWS says 'notification hub with region eu-west-1 not found'). The register -> ACTIVE -> deregister -> removed lifecycle itself behaved correctly; only the not-found message diverges.
Why: a client surfacing or matching the AWS not-found text for hub deregistration sees emulator-specific prose; the same wording family is used across the service's not-found paths, so the divergence is systematic rather than one string.
Fix: the DeregisterNotificationHub handler's not-found error construction (and the shared notifications not-found message helper it uses).
Failing check: notifications.notification-hub-register-active-deregister-removes-it.

Note: an open issue, 20261023T000909Z-notificationscontacts-error-message-fidelity-divergences-in-validation-and.md, records the same CLASS of message-fidelity defects in the sibling service notificationscontacts. It is a different service and does not cover any of the causes above. The unimplemented-stub issues for AssociateChannel, CreateEventRule, TagResource, ListTagsForResource and ListManagedNotificationConfigurations are already filed separately and are not re-filed here.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): notifications 3/26 passed, 23 failed.

Cause 1 — expected vs actual error messages (code ValidationException, HTTP 400, in all four):
  get-notification-configuration-invalid-arn-validation:
    expected: "1 validation error detected: Value at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}"
    actual:   "1 validation error detected: Value 'not-a-valid-arn' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$"
  create-event-rule-invalid-source-pattern-rejected:
    expected: "...Value at 'source' failed to satisfy constraint: Member must satisfy regular expression pattern: aws.([a-z0-9\\-])+"
    actual:   "...Value 'ec2' at 'source' failed to satisfy constraint: Member must satisfy regular expression pattern: ^aws.([a-z0-9\\-])+$"
  create-notification-configuration-invalid-name-pattern-rejected:
    expected: "...Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9_\\-]+"
    actual:   "...Value 'sim2real behav 21510680cbe7!' at 'name' failed ... pattern: ^[A-Za-z0-9_\\-]+$"
  associate-channel-invalid-arn-pattern-rejected:
    expected: "...Value at 'arn' failed ... pattern: arn:[a-z-]{3,10}:(chatbot|consoleapp|notifications-contacts):[a-zA-Z0-9-]*:[0-9]{12}:[a-zA-Z0-9-_.@]+/[a-zA-Z0-9/_.@:-]+"
    actual:   "...Value 'arn:aws:sns:us-west-2:123456789012:not-a-supported-channel-type' at 'arn' failed ... pattern: ^arn:...$"
  (pattern bodies above are the modelled ones; the two deviations are the echoed value and the retained ^/$ anchors, not the pattern itself.)

Cause 2:
  notification-configuration-lifecycle-create-active-delete:
    AssertionError: unexpected initial status 'ACTIVE'  (assert created["status"] == "INACTIVE")
  list-notification-configurations-filters-by-status:
    Error: Check timed out after 60.0s — polling ListNotificationConfigurations(status="INACTIVE", maxResults=100) for the ARN just returned by CreateNotificationConfiguration never saw it.

Cause 3:
  tag-resource-exceeds-max-tags-rejected (code asserted ValidationException and MATCHED; message diverged):
    expected: '[object has too many properties (found 201 but schema requires at most 200)]'
    actual:   "1 validation error detected: Value '201 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 200"

Cause 4:
  notification-hub-register-active-deregister-removes-it (code asserted ResourceNotFoundException and MATCHED; message diverged):
    expected: 'notification hub with region eu-west-1 not found'
    actual:   'no NotificationHub is registered in region eu-west-1'
  All earlier assertions in that check passed: register -> status ACTIVE in ListNotificationHubs -> deregister -> absent from ListNotificationHubs.

Smithy consulted (services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json): CreateNotificationConfiguration (POST /notification-configurations, 201; name @length 1..64 @pattern ^[A-Za-z0-9_\\-]+$; tags -> TagMap @length 0..200; output status required, NotificationConfigurationStatus), GetNotificationConfiguration (GET /notification-configurations/{arn}), ListNotificationConfigurations (GET, httpQuery status), AssociateChannel (POST /channels/associate/{arn}), CreateEventRule (POST /event-rules; source @pattern ^aws.([a-z0-9\\-])+$), TagResource (POST /tags/{arn}; tags required TagMap @length 0..200), DeregisterNotificationHub (DELETE /notification-hubs/{notificationHubRegion}); ValidationException httpError 400, ResourceNotFoundException httpError 404; NotificationConfigurationStatus member docs for ACTIVE/INACTIVE.

## Steps to Reproduce
Boot the emulator and point a boto3 'notifications' client at it (region override is a no-op against the emulator's shared endpoint).

Cause 1:
1. notif.get_notification_configuration(arn="not-a-valid-arn") -> expect HTTP 400 ValidationException whose message omits the value and renders the pattern unanchored; observe the echoed value and the ^...$ anchors.
2. Repeat with notif.create_notification_configuration(name="sim2real behav 1!", description="x"), notif.create_event_rule(notificationConfigurationArn=<valid arn>, source="ec2", eventType="X", regions=["us-east-1"]), and notif.associate_channel(arn="arn:aws:sns:us-west-2:123456789012:not-a-supported-channel-type", notificationConfigurationArn=<valid arn>) — same two deviations on 'name', 'source' and 'arn'.

Cause 2:
1. Ensure an ACTIVE NotificationHub exists (register_notification_hub + poll list_notification_hubs).
2. resp = notif.create_notification_configuration(name="sim2realbehav1", description="x") -> resp["status"] is 'ACTIVE'; AWS returns 'INACTIVE' (zero EventRules).
3. notif.get_notification_configuration(arn=resp["arn"])["status"] -> also 'ACTIVE'.
4. Page notif.list_notification_configurations(status="INACTIVE", maxResults=100) -> the new ARN is absent (AWS lists it); it appears under status="ACTIVE" instead.

Cause 3:
1. Create a configuration as above.
2. notif.tag_resource(arn=<arn>, tags={f"sim2real-tag-{i}": str(i) for i in range(201)}) -> ValidationException (correct) but with the generic length message instead of '[object has too many properties (found 201 but schema requires at most 200)]'.

Cause 4:
1. Ensure a baseline hub exists in one region, then notif.register_notification_hub(notificationHubRegion="eu-west-1"), poll until ACTIVE, notif.deregister_notification_hub(notificationHubRegion="eu-west-1"), poll until absent.
2. Call notif.deregister_notification_hub(notificationHubRegion="eu-west-1") again -> ResourceNotFoundException (correct) with message 'no NotificationHub is registered in region eu-west-1' instead of 'notification hub with region eu-west-1 not found'.
