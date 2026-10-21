Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the SNS emulator

_Filed: 2026-10-21T23:18:13Z_
_Operation: service-com-amazonaws-sns-amazonsimplenotificationservice | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 42 SNS operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no SNS client works at all. Implement a FIRST, SMALL stateful slice: the topic + subscription + publish happy path.

Happy path (in order, awsQuery protocol, sdkId `SNS`):

1. **CreateTopic** — in: `Name` (required), optional `Attributes` (map<string,string>), `Tags` (list<Tag>), `DataProtectionPolicy`. Out: `TopicArn`. Must mint and PERSIST a topic record under a deterministic ARN `arn:aws:sns:<region>:<account>:<Name>`; calling it twice with the same `Name` (and no conflicting attributes) must be idempotent and return the SAME ARN rather than creating a duplicate.
2. **Subscribe** — in: `TopicArn` (required), `Protocol` (required), `Endpoint`, `Attributes`, `ReturnSubscriptionArn` (default false). Out: `SubscriptionArn`. Must look the topic up in state and raise `NotFoundException` (HTTP 404) for an unknown ARN; must store a subscription record keyed by a generated subscription ARN `arn:aws:sns:<region>:<account>:<topic-name>:<uuid>`.
3. **Publish** — in: `Message` (required), `TopicArn` / `TargetArn` (one of), `Subject`, `MessageStructure`, `MessageAttributes`, `MessageDeduplicationId`, `MessageGroupId`. Out: `MessageId` (and `SequenceNumber` for FIFO topics). Must resolve the topic from state (`NotFoundException` 404 when absent), generate a fresh unique `MessageId` per call, and record the delivery against each matching subscription so a test can observe that the published message reached the subscribers. Reject a request carrying neither `TopicArn` nor `TargetArn` nor `PhoneNumber` with `InvalidParameterException` (400).
4. **ListSubscriptionsByTopic** — in: `TopicArn` (required), `NextToken`. Out: `Subscriptions` (list<Subscription>: `SubscriptionArn`, `Owner`, `Protocol`, `Endpoint`, `TopicArn`), `NextToken`. Must enumerate the subscriptions actually created in step 2 for that topic, and 404 (`NotFoundException`) for an unknown topic.
5. **ListTopics** — in: `NextToken`. Out: `Topics` (list<Topic> with `TopicArn`), `NextToken`. Must list exactly the topics in state (paginated per the `@paginated(inputToken=NextToken, outputToken=NextToken, items=Topics)` trait).
6. **GetTopicAttributes** — in: `TopicArn` (required). Out: `Attributes`. Must return the stored attributes plus the computed server-side ones (`TopicArn`, `Owner`, `DisplayName`, `SubscriptionsConfirmed`, `SubscriptionsPending`, `SubscriptionsDeleted`, `Policy`, `EffectiveDeliveryPolicy`), with the subscription counts derived from the subscription store — not hardcoded.
7. **Unsubscribe** — in: `SubscriptionArn` (required). Out: no members. Must remove the subscription so a following ListSubscriptionsByTopic no longer shows it; `NotFoundException` (404) for an unknown ARN.
8. **DeleteTopic** — in: `TopicArn` (required). Out: no members. Must remove the topic and its subscriptions; subsequent GetTopicAttributes/Subscribe on that ARN must fail with `NotFoundException` (404). Deleting an already-absent topic succeeds (SNS DeleteTopic is idempotent).

NEVER STUBS: each of these must compute its response from the persisted store reached through `crate::state::service_state(&*self.engine)` (see README "State"). No canned bodies, no echoing the input back, no minting an ARN/MessageId without storing anything. A create followed by a read must return what was created, in the same process and across calls.

## State Model

In `ComAmazonawsSnsAmazonsimplenotificationserviceState`:

- `topics: HashMap<String /* TopicArn */, Topic>` where
  `Topic { arn: String, name: String, owner: String /* account id */, display_name: Option<String>, attributes: HashMap<String, String>, tags: HashMap<String, String>, data_protection_policy: Option<String>, policy: Option<String>, is_fifo: bool, created_at: SystemTime }`
