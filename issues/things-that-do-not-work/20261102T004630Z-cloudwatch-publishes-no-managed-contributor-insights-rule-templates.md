Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item cloudwatch.put-managed-insight-rules-invalid-template-rejected cloudwatch.managed-insight-rule-report-reflects-applied-template
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item|run#4|AssertionError|assert offered, f"ListManagedInsightRules offered no template for {resource_arn!r}"
Issue-Key: cloudwatch.put-managed-insight-rules-invalid-template-rejected|run#5|AssertionError|raise AssertionError(
Issue-Key: cloudwatch.managed-insight-rule-report-reflects-applied-template|run#5|AssertionError|raise AssertionError(
Issue-Commit: 128acfbf958cf80ba0f1601c0fdbd38f5251e569

# CloudWatch publishes no managed Contributor Insights rule templates: ListManagedInsightRules returns an empty ManagedRules for a VPC PrivateLink endpoint service, so no managed rule can ever be enabled

> _Note: this issue was identified against the code base at commit `128acfbf958cf80ba0f1601c0fdbd38f5251e569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListManagedInsightRules

## Description
One net-new root cause in the CloudWatch emulator, grounded in 3 failing behavioral checks.

Cause 1 of 1: the service-side managed-rule template catalog is permanently empty, so `ListManagedInsightRules` reports no available managed rule for ANY resource ARN — including a VPC PrivateLink endpoint service, the resource type that is the documented carrier of managed Contributor Insights rules. Every managed-rule workflow therefore dead-ends on its very first call, and `PutManagedInsightRules` can never be exercised with a real template name.

AWS: `ListManagedInsightRules` takes a required `ResourceARN` (Smithy: `ListManagedInsightRulesInput$ResourceARN`, target `AmazonResourceName`, @length 1..1024) and answers 200 with `ManagedRules` — documented as "The managed rules that are available for the specified Amazon Web Services resource." (`ListManagedInsightRulesOutput$ManagedRules` -> `ManagedRuleDescriptions` of `ManagedRuleDescription`). Each `ManagedRuleDescription` carries `TemplateName` (@length 1..128, @pattern `^[0-9A-Za-z][\-\.\_0-9A-Za-z]{0,126}[0-9A-Za-z]$`), described as "The template name for the managed rule. Used to enable managed rules using PutManagedInsightRules"; `ResourceARN`, present only "If a managed rule is enabled"; and an optional `RuleState` (`ManagedRuleState{RuleName, State}`) that is "present" only once a rule has been enabled from that template. The modeled errors are only `InvalidNextToken`, `InvalidParameterValueException` and `MissingRequiredParameterException` (all HTTP 400) — there is no modeled way to say "this resource type has no templates", because the service answers 200 with the catalog it publishes for the resource. Recorded behavior: for a freshly provisioned VPC PrivateLink endpoint service (`arn:<partition>:ec2:<region>:<account>:vpc-endpoint-service/<vpce-svc-id>`), real CloudWatch answers 200 with a NON-EMPTY `ManagedRules`, every entry carrying a `TemplateName` and NO `RuleState` (nothing enabled yet); the first of those template names is then accepted by `PutManagedInsightRules` with an empty `Failures` list, and the resulting rule appears in `ListManagedInsightRules` with `RuleState.State == "ENABLED"` and in `DescribeInsightRules` with `ManagedRule == true`. The specific template names CloudWatch publishes are not stated in the Smithy model — recorded behavior only; the requirement is that at least one template is published for a VPC endpoint service resource, with a `TemplateName` matching the modeled @pattern.

Emulator: `ListManagedInsightRules` returns HTTP 200 with `ManagedRules: []` for the endpoint-service ARN. All three checks abort on that empty list: `AssertionError: ListManagedInsightRules offered no template for 'arn:aws:ec2:us-west-2:000000000000:vpc-endpoint-service/vpce-svc-00000000000000039'`, `AssertionError: no managed Contributor Insights rule templates are available for VPC endpoint services in this account/region`, and `AssertionError: no managed Contributor Insights rule templates are available for a VPC PrivateLink endpoint service in this account/region, so the positive control cannot be established`. The operation handler itself is implemented (paging, ARN validation, NextToken, ManagedRuleDescription projection are all there) — it is the catalog it reads that is empty. In `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state.rs` the field `managed_rule_templates` is documented as starting EMPTY, and the only writer is `publish_managed_rule_template`, which no code path in the service ever calls; `managed_rules_available_for(resource_arn)` consequently returns only templates that an already-enabled rule back-references, i.e. nothing on a fresh resource.

Why: a client that follows the documented discover-then-enable flow (`ListManagedInsightRules` -> pick a `TemplateName` -> `PutManagedInsightRules`) can never get off the ground. It sees a supported PrivateLink resource as having no managed Contributor Insights rules at all, so managed rules are untestable and un-enableable against the emulator; and because every template name a client might hard-code is answered as a per-item `ResourceNotFoundException`/`TEMPLATE_NAME` partial failure rather than succeeding, the client concludes the resource type is unsupported in this account/region instead of observing the real behavior. It also makes the downstream contract unreachable and silently unverified: the `ENABLED` `RuleState` in `ListManagedInsightRules`, the `ManagedRule: true` / `Definition.Resource == <resource ARN>` entry in `DescribeInsightRules`, and the `Contributors` + `MetricDatapoints` schema from `GetInsightRuleReport` for a managed rule.

Fix: the CloudWatch service's managed-rule template catalog in its state — `managed_rule_templates` in `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state.rs` must be seeded (through the existing `publish_managed_rule_template` seam) with the template(s) CloudWatch publishes for the `vpc-endpoint-service` EC2 resource type, so that `managed_rules_available_for` returns at least one entry for such an ARN. `ListManagedInsightRules`'s own handler (`src/ops/list_managed_insight_rules.rs`) needs no change to its paging or projection; `PutManagedInsightRules` (`src/ops/put_managed_insight_rules.rs`) must then accept a published template and mint an ENABLED managed rule bound to the resource, while continuing to report an unknown or pattern-breaking `TemplateName` as a per-item `PartialFailure` (`ExceptionType: ResourceNotFoundException`, `FailureCode: TEMPLATE_NAME`, `FailureDescription: "Could not find template with the specified templateName."`, `FailureResource` = the offending template name) rather than as a top-level error.

Traces: cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item, cloudwatch.put-managed-insight-rules-invalid-template-rejected, cloudwatch.managed-insight-rule-report-reflects-applied-template.

## Evidence
All three checks provision a VPC + subnet + internal NLB + VPC endpoint service configuration, then call ListManagedInsightRules with the endpoint-service ARN.

Expected (AWS): HTTP 200 with a non-empty ManagedRules list, e.g.
  {"ManagedRules": [{"TemplateName": "<published-template>"}, ...]}
where each entry's TemplateName matches ^[0-9A-Za-z][\-\.\_0-9A-Za-z]{0,126}[0-9A-Za-z]$ and no entry carries RuleState on a fresh resource. PutManagedInsightRules with that TemplateName then returns Failures == [].

Actual (emulator): HTTP 200 with {"ManagedRules": []}.

Failing assertions, verbatim from the replay:
- cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item (check_cloudwatch_put_managed_insight_rules_invalid_template_name_rejected_per_item.py:39)
  AssertionError: ListManagedInsightRules offered no template for 'arn:aws:ec2:us-west-2:000000000000:vpc-endpoint-service/vpce-svc-00000000000000039'
  (the ARN is this run's generated id; the requirement is that ANY valid vpc-endpoint-service ARN yields at least one template)
- cloudwatch.put-managed-insight-rules-invalid-template-rejected (check_cloudwatch_put_managed_insight_rules_invalid_template_rejected.py:27)
  AssertionError: no managed Contributor Insights rule templates are available for a VPC PrivateLink endpoint service in this account/region, so the positive control cannot be established
- cloudwatch.managed-insight-rule-report-reflects-applied-template (check_cloudwatch_managed_insight_rule_report_reflects_applied_template.py:43)
  AssertionError: no managed Contributor Insights rule templates are available for VPC endpoint services in this account/region

Source evidence in the checkout:
- services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state.rs:1848-1864 — `managed_rule_templates` is documented as starting EMPTY ("An empty catalog therefore says that THIS emulator publishes no managed-rule template").
- services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state.rs:3614 — `publish_managed_rule_template`, the only writer of that catalog, is never called by any service code path.
- services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state.rs:3658 — `managed_rules_available_for` derives entries only from the (empty) catalog plus already-enabled rules, so a fresh resource yields [].
- services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/list_managed_insight_rules.rs:208 — the handler reads that catalog and faithfully serializes an empty page.

Smithy citations used: ListManagedInsightRules input ResourceARN (required, @length 1..1024), output ManagedRules -> ManagedRuleDescriptions; ManagedRuleDescription.{TemplateName @length 1..128 @pattern, ResourceARN, RuleState}; ManagedRuleState.{RuleName, State} both required; ManagedRule.{TemplateName, ResourceARN} required; PutManagedInsightRules output Failures -> BatchFailures of PartialFailure{FailureResource, ExceptionType, FailureCode, FailureDescription}; modeled errors InvalidNextToken / InvalidParameterValueException / MissingRequiredParameterException, all HTTP 400.

## Steps to Reproduce
1. Provision a VPC PrivateLink endpoint service against the emulator: ec2 CreateVpc -> CreateSubnet -> elbv2 CreateLoadBalancer (Type=network, Scheme=internal) -> wait active -> ec2 CreateVpcEndpointServiceConfiguration (AcceptanceRequired=false) -> wait ServiceState=Available.
2. Build the resource ARN arn:<partition>:ec2:<region>:<account>:vpc-endpoint-service/<ServiceId>.
3. Call cloudwatch ListManagedInsightRules with that ResourceARN.
   Observed: 200 with ManagedRules == [].
   Expected: 200 with at least one ManagedRuleDescription carrying a TemplateName and no RuleState.
4. Take that TemplateName and call PutManagedInsightRules with ManagedRules=[{TemplateName: <it>, ResourceARN: <arn>}].
   Expected: Failures == [], then ListManagedInsightRules shows that template with RuleState.State == "ENABLED", DescribeInsightRules shows the rule with ManagedRule == true and Definition.Resource == the resource ARN, and GetInsightRuleReport(RuleName=<rule>, Period=300, Metrics=["Sum"]) returns both Contributors and MetricDatapoints lists.
5. Call PutManagedInsightRules with an unknown TemplateName ("NoSuchTemplate-v1") and a pattern-breaking one ("-bad-template-").
   Expected: one PartialFailure per bad item with FailureResource = the template name, ExceptionType "ResourceNotFoundException", FailureCode "TEMPLATE_NAME", FailureDescription "Could not find template with the specified templateName.", and no rule enabled for either.

Replay the three grounding checks to confirm: cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item, cloudwatch.put-managed-insight-rules-invalid-template-rejected, cloudwatch.managed-insight-rule-report-reflects-applied-template.
