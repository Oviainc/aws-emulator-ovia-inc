Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: network-firewall.subnet-change-protection-blocks-associate-rejected network-firewall.firewall-deletion-removes-resource
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall.subnet-change-protection-blocks-associate-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: network-firewall.firewall-deletion-removes-resource|run#17|AssertionError|assert mutate_code == "ResourceNotFoundException", (
Issue-Commit: 097f2e9dc37623fea40de160c134d8e6888945a6

# Network Firewall refusal wording and the deleted-firewall not-found gate diverge: AssociateSubnets' SubnetChangeProtection message is emulator-authored, and UpdateFirewallDeleteProtection is still a 501 scaffold (2 causes)

> _Note: this issue was identified against the code base at commit `097f2e9dc37623fea40de160c134d8e6888945a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateSubnets

## Description
TWO net-new root causes, one service (network-firewall). Each is its own replay cluster and BOTH must be closed before this issue is submitted. The service IS serving these paths (CreateFirewall, CreateFirewallPolicy, DescribeFirewall, DeleteFirewall, AssociateSubnets and UpdateSubnetChangeProtection all answer 200 in these same checks), so these are fidelity gaps, not a bootstrap gap.

Cause 1 of 2: AssociateSubnets refuses a subnet change on a SubnetChangeProtection=True firewall with the right code but the wrong message text.
AWS: Smithy (com.amazonaws.networkfirewall#NetworkFirewall_20201112, awsJson1_0) models AssociateSubnets with the client error InvalidOperationException (HTTP 400, member Message -> ErrorMessage), documented "The operation failed because it's not valid. For example, you might have tried to delete a rule group or firewall policy that's in use." The model does not state the message text, so the wording is recorded behavior only: AWS answers HTTP 400 / InvalidOperationException with the message "Cannot AssociateSubnets because SubnetChangeProtection is on". Its FIXED text is "Cannot " + " because SubnetChangeProtection is on"; its single VARIABLE part is the name of the refused operation, taken from the request being served (AssociateSubnets here; the same template names DisassociateSubnets on that call). No subnet id, ARN, account id or firewall name appears in it. The mapping is left untouched, and after UpdateSubnetChangeProtection sets the flag False the identical call succeeds (200) and both mappings appear.
Emulator: the code and status are now correct (HTTP 400 / InvalidOperationException), but the message is emulator-authored text: "Unable to change the subnet associations because the firewall is protected against subnet changes". It comes from the constant crate::state::SUBNET_CHANGE_PROTECTED_MESSAGE, which the AssociateSubnets handler puts on InvalidOperationException for the SubnetAssociationBreach::Protected arm.
Why: the error message is part of the contract a client sees. Operators and test suites assert on this text (it is the only place AWS names WHICH guardrail blocked the call and which operation it blocked), and log/alert matchers keyed on AWS's wording never fire against the emulator; a client cannot tell SubnetChangeProtection from any other InvalidOperationException cause because the emulator's text drops the flag name and the operation name.
Fix: the AssociateSubnets validation path — the Protected arm of associate_breach_error must carry AWS's template, and the shared constant in the service's state module must be rendered per operation ("Cannot <Operation> because SubnetChangeProtection is on") rather than being one fixed emulator sentence shared by the subnet-change paths. Note the resolved issue "Network Firewall error responses replace AWS's fixed exception messages with emulator-authored text" (20261023T104156Z) and "Network Firewall firewall, rule-group capacity and resource-policy error paths return emulator-authored messages..." (20261023T132856Z) corrected other message families; neither covers this constant, which is still emulator-authored. The open issue "Network Firewall does not track policy->rule-group associations, and the DisassociateSubnets / SubnetChangeProtection / UpdateLoggingConfiguration control paths are not served" (20261023T132856Z-network-firewall-does-not-track-policy-rule-group.md) names this check for the error CODE only (its replay key is the `code == "InvalidOperationException"` assertion, which now passes); the message assertion that now fails is not covered there.
Traces: network-firewall.subnet-change-protection-blocks-associate-rejected.

Cause 2 of 2: a mutation addressed to a DELETED firewall is answered with the scaffold error instead of ResourceNotFoundException, because UpdateFirewallDeleteProtection is still an unimplemented stub.
AWS: Smithy models UpdateFirewallDeleteProtection (input DeleteProtection required, FirewallArn/FirewallName; output FirewallArn/FirewallName/DeleteProtection/UpdateToken) with the client errors InvalidRequestException, InvalidTokenException, ResourceNotFoundException, ResourceOwnerCheckException and ThrottlingException (all HTTP 400) plus InternalServerError. ResourceNotFoundException is documented "Unable to locate a resource using the parameters that you provided." Once DeleteFirewall has removed a firewall, EVERY call naming that ARN — reads and mutations alike — is refused with HTTP 400 / ResourceNotFoundException, message "No object with the provided name or resource identifier was found". That message is entirely FIXED text with no variable part (it deliberately does not echo the name or ARN). The emulator already returns exactly that text for DescribeFirewall on the deleted ARN in this same check, so the not-found projection exists; it is the mutation path that never reaches it.
Emulator: UpdateFirewallDeleteProtection answers the scaffold error — error code "NotImplemented" ("operation UpdateFirewallDeleteProtection is scaffolded but not implemented in this emulator") — for every request, including one naming an ARN that no longer exists. The handler body is `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateFirewallDeleteProtection"))`.
Why: a client that proves a resource is gone by observing that a write to it is refused with ResourceNotFoundException gets an unmodelled error code that no SDK error branch, waiter or retry policy matches; delete-then-verify teardown logic and idempotent reconcilers key on the 400/ResourceNotFoundException and instead see a 501-class failure that reads as a transport/service fault. The modelled flag cannot be toggled on a live firewall either, so DeleteProtection can only ever be set at CreateFirewall time.
Fix: the UpdateFirewallDeleteProtection handler (services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/update_firewall_delete_protection.rs) must be served: resolve the firewall through the state module's firewall resolver, refuse an unknown or already-deleted ARN with ResourceNotFoundException carrying the service's fixed not-found text (the same constant crate::state::RESOURCE_NOT_FOUND_MESSAGE that AssociateSubnets already uses), honour UpdateToken as the optimistic lock, then flip the record's DeleteProtection and echo it back with a re-minted UpdateToken. An issue filed for the unimplemented scaffold of this operation (raised by network-firewall.delete-protected-firewall-rejected) names the operation and that file but covers only "implement the stub"; it does not state the deleted-ARN not-found precedence this check requires, so this check must be re-replayed after the handler is served.
Traces: network-firewall.firewall-deletion-removes-resource.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the built emulator. Overall 15/26 (57.7%); per-service network-firewall 13/22 (59%), 9 failed. Generated firewall/policy/subnet names, the account id, the region, ARNs, subnet ids and timestamps quoted below are environment-specific and are NOT part of any requirement.

