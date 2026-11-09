Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lexv2-models.cfn-lex-bot-properties-and-getatt-read-back-then-resource-not-found-after-delete cloudformation__lexv2-models.cfn-lex-bot-duplicate-name-rejected-leaving-the-original cloudformation__lexv2-models.cfn-lex-bot-version-and-alias-read-back-then-resource-not-found-after-delete cloudformation__lexv2-models.cfn-lex-resource-policy-read-back-then-resource-not-found-after-delete
Issue-Key: cloudformation__lexv2-models.cfn-lex-bot-properties-and-getatt-read-back-then-resource-not-found-after-delete|run#14|AssertionError|assert listed == [ref_bot_id], f"ListBots under {bot_name!r} gave {listed!r}, expected Ref {ref_bot_id!r}"
Issue-Key: lex-models-v2|DescribeBot|ValidationException
Issue-Key: cloudformation__lexv2-models.cfn-lex-bot-version-and-alias-read-back-then-resource-not-found-after-delete|stack_outputs#12|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__lexv2-models.cfn-lex-resource-policy-read-back-then-resource-not-found-after-delete|stack_outputs#11|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 0e87dfce12d4662a21658c8a4f6bcf11c3f1d791

# cloudformation__lexv2-models: 2 net-new causes — AWS::Lex::Bot is stub-provisioned with the template Name as Ref/PhysicalResourceId, and Lex child-resource stack Outputs carry no OutputValue

> _Note: this issue was identified against the code base at commit `0e87dfce12d4662a21658c8a4f6bcf11c3f1d791`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Two distinct root causes in CloudFormation's AWS::Lex::* resource providers. Both were reached from CREATE_COMPLETE stacks, so CloudFormation reports success for work it never did.

Cause 1 of 2: AWS::Lex::Bot is stub-provisioned — nothing is created at Lex, and Ref / PhysicalResourceId hand back the template's Name instead of the Lex-assigned bot id.
Failing checks: cloudformation__lexv2-models.cfn-lex-bot-properties-and-getatt-read-back-then-resource-not-found-after-delete, cloudformation__lexv2-models.cfn-lex-bot-duplicate-name-rejected-leaving-the-original.
AWS: for AWS::Lex::Bot, Ref returns the bot's Id and that Id is the resource's PhysicalResourceId; the bot is really created by Lex's CreateBot, so it is then visible to the Lex control plane. Smithy (lexmodelsv2): DescribeBot.botId and ListBots' BotSummary.botId target `Id`, which is @length(min:10,max:10) and @pattern "^[0-9a-zA-Z]+$" — an opaque 10-character alphanumeric id Lex assigns, never the bot's Name (`Name` is @length 1-100, @pattern "^([0-9a-zA-Z][_-]?){1,100}$", so names routinely contain '-' and exceed 10 characters and can never be a valid Id). ListBots with filter {name: BotName, values:[<name>], operator: EQ} must list exactly the bot the stack created, and DescribeBot(botId=<that id>) must return the declared properties (botName, roleArn from GetAtt Role.Arn, dataPrivacy, idleSessionTTLInSeconds, botStatus "Available").
Emulator: the stack reaches CREATE_COMPLETE, Ref Bot and the stack resource's PhysicalResourceId both equal the template's Name property, and Lex holds no such bot. ListBots filtered on that exact name returns an empty botSummaries list: "AssertionError: ListBots under 'sim2real-behav-lexbot-<suffix>' gave [], expected Ref 'sim2real-behav-lexbot-<suffix>'" — note the expected value is itself the name, which is the second half of this cause. Feeding that Ref value to DescribeBot is then refused by the framework's constraint validator with HTTP 400, code `ValidationException`, message "2 validation errors detected: Value at 'botId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]+$; Value at 'botId' failed to satisfy constraint: Member must have length less than or equal to 10" (fixed text is the AWS constraint-violation template; 'botId', the pattern and the bound 10 are the member and its modeled constraints — the bot name in the request is the offending value and is not part of the template). Because no bot exists, nothing downstream of the Ref can work: the duplicate-name check cannot even read stack A's bot back before deploying stack B, so the second stack's expected rollback with handler message "Couldn't create the bot. A bot with the same name already exists. Use a different name and try your request again." (HandlerErrorCode AlreadyExists) is never exercised either.
Why: a client that deploys a Lex bot with CloudFormation gets a stack that claims the bot exists and an identifier that is not a bot id. Every subsequent call — DescribeBot, ListBots, building a locale, creating a version or alias, attaching a resource policy, or any lexv2-runtime conversation against the deployed bot — fails, with the Ref value rejected as a malformed botId rather than reported as missing. Stack deletion likewise removes nothing from Lex, so a duplicate-name deploy is never refused and name uniqueness is unenforced.
Fix: the AWS::Lex::Bot resource provider in the CloudFormation crate's resource-type registry — its create path must call the lexmodelsv2 CreateBot handler and store the returned botId as the resource's physical id (so Ref and PhysicalResourceId resolve to it, and GetAtt Arn is built as arn:<partition>:lex:<region>:<account>:bot/<botId>), its delete path must call DeleteBot, and a CreateBot refusal must fail the resource and roll the stack back carrying Lex's own message and HandlerErrorCode.

