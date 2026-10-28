Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.remove-tags-then-verify-and-reject-missing-resource
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.remove-tags-then-verify-and-reject-missing-resource|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 990924ba522eb45c22a280dae132295e30055de9

# ELBv2 RemoveTags: the TargetGroupNotFound refusal uses the collection wording "One or more target groups not found" instead of AWS's "Target groups '<arn>' not found"

> _Note: this issue was identified against the code base at commit `990924ba522eb45c22a280dae132295e30055de9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RemoveTags

## Description
REOPENS 20261026T071220Z-elbv2-removetags-is-still-a-501-notimplemented-scaffold.md: its fix left the call refused. That fixed issue is grounded in exactly this check's replay key (elbv2.remove-tags-then-verify-and-reject-missing-resource) and stated the required refusal verbatim as "Target groups '<arn>' not found". RemoveTags is no longer a 501 scaffold — the success path (remove one of two keys, the other key survives, the rejected call mutates nothing) now passes, and the refusal's status and code are right — but the refusal's MESSAGE still diverges, so the same check is still red on the same RemoveTags call. This is not a new defect; it is the un-finished part of that fix.

Cause 1 of 1 — the TargetGroupNotFound message RemoveTags emits is the collection-level template, not the per-ARN template AWS uses for a target-group ARN.

AWS: Smithy RemoveTags(ResourceArns: ResourceArns required, TagKeys: TagKeys required) -> RemoveTagsOutput, SUCCESS_STATUS 200, empty output body. Modeled errors are all errorKind client / httpError 400: ListenerNotFoundException (awsQueryError code ListenerNotFound), LoadBalancerNotFoundException (LoadBalancerNotFound), RuleNotFoundException (RuleNotFound), TargetGroupNotFoundException (TargetGroupNotFound), TooManyTagsException (TooManyTags), TrustStoreNotFoundException (TrustStoreNotFound). Model doc for TargetGroupNotFoundException: "The specified target group does not exist." The model does not state message text, so the message is recorded behavior only: a RemoveTags whose ResourceArns names a targetgroup-shaped ARN with nothing behind it is refused with HTTP 400 / Error.Code=TargetGroupNotFound and Error.Message "Target groups '<arn>' not found". In that template "Target groups '" and "' not found" are FIXED text and the quoted value is the offending ARN taken verbatim from the request's ResourceArns — the recorded run's particular ARN (a bogus name+id spliced onto the live target group's account/region prefix) fills the template and is not part of it. Note that this wording is per-resource-kind/per-operation and NOT uniform across the tag family: fixed issue 20261027T051252Z-elbv2-addtags-on-a-deleted-load-balancer-the.md pinned the opposite form for the load-balancer kind (AddTags -> LoadBalancerNotFound "One or more load balancers not found", pure fixed text, no ARN interpolated). The target-group refusal of RemoveTags keeps the parameterised form.

Emulator: HTTP 400 and Error.Code=TargetGroupNotFound (both correct), but Error.Message is the collection sentence "One or more target groups not found" — no ARN interpolated at all. The handler builds the message from a single kind-keyed collection template shared by the whole tag family, so every ARN of every kind gets the un-parameterised sentence, including the kinds AWS parameterises.

Why: the error message is part of the contract clients read. RemoveTags may name up to many ARNs in one call, and the per-ARN form is what tells a caller WHICH of the ARNs it submitted does not exist; the collection form says only that at least one is gone. A reconciliation or tag-cleanup client that parses or logs the ARN out of the TargetGroupNotFound message gets nothing to parse, and anything matching ELBv2's not-found text sees wording AWS never emits for this error class. It also makes the just-"fixed" refusal path indistinguishable from AWS only in status and code, not in body.

Fix: the message construction in the RemoveTags handler's not-found path — services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/remove_tags.rs, fn not_found (the TaggableResource::TargetGroup arm), which fills every variant's message from crate::state::taggable_not_found_message(kind) and whose doc comment asserts the collection wording for all kinds. That kind-keyed helper needs a parameterised form for the kinds AWS parameterises, and the TargetGroup arm of RemoveTags must use the per-ARN template with the offending ResourceArns entry interpolated. Nothing else about the refusal changes: still HTTP 400, still awsQueryError code TargetGroupNotFound, still atomic (no tag withdrawn from the live resources named in the same call). The sibling kinds' templates should be re-grounded the same way rather than all sharing one sentence.

Traces: elbv2.remove-tags-then-verify-and-reject-missing-resource (its final leg: the bogus-target-group-ARN refusal; the AddTags/DescribeTags round trip and the one-of-two-keys removal in the same check now pass). Related open issue 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md still names "AddTags / RemoveTags" in its Cause 1, but its key for that cause is elbv2|AddTags|NotImplemented and it does not cover this message defect.

## Evidence
Expected (AWS, recorded): RemoveTags(ResourceArns=[<a targetgroup-shaped ARN sharing the live target group's account/region prefix but naming a name+id never created>], TagKeys=['owner']) -> HTTP 400, Error.Code = TargetGroupNotFound, Error.Message = "Target groups '<that ARN>' not found" (fixed text "Target groups '" + the ARN from the request + "' not found").
Actual (emulator): HTTP 400, Error.Code = TargetGroupNotFound (both correct), Error.Message = "One or more target groups not found" — the collection sentence, with no ARN interpolated.
Replay output (elbv2.remove-tags-then-verify-and-reject-missing-resource, mismatch), from check_elbv2_remove_tags_then_verify_and_reject_missing_resource.py line 47:
  AssertionError: expected message "Target groups 'arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-nonexistent-tg/0123456789abcdef' not found", got 'One or more target groups not found'
(the region/account/name/id in that literal are per-run values filling the template, not requirements).
Passing legs of the same check, proving only the wording diverges: CreateTargetGroup(TargetType='lambda') 200; AddTags two keys then DescribeTags -> {env:test, owner:sim2real}; RemoveTags(TagKeys=['env']) -> 200 and DescribeTags -> {owner:sim2real}; the error assertion on code == 'TargetGroupNotFound' passes; the post-refusal re-read showing the live target group still carries owner=sim2real passes.
Source: services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/remove_tags.rs:280-281 (fn not_found -> crate::state::taggable_not_found_message(kind)) and its doc comment at :276-279 ("the collection-level wording of that kind, because this call selects its resources through a LIST and so names none of them back") — that premise is wrong for the target-group kind.
Contrast already pinned by a fixed issue: 20261027T051252Z-elbv2-addtags-on-a-deleted-load-balancer-the.md (AddTags/LoadBalancerNotFound IS the un-parameterised "One or more load balancers not found"), so the two templates must be chosen per kind rather than unified.

## Steps to Reproduce
1. CreateTargetGroup(Name=<unique>, TargetType='lambda') -> note TargetGroupArn (tg_arn).
2. AddTags(ResourceArns=[tg_arn], Tags=[{Key:'env',Value:'test'},{Key:'owner',Value:'sim2real'}]); DescribeTags(ResourceArns=[tg_arn]) confirms both keys.
3. RemoveTags(ResourceArns=[tg_arn], TagKeys=['env']) -> 200; DescribeTags shows only owner=sim2real. (Both pass today.)
4. Build a bogus ARN: take tg_arn.rsplit('/', 2)[0] and append '/sim2real-nonexistent-tg/0123456789abcdef'.
5. Call RemoveTags(ResourceArns=[bogus_arn], TagKeys=['owner']) and read the HTTP status, Error.Code and Error.Message.
6. Expected: 400 / TargetGroupNotFound / "Target groups '<bogus_arn>' not found". Observed: 400 / TargetGroupNotFound / "One or more target groups not found".
7. Confirm the refusal is still atomic: DescribeTags(ResourceArns=[tg_arn]) still shows owner=sim2real (this passes).
8. For contrast, repeat step 5 against a deleted load balancer ARN via AddTags — that path must stay the un-parameterised "One or more load balancers not found", so the fix cannot simply parameterise every kind.
