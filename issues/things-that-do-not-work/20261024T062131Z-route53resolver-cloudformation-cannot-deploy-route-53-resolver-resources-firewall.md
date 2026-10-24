Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.deploy-firewall-domain-list-cfn-blocks-domain route53resolver.deploy-firewall-rule-group-cfn-association-active route53resolver.deploy-firewall-rule-group-cfn-enforced-blocking route53resolver.deploy-resolver-endpoint-cfn-behaves
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|CreateStack|ValidationError
Issue-Key: route53resolver|GetResolverEndpoint|ResourceNotFoundException
Issue-Commit: df1261813c70037074c7525ba4fddd96df97f3b7

# CloudFormation cannot deploy Route 53 Resolver resources: firewall types are unrecognized and ResolverEndpoint is never provisioned

> _Note: this issue was identified against the code base at commit `df1261813c70037074c7525ba4fddd96df97f3b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two root causes behind every CloudFormation-based Route 53 Resolver behavioral check. The failures surface in route53resolver checks (hence this issue's service token) but the code that must change is the CloudFormation crate's resource-type registry and its per-type provisioner. A fixer must close BOTH causes before submitting.

Cause 1 of 2: the CloudFormation template resource-type registry does not recognise the AWS::Route53Resolver::* DNS Firewall types, so CreateStack refuses the template outright.
AWS: AWS::Route53Resolver::FirewallDomainList, AWS::Route53Resolver::FirewallRuleGroup and AWS::Route53Resolver::FirewallRuleGroupAssociation are published CloudFormation resource types; a template declaring them is accepted and the stack creates the corresponding Resolver DNS Firewall resources (recorded behavior only for the stack transition — the recordings show the stack reaching CREATE_COMPLETE and the firewall resources existing in Route 53 Resolver afterwards). CloudFormation's "Unrecognized resource types" ValidationError is reserved for types that genuinely do not exist.
Emulator: CreateStack answers HTTP 400 with error code ValidationError and message "Template format error: Unrecognized resource types: [AWS::Route53Resolver::FirewallDomainList, AWS::Route53Resolver::FirewallRuleGroup, AWS::Route53Resolver::FirewallRuleGroupAssociation]" (and the two-type variant), because those types are absent from the recognised set the handler checks.
Why: every infrastructure-as-code path to Route 53 Resolver DNS Firewall is closed. A client deploying a real, valid template is told its template is malformed, which is indistinguishable from a template bug — so DNS-firewall deployments cannot be tested at all, and a CI pipeline that deploys them fails against the emulator while succeeding on AWS.
Fix: add the three AWS::Route53Resolver::* firewall types to CloudFormation's recognised resource-type set (services/service-com-amazonaws-cloudformation-cloudformation/src/state/resource_types.rs, consulted by CreateStack) and give each a provisioner arm that calls the owning Resolver operations (CreateFirewallDomainList/UpdateFirewallDomains, CreateFirewallRuleGroup/CreateFirewallRule, AssociateFirewallRuleGroup) so the declared resources actually exist.
Traces: route53resolver.deploy-firewall-domain-list-cfn-blocks-domain, route53resolver.deploy-firewall-rule-group-cfn-association-active, route53resolver.deploy-firewall-rule-group-cfn-enforced-blocking.

Cause 2 of 2: AWS::Route53Resolver::ResolverEndpoint has no provisioner, so the stack completes without creating any endpoint and Ref resolves to a name instead of the endpoint id.
AWS: for AWS::Route53Resolver::ResolverEndpoint, Ref returns the Resolver endpoint's ID (the rslvr-* id GetResolverEndpoint accepts), and the stack really creates the endpoint, which then reaches OPERATIONAL and can carry resolver rules (recorded behavior only: the recording shows the stack output feeding GetResolverEndpoint successfully and a FORWARD rule being created against it).
Emulator: the type has no entry in the per-type provisioner, so it is left "declared-only": no CreateResolverEndpoint is ever called, the stack still reaches CREATE_COMPLETE, and the output's Ref resolves to the template's endpoint Name. Replaying the stack output into GetResolverEndpoint fails with HTTP 400 ResourceNotFoundException, message '[RSLVR-00501] Resolver endpoint with ID "sim2real-endpoint-103d7bb2f28e" does not exist.' (the id shown is the Name the template passed, not an rslvr-* id).
Why: a stack reports success for a resource that does not exist, which is the worst possible failure mode for infrastructure code — subsequent steps (GetResolverEndpoint, CreateResolverRule with ResolverEndpointId, associating that rule) all fail or operate on a phantom id, and a deployment that AWS would have made usable silently produces nothing.
Fix: add an AWS::Route53Resolver::ResolverEndpoint arm to CloudFormation's per-type provisioner (services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs, reached via provisioner_for) that calls CreateResolverEndpoint in the Resolver service with the template's Direction/Name/IpAddresses/SecurityGroupIds, reports the id that service assigns as the resource's physical resource id (so Ref resolves to it), and deletes it on stack deletion.
Traces: route53resolver.deploy-resolver-endpoint-cfn-behaves.

## Evidence
Cause 1: route53resolver.deploy-firewall-domain-list-cfn-blocks-domain, route53resolver.deploy-firewall-rule-group-cfn-association-active and route53resolver.deploy-firewall-rule-group-cfn-enforced-blocking all abort on the first call — botocore ClientError (ValidationError) calling CreateStack: "Template format error: Unrecognized resource types: [AWS::Route53Resolver::FirewallDomainList, AWS::Route53Resolver::FirewallRuleGroup, AWS::Route53Resolver::FirewallRuleGroupAssociation]" (two-type variant for the rule-group templates). Expected: the stack is accepted and creates those resources. CreateStack's own docs point at crate::state::resource_types for the recognised set.
Cause 2: route53resolver.deploy-resolver-endpoint-cfn-behaves — the stack with a single AWS::Route53Resolver::ResolverEndpoint resource reaches stack_create_complete and exports Outputs.ResolverEndpointId, but GetResolverEndpoint(ResolverEndpointId=<that output>) raises ResourceNotFoundException '[RSLVR-00501] Resolver endpoint with ID "sim2real-endpoint-103d7bb2f28e" does not exist.' — the output value is the EndpointName parameter, not a Resolver endpoint id, and no endpoint was created. services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioner.rs contains no Route53Resolver arm, and provisioning.rs documents that a type with no entry is left declared-only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay behavioral check route53resolver.deploy-firewall-domain-list-cfn-blocks-domain (or either deploy-firewall-rule-group-cfn-* check): CreateStack with a template declaring AWS::Route53Resolver::FirewallDomainList / FirewallRuleGroup / FirewallRuleGroupAssociation is refused with HTTP 400 ValidationError "Unrecognized resource types".
3. Replay route53resolver.deploy-resolver-endpoint-cfn-behaves: CreateStack a template whose only resource is AWS::Route53Resolver::ResolverEndpoint (Direction OUTBOUND, two subnets, one security group) with Output Value: !Ref ResolverEndpoint.
4. Wait for stack_create_complete and read Outputs.ResolverEndpointId — it is the template's EndpointName.
5. Call GetResolverEndpoint with that value: HTTP 400 ResourceNotFoundException RSLVR-00501, and ListResolverEndpoints shows no endpoint was created by the stack.
