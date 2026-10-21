Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the codestar-notifications emulator

_Filed: 2026-10-21T21:38:14Z_
_Operation: service-com-amazonaws-codestarnotifications-codestarnotifications-20191015 | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), holding zero state. Implement a FIRST, SMALL stateful slice: the notification-rule lifecycle plus target subscription. Protocol is `restJson1`, sigv4 signing name `codestar-notifications`.

### Happy path to implement (in order)

1. **CreateNotificationRule** — `POST /createNotificationRule`. Input: `Name` (required, 1..64, `^[A-Za-z0-9\-_ ]+$`), `EventTypeIds` (required list<string>), `Resource` (required resource ARN), `Targets` (required list<Target{TargetType,TargetAddress}>, 0..10), `DetailType` (required, BASIC|FULL), optional `ClientRequestToken` (@idempotencyToken), `Tags`, `Status` (ENABLED default when absent). Must mint and persist a real rule: generate a `NotificationRuleId` (1..40) and an `Arn` matching `^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$` from the request region/account, store the rule, and return that `Arn`. A second create with the same `Name` must fail `ResourceAlreadyExistsException` (HTTP 409) — rule names are unique per account. A repeat with the same `ClientRequestToken` and identical parameters must return the original `Arn` rather than creating a second rule.
2. **DescribeNotificationRule** — `POST /describeNotificationRule`. Input: `Arn` (required). Must return the PERSISTED rule, not an echo: `Arn` (required), `Name`, `Resource`, `DetailType`, `Status`, `Tags`, `CreatedBy`, `CreatedTimestamp`, `LastModifiedTimestamp`, `EventTypes` (list<EventTypeSummary{EventTypeId,ServiceName,EventTypeName,ResourceType}> resolved from the stored `EventTypeIds`), and `Targets` (list<TargetSummary{TargetAddress,TargetType,TargetStatus}>). Unknown ARN ⇒ `ResourceNotFoundException` (HTTP 404).
3. **ListNotificationRules** — `POST /listNotificationRules`. Output `NotificationRules` = list<NotificationRuleSummary{Id,Arn}> built from the store (never a constant/empty list). Honor `MaxResults` (1..100) by truncating and returning an opaque `NextToken` (`^[\w/+=]+$`) that resumes exactly where the page ended; a token that does not decode ⇒ `InvalidNextTokenException` (HTTP 400).
4. **Subscribe** — `POST /subscribe`. Input: `Arn` (required), `Target` (required), optional `ClientRequestToken`. Must append/upsert the target on the stored rule (dedup by `TargetAddress`) so a subsequent DescribeNotificationRule and ListTargets both show it; return the rule `Arn`. Unknown rule ⇒ `ResourceNotFoundException` (404). Target count above the modeled 10 ⇒ `ConfigurationException`/`ValidationException` (400).
5. **UpdateNotificationRule** — `POST /updateNotificationRule`. Input: `Arn` (required) plus optional `Name`, `Status`, `EventTypeIds`, `Targets`, `DetailType`. Must mutate only the supplied fields on the stored rule, bump `LastModifiedTimestamp`, and return an empty body; the change must be visible to the next DescribeNotificationRule. Unknown ARN ⇒ `ResourceNotFoundException` (404).
6. **DeleteNotificationRule** — `POST /deleteNotificationRule`. Input: `Arn` (required). Must remove the rule from the store and echo the deleted `Arn`; afterwards DescribeNotificationRule ⇒ 404 and the rule is absent from ListNotificationRules.

NEVER STUBS: each of these six must compute its response from persisted state. Hardcoded or echoed responses, and minting an ARN without storing the rule, are failures. The remaining operations (`ListEventTypes`, `ListTargets` as a standalone listing, `TagResource`, `UntagResource`, `ListTagsForResource`, `Unsubscribe`, `DeleteTarget`) stay 501 for now and will be filed as individual gap issues later — though storing tags at create time and keeping a target registry (below) makes them cheap follow-ups.

## State Model

Service state lives behind `crate::state::service_state(&*self.engine)` and its lock. Concrete structures:

