Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-list-managed-cache-policies
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-list-managed-cache-policies cloudfront-list-managed-origin-request-policies cloudfront-list-response-headers-policies-managed-and-pagination cloudfront-origin-access-control-lifecycle cloudfront-origin-access-identity-lifecycle cloudfront-readonly-inventory-empty-account cloudfront-realtime-log-config-update-and-list-lifecycle
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetCachePolicy|2xx->2xx|$.CachePolicy.CachePolicyConfig.Comment
Issue-Key: cloudfront|GetCachePolicy|2xx->2xx|$.CachePolicy.CachePolicyConfig.MinTTL
Issue-Key: cloudfront|GetCachePolicy|2xx->2xx|$.CachePolicy.CachePolicyConfig.Name
Issue-Key: cloudfront|GetCachePolicy|2xx->2xx|$.CachePolicy.CachePolicyConfig.ParametersInCacheKeyAndForwardedToOrigin.EnableAcceptEncodingBrotli
Issue-Key: cloudfront|GetCachePolicy|2xx->2xx|$.CachePolicy.CachePolicyConfig.ParametersInCacheKeyAndForwardedToOrigin.HeadersConfig.HeaderBehavior
Issue-Key: cloudfront|GetCachePolicy|2xx->2xx|$.CachePolicy.CachePolicyConfig.ParametersInCacheKeyAndForwardedToOrigin.QueryStringsConfig.QueryStringBehavior
Issue-Key: cloudfront|ListOriginRequestPolicies|2xx->2xx|$.OriginRequestPolicyList.Items.OriginRequestPolicySummary[*]
Issue-Key: cloudfront|GetOriginRequestPolicy|2xx->2xx|$.OriginRequestPolicy.OriginRequestPolicyConfig.Comment
Issue-Key: cloudfront|GetOriginRequestPolicy|2xx->2xx|$.OriginRequestPolicy.OriginRequestPolicyConfig.HeadersConfig.Headers.Items.Name[*]
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.Comment
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlAllowHeaders.Items
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlAllowHeaders.Quantity
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlAllowMethods.Items
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlAllowMethods.Items.Method[*]
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlAllowMethods.Quantity
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlExposeHeaders
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.OriginOverride
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.Name
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ContentSecurityPolicy
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*].ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ContentTypeOptions.Override
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.Comment
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlAllowHeaders.Items
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlAllowHeaders.Quantity
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlAllowMethods.Items
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlAllowMethods.Quantity
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.AccessControlExposeHeaders
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.CorsConfig.OriginOverride
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.Name
Issue-Key: cloudfront|ListOriginAccessControls|2xx->2xx|$.OriginAccessControlList.MaxItems
Issue-Key: cloudfront|ListCloudFrontOriginAccessIdentities|2xx->2xx|$.CloudFrontOriginAccessIdentityList.MaxItems
Issue-Key: cloudfront|ListRealtimeLogConfigs|2xx->2xx|$.RealtimeLogConfigs.MaxItems
Issue-Key: cloudfront|ListRealtimeLogConfigs|2xx->2xx|$.RealtimeLogConfigs.Items
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront managed policy catalogue contents and list MaxItems defaults are wrong

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCachePolicy

## Description
Nine causes, one shared fault: the emulator's built-in, account-independent catalogue data is wrong. AWS-managed policies are bound to the wrong ids and carry the wrong contents, and the default MaxItems each List* reports does not match AWS's documented per-operation default.

Causes 1-5 (managed policy catalogue). AWS ships a fixed set of managed policies with stable ids, names, comments and bodies; these are the same in every account, so they are a requirement and not a per-run value.
Cause 1 of 9: GetCachePolicy for the Managed-CachingOptimized id returns a different policy. AWS: that id resolves to Name `Managed-CachingOptimized`, Comment `Policy with caching enabled. Supports Gzip and Brotli compression.`, MinTTL 1, EnableAcceptEncodingBrotli true, HeaderBehavior `none`, QueryStringBehavior `none`. Emulator: returns `Managed-Elemental-MediaPackage` ('Policy for an AWS Elemental MediaPackage origin') with MinTTL 0, Brotli false and whitelist header/query behaviour. Why: a client that looks up a managed policy by its documented id gets a completely different caching policy, so distributions built on managed ids behave wrongly. Fix: the managed cache-policy catalogue (id -> policy mapping) in state. Traces cloudfront-list-managed-cache-policies#task#2.
Cause 2 of 9: ListOriginRequestPolicies returns the managed policies in the wrong order / with the wrong first entry. AWS: the first managed summary is id acba4595-bd28-49b8-b9fe-13317c0390fa, `Managed-UserAgentRefererHeaders`. Emulator: 216adef6-5c7f-47e4-b989-5492eafa07d3, `Managed-AllViewer`. Fix: the managed origin-request-policy catalogue and its list ordering. Traces cloudfront-list-managed-origin-request-policies#task#1.
Cause 3 of 9: GetOriginRequestPolicy for Managed-UserAgentRefererHeaders has the wrong comment and header items. AWS: Comment `Policy to forward user-agent and referer headers to origin`, Headers.Items.Name = ['referer', 'user-agent'] (lower case, in that order). Emulator: Comment 'Policy to forward the User-Agent and Referer headers', Headers = ['User-Agent', 'Referer']. Fix: the managed origin-request-policy catalogue entry. Traces cloudfront-list-managed-origin-request-policies#task#2.
Cause 4 of 9: ListResponseHeadersPolicies returns managed response-headers policies with swapped identities and wrong CORS bodies. AWS: summary[0] is `Managed-SimpleCORS` ('Allows all origins for simple CORS requests') with AccessControlAllowHeaders.Quantity 0, AccessControlAllowMethods.Quantity 0, AccessControlExposeHeaders present with Quantity 0 and OriginOverride false; summary[1] is `Managed-CORS-With-Preflight`. Emulator: the two are swapped and the CORS bodies carry Header '*', Method 'ALL', no AccessControlExposeHeaders and OriginOverride true. Fix: the managed response-headers-policy catalogue. Traces cloudfront-list-response-headers-policies-managed-and-pagination#task#3.
Cause 5 of 9: the same swap/content defect on the paginated single-item page. Traces cloudfront-list-response-headers-policies-managed-and-pagination#task#4.

