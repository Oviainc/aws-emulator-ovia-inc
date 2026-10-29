Issue-Source: trace-sweep-divergence
Issue-Trace: email-configuration-set-full-lifecycle-with-event-destinations
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-configuration-set-full-lifecycle-with-event-destinations sesv2-configuration-set-security-options-lifecycle
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|CreateConfigurationSet|4xx->4xx|$.message
Issue-Key: sesv2|GetConfigurationSet|2xx->2xx|$.DeliveryOptions
Issue-Key: sesv2|GetConfigurationSet|2xx->2xx|$.ReputationOptions.ReputationMetricsEnabled
Issue-Key: sesv2|GetConfigurationSet|2xx->2xx|$.Tags
Issue-Key: sesv2|GetConfigurationSet|2xx->2xx|$headers.content-type
Issue-Key: sesv2|GetConfigurationSet|2xx->2xx|$.VdmOptions
Issue-Key: sesv2|GetConfigurationSet|2xx->2xx|$.MessageSecurityOptions
Issue-Key: sesv2|GetConfigurationSetEventDestinations|2xx->2xx|$.EventDestinations[Name="sim2real-probe-sesv2-dest-44ed6c39"].MatchingEventTypes[*]
Issue-Key: sesv2|GetConfigurationSetEventDestinations|2xx->2xx|$headers.content-type
Issue-Key: sesv2|DeleteConfigurationSetEventDestination|2xx->2xx|$headers.content-type
Issue-Key: sesv2|DeleteConfigurationSetEventDestination|4xx->4xx|$.message
Issue-Key: sesv2|DeleteConfigurationSetEventDestination|4xx->4xx|$headers.content-type
Issue-Key: sesv2|DeleteConfigurationSet|2xx->2xx|$headers.content-type
Issue-Key: sesv2|GetConfigurationSet|4xx->4xx|$headers.content-type
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Configuration-set reads lose persisted options and every response carries application/json

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfigurationSet

## Description
Ten net-new causes in the SESv2 configuration-set plane. Smithy: service SimpleEmailService_v2 (sdkId SESv2), protocol restJson1; GetConfigurationSet -> GetConfigurationSetResponse {ConfigurationSetName, TrackingOptions, DeliveryOptions, ReputationOptions, SendingOptions, Tags, SuppressionOptions, VdmOptions, ArchivingOptions, MessageSecurityOptions}, errors BadRequestException(400)/NotFoundException(404).

Cause 1 of 10: CreateConfigurationSet duplicate-name refusal message. AWS: 400 AlreadyExistsException with the sentence 'Configuration set <NAME> already exists.' — the fixed text is 'Configuration set ' + '<' + name + '>' + ' already exists.'; the only variable part is the configuration-set name from the request, and AWS WRAPS it in angle brackets. Emulator: same 400 but the message omits the angle brackets around the name ('Configuration set NAME already exists.'). Why: a client matching or displaying AWS's refusal sentence sees different text for the same error class. Fix: the CreateConfigurationSet handler's duplicate-name refusal message. Traces: email-configuration-set-full-lifecycle-with-event-destinations#task#2.

Cause 2 of 10: GetConfigurationSet invents DeliveryOptions and drops Tags/ReputationOptions state. AWS: for a configuration set created without DeliveryOptions the response OMITS DeliveryOptions entirely, returns Tags as an empty list when there are no tags, and ReputationOptions.ReputationMetricsEnabled reflects what the create/put request set (true here). Emulator: returns DeliveryOptions {TlsPolicy: OPTIONAL} that was never configured, omits Tags instead of serializing the empty list, and reports ReputationMetricsEnabled false although the configuration set was created with it enabled. Why: a client reading back a configuration set sees a TLS policy it never set and believes reputation-metric collection is off, so it re-issues or mis-reports configuration. Fix: the GetConfigurationSet serializer plus the configuration-set state that records ReputationOptions/Tags. Traces: email-configuration-set-full-lifecycle-with-event-destinations#task#3.

Cause 3 of 10: GetConfigurationSet never reports VdmOptions. AWS: after PutConfigurationSetVdmOptions the read returns VdmOptions {DashboardOptions{EngagementMetrics}, GuardianOptions{OptimizedSharedDelivery}} with the submitted values, plus Tags [] and the enabled ReputationMetricsEnabled. Emulator: VdmOptions is absent from the response (so the Put is not persisted or not serialized), Tags absent, ReputationMetricsEnabled false. Why: VDM configuration is write-only in the emulator — a client cannot confirm its own configuration change. Fix: the configuration-set state for VdmOptions and the GetConfigurationSet serializer. Traces: email-configuration-set-full-lifecycle-with-event-destinations#task#12.

Cause 4 of 10: GetConfigurationSet never reports MessageSecurityOptions. AWS: after the message-security option is set the read returns MessageSecurityOptions {SigningScheme: {DefaultScheme: {}}} (Smithy: MessageSecurityOptions.SigningScheme -> SigningScheme). Emulator: MessageSecurityOptions is absent, and the same DeliveryOptions/ReputationOptions/Tags defects as Cause 2 appear. Why: a client cannot read back the S/MIME signing scheme it configured. Fix: the configuration-set state for MessageSecurityOptions and the GetConfigurationSet serializer. Traces: sesv2-configuration-set-security-options-lifecycle#task#4.

