Issue-Source: trace-sweep-divergence
Issue-Trace: chatbot-slack-configuration-error-paths
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chatbot-slack-configuration-error-paths
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot|CreateSlackChannelConfiguration|4xx->2xx|$.message
Issue-Key: chatbot|CreateSlackChannelConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: chatbot|CreateSlackChannelConfiguration|4xx->2xx|status
Issue-Commit: 5be2af8c64fc264fdb55b7f2409ec989192d8052

# CreateSlackChannelConfiguration accepts unauthorized Slack workspace instead of returning 400

> _Note: this issue was identified against the code base at commit `5be2af8c64fc264fdb55b7f2409ec989192d8052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSlackChannelConfiguration

## Description
Cause 1 of 1: CreateSlackChannelConfiguration does not enforce that the Slack workspace named by SlackTeamId is authorized with the calling AWS account; it unconditionally creates the configuration.

AWS: AWS Chatbot requires a Slack workspace to be authorized with the AWS account (the one-time Slack client/OAuth setup) before a Slack channel configuration can be created for it. When the SlackTeamId refers to a workspace that is not authorized for the account, the request is rejected with HTTP 400 carrying a modeled client error. Smithy: CreateSlackChannelConfiguration (POST /create-slack-channel-configuration, success code 201) models SlackTeamId as required (@length 1..255, @pattern ^[0-9A-Z]{1,255}$) and models the client errors InvalidRequestException (httpError 400, member `message`), InvalidParameterException (httpError 400, member `message`), ConflictException (409) and LimitExceededException (403). The recorded response is HTTP 400 with a body whose error payload is at the lowercase `message` member — i.e. the InvalidRequestException/InvalidParameterException shape family (lowercase `message`), not ResourceNotFoundException (uppercase `Message`) — plus an x-amzn-errortype header. Recorded message, as an EXAMPLE of the rule (built from the request's SlackTeamId and the caller's account id): 'Unable to create the configuration because Slack workspace T47ACFD6FF1 is not authorized with AWS account 000000000000. See  https://docs.aws.amazon.com/chatbot/latest/adminguide/slack-setup.html#slack-client-setup'. The requirement is the status (400) and a modeled 400 client error whose message names the unauthorized workspace id and the account; the exact recorded wording and URL are an example, not a requirement.

Emulator: returns HTTP 201 with a created ChannelConfiguration. The replay reports 'status: expected 400, got 201', '$.message: missing (expected the unauthorized-workspace message, got None)' and '$headers.x-amzn-errortype: missing (expected <shaped-not-asserted>, got None)' — so no error envelope at all is emitted and the configuration is persisted.

Why: a client that relies on AWS's behavior gets the opposite of the real control-plane outcome. Code paths that probe for workspace authorization (or that treat a successful create as proof the workspace is wired up) see success where AWS fails, so error handling for the unauthorized-workspace case is never exercised, and the emulator's state then contains a Slack channel configuration bound to a workspace that was never authorized — later list/describe/delete flows built on that state diverge too. Tests asserting the 400 + error-type contract fail against the emulator while passing against AWS.

Fix: CreateSlackChannelConfiguration's handler/validation in the chatbot service crate. Before persisting the configuration, check SlackTeamId against the set of Slack workspaces authorized for the account in the service's state; when it is absent, return the modeled 400 client error (lowercase `message` member, x-amzn-errortype header set) instead of creating the resource and returning 201. The authorized-workspace set needs to be part of chatbot's state model so that only workspaces registered there can back a configuration.

Traces: chatbot-slack-configuration-error-paths#task#6 (the only member of this cluster).

## Evidence
Fixture chatbot-slack-configuration-error-paths, step task#6, operation CreateSlackChannelConfiguration.
Expected (recorded AWS): HTTP 400, modeled client error, body member `message` present (example text: 'Unable to create the configuration because Slack workspace T47ACFD6FF1 is not authorized with AWS account 000000000000. See  https://docs.aws.amazon.com/chatbot/latest/adminguide/slack-setup.html#slack-client-setup'), response header x-amzn-errortype present.
Actual (emulator): HTTP 201 with a created ChannelConfiguration; $.message absent; x-amzn-errortype absent.
Replay divergence_key: service=chatbot|CreateSlackChannelConfiguration|status=400->201|keys=$.message,$headers.x-amzn-errortype,status
Model evidence: Smithy CreateSlackChannelConfiguration errors = ConflictException(409), CreateSlackChannelConfigurationException(500), InvalidParameterException(400, member `message`), InvalidRequestException(400, member `message`), LimitExceededException(403); http success code 201.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or hand-drive the step:
2. POST /create-slack-channel-configuration against the chatbot endpoint with a well-formed body whose SlackTeamId is a Slack workspace that has NOT been authorized with the account (e.g. SlackTeamId matching ^[0-9A-Z]{1,255}$ such as T47ACFD6FF1), plus the required SlackChannelId, IamRoleArn and ConfigurationName.
3. Observe the emulator answers 201 with a ChannelConfiguration body.
4. AWS answers 400 with a modeled client error (lowercase `message` member naming the workspace id and account, x-amzn-errortype header set) and creates nothing.
Fixture/step: chatbot-slack-configuration-error-paths#task#6.
