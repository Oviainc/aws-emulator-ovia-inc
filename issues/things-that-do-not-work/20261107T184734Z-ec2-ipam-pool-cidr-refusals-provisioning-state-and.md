Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-consolidated-delete-and-deprovision-lifecycle ec2-ipam-trajwire-provision-cidr-mistakes ec2-ipam-trajwire-read-side-mistakes ec2-ipam-trajwire-reference-path-regional-child-pool ec2-ipam-trajwire-vpc-from-top-pool-then-child ec2-ipam-trajwire-stale-pool-reference ec2-ipam-trajwire-teardown-order-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DeprovisionIpamPoolCidr|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeprovisionIpamPoolCidr|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|ProvisionIpamPoolCidr|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|ProvisionIpamPoolCidr|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|ProvisionIpamPoolCidr|2xx->2xx|$.ProvisionIpamPoolCidrResponse.ipamPoolCidr.ipamPoolCidrId
Issue-Key: ec2|ProvisionIpamPoolCidr|2xx->4xx|$.ProvisionIpamPoolCidrResponse
Issue-Key: ec2|ProvisionIpamPoolCidr|2xx->4xx|status
Issue-Key: ec2|GetIpamPoolCidrs|2xx->2xx|$
Issue-Key: ec2|GetIpamPoolCidrs|2xx->2xx|$.GetIpamPoolCidrsResponse.ipamPoolCidrSet
Issue-Key: ec2|GetIpamPoolCidrs|2xx->2xx|$.GetIpamPoolCidrsResponse.ipamPoolCidrSet.item.state
Issue-Key: ec2|GetIpamPoolCidrs|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteIpamPool|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteIpam|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteIpam|2xx->4xx|status
Issue-Commit: ab92470025dd8eeb501814706ada4aa2b617e26c

# EC2 IPAM pool CIDR refusals, provisioning state and delete gating diverge: 10 net-new causes

