Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.tag-resource-reserved-aws-prefix-rejected wafv2.permission-policy-invalid-document-rejected wafv2.mobile-sdk-release-url-generation-lifecycle
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.tag-resource-reserved-aws-prefix-rejected|run#9|AssertionError|assert code == "WAFTagOperationException", f"expected WAFTagOperationException, got {code}"
Issue-Key: wafv2.permission-policy-invalid-document-rejected|run#7|AssertionError|assert code == "WAFInvalidPermissionPolicyException", code
Issue-Key: wafv2.mobile-sdk-release-url-generation-lifecycle|run#9|AssertionError|assert list_error == "WAFInvalidOperationException", (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# wafv2 rejection semantics missing on three operations: TagResource, PutPermissionPolicy and the Mobile-SDK family answer error code 'NotImplemented' instead of their modeled WAFv2 errors

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three behavioral checks exercise *negative* (rejection) paths of the wafv2 control plane. In every case the emulator answers a ClientError whose error code is the literal string 'NotImplemented' — a code that is not a member of any of these operations' modeled error sets — instead of the specific WAFv2 exception AWS raises. The checks therefore fail on the error *code*, not merely on a message difference. Each of the three handlers is also the subject of an existing "unimplemented scaffold stub" issue (20261024T065754Z-wafv2-tagresource-stub.md, 20261024T065754Z-wafv2-putpermissionpolicy-stub.md, 20261024T065754Z-wafv2-listmobilesdkreleases-stub.md); those issues ask for the operation to exist at all, whereas the causes below state the *rejection semantics* each operation must implement once it does, and each is grounded in a distinct failing check that the replay report did NOT mark as already filed or owned. This issue bundles THREE root causes; all three must be closed before submitting.

