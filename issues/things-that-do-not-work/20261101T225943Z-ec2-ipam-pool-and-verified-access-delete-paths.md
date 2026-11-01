Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-consolidated-delete-and-deprovision-lifecycle ec2-verified-access-delete-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeIpamPools|4xx->2xx|$
Issue-Key: ec2|DescribeIpamPools|4xx->2xx|$.Response
Issue-Key: ec2|DescribeIpamPools|4xx->2xx|status
Issue-Key: ec2|DescribeIpamPools|2xx->2xx|$.DescribeIpamPoolsResponse.ipamPoolSet.item.locale
Issue-Key: ec2|DeleteVerifiedAccessInstance|2xx->4xx|$.DeleteVerifiedAccessInstanceResponse
Issue-Key: ec2|DeleteVerifiedAccessInstance|2xx->4xx|status
Issue-Commit: 25868398053db3fb9f24c4ac75ab683f5b2a8f47

# EC2 IPAM pool and verified-access delete paths leave deleted resources readable and refuse a valid delete

> _Note: this issue was identified against the code base at commit `25868398053db3fb9f24c4ac75ab683f5b2a8f47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeIpamPools

## Description
Three net-new wire divergences on EC2 delete/teardown paths: a deleted IPAM pool stays readable instead of becoming NotFound, the IPAM pool projection drops a modeled member, and a valid DeleteVerifiedAccessInstance is refused. This issue bundles 3 causes; all 3 must be closed. The open issues '20261030T124526Z-ec2-deleteipampool-response-omits-the-modeled-ipampool-locale.md' (same locale omission on DeleteIpamPool) and '20261030T210852Z-ec2-ipam-prefix-list-resolver-reads-omit-version.md' (deleted resolvers stay readable) describe the same families on other operations but do not carry these replay keys; '20261030T063325Z-ec2-vpc-verified-access-public-ipv4-pool-and.md' covers DescribeVerifiedAccessInstances, not the delete path.

Cause 1 of 3: after the pool is deleted, DescribeIpamPools keeps answering 200 with the pool in state 'delete-complete' instead of the NotFound refusal.
AWS: recorded behavior only for the terminal read (this checkout's EC2 model declares no errors on DescribeIpamPools, so the code is not modeled): once deletion completes, the recording answers HTTP 400 with Response.Errors.Error.Code 'InvalidIpamPoolId.NotFound' and Message 'The IPAM pool (ipam-pool-0000000000000002b) does not exist.' The message template is the fixed text 'The IPAM pool (' + the IpamPoolId taken from the request's IpamPoolIds + ') does not exist.'; the id in the recording is that run's value, not part of the requirement.
Emulator: HTTP 200 with DescribeIpamPoolsResponse.ipamPoolSet.item describing the pool with state 'delete-complete', for the full 60s the step polls (13 attempts), so the fixture's wait-for-NotFound never resolves.
Why: a client's teardown loop polls this read until it 404s/400s; the emulator keeps the resource visible forever, so the teardown hangs, and any inventory reconciliation counts a deleted pool as live.
Fix: the DescribeIpamPools handler's state lookup - a pool that has reached delete-complete must be removed from the readable set and the id answered with InvalidIpamPoolId.NotFound.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#15.

Cause 2 of 3: DescribeIpamPools omits the modeled ipamPool locale member.
AWS: Smithy: IpamPool.Locale, xmlName 'locale' - 'The locale of the IPAM pool.' The recording carries the member on ipamPoolSet.item (present, with no value, for a pool created without a locale).
Emulator: the key is absent from the item entirely.
Why: a strict deserializer that expects the member gets a missing key rather than an empty one, and a client that reads locale to decide where the pool may allocate has nothing to read - the same defect already filed for DeleteIpamPool, so both projections share the fix.
Fix: the IpamPool projection used by the DescribeIpamPools handler - emit locale even when unset.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#12.

Cause 3 of 3: DeleteVerifiedAccessInstance refuses a valid delete with HTTP 400.
AWS: Smithy: DeleteVerifiedAccessInstance takes a required VerifiedAccessInstanceId and declares no modeled errors (errors == []); its output carries VerifiedAccessInstance (xmlName 'verifiedAccessInstance'). The recording answers HTTP 200 and echoes the deleted instance - verifiedAccessInstanceId, description, fipsEnabled, creationTime, lastUpdatedTime and the (empty) verifiedAccessTrustProviderSet and verifiedAccessInstanceTransitGatewayAttachmentSet. The instance id, description and timestamps in the recording are that run's values.
Emulator: HTTP 400 and no DeleteVerifiedAccessInstanceResponse body at all, for an instance it had just created in the same fixture.
Why: the delete of an existing, unattached verified-access instance is the normal teardown call; refusing it leaks the resource and makes the client's cleanup fail hard.
Fix: the DeleteVerifiedAccessInstance handler - it must resolve the existing instance from state, delete it and answer 200 with the deleted instance echoed.
Traces: ec2-verified-access-delete-lifecycle#task#13.

## Evidence
Cause 1 (ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#15): status expected 400, got 200. Expected body {'Response': {'Errors': {'Error': {'Code': 'InvalidIpamPoolId.NotFound', 'Message': 'The IPAM pool (ipam-pool-0000000000000002b) does not exist.'}}, 'RequestID': ...}}; got {'DescribeIpamPoolsResponse': {'ipamPoolSet': {'item': {... 'ipamPoolId': 'ipam-pool-0000000000000002b', 'state': 'delete-complete' ...}}}}. Replay note: 'poll: error code None not in ["InvalidIpamPoolId.NotFound"] after 13 attempt(s)', 'poll capped at 60s'.
Cause 2 (same fixture, task#12): $.DescribeIpamPoolsResponse.ipamPoolSet.item.locale: missing - the member is present in the recorded response and absent from the emulator's.
Cause 3 (ec2-verified-access-delete-lifecycle#task#13): status expected 200, got 400; $.DeleteVerifiedAccessInstanceResponse missing entirely (expected the response with verifiedAccessInstance{creationTime, description, fipsEnabled, lastUpdatedTime, verifiedAccessInstanceId, verifiedAccessInstanceTransitGatewayAttachmentSet '', verifiedAccessTrustProviderSet ''}).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. ec2-ipam-consolidated-delete-and-deprovision-lifecycle: run the fixture through pool creation, deprovision and DeleteIpamPool, then call DescribeIpamPools with the deleted pool id (step task#15) - the emulator answers 200 with state 'delete-complete' instead of 400 InvalidIpamPoolId.NotFound; at step task#12 compare the ipamPoolSet item against the recording and note the absent locale member.
3. ec2-verified-access-delete-lifecycle: create a verified-access instance as the fixture does, then call DeleteVerifiedAccessInstance with its id (step task#13) - the emulator answers 400 with no response body where the recording answers 200 echoing the deleted instance.