> _Note: this issue was identified against the code base at commit `ab92470025dd8eeb501814706ada4aa2b617e26c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ProvisionIpamPoolCidr

## Description
The emulator's IPAM plane is implemented (these steps are NOT scaffold stubs and no open issue carries their replay keys), but its refusal codes, refusal messages, generated ids, provisioning state machine and delete gating all diverge from AWS. This issue bundles 10 net-new causes, all in service ec2. The ec2 Smithy model declares no operation-level errors for any of these operations (query=operation reports errors: [] for ProvisionIpamPoolCidr, DeprovisionIpamPoolCidr, DeleteIpam), so every status/code/message requirement below is taken from the recorded response and is labelled "recorded behavior only". Open issues 20261101T225943Z-ec2-ipam-pool-and-verified-access-delete-paths.md, 20261029T182456Z-ec2-ten-net-new-divergences-in-filter-validation.md and the various *-stub issues (ProvisionIpamPoolCidr, DeprovisionIpamPoolCidr, DeleteIpam, GetIpamPoolCidrs) describe neighbouring IPAM work; none of them covers the causes below.

Cause 1 of 10 - DeprovisionIpamPoolCidr refuses an unprovisioned CIDR with the wrong error class.
AWS: recorded behavior only (Smithy: DeprovisionIpamPoolCidrRequest{IpamPoolId required, Cidr}; errors: []). Deprovisioning a CIDR that is not provisioned in the pool is answered HTTP 400 with Response.Errors.Error.Code "IncorrectState" and Message "The specified action is not valid.The pool CIDR <cidr> does not exist." - the fixed text is "The specified action is not valid." followed by "The pool CIDR " <cidr> " does not exist."; the only variable part is the CIDR copied from the request.
Emulator: HTTP 400 with Code "InvalidCidr.NotFound" and Message "The CIDR (<cidr>) does not exist."
Why: a client that branches on IncorrectState (treat-as-already-deprovisioned, then continue teardown) sees an unknown code and aborts.
Fix: the error mapping in the DeprovisionIpamPoolCidr handler's validation.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#13.

Cause 2 of 10 - ProvisionIpamPoolCidr with neither Cidr nor NetmaskLength uses the wrong code and wording.
AWS: recorded behavior only (Smithy: both ProvisionIpamPoolCidrRequest.Cidr and .NetmaskLength are optional members, so the mutual requirement is enforced at runtime, errors: []). The call is refused HTTP 400 with Code "InvalidParameterCombination" and Message "The request must contain either a CIDR or Netmask Length." - entirely fixed text, no variable parts.
Emulator: HTTP 400 with Code "MissingParameter" and Message "Either 'cidr' or 'netmaskLength' must be provided".
Why: InvalidParameterCombination vs MissingParameter is a different error class for SDK retry/validation handling, and the message is emulator-authored prose.
Fix: the ProvisionIpamPoolCidr handler's input validation.
Traces: ec2-ipam-trajwire-provision-cidr-mistakes#task#5 (cluster also covers #task#6).

Cause 3 of 10 - the minted ipamPoolCidrId does not match the recorded id shape.
AWS: recorded behavior only (Smithy: IpamPoolCidr.IpamPoolCidrId -> IpamPoolCidrId, serialized as ipamPoolCidrId). AWS mints one opaque per-CIDR id; the fixture asserts its shape rather than a literal value.
Emulator: returns an id drawn from the shared sequential resource counter (prefix "ipam-pool-cidr-" plus the zero-padded counter value), which the recorded shape assertion rejects.
Why: clients and the corpus correlate the provisioned CIDR by this id; a counter-derived id is not an AWS-shaped id, and it collides in form with every other emulator resource id.
Fix: ipamPoolCidrId minting in the IPAM state (give pool CIDRs their own AWS-shaped id generator, as used for the other ipam-* resources).
Traces: ec2-ipam-trajwire-provision-cidr-mistakes#task#7 (cluster also covers #task#12, ec2-ipam-trajwire-read-side-mistakes#task#9, ec2-ipam-trajwire-reference-path-regional-child-pool#task#7, ec2-ipam-trajwire-vpc-from-top-pool-then-child#task#12).

Cause 4 of 10 - a netmask length the parent pool cannot satisfy is refused synchronously instead of accepted and failed asynchronously.
AWS: recorded behavior only. ProvisionIpamPoolCidr with a NetmaskLength the parent pool cannot satisfy is ACCEPTED: HTTP 200 with ipamPoolCidr{ipamPoolCidrId, netmaskLength echoed from the request, state "pending-provision"}. The failure is reported later through the CIDR's state, not at call time.
Emulator: HTTP 400, no ProvisionIpamPoolCidrResponse at all.
Why: the client never gets a CIDR id to poll, so the whole "provision then observe failure" flow is unreachable.
Fix: the ProvisionIpamPoolCidr handler - move the parent-pool capacity check out of the synchronous reject path into the provisioning state machine.
Traces: ec2-ipam-trajwire-provision-cidr-mistakes#task#10.

Cause 5 of 10 - GetIpamPoolCidrs never surfaces a failed-provision CIDR or its failureReason.
AWS: Smithy: IpamPoolCidr.State -> IpamPoolCidrState and IpamPoolCidr.FailureReason -> IpamPoolCidrFailureReason{code, message}. Recorded: after the request in cause 4, GetIpamPoolCidrs returns ipamPoolCidrSet.item with state "failed-provision" and failureReason.message "A CIDR with this netmask length is not available in the parent pool." (fixed text for this failure class).
Emulator: ipamPoolCidrSet is serialized empty for the whole polling window (the replay's poll is capped at 180s after 37 attempts).
Why: a waiter on failed-provision never terminates, and the failure reason is unavailable to the caller.
Fix: the IPAM pool-CIDR state machine plus the GetIpamPoolCidrs serializer (emit the item with state and failureReason).
Traces: ec2-ipam-trajwire-provision-cidr-mistakes#task#11.

Cause 6 of 10 - unknown-pool message wording on GetIpamPoolCidrs.
AWS: recorded behavior only. A read against a deleted/unknown pool is refused HTTP 400 with Message "The ipam-pool ID '<ipam-pool-id>' does not exist" - fixed text "The ipam-pool ID '" ... "' does not exist", where the id is the one from the request. AWS names the resource by its wire type token "ipam-pool".
Emulator: same status and code, but Message "The IPAM pool ID '<ipam-pool-id>' does not exist" ("IPAM pool" instead of "ipam-pool").
Why: callers and tests that match the documented message text do not match.
Fix: the shared IPAM not-found message formatter used by the GetIpamPoolCidrs handler.
Traces: ec2-ipam-trajwire-stale-pool-reference#task#7.

Cause 7 of 10 - the same unknown-pool wording defect on ProvisionIpamPoolCidr.
AWS: recorded behavior only - "The ipam-pool ID '<ipam-pool-id>' does not exist", as in cause 6.
Emulator: "The IPAM pool ID '<ipam-pool-id>' does not exist".
Why: as cause 6; filed separately because it is a second call site (a fix to one handler does not necessarily fix the other).
Fix: the ProvisionIpamPoolCidr handler's pool lookup error path.
Traces: ec2-ipam-trajwire-stale-pool-reference#task#9.

Cause 8 of 10 - DeleteIpamPool refuses with the wrong reason when child pools source from the pool.
AWS: recorded behavior only. HTTP 400 with Message "The specified action is not valid for <ipam-pool-id>. There are one or more pools that use the pool as a source." - fixed text with the pool id from the request.
Emulator: HTTP 400 but Message "The specified action is not valid for <ipam-pool-id>. There are one or more pool cidrs provisioned in the pool." - it reports the provisioned-CIDR reason, i.e. it checks the wrong dependency.
Why: the caller is told to deprovision CIDRs when the real blocker is a child pool, so the remediation it performs never unblocks the delete.
Fix: the DeleteIpamPool handler's dependency gating (distinguish source-pool references from provisioned CIDRs and emit the matching reason).
Traces: ec2-ipam-trajwire-teardown-order-mistakes#task#14.

Cause 9 of 10 - DeleteIpam names the wrong resource and reason when pools are still attached.
AWS: recorded behavior only. HTTP 400 with Message "The specified action is not valid for <ipam-scope-id>. The scope is either default scope or there are one or more pools attached to the scope." - the variable part is the SCOPE id.
Emulator: Message "The specified action is not valid for <ipam-id>. There are one or more pools in the ipam." - it names the IPAM id and an emulator-authored reason.
Why: the message points the caller at the wrong resource, so it cannot find the scope whose pools block the delete.
Fix: the DeleteIpam handler's dependency gating and message formatting.
Traces: ec2-ipam-trajwire-teardown-order-mistakes#task#15.

Cause 10 of 10 - DeleteIpam refuses a valid delete at teardown.
AWS: recorded behavior only. Once the dependent pools/scopes are gone, DeleteIpam succeeds with HTTP 200 and a DeleteIpamResult (Smithy: DeleteIpamResult.Ipam -> ipam).
Emulator: HTTP 400.
Why: teardown cannot complete; the IPAM is leaked and every subsequent run inherits it.
Fix: the DeleteIpam handler's gating/state cleanup (the dependency check still sees removed children).
Traces: ec2-ipam-trajwire-teardown-order-mistakes#teardown#19.

## Evidence
Per-cause expected vs actual, from the replay of trace set simparity-wire-aws-v1 (emulator up, 419 fixtures replayed):
1) ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#13 DeprovisionIpamPoolCidr: expected 400 Code 'IncorrectState', Message 'The specified action is not valid.The pool CIDR 10.99.0.0/24 does not exist.'; got 400 Code 'InvalidCidr.NotFound', Message 'The CIDR (10.99.0.0/24) does not exist.'
2) ec2-ipam-trajwire-provision-cidr-mistakes#task#5 ProvisionIpamPoolCidr: expected Code 'InvalidParameterCombination', Message 'The request must contain either a CIDR or Netmask Length.'; got Code 'MissingParameter', Message "Either 'cidr' or 'netmaskLength' must be provided".
3) ec2-ipam-trajwire-provision-cidr-mistakes#task#7 ProvisionIpamPoolCidr: $.ProvisionIpamPoolCidrResponse.ipamPoolCidr.ipamPoolCidrId shape-assertion failure; emulator returned 'ipam-pool-cidr-0000000000000003a' (sequential counter).
4) ec2-ipam-trajwire-provision-cidr-mistakes#task#10 ProvisionIpamPoolCidr: expected 200 with ipamPoolCidr{netmaskLength '7', state 'pending-provision'}; got 400 and no response body.
5) ec2-ipam-trajwire-provision-cidr-mistakes#task#11 GetIpamPoolCidrs: expected ipamPoolCidrSet.item{state 'failed-provision', failureReason.message 'A CIDR with this netmask length is not available in the parent pool.'}; got ipamPoolCidrSet '' (poll capped at 180s, state not in ['failed-provision'] after 37 attempts).
6) ec2-ipam-trajwire-stale-pool-reference#task#7 GetIpamPoolCidrs: expected Message "The ipam-pool ID 'ipam-pool-0000000000000000f' does not exist"; got "The IPAM pool ID 'ipam-pool-0000000000000000f' does not exist".
7) ec2-ipam-trajwire-stale-pool-reference#task#9 ProvisionIpamPoolCidr: same expected/actual wording divergence.
8) ec2-ipam-trajwire-teardown-order-mistakes#task#14 DeleteIpamPool: expected '... There are one or more pools that use the pool as a source.'; got '... There are one or more pool cidrs provisioned in the pool.'
9) ec2-ipam-trajwire-teardown-order-mistakes#task#15 DeleteIpam: expected 'The specified action is not valid for ipam-scope-<id>. The scope is either default scope or there are one or more pools attached to the scope.'; got 'The specified action is not valid for ipam-<id>. There are one or more pools in the ipam.'
10) ec2-ipam-trajwire-teardown-order-mistakes#teardown#19 DeleteIpam: expected 200; got 400.
(The ids, CIDRs and request ids above are this run's values; the requirement is the status, code and the message template, not the literal values.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture corpus (trace set simparity-wire-aws-v1).
2. Replay fixture ec2-ipam-consolidated-delete-and-deprovision-lifecycle, step task#13 (the representative), then the nine fixture#step bindings listed in trace_ids.
3. For each, compare HTTP status, Response.Errors.Error.Code and Response.Errors.Error.Message (or the response body for the 200 cases) against the recorded response.
