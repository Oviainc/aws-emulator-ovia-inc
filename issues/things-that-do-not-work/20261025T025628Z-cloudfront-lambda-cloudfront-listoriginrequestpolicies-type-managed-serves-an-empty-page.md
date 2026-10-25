Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied
Issue-Key: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied|_managed_origin_request_policy_id#24|KeyError|for item in page["Items"]:
Issue-Commit: e0255819ca3e503154d6ac924edd987da058b9b5

# cloudfront: ListOriginRequestPolicies(Type="managed") serves an empty page — the AWS-managed origin-request-policy catalog is never seeded, so Managed-AllViewerExceptHostHeader cannot be resolved

> _Note: this issue was identified against the code base at commit `e0255819ca3e503154d6ac924edd987da058b9b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListOriginRequestPolicies

## Description
One net-new root cause in the CloudFront emulator, grounded in behavioral replay.

=== Cause 1 of 1: the AWS-managed origin-request-policy catalog does not exist in state, so ListOriginRequestPolicies(Type="managed") answers a page with Quantity 0 and no Items element at all ===

AWS: Smithy: ListOriginRequestPolicies, GET /2020-05-31/origin-request-policy, success code 200; input Type (@httpQuery "Type", enum OriginRequestPolicyType with values managed | custom), Marker (@httpQuery), MaxItems (@httpQuery); output OriginRequestPolicyList as the @httpPayload; errors AccessDenied 403, InvalidArgument 400, NoSuchOriginRequestPolicy 404. Shape OriginRequestPolicyList: MaxItems (@required), Quantity (@required, "The total number of origin request policies returned in the response"), Items (OriginRequestPolicySummaryList, "Contains the origin request policies in the list"), NextMarker. The operation's own model documentation states the filter semantics: "managed – Returns only the managed policies created by Amazon Web Services". The model does not enumerate which policies those are, so for the catalog's CONTENT the authority is AWS's documented behavior plus the recorded run: AWS pre-provisions a fixed set of managed origin request policies in every account (among them Managed-AllViewerExceptHostHeader, the policy CloudFront documents for a Lambda function URL origin, plus Managed-AllViewer, Managed-AllViewerAndCloudFrontHeaders-2022-06, Managed-CORS-CustomOrigin, Managed-CORS-S3Origin, Managed-UserAgentRefererHeaders), and a managed-filter listing therefore returns 200 with Quantity > 0 and an Items element whose summaries carry Type == "managed" and OriginRequestPolicy.OriginRequestPolicyConfig.Name set to those names. The ids are account-independent reference data the service reports, so a client is expected to resolve one BY NAME out of this listing rather than pin a literal.

Emulator: ListOriginRequestPolicies treats "managed" as a kind this account can never hold. The handler computes `serves_stored_policies = Type is none or Type == "custom"`, so for Type="managed" it skips the store scan entirely and returns OriginRequestPolicyList { max_items, quantity: 0, items: None, next_marker: None } — the Items element is omitted rather than empty. The replay's catalog walk reads page["Items"] on that response and raises KeyError: 'Items'; nothing in the managed catalog can be found by name, and no managed origin request policy id exists to be referenced by a distribution.

Why: managed origin request policy ids are the normal way a distribution's cache behavior is configured, and the only supported way to configure a Lambda function URL or OAC-signed origin (the viewer Host header must not be forwarded or the sigv4 signature the OAC computes will not verify). A client that enumerates the managed catalog to pick a policy gets an empty page and concludes the policy does not exist — the opposite of AWS, where it is guaranteed present in every account. Clients that defensively index Items (an element AWS always sends when the list is non-empty) fault outright rather than degrade, and infrastructure tooling that treats "absent" as "must be created" will try to create a policy it must not own. This is the origin-request-policy twin of the already-filed managed cache-policy gap in 20261025T020309Z-cloudfront-disassociatedistributiontenantwebacl-answers-notimplemented-and-the-aws-managed.md (Cause 2 there covers GetCachePolicy/ListCachePolicies/DeleteCachePolicy over the managed CACHE policy catalog only; it names neither this operation nor the origin-request-policy catalog, so this cause is filed separately).

Fix: the CloudFront origin-request-policy state must seed the AWS-managed catalog (ids, names and configs, classified Type="managed"), and the ListOriginRequestPolicies handler must resolve the managed filter against it instead of hard-coding TYPE_CUSTOM for every record and returning nothing for Type="managed" — see services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_origin_request_policies.rs lines 93-98 (TYPE_CUSTOM) and 224-259 (the serves_stored_policies filter), with the seeded records living in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/state.rs. GetOriginRequestPolicy and the distribution validation that resolves an OriginRequestPolicyId must resolve against the same catalog so a managed id referenced by a cache behavior is accepted.

Traces: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied

## Evidence
Cause 1 — check cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied (verdict: error):
  Expected (AWS): ListOriginRequestPolicies(Type="managed", MaxItems="100") -> HTTP 200 with OriginRequestPolicyList.Quantity > 0 and an Items element containing the AWS-managed policies; walking the pages finds a summary whose OriginRequestPolicy.OriginRequestPolicyConfig.Name == "Managed-AllViewerExceptHostHeader", whose OriginRequestPolicy.Id is then usable as the OriginRequestPolicyId of a cache behavior.
  Actual (emulator): the same call answers 200 with Quantity 0 and NO Items member, so the check's catalog walk fails with
    File "check_cloudfront__lambda_origin_access_control_signs_lambda_url_origin_and_direct_call_denied.py", line 130, in _managed_origin_request_policy_id
      for item in page["Items"]:
    KeyError: 'Items'
  raised from line 263: orp_id = _managed_origin_request_policy_id(cf, _ALL_VIEWER_EXCEPT_HOST).
  Source confirmation (not a patch, a citation): services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_origin_request_policies.rs:98 defines `const TYPE_CUSTOM: &str = "custom";` and documents at lines 10-13 that "a request asking only for the `managed` ones (the policies Amazon Web Services itself creates) lists none"; line 230-233 sets serves_stored_policies only when Type is absent or "custom"; line 283 omits Items when the page is empty.
  Controls that did pass in the same check: CreateOriginAccessControl with OriginAccessControlOriginType="lambda" and the GetOriginAccessControl read-back both succeeded, so the failure is specifically the absent managed origin-request-policy catalog, not a broken origin-request-policy surface.

## Steps to Reproduce
1. Call ListOriginRequestPolicies(Type="managed", MaxItems="100") against the emulator with no origin request policies created in the account.
   Expect (AWS): HTTP 200, OriginRequestPolicyList.Quantity > 0, an Items element present, and one summary with Type == "managed" and OriginRequestPolicyConfig.Name == "Managed-AllViewerExceptHostHeader".
   Observed: HTTP 200 with Quantity 0 and the Items member absent entirely.
2. Page the listing with the NextMarker/Marker loop while looking for the policy named "Managed-AllViewerExceptHostHeader" — indexing response["OriginRequestPolicyList"]["Items"] raises KeyError: 'Items' on the first page.
3. Call GetOriginRequestPolicy on any documented AWS-managed origin request policy id and expect HTTP 200 with the managed config.
4. Create a distribution whose DefaultCacheBehavior sets OriginRequestPolicyId to the managed id resolved in step 1/2 (the configuration CloudFront documents for a Lambda function URL / OAC-signed origin) and expect it to be accepted.
