Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule
Issue-Key: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule|run#36|AssertionError|assert len(parts) == 3 and parts[0] == MANAGED_VENDOR and parts[1] == MANAGED_GROUP, (
Issue-Commit: 032f9a5bcc1a20e2d342a3403d3d533abd316530

# wafv2 GetSampledRequests: RuleNameWithinRuleGroup reports the bare rule name for a managed-rule-group block instead of the documented <vendor>#<group>#<rule> form (1 cause)

> _Note: this issue was identified against the code base at commit `032f9a5bcc1a20e2d342a3403d3d533abd316530`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSampledRequests

## Description
Cause 1 of 1: when a request is terminated by a rule that lives INSIDE a managed rule group (a ManagedRuleGroupStatement rule with OverrideAction None), the sample GetSampledRequests returns names the terminating rule with its bare, group-local name only. It does not qualify the name with the vendor and the managed rule group, so a client cannot tell which rule group acted, and the name cannot be joined back to DescribeManagedRuleGroup output for that vendor/group.
Failing check: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule

AWS: SampledHTTPRequest.RuleNameWithinRuleGroup is modelled and documented as a qualified name. Smithy (services/service-com-amazonaws-wafv2-awswaf-20190729/smithy-model/com-amazonaws-wafv2-awswaf-20190729.json), SampledHTTPRequest.RuleNameWithinRuleGroup: "The name of the Rule that the request matched. For managed rule groups, the format for this name is <vendor name>#<managed rule group name>#<rule name>. For your own rule groups, the format for this name is <rule group name>#<rule name>. If the rule is not in a rule group, this field is absent." So the value is a template with three '#'-separated segments for a managed group: segment 1 is the VendorName from the ManagedRuleGroupStatement in the request that created/updated the web ACL, segment 2 is that statement's Name (the managed rule group name), segment 3 is the group-local rule name, which must be one of the names DescribeManagedRuleGroup(VendorName, Name, Scope) declares under Rules[].Name. None of those three segments is a per-run value: they come from the web ACL's own rule statement and from the managed group's declaration. GetSampledRequests itself is modelled with input WebAclArn/RuleMetricName/Scope/TimeWindow/MaxItems (all required) and output SampledRequests/PopulationSize/TimeWindow; errors WAFInternalErrorException (server, 500), WAFInvalidParameterException and WAFNonexistentItemException (client, 400). The matching/termination behaviour of an AWS-vendor SQLi group at an API Gateway edge is recorded behavior only; what is structural is the NAME FORMAT the model documents.

Emulator: the sample is otherwise well formed — PopulationSize > 0, Action 'BLOCK', Request.Method 'GET', the marker path present in Request.URI, an IPv4 Request.ClientIP and a Host header all passed — but RuleNameWithinRuleGroup carries only the group-local segment. Replay detail: AssertionError: the sample does not attribute the refusal to a rule inside AWSManagedRulesSQLiRuleSet in the documented <vendor>#<group>#<rule> form: 'SQLi_QUERYARGUMENTS'. Splitting that value on '#' yields one segment, not three; the 'AWS' vendor segment and the 'AWSManagedRulesSQLiRuleSet' group segment are both missing. (The rule-local segment itself is plausible; only the qualification is absent.)

Why: a client reading GetSampledRequests to attribute a 403 cannot determine WHICH rule group refused the request. With several managed rule groups in one web ACL, bare group-local rule names are ambiguous and can collide across vendors/groups, so dashboards, triage runbooks and false-positive workflows that parse <vendor>#<group>#<rule> (the documented contract) either mis-attribute the block or fail to parse the field at all. Any automation that feeds segment 3 back into DescribeManagedRuleGroup to decide an exclusion/override breaks, because it has no vendor or group to call with.

Fix: the GetSampledRequests response serializer / the sampled-request recorder in the wafv2 service — whatever records the terminating rule for a sample must record the rule's provenance (vendor + managed rule group name, or the customer rule-group name) alongside the group-local rule name and render RuleNameWithinRuleGroup as '<vendor>#<group>#<rule>' for a managed rule group and '<rule group name>#<rule name>' for a customer rule group, and omit the member entirely when the terminating rule is a top-level web-ACL rule.

Note: open issue 20261029T070050Z-wafv2-getsampledrequests-never-returns-a-sample-for-a.md covers GetSampledRequests returning NO sample at all for a matched rule; it does not cover the name format of a sample that IS returned, which is what fails here (a sample is returned and every other member of it is correct). Open issue 20261027T004450Z-wafv2-describemanagedrulegroup-publishes-no-availablelabels-for-aws-vendor.md covers the AvailableLabels assertion later in this same check, which was never reached.

