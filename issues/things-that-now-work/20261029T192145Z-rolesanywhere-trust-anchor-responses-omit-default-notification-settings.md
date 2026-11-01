Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-notification-settings-workflow
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-notification-settings-workflow rolesanywhere-trust-anchor-lifecycle
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|CreateTrustAnchor|2xx->2xx|$.trustAnchor.createdAt
Issue-Key: rolesanywhere|CreateTrustAnchor|2xx->2xx|$.trustAnchor.crlDpRevocationEnabled
Issue-Key: rolesanywhere|CreateTrustAnchor|2xx->2xx|$.trustAnchor.notificationSettings[*]
Issue-Key: rolesanywhere|CreateTrustAnchor|2xx->2xx|$.trustAnchor.updatedAt
Issue-Key: rolesanywhere|GetTrustAnchor|2xx->2xx|$.trustAnchor.createdAt
Issue-Key: rolesanywhere|GetTrustAnchor|2xx->2xx|$.trustAnchor.crlDpRevocationEnabled
Issue-Key: rolesanywhere|GetTrustAnchor|2xx->2xx|$.trustAnchor.notificationSettings[*]
Issue-Key: rolesanywhere|GetTrustAnchor|2xx->2xx|$.trustAnchor.updatedAt
Issue-Key: rolesanywhere|DeleteTrustAnchor|2xx->2xx|$.trustAnchor.createdAt
Issue-Key: rolesanywhere|DeleteTrustAnchor|2xx->2xx|$.trustAnchor.crlDpRevocationEnabled
Issue-Key: rolesanywhere|DeleteTrustAnchor|2xx->2xx|$.trustAnchor.notificationSettings[*]
Issue-Key: rolesanywhere|DeleteTrustAnchor|2xx->2xx|$.trustAnchor.updatedAt
Issue-Key: rolesanywhere|ListTrustAnchors|2xx->2xx|$.trustAnchors[name="sim2real-probe-rolesanywhere-ta-861e9626"].createdAt
Issue-Key: rolesanywhere|ListTrustAnchors|2xx->2xx|$.trustAnchors[name="sim2real-probe-rolesanywhere-ta-861e9626"].crlDpRevocationEnabled
Issue-Key: rolesanywhere|ListTrustAnchors|2xx->2xx|$.trustAnchors[name="sim2real-probe-rolesanywhere-ta-861e9626"].notificationSettings[*]
Issue-Key: rolesanywhere|ListTrustAnchors|2xx->2xx|$.trustAnchors[name="sim2real-probe-rolesanywhere-ta-861e9626"].updatedAt
Issue-Key: rolesanywhere|DeleteTrustAnchor|2xx->2xx|$.trustAnchor.name
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# rolesanywhere trust anchor responses omit default notification settings and crlDpRevocationEnabled

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrustAnchor

## Description
The TrustAnchorDetail the emulator serializes is missing the trust anchor's notification settings and its CRL-distribution-point revocation flag, serializes its timestamps as epoch numbers, and UpdateTrustAnchor's rename is never persisted. Five causes, one per failing replay cluster.