Cause 2 of 2: DescribeStacks returns Outputs entries for AWS::Lex::BotVersion, AWS::Lex::BotAlias and AWS::Lex::ResourcePolicy with an OutputKey but no OutputValue member at all.
Failing checks: cloudformation__lexv2-models.cfn-lex-bot-version-and-alias-read-back-then-resource-not-found-after-delete, cloudformation__lexv2-models.cfn-lex-resource-policy-read-back-then-resource-not-found-after-delete.
AWS: every Output of a stack in CREATE_COMPLETE carries both OutputKey and OutputValue in DescribeStacks — an intrinsic the provider cannot resolve (Ref or Fn::GetAtt on an attribute the resource type does not expose) is a template/handler error that fails the resource and rolls the stack back, it is never a successful stack with a value-less Output. Here the unresolved outputs are {"Ref": "BotAlias"} (AWS: the composite "<botAliasId>|<botId>"), Fn::GetAtt BotVersion.BotVersion (the version number Lex assigned), Fn::GetAtt BotAlias.Arn (arn:<partition>:lex:<region>:<account>:bot-alias/<botId>/<botAliasId>), {"Ref": "ResourcePolicy"} (AWS: "<resourceArn>/ResourcePolicy") and Fn::GetAtt ResourcePolicy.RevisionId (the revisionId DescribeResourcePolicy reports).
Emulator: the stack reports CREATE_COMPLETE and DescribeStacks lists the Output entries, but the entries have no OutputValue key, so any client reading them raises on the missing member: "KeyError: 'OutputValue'" from `{o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}`. Both checks die there, before they can compare the composite PhysicalResourceIds, the alias/version read-backs or the resource-policy statement.
Why: every consumer of a Lex stack's outputs breaks. botocore/AWS SDK response models declare OutputValue, so SDK users index it and crash rather than getting a value or a clear failure; CloudFormation cross-stack Fn::ImportValue and `aws cloudformation describe-stacks --query Outputs` are equally unusable. Worse, the stack still reads as CREATE_COMPLETE, so a deployment pipeline believes it succeeded.
Fix: CloudFormation's output-resolution path in the stack engine (Ref/Fn::GetAtt resolution against resource attributes, and the DescribeStacks Outputs serializer) plus the AWS::Lex::BotVersion, AWS::Lex::BotAlias and AWS::Lex::ResourcePolicy providers' attribute/physical-id surfaces: each provider must expose its composite primary identifier and its documented GetAtt attributes, and an output the engine cannot resolve must fail the resource rather than be emitted without OutputValue.

## Evidence
Suite: simparity-behavioral-aws-v1, service heading `### cloudformation__lexv2-models` — 0/4 checks passed.

Cause 1:
- cfn-lex-bot-properties-and-getatt-read-back-then-resource-not-found-after-delete [mismatch]
  expected: ListBots(filters=[{name:BotName,values:[<bot_name>],operator:EQ}]) lists exactly [<Lex-assigned 10-char botId>], and that id equals Ref Bot / PhysicalResourceId.
  actual:   `AssertionError: ListBots under 'sim2real-behav-lexbot-3294ab3d729b' gave [], expected Ref 'sim2real-behav-lexbot-3294ab3d729b'` — Ref resolved to the template's Name, and Lex holds no bot under that name (the id/suffix shown is one run's value; the defect is Ref==Name and the empty listing).
