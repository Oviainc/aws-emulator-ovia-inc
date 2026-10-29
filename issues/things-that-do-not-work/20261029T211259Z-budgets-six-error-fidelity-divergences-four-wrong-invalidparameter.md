Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: budgets.create-budget-invalid-time-period-rejected budgets.create-budget-limit-and-planned-conflict-rejected budgets.create-notification-quota-exceeded-rejected budgets.delete-budget-removes-resource budgets.delete-budget-cascades-notifications-and-subscribers budgets.delete-budget-action-removes-resource budgets.execute-budget-action-wrong-state-rejected
Issue-Shape-Id: com.amazonaws.budgets#AWSBudgetServiceGateway
Issue-Key: budgets.create-budget-invalid-time-period-rejected|run#7|AssertionError|assert error.get("Message") == expected_message, f"expected {expected_message!r}, got {error.get('Message')!r}"
Issue-Key: budgets.create-budget-limit-and-planned-conflict-rejected|run#8|AssertionError|assert error.get("Message") == expected_message, f"expected {expected_message!r}, got {error.get('Message')!r}"
Issue-Key: budgets.create-notification-quota-exceeded-rejected|run#11|AssertionError|assert error.get("Message") == expected_message, f"expected {expected_message!r}, got {error.get('Message')!r}"
Issue-Key: budgets.delete-budget-removes-resource|run#12|AssertionError|assert update_error.get("Message") == gone_message, (
Issue-Key: budgets.delete-budget-cascades-notifications-and-subscribers|run#15|AssertionError|assert error.get("Message") == gone_message, f"expected {gone_message!r}, got {error.get('Message')!r}"
Issue-Key: budgets.delete-budget-action-removes-resource|run#13|AssertionError|assert error.get("Message") == expected_message, f"expected {expected_message!r}, got {error.get('Message')!r}"
Issue-Key: budgets.execute-budget-action-wrong-state-rejected|run#11|AssertionError|assert error["Code"] == "ResourceLockedException", f"expected ResourceLockedException, got {error['Code']!r}"
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# budgets: six error-fidelity divergences — four wrong InvalidParameter/NotFound/CreationLimit messages, per-operation (not uniform) budget-not-found text, and ExecuteBudgetAction on a STANDBY action misclassified as InvalidParameterException instead of ResourceLockedException

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBudget

## Description
Bundle of 6 net-new root causes in the budgets emulator's error construction and error classification. All are grounded in failing behavioral checks; each cause below lists its own check ids. NOTE: the open issue 20261028T164845Z-budgets-createbudget-validation--r3.md concerns CreateBudget accepting a Start-after-End TimePeriod when End is OMITTED; it does not cover any cause here (Cause 1 is the message text emitted when BOTH dates are supplied), so these remain net-new.

Cause 1 of 6: CreateBudget TimePeriod Start-after-End rejection message.
AWS: Smithy: CreateBudget errors include InvalidParameterException (httpError 400, member Message). Recorded behavior: AWS rejects a Budget whose TimePeriod.End precedes TimePeriod.Start with code InvalidParameterException and the fixed message text 'Unable to create/update budget - end time has to be after start time.' (no variable parts — the whole string is fixed text).
Emulator: returns InvalidParameterException with 'Unable to create/update budget: TimePeriod Start must come before End.' — a locally invented template.
Why: clients and test suites that match on the documented message (log scrapers, error-mapping layers, snapshot tests) do not recognize the refusal, and operators debugging a rejected budget see wording that exists nowhere in AWS.
Fix: the CreateBudget/UpdateBudget input validation that builds the InvalidParameterException message (budget TimePeriod validation).
Traces: budgets.create-budget-invalid-time-period-rejected.

Cause 2 of 6: CreateBudget BudgetLimit + PlannedBudgetLimits conflict message.
AWS: Smithy: CreateBudget -> InvalidParameterException (400). Recorded behavior: a Budget that sets both BudgetLimit and PlannedBudgetLimits is refused with InvalidParameterException and the fixed message 'Unable to create/update budget - please provide only one of the followings: Budget Limit/ Planned Budget Limit/ Auto Adjust Data' (entirely fixed text; AWS names all three mutually exclusive members, not just the two supplied).
Emulator: InvalidParameterException with "Unable to create/update budget: BudgetLimit can't be used with PlannedBudgetLimits."
Why: same as Cause 1 — the refusal is correct in code but unmatchable in text, and it omits AutoAdjustData from the exclusivity set so a client cannot learn the real rule from the error.
Fix: the CreateBudget/UpdateBudget validation that enforces the BudgetLimit / PlannedBudgetLimits / AutoAdjustData exclusivity.
Traces: budgets.create-budget-limit-and-planned-conflict-rejected.

