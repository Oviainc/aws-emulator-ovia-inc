Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: billing.associate-source-views-on-primary-view-rejected billing.get-resource-policy-not-found-when-unshared billing.list-source-views-for-nonexistent-billing-view-rejected billing.tag-resource-managed-billing-view-rejected
Issue-Shape-Id: com.amazonaws.billing#AWSBilling
Issue-Key: billing.associate-source-views-on-primary-view-rejected|run#5|AssertionError|assert code == "ValidationException", code
Issue-Key: billing.get-resource-policy-not-found-when-unshared|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: billing.list-source-views-for-nonexistent-billing-view-rejected|run#7|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Key: billing.tag-resource-managed-billing-view-rejected|run#7|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# Billing: four billing-view operations answer 501 NotImplemented instead of their modeled responses (AssociateSourceViews, GetResourcePolicy, ListSourceViewsForBillingView, TagResource/ListTagsForResource)

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateSourceViews

## Description
Four operations of the AWS Billing (awsbilling) emulator are still scaffold stubs: they answer HTTP 501 with error code `NotImplemented` for every request, so no client can exercise the source-view, resource-policy or tagging surface at all. This issue bundles FOUR distinct root causes, one per operation group. Each must be closed before submitting.

Cause 1 of 4: AssociateSourceViews answers 501 NotImplemented.
  AWS: Smithy model (services/service-com-amazonaws-billing-awsbilling/smithy-model/com-amazonaws-billing-awsbilling.json) binds AssociateSourceViews with input `arn` (BillingViewArn, required, pattern ^arn:aws[a-z-]*:(billing)::[0-9]{12}:billingview/[a-zA-Z0-9/:_\+=\.\-@]{0,75}[a-zA-Z0-9]$) and `sourceViews` (BillingViewSourceViewsList, required, length 1..10), output `arn`, and errors BillingViewHealthStatusException, ConflictException, ResourceNotFoundException, ServiceQuotaExceededException plus the service-common AccessDeniedException/ValidationException (400)/ThrottlingException/InternalServerException. Recorded behavior: associating a billing view with ITSELF is refused with HTTP 400 `ValidationException` (recorded message example: "A billing view cannot be associated to itself"), and the target view's `sourceViewCount` is unchanged. The requirement is the 400 + ValidationException classification and the absence of any state change; the message text above is an example.
  Emulator: the request is answered with code `NotImplemented` (HTTP 501). The self-association business rule never runs.
  Why: a client that depends on AWS refusing a self-referential association gets an un-modeled 501 instead of a retry-safe 400, and SDK error handling (which only knows the modeled error set) cannot classify it. Any client probing whether an association is legal sees a transport-level failure.
  Fix: implement the operation's handler and its validation — the self-association rule and the source-view state update belong in the handler, which is the single file for this operation.
  Traces: billing.associate-source-views-on-primary-view-rejected (handler: services/service-com-amazonaws-billing-awsbilling/src/ops/associate_source_views.rs).

Cause 2 of 4: GetResourcePolicy answers 501 NotImplemented.
  AWS: Smithy model binds GetResourcePolicy with input `resourceArn` (ResourceArn, required, pattern ^arn:aws[a-z-]*:(billing)::[0-9]{12}:[a-zA-Z0-9/:_\+=\.\@-]{0,70}[a-zA-Z0-9]$), output {resourceArn (required), policy}, and modeled error ResourceNotFoundException (HTTP 404, members message/resourceId/resourceType all required). Recorded behavior: GetResourcePolicy addressed at the account's PRIMARY billing view is refused with HTTP 404 `ResourceNotFoundException` because only a CUSTOM billing view can ever carry a resource-based policy (recorded message example: "The custom billing view does not exist."), even though the same ARN resolves successfully through GetBillingView. The requirement is the 404 + ResourceNotFoundException classification for a non-CUSTOM / policy-less view; the message text is an example.
  Emulator: the request is answered with code `NotImplemented` (HTTP 501).
  Why: a client reading a policy to decide whether a view is shared cannot distinguish "no policy" from "service broken"; 501 is not in the modeled error set, so retry/branch logic keyed on ResourceNotFoundException never fires.
  Fix: implement the operation's handler and its not-found path.
  Traces: billing.get-resource-policy-not-found-when-unshared (handler: services/service-com-amazonaws-billing-awsbilling/src/ops/get_resource_policy.rs).

Cause 3 of 4: ListSourceViewsForBillingView answers 501 NotImplemented, so the not-found path for a well-formed but nonexistent billing-view ARN never runs.
  AWS: Smithy model binds ListSourceViewsForBillingView with input `arn` (BillingViewArn, required), paginated output `sourceViews` (required) + `nextToken`, and modeled error ResourceNotFoundException (HTTP 404). Recorded behavior: a syntactically valid billingview ARN that names no existing view is refused with ResourceNotFoundException (recorded message example: "[<arn>] is an invalid billing view arn"), NEVER a 200 with an empty list — an empty list is reserved for a view that exists with zero sources. The requirement is the ResourceNotFoundException classification that distinguishes "does not exist" from "exists with no sources"; the message text is an example.
  Emulator: the request is answered with code `NotImplemented` (HTTP 501) for every input, so neither the not-found nor the success path exists.
  Why: a client cannot tell a missing view from an empty one, and aggregation logic over source views breaks on an un-modeled 501.
  Fix: implement the operation's handler, including the lookup-then-404 path, in the operation's own file. NOTE: the separately filed scaffold-stub issue "Implement billing ListSourceViewsForBillingView: unimplemented scaffold stub" (filed for this run against services/service-com-amazonaws-billing-awsbilling/src/ops/list_source_views_for_billing_view.rs) names the same operation; this cause records the specific not-found-vs-empty semantics that implementation must satisfy, which that issue does not state.
  Traces: billing.list-source-views-for-nonexistent-billing-view-rejected.

