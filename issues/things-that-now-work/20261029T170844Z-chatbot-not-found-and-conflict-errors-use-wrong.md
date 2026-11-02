Issue-Source: trace-sweep-divergence
Issue-Trace: chatbot-resource-not-found-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chatbot-resource-not-found-error-paths chatbot-custom-action-lifecycle chatbot-slack-configuration-error-paths
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot|GetCustomAction|4xx->4xx|$.Message
Issue-Key: chatbot|GetCustomAction|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chatbot|GetCustomAction|4xx->4xx|status
Issue-Key: chatbot|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: chatbot|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chatbot|ListTagsForResource|4xx->4xx|status
Issue-Key: chatbot|TagResource|4xx->4xx|$.Message
Issue-Key: chatbot|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chatbot|TagResource|4xx->4xx|status
Issue-Key: chatbot|CreateCustomAction|4xx->4xx|$.message
Issue-Key: chatbot|DeleteSlackChannelConfiguration|4xx->4xx|$.Message
Issue-Key: chatbot|UpdateSlackChannelConfiguration|4xx->4xx|$.Message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Chatbot not-found and conflict errors use wrong status, error type and message

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCustomAction

## Description
The chatbot emulator's error paths diverge from AWS on all three parts of an error response: the HTTP status, the x-amzn-errortype error code, and the message body. Operations that address a resource that does not exist answer 403 UnauthorizedException instead of 404 ResourceNotFoundException, and the operations that do pick the right error class word the message differently from AWS. Six causes, all in the chatbot service.

Cause 1 of 6: GetCustomAction on a custom-action ARN that does not exist.
AWS: Smithy: GetCustomAction errors include ResourceNotFoundException (errorKind client, httpError 404, member `Message`); x-amzn-errortype is ResourceNotFoundException. The recorded response is 404 with body member `Message` reading `sdk action [<ActionName>] not found for account [<accountId>]` - the fixed text is `sdk action [` ... `] not found for account [` ... `]`, with the action name taken from the requested CustomActionArn and the account id from the caller's account.
Emulator: 403 with x-amzn-errortype UnauthorizedException and no `Message` member at all.
Why: an SDK client maps 403/UnauthorizedException to a credentials/permissions failure and does not retry or branch on it; code that handles "custom action absent" by catching ResourceNotFoundException never sees it, so existence probes and idempotent create-if-missing logic fail with an auth error instead.
Fix: the GetCustomAction handler's resource-lookup path in the chatbot service crate (src/ops/) must raise the modeled ResourceNotFoundException, and the error serializer must emit the 404 status, the ResourceNotFoundException errortype header and the `Message` member.
Traces: chatbot-resource-not-found-error-paths#task#1.

Cause 2 of 6: ListTagsForResource on a chatbot resource ARN that does not exist.
AWS: Smithy: ListTagsForResource errors are InternalServiceError, ResourceNotFoundException (404, member `Message`), ServiceUnavailableException. The recording shows 404, x-amzn-errortype ResourceNotFoundException, and `Message` = `Resource <ResourceARN> does not exist!` - fixed text `Resource ` + `ResourceARN` from the request + ` does not exist!`.
Emulator: 403, x-amzn-errortype UnauthorizedException, no `Message`. UnauthorizedException is not even a modeled error for this operation.
Why: tag-reconciliation clients cannot distinguish "resource gone" from "no permission"; they abort instead of treating the resource as absent.
Fix: ListTagsForResource handler's ARN resolution + the error serializer, same as cause 1.
Traces: chatbot-resource-not-found-error-paths#task#4.

Cause 3 of 6: TagResource on a chatbot resource ARN that does not exist.
AWS: Smithy: TagResource errors are InternalServiceError, ResourceNotFoundException (404, member `Message`), ServiceUnavailableException, TooManyTagsException. Recorded: 404, x-amzn-errortype ResourceNotFoundException, `Message` = `Resource <ResourceARN> does not exist!` (same template as cause 2, ARN from the request).
Emulator: 403, x-amzn-errortype UnauthorizedException, no `Message`.
Why: same as cause 2 - a write to a missing resource is reported as an auth failure, so retry/backoff logic keyed on auth errors spins instead of failing fast.
Fix: TagResource handler's ARN resolution + the error serializer.
Traces: chatbot-resource-not-found-error-paths#task#5.

Cause 4 of 6: CreateCustomAction for an ActionName that already exists.
AWS: Smithy: CreateCustomAction error ConflictException (errorKind client, httpError 409, member `message` - lowercase). The emulator already returns 409 ConflictException, so only the message diverges. Recorded `message`: `Resource with arn <customActionArn> already exists.` - fixed text `Resource with arn ` + the existing custom action's ARN (regional form, arn:aws:chatbot:<region>:<account>:custom-action/<ActionName>) + ` already exists.`
Emulator: `a custom action named <ActionName> already exists` - different wording and it names the action name rather than the ARN.
Why: clients and operators match on AWS's message text when the error code alone is ambiguous, and tests that assert the AWS wording fail; the emulator message also omits the ARN that AWS gives callers to identify the colliding resource.
Fix: the CreateCustomAction handler's conflict branch - format the modeled ConflictException message from the existing resource's ARN.
Traces: chatbot-custom-action-lifecycle#task#6.

