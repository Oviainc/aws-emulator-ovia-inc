Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.deploy-iot-topic-rule-cfn-routes-messages iot.deploy-topic-rule-cfn-routes-matching-messages iot.deploy-topic-rule-cfn-routes-message iot.shadow-client-token-echoed-not-persisted iot.topic-rule-with-unconfirmed-destination-blocks-delivery
Issue-Key: iot|GetTopicRule|UnauthorizedException
Issue-Key: iot.deploy-topic-rule-cfn-routes-message|run#20|AssertionError|assert match_marker in found_markers, "matching message was never routed to the SQS action"
Issue-Key: iot.shadow-client-token-echoed-not-persisted|run#11|AssertionError|assert "delta" not in accepted["state"], (
Issue-Key: iot.topic-rule-with-unconfirmed-destination-blocks-delivery|_self_test_endpoint_with_retry#1|Timeout|time.sleep(interval_s)
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# iot: CFN-deployed AWS::IoT::TopicRule is not a working rule, UpdateThingShadow's reply leaks the computed state.delta, and an HTTP topic-rule destination is never sent its DestinationConfirmation (3 causes)

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTopicRule

## Description
Three distinct root causes in the iot surface, grounded in 5 failing behavioral checks. Each must be closed before this issue is submitted.

=== Cause 1 of 3: a CloudFormation stack containing AWS::IoT::TopicRule reaches CREATE_COMPLETE but no topic rule is registered with the IoT service ===
AWS: CloudFormation's AWS::IoT::TopicRule resource provider calls iot:CreateTopicRule with the template's RuleName/TopicRulePayload, so once the stack is CREATE_COMPLETE the rule is an ordinary IoT topic rule: GetTopicRule(ruleName) answers 200 with {ruleArn, rule} (Smithy: GetTopicRule GET /rules/{ruleName}, output GetTopicRuleResponse{ruleArn: RuleArn, rule: TopicRule}; modelled errors InternalException 500, InvalidRequestException 400, ServiceUnavailableException 503, UnauthorizedException 401 — there is no ResourceNotFoundException, which is why a rule that does not exist is reported as a 401 Unauthorized by this operation), and the rule's actions run on a matching publish.
Emulator: the stack is driven to CREATE_COMPLETE (the stack_create_complete waiter returns, and for one check the stack's Outputs are read back successfully), but the rule the template declared does not exist in the IoT store. GetTopicRule on it answers 401 UnauthorizedException with message "Not authorized to perform: iot:GetTopicRule on resource: arn:aws:iot:<region>:<account>:rule/<ruleName>" — fixed text "Not authorized to perform: iot:GetTopicRule on resource: " followed by the rule ARN built from the request's ruleName and the caller's region/account; the recorded instances were .../rule/s2r_rule_in_726005191b8c and .../rule/sim2real_topicrule_bba685bfc216. The third check never reads the rule at all and instead publishes to the rule's SQL topic: the Sqs action never delivers, so the matching marker never arrives on the queue the same stack created ("matching message was never routed to the SQS action").
Why: a client that provisions its IoT rules with CloudFormation — the documented, and for most users the only — way to deploy them gets a green stack and no rule. Every later control-plane read of that rule is refused as unauthorized (an authorization problem, which sends the user hunting IAM), and the whole data path the stack was supposed to stand up is silently dead.
Fix: the CloudFormation resource provider for AWS::IoT::TopicRule must actually create the rule in the iot service's topic-rule store on stack create (and remove it on stack delete), instead of settling the logical resource with no side effect. If the type is simply unrecognised and silently skipped, the stack must not reach CREATE_COMPLETE. Related open issue 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md covers other unrecognised IoT types (AWS::IoT::Authorizer) but not this one: here the stack succeeds, which is the worse failure. The rule-action execution half overlaps open issue 20261024T035602Z-iot-topic-rules-never-execute-their-actions-when.md, which does not own any of these three checks.
Traces: iot.deploy-iot-topic-rule-cfn-routes-messages, iot.deploy-topic-rule-cfn-routes-matching-messages, iot.deploy-topic-rule-cfn-routes-message.

=== Cause 2 of 3: the UpdateThingShadow accepted reply carries the server-computed state.delta ===
AWS: recorded behavior only (the model types the reply as an opaque @httpPayload JsonDocument blob, so it constrains nothing here). The recording shows the UpdateThingShadow reply document has exactly the keys {state, metadata, version, timestamp, clientToken} and that state carries only what the request wrote (desired and/or reported) — the reply has NO delta. The STORED document, read back by GetThingShadow, is the mirror image: no clientToken, but it does carry the computed state.delta. The two documents are deliberately not interchangeable in either direction.
Emulator: the reply's state contains ['delta', 'desired'] after an update that wrote only state.desired={"a": 1} — the handler builds the reply from the same merged stored document it persists (services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/update_thing_shadow.rs:315 builds `stored_document = merged.document(version)` and line 351 hands that same document to `reply_with_client_token`), so the derived delta node leaks into the reply.
Why: a device or client that reads state.delta off the update reply to decide what it still has to apply sees a delta on its OWN write, so it reacts to settings it just reported, or treats the reply and a subsequent GetThingShadow as the same document and loops. Clients that assert the reply's key set outright break.
Fix: the UpdateThingShadow reply must be built separately from the persisted document — the reply projection in services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/update_thing_shadow.rs (and the `document()` / `reply_with_client_token` helpers in that crate's src/state.rs) must omit the derived delta node, while GetThingShadow keeps reporting it.
Traces: iot.shadow-client-token-echoed-not-persisted.

=== Cause 3 of 3: CreateTopicRuleDestination never delivers a DestinationConfirmation request to the confirmationUrl, so an HTTP topic-rule destination can never be confirmed ===
AWS: recorded behavior only for the delivery itself (the model types CreateTopicRuleDestination's httpUrlConfiguration.confirmationUrl but says nothing about the callback). AWS POSTs a DestinationConfirmation message carrying a confirmationToken to the confirmationUrl when the destination is created (and again on UpdateTopicRuleDestination(status=IN_PROGRESS)); the caller then passes that token to ConfirmTopicRuleDestination, the destination settles at DISABLED, and only after UpdateTopicRuleDestination(status=ENABLED) does an http action against it deliver.
Emulator: no confirmation request is ever observed at the endpoint, so the check cannot obtain a token and makes no progress at all — it is killed by the harness's 60 s ceiling ("Check timed out after 60.0s") while still polling for the token, long before its own 240 s token budget. Consequently neither half of the contract under test (unconfirmed destination blocks delivery; confirmed+enabled destination delivers) is observable.
Why: a client cannot bring an HTTP topic rule destination out of IN_PROGRESS at all, so the entire HTTP-action delivery path of IoT rules is unreachable in this emulator — and nothing distinguishes "blocked because unconfirmed" from "nothing is ever delivered", which is exactly the safety property this operation exists to provide.
Fix: the topic-rule-destination handling in the iot crate (services/service-com-amazonaws-iot-awsiotservice/src/ops/create_topic_rule_destination.rs and update_topic_rule_destination.rs, plus the destination state in that crate's src/state.rs) must emit the DestinationConfirmation callback to the configured confirmationUrl and mint the token that ConfirmTopicRuleDestination accepts. Hypothesis: the confirming endpoint in this check is an API Gateway HTTP API fronting a Lambda, so the same Lambda-package execution gap named in open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (which owns the sibling check iot.e2e-topic-rule-destination-confirm-then-deliver but not this one) may also contribute; the missing callback is nevertheless the iot-side defect.
Traces: iot.topic-rule-with-unconfirmed-destination-blocks-delivery.

## Evidence
Cause 1 — iot.deploy-iot-topic-rule-cfn-routes-messages:
  expected: GetTopicRule(ruleName=<rule created by the stack>) -> 200 {ruleArn, rule} with rule.actions[0].republish.topic == the template's Topic
  actual:   botocore.errorfactory.UnauthorizedException: An error occurred (UnauthorizedException) when calling the GetTopicRule operation: Not authorized to perform: iot:GetTopicRule on resource: arn:aws:iot:us-west-2:000000000000:rule/s2r_rule_in_726005191b8c
           (fixed text: "Not authorized to perform: iot:GetTopicRule on resource: "; variable part: the rule ARN from the request's ruleName + caller region/account)
Cause 1 — iot.deploy-topic-rule-cfn-routes-matching-messages:
  actual:   An error occurred (UnauthorizedException) when calling the GetTopicRule operation: Not authorized to perform: iot:GetTopicRule on resource: arn:aws:iot:us-west-2:000000000000:rule/sim2real_topicrule_bba685bfc216
Cause 1 — iot.deploy-topic-rule-cfn-routes-message (stack reached CREATE_COMPLETE and its Outputs were read):
  expected: the marker published to the rule's SQL topic arrives on the stack's SQS queue; the marker published to a non-matching topic does not
  actual:   AssertionError: matching message was never routed to the SQS action

Cause 2 — iot.shadow-client-token-echoed-not-persisted:
  expected: the UpdateThingShadow reply document's state holds only what was written (['desired']); its key set is exactly {state, metadata, version, timestamp, clientToken}
  actual:   AssertionError: the update REPLY carries no computed delta, only what was written: ['delta', 'desired']
  (the clientToken echo, the reply key set and version==1 all held; only the delta node diverges)

Cause 3 — iot.topic-rule-with-unconfirmed-destination-blocks-delivery:
  expected: after create_topic_rule_destination(httpUrlConfiguration.confirmationUrl=<url>), a DestinationConfirmation carrying a confirmationToken reaches <url>
  actual:   Check timed out after 60.0s — no confirmation request ever observed, so ConfirmTopicRuleDestination is never reachable

## Steps to Reproduce
Cause 1:
1. cloudformation:CreateStack with a template whose Resources contain an AWS::IoT::TopicRule (RuleName=<name>, TopicRulePayload.Sql="SELECT * FROM '<topic>'", RuleDisabled=false, Actions=[{Sqs|Republish: ...}]).
2. Wait for the stack_create_complete waiter -> it returns CREATE_COMPLETE.
3. Call iot:GetTopicRule(ruleName=<name>) -> expect 200; observe 401 UnauthorizedException "Not authorized to perform: iot:GetTopicRule on resource: arn:aws:iot:<region>:<account>:rule/<name>".
4. Publish to <topic> via iot-data:Publish and poll the action target -> nothing is ever delivered.

Cause 2:
1. iot:CreateThing(thingName=T).
2. iot-data:UpdateThingShadow(thingName=T, payload={"clientToken":"tok","state":{"desired":{"a":1}}}).
3. Parse the reply payload -> expect state keys == ['desired'] and no 'delta'; observe ['delta','desired'].
4. iot-data:GetThingShadow(thingName=T) -> the stored document correctly carries state.delta == {"a":1}; the reply must not.

Cause 3:
1. Stand up any HTTPS endpoint that logs the request it receives.
2. iot:CreateTopicRuleDestination(destinationConfiguration={"httpUrlConfiguration":{"confirmationUrl":<url>}}).
3. Watch <url> for a DestinationConfirmation POST carrying confirmationToken (optionally re-trigger with iot:UpdateTopicRuleDestination(arn=<arn>, status="IN_PROGRESS")) -> none arrives, so iot:ConfirmTopicRuleDestination can never be called and the destination stays IN_PROGRESS.
