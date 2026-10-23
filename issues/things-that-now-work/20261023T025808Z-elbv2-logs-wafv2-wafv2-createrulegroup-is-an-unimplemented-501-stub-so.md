Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests
Issue-Key: elbv2__logs__wafv2|CreateRuleGroup|NotImplemented
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# WAFv2 CreateRuleGroup is an unimplemented 501 stub, so WAF logging of rule-group-blocked vs default-allowed requests cannot be exercised

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRuleGroup

## Description
One root cause, grounded in 1 failing behavioral check whose id's leading token is the composite 'elbv2__logs__wafv2'. No open issue names wafv2 CreateRuleGroup.

Cause 1 of 1: CreateRuleGroup is scaffolded but not implemented, so no custom rule group exists to attach to a web ACL and no WAF log record can distinguish a rule-group BLOCK from the web ACL's default ALLOW.
AWS: Smithy com.amazonaws.wafv2#CreateRuleGroup takes Name (required, @length 1..128, @pattern ^[\w\-]+$), Scope (required enum), Capacity (required long @range min 1), VisibilityConfig (required), and optional Description (@length 1..256), Rules, Tags (@length min 1), CustomResponseBodies; it returns {Summary: RuleGroupSummary} on success. Its modeled errors are WAFDuplicateItem, WAFInvalidParameter, WAFInvalidOperation, WAFLimitsExceeded, WAFNonexistentItem, WAFOptimisticLock, WAFSubscriptionNotFound, WAFTagOperation, WAFUnavailableEntity (HTTP 400) and WAFInternalError, WAFTagOperationInternalError (HTTP 500). There is no 'NotImplemented' in that set.
Emulator: the call is refused with Error.Code 'NotImplemented' and message 'operation CreateRuleGroup is scaffolded but not implemented in this emulator'.
Why: a client cannot create any custom WAF rule group, so it can neither attach one to an ALB-associated web ACL nor observe the WAF log records that prove a request was blocked by a rule group while another was allowed by the ACL default action — the entire WAF rule-group surface is unusable, and the refusal is a code no SDK models for this operation.
Fix: the CreateRuleGroup handler of the wafv2 service crate (services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/create_rule_group.rs) plus the rule-group state it must persist for GetRuleGroup/UpdateWebACL and for WAF log-record generation.
Traces: elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests.

## Evidence
elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateRuleGroup operation: operation CreateRuleGroup is scaffolded but not implemented in this emulator.
Expected per Smithy (com-amazonaws-wafv2-awswaf-20190729.json): HTTP 200 with {Summary: {Name, Id, Description, LockToken, ARN}}, or one of the modeled WAF* errors (HTTP 400/500). 'NotImplemented' is not a modeled error of this operation.

## Steps to Reproduce
1. Boot the emulator.
2. waf = boto3.client('wafv2'); waf.create_rule_group(Name='sim2real-rg', Scope='REGIONAL', Capacity=10, Rules=[...], VisibilityConfig={'SampledRequestsEnabled': True, 'CloudWatchMetricsEnabled': True, 'MetricName': 'sim2real'}).
3. Observe Error.Code 'NotImplemented' with message 'operation CreateRuleGroup is scaffolded but not implemented in this emulator' instead of 200 + Summary.
4. Re-replay elbv2__logs__wafv2.log-edge-producers-waf-log-records-rule-group-blocked-and-default-allowed-requests.