- `rules: HashMap<String /* rule ARN */, NotificationRule>` — the primary store.
- `rule_names: HashMap<String /* Name */, String /* ARN */>` — uniqueness index backing `ResourceAlreadyExistsException`.
- `idempotency: HashMap<String /* ClientRequestToken */, String /* ARN */>` — for CreateNotificationRule / Subscribe token replay.
- `NotificationRule { id: String, arn: String, name: String, resource: String, detail_type: DetailType, status: NotificationRuleStatus, event_type_ids: Vec<String>, targets: Vec<RuleTarget>, tags: BTreeMap<String,String>, created_by: String, created_timestamp: DateTime, last_modified_timestamp: DateTime }`.
- `RuleTarget { target_address: String, target_type: String, target_status: TargetStatus /* ACTIVE after subscribe */ }`.
- `targets: HashMap<String /* TargetAddress */, TargetRegistryEntry { target_type: String, status: TargetStatus, subscribed_rules: HashSet<String /* rule ARN */> }>` — the account-wide target registry that ListTargets/Unsubscribe/DeleteTarget will read later; Subscribe and CreateNotificationRule must keep it in sync.
- `event_types: Vec<EventTypeSummary>` — a static seeded catalog (CodeCommit/CodeBuild/CodePipeline event type ids) used to resolve `EventTypeIds` into `EventTypes` in DescribeNotificationRule, and later to serve ListEventTypes.
- Pagination tokens are opaque, encode the position in a stable ordering of `rules`, and are validated on the way back in.

Done = a client script can create a rule, describe it and see exactly the name/resource/detail type/status/event types/targets/tags it created, list it, subscribe an extra SNS target and see it appear, update the status to DISABLED and observe it, then delete the rule and get a 404 from describe — all without any 501 and with no canned values.

## Evidence

OPERATIONS.md for this crate (13 operations, `restJson1`, endpoint prefix `codestar-notifications`) documents each operation's route, input/output members and errors: CreateNotificationRule (`POST /createNotificationRule`, required Name/EventTypeIds/Resource/Targets/DetailType, output `Arn`, errors incl. ResourceAlreadyExistsException 409), DescribeNotificationRule (required output `Arn` plus Name/EventTypes/Resource/Targets/DetailType/CreatedBy/Status/CreatedTimestamp/LastModifiedTimestamp/Tags, ResourceNotFoundException 404), ListNotificationRules (@paginated inputToken=NextToken outputToken=NextToken items=NotificationRules, MaxResults 1..100, InvalidNextTokenException 400), Subscribe (required Arn + Target, ResourceNotFoundException 404/ConfigurationException 400), UpdateNotificationRule (required Arn, optional Name/Status/EventTypeIds/Targets/DetailType, no output members, ResourceNotFoundException 404), DeleteNotificationRule (required Arn, output Arn). Shape details for Target{TargetType,TargetAddress}, TargetSummary{TargetAddress,TargetType,TargetStatus}, EventTypeSummary{EventTypeId,ServiceName,EventTypeName,ResourceType} and NotificationRuleSummary{Id,Arn} come from smithy_query on the model. Stub proof: src/ops/create_notification_rule.rs lines 4 and 201 — `//! SCAFFOLDED, NOT IMPLEMENTED:` header and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateNotificationRule"))`; all 13 op files in src/ops/ are the same generated scaffold. No tests assert any behavior.

## Steps to Reproduce

1. Point an AWS SDK/CLI client for codestar-notifications at the emulator endpoint.
2. create-notification-rule --name demo-rule --resource arn:aws:codecommit:us-east-1:123456789012:MyRepo --event-type-ids codecommit-repository-comments-on-commits --targets TargetType=SNS,TargetAddress=arn:aws:sns:us-east-1:123456789012:demo-topic --detail-type FULL --tags Team=dev  => expect 200 with a notificationrule ARN (currently HTTP 501 NotImplemented).
3. describe-notification-rule --arn <ARN from step 2>  => expect the created Name, Resource, DetailType=FULL, Status=ENABLED, Tags{Team=dev}, EventTypes resolved from the submitted id, and Targets containing the SNS address.
4. list-notification-rules  => expect the rule's Id and Arn in NotificationRules; repeat with --max-results 1 and follow NextToken to page.
5. Repeat step 2 with the same --name  => expect ResourceAlreadyExistsException (HTTP 409).
6. subscribe --arn <ARN> --target TargetType=SNS,TargetAddress=arn:aws:sns:us-east-1:123456789012:second-topic  => expect 200 and the new target visible in a follow-up describe-notification-rule.
7. update-notification-rule --arn <ARN> --status DISABLED  => expect 200, then describe-notification-rule shows Status=DISABLED and a bumped LastModifiedTimestamp.
8. delete-notification-rule --arn <ARN>  => expect 200 echoing the ARN; then describe-notification-rule --arn <ARN> => ResourceNotFoundException (HTTP 404) and the rule absent from list-notification-rules.