Cause 5 of 6: DeleteSlackChannelConfiguration for a ChatConfigurationArn that does not exist.
AWS: Smithy: DeleteSlackChannelConfiguration error ResourceNotFoundException (404, member `Message`). Status and errortype already match. Recorded `Message`: `No Slack Destination exists for Wheatley Channel: <ChatConfigurationArn>.` - fixed text `No Slack Destination exists for Wheatley Channel: ` + the ARN from the request + `.`
Emulator: `no chat configuration with ARN <ChatConfigurationArn>` - a generic emulator-wide phrasing shared with other operations, with no trailing period and not AWS's wording.
Why: the message is part of the contract for this error class; a client or test asserting AWS's text sees a mismatch, and the generic text hides which chat-configuration flavour the API rejected.
Fix: the DeleteSlackChannelConfiguration handler's not-found branch must build its own per-operation message instead of reusing the shared generic one.
Traces: chatbot-slack-configuration-error-paths#task#7.

Cause 6 of 6: UpdateSlackChannelConfiguration for a ChatConfigurationArn that does not exist.
AWS: Smithy: UpdateSlackChannelConfiguration error ResourceNotFoundException (404, member `Message`). Recorded `Message`: `Chat Configuration with ARN <ChatConfigurationArn> not found.` - fixed text `Chat Configuration with ARN ` + the ARN from the request + ` not found.` Note AWS words this differently from the delete path (cause 5), so one shared string cannot satisfy both.
Emulator: the same generic `no chat configuration with ARN <ChatConfigurationArn>`.
Why: as cause 5 - the diverging message breaks clients and assertions that rely on AWS's wording per error class.
Fix: the UpdateSlackChannelConfiguration handler's not-found branch.
Traces: chatbot-slack-configuration-error-paths#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (chatbot, 8 fixtures):

1) chatbot-resource-not-found-error-paths#task#1 GetCustomAction - expected 404, got 403; $.Message missing (expected 'sdk action [<name>] not found for account [<account>]'); $headers.x-amzn-errortype expected 'ResourceNotFoundException', got 'UnauthorizedException'.
2) chatbot-resource-not-found-error-paths#task#4 ListTagsForResource - expected 404, got 403; $.Message missing (expected 'Resource <arn> does not exist!'); errortype 'ResourceNotFoundException' -> 'UnauthorizedException'.
3) chatbot-resource-not-found-error-paths#task#5 TagResource - expected 404, got 403; $.Message missing (expected 'Resource <arn> does not exist!'); errortype 'ResourceNotFoundException' -> 'UnauthorizedException'.
4) chatbot-custom-action-lifecycle#task#6 CreateCustomAction - 409 both sides; $.message expected 'Resource with arn <custom-action-arn> already exists.', got 'a custom action named <name> already exists'.
5) chatbot-slack-configuration-error-paths#task#7 DeleteSlackChannelConfiguration - 404 both sides; $.Message expected 'No Slack Destination exists for Wheatley Channel: <arn>.', got 'no chat configuration with ARN <arn>'.
6) chatbot-slack-configuration-error-paths#task#8 UpdateSlackChannelConfiguration - 404 both sides; $.Message expected 'Chat Configuration with ARN <arn> not found.', got 'no chat configuration with ARN <arn>'.

Model grounding (services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/smithy-model/com-amazonaws-chatbot-wheatleyorchestration-20171011.json): ResourceNotFoundException = client error, httpError 404, member `Message`; ConflictException = client error, httpError 409, member `message`; UnauthorizedException = client error, httpError 403, member `message` and is NOT a modeled error of ListTagsForResource/TagResource.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture chatbot-resource-not-found-error-paths: step task#1 calls POST /get-custom-action with a custom-action ARN that was never created; steps task#4 and task#5 call POST /list-tags-for-resource and POST /tag-resource with that same non-existent ARN. Observe 403/UnauthorizedException with no Message member instead of 404/ResourceNotFoundException with the documented Message.
3. Fixture chatbot-custom-action-lifecycle: create a custom action (setup#1), then call POST /create-custom-action again with the same ActionName (task#6). Observe the 409 body's `message` wording.
4. Fixture chatbot-slack-configuration-error-paths: call POST /delete-slack-channel-configuration (task#7) and POST /update-slack-channel-configuration (task#8) with a chat-configuration ARN that does not exist. Observe the identical generic `Message` on both instead of AWS's two distinct texts.
