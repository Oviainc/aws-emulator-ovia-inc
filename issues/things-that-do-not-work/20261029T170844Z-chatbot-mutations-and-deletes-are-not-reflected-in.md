Issue-Source: trace-sweep-divergence
Issue-Trace: chatbot-chime-webhook-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chatbot-chime-webhook-lifecycle chatbot-custom-action-lifecycle chatbot-custom-action-tagging
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot|DescribeChimeWebhookConfigurations|2xx->2xx|$.WebhookConfigurations[*]
Issue-Key: chatbot|GetCustomAction|2xx->2xx|$.CustomAction.Definition.CommandText
Issue-Key: chatbot|GetCustomAction|4xx->2xx|$
Issue-Key: chatbot|GetCustomAction|4xx->2xx|$.Message
Issue-Key: chatbot|GetCustomAction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: chatbot|GetCustomAction|4xx->2xx|status
Issue-Key: chatbot|ListTagsForResource|2xx->2xx|$.Tags[*]
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Chatbot mutations and deletes are not reflected in subsequent reads

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateChimeWebhookConfiguration

## Description
The chatbot emulator accepts mutating calls (update, delete, untag) and answers them successfully, but does not apply them to its stored state: the following read returns the pre-mutation resource. Four causes, all in the chatbot service, all the same shape of defect on different resources.

Cause 1 of 4: UpdateChimeWebhookConfiguration does not persist the updated fields.
AWS: Smithy: UpdateChimeWebhookConfiguration takes ChatConfigurationArn (required) plus WebhookDescription, WebhookUrl, SnsTopicArns, IamRoleArn, LoggingLevel and returns the updated ChimeWebhookConfiguration; a following DescribeChimeWebhookConfigurations returns the configuration with the supplied values in place. The recording shows the described configuration carrying the new WebhookDescription and the new LoggingLevel after the update.
Emulator: DescribeChimeWebhookConfigurations still returns the values the configuration was created with - the old WebhookDescription and LoggingLevel INFO rather than the updated description and ERROR.
Why: a client that updates a configuration and then reads it back to confirm sees its change silently lost; configuration-drift reconcilers loop forever re-applying an update that never takes effect.
Fix: the UpdateChimeWebhookConfiguration handler must write the supplied members through to the chatbot crate's state (src/state.rs) rather than only echoing them in its response.
Traces: chatbot-chime-webhook-lifecycle#task#4 (and the same stale read at teardown#6).

Cause 2 of 4: UpdateCustomAction does not persist the updated Definition.
AWS: Smithy: UpdateCustomAction takes CustomActionArn and a required Definition (CustomActionDefinition.CommandText, required, length 1-100); GetCustomAction afterwards returns CustomAction.Definition.CommandText equal to the command text just supplied.
Emulator: GetCustomAction returns the CommandText the action was created with, not the one the update supplied.
Why: the update appears to succeed, so a client believes the action now runs the new command; invocations keep running the old command text, which for a custom action is arbitrary operator-facing behavior.
Fix: the UpdateCustomAction handler must replace the stored custom action's Definition (and AliasName/Attachments) in state.
Traces: chatbot-custom-action-lifecycle#task#5.

Cause 3 of 4: DeleteCustomAction does not remove the custom action.
AWS: Smithy: DeleteCustomAction (http code 204, idempotent) deletes the action; GetCustomAction on the deleted ARN then fails with ResourceNotFoundException - httpError 404, member `Message`, x-amzn-errortype ResourceNotFoundException. Recorded `Message`: `sdk action [<ActionName>] not found for account [<accountId>]` (fixed text around the action name from the request and the caller's account id).
Emulator: GetCustomAction keeps answering 200 with the full CustomAction body indefinitely - the fixture's poll for the ResourceNotFoundException gave up after 29 attempts, so the deletion never becomes visible.
Why: the resource is undeletable from a client's point of view: teardown/cleanup never converges, re-creating the action under the same name hits a spurious ConflictException, and poll-until-gone logic hangs until it times out.
Fix: the DeleteCustomAction handler must remove the action from the chatbot crate's state so GetCustomAction/ListCustomActions stop returning it (and GetCustomAction then raises ResourceNotFoundException).
Traces: chatbot-custom-action-lifecycle#teardown#8.

Cause 4 of 4: UntagResource does not remove the tag.
AWS: Smithy: UntagResource takes ResourceARN and TagKeys (both required) and returns an empty body; ListTagsForResource afterwards returns only the tags whose keys were not removed. The recording shows a single remaining tag after untagging one of two.
Emulator: ListTagsForResource still returns the removed tag (the second entry is extra), i.e. the untag was a no-op.
Why: tags are load-bearing for cost allocation and access control; a client that removes a tag and verifies the removal sees it still present, and tag-based policy or billing attribution keeps applying.
Fix: the UntagResource handler must delete the named keys from the resource's tag map in state (TagResource/ListTagsForResource read the same map).
Traces: chatbot-custom-action-tagging#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (chatbot):

1) chatbot-chime-webhook-lifecycle#task#4 DescribeChimeWebhookConfigurations - 200 both sides; $.WebhookConfigurations[0] mismatch: expected WebhookDescription '<renamed description>' and LoggingLevel 'ERROR' (the values UpdateChimeWebhookConfiguration supplied), got the original WebhookDescription and LoggingLevel 'INFO'. Same stale read again at teardown#6 (affected cluster member).
2) chatbot-custom-action-lifecycle#task#5 GetCustomAction - 200 both sides; $.CustomAction.Definition.CommandText expected the updated command text ('... --output json' appended by UpdateCustomAction), got the original command text.
3) chatbot-custom-action-lifecycle#teardown#8 GetCustomAction after DeleteCustomAction - expected 404, got 200; $.Message missing (expected 'sdk action [<name>] not found for account [<account>]'); $headers.x-amzn-errortype missing; poll gave up: 'error code None not in [ResourceNotFoundException] after 29 attempt(s)', with the full CustomAction body still returned.
4) chatbot-custom-action-tagging#task#5 ListTagsForResource after UntagResource - 200 both sides; $.Tags[1] extra: the untagged key/value pair is still present (expected a single remaining tag).

Model grounding (chatbot smithy model): UpdateChimeWebhookConfiguration/UpdateCustomAction return the updated resource; DeleteCustomAction is idempotent with http code 204 and GetCustomAction models ResourceNotFoundException (404, member `Message`); UntagResource takes required TagKeys.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. chatbot-chime-webhook-lifecycle: create a Chime webhook configuration, POST /update-chime-webhook-configuration with a new WebhookDescription and LoggingLevel=ERROR, then POST /describe-chime-webhook-configurations (task#4) and compare the returned configuration with the values just sent.
3. chatbot-custom-action-lifecycle: create a custom action, POST /update-custom-action with a changed Definition.CommandText, then POST /get-custom-action (task#5).
4. Same fixture: POST /delete-custom-action, then poll POST /get-custom-action (teardown#8) - it answers 200 forever instead of 404 ResourceNotFoundException.
5. chatbot-custom-action-tagging: tag a custom action with two keys, POST /untag-resource with one key, then POST /list-tags-for-resource (task#5).
