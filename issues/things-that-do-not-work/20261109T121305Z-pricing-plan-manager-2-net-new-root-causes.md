Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pricing-plan-manager.create-subscription-rejections-leave-no-subscription pricing-plan-manager.mutations-on-nonexistent-subscription-rejected-consistently
Issue-Shape-Id: com.amazonaws.pricingplanmanager#AWSPricingPlanManager
Issue-Key: pricing-plan-manager.create-subscription-rejections-leave-no-subscription|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: pricing-plan-manager.mutations-on-nonexistent-subscription-rejected-consistently|run#7|AssertionError|assert code == "ValidationException", (
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# pricing-plan-manager: 2 net-new root causes — CreateSubscription accepts an unrecognized planFamily, and subscription-ARN operations answer 404 ResourceNotFoundException instead of 400 ValidationException "Invalid subscription ARN: <arn>"

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscription

## Description
Two distinct, net-new root causes in the Pricing Plan Manager emulator. Neither is covered by any open issue (list_existing_issues for 'pricing' and 'subscription' turns up nothing for this service), and neither failure is marked ALREADY FILED or OWNED by the suite report.

Cause 1 of 2: CreateSubscription performs no plan-catalog validation, so an unrecognized planFamily is accepted and a subscription is minted.
AWS: CreateSubscription models planFamily as a required String whose documented domain is the set of offered pricing plan families ("The pricing plan family to subscribe to, such as CloudFront"). A family the service does not offer is a business-rule failure, which this service reports through the modeled ValidationException (httpError 400, members message/resourceId; doc: "The request failed a business rule validation"). Smithy: CreateSubscription.planFamily @required String; error ValidationException (400). Recorded behavior: the refusal is HTTP 400, Error.Code "ValidationException", Error.Message "Invalid plan name: <planFamily>" — the fixed text is "Invalid plan name: " and the only variable part is the planFamily value copied verbatim from the request.
Emulator: CreateSubscription(planFamily="NotARealPlanFamily", planTier="PRO", resourceArns=[<cf-arn>,<waf-arn>]) answers HTTP 200 with a fully-formed subscription whose planFamily echoes the unrecognized family and whose status is ACTIVE. services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/create_subscription.rs:190-294 destructures the input and goes straight to minting: the only validation in handle() is the approvalMode enum re-check (lines 202-221) and the account capacity check (lines 245-256). There is no plan-family lookup, no plan-tier lookup against the family's offered tiers, and nothing that keys off the catalog at all.
Why: a client that relies on AWS refusing a typo'd or unavailable plan family instead gets a 200 and a persisted ACTIVE subscription against a plan that does not exist. The error path it was written to handle never fires, the bad name is silently accepted into state, and every later read (GetSubscription, ListSubscriptions) reports a subscription to a nonexistent plan — so a provisioning flow that validates by attempting the call cannot tell a supported family from an unsupported one. The same missing catalog gate means the check's two downstream assertions in this group — an unoffered planTier must be refused with ValidationException 'Invalid plan tier "<planTier>" for the plan "<plan-code>"', and a resourceArns list longer than the modeled @length max of 10 must be refused with AWS's standard constraint sentence ("1 validation error detected: Value '[...]' at 'resourceArns' failed to satisfy constraint: Member must have length less than or equal to 10") — were never reached, so they are unverified behind this first refusal and should be re-checked once the gate exists.
Fix: CreateSubscription's input validation in its handler (services/.../src/ops/create_subscription.rs handle()), backed by a plan catalog in crate::state: resolve planFamily (and then planTier within that family) before any state mutation and return the modeled ValidationException with AWS's wording when either does not resolve.
Traces: pricing-plan-manager.create-subscription-rejections-leave-no-subscription.

