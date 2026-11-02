Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-nfwr-traj-create-firewall-to-test-path-refusals
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-nfwr-traj-create-firewall-to-test-path-refusals
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|CreateFirewall|4xx->2xx|$.Message
Issue-Key: network-firewall|CreateFirewall|4xx->2xx|$.__type
Issue-Key: network-firewall|CreateFirewall|4xx->2xx|status
Issue-Key: network-firewall|CreateFirewall|4xx->4xx|$.Message
Issue-Key: network-firewall|CreateFirewall|4xx->4xx|$.__type
Issue-Key: network-firewall|DescribeFirewall|4xx->2xx|$.Message
Issue-Key: network-firewall|DescribeFirewall|4xx->2xx|$.__type
Issue-Key: network-firewall|DescribeFirewall|4xx->2xx|status
Issue-Key: network-firewall|DeleteFirewall|4xx->2xx|status
Issue-Commit: c1ca043e5c77dd90c2a96695b3dfa06a20838f7d

# CreateFirewall skips VPC and subnet reachability validation, so the firewall is created and its lifecycle diverges

> _Note: this issue was identified against the code base at commit `c1ca043e5c77dd90c2a96695b3dfa06a20838f7d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewall

## Description
CreateFirewall accepts a request whose VpcId / SubnetMappings do not name resources the calling account can reach. AWS refuses those requests, so no firewall is ever created; the emulator creates one, and every later step in the firewall's lifecycle then diverges as well. This bundle has 4 causes.

Cause 1 of 4 - CreateFirewall accepts a VPC the account cannot reach (fixture network-firewall-nfwr-traj-create-firewall-to-test-path-refusals, step task#5).
AWS: CreateFirewall takes VpcId (Smithy: CreateFirewall.VpcId -> VpcId, pattern ^vpc-[0-9a-f]+$, length 1..128) and refuses a syntactically valid id that is not a VPC of the caller's account with HTTP 400 and the modeled common error InvalidRequestException (Smithy: InvalidRequestException, errorKind client, httpError 400, documented as "The operation failed because of a problem with your request... You specified an unsupported parameter name or value"). The recorded response body is __type 'com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException' with Message 'Given VPC is inaccessible from account' - the whole message is fixed text for this error class; it carries no value from the request.
Emulator: returns HTTP 200 with a created firewall; no __type and no Message at all.

Cause 2 of 4 - CreateFirewall accepts subnets the account cannot reach, and reports the wrong error class when it does refuse (fixture network-firewall-nfwr-traj-create-firewall-to-test-path-refusals, step task#6).
AWS: SubnetMappings entries that do not name subnets of the caller's account are refused with HTTP 400, __type 'com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException', Message 'Given subnet(s) is/are inaccessible from account' (fixed text in full; no request values interpolated).
Emulator: HTTP 400 but __type 'InvalidOperationException' with Message 'A resource with the specified name already exists' - it did not evaluate the subnets at all, it merely tripped over the firewall name that Cause 1 wrongly created. Note the error code is also unqualified: the recording shows the fully qualified 'com.amazonaws.networkfirewall.customerapi.v20201112#<Shape>' form for this service's awsJson1_0 errors.

Cause 3 of 4 - DescribeFirewall finds a firewall that should never have existed (fixture network-firewall-nfwr-traj-create-firewall-to-test-path-refusals, step task#8).
AWS: because every create above was refused, DescribeFirewall on that name returns HTTP 400 with __type 'com.amazonaws.networkfirewall.customerapi.v20201112#ResourceNotFoundException' and Message 'No object with the provided name or resource identifier was found' (Smithy: DescribeFirewall errors include ResourceNotFoundException, errorKind client, httpError 400, "Unable to locate a resource using the parameters that you provided"). The message is fixed text for this error class.
Emulator: HTTP 200 with a firewall description, no __type and no Message.

Cause 4 of 4 - DeleteFirewall deletes a firewall that should never have existed (fixture network-firewall-nfwr-traj-create-firewall-to-test-path-refusals, step teardown#9).
AWS: DeleteFirewall on the never-created name returns HTTP 400 (Smithy: DeleteFirewall errors include ResourceNotFoundException, httpError 400).
Emulator: HTTP 200.

Why: a client that uses CreateFirewall to validate its networking inputs gets a false success - it believes a firewall exists on an unreachable VPC/subnet, and its error-handling path for inaccessible-resource refusals is never exercised. The downstream Describe/Delete divergences mean the emulator's resource inventory permanently disagrees with AWS's after one bad create, so negative tests that assert absence pass against AWS and fail here (or vice versa).

Fix: CreateFirewall's handler/validation in the network-firewall service crate - add the account-reachability checks on VpcId and SubnetMappings before any state mutation, and raise InvalidRequestException with the fully qualified __type. Causes 3 and 4 should resolve once no firewall is created; verify both after fixing Causes 1-2.

## Evidence
task#5: expected 400 + __type com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException, Message 'Given VPC is inaccessible from account'; actual 200, __type and Message absent.
task#6 (and task#7 in the same cluster): expected 400 __type ...#InvalidRequestException, Message 'Given subnet(s) is/are inaccessible from account'; actual 400 __type 'InvalidOperationException', Message 'A resource with the specified name already exists'.
task#8: expected 400 __type ...#ResourceNotFoundException, Message 'No object with the provided name or resource identifier was found'; actual 200.
teardown#9: expected 400; actual 200.

## Steps to Reproduce
Replay fixture network-firewall-nfwr-traj-create-firewall-to-test-path-refusals against the emulator and compare steps task#5, task#6, task#7, task#8 and teardown#9. Equivalently: CreateFirewall with a well-formed but non-existent VpcId / SubnetMappings, then DescribeFirewall and DeleteFirewall on that FirewallName.
