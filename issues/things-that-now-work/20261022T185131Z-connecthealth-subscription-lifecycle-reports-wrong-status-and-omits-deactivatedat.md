Issue-Source: trace-sweep-divergence
Issue-Trace: connecthealth-subscription-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connecthealth-subscription-lifecycle connecthealth-get-subscription-not-found connecthealth-medical-scribe-listening-session-not-found
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth|CreateSubscription|2xx->2xx|$.lastUpdatedAt
Issue-Key: connecthealth|CreateSubscription|2xx->2xx|$.status
Issue-Key: connecthealth|GetSubscription|2xx->2xx|$.subscription.status
Issue-Key: connecthealth|ListSubscriptions|2xx->2xx|$.subscriptions[arn="arn
Issue-Key: connecthealth|DeactivateSubscription|2xx->2xx|$.subscription.deactivatedAt
Issue-Key: connecthealth|GetSubscription|2xx->2xx|$.subscription.deactivatedAt
Issue-Key: connecthealth|ActivateSubscription|2xx->2xx|$.subscription.deactivatedAt
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# Subscription lifecycle reports wrong status and omits deactivatedAt timestamp

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscription

## Description
The ConnectHealth Subscription state machine is wrong in two linked ways: a newly created subscription is left in INACTIVE instead of the ACTIVE state the recording shows, and the deactivation timestamp is never written to or read back from subscription state, so every op that projects a SubscriptionDescription loses it. This bundle carries 6 causes; all 6 must be closed.

Cause 1 of 6 - CreateSubscription returns status INACTIVE instead of ACTIVE, and populates lastUpdatedAt where the recording shows it absent (fixture connecthealth-subscription-lifecycle, step task#2).
AWS: Smithy CreateSubscriptionOutput.status -> SubscriptionStatus, enum values ACTIVE, INACTIVE, DELETED; CreateSubscription is documented 'Creates a new Subscription within a Domain for billing and user management' and the recorded 201 response reports the new subscription as ACTIVE - creation activates it, it is not a two-step create-then-activate resource. The recording also shows no lastUpdatedAt in the create response (the resource has not been updated yet), even though the model marks CreateSubscriptionOutput.lastUpdatedAt required.
Emulator: answers 201 with $.status = 'INACTIVE' and a populated $.lastUpdatedAt (type_mismatch, expected None).
Why: status is the billing-enablement signal for the subscription. A client that creates a subscription and then checks status before admitting the user sees INACTIVE and concludes creation half-failed, or issues a redundant ActivateSubscription; a client that diffs lastUpdatedAt to detect concurrent modification sees a change on a resource AWS reports as never updated.
Fix: the CreateSubscription handler's initial state - create the subscription in the ACTIVE state and leave the last-updated timestamp unset until the first mutation.

Cause 2 of 6 - GetSubscription returns subscription.status INACTIVE where the recording shows ACTIVE (fixture connecthealth-get-subscription-not-found, step task#3).
AWS: Smithy GetSubscriptionOutput.subscription -> SubscriptionDescription, whose status member targets SubscriptionStatus [ACTIVE, INACTIVE, DELETED]. The recording shows a freshly created, never-deactivated subscription read back as ACTIVE.
Emulator: answers 200 with $.subscription.status = 'INACTIVE'.
Why: this is the read-back half of Cause 1 - the wrong creation state is persisted, so every describe of the subscription reports a disabled subscription. This cluster covers 4 steps across two fixtures (including the medical-scribe fixture), so it is the broadest-reach symptom in the bundle.
Fix: follows from Cause 1 (the persisted initial status), plus the GetSubscription projection if it overrides status.

Cause 3 of 6 - ListSubscriptions reports the subscription's status as INACTIVE instead of ACTIVE (fixture connecthealth-subscription-lifecycle, step task#3).
AWS: Smithy ListSubscriptionsOutput.subscriptions -> SubscriptionList (required); each element carries the subscription's status from SubscriptionStatus. The recording shows the just-created subscription listed as ACTIVE.
Emulator: answers 200 with the matching subscription element carrying status 'INACTIVE'.
Why: list and get must agree with create. A client that enumerates a domain's subscriptions to count billable users counts zero.
Fix: the ListSubscriptions projection reads the same persisted status as Cause 1 - correcting the initial state must be verified through the list path as well, since it serializes each element separately.

Cause 4 of 6 - DeactivateSubscription response omits subscription.deactivatedAt (fixture connecthealth-subscription-lifecycle, step task#4).
AWS: Smithy SubscriptionDescription.deactivatedAt -> Timestamp (optional member); DeactivateSubscription is documented 'Deactivates a Subscription to stop billing for a user'. The recording shows the 200 response carrying deactivatedAt stamped with the moment of deactivation.
Emulator: answers 200 with $.subscription.deactivatedAt absent.
Why: deactivatedAt is the billing-stop boundary. A client that computes a billing period from it has no end timestamp and must guess, so usage accounting against the emulator cannot be reproduced.
Fix: the DeactivateSubscription handler's state transition - write the deactivation timestamp onto the subscription and project it in the response.

Cause 5 of 6 - GetSubscription omits subscription.deactivatedAt for a deactivated subscription (fixture connecthealth-subscription-lifecycle, step task#5).
AWS: as Cause 4 - SubscriptionDescription.deactivatedAt is a Timestamp member, and the recording shows GetSubscription returning it for a subscription that has been deactivated.
Emulator: answers 200 with $.subscription.deactivatedAt absent.
Why: the deactivation boundary is unreadable on the describe path, so a client cannot recover it after the Deactivate call's response is gone.
Fix: follows from Cause 4 (persisting the timestamp) plus the GetSubscription projection of SubscriptionDescription.

Cause 6 of 6 - ActivateSubscription omits subscription.deactivatedAt after re-activation (fixture connecthealth-subscription-lifecycle, step task#6).
AWS: SubscriptionDescription.deactivatedAt -> Timestamp; ActivateSubscription is documented 'Activates a Subscription to enable billing for a user'. The recording shows the 200 re-activation response STILL carrying the prior deactivatedAt - AWS retains the historical deactivation stamp rather than clearing it on re-activation.
Emulator: answers 200 with $.subscription.deactivatedAt absent.
Why: a client reconstructing a subscription's billing history from activatedAt/deactivatedAt loses the previous inactive interval entirely. Note the requirement here is retention, so a fix that merely clears the field on activate would still diverge.
Fix: the ActivateSubscription handler's state transition - set activation state without clearing the deactivation timestamp, and project it in the response.

Traces: connecthealth-subscription-lifecycle#task#2 (CreateSubscription), connecthealth-get-subscription-not-found#task#3 (GetSubscription; cluster also covers connecthealth-get-subscription-not-found#task#5, connecthealth-medical-scribe-listening-session-not-found#task#3, connecthealth-medical-scribe-listening-session-not-found#task#5), connecthealth-subscription-lifecycle#task#3 (ListSubscriptions), connecthealth-subscription-lifecycle#task#4 (DeactivateSubscription), connecthealth-subscription-lifecycle#task#5 (GetSubscription; cluster also covers connecthealth-subscription-lifecycle#task#7), connecthealth-subscription-lifecycle#task#6 (ActivateSubscription).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, state_reset_failures=0, 15/43 steps passed, score 34.9%).

Cause 1 - connecthealth-subscription-lifecycle#task#2, CreateSubscription, expected 201 / actual 201:
  $.status: value_mismatch (expected 'ACTIVE', got 'INACTIVE')
  $.lastUpdatedAt: type_mismatch (expected None, got 1792694907.9145951)

Cause 2 - connecthealth-get-subscription-not-found#task#3, GetSubscription, expected 200 / actual 200 (cluster of 4: also connecthealth-get-subscription-not-found#task#5, connecthealth-medical-scribe-listening-session-not-found#task#3, connecthealth-medical-scribe-listening-session-not-found#task#5):
  $.subscription.status: value_mismatch (expected 'ACTIVE', got 'INACTIVE')

Cause 3 - connecthealth-subscription-lifecycle#task#3, ListSubscriptions, expected 200 / actual 200:
  $.subscriptions[arn="arn:aws:health-agent:us-west-2:000000000000:domain/dom-13f29b0f6b8f7543/subscription/sub-3a95c6c9d07ab56013ac2"].status: value_mismatch (expected 'ACTIVE', got 'INACTIVE')

Cause 4 - connecthealth-subscription-lifecycle#task#4, DeactivateSubscription, expected 200 / actual 200:
  $.subscription.deactivatedAt: missing (expected 1788661206.693, got None)

Cause 5 - connecthealth-subscription-lifecycle#task#5, GetSubscription, expected 200 / actual 200 (cluster also covers connecthealth-subscription-lifecycle#task#7):
  $.subscription.deactivatedAt: missing (expected 1788661206.693, got None)

Cause 6 - connecthealth-subscription-lifecycle#task#6, ActivateSubscription, expected 200 / actual 200:
  $.subscription.deactivatedAt: missing (expected 1788661206.693, got None)

The ARN, domain id and the timestamp values (1788661206.693, 1792694907.9145951) are environment-specific examples, not requirements. The requirements are: the enum VALUE 'ACTIVE' vs 'INACTIVE'; the PRESENCE of deactivatedAt after a deactivation and its RETENTION across a re-activation; and the ABSENCE of lastUpdatedAt in the CreateSubscription response.

Model citations: Smithy SubscriptionStatus enum [ACTIVE, INACTIVE, DELETED]; CreateSubscriptionOutput.status (required) / .lastUpdatedAt (required in model, absent in recording) / .deactivatedAt (optional Timestamp); SubscriptionDescription.status, .deactivatedAt; GetSubscriptionOutput.subscription -> SubscriptionDescription; ListSubscriptionsOutput.subscriptions -> SubscriptionList (required).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: fixture connecthealth-subscription-lifecycle, step task#2 - POST /domains/{domainId}/subscriptions; inspect the 201 body: status must be ACTIVE and lastUpdatedAt must be absent.
4. Cause 2: fixture connecthealth-get-subscription-not-found, step task#3 - GET /domains/{domainId}/subscriptions/{subscriptionId} on a never-deactivated subscription; subscription.status must be ACTIVE. Re-replay the cluster's other members (connecthealth-get-subscription-not-found#task#5, connecthealth-medical-scribe-listening-session-not-found#task#3 and #task#5).
5. Cause 3: fixture connecthealth-subscription-lifecycle, step task#3 - GET /domains/{domainId}/subscriptions; the listed element's status must be ACTIVE.
6. Cause 4: same fixture, step task#4 - POST /domains/{domainId}/subscriptions/{subscriptionId}/deactivate; subscription.deactivatedAt must be present.
7. Cause 5: same fixture, step task#5 - GET the subscription; subscription.deactivatedAt must still be present (also re-replay task#7).
8. Cause 6: same fixture, step task#6 - POST .../activate; subscription.deactivatedAt must be RETAINED, not cleared.
9. All six causes must pass before this issue is closed.