Cause 1 of 5 (CreateTrustAnchor, fixture rolesanywhere-notification-settings-workflow step setup#1):
AWS: TrustAnchorDetail.createdAt/updatedAt carry @timestampFormat date-time (Smithy: TrustAnchorDetail.createdAt/updatedAt, doc 'The ISO-8601 timestamp when the trust anchor was created'), so they serialize as ISO-8601 strings. A trust anchor created WITHOUT notificationSettings is still returned with the two service-default NotificationSettingDetail entries: event CA_CERTIFICATE_EXPIRY and event END_ENTITY_CERTIFICATE_EXPIRY, each with enabled=true, threshold=45, channel=ALL and configuredBy='rolesanywhere.amazonaws.com' - the model states the configuredBy rule exactly ('For default settings configured by IAM Roles Anywhere, the value is rolesanywhere.amazonaws.com, and for customized notifications settings, it is the respective account ID'), and the thresholds/channel are recorded behavior only. The response also carries crlDpRevocationEnabled=false (recorded behavior only - the member is not in this checkout's TrustAnchorDetail model).
Emulator: notificationSettings is absent, crlDpRevocationEnabled is absent, and createdAt/updatedAt are JSON epoch floats instead of ISO-8601 strings.
Why: a client cannot discover that expiry notifications are on by default, nor at what threshold, so notification tooling built against the emulator sees an unmonitored trust anchor; and a date-time deserializer fails on the numeric timestamps.
Fix: the CreateTrustAnchor handler (seed the two default notification settings and the crlDpRevocationEnabled flag into state) and the shared TrustAnchorDetail serializer (date-time formatting).
Traces: rolesanywhere-notification-settings-workflow#setup#1 (also rolesanywhere-trust-anchor-lifecycle#setup#1).

Cause 2 of 5 (GetTrustAnchor after PutNotificationSettings, fixture rolesanywhere-notification-settings-workflow step task#3):
AWS: after a customization, GetTrustAnchor returns one NotificationSettingDetail per event reflecting the SUBMITTED values - the threshold the caller set for each event, enabled as submitted, channel ALL - and configuredBy switches from 'rolesanywhere.amazonaws.com' to the caller's account id for every customized setting (Smithy: NotificationSettingDetail.configuredBy doc, as quoted above). createdAt/updatedAt remain ISO-8601 strings and crlDpRevocationEnabled remains present.
Emulator: notificationSettings and crlDpRevocationEnabled are still absent from the read and the timestamps are still epoch floats, so the customization is invisible.
Why: a client that sets expiry thresholds and reads them back gets no notificationSettings at all - read-after-write on notification settings does not hold.
Fix: the PutNotificationSettings write into trust-anchor state and the shared TrustAnchorDetail serializer used by GetTrustAnchor.
Traces: rolesanywhere-notification-settings-workflow#task#3 (cluster also covers that fixture's task#5 and rolesanywhere-trust-anchor-lifecycle#task#2).

Cause 3 of 5 (DeleteTrustAnchor, fixture rolesanywhere-notification-settings-workflow step teardown#6):
AWS: DeleteTrustAnchor answers 200 with the deleted anchor's final TrustAnchorDetail (Smithy: DeleteTrustAnchor output TrustAnchorDetailResponse.trustAnchor, @required). After ResetNotificationSettings for one event, that event's setting is back to its service default (threshold 45, configuredBy 'rolesanywhere.amazonaws.com') while the still-customized event keeps the caller's threshold and the account id as configuredBy - so the echo shows one defaulted and one customized setting.
Emulator: the delete echo carries no notificationSettings and no crlDpRevocationEnabled, and epoch-float timestamps.
Why: a client cannot confirm from the delete echo which notification settings were in force, and the reset path is unobservable.
Fix: the DeleteTrustAnchor handler's response serialization and the ResetNotificationSettings write into state.
Traces: rolesanywhere-notification-settings-workflow#teardown#6.

Cause 4 of 5 (ListTrustAnchors, fixture rolesanywhere-trust-anchor-lifecycle step task#3):
AWS: ListTrustAnchors returns a list of the SAME TrustAnchorDetail shape, so each element carries the two default notificationSettings (threshold 45, channel ALL, enabled true, configuredBy 'rolesanywhere.amazonaws.com'), crlDpRevocationEnabled=false and ISO-8601 createdAt/updatedAt.
Emulator: each listed element omits notificationSettings and crlDpRevocationEnabled and carries epoch-float timestamps.
Why: enumeration is as broken as the single read, so a client sweeping trust anchors for expiry configuration sees nothing anywhere.
Fix: the ListTrustAnchors handler's use of the shared TrustAnchorDetail serializer.
Traces: rolesanywhere-trust-anchor-lifecycle#task#3.

Cause 5 of 5 (DeleteTrustAnchor after UpdateTrustAnchor, fixture rolesanywhere-trust-anchor-lifecycle step teardown#7):
AWS: UpdateTrustAnchor persists the name it was given (Smithy: UpdateTrustAnchor.name -> ResourceName @length 1..255, @pattern ^[ a-zA-Z0-9-_]*$) and bumps updatedAt, so every later read and the delete echo report the NEW name.
Emulator: the delete echo still reports the pre-update name, i.e. the rename was never written to state; it also omits notificationSettings and crlDpRevocationEnabled and emits epoch-float timestamps.
Why: read-after-write does not hold for a rename - a client that renames a trust anchor and reads it back sees the old name and may rename repeatedly or key its own records on a stale name.
Fix: the UpdateTrustAnchor handler's write into the trust-anchor state record (name, updatedAt), plus the shared TrustAnchorDetail serializer.
Traces: rolesanywhere-trust-anchor-lifecycle#teardown#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator, 5 mismatch clusters, all status-matched but body-diverged.

Cause 1 - rolesanywhere-notification-settings-workflow#setup#1 CreateTrustAnchor, expected 201 / actual 201 (cluster also covers rolesanywhere-trust-anchor-lifecycle#setup#1):
  $.trustAnchor.createdAt: value_mismatch (expected an ISO-8601 date-time string, got 1793301479.4753585)
  $.trustAnchor.updatedAt: value_mismatch (expected an ISO-8601 date-time string, got 1793301479.4753585)
  $.trustAnchor.crlDpRevocationEnabled: missing (expected False, got None)
  $.trustAnchor.notificationSettings[0]: missing (expected {'channel': 'ALL', 'configuredBy': 'rolesanywhere.amazonaws.com', 'enabled': True, 'event': 'CA_CERTIFICATE_EXPIRY', 'threshold': 45}, got None)
  $.trustAnchor.notificationSettings[1]: missing (expected {'channel': 'ALL', 'configuredBy': 'rolesanywhere.amazonaws.com', 'enabled': True, 'event': 'END_ENTITY_CERTIFICATE_EXPIRY', 'threshold': 45}, got None)

Cause 2 - rolesanywhere-notification-settings-workflow#task#3 GetTrustAnchor, 200/200 (cluster also covers task#5 and rolesanywhere-trust-anchor-lifecycle#task#2): same five keys, with the expected settings now customized - notificationSettings[0] {'channel': 'ALL', 'configuredBy': '<caller account id>', 'enabled': True, 'event': 'CA_CERTIFICATE_EXPIRY', 'threshold': <submitted>} and notificationSettings[1] {..., 'event': 'END_ENTITY_CERTIFICATE_EXPIRY', 'threshold': <submitted>} - got None for both.

Cause 3 - rolesanywhere-notification-settings-workflow#teardown#6 DeleteTrustAnchor, 200/200: notificationSettings[0] expected back at its default (configuredBy 'rolesanywhere.amazonaws.com', threshold 45) and notificationSettings[1] still customized (configuredBy the account id, the submitted threshold) - both got None; crlDpRevocationEnabled missing; createdAt/updatedAt epoch floats.

Cause 4 - rolesanywhere-trust-anchor-lifecycle#task#3 ListTrustAnchors, 200/200: the same five keys under $.trustAnchors[name="<created anchor name>"].

Cause 5 - rolesanywhere-trust-anchor-lifecycle#teardown#7 DeleteTrustAnchor, 200/200:
  $.trustAnchor.name: value_mismatch (expected the name submitted to UpdateTrustAnchor, got the original create-time name)
  plus crlDpRevocationEnabled missing, notificationSettings[0..1] missing, createdAt/updatedAt epoch floats.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Cause 1: POST /trustanchors with name+source only - fixture rolesanywhere-notification-settings-workflow step setup#1. Observe no notificationSettings, no crlDpRevocationEnabled, numeric createdAt/updatedAt.
3. Cause 2: PUT the notification settings for both expiry events, then GET /trustanchor/{trustAnchorId} - same fixture, step task#3 (and task#5). The customized settings are absent from the read.
4. Cause 3: reset one event's notification setting and DELETE /trustanchor/{trustAnchorId} - same fixture, step teardown#6.
5. Cause 4: GET /trustanchors - fixture rolesanywhere-trust-anchor-lifecycle step task#3.
6. Cause 5: PATCH /trustanchor/{trustAnchorId} with a new name, then DELETE it - fixture rolesanywhere-trust-anchor-lifecycle step teardown#7; the echo still carries the old name.
