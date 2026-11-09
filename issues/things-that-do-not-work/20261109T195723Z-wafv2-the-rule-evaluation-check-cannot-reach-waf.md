Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.wafv2-rule-evaluation-allows-blocks
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.wafv2-rule-evaluation-allows-blocks|allowed_status#31|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: 2f19ef1c319cb432a408a2dffd91968e32ad629f

# wafv2: the rule-evaluation check cannot reach WAF at the edge — the load balancer the WebACL is associated with publishes an elb.amazonaws.com DNSName the emulator does not serve

> _Note: this issue was identified against the code base at commit `2f19ef1c319cb432a408a2dffd91968e32ad629f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
One net-new root cause, grounded in a failing behavioral check of the wafv2 service. Two open issues name the same defect for other checks and do NOT cover this one: 20261109T153103Z-elbv2-wafv2-the-alb-s-published-dnsname-is.md (check elbv2__wafv2.associated-web-acl-blocks-the-request-before-the-alb-reaches-the-target) and 20261109T143515Z-elbv2-logs-wafv2-the-waf-protected-alb-s-endpoint-host-is.md (check elbv2__logs__wafv2.log-edge-producers-...). The check below is reported under the wafv2 service and carries its own replay key, so it is filed here; a fixer closing either of those should re-replay this check too.

Cause 1 of 1: the address published for an application load balancer is a real-AWS hostname rather than an address the emulator answers on, so no request can be driven through an associated WebACL.
AWS: CreateLoadBalancer returns LoadBalancers[0].DNSName, the name a client resolves and connects to in order to reach that load balancer; after AssociateWebACL binds a REGIONAL web ACL to the load balancer's ARN, a plain GET of http://<DNSName>/ is answered 200 by the listener's fixed-response action, and a GET carrying a header matched by a Block rule is answered 403 by WAF before the listener is reached (Smithy: wafv2 AssociateWebACL takes WebACLArn and ResourceArn, both @length 20..2048 @pattern \S; the 403-at-the-edge behavior and the fixed-response 200 are recorded behavior only — the recording shows both statuses being observed through the published DNSName). The requirement is that the published address is one a customer of the API can actually reach; the host and port themselves are environment values and are never part of the requirement.
Emulator: the DNSName handed back is of the form `<lb-name>-<id>.<region>.elb.amazonaws.com` — a real-AWS host the emulator does not serve and the caller cannot reach. The harness refuses the egress rather than leaving the network: `simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-wafalb-068bdd56-6885066892.us-west-2.elb.amazonaws.com/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<emulator host>:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. Everything up to that point succeeds: the web ACL, the load balancer, the listener, the active state and AssociateWebACL are all accepted. The specific host/port strings above are per-run values.
Why: WAFv2 rule evaluation is unobservable for any load-balancer-protected resource. A client that associates a web ACL and then sends traffic to the address the API published is sent to real AWS (or nowhere), so it never sees the 200 for an allowed request nor the 403 WAF returns for a blocked one, and cannot verify that its rules take effect at all.
Fix: the address-publishing path for the load-balancer resource — the DNSName the CreateLoadBalancer/DescribeLoadBalancers handler stores and serializes must be an address the emulator serves (its own host/port, or a per-resource listener), and the WAF data-plane evaluation must be reached on that address so the associated WebACL's decision is applied before the listener's default action.
Traces: wafv2.wafv2-rule-evaluation-allows-blocks.

## Evidence
Check wafv2.wafv2-rule-evaluation-allows-blocks, verdict [mismatch]:
  File "context.py", line 572, in endpoint_route
    return self._published_target(url, "endpoint_url"), {}
  File "context.py", line 586, in _published_target
    raise PublishedUrlRejected(...)
  simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-wafalb-068bdd56-6885066892.us-west-2.elb.amazonaws.com/ (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:38591/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

Expected: CreateLoadBalancer publishes a DNSName the emulator serves; GET http://<DNSName>/ returns 200 from the listener's fixed-response action, GET with the header matched by the WebACL's Block rule returns 403, and the allowed request still returns 200 afterwards.
Actual: the published DNSName is an elb.amazonaws.com host, so the request is never sent to the emulator and no allow/block verdict is ever observed. The load-balancer name, the generated id, the region, and the emulator's own host and port are per-run values and are not part of the requirement.

## Steps to Reproduce
1. CreateWebACL (Scope REGIONAL, DefaultAction Allow) with one Priority-0 Block rule matching a request header (e.g. a ByteMatchStatement on header x-waf-block).
2. CreateLoadBalancer for an internet-facing application load balancer and note LoadBalancers[0].DNSName; CreateListener on port 80 with a fixed-response 200 default action; poll DescribeLoadBalancers until State.Code == "active".
3. AssociateWebACL with the web ACL's ARN and the load balancer's ARN (accepted).
4. GET http://<DNSName>/ — the published DNSName is `<name>-<id>.<region>.elb.amazonaws.com`, a host the emulator does not serve, so the request cannot be delivered; AWS answers 200 here and 403 for the same GET carrying the blocked header.
5. Or replay check wafv2.wafv2-rule-evaluation-allows-blocks.
