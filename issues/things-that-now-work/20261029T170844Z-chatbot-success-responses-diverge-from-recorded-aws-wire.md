Issue-Source: trace-sweep-divergence
Issue-Trace: chatbot-chime-webhook-arn-filter
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chatbot-chime-webhook-arn-filter chatbot-chime-webhook-lifecycle chatbot-slack-configuration-error-paths chatbot-custom-action-lifecycle chatbot-custom-action-tagging chatbot-account-preferences-roundtrip chatbot-teams-configuration-error-paths
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot|CreateChimeWebhookConfiguration|2xx->2xx|status
Issue-Key: chatbot|CreateCustomAction|2xx->2xx|status
Issue-Key: chatbot|CreateCustomAction|2xx->2xx|$.CustomActionArn
Issue-Key: chatbot|UpdateAccountPreferences|2xx->2xx|$.AccountPreferences.TrainingDataCollectionEnabled
Issue-Key: chatbot|ListMicrosoftTeamsConfiguredTeams|2xx->2xx|$.ConfiguredTeams[*]
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Chatbot success responses diverge from recorded AWS wire shape

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomAction

## Description
On operations the chatbot emulator does implement, the 2xx response diverges from the recorded AWS response: the wrong success status on the create operations, the wrong ARN form, a member serialized that AWS omits, and list data that no request in the fixture created. Five causes, all in the chatbot service.