Cause 3 of 6: CreateNotification per-budget notification quota message.
AWS: Smithy: CreateNotification -> CreationLimitExceededException (httpError 405), doc "You've exceeded the notification or subscriber limit." Recorded behavior: the 11th notification on one budget is refused with CreationLimitExceededException and the fixed message 'Unable to create notification - one budget can only have 10 notifications' — all fixed text, with NO budget name interpolated and no trailing period.
Emulator: CreationLimitExceededException with 'Unable to create notification: <BudgetName> - a budget can have at most 10 notifications.' — it interpolates the budget name (a request value AWS does not include here) and rewords the sentence.
Why: a client that keys on the documented quota message, or that parses the message for a resource name, gets a different shape of string than AWS ever sends.
Fix: the CreateNotification handler's quota-exceeded error construction.
Traces: budgets.create-notification-quota-exceeded-rejected.

Cause 4 of 6: NotFoundException for a missing budget is reworded per operation instead of AWS's single budget-not-found message.
AWS: Smithy: UpdateBudget and DescribeNotificationsForBudget both declare NotFoundException (httpError 404, doc "We can't locate the resource that you specified."). Recorded behavior: whichever operation is addressed, when the named budget does not exist AWS answers NotFoundException with the one message template 'Unable to get budget: <BudgetName> - the budget doesn't exist.' — fixed text 'Unable to get budget: ' and ' - the budget doesn't exist.', with <BudgetName> taken verbatim from the request. The verb is 'get' even for UpdateBudget and for the notification listing.
Emulator: builds an operation-specific message instead: UpdateBudget on a deleted budget answers 'Unable to update budget: <name> - the budget doesn't exist.' and DescribeNotificationsForBudget answers 'Unable to get notifications: <name> - the budget doesn't exist.'
Why: a client that recognizes the budget-not-found condition by message (there is only one NotFoundException code shared by every missing-resource case in this service, so the message is the only discriminator) fails to recognize it on every operation except DescribeBudget.
Fix: the shared budget-lookup/not-found error construction in the budgets state layer, which must emit one budget-not-found message for all callers rather than letting each handler word its own.
Traces: budgets.delete-budget-removes-resource, budgets.delete-budget-cascades-notifications-and-subscribers.

Cause 5 of 6: DescribeBudgetAction not-found message uses the budget-style template instead of the action-style one.
AWS: Smithy: DescribeBudgetAction -> NotFoundException (404). Recorded behavior: describing an action id that does not exist answers NotFoundException with 'Action for [AccountId: <AccountId>, ActionId: <ActionId>] is not found!' — fixed text 'Action for [AccountId: ', ', ActionId: ', '] is not found!', with the account id and action id taken from the request.
Emulator: answers NotFoundException with "Unable to get budget action: <ActionId> - the action doesn't exist." — the budget-shaped template, dropping AccountId entirely.
Why: the action-not-found condition is reported in a wording AWS never emits, so clients that distinguish a missing action from a missing budget by message text cannot.
Fix: the DescribeBudgetAction handler / budget-action lookup not-found error construction.
Traces: budgets.delete-budget-action-removes-resource.

Cause 6 of 6: ExecuteBudgetAction(REVERSE_BUDGET_ACTION) on a never-executed action is misclassified.
AWS: Smithy: ExecuteBudgetAction declares ResourceLockedException (errorKind client, httpError 423, doc "The request was received and recognized by the server, but the server rejected that particular method for the requested resource.") alongside InvalidParameterException (400). Recorded behavior: reversing an action that is in the Standby state is refused with code ResourceLockedException and the message 'This method is not allowed during [ActionStatus: Standby]' — fixed text 'This method is not allowed during [ActionStatus: ' + the action's current status as held in service state + ']'.
Emulator: refuses with code InvalidParameterException (HTTP 400) and a non-matching message, i.e. a wrong error class and a wrong HTTP status for a state-conflict refusal.
Why: ResourceLockedException is a retryable/state-conflict signal at 423 that SDK retry and error-mapping layers treat differently from a 400 InvalidParameterException (which clients treat as a permanent caller bug). Code that waits for an action to leave Standby and retries will instead abort.
Fix: the ExecuteBudgetAction handler's action-state gate — it must classify the wrong-state refusal as ResourceLockedException and render the status-specific message.
Traces: budgets.execute-budget-action-wrong-state-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, 14/25 passed, all 11 failures in budgets.