Cause 5 of 10: GetConfigurationSetEventDestinations reorders MatchingEventTypes. AWS: the event destination's MatchingEventTypes list is returned in the order it was supplied on CreateConfigurationSetEventDestination (here BOUNCE then SEND); it is a modeled list, so order is part of the value. Emulator: returns the list in a different order (SEND then BOUNCE), i.e. it is stored in a reordered/normalized collection. Why: a client that compares the event-type list it submitted against the one it reads back sees a spurious difference. Fix: the event-destination state must preserve list order; the GetConfigurationSetEventDestinations serializer must not sort. Traces: email-configuration-set-full-lifecycle-with-event-destinations#task#15.

Cause 6 of 10: response content-type on GetConfigurationSetEventDestinations. AWS: recorded behavior only — every SESv2 response carries the header content-type: application/x-amz-json-1.1 (the model declares restJson1, whose default media type is application/json, so the recording is the requirement). Emulator: sends content-type: application/json. Why: a client or proxy that keys on the AWS content type mis-dispatches the body. Fix: the service's response serializer/content-type default, one place for all SESv2 operations. Traces: email-configuration-set-full-lifecycle-with-event-destinations#task#17.

Cause 7 of 10: same content-type defect on the successful DeleteConfigurationSetEventDestination response (200). Traces: email-configuration-set-full-lifecycle-with-event-destinations#task#19.

Cause 8 of 10: DeleteConfigurationSetEventDestination not-found refusal message. AWS: 404 NotFoundException with 'No EventDestination found for <configuration-set-name>' — the fixed text is 'No EventDestination found for ' and the single variable part is the CONFIGURATION SET name from the request path. Emulator: 404 but with its own sentence 'Event destination <dest> does not exist in configuration set <set>.', naming the destination as well, and the response also carries the wrong content-type. Why: a client matching AWS's message text does not recognise the error. Fix: the DeleteConfigurationSetEventDestination handler's not-found refusal message. Traces: email-configuration-set-full-lifecycle-with-event-destinations#task#20.

Cause 9 of 10: same content-type defect on the successful DeleteConfigurationSet response (200). Traces: email-configuration-set-full-lifecycle-with-event-destinations#task#21.

Cause 10 of 10: same content-type defect on the GetConfigurationSet 404 (NotFoundException) response. Traces: email-configuration-set-full-lifecycle-with-event-destinations#teardown#22, and the same cluster covers sesv2-configuration-set-security-options-lifecycle#teardown#7.

No open issue covers these: the existing configuration-set issues (20261027T023557Z-ses-2-net-new-behavioral-root-causes-a.md, 20261026T065304Z-ses-the-configuration-set-tracking-options-plane-diverges.md) concern CFN visibility and tracking options, and 20261029T065607Z-ses-3-net-new-refusal-message-divergences-the.md concerns v1/tagging messages.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.
Cause 1 (task#2): status 400 both sides; $.message expected 'Configuration set <sim2real-probe-sesv2-cfgset-44ed6c39> already exists.', got 'Configuration set sim2real-probe-sesv2-cfgset-44ed6c39 already exists.'
Cause 2 (task#3): 200 both sides; $.DeliveryOptions expected absent, got {'TlsPolicy': 'OPTIONAL'}; $.ReputationOptions.ReputationMetricsEnabled expected True, got False; $.Tags expected [], got absent; $headers.content-type expected 'application/x-amz-json-1.1', got 'application/json'.
Cause 3 (task#12): $.VdmOptions expected {'DashboardOptions': {'EngagementMetrics': 'ENABLED'}, 'GuardianOptions': {'OptimizedSharedDelivery': 'ENABLED'}}, got absent; $.ReputationOptions.ReputationMetricsEnabled expected True, got False; $.Tags expected [], got absent.
Cause 4 (sesv2-configuration-set-security-options-lifecycle#task#4): $.MessageSecurityOptions expected {'SigningScheme': {'DefaultScheme': {}}}, got absent; $.DeliveryOptions expected absent, got {'TlsPolicy': 'OPTIONAL'}; $.ReputationOptions.ReputationMetricsEnabled expected True, got False; $.Tags expected [], got absent.
Cause 5 (task#15): $.EventDestinations[Name="...dest-44ed6c39"].MatchingEventTypes[0] expected 'BOUNCE', got 'SEND'; [1] expected 'SEND', got 'BOUNCE'.
Causes 6,7,9,10 (task#17, task#19, task#21, teardown#22): $headers.content-type expected 'application/x-amz-json-1.1', got 'application/json' (status matches on each).
Cause 8 (task#20): status 404 both sides; $.message expected 'No EventDestination found for sim2real-probe-sesv2-cfgset-44ed6c39', got 'Event destination sim2real-probe-sesv2-dest-44ed6c39 does not exist in configuration set sim2real-probe-sesv2-cfgset-44ed6c39.'

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture email-configuration-set-full-lifecycle-with-event-destinations (steps task#2, task#3, task#12, task#15, task#17, task#19, task#20, task#21, teardown#22) and fixture sesv2-configuration-set-security-options-lifecycle (step task#4).
2. Cause 1: CreateConfigurationSet with a name that already exists; compare the 400 message text.
3. Causes 2-4: create a configuration set, then PutConfigurationSetReputationOptions / PutConfigurationSetVdmOptions / the message-security option, then GetConfigurationSet and compare DeliveryOptions, Tags, ReputationOptions.ReputationMetricsEnabled, VdmOptions, MessageSecurityOptions.
4. Cause 5: create an event destination with MatchingEventTypes [BOUNCE, SEND] and read it back with GetConfigurationSetEventDestinations.
5. Causes 6,7,9,10: inspect the content-type response header on any SESv2 call.
6. Cause 8: DeleteConfigurationSetEventDestination for a destination that does not exist and compare the 404 message.
