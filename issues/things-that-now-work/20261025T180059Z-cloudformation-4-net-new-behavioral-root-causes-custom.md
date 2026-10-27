Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.execute-changeset-rejects-non-executable-status cloudformation.execute-failed-changeset-rejected cloudformation.deep-cloudformation-3-cfn-flight-update-while-wait-condition-barriered-rejected-then-succeeds
Issue-Key: cloudformation.execute-changeset-rejects-non-executable-status|_wait_stack#11|Timeout|time.sleep(3)
Issue-Key: cloudformation.execute-failed-changeset-rejected|_wait_stack_status#11|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation.deep-cloudformation-3-cfn-flight-update-while-wait-condition-barriered-rejected-then-succeeds|_wait_resource_status#8|Timeout|time.sleep(2)
Issue-Commit: 8fdd90c15790d38aa3c5d0088f99b03c79823121

# cloudformation: the change-set execute lifecycle never reached a terminal status, and a WaitCondition-barriered update was unobservable — FIXED

> _This report was filed as a bundle of four root causes. The two below are fixed; the other
> two (custom-resource providers are never invoked, and `SetTypeConfiguration` for a
> just-activated Hook) are still open and carry their own report,
> `issues/things-that-do-not-work/20261025T180059Z-cloudformation-custom-resource-providers-and-hook-configuration.md`,
> with the seven checks that belong to them._

## Operation
ExecuteChangeSet, UpdateStack, DescribeStackResource

## Description

=== Cause: the change-set create/execute lifecycle never reached the terminal status a client polls for ===
AWS: `ExecuteChangeSet` on an executable set drives the stack `UPDATE_IN_PROGRESS` -> `UPDATE_COMPLETE`; a no-op revision's change set settles `FAILED` with `StatusReason` "The submitted information didn't contain changes. Submit different information to create a change set."; executing that one is refused HTTP 400 `InvalidChangeSetStatus` with "ChangeSet [<arn>] cannot be executed in its current status of [FAILED]", leaving the stack where it was.
Emulator (before the fix): the change-set settle path and the refusal were already right; what never arrived was the stack's terminal status. Executing a revision that re-declared the stack's `AWS::SNS::Topic` with a changed property left the stack `UPDATE_FAILED` — "The following resource(s) failed to update: [Topic]." with the resource event "NotImplemented: this emulator does not change the properties of an existing AWS::SNS::Topic in place" — because `state::provisioner`'s `Provisioner::update` had no arm for that type.
Fixed by: `update_topic` in `src/state/provisioner.rs`, which applies a changed declaration through Amazon SNS's own `SetTopicAttributes` (an attribute the revision stops declaring going back to its documented absent value), `PutDataProtectionPolicy` and `TagResource`/`UntagResource`, and refuses — rather than silently dropping — a changed inline `Subscription` list, whose entries are subscriptions of SNS's own. Covered by `tests/all/in_place_resource_updates.rs` and `core/router/tests/all/cloudformation_provisions_sns_topics.rs`.

=== Cause: an update held open by an unsignalled WaitCondition was not observable ===
AWS: while an `UpdateStack` is in flight the stack reports `UPDATE_IN_PROGRESS`, a gate resource the update ADDS reports `ResourceStatus` `CREATE_IN_PROGRESS` through `DescribeStackResource`, a second `UpdateStack` is refused HTTP 400 `ValidationError` "Stack:<arn> is in UPDATE_IN_PROGRESS state and can not be updated." with no effect, and after `SignalResource(Status=SUCCESS)` the stack reaches `UPDATE_COMPLETE` and the gate `CREATE_COMPLETE`.
Emulator (before the fix): the gate was reported `UPDATE_IN_PROGRESS`, because `StoredStack::reported_resources` derived an in-flight resource's status from the STACK's status.
Fixed by: `StoredStack::in_progress_status_of` in `src/state.rs`, which decides per resource — a resource the stack does not already hold is being created, whatever the operation around it is — the same rule `installed_resources` applies to a completed operation. Covered by `tests/all/in_flight_resource_status.rs`.

## Evidence
All three checks above pass on replay; the recorded `InvalidChangeSetStatus` refusal, the no-op `StatusReason`, the `ValidationError` wording and the per-resource statuses are asserted by the suites named above.