Traces: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule (mismatch).

## Evidence
[mismatch] apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule
  File "check_apigateway__wafv2_wafv2_managed_sqli_rule_group_refuses_a_payload_and_the_sample_names_the_rule.py", line 376, in run
    assert len(parts) == 3 and parts[0] == MANAGED_VENDOR and parts[1] == MANAGED_GROUP, (
  AssertionError: the sample does not attribute the refusal to a rule inside AWSManagedRulesSQLiRuleSet in the documented <vendor>#<group>#<rule> form: 'SQLi_QUERYARGUMENTS'

Setup that reached this point successfully (so the divergence is isolated to the field's value): a REGIONAL web ACL with a single rule {Statement: {ManagedRuleGroupStatement: {VendorName: 'AWS', Name: 'AWSManagedRulesSQLiRuleSet'}}, OverrideAction: {None: {}}} associated with a deployed API Gateway REST stage; a SQL-injection tautology in the QUERY STRING was refused with HTTP 403 while a benign GET to the same path was served 200 by the MOCK integration.

Expected (AWS), from GetSampledRequests(WebAclArn=<acl arn>, RuleMetricName=<the managed-group rule's metric name>, Scope=REGIONAL, TimeWindow=<last 30 min>, MaxItems=100), for the sample whose Request.URI contains the run's marker path:
  SampledRequests[i].RuleNameWithinRuleGroup == 'AWS#AWSManagedRulesSQLiRuleSet#<rule name>' — three '#'-separated segments, where 'AWS' is the VendorName from the web ACL's ManagedRuleGroupStatement (fixed text for this request), 'AWSManagedRulesSQLiRuleSet' is that statement's Name (fixed text for this request), and <rule name> is a value taken from the group's own declaration, i.e. it must appear in DescribeManagedRuleGroup(VendorName='AWS', Name='AWSManagedRulesSQLiRuleSet', Scope='REGIONAL').Rules[].Name.
Actual (emulator):
  SampledRequests[i].RuleNameWithinRuleGroup == 'SQLi_QUERYARGUMENTS' — a single segment; no vendor segment, no managed-rule-group segment, no '#' separator.

All other members of the same sample matched AWS: Action 'BLOCK', Request.Method 'GET', Request.URI containing the marker path, Request.ClientIP an IPv4 address, a Host header present, PopulationSize > 0.

## Steps to Reproduce
1. apigateway CreateRestApi (REGIONAL) with a resource/method backed by a MOCK integration returning 200, and CreateDeployment to a stage; confirm GET <invoke_url><path>?q=<sqli tautology> returns 200 from the origin before WAF is attached.
2. wafv2 CreateWebACL(Scope=REGIONAL, DefaultAction={Allow:{}}) with one rule: Name='sqliset', Priority=0, Statement={ManagedRuleGroupStatement:{VendorName:'AWS', Name:'AWSManagedRulesSQLiRuleSet'}}, OverrideAction={None:{}}, VisibilityConfig={SampledRequestsEnabled:true, CloudWatchMetricsEnabled:true, MetricName:'sqliset'}.
3. wafv2 AssociateWebACL(WebACLArn=<acl arn>, ResourceArn='arn:aws:apigateway:<region>::/restapis/<api-id>/stages/<stage>'), retrying WAFUnavailableEntityException.
4. GET <invoke_url><path>?q=1'%20OR%20'1'%3D'1 until it is steadily refused with HTTP 403; confirm a benign GET to the same path still returns 200.
5. wafv2 GetSampledRequests(WebAclArn=<acl arn>, RuleMetricName='sqliset', Scope='REGIONAL', TimeWindow={StartTime: now-1800, EndTime: now}, MaxItems=100) and pick the sample whose Request.URI contains the path from step 4.
6. Read SampledRequests[i].RuleNameWithinRuleGroup: the emulator answers a bare group-local name such as 'SQLi_QUERYARGUMENTS', where AWS answers 'AWS#AWSManagedRulesSQLiRuleSet#<rule name>'. Split on '#' — the emulator yields 1 segment, AWS yields 3.
7. Cross-check that the third segment is joinable: wafv2 DescribeManagedRuleGroup(VendorName='AWS', Name='AWSManagedRulesSQLiRuleSet', Scope='REGIONAL').Rules[].Name must contain it.
8. Or replay apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule.
