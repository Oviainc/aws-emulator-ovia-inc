Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__sns.register-event-topic-binds-an-existing-topic-and-never-records-a-missing-one
Issue-Key: ds__sns.register-event-topic-binds-an-existing-topic-and-never-records-a-missing-one|run#31|AssertionError|assert recorded.get("Status") != "Registered", (
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ds__sns: RegisterEventTopic reports Status 'Registered' for a topic SNS does not hold, so a missing notification channel is never surfaced as 'Topic not found'

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterEventTopic

## Description
Cause 1 of 1: RegisterEventTopic binds a topic name without consulting SNS, and DescribeEventTopics then reports the binding as Status='Registered' even when SNS holds no such topic — the modelled 'Topic not found' status is unreachable.

AWS: Smithy RegisterEventTopic takes DirectoryId (@required, ^d-[0-9a-f]{10}$) and TopicName (@required, @length 1..256, @pattern ^[a-zA-Z0-9_-]+$) and models ClientException, EntityDoesNotExistException and InvalidParameterException (client, HTTP 400); EventTopic.Status targets the TopicStatus enum whose values are exactly 'Registered', 'Topic not found', 'Failed' and 'Deleted'. Recorded behavior: when the directory's registered topic does not (or no longer) exist in SNS, DescribeEventTopics reports that topic's Status as 'Topic not found' — the enum member that exists for precisely this case — and only a topic SNS actually holds is reported 'Registered'.
Emulator: DescribeEventTopics reports the record for a topic SNS does not hold as {TopicName: <name>, TopicArn: arn:aws:sns:<region>:<account>:<name>, CreatedDateTime: <ts>, Status: 'Registered'} — it synthesises a plausible ARN and claims the binding is live.
Why: this is silent misconfiguration of the directory's ONLY notification channel: an operator who deletes or mistypes the topic is told by the emulator that directory event notifications are wired up, so no event ever reaches them and nothing reports the break. A client that polls Status to verify its channel can never observe the failure state the model defines.
Fix: the RegisterEventTopic handler and the DescribeEventTopics projection — resolve the topic against SNS (at register time and on read) and report Status from the TopicStatus enum accordingly, using 'Topic not found' when SNS holds no such topic.
Traces: ds__sns.register-event-topic-binds-an-existing-topic-and-never-records-a-missing-one.

## Evidence
Suite simparity-behavioral-aws-v1 against this checkout: Total 56/178 passed (31.5%). Service ds__sns: 0/1 (0%), 1 failed.

Failing check: [mismatch] ds__sns.register-event-topic-binds-an-existing-topic-and-never-records-a-missing-one
  assert recorded.get('Status') != 'Registered'
  AssertionError: expected a topic SNS does not hold NEVER to be reported as 'Registered' — that is silent misconfiguration of the directory's only notification channel — got {'DirectoryId': 'd-003ba5fe5a', 'TopicName': 'sim2real-ds-evt-gone-<run>', 'TopicArn': 'arn:aws:sns:us-west-2:000000000000:sim2real-ds-evt-gone-<run>', 'CreatedDateTime': <timestamp>, 'Status': 'Registered'}. The modelled enum has 'Topic not found' for exactly this case.

The check records the enum verbatim from the model as the only legal Status values: ('Registered', 'Topic not found', 'Failed', 'Deleted'), with 'Registered' being the only one that means the binding took. (The DirectoryId, topic name, account id and timestamp above are per-run values, not requirements; the requirement is the Status value for a topic SNS does not hold.)

Model evidence (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): RegisterEventTopic input DirectoryId @required @pattern ^d-[0-9a-f]{10}$, TopicName @required @length 1..256 @pattern ^[a-zA-Z0-9_-]+$; errors ClientException, EntityDoesNotExistException, InvalidParameterException (400) + ServiceException (500); TopicStatus enum values = ['Registered', 'Topic not found', 'Failed', 'Deleted'].

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite() to build, boot and replay against the emulator.
2. Create a Managed Microsoft AD and wait for Stage=Active.
3. Create an SNS topic, call ds RegisterEventTopic(DirectoryId=<dir>, TopicName=<that topic>), and confirm DescribeEventTopics reports Status='Registered' (positive control).
4. Delete that SNS topic (or register a TopicName SNS never held) and call DescribeEventTopics(DirectoryId=<dir>) again. Expect the record's Status to be 'Topic not found'; observe 'Registered' with a synthesised TopicArn.
5. Re-replay ds__sns.register-event-topic-binds-an-existing-topic-and-never-records-a-missing-one.
