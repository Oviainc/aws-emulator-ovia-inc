Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position
Issue-Key: apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position|_poll#17|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 595d3d5604be8cb7f050ded6602d61000805e4f4

# apigateway__wafv2: an IPSetReferenceStatement with IPSetForwardedIPConfig is not enforced at the API Gateway edge — a listed forwarded address is never blocked, so header-position matching is unverifiable

> _Note: this issue was identified against the code base at commit `595d3d5604be8cb7f050ded6602d61000805e4f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebACL

## Description
One net-new root cause, grounded in one failing behavioral check that the replay report marks neither ALREADY FILED nor OWNED. Cause count: 1.

=== Cause 1 of 1: a WebACL rule whose IPSetReferenceStatement carries IPSetForwardedIPConfig never blocks a request presenting a listed address in the configured forwarded header, so the edge stays permissive ===
AWS: Smithy (com.amazonaws.wafv2#CreateWebACL / UpdateWebACL) accepts a Rule whose Statement is an IPSetReferenceStatement{ARN, IPSetForwardedIPConfig{HeaderName, FallbackBehavior, Position}}; Position targets the ForwardedIPPosition enum (FIRST, LAST, ANY) and FallbackBehavior targets FallbackBehavior (MATCH, NO_MATCH). AWS documents this statement as inspecting an address taken from the named forwarded header at the configured position rather than the connection's source address (https://docs.aws.amazon.com/waf/latest/developerguide/waf-rule-statement-forwarded-ip-address.html). Recorded behavior for a REGIONAL WebACL associated with an API Gateway stage: once the rule is live, a request whose X-Forwarded-For first element is an address in the referenced IPSet is answered by WAF with HTTP 403 at the edge and the origin integration is NOT run; an address that is not in the set is answered 200 with the origin's own body; a request with no X-Forwarded-For header at all is answered 200 because FallbackBehavior is NO_MATCH; with Position FIRST, the listed address appearing SECOND in the chain must NOT match and must be answered 200 with the origin's body. GetWebACL must round-trip IPSetForwardedIPConfig unchanged.
Emulator: the edge never blocks. After CreateIPSet(Addresses=['198.51.100.7/32']), CreateWebACL with the Block rule above and AssociateWebACL with the stage ARN all succeed, a GET carrying X-Forwarded-For: 198.51.100.7 is never answered 403 on four successive requests within the propagation budget — the check dies with TimeoutError("the listed forwarded address to be consistently blocked: the run budget ended first; not graded"). Because the blocking direction never engages, every later leg (unlisted address allowed, absent header allowed under NO_MATCH, FIRST-position match in a chain, SECOND-position non-match, and the GetWebACL round-trip of IPSetForwardedIPConfig) is ungraded. Note the check's own diagnostic distinguishes "nothing answered at the URL" (it would have raised a DATAPLANE_UNIMPLEMENTED assertion): statuses were observed, they were simply never 403.
Why: this is a fail-OPEN security divergence. A caller that blocks an attacker's forwarded source address through an IPSet reference is told the WebACL is created and associated, and the traffic it intended to block is served by the origin instead — the opposite of the configured intent, with no error to attribute it to. Equally, because the position semantics are never exercised, an implementation that scans the whole X-Forwarded-For chain (over-blocking, and spoofable by appending an address) would also go undetected.
Fix: the WAF rule-evaluation path at the API Gateway edge — IPSetReferenceStatement must resolve the referenced IPSet's addresses and, when IPSetForwardedIPConfig is present, take the candidate address from the named header at the configured Position (FIRST/LAST/ANY) instead of the connection source, apply FallbackBehavior when the header is absent or unparsable, and apply the rule's Block action as an edge-generated HTTP 403 that does not run the integration. The stored IPSetForwardedIPConfig must round-trip through GetWebACL as written.
Related open issues that do NOT carry this check id and do not describe forwarded-IP position matching: 20261025T092822Z-apigateway-wafv2-six-wafv2-at-the-edge-checks.md and 20261029T041807Z-apigateway-wafv2-three-more-wafv2-at-the-edge.md.
Traces: apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position.

## Evidence
Replay report, suite simparity-behavioral-ga-prod-v1 (service group apigateway__wafv2: 0/8 passed).

  [error] apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position
  File "check_apigateway__wafv2_wafv2_forwarded_ip_set_reference_blocks_by_header_position.py", line 276, in _stable_http
    _poll(ctx, _once, lambda n: n >= consecutive, timeout_s=timeout_s, interval_s=interval_s,
  File "...", line 80, in _poll
    raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: the listed forwarded address to be consistently blocked: the run budget ended first; not graded

Setup, all accepted: apigateway CreateRestApi + a MOCK integration on /xff returning {"origin":"originmarker<token>"} + CreateDeployment/stage; a pre-flight GET of the stage URL polled to 200 with the origin marker present (so the origin was answering before WAF was attached); wafv2 CreateIPSet(Scope='REGIONAL', IPAddressVersion='IPV4', Addresses=['198.51.100.7/32']); CreateWebACL with Rules=[{'Name':'xffblocker','Priority':0,'Statement':{'IPSetReferenceStatement':{'ARN':<ipset arn>,'IPSetForwardedIPConfig':{'HeaderName':'X-Forwarded-For','FallbackBehavior':'NO_MATCH','Position':'FIRST'}}},'Action':{'Block':{}},'VisibilityConfig':{...}}]; AssociateWebACL(the apigateway stage ARN) polled until it stopped reporting the stage unavailable.

Expected (leg 1): GET <stage>/xff with header X-Forwarded-For: 198.51.100.7 answered HTTP 403 on four successive requests within the propagation budget. Actual: 403 is never observed on the required run of successive requests; the poll exhausts its budget.
Ungraded as a consequence (the legs behind leg 1): X-Forwarded-For: 203.0.113.9 -> three successive 200s carrying the origin marker; no X-Forwarded-For header -> three successive 200s (FallbackBehavior NO_MATCH); X-Forwarded-For: '198.51.100.7, 203.0.113.9' -> 403 (Position FIRST matches); X-Forwarded-For: '203.0.113.9, 198.51.100.7' -> three successive 200s carrying the origin marker (Position FIRST must NOT match a second-position address); GetWebACL's Rules[0].Statement.IPSetReferenceStatement.IPSetForwardedIPConfig == {'HeaderName':'X-Forwarded-For','FallbackBehavior':'NO_MATCH','Position':'FIRST'}.

## Steps to Reproduce
Boot the emulator and point botocore at it.
  1. apigateway CreateRestApi; CreateResource(pathPart='xff'); PutMethod(GET, authorizationType='NONE'); PutIntegration(type='MOCK', requestTemplates={'application/json':'{"statusCode": 200}'}); PutMethodResponse(200); PutIntegrationResponse(200, responseTemplates={'application/json':'{"origin":"originmarkerABCDEFGH"}'}); CreateDeployment(stageName=<stage>). Confirm GET <stage url>/xff returns 200 with the origin marker.
  2. wafv2 CreateIPSet(Name=<n>, Scope='REGIONAL', IPAddressVersion='IPV4', Addresses=['198.51.100.7/32']) -> the IPSet ARN.
  3. wafv2 CreateWebACL(Name=<n>, Scope='REGIONAL', DefaultAction={'Allow':{}}, Rules=[{'Name':'xffblocker','Priority':0,'Statement':{'IPSetReferenceStatement':{'ARN':<ipset arn>,'IPSetForwardedIPConfig':{'HeaderName':'X-Forwarded-For','FallbackBehavior':'NO_MATCH','Position':'FIRST'}}},'Action':{'Block':{}},'VisibilityConfig':{'SampledRequestsEnabled':True,'CloudWatchMetricsEnabled':True,'MetricName':'xffblocker'}}], VisibilityConfig={...}); AssociateWebACL(WebACLArn=<acl arn>, ResourceArn='arn:aws:apigateway:<region>::/restapis/<api>/stages/<stage>').
  4. GET <stage url>/xff with header 'X-Forwarded-For: 198.51.100.7', repeatedly.
     -> OBSERVE the status is never 403 on successive requests (the origin keeps answering 200 with its marker).
     -> EXPECT HTTP 403 generated at the edge, with the origin's body absent.
  5. Then confirm the discriminating legs: 'X-Forwarded-For: 203.0.113.9' -> 200 with the origin marker; no X-Forwarded-For header -> 200 (FallbackBehavior NO_MATCH); 'X-Forwarded-For: 198.51.100.7, 203.0.113.9' -> 403 (Position FIRST); 'X-Forwarded-For: 203.0.113.9, 198.51.100.7' -> 200 with the origin marker (Position FIRST must not scan the rest of the chain); GetWebACL -> Rules[0].Statement.IPSetReferenceStatement.IPSetForwardedIPConfig round-trips exactly as written.
  6. Re-replay the check id in trace_ids to confirm the fix.
