Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-response-headers-policy-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-response-headers-policy-lifecycle cfdeep-policy-inuse-conflicts cloudfront-response-headers-policy-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ContentSecurityPolicy
Issue-Key: cloudfront|CreateResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ContentTypeOptions
Issue-Key: cloudfront|CreateResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.FrameOptions
Issue-Key: cloudfront|CreateResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ReferrerPolicy
Issue-Key: cloudfront|CreateResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.XSSProtection
Issue-Key: cloudfront|GetResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ContentSecurityPolicy
Issue-Key: cloudfront|GetResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.FrameOptions
Issue-Key: cloudfront|GetResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ReferrerPolicy
Issue-Key: cloudfront|GetResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.StrictTransportSecurity
Issue-Key: cloudfront|GetResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.XSSProtection
Issue-Key: cloudfront|CreateResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.StrictTransportSecurity
Issue-Key: cloudfront|GetResponseHeadersPolicy|2xx->2xx|$.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ContentTypeOptions
Issue-Key: cloudfront|GetResponseHeadersPolicyConfig|2xx->2xx|$.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ContentSecurityPolicy
Issue-Key: cloudfront|GetResponseHeadersPolicyConfig|2xx->2xx|$.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ContentTypeOptions
Issue-Key: cloudfront|GetResponseHeadersPolicyConfig|2xx->2xx|$.ResponseHeadersPolicyConfig.SecurityHeadersConfig.FrameOptions
Issue-Key: cloudfront|GetResponseHeadersPolicyConfig|2xx->2xx|$.ResponseHeadersPolicyConfig.SecurityHeadersConfig.ReferrerPolicy
Issue-Key: cloudfront|GetResponseHeadersPolicyConfig|2xx->2xx|$.ResponseHeadersPolicyConfig.SecurityHeadersConfig.XSSProtection
Issue-Key: cloudfront|ListResponseHeadersPolicies|2xx->2xx|$.ResponseHeadersPolicyList.Items.ResponseHeadersPolicySummary[*]
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront ResponseHeadersPolicy omits unset SecurityHeadersConfig members from every response

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResponseHeadersPolicy

## Description
Six causes, one shared fault: AWS materialises every member of ResponseHeadersPolicySecurityHeadersConfig in the serialized policy - the ones the request did not set come back as empty elements - while the emulator emits only the members that were set. The defect shows up identically on the create, two get, the config-only get and the list paths, so it is one serializer.

AWS (all six): SecurityHeadersConfig is returned with XSSProtection, FrameOptions, ReferrerPolicy, ContentSecurityPolicy, ContentTypeOptions and StrictTransportSecurity all present; unset ones are empty elements (Smithy: ResponseHeadersPolicySecurityHeadersConfig has exactly these six optional members; recorded responses carry all six).
Emulator (all six): only the explicitly-configured members appear; the rest are absent from the XML.
Why: a client that reads SecurityHeadersConfig to decide which security headers are configured cannot distinguish "not configured" from "member not serialized", and a GetResponseHeadersPolicyConfig -> UpdateResponseHeadersPolicy round trip silently changes the document shape.
Fix (all six): the ResponseHeadersPolicySecurityHeadersConfig serializer, shared by the create/get/get-config/list paths for response headers policies.

Cause 1 of 6: CreateResponseHeadersPolicy - XSSProtection, FrameOptions, ReferrerPolicy, ContentSecurityPolicy and ContentTypeOptions absent. Traces cloudfront-response-headers-policy-lifecycle#task#1.
Cause 2 of 6: GetResponseHeadersPolicy - XSSProtection, FrameOptions, ReferrerPolicy, ContentSecurityPolicy and StrictTransportSecurity absent. Traces cfdeep-policy-inuse-conflicts#task#11.
Cause 3 of 6: CreateResponseHeadersPolicy (errors fixture) - XSSProtection, ReferrerPolicy, ContentSecurityPolicy and StrictTransportSecurity absent. Traces cloudfront-response-headers-policy-errors#task#1.
Cause 4 of 6: GetResponseHeadersPolicy - the same five members absent as cause 1. Traces cloudfront-response-headers-policy-lifecycle#task#3.
Cause 5 of 6: GetResponseHeadersPolicyConfig - the same five members absent, confirming the config-only projection shares the defect. Traces cloudfront-response-headers-policy-lifecycle#task#4.
Cause 6 of 6: ListResponseHeadersPolicies - the summary's embedded SecurityHeadersConfig carries only StrictTransportSecurity, so the listed policy differs from AWS's listed policy in exactly the same way. Traces cloudfront-response-headers-policy-lifecycle#task#5.

## Evidence
All six clusters match on status (201/200) and diverge only on absent SecurityHeadersConfig members.

1. CreateResponseHeadersPolicy: $.ResponseHeadersPolicy.ResponseHeadersPolicyConfig.SecurityHeadersConfig.{XSSProtection,FrameOptions,ReferrerPolicy,ContentSecurityPolicy,ContentTypeOptions} each 'missing (expected '', got None)'.
2. GetResponseHeadersPolicy: {XSSProtection,FrameOptions,ReferrerPolicy,ContentSecurityPolicy,StrictTransportSecurity} missing.
3. CreateResponseHeadersPolicy: {XSSProtection,ReferrerPolicy,ContentSecurityPolicy,StrictTransportSecurity} missing.
4. GetResponseHeadersPolicy: {XSSProtection,FrameOptions,ReferrerPolicy,ContentSecurityPolicy,ContentTypeOptions} missing.
5. GetResponseHeadersPolicyConfig: $.ResponseHeadersPolicyConfig.SecurityHeadersConfig.{XSSProtection,FrameOptions,ReferrerPolicy,ContentSecurityPolicy,ContentTypeOptions} missing.
6. ListResponseHeadersPolicies: the summary's SecurityHeadersConfig was expected to contain XSSProtection/FrameOptions/ReferrerPolicy/ContentSecurityPolicy/ContentTypeOptions as empty elements alongside StrictTransportSecurity, and contained only StrictTransportSecurity.

Which subset is missing in each cause is just which members that fixture left unset; the requirement is that all six members are always serialized.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. CreateResponseHeadersPolicy with a SecurityHeadersConfig that sets only some of the six members (e.g. only StrictTransportSecurity, or only FrameOptions + ContentTypeOptions).
3. Read the create response, then GetResponseHeadersPolicy, GetResponseHeadersPolicyConfig and ListResponseHeadersPolicies.
4. In every response the unset members are absent from SecurityHeadersConfig instead of appearing as empty elements.
5. Representative step: fixture cloudfront-response-headers-policy-lifecycle, step task#1 (CreateResponseHeadersPolicy).
