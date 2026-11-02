Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs__sts.change-set-notification-arns-deliver-every-stack-event-to-the-topic
Issue-Key: cloudformation__sns__sqs__sts.change-set-notification-arns-deliver-every-stack-event-to-the-topic|run#17|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack["Outputs"]}
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# cloudformation__sns__sqs__sts: an executed change set's stack Outputs are returned without OutputValue, so AWS::NotificationARNs never resolves and no output can be read

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
ONE net-new root cause of service `cloudformation__sns__sqs__sts`, grounded in the single failing check in trace_ids. The check was not marked ALREADY FILED or OWNED by the replay report, and there is no open issue for this service. The sibling service `cloudformation__sns__sqs` shows the same projection defect through the CDK deploy path and is filed separately in this sweep (its Cause 1), because an issue carries exactly one service token.

=== Cause 1 of 1: DescribeStacks returns Output entries with no OutputValue member ===
AWS: Smithy (com.amazonaws.cloudformation#Output) declares the Output structure as OutputKey, OutputValue, Description, ExportName. After ExecuteChangeSet drives the stack to CREATE_COMPLETE, DescribeStacks returns every declared Output with BOTH OutputKey and OutputValue populated. Here the template's single Output is `{"Topics": {"Value": {"Fn::Join": [",", {"Ref": "AWS::NotificationARNs"}]}}}`, and the change set was created with NotificationARNs=[<topic>] (Smithy: CreateChangeSetInput$NotificationARNs, list of NotificationARN, length 0-5), so AWS resolves the pseudo-parameter to that one topic ARN and OutputValue is exactly the topic ARN — the comma-joined list in general, and the empty string when the stack has no notification topics. Recorded behavior only for the pseudo-parameter's resolution; the member set is modeled.
Emulator: ExecuteChangeSet succeeds and the stack reports CREATE_COMPLETE with NotificationARNs == [<topic>], but the Outputs list carries an entry with NO OutputValue key, so a client reading `o["OutputValue"]` fails with `KeyError: 'OutputValue'`.
Why: no output of the stack can be read, whatever its Value expression — SDKs and the CLI model Output.OutputValue as populated on a completed stack, so a deployment that reports success returns no values and every downstream step keyed on an output (a topic ARN, a queue URL, a role ARN) breaks or silently reads null. The specific loss here is that AWS::NotificationARNs resolves to nothing, so a template cannot discover its own notification topics.
Fix: the DescribeStacks (and DescribeChangeSet-driven) output projection together with the template Outputs evaluator — resolve Ref AWS::NotificationARNs and Fn::Join over it from the stack's recorded NotificationARNs, and always serialize OutputValue, using the empty string for an output that resolves to nothing rather than omitting the member.
Traces: cloudformation__sns__sqs__sts.change-set-notification-arns-deliver-every-stack-event-to-the-topic.

## Evidence
cloudformation__sns__sqs__sts.change-set-notification-arns-deliver-every-stack-event-to-the-topic [error]:
  File "check_cloudformation__sns__sqs__sts_change_set_notification_arns_deliver_every_stack_event_to_the_topic.py", line 176, in run
    outputs = {o["OutputKey"]: o["OutputValue"] for o in stack["Outputs"]}
  KeyError: 'OutputValue'

expected: after CreateChangeSet(ChangeSetType="CREATE", NotificationARNs=[<topic>], TemplateBody with Outputs {"Topics": {"Value": {"Fn::Join": [",", {"Ref": "AWS::NotificationARNs"}]}}}) and ExecuteChangeSet, DescribeStacks reports StackStatus CREATE_COMPLETE, NotificationARNs == [<topic>] and Outputs == [{"OutputKey": "Topics", "OutputValue": "<topic arn>"}] (the check asserts outputs == {"Topics": <topic>}).
actual: the change set reaches CREATE_COMPLETE and the stack reports the NotificationARNs correctly, but the Outputs entry has no OutputValue member at all, so the output is unreadable; the later assertions of this check (one Key='value' notification per StackEvent) were never reached.

## Steps to Reproduce
1. sns.CreateTopic; sqs.CreateQueue; set the queue policy to allow Principal {"Service":"sns.amazonaws.com"} sqs:SendMessage on the queue ARN conditioned on ArnEquals aws:SourceArn = the topic ARN; sns.Subscribe(Protocol="sqs", Endpoint=<queue arn>).
2. cloudformation.CreateChangeSet(StackName=<new>, ChangeSetType="CREATE", NotificationARNs=[<topic arn>], TemplateBody={"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},"Outputs":{"Topics":{"Value":{"Fn::Join":[",",{"Ref":"AWS::NotificationARNs"}]}}}}).
3. Poll DescribeChangeSet to CREATE_COMPLETE, then ExecuteChangeSet and poll DescribeStacks to CREATE_COMPLETE.
4. Read Stacks[0].Outputs — against AWS each entry carries OutputKey and OutputValue ("Topics" -> the topic ARN); against the emulator the entry has no OutputValue member. Re-replay the check id in trace_ids to confirm the fix.