Cause 1 — network-firewall.subnet-change-protection-blocks-associate-rejected [mismatch]:
  Setup: CreateFirewallPolicy (StatelessDefaultActions/StatelessFragmentDefaultActions = [aws:forward_to_sfe]); CreateFirewall with one SubnetMapping and SubnetChangeProtection=True; a second subnet created in another AZ. DescribeFirewall confirms exactly one SubnetMapping (this passes).
  Call: nfw.associate_subnets(FirewallArn=<arn>, SubnetMappings=[{'SubnetId': <second subnet>}]).
  Expected: HTTP 400, Error.Code 'InvalidOperationException', Error.Message 'Cannot AssociateSubnets because SubnetChangeProtection is on'.
  Actual: HTTP 400, Error.Code 'InvalidOperationException' (correct), Error.Message 'Unable to change the subnet associations because the firewall is protected against subnet changes'.
  Failing assertion: check_network_firewall_subnet_change_protection_blocks_associate_rejected.py line 43 — assert message == expected_message -> AssertionError: expected message 'Cannot AssociateSubnets because SubnetChangeProtection is on', got 'Unable to change the subnet associations because the firewall is protected against subnet changes'.
  Source: services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/associate_subnets.rs, fn associate_breach_error, Breach::Protected arm -> InvalidOperationException { message: crate::state::SUBNET_CHANGE_PROTECTED_MESSAGE }.