Cause 2 of 2: the operations that take a subscription ARN never validate the ARN's syntax, so a malformed ARN is treated as a missing resource — wrong status, wrong error code and wrong message.
AWS: Smithy: GetSubscription.arn targets SubscriptionArn (@length 1..2048, @arnReference AWS::PricingPlanManager::Subscription); the operation models both ValidationException (400) and ResourceNotFoundException (404). An ARN that is not a well-formed subscription ARN fails input validation before any lookup, so AWS answers HTTP 400 with Error.Code "ValidationException". Recorded behavior: Error.Message "Invalid subscription ARN: <arn>" — fixed text "Invalid subscription ARN: " plus the arn value copied verbatim from the request. The ARN exercised here carries no region (arn:aws:pricingplanmanager::<account>:subscription/<id>), which is why it is a syntax refusal and not a not-found. AWS applies that check identically, and before any other input the operation also carries, on every operation that accepts a subscription ARN.
Emulator: GetSubscription(arn="arn:aws:pricingplanmanager::<account>:subscription/<id>") answers HTTP 404 with Error.Code "ResourceNotFoundException" and the emulator-authored message "no subscription with ARN <arn>". services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/get_subscription.rs:160-172 goes straight from the store lock to store.subscription(&input.arn) and reports ResourceNotFoundException on the miss; there is no ARN well-formedness check anywhere ahead of it. The identical shape is in the other four handlers the check exercises (associate_resources_to_subscription.rs, disassociate_resources_from_subscription.rs, cancel_subscription.rs, cancel_subscription_change.rs), so the divergence is not specific to GetSubscription — the check stopped at the first one.
Why: a client cannot distinguish a client-side ARN construction bug from a subscription that was legitimately cancelled or never existed. It sees a 404 it will treat as "gone, carry on" (and botocore's retry/branching on ResourceNotFoundException vs ValidationException differ) when AWS is telling it the request itself is malformed, and the message it logs or surfaces is emulator wording rather than AWS's. Because the check also asserts the five ARN-taking operations refuse identically and before looking at their other inputs, a fix must install the validation uniformly, not just in GetSubscription.
Fix: shared subscription-ARN validation in crate::state (an ARN parser/validator), invoked at the top of each ARN-taking operation's handler — get_subscription, associate_resources_to_subscription, disassociate_resources_from_subscription, cancel_subscription, cancel_subscription_change — ahead of the store lookup and ahead of ifMatch/resourceArns handling, returning the modeled ValidationException (400) with the message above; the existing ResourceNotFoundException path stays for well-formed ARNs that name nothing.
Traces: pricing-plan-manager.mutations-on-nonexistent-subscription-rejected-consistently.

## Evidence
Cause 1 — check pricing-plan-manager.create-subscription-rejections-leave-no-subscription
Expected (AWS): HTTP 400, Error.Code="ValidationException", Error.Message="Invalid plan name: NotARealPlanFamily" (fixed text "Invalid plan name: " + the request's planFamily).
Actual (emulator): HTTP 200 with a created subscription —
  AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'eTag': '...', 'subscription': {'arn': 'arn:aws:pricingplanmanager:us-east-1:<account>:subscription/<uuid>', 'planFamily': 'NotARealPlanFamily', 'planTier': 'PRO', 'status': 'ACTIVE', 'resourceArns': ['arn:aws:cloudfront::<account>:distribution/<marker>', 'arn:aws:wafv2:us-east-1:<account>:global/webacl/<marker>/<uuid>'], 'createdAt': ..., 'updatedAt': ...}}
(The arn, eTag, marker and timestamps are per-run values, not part of the requirement.)
Handler with no catalog gate: services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/create_subscription.rs:190-294.
Unreached assertions in the same check (unverified behind cause 1): planTier refusal message 'Invalid plan tier "<planTier>" for the plan "<plan-code>"'; oversized resourceArns refusal "1 validation error detected: Value '[<arns>]' at 'resourceArns' failed to satisfy constraint: Member must have length less than or equal to 10".

Cause 2 — check pricing-plan-manager.mutations-on-nonexistent-subscription-rejected-consistently
Expected (AWS): HTTP 400, Error.Code="ValidationException", Error.Message="Invalid subscription ARN: <arn>" for each of GetSubscription, AssociateResourcesToSubscription, DisassociateResourcesFromSubscription, CancelSubscription, CancelSubscriptionChange.
Actual (emulator), on the first call:
  AssertionError: GetSubscription: expected ValidationException, got ResourceNotFoundException: 'no subscription with ARN arn:aws:pricingplanmanager::<account>:subscription/<id>'
Served as HTTP 404 ResourceNotFoundException. Source: services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/src/ops/get_subscription.rs:164-172 (`message: format!("no subscription with ARN {}", input.arn)`). Smithy: ValidationException httpError 400, ResourceNotFoundException httpError 404; GetSubscription models both.

## Steps to Reproduce
Cause 1:
1. Point a botocore 'pricing-plan-manager' client at the emulator (region us-east-1).
2. Call CreateSubscription(planFamily="NotARealPlanFamily", planTier="PRO", resourceArns=["arn:aws:cloudfront::<account>:distribution/<any>", "arn:aws:wafv2:us-east-1:<account>:global/webacl/<any>/11111111-1111-1111-1111-111111111111"]).
3. Observe HTTP 200 and a persisted ACTIVE subscription; AWS answers 400 ValidationException "Invalid plan name: NotARealPlanFamily".
4. Then call CreateSubscription(planFamily="CloudFront", planTier="NotARealPlanTier", ...) and CreateSubscription(planFamily="CloudFront", planTier="PRO", resourceArns=[<11 ARNs>]) to exercise the two assertions that sit behind this one.

Cause 2:
1. Same client.
2. Call GetSubscription(arn="arn:aws:pricingplanmanager::<account>:subscription/does-not-exist") — note the empty region segment, which makes the ARN malformed.
3. Observe HTTP 404 ResourceNotFoundException "no subscription with ARN <arn>"; AWS answers HTTP 400 ValidationException "Invalid subscription ARN: <arn>".
4. Repeat with AssociateResourcesToSubscription / DisassociateResourcesFromSubscription / CancelSubscription / CancelSubscriptionChange (arn=<same>, ifMatch="1") — each must give the identical 400/ValidationException/message, and must do so before validating its other inputs.

Replay: run_behavioral_suite and re-run the two checks named in trace_ids.