Causes 6-9 (List* default MaxItems). AWS echoes the operation's own documented default MaxItems when the request does not supply one (Smithy: MaxItems is an optional httpQuery integer, so the service supplies the default). The emulator reports a single hard-coded 100 for every list.
Cause 6 of 9: ListOriginAccessControls - AWS 200, MaxItems 100 is wrong; AWS reports 200. Emulator reports 100. Fix: the ListOriginAccessControls default. Traces cloudfront-origin-access-control-lifecycle#task#2.
Cause 7 of 9: ListCloudFrontOriginAccessIdentities - AWS reports MaxItems 200; emulator 100. Fix: that operation's default. Traces cloudfront-origin-access-identity-lifecycle#task#4.
Cause 8 of 9: ListRealtimeLogConfigs - AWS reports MaxItems 150; emulator 100. Fix: that operation's default. Traces cloudfront-readonly-inventory-empty-account#task#2 (also cloudfront-realtime-log-config-update-and-list-lifecycle#task#6).
Cause 9 of 9: ListRealtimeLogConfigs on an emptied account - AWS reports MaxItems 150 AND an empty `Items` element; the emulator reports 100 and omits Items entirely. Fix: that operation's default plus the real-time-log-config list serializer (emit an empty Items wrapper). Traces cloudfront-realtime-log-config-update-and-list-lifecycle#task#10.
Why (causes 6-9): a client that paginates using the echoed MaxItems computes the wrong page size, and a missing Items wrapper breaks strict parsers on empty pages.

## Evidence
1. GetCachePolicy 200->200: Name expected 'Managed-CachingOptimized' got 'Managed-Elemental-MediaPackage'; Comment expected 'Policy with caching enabled. Supports Gzip and Brotli compression.' got 'Policy for an AWS Elemental MediaPackage origin'; MinTTL expected '1' got '0'; EnableAcceptEncodingBrotli expected 'true' got 'false'; HeaderBehavior expected 'none' got 'whitelist'; QueryStringBehavior expected 'none' got 'whitelist'.
2. ListOriginRequestPolicies 200->200: summary[0] expected the managed policy acba4595-bd28-49b8-b9fe-13317c0390fa 'Managed-UserAgentRefererHeaders'; got 216adef6-5c7f-47e4-b989-5492eafa07d3 'Managed-AllViewer'.
3. GetOriginRequestPolicy 200->200: Comment expected 'Policy to forward user-agent and referer headers to origin' got 'Policy to forward the User-Agent and Referer headers'; Headers.Items.Name[0] expected 'referer' got 'User-Agent'; Name[1] expected 'user-agent' got 'Referer'.
4/5. ListResponseHeadersPolicies 200->200: summary[0] Name expected 'Managed-SimpleCORS' got 'Managed-CORS-With-Preflight'; AccessControlAllowHeaders.Quantity expected '0' got '1' with Items {Header: '*'}; AccessControlAllowMethods.Quantity expected '0' got '1' with Items {Method: 'ALL'}; AccessControlExposeHeaders missing (expected {Quantity: 0, Items: ''}); OriginOverride expected 'false' got 'true'; summary[1] correspondingly expected 'Managed-CORS-With-Preflight' and got the simple-CORS body.
6. ListOriginAccessControls: $.OriginAccessControlList.MaxItems expected '200', got '100'.
7. ListCloudFrontOriginAccessIdentities: MaxItems expected '200', got '100'.
8. ListRealtimeLogConfigs: $.RealtimeLogConfigs.MaxItems expected '150', got '100'.
9. ListRealtimeLogConfigs: MaxItems expected '150' got '100', and $.RealtimeLogConfigs.Items missing (expected an empty element).

The managed policy ids/names/comments quoted here are AWS's fixed catalogue values, identical in every account, not per-run data.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Causes 1-5: GetCachePolicy with the documented Managed-CachingOptimized id; ListOriginRequestPolicies and GetOriginRequestPolicy for Managed-UserAgentRefererHeaders; ListResponseHeadersPolicies with and without MaxItems=1.
3. Causes 6-9: call ListOriginAccessControls, ListCloudFrontOriginAccessIdentities and ListRealtimeLogConfigs with NO MaxItems query parameter and read the echoed MaxItems (and, on an empty account, whether Items is present).
4. Representative step: fixture cloudfront-list-managed-cache-policies, step task#2 (GetCachePolicy).