Cause 2 — network-firewall.firewall-deletion-removes-resource [mismatch]:
  The whole delete lifecycle up to the final step passes: DeleteFirewall answers a pre-deletion FirewallStatus.Status, DescribeFirewall on the ARN eventually answers ResourceNotFoundException with the expected message 'No object with the provided name or resource identifier was found', and ListFirewalls no longer reports the ARN.
  Call: nfw.update_firewall_delete_protection(FirewallArn=<the deleted arn>, DeleteProtection=False).
  Expected: HTTP 400, Error.Code 'ResourceNotFoundException', Error.Message 'No object with the provided name or resource identifier was found'.
  Actual: Error.Code 'NotImplemented' (scaffold error, message 'operation UpdateFirewallDeleteProtection is scaffolded but not implemented in this emulator').
  Failing assertion: check_network_firewall_firewall_deletion_removes_resource.py line 61 — assert mutate_code == "ResourceNotFoundException" -> AssertionError: expected a mutation on the deleted firewall to fail with ResourceNotFoundException, got NotImplemented.
  Source: services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/update_firewall_delete_protection.rs line 207 — Err(crate::scaffolded::Scaffolded::not_implemented("UpdateFirewallDeleteProtection")).

Smithy consulted (model_path services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json): AssociateSubnets (required SubnetMappings; errors InsufficientCapacityException/InternalServerError 500, InvalidOperationException/InvalidRequestException/InvalidTokenException/ResourceNotFoundException/ThrottlingException 400); UpdateFirewallDeleteProtection (required DeleteProtection, default false; errors InternalServerError 500, InvalidRequestException/InvalidTokenException/ResourceNotFoundException/ResourceOwnerCheckException/ThrottlingException 400); DeleteFirewall and DescribeFirewall (both model ResourceNotFoundException, HTTP 400); InvalidOperationException (client, HTTP 400, Message -> ErrorMessage, "The operation failed because it's not valid..."); ResourceNotFoundException (client, HTTP 400, Message -> ErrorMessage, "Unable to locate a resource using the parameters that you provided."). Neither exception's message text is modelled, so both wordings above are recorded behavior.

## Steps to Reproduce
Cause 1 (AssociateSubnets SubnetChangeProtection message):
1. aws network-firewall create-firewall-policy --firewall-policy-name p1 --firewall-policy '{"StatelessDefaultActions":["aws:forward_to_sfe"],"StatelessFragmentDefaultActions":["aws:forward_to_sfe"]}'
2. Create a VPC and two subnets in two different AZs.
3. aws network-firewall create-firewall --firewall-name fw1 --firewall-policy-arn <p1 arn> --vpc-id <vpc> --subnet-mappings SubnetId=<subnet1> --subnet-change-protection
4. aws network-firewall associate-subnets --firewall-arn <fw1 arn> --subnet-mappings SubnetId=<subnet2>
   -> expected: HTTP 400 InvalidOperationException, message exactly 'Cannot AssociateSubnets because SubnetChangeProtection is on'.
   -> emulator: HTTP 400 InvalidOperationException, message 'Unable to change the subnet associations because the firewall is protected against subnet changes'.
5. aws network-firewall describe-firewall --firewall-arn <fw1 arn> -> still exactly one SubnetMapping (this part already holds).
6. Positive control after the fix: aws network-firewall update-subnet-change-protection --firewall-arn <fw1 arn> --no-subnet-change-protection, repeat step 4 -> 200, and describe-firewall reports both mappings.
7. Re-replay network-firewall.subnet-change-protection-blocks-associate-rejected.

Cause 2 (mutation on a deleted firewall):
1. Create a firewall fw2 (DeleteProtection=False) with one subnet mapping, as above.
2. aws network-firewall delete-firewall --firewall-arn <fw2 arn> -> 200 with a pre-deletion FirewallStatus.Status.
3. Poll aws network-firewall describe-firewall --firewall-arn <fw2 arn> until it answers HTTP 400 ResourceNotFoundException with message 'No object with the provided name or resource identifier was found' (already correct), and confirm list-firewalls no longer reports the ARN.
4. aws network-firewall update-firewall-delete-protection --firewall-arn <fw2 arn> --no-delete-protection
   -> expected: HTTP 400 ResourceNotFoundException, message 'No object with the provided name or resource identifier was found'.
   -> emulator: error code NotImplemented ('operation UpdateFirewallDeleteProtection is scaffolded but not implemented in this emulator').
5. Positive control after the fix: on a LIVE firewall, update-firewall-delete-protection --delete-protection must answer 200 echoing DeleteProtection=true with a new UpdateToken, describe-firewall must report the flag, and passing a stale UpdateToken must be refused with InvalidTokenException.
6. Re-replay network-firewall.firewall-deletion-removes-resource.
