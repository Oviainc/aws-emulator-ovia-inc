Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chatbot.chime-webhook-invalid-url-pattern-rejected chatbot.describe-slack-channel-configurations-invalid-next-token-rejected chatbot.list-custom-actions-maxresults-out-of-range-rejected chatbot.teams-channel-invalid-team-id-uuid-rejected chatbot.tag-resource-nonexistent-arn-rejected chatbot.custom-action-get-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot.chime-webhook-invalid-url-pattern-rejected|run#6|AssertionError|assert code == "InvalidRequestException", f"unexpected code={code!r}"
Issue-Key: chatbot.describe-slack-channel-configurations-invalid-next-token-rejected|run#3|AssertionError|assert code == "InvalidRequestException", f"unexpected code={code!r}"
Issue-Key: chatbot.list-custom-actions-maxresults-out-of-range-rejected|run#5|AssertionError|assert code_low == "InvalidRequestException", f"unexpected code={code_low!r}"
Issue-Key: chatbot.teams-channel-invalid-team-id-uuid-rejected|run#8|AssertionError|assert code == "InvalidRequestException", f"malformed code={code!r} message={message!r}"
Issue-Key: chatbot.tag-resource-nonexistent-arn-rejected|run#7|AssertionError|assert code == "UnauthorizedException", f"unexpected code={code!r}"
Issue-Key: chatbot.custom-action-get-nonexistent-rejected|run#6|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got code={code!r}"
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Chatbot error contract diverges: constraint refusals answer unmodeled ValidationException, nonexistent-ARN tagging answers 404 ResourceNotFoundException, and GetCustomAction is a 501 stub

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChimeWebhookConfiguration, DescribeSlackChannelConfigurations, ListCustomActions, CreateMicrosoftTeamsChannelConfiguration, TagResource, ListTagsForResource, GetCustomAction

## Description
Three distinct root causes in the chatbot emulator, all grounded in failing behavioral checks of the suite run (chatbot: 0/42 passed). Each must be closed before this issue is submitted.

=== Cause 1 of 3: input-constraint refusals are returned as an unmodeled `ValidationException` instead of `InvalidRequestException` ===
AWS: AWS Chatbot refuses a request whose input violates a modeled constraint with the modeled client error `InvalidRequestException` (HTTP 400). Smithy (services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/smithy-model/com-amazonaws-chatbot-wheatleyorchestration-20171011.json): `InvalidRequestException` is `@error("client") @httpError(400)`, documented "Your request input doesn't meet the constraints required by AWS Chatbot", and is listed on CreateChimeWebhookConfiguration, DescribeSlackChannelConfigurations, ListCustomActions and CreateMicrosoftTeamsChannelConfiguration. The constraints involved are all modeled: CreateChimeWebhookConfiguration.WebhookUrl pattern `^https://hooks\.chime\.aws/incomingwebhooks/[A-Za-z0-9\-]+?\?token=[A-Za-z0-9\-]+$`; DescribeSlackChannelConfigurations.NextToken (PaginationToken) pattern `^[a-zA-Z0-9=\/+_.\-,#:\\\"{}]{4,1276}$`; ListCustomActions.MaxResults @range min 1 max 100; CreateMicrosoftTeamsChannelConfiguration.TeamId (UUID) pattern `^[0-9A-Fa-f]{8}(?:-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}$`. The service model declares NO `ValidationException` shape at all (smithy_query for shape ValidationException: "shape 'ValidationException' not found").
Emulator: every one of those refusals comes back with error code `ValidationException` (HTTP 400) — a synthetic shape in the `smithy.framework` namespace, not in the chatbot model (services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/scaffolded.rs, VALIDATION_EXCEPTION_SCHEMA / `shape_name()` returning the literal "ValidationException"). Observed: `AssertionError: unexpected code='ValidationException'` on all four checks; the teams check also shows the message form the boundary validator produces, e.g. "1 validation error detected: Value '111...' at 'teamId' failed to satisfy constraint: Member must satisfy regular expression pattern: ...".
Why: a client that catches the modeled `InvalidRequestException` (botocore `client.exceptions.InvalidRequestException`, or a `Code == "InvalidRequestException"` branch) never matches, so malformed-input handling falls through to a generic/unknown-error path; the code it does see is not in the service model, so code generation and typed catch blocks cannot name it at all. The HTTP status (400) matches; the error CODE does not.
Fix: this service's boundary/constraint-validation error mapping — modeled-constraint refusals for chatbot must render as the modeled `InvalidRequestException` (400) rather than the synthetic `smithy.framework#ValidationException` of src/scaffolded.rs. The recorded AWS message for these refusals is the fixed literal "Invalid request body" (same text on all four operations); the requirement filed here is the status and code, with that message as a recorded example.
Traces: chatbot.chime-webhook-invalid-url-pattern-rejected, chatbot.describe-slack-channel-configurations-invalid-next-token-rejected, chatbot.list-custom-actions-maxresults-out-of-range-rejected, chatbot.teams-channel-invalid-team-id-uuid-rejected.

