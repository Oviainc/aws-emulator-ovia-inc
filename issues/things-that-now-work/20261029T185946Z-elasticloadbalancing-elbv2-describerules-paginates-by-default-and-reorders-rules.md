Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2env-listener-attributes-targets-and-rule-pagination
Issue-Step: task#17
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2env-listener-attributes-targets-and-rule-pagination elbv2rule-condition-combination-limits elbv2rule-condition-legacy-values-mirroring elbv2rule-redirect-component-pattern-validation elbv2rule-redirect-omitted-components-default-to-placeholders elbv2rule-redirect-self-loop-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.NextMarker
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Actions.member.FixedResponseConfig.MessageBody
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Actions.member.FixedResponseConfig.StatusCode
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Conditions
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].IsDefault
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Priority
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Actions.member.FixedResponseConfig
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Actions.member.RedirectConfig.Host
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Actions.member.RedirectConfig.Path
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Actions.member.RedirectConfig.Port
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Actions.member.RedirectConfig.Query
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Actions.member.RedirectConfig.StatusCode
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Actions.member.Type
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Conditions.member.PathPatternConfig.Values.member
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Conditions.member.Values.member
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].RuleArn
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member.Actions.member.RedirectConfig.Port
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member.Actions.member.RedirectConfig.Protocol
Issue-Related: 20261029T185946Z-elasticloadbalancing-elbv2-forward-actions-and-listener-mutual-authentication-omit.md
Issue-Related: 20261029T185946Z-elasticloadbalancing-elbv2-rule-responses-omit-transforms-and-the-fixedresponseconfig.md
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ELBv2 DescribeRules paginates by default and reorders rules, conditions and redirect config

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRules

## Description
DescribeRules diverges in how it pages, how it orders what it returns, and what it reports for rules created or modified through calls the emulator should have rejected. 7 causes.

Cause 1 of 7: DescribeRules with no PageSize returns one rule plus a NextMarker.
AWS: Smithy DescribeRules is paginated (inputToken Marker, outputToken NextMarker, items Rules) with PageSize range 1..400; with no PageSize AWS returns ALL of the listener's rules in one page and NO NextMarker.
Emulator: a single Rules.member and a NextMarker, so a client that does not paginate sees one of three rules.
Why: a non-paginating client silently loses rules — the most consequential form of this bug, since rule sets drive routing.
Fix: the DescribeRules handler's default page size.
Traces: elbv2env-listener-attributes-targets-and-rule-pagination#task#17.

Cause 2 of 7: the NextMarker token itself.
AWS: NextMarker is an opaque Marker string (the recording shape-checks it); it is not a page index.
Emulator: NextMarker '1' — a bare ordinal.
Why: clients that persist or validate the marker treat it as opaque; an ordinal also breaks as soon as the underlying list changes.
Fix: DescribeRules marker encoding. (This step also shows the Transforms and FixedResponseConfig.ContentType omissions filed separately.)
Traces: elbv2env-listener-attributes-targets-and-rule-pagination#task#18 (also #task#19, #task#20).

Cause 3 of 7: condition ordering within a rule.
AWS: echoes a rule's Conditions in the order they were supplied to CreateRule (recorded: host-header, http-header, path-pattern).
Emulator: returns them re-grouped (host-header, path-pattern, http-header).
Why: a client diffing a described rule against its desired config sees a change on every read and rewrites the rule.
Fix: preserve condition order in rule state.
Traces: elbv2rule-condition-combination-limits#task#12.

Cause 4 of 7: rule set contents after a CreateRule AWS rejects.
AWS: the listener holds six rules plus its default rule LAST, with Priority 'default', IsDefault 'true', Conditions empty and the listener's fixed-response action (StatusCode 404, MessageBody 'no route').
Emulator: an extra seventh rule exists (the source-ip + legacy Values rule AWS refused), and the rule the emulator reports in the default position carries Priority '16', IsDefault 'false' and a source-ip condition.
Fix: the CreateRule condition validation filed in the companion 'accepts invalid requests' issue; this step is its state check.
Traces: elbv2rule-condition-legacy-values-mirroring#task#14.

Cause 5 of 7: rule set after redirect-component validation.
AWS: rules with priorities 11 and 14, path patterns /b and /e, RedirectConfig.Path '/#{path}' and '/new/#{path}', Port 8443 — the malformed redirects (a Path without a leading slash, a Query without '?') were rejected, so they never entered the rule set.
Emulator: the set is shifted by the accepted-but-invalid rules (priorities 10 and 11, paths /a and /b) and echoes RedirectConfig.Path 'no-leading-slash'.
Fix: redirect-component pattern validation in CreateRule/ModifyRule; this step is its state check.
Traces: elbv2rule-redirect-component-pattern-validation#task#12.

Cause 6 of 7: redirect placeholders not resolved to the values the rule was created with.
AWS: the described rule carries RedirectConfig.Protocol 'HTTPS' and Port '443' — the concrete values, because the later ModifyRule that would have replaced them with '#{protocol}'/'#{port}' was rejected as a loop.
Emulator: Protocol '#{protocol}' and Port '#{port}'.
Fix: the ModifyRule redirect self-loop validation filed in the companion issue; this step is its state check.
Traces: elbv2rule-redirect-omitted-components-default-to-placeholders#task#13.

Cause 7 of 7: rule set after the self-loop redirect rejections.
AWS: one non-default rule (priority 12, path /moved, RedirectConfig.Host 'moved.example.com') followed by the default rule (Priority 'default', IsDefault 'true', Conditions empty, fixed-response 404 'no route').
Emulator: a rule at priority 10 with path /loop and Host '#{host}', then a non-default rule at priority 11 with a redirect action in the position where AWS reports the default rule.
Fix: the CreateRule redirect/condition validation filed in the companion issue; this step is its state check.
Traces: elbv2rule-redirect-self-loop-rejected#task#10.

## Evidence
Cause 1: $.DescribeRulesResponse.DescribeRulesResult.Rules.member expected a 3-element list with no NextMarker, got a single rule plus NextMarker.
Cause 2: NextMarker expected <shaped opaque token>, got '1'.
Cause 3: Rules.member[0].Conditions expected [host-header, http-header, path-pattern], got [host-header, path-pattern, http-header].
Cause 4: member[6] expected IsDefault 'true'/Priority 'default'/Conditions ''/FixedResponseConfig StatusCode 404 + MessageBody 'no route', got IsDefault 'false'/Priority '16'/source-ip condition/StatusCode 200; member[7] extra.
Cause 5: member[0] Priority expected '11' got '10'; path expected '/b' got '/a'; RedirectConfig.Path expected '/#{path}' got 'no-leading-slash'; Query expected '?a=b' got '#{query}'.
Cause 6: RedirectConfig.Protocol expected 'HTTPS' got '#{protocol}'; Port expected '443' got '#{port}'.
Cause 7: member[0] Priority expected '12' got '10', RedirectConfig.Host expected 'moved.example.com' got '#{host}'; member[1] expected the default rule, got a non-default redirect rule.

## Steps to Reproduce
Replay the six fixtures named under Traces and diff the listed DescribeRules steps against their recorded responses (replay_wire_corpus, trace set simparity-wire-aws-v1).
