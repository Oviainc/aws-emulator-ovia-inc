Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.deploy-resolver-rule-cfn-forwards-queries
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|GetResolverRuleAssociation|ResourceNotFoundException
Issue-Commit: 5f3c74c83654b73bfd2d2e3d0b1746f9df515a9b

# CloudFormation has no provisioner for AWS::Route53Resolver::ResolverRule and ::ResolverRuleAssociation, so the stack reports CREATE_COMPLETE and Ref yields a stack-derived name instead of a resolver rule/association id

> _Note: this issue was identified against the code base at commit `5f3c74c83654b73bfd2d2e3d0b1746f9df515a9b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause, surfacing in a route53resolver behavioral check (hence this issue's service token) but fixed in the CloudFormation crate's per-type provisioner.

Cause 1 of 1: the AWS::Route53Resolver::ResolverRule and AWS::Route53Resolver::ResolverRuleAssociation resource types have no provisioner arm, so a stack declaring them is left declared-only: no Resolver resources are created, the stack still reaches CREATE_COMPLETE, and each resource's physical id is a CloudFormation-generated '<stack>-<logical-id>-<suffix>' name rather than the id the Resolver service assigns.
AWS: AWS::Route53Resolver::ResolverRule and AWS::Route53Resolver::ResolverRuleAssociation are published CloudFormation resource types; Ref on them returns the resource's Resolver id (an rslvr-rr-* rule id and an rslvr-rrassoc-* association id), and the stack really creates them, so GetResolverRule / GetResolverRuleAssociation answer for those ids and the association converges to Status COMPLETE referencing the declared rule and VPC. Smithy: GetResolverRuleAssociation takes ResolverRuleAssociationId (ResourceId, @length 1..64) and models ResourceNotFoundException (HTTP 400) for an id that does not exist — so the emulator's 400 is a correct answer to the wrong id, and the defect is the id CloudFormation handed out. Recorded behavior only for the stack transition (the recording shows the stack reaching CREATE_COMPLETE with the rule and association readable afterwards and the association COMPLETE).
Emulator: the stack completes, Outputs.RuleId / Outputs.AssociationId carry stack-derived names (observed AssociationId 'sim2real-r53r-stack-5c816025514e-RuleAssociation-8C34351D89E1'), and replaying that value into GetResolverRuleAssociation answers HTTP 400 ResourceNotFoundException ('Resolver rule association with ID "…" does not exist.'). No CreateResolverRule / AssociateResolverRule is ever invoked by the stack.
Why: a stack reports success for resources that do not exist — the worst failure mode for infrastructure code. Every subsequent step (reading the rule, polling the association to COMPLETE, forwarding queries through the rule, and the delete-time assertion that both are gone) operates on a phantom id, so an IaC pipeline that works on AWS fails against the emulator, or worse, passes CreateStack and then fails unexplainably downstream.
Fix: add AWS::Route53Resolver::ResolverRule and AWS::Route53Resolver::ResolverRuleAssociation arms to CloudFormation's per-type provisioner (services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs, reached via provisioner_for; a type with no entry is left declared-only per provisioning.rs) that call CreateResolverRule and AssociateResolverRule with the template's DomainName/RuleType/ResolverEndpointId/TargetIps and ResolverRuleId/VPCId, report the ids those operations assign as the physical resource ids (so Ref resolves to them), and delete them (DisassociateResolverRule / DeleteResolverRule) on stack deletion. This is the SAME class of gap as open issue 20261024T062131Z-route53resolver-cloudformation-cannot-deploy-route-53-resolver-resources-firewall.md, which covers the firewall types and ResolverEndpoint only and does NOT cover ResolverRule or ResolverRuleAssociation.
Traces: route53resolver.deploy-resolver-rule-cfn-forwards-queries.

## Evidence
route53resolver.deploy-resolver-rule-cfn-forwards-queries: a stack whose template declares AWS::EC2::VPC + two subnets + a security group + AWS::Route53Resolver::ResolverEndpoint (OUTBOUND) + AWS::Route53Resolver::ResolverRule (FORWARD, DomainName 'sim2real-cfn-forward.test.', TargetIps 10.254.254.254:53) + AWS::Route53Resolver::ResolverRuleAssociation, with Outputs RuleId: !Ref Rule and AssociationId: !Ref RuleAssociation, reaches stack_create_complete. The check then fails with botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetResolverRuleAssociation operation: Resolver rule association with ID "sim2real-r53r-stack-5c816025514e-RuleAssociation-8C34351D89E1" does not exist. Expected: Ref returns an rslvr-rrassoc-* id, GetResolverRuleAssociation answers HTTP 200 and the association converges to Status COMPLETE with ResolverRuleId == Outputs.RuleId and VPCId == Outputs.VpcId.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. CreateStack with a template declaring AWS::Route53Resolver::ResolverEndpoint (OUTBOUND, two subnets, one SG), AWS::Route53Resolver::ResolverRule (RuleType FORWARD, ResolverEndpointId: !Ref of the endpoint, one TargetIps entry) and AWS::Route53Resolver::ResolverRuleAssociation (ResolverRuleId: !Ref Rule, VPCId: !Ref Vpc), exporting Outputs RuleId: !Ref Rule and AssociationId: !Ref RuleAssociation.
3. Wait for stack_create_complete — it succeeds.
4. Read Outputs.AssociationId: it is a stack-derived name such as '<stack>-RuleAssociation-<suffix>', not an rslvr-rrassoc-* id.
5. Call GetResolverRuleAssociation(ResolverRuleAssociationId=<that output>): HTTP 400 ResourceNotFoundException. Likewise GetResolverRule(Outputs.RuleId), and ListResolverRules shows the stack created no rule.
6. Replay behavioral check route53resolver.deploy-resolver-rule-cfn-forwards-queries.