- cfn-lex-bot-duplicate-name-rejected-leaving-the-original [error]
  expected: DescribeBot(botId=outputs["BotId"]) returns the stack's bot.
  actual:   HTTP 400 `ValidationException`: "2 validation errors detected: Value at 'botId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]+$; Value at 'botId' failed to satisfy constraint: Member must have length less than or equal to 10" — the Ref value is a bot name, not an Id.
  Smithy lexmodelsv2: DescribeBot.botId -> Id, @length(min 10, max 10), @pattern ^[0-9a-zA-Z]+$, httpLabel, required; errors include ResourceNotFoundException (404) and ValidationException (400).

Cause 2:
- cfn-lex-bot-version-and-alias-read-back-then-resource-not-found-after-delete [error]
  Outputs: BotId={Ref Bot}, BotVersionNumber={Fn::GetAtt BotVersion.BotVersion}, BotAliasId={Ref BotAlias}, BotAliasArn={Fn::GetAtt BotAlias.Arn}.
  expected: DescribeStacks Outputs each carry OutputKey and OutputValue.
  actual:   `KeyError: 'OutputValue'` at `stack_outputs()` on a CREATE_COMPLETE stack.
- cfn-lex-resource-policy-read-back-then-resource-not-found-after-delete [error]
  Outputs: BotArn={Fn::GetAtt Bot.Arn}, PolicyId={Ref ResourcePolicy}, PolicyRevisionId={Fn::GetAtt ResourcePolicy.RevisionId}.
  expected: same.
  actual:   `KeyError: 'OutputValue'` at `stack_outputs()` on a CREATE_COMPLETE stack.

No open issue covers CloudFormation's AWS::Lex::* resource types (searched 'lex' and 'cloudformation'); the closest open items are value-less-Output reports for other services (e.g. 20261109T141301Z-cloudformation-s3tables-..., 20261109T143402Z-cloudformation-mq-...), which do not carry these replay keys.

## Steps to Reproduce
Cause 1:
1. CreateStack with Capabilities=[CAPABILITY_IAM] and a template containing an AWS::IAM::Role (lexv2.amazonaws.com trust) and an AWS::Lex::Bot with Properties {Name: <unique bot name>, RoleArn: {Fn::GetAtt [Role, Arn]}, DataPrivacy: {ChildDirected: false}, IdleSessionTTLInSeconds: 300} and Outputs {BotId: {Ref: Bot}}.
2. Wait for CREATE_COMPLETE; read DescribeStackResources PhysicalResourceId for logical id "Bot" and the BotId output — both come back as the bot NAME instead of a 10-character Lex id.
3. lexv2-models ListBots(filters=[{name:"BotName",values:[<bot name>],operator:"EQ"}]) -> botSummaries is empty.
4. lexv2-models DescribeBot(botId=<that Ref value>) -> 400 ValidationException about the ^[0-9a-zA-Z]+$ pattern and the max length 10, instead of the bot (or ResourceNotFoundException).
5. Deploy a second stack with the same bot Name: it should reach ROLLBACK_COMPLETE with the Bot resource's reason carrying "Couldn't create the bot. A bot with the same name already exists. Use a different name and try your request again." / HandlerErrorCode AlreadyExists.

Cause 2:
1. CreateStack a template whose Bot additionally declares BotLocales + AutoBuildBotLocales, plus AWS::Lex::BotVersion {BotId: {Ref: Bot}, BotVersionLocaleSpecification:[{LocaleId: en_US, BotVersionLocaleDetails:{SourceBotVersion: DRAFT}}]} and AWS::Lex::BotAlias {BotId: {Ref: Bot}, BotAliasName: ..., BotVersion: {Fn::GetAtt [BotVersion, BotVersion]}}, with Outputs BotVersionNumber={Fn::GetAtt [BotVersion, BotVersion]}, BotAliasId={Ref BotAlias}, BotAliasArn={Fn::GetAtt [BotAlias, Arn]}.
2. Wait for CREATE_COMPLETE, then DescribeStacks(StackName=<stack id>) and read Stacks[0].Outputs — entries have OutputKey but no OutputValue.
3. Repeat with a template declaring AWS::Lex::ResourcePolicy {ResourceArn: {Fn::GetAtt [Bot, Arn]}, Policy: <policy>} and Outputs PolicyId={Ref ResourcePolicy}, PolicyRevisionId={Fn::GetAtt [ResourcePolicy, RevisionId]} — same missing OutputValue.