- `subscriptions: HashMap<String /* SubscriptionArn */, Subscription>` where
  `Subscription { arn: String, topic_arn: String, protocol: String, endpoint: Option<String>, owner: String, attributes: HashMap<String, String> /* FilterPolicy, RawMessageDelivery, ... */, pending_confirmation: bool, created_at: SystemTime }`
- `topic_subscriptions: HashMap<String /* TopicArn */, Vec<String /* SubscriptionArn */>>` (or derive by scanning `subscriptions`) to back ListSubscriptionsByTopic and the SubscriptionsConfirmed/Pending counts.
- `delivered: HashMap<String /* SubscriptionArn */, Vec<PublishedMessage>>` where
  `PublishedMessage { message_id: String, topic_arn: String, message: String, subject: Option<String>, message_structure: Option<String>, message_attributes: HashMap<String, MessageAttributeValue>, sequence_number: Option<String>, published_at: SystemTime }` — this is what makes Publish observable end-to-end instead of a fire-and-forget MessageId.
- `next_sequence_number: u64` for FIFO topic `SequenceNumber` generation.

Everything else (platform applications/endpoints, SMS sandbox, phone-number opt-out, PublishBatch, Add/RemovePermission, Tag/Untag/ListTagsForResource, data-protection policies, Set*Attributes, ConfirmSubscription, ListSubscriptions) stays 501 for now and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-sns-amazonsimplenotificationservice/OPERATIONS.md` lists all 42 bound operations with the exact input/output members and errors cited above: CreateTopic (line 62: Name required -> TopicArn), DeleteTopic (line 86), GetTopicAttributes (line 128), ListTopics (line 189, @paginated items=Topics), ListSubscriptionsByTopic (line 176, @paginated items=Subscriptions, NotFoundException 404), Publish (line 202: Message required, optional TopicArn/TargetArn/PhoneNumber -> MessageId/SequenceNumber, NotFoundException 404, InvalidParameterException 400), Subscribe (line 256: TopicArn+Protocol required -> SubscriptionArn, NotFoundException 404), Unsubscribe (line 268). Wire protocol is `awsQuery` (OPERATIONS.md line 10). A grep for `SCAFFOLDED, NOT IMPLEMENTED` across `src/ops/` matches all 42 handler files, i.e. the whole service answers HTTP 501 and persists nothing; the crate's only test is the generated `tests/state_roundtrip.rs` example, so no behavior is asserted anywhere today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI SNS client at it (`aws --endpoint-url <emulator> sns ...`).
2. `CreateTopic Name=bootstrap-topic` -> expect HTTP 200 with `TopicArn=arn:aws:sns:<region>:<account>:bootstrap-topic`; today it returns HTTP 501 NotImplemented.
3. `CreateTopic Name=bootstrap-topic` again -> expect the same TopicArn (idempotent), not a second topic.
4. `ListTopics` -> expect the created ARN present in `Topics`.
5. `Subscribe TopicArn=<arn> Protocol=sqs Endpoint=arn:aws:sqs:...:q ReturnSubscriptionArn=true` -> expect a `SubscriptionArn` under the topic.
6. `ListSubscriptionsByTopic TopicArn=<arn>` -> expect exactly that one subscription with the Protocol/Endpoint/TopicArn given at Subscribe time.
7. `GetTopicAttributes TopicArn=<arn>` -> expect `TopicArn`, `Owner`, and `SubscriptionsConfirmed=1` derived from state.
8. `Publish TopicArn=<arn> Message=hello Subject=hi` -> expect HTTP 200 with a non-empty, unique `MessageId`, and the message recorded as delivered to the subscription.
9. `Publish TopicArn=arn:aws:sns:<region>:<account>:does-not-exist Message=x` -> expect `NotFoundException` with HTTP 404.
10. `Unsubscribe SubscriptionArn=<sub-arn>` then `ListSubscriptionsByTopic TopicArn=<arn>` -> expect an empty `Subscriptions` list.
11. `DeleteTopic TopicArn=<arn>` then `GetTopicAttributes TopicArn=<arn>` -> expect `NotFoundException` HTTP 404, and `ListTopics` no longer showing the ARN.
