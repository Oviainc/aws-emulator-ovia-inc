Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codestar-notifications.create-rule-nonexistent-event-type-rejected
Issue-Shape-Id: com.amazonaws.codestarnotifications#CodeStarNotifications_20191015
Issue-Key: codestar-notifications.create-rule-nonexistent-event-type-rejected|_error#5|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# codestar-notifications: CreateNotificationRule accepts an EventTypeIds entry that is not in the service event-type catalog (no 400 ValidationException, rule is created)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNotificationRule

## Description
This bundle has 1 cause; it must be closed before submitting.

Related open issue: 20261029T183307Z-codestar-notifications-four-behavioral-divergences-createnotificationrule-accepts-unknown.md describes a similar CreateNotificationRule event-type divergence as its Cause 1, but the suite did NOT mark this check as OWNED by it (the check's replay key has drifted: the failing frame is now the `_error` helper, not `_error_code`, and the check now additionally asserts the exact Error.Message text, which that issue explicitly declared out of scope with "The check asserts the error CODE only"). It is therefore filed here as a net-new work item, and the message template below is a requirement that the related issue does not state.

Cause 1 of 1: CreateNotificationRule does not validate EventTypeIds against the service's event-type catalog, so an unknown event-type id is accepted and a rule is created (failing check: codestar-notifications.create-rule-nonexistent-event-type-rejected).

AWS: Smithy (services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/smithy-model/com-amazonaws-codestarnotifications-codestarnotifications-20191015.json): CreateNotificationRule, POST /createNotificationRule, success code 200; input CreateNotificationRuleRequest.EventTypeIds targets the list shape EventTypeIds and is required; Resource targets NotificationRuleResource (pattern ^arn:aws[^:\s]*:[^:\s]*:[^:\s]*:[0-9]{12}:[^\s]+$, required); modeled errors are AccessDeniedException(403), ConcurrentModificationException(400), ConfigurationException(400), LimitExceededException(400), ResourceAlreadyExistsException(409) and the service-common ValidationException(400, member Message). The model constrains only the list/member shape, so the catalog check itself is recorded behavior only: AWS resolves every EventTypeIds entry against the same event-type catalog ListEventTypes (POST /listEventTypes) returns, and refuses an id that does not resolve with HTTP 400 Error.Code 'ValidationException' before any rule is persisted. Error.Message is a template — fixed text 'The notification rule cannot be created or modified because the following notification event type ID is not valid: ' + the offending event-type id exactly as submitted + fixed text '. For more information, see the AWS CodeStar Notifications API Reference.' The only variable part is the submitted id; the sim2real-* id in the recording fills the slot and is not part of the requirement. A known id on the same Resource/Targets still succeeds (positive control), so the refusal is specific to catalog resolution, not to the resource or target.

Emulator: returns HTTP 200 with a freshly minted rule ARN (shape arn:aws:codestar-notifications:<region>:<account>:notificationrule/<uuid>) for EventTypeIds=['sim2real-behav-nonexistent-event-type-id'], i.e. the unknown event type is accepted and the rule is persisted. No ValidationException is raised and none of the fixed message text is ever produced; the subsequent ListNotificationRules(Filters=[{Name:'RESOURCE',Value:<resource arn>}]) consequently returns the bogus rule instead of an empty list.

Why: the catalog check is the only point at which a client learns that an event-type id is mistyped, deprecated or belongs to a different Developer Tools service. A client that gets 200 plus an ARN believes it owns a wired-up notification rule that in fact can never fire, and the specific ValidationException message that names the offending id — the text operators grep for and that points at the API Reference — never reaches it. Reconciliation and infrastructure-as-code flows then report converged state for a rule that silently delivers nothing.

Fix: CreateNotificationRule's input validation in its operation handler for the codestar-notifications service — resolve each EventTypeIds entry against the seeded event-type catalog in the service's state layer and refuse the first unresolvable id with ValidationException (HTTP 400), rendering the message template above, before any rule is inserted into state; the rule must not be created on the refusal path. UpdateNotificationRule's EventTypeIds path needs the identical check so a rule cannot be re-pointed at an unknown event type after creation.

Traces: codestar-notifications.create-rule-nonexistent-event-type-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 16/23 passed (69.6%), 4 mismatched, 3 errored, 0 skipped, bootstrap_signal false. Service codestar-notifications 16/23 (70%), 7 failed. Of those, 3 errored checks are already filed against the unimplemented ListTagsForResource scaffold stub, and 3 mismatches are marked OWNED by 20261029T183307Z-codestar-notifications-four-behavioral-divergences-createnotificationrule-accepts-unknown.md; this is the one remaining, unowned failure.

Cause 1 — codestar-notifications.create-rule-nonexistent-event-type-rejected [mismatch]:
  setup: CreateApplication(computePlatform='Server') in codedeploy -> application ARN; CreateTopic in sns with a codestar-notifications publish policy -> target ARN.
  call: CreateNotificationRule(Name=<unique>, EventTypeIds=['sim2real-behav-nonexistent-event-type-id'], Resource=<CodeDeploy application ARN>, Targets=[{TargetType:'SNS', TargetAddress:<topic ARN>}], DetailType='BASIC')
  expected: refusal — HTTP 400, Error.Code == 'ValidationException', Error.Message == 'The notification rule cannot be created or modified because the following notification event type ID is not valid: sim2real-behav-nonexistent-event-type-id. For more information, see the AWS CodeStar Notifications API Reference.' (fixed text either side of the submitted event-type id), and effect absent: ListNotificationRules(Filters=[{Name:'RESOURCE',Value:<application ARN>}]) returns NotificationRules == [].
  actual: HTTP 200, content-type application/json, body {'Arn': 'arn:aws:codestar-notifications:us-west-2:000000000000:notificationrule/9a43794d-d207-4f71-81ac-ada122d317a5'}.
  harness: AssertionError: expected rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Arn': ...} (raised in the check's _error helper at line 108).
  The account id 000000000000, region us-west-2, the generated rule uuid, the date header and the sim2real-* names are environment-specific; the defect is that the call is accepted at all and that the ValidationException template is never rendered.