Cause 1 of 5: CreateChimeWebhookConfiguration answers 201 instead of 200.
AWS: the recorded response status for POST /create-chime-webhook-configuration is 200 with the CreateChimeWebhookConfigurationResult body. (Smithy: CreateChimeWebhookConfiguration carries @http code 201, which the live service does not use - recorded behavior is the requirement here and the model's code is stale.)
Emulator: 201 with an otherwise matching body, because it serves the modeled @http code verbatim.
Why: clients and gateways that branch on the exact status (strict 200 assertions, status-based caching/retry rules, SDK response-code checks in test harnesses) see a different status than against AWS; a 201 also implies Location-style create semantics this API does not have.
Fix: the CreateChimeWebhookConfiguration handler's success-status selection in the chatbot service crate (src/ops/) - return 200 rather than the model's 201.
Traces: chatbot-chime-webhook-arn-filter#setup#1 (same cluster: chatbot-chime-webhook-arn-filter#setup#2, chatbot-chime-webhook-lifecycle#setup#1, chatbot-slack-configuration-error-paths#setup#2).

Cause 2 of 5: CreateCustomAction answers 201 instead of 200.
AWS: the recorded response status for POST /create-custom-action is 200 with CreateCustomActionResult. (Smithy: CreateCustomAction carries @http code 201; recorded behavior shows 200.)
Emulator: 201.
Why: same as cause 1 - status-sensitive clients diverge on every custom-action creation.
Fix: the CreateCustomAction handler's success status.
Traces: chatbot-custom-action-lifecycle#setup#1.

Cause 3 of 5: CreateCustomAction returns a regional custom-action ARN where AWS returns the region-less form.
AWS: recorded behavior only - CreateCustomActionResult.CustomActionArn comes back as arn:aws:chatbot::<accountId>:custom-action/<ActionName>, i.e. with an EMPTY region field (the Smithy CustomActionArn pattern ^arn:aws:chatbot:[A-Za-z0-9_/.-]{0,63}:... permits a zero-length region, and the same region-less form is what the fixtures then pass to ListTagsForResource/TagResource).
Emulator: arn:aws:chatbot:<region>:<accountId>:custom-action/<ActionName>, with the region filled in.
Why: the ARN is the resource identifier clients store and pass back; one built with a different region component does not compare equal to the ARN AWS hands out, so stored identifiers, policy documents and IAM resource matches built from an emulator run do not line up with AWS.
Fix: the CreateCustomAction handler's ARN construction / response serializer.
Traces: chatbot-custom-action-tagging#setup#1 (this step also shows cause 2's 201 status).

Cause 4 of 5: UpdateAccountPreferences serializes an account preference that was never set.
AWS: Smithy: UpdateAccountPreferences input members UserAuthorizationRequired and TrainingDataCollectionEnabled are both optional, and the output AccountPreferences structure has both as optional booleans with no default. The recording shows the response AccountPreferences omitting TrainingDataCollectionEnabled entirely when the request did not supply it.
Emulator: emits "TrainingDataCollectionEnabled": false - an unset optional boolean serialized as its zero value.
Why: a client cannot distinguish "preference not configured" from "preference explicitly disabled"; read-modify-write of account preferences will push an explicit false back to AWS and change account behavior that was previously unset.
Fix: the UpdateAccountPreferences response serializer / the AccountPreferences state representation - keep the member Option and omit it when absent instead of defaulting it.
Traces: chatbot-account-preferences-roundtrip#setup#1 (same cluster: task#3, teardown#5).

Cause 5 of 5: ListMicrosoftTeamsConfiguredTeams returns pre-seeded teams that were never configured.
AWS: Smithy: ListMicrosoftTeamsConfiguredTeamsResult.ConfiguredTeams lists the Microsoft Teams teams authorized for the calling account. The recording for an account with no Teams authorization returns the list with no entries.
Emulator: returns two fabricated ConfiguredTeams entries (fixed TenantId/TeamId values with generated TeamNames) that no step in the fixture created.
Why: clients enumerating configured teams act on resources that do not exist - they try to create channel configurations against phantom teams, and "is Teams onboarded?" checks answer yes for an account that never authorized a team.
Fix: the ListMicrosoftTeamsConfiguredTeams handler must read the chatbot crate's state (src/state.rs) and return only teams actually authorized, with no seeded fixtures.
Traces: chatbot-teams-configuration-error-paths#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (chatbot):

1) chatbot-chime-webhook-arn-filter#setup#1 CreateChimeWebhookConfiguration - status: expected 200, got 201 (cluster also covers chatbot-chime-webhook-arn-filter#setup#2, chatbot-chime-webhook-lifecycle#setup#1, chatbot-slack-configuration-error-paths#setup#2).
2) chatbot-custom-action-lifecycle#setup#1 CreateCustomAction - status: expected 200, got 201.
3) chatbot-custom-action-tagging#setup#1 CreateCustomAction - status: expected 200, got 201; $.CustomActionArn expected the region-less form 'arn:aws:chatbot::<account>:custom-action/<name>', got 'arn:aws:chatbot:<region>:<account>:custom-action/<name>'.
4) chatbot-account-preferences-roundtrip#setup#1 UpdateAccountPreferences - 200 both sides; $.AccountPreferences.TrainingDataCollectionEnabled: type_mismatch, expected absent, got False (cluster also covers task#3 and teardown#5).
5) chatbot-teams-configuration-error-paths#task#2 ListMicrosoftTeamsConfiguredTeams - 200 both sides; $.ConfiguredTeams[0] and $.ConfiguredTeams[1] extra: two seeded teams returned where the recording has none.

Model grounding (chatbot smithy model): CreateChimeWebhookConfiguration and CreateCustomAction both declare @http code 201 while the recorded AWS responses are 200; CustomActionArn's pattern allows an empty region segment; AccountPreferences members are optional booleans with no default.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. POST /create-chime-webhook-configuration (chatbot-chime-webhook-arn-filter setup#1) and POST /create-custom-action (chatbot-custom-action-lifecycle setup#1): observe HTTP 201 where the recordings are HTTP 200.
3. POST /create-custom-action (chatbot-custom-action-tagging setup#1): compare the returned CustomActionArn's region segment with the recorded region-less ARN.
4. POST /update-account-preferences supplying only UserAuthorizationRequired (chatbot-account-preferences-roundtrip setup#1): observe TrainingDataCollectionEnabled:false in the AccountPreferences body where the recording omits the member.
5. POST /list-ms-teams-configured-teams on a fresh emulator (chatbot-teams-configuration-error-paths task#2): observe two seeded ConfiguredTeams entries where the recording returns none.