Cause 1 — budgets.create-budget-invalid-time-period-rejected [mismatch]:
  AssertionError: expected 'Unable to create/update budget - end time has to be after start time.', got 'Unable to create/update budget: TimePeriod Start must come before End.'
  (code InvalidParameterException matched; only the message diverges)

Cause 2 — budgets.create-budget-limit-and-planned-conflict-rejected [mismatch]:
  AssertionError: expected 'Unable to create/update budget - please provide only one of the followings: Budget Limit/ Planned Budget Limit/ Auto Adjust Data', got "Unable to create/update budget: BudgetLimit can't be used with PlannedBudgetLimits."

Cause 3 — budgets.create-notification-quota-exceeded-rejected [mismatch]:
  AssertionError: expected 'Unable to create notification - one budget can only have 10 notifications', got 'Unable to create notification: <budget-name> - a budget can have at most 10 notifications.' (the recorded run interpolated the generated budget name; the name is a per-run value, the template is the requirement)

Cause 4 — budgets.delete-budget-removes-resource [mismatch]:
  AssertionError: expected "Unable to get budget: <name> - the budget doesn't exist.", got "Unable to update budget: <name> - the budget doesn't exist." (UpdateBudget after DeleteBudget)
  budgets.delete-budget-cascades-notifications-and-subscribers [mismatch]:
  AssertionError: expected "Unable to get budget: <name> - the budget doesn't exist.", got "Unable to get notifications: <name> - the budget doesn't exist." (DescribeNotificationsForBudget after DeleteBudget)

Cause 5 — budgets.delete-budget-action-removes-resource [mismatch]:
  AssertionError: expected 'Action for [AccountId: <account-id>, ActionId: <action-id>] is not found!', got "Unable to get budget action: <action-id> - the action doesn't exist."

Cause 6 — budgets.execute-budget-action-wrong-state-rejected [mismatch]:
  AssertionError: expected ResourceLockedException, got 'InvalidParameterException' (and the expected message 'This method is not allowed during [ActionStatus: Standby]' was therefore never matched)

## Steps to Reproduce
All against the emulator with account 000000000000.

Cause 1: CreateBudget with Budget={BudgetName:N, BudgetLimit:{Amount:'50.0',Unit:'USD'}, TimeUnit:MONTHLY, BudgetType:COST, TimePeriod:{Start:2030-01-01Z, End:2029-01-01Z}} -> observe InvalidParameterException message.
Cause 2: CreateBudget with both BudgetLimit and PlannedBudgetLimits={'<epoch>':{Amount:'100.0',Unit:'USD'}} -> observe InvalidParameterException message.
Cause 3: CreateBudget N; CreateNotification 10 times (thresholds 10..100, ACTUAL/GREATER_THAN/PERCENTAGE, one EMAIL subscriber); CreateNotification an 11th (threshold 110) -> observe CreationLimitExceededException message.
Cause 4: CreateBudget N; DeleteBudget N; then (a) UpdateBudget with NewBudget named N, and (b) DescribeNotificationsForBudget N -> observe both NotFoundException messages.
Cause 5: CreateBudget N; CreateBudgetAction (APPLY_IAM_POLICY, MANUAL, ActionThreshold 80 PERCENTAGE, an assumable ExecutionRoleArn); DeleteBudgetAction; DescribeBudgetAction with the same ActionId -> observe NotFoundException message.
Cause 6: CreateBudget N; CreateBudgetAction as above (action is in Standby); ExecuteBudgetAction(ExecutionType='REVERSE_BUDGET_ACTION') -> observe the error code/status.

Replay: the check ids in trace_ids.