=== Cause 2 of 3: tagging/reading tags on a well-formed but nonexistent chatbot ARN answers 404 ResourceNotFoundException instead of 403 UnauthorizedException ===
AWS: recorded behavior only — the model lists `ResourceNotFoundException` (404) on both TagResource and ListTagsForResource and does NOT list `UnauthorizedException` on either, but the recording shows real AWS Chatbot failing CLOSED on authorization before any existence lookup: for a syntactically valid custom-action ARN in the caller's own account that was never created, both TagResource and ListTagsForResource are refused with error code `UnauthorizedException`, HTTP 403 (the service does model `UnauthorizedException` as `@error("client") @httpError(403)`, "The request was rejected because it doesn't have valid credentials for the target resource"). Recorded message: "Unauthorized" (example).
Emulator: TagResource against that ARN is refused with code `ResourceNotFoundException` — `AssertionError: unexpected code='ResourceNotFoundException'`.
Why: the divergence leaks resource existence. A client (or an operator policy test) distinguishing "you may not touch this" from "this does not exist" gets the opposite answer, and retry/permission-remediation logic keyed on 403 UnauthorizedException never fires while not-found handling fires spuriously. Status diverges too: 404 instead of 403.
Fix: the tagging operations' handlers/authorization ordering for chatbot resource ARNs — the authorization decision must be made against the requested ARN before the state lookup, so an unknown-but-well-formed ARN yields UnauthorizedException (403), not ResourceNotFoundException (404). Applies to both TagResource and ListTagsForResource, which the same check exercises.
Traces: chatbot.tag-resource-nonexistent-arn-rejected.