Cause 4 of 4: TagResource and ListTagsForResource both answer 501 NotImplemented, so the managed-billing-view and not-found rejection classes never run.
  AWS: Smithy model binds TagResource (input resourceArn required + resourceTags required, length 0..200; modeled error ResourceNotFoundException HTTP 404) and ListTagsForResource (input resourceArn required; output resourceTags; modeled error ResourceNotFoundException HTTP 404), both with the service-common ValidationException (HTTP 400). Recorded behavior: against the account's PRIMARY (AWS-managed) billing view BOTH operations are refused with HTTP 400 `ValidationException` because managed billing views are not taggable (recorded message example: "Managed billing views cannot be tagged"); against a well-formed CUSTOM billingview ARN that does not exist BOTH are refused with HTTP 404 `ResourceNotFoundException` (recorded message example: "Resource not found"). The requirement is the two distinct classifications — 400 ValidationException for a managed view, 404 ResourceNotFoundException for a nonexistent one — and that a failed TagResource attaches no tag; the message texts are examples.
  Emulator: both operations are answered with code `NotImplemented` (HTTP 501) regardless of ARN, so neither classification is distinguishable.
  Why: tag-management clients (cost allocation, tag policies) get an un-modeled 501 for every call, and cannot tell an untaggable managed resource from a missing one.
  Fix: implement both operations' handlers and their validation (managed-view check before the existence check), plus the tag store they read and write.
  Traces: billing.tag-resource-managed-billing-view-rejected (handlers: services/service-com-amazonaws-billing-awsbilling/src/ops/tag_resource.rs, which still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Scaffolded::not_implemented("TagResource")` at line 160, and .../src/ops/list_tags_for_resource.rs).

## Evidence
Replayed behavioral suite (simparity-behavioral-aws-v1), billing 3/19 passed.

1) billing.associate-source-views-on-primary-view-rejected
   Expected: error code "ValidationException" (HTTP 400) from AssociateSourceViews(arn=<PRIMARY>, sourceViews=[<PRIMARY>]).
   Actual:   AssertionError: NotImplemented  (assert code == "ValidationException", code)

2) billing.get-resource-policy-not-found-when-unshared
   Expected: error code "ResourceNotFoundException" (HTTP 404) from GetResourcePolicy(resourceArn=<PRIMARY>).
   Actual:   AssertionError: expected ResourceNotFoundException, got NotImplemented

3) billing.list-source-views-for-nonexistent-billing-view-rejected
   Expected: error code "ResourceNotFoundException" from ListSourceViewsForBillingView(arn=arn:aws:billing::<acct>:billingview/sim2real-behav-does-not-exist).
   Actual:   AssertionError: NotImplemented

4) billing.tag-resource-managed-billing-view-rejected
   Expected: "ResourceNotFoundException" from TagResource/ListTagsForResource on a nonexistent custom billingview ARN, then "ValidationException" on the PRIMARY view.
   Actual:   AssertionError: NotImplemented  (first assertion, the nonexistent-ARN TagResource call)

Source confirmation: services/service-com-amazonaws-billing-awsbilling/src/ops/tag_resource.rs lines 3-6 declare "SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`" and line 160 returns `Err(crate::scaffolded::Scaffolded::not_implemented("TagResource"))`. By contrast .../src/ops/get_billing_view.rs is implemented and reads real state, confirming the service itself is stood up and serving (its bootstrap issue 20261021T215315Z-bootstrap-service-com-amazonaws-billing-awsbilling.md is resolved).

## Steps to Reproduce
Boot the emulator and point a botocore `billing` client at it with any credentials.

1. primary = client.list_billing_views(billingViewTypes=["PRIMARY"])["billingViews"][0]["arn"]
2. client.associate_source_views(arn=primary, sourceViews=[primary])
   -> observe code NotImplemented / HTTP 501; AWS answers 400 ValidationException.
3. client.get_resource_policy(resourceArn=primary)
   -> observe code NotImplemented / HTTP 501; AWS answers 404 ResourceNotFoundException.
4. client.list_source_views_for_billing_view(arn=f"arn:aws:billing::{account_id}:billingview/does-not-exist")
   -> observe code NotImplemented / HTTP 501; AWS answers 404 ResourceNotFoundException.
5. client.tag_resource(resourceArn=f"arn:aws:billing::{account_id}:billingview/custom-a1b2c3d4-e5f6-47a8-9b0c-d1e2f3a4b5c6", resourceTags=[{"key":"k","value":"v"}])
   -> observe code NotImplemented / HTTP 501; AWS answers 404 ResourceNotFoundException.
6. client.tag_resource(resourceArn=primary, resourceTags=[{"key":"k","value":"v"}]) and client.list_tags_for_resource(resourceArn=primary)
   -> observe code NotImplemented / HTTP 501; AWS answers 400 ValidationException for a managed view.

Re-replay billing.associate-source-views-on-primary-view-rejected, billing.get-resource-policy-not-found-when-unshared, billing.list-source-views-for-nonexistent-billing-view-rejected and billing.tag-resource-managed-billing-view-rejected to confirm a fix.