Model grounding (smithy_query, query='operation', operation='CreateNotificationRule,ListEventTypes'): CreateNotificationRule POST /createNotificationRule code 200, EventTypeIds required (list), Resource required (NotificationRuleResource pattern as above), Targets required 0..10, DetailType required enum, output Arn targets NotificationRuleArn (pattern ^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$); errors include ValidationException httpError 400. ListEventTypes POST /listEventTypes returns EventTypes (EventTypeBatch) — the catalog the submitted ids must resolve against. The model does not itself express the catalog membership rule, hence 'recorded behavior only' above.

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from it.
2. Run the behavioral suite simparity-behavioral-aws-v1 (run_behavioral_suite); re-replay codestar-notifications.create-rule-nonexistent-event-type-rejected to confirm a fix.
3. Reproduce directly: in codedeploy, CreateApplication(applicationName=<unique>, computePlatform='Server') and TagResource on the resulting ARN arn:aws:codedeploy:<region>:<account>:application:<name>; in sns, CreateTopic(Name=<unique>) and SetTopicAttributes(AttributeName='Policy') granting SNS:Publish to codestar-notifications.amazonaws.com.
4. POST /createNotificationRule with Name=<unique>, EventTypeIds=['sim2real-behav-nonexistent-event-type-id'] (any id not returned by ListEventTypes), Resource=<the CodeDeploy application ARN>, Targets=[{TargetType:'SNS', TargetAddress:<topic ARN>}], DetailType='BASIC'.
5. Assert the call is REFUSED with HTTP 400 and Error.Code 'ValidationException', and that Error.Message equals the fixed text 'The notification rule cannot be created or modified because the following notification event type ID is not valid: ' + the submitted id + '. For more information, see the AWS CodeStar Notifications API Reference.' Today the call returns HTTP 200 with a rule Arn.
6. Assert the effect is absent: ListNotificationRules(Filters=[{Name:'RESOURCE',Value:<application ARN>}]) returns an empty NotificationRules list after the refusal.
7. Positive control: repeat step 4 with EventTypeIds=['codedeploy-application-deployment-succeeded'] (a catalog id) and assert it still returns 200 with an Arn and that DescribeNotificationRule(Arn=<that arn>) echoes the submitted Name.
8. Also apply the same assertions to UpdateNotificationRule(EventTypeIds=[<unknown id>]) on an existing rule.
9. Assert only on status + error code + the message's fixed text and its submitted-id slot — never on the account id, region, generated rule uuid or timestamps.