=== Cause 3 of 3: GetCustomAction is an unimplemented scaffold stub ===
AWS: Smithy: GetCustomAction POST /get-custom-action, success 200 returning `CustomAction`; modeled errors InternalServiceError (500), InvalidRequestException (400), ResourceNotFoundException (404), UnauthorizedException (403). For a well-formed CustomActionArn in the caller's account that was never created, the recording shows error code `UnauthorizedException`, HTTP 403, message "Unauthorized" (recorded behavior only for the choice of Unauthorized over ResourceNotFound — same fail-closed behavior as Cause 2); for an ARN that does exist, 200 with the action echoed back.
Emulator: answers HTTP 501 with error code `NotImplemented` ("operation GetCustomAction is scaffolded but not implemented in this emulator") for every call — `AssertionError: expected UnauthorizedException, got code='NotImplemented'`. Confirmed in the checkout: services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/get_custom_action.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("GetCustomAction"))`. NOTE: this stub is NOT one of the four the suite auto-filed (GetAccountPreferences, CreateChimeWebhookConfiguration, CreateCustomAction, ListMicrosoftTeamsConfiguredTeams), so no existing issue covers it.
Why: no client can read a custom action back; a 501 is not a modeled outcome of this operation, so both the success path and the fail-closed 403 path are unavailable.
Fix: implement the handler in services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/src/ops/get_custom_action.rs (200 + CustomAction for an existing ARN, 403 UnauthorizedException for a well-formed unknown ARN) and delete the SCAFFOLDED marker line.
Traces: chatbot.custom-action-get-nonexistent-rejected.

## Evidence
Suite run: chatbot 0/42 passed; 8 mismatched, 34 errored.

Cause 1 (4 checks), expected vs actual error code — status 400 in both cases:
- chatbot.chime-webhook-invalid-url-pattern-rejected: expected code InvalidRequestException (message "Invalid request body"); actual `AssertionError: unexpected code='ValidationException'`. Request: CreateChimeWebhookConfiguration with WebhookUrl="https://hooks.chime.aws/incomingwebhooks/missing-token-param" (no ?token=, violates the modeled pattern).
- chatbot.describe-slack-channel-configurations-invalid-next-token-rejected: expected InvalidRequestException; actual `unexpected code='ValidationException'`. Request: DescribeSlackChannelConfigurations(NextToken="invalid!token!!").
- chatbot.list-custom-actions-maxresults-out-of-range-rejected: expected InvalidRequestException; actual `unexpected code='ValidationException'`. Request: ListCustomActions(MaxResults=0) with client-side validation disabled (also asserts MaxResults=101).
- chatbot.teams-channel-invalid-team-id-uuid-rejected: expected InvalidRequestException; actual `malformed code='ValidationException' message="1 validation error detected: Value '111111111111111111111111111111111111' at 'teamId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Fa-f]{8}(?:-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}$"`.
Model evidence: smithy_query(shape="ValidationException") -> not found in the chatbot model; smithy_query(shape="InvalidRequestException") -> client error, httpError 400, bound to all four operations.

Cause 2 (1 check):
- chatbot.tag-resource-nonexistent-arn-rejected: TagResource(ResourceARN="arn:aws:chatbot:<region>:<account>:custom-action/<never-created>", Tags=[{k,v}]). Expected 403 UnauthorizedException (message "Unauthorized"); actual `AssertionError: unexpected code='ResourceNotFoundException'` (404). The same check then expects UnauthorizedException from ListTagsForResource on the same ARN.

Cause 3 (1 check):
- chatbot.custom-action-get-nonexistent-rejected: GetCustomAction(CustomActionArn="arn:aws:chatbot:<region>:<account>:custom-action/sim2real-never-created-action"). Expected 403 UnauthorizedException; actual `AssertionError: expected UnauthorizedException, got code='NotImplemented'` (HTTP 501, "operation GetCustomAction is scaffolded but not implemented in this emulator"). Stub confirmed at src/ops/get_custom_action.rs lines 4 and 161.

## Steps to Reproduce
Boot the emulator and point a botocore chatbot client at it, then:

Cause 1:
1. chatbot.create_chime_webhook_configuration(WebhookDescription="bad url", WebhookUrl="https://hooks.chime.aws/incomingwebhooks/missing-token-param", SnsTopicArns=[<topic>], IamRoleArn=<role>, ConfigurationName=<name>) -> observe Error.Code == "ValidationException"; AWS returns InvalidRequestException (400).
2. chatbot.describe_slack_channel_configurations(NextToken="invalid!token!!") -> same.
3. With Config(parameter_validation=False): chatbot.list_custom_actions(MaxResults=0) and MaxResults=101 -> same.
4. chatbot.create_microsoft_teams_channel_configuration(... TeamId="111111111111111111111111111111111111" ...) -> same.

Cause 2:
5. chatbot.tag_resource(ResourceARN=f"arn:aws:chatbot:{region}:{account}:custom-action/never-created", Tags=[{"TagKey":"k","TagValue":"v"}]) -> observe 404 ResourceNotFoundException; AWS returns 403 UnauthorizedException. Repeat with chatbot.list_tags_for_resource on the same ARN.

Cause 3:
6. chatbot.get_custom_action(CustomActionArn=f"arn:aws:chatbot:{region}:{account}:custom-action/never-created") -> observe HTTP 501 NotImplemented; AWS returns 403 UnauthorizedException (and 200 with the action for an ARN that exists).

Or replay the checks directly: chatbot.chime-webhook-invalid-url-pattern-rejected, chatbot.describe-slack-channel-configurations-invalid-next-token-rejected, chatbot.list-custom-actions-maxresults-out-of-range-rejected, chatbot.teams-channel-invalid-team-id-uuid-rejected, chatbot.tag-resource-nonexistent-arn-rejected, chatbot.custom-action-get-nonexistent-rejected.