=== Cause 1 of 3: TagResource does not reject a reserved 'aws:' tag key ===
AWS: Smithy (com-amazonaws-wafv2-awswaf-20190729) models TagResource with input ResourceARN (ResourceArn, required, @length 20-2048, pattern \S) and Tags (TagList, required, @length min 1), and its error set includes WAFTagOperationException (errorKind client, httpError 400), alongside WAFInvalidParameterException, WAFLimitsExceededException, WAFNonexistentItemException, WAFInvalidOperationException, WAFInternalErrorException, WAFTagOperationInternalErrorException. AWS reserves tag keys beginning with the 'aws:' prefix for system tags and refuses to set them; the recorded rejection is HTTP 400 with error code WAFTagOperationException. The message is AWS-documented for the reserved-prefix rule, recorded as the example "You tried to tag your resources with system tag keys. These tags have an aws: prefix and are reserved by AWS. Change your tag keys, and try again." — the requirement is the rule (reject a key with the reserved 'aws:' prefix, 400 / WAFTagOperationException), not the literal text. The resource must be left without that tag, while a non-reserved key (the check's positive control, e.g. Team=sim2real) is accepted and surfaced by ListTagsForResource.
Emulator: TagResource on an IPSet ARN with Tags=[{Key: 'aws:Reserved', Value: 'x'}] returns error code 'NotImplemented'. Replay: "AssertionError: expected WAFTagOperationException, got NotImplemented".
Why: a client that distinguishes a reserved-tag rejection from other tagging failures (retry logic, tag-governance tooling, CDK/Terraform tag propagation) sees an unmodeled code it cannot classify; worse, nothing in the emulator demonstrates that reserved keys are refused, so code that would be rejected in AWS appears untested, and tag reads cannot be relied on to exclude system tags.
Fix: the TagResource handler and its input validation — reject keys matching the reserved 'aws:' prefix with 400/WAFTagOperationException before mutating tag state, and keep tag state unchanged on rejection.
Traces: wafv2.tag-resource-reserved-aws-prefix-rejected.

=== Cause 2 of 3: PutPermissionPolicy does not validate the policy document ===
AWS: Smithy models PutPermissionPolicy with input ResourceArn (required, @length 20-2048, pattern \S) and Policy (PolicyString, required, @length 1-395000, pattern \S), and its error set is exactly WAFInternalErrorException (500), WAFInvalidParameterException (400), WAFInvalidPermissionPolicyException (400), WAFNonexistentItemException (400). A syntactically malformed policy document (the check sends the non-JSON string '{not valid json' against a freshly created rule group's ARN) is refused with HTTP 400 and error code WAFInvalidPermissionPolicyException — the model names this error precisely for an unacceptable policy specification. Recorded message example: "The policy specification doesn't meet the requirements to be attached to an AWS WAF resource." The rejected document must not be attached: a follow-up GetPermissionPolicy on the same rule group must still raise WAFNonexistentItemException (400).
Emulator: PutPermissionPolicy with the malformed document returns error code 'NotImplemented'. Replay: "AssertionError: NotImplemented" at the assert on WAFInvalidPermissionPolicyException.
Why: resource-policy validation is the gate that stops a malformed or over-broad cross-account policy from being attached to a rule group. A client gets an unmodeled code, cannot tell "your policy document is bad" from a server fault, and no emulator-side test of policy validation or of the attached/not-attached state transition is possible.
Fix: the PutPermissionPolicy handler plus its policy-document validation, and the permission-policy state it writes (nothing may be persisted when validation fails, so GetPermissionPolicy continues to answer WAFNonexistentItemException).
Traces: wafv2.permission-policy-invalid-document-rejected.

=== Cause 3 of 3: the Mobile-SDK operation family does not apply its entitlement gate consistently ===
AWS: Smithy models ListMobileSdkReleases (input Platform, required enum; errors WAFInternalErrorException 500, WAFInvalidOperationException 400, WAFInvalidParameterException 400), GenerateMobileSdkReleaseUrl (input Platform + ReleaseVersion, required; output Url; errors additionally WAFNonexistentItemException 400) and GetMobileSdkRelease (same input shape; output MobileSdkRelease; same error set). The Mobile-SDK feature is entitled per account: on an account not authorized for it, ListMobileSdkReleases itself is refused with HTTP 400 / WAFInvalidOperationException, and — this is the invariant under test — GenerateMobileSdkReleaseUrl and GetMobileSdkRelease are refused with the SAME error rather than fabricating a download URL or release metadata for an account that was never allowed to see the release catalog. Recorded behavior only for the gate's message, example "This account is not authorized to use Mobile-SDK"; the requirement is the status/code and the family-wide consistency, not the text. On an entitled account the alternative path requires Generate to return an https URL for a listed release and GetMobileSdkRelease's Timestamp/ReleaseVersion to agree with the ListMobileSdkReleases summary for that release.
Emulator: ListMobileSdkReleases (for both IOS and ANDROID) returns error code 'NotImplemented', so neither branch of the state machine can be reached. Replay: "AssertionError: expected ListMobileSdkReleases to reject with WAFInvalidOperationException, got 'NotImplemented'". Note the check's positive control, ListWebACLs(Scope=REGIONAL, Limit=1), succeeded, so this is specific to the Mobile-SDK family and not a blanket connectivity or credential failure.
Why: a client cannot distinguish "this account is not entitled to Mobile-SDK" (a permanent, non-retryable WAFInvalidOperationException) from an unmodeled failure, and there is no guarantee the three operations agree — the dangerous divergence being a URL or release record handed back for an unentitled account.
Fix: the three Mobile-SDK handlers (ListMobileSdkReleases, GenerateMobileSdkReleaseUrl, GetMobileSdkRelease) and the shared entitlement/release-catalog state they consult, so one gate decides all three and either all reject with 400/WAFInvalidOperationException or all serve a consistent catalog.
Traces: wafv2.mobile-sdk-release-url-generation-lifecycle.

## Evidence
Failing checks from suite simparity-behavioral-ga-prod-v1 (service wafv2, 36/94 passed), none marked ALREADY FILED or OWNED:

1) [mismatch] wafv2.tag-resource-reserved-aws-prefix-rejected
   check_wafv2_tag_resource_reserved_aws_prefix_rejected.py line 46
   assert code == "WAFTagOperationException"
   AssertionError: expected WAFTagOperationException, got NotImplemented
   Expected: HTTP 400, error code WAFTagOperationException for Tags=[{Key: 'aws:Reserved'}]; tag absent afterwards; non-reserved key accepted.
   Actual:   error code 'NotImplemented' (not in TagResource's modeled error set).

2) [mismatch] wafv2.permission-policy-invalid-document-rejected
   check_wafv2_permission_policy_invalid_document_rejected.py line 113
   assert code == "WAFInvalidPermissionPolicyException"
   AssertionError: NotImplemented
   Expected: HTTP 400, error code WAFInvalidPermissionPolicyException for Policy='{not valid json'; then GetPermissionPolicy -> 400 WAFNonexistentItemException.
   Actual:   error code 'NotImplemented' (not in PutPermissionPolicy's modeled error set: WAFInternalErrorException, WAFInvalidParameterException, WAFInvalidPermissionPolicyException, WAFNonexistentItemException).

3) [mismatch] wafv2.mobile-sdk-release-url-generation-lifecycle
   check_wafv2_mobile_sdk_release_url_generation_lifecycle.py line 65
   assert list_error == "WAFInvalidOperationException"
   AssertionError: expected ListMobileSdkReleases to reject with WAFInvalidOperationException, got 'NotImplemented'
   Expected: HTTP 400, error code WAFInvalidOperationException from ListMobileSdkReleases and the SAME code from GenerateMobileSdkReleaseUrl and GetMobileSdkRelease (or, when entitled, a consistent list -> generate -> get sequence).
   Actual:   error code 'NotImplemented'; the positive control ListWebACLs(Scope=REGIONAL, Limit=1) succeeded in the same check, so the failure is scoped to the Mobile-SDK family.

Model source cited: services/service-com-amazonaws-wafv2-awswaf-20190729/smithy-model/com-amazonaws-wafv2-awswaf-20190729.json (operation shapes and error sets for TagResource, PutPermissionPolicy, ListMobileSdkReleases, GenerateMobileSdkReleaseUrl, GetMobileSdkRelease).

Related already-filed stub issues (implementation-existence, not these rejection semantics): 20261024T065754Z-wafv2-tagresource-stub.md, 20261024T065754Z-wafv2-putpermissionpolicy-stub.md, 20261024T065754Z-wafv2-listmobilesdkreleases-stub.md.

## Steps to Reproduce
Boot the emulator and point a botocore wafv2 client (Scope=REGIONAL) at it.

Cause 1 — TagResource reserved prefix:
1. create_ip_set(Name=<unique>, Scope='REGIONAL', IPAddressVersion='IPV4', Addresses=['203.0.113.0/24']) and take Summary.ARN.
2. tag_resource(ResourceARN=<arn>, Tags=[{'Key': 'aws:Reserved', 'Value': 'x'}]).
   Expect: ClientError, HTTP 400, Error.Code == 'WAFTagOperationException'. Observed: Error.Code == 'NotImplemented'.
3. list_tags_for_resource(ResourceARN=<arn>) must contain no 'aws:Reserved' key; tag_resource with {'Key':'Team','Value':'sim2real'} must succeed and appear on read.

Cause 2 — PutPermissionPolicy malformed document:
1. create_rule_group(Name=<unique>, Scope='REGIONAL', Capacity=10, Rules=[], VisibilityConfig={...}) and take Summary.ARN.
2. put_permission_policy(ResourceArn=<arn>, Policy='{not valid json').
   Expect: ClientError, HTTP 400, Error.Code == 'WAFInvalidPermissionPolicyException'. Observed: Error.Code == 'NotImplemented'.
3. get_permission_policy(ResourceArn=<arn>) must still raise HTTP 400 / 'WAFNonexistentItemException' (nothing attached).

Cause 3 — Mobile-SDK entitlement gate:
1. list_web_acls(Scope='REGIONAL', Limit=1) as a positive control — succeeds today.
2. list_mobile_sdk_releases(Platform='IOS') (and 'ANDROID').
   Expect: ClientError, HTTP 400, Error.Code == 'WAFInvalidOperationException' on an unentitled account. Observed: Error.Code == 'NotImplemented'.
3. generate_mobile_sdk_release_url(Platform='IOS', ReleaseVersion='1.0.0') and get_mobile_sdk_release(Platform='IOS', ReleaseVersion='1.0.0') must be refused with the SAME code — never returning a Url or MobileSdkRelease.

Re-replay: wafv2.tag-resource-reserved-aws-prefix-rejected, wafv2.permission-policy-invalid-document-rejected, wafv2.mobile-sdk-release-url-generation-lifecycle.
