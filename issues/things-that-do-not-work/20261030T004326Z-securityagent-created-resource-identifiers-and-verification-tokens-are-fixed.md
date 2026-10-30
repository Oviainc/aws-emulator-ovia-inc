Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-agent-space-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-agent-space-lifecycle securityagent-artifact-batch-and-error-paths securityagent-artifact-lifecycle securityagent-security-requirement-pack-lifecycle securityagent-target-domain-tagging-and-errors
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.agentSpaceId
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.id
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.arn
Issue-Key: securityagent|CreateSecurityRequirementPack|2xx->2xx|$.packId
Issue-Key: securityagent|CreateTargetDomain|2xx->2xx|$.targetDomainId
Issue-Key: securityagent|CreateTargetDomain|2xx->2xx|$.verificationDetails.httpRoute.token
Issue-Commit: 716594ad03747236be5f362ba3ba8170b7a2d152

# Created resource identifiers and verification tokens are fixed constants

> _Note: this issue was identified against the code base at commit `716594ad03747236be5f362ba3ba8170b7a2d152`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentSpace

## Description
FOUR causes, one shared fault: every securityagent create-path emits a fixed, non-unique identifier (and, for target domains, a fixed verification token) instead of minting a fresh opaque value per resource. The emulator hands back the literal 32-character id '00000000000000000000000000000001' for the first resource of EVERY resource type (agent space, security-requirement pack, target domain), so identifiers collide across resource types and are identical on every run, and the DNS/HTTP domain-ownership token is a single baked-in constant shared by all domains. Shape and HTTP status are correct in all four causes; only the identity values diverge. All four must be closed before submitting.

Cause 1 of 4: CreateAgentSpace (fixture securityagent-agent-space-lifecycle, step setup#1).
AWS: Smithy: CreateAgentSpace -> CreateAgentSpaceOutput.agentSpaceId @required, target AgentSpaceId, documented '<p>Unique identifier of the agent space.</p>'; http POST /CreateAgentSpace, code 200. AWS returns a newly generated, unique, opaque agent-space identifier for each CreateAgentSpace call, and the response's id/agentSpaceId fields carry that per-resource value. (Format beyond 'string' is recorded behavior only: a 32-character lowercase hex-style token.)
Emulator: returns 200 with $.id and $.agentSpaceId both set to the constant '00000000000000000000000000000001' rather than a value unique to the resource just created; the replay step expected the fixture's captured agent-space id ({{agent_space_id}}) and got the constant instead.
Why: a client that creates two or more agent spaces (or an agent space plus a pack plus a domain) receives the same identifier for distinct resources, so any map/cache keyed by id silently aliases them, subsequent per-id calls address the wrong resource, and ARNs built from the id are not distinct. Tests that assert the id they just received is new also break, because a second run of the same workflow reuses the identifier of the first.
Fix: the CreateAgentSpace handler's identifier allocation in the securityagent state layer - the id must be minted per created resource from a generator whose values are unique within the account and across resource types, and the same value must then be echoed into $.id, $.agentSpaceId and the resource ARN.
Traces: securityagent-agent-space-lifecycle#setup#1.

Cause 2 of 4: CreateAgentSpace on the artifact fixtures (fixture securityagent-artifact-batch-and-error-paths, step task#1; also affects securityagent-artifact-lifecycle#setup#1).
AWS: same model binding as Cause 1; additionally the resource ARN embeds the newly minted agent-space id, so arn and agentSpaceId agree and are unique per resource.
Emulator: returns 200 with $.id, $.agentSpaceId and $.arn all built from the same constant '00000000000000000000000000000001' ($.arn = 'arn:aws:securityagent:<region>:<account>:agent-instance/00000000000000000000000000000001'), where the fixture expected the id captured from this call ({{agent_space_id_err}}). This confirms the constant is not fixture-specific: two independent fixtures creating different agent spaces get byte-identical identifiers and byte-identical ARNs.
Why: the ARN is the cross-service handle for the resource; two different agent spaces sharing one ARN makes tagging, IAM-resource matching and any ARN-keyed bookkeeping address the wrong resource.
Fix: same allocator as Cause 1, with the ARN serializer for CreateAgentSpace deriving its resource suffix from the per-resource id rather than from a constant.
Traces: securityagent-artifact-batch-and-error-paths#task#1, securityagent-artifact-lifecycle#setup#1.

Cause 3 of 4: CreateSecurityRequirementPack (fixture securityagent-security-requirement-pack-lifecycle, step task#1).
AWS: Smithy: CreateSecurityRequirementPack -> CreateSecurityRequirementPackOutput.packId @required, target SecurityRequirementPackId; http POST /CreateSecurityRequirementPack, code 201. AWS returns a unique pack identifier for the pack just created, distinct from the identifiers of any other resource type.
Emulator: returns the correct 201 but with $.packId = '00000000000000000000000000000001' - the SAME constant it already used for agent-space ids - where the fixture expected the pack id captured from this call ({{pack_id}}).
Why: the pack id collides with an agent-space id, so a client holding both cannot tell them apart and the per-type uniqueness AWS guarantees is lost; a workflow that creates two packs cannot address the second one.
Fix: the CreateSecurityRequirementPack handler's id allocation in the securityagent state layer; allocate from a generator that does not restart per resource type.
Traces: securityagent-security-requirement-pack-lifecycle#task#1.

Cause 4 of 4: CreateTargetDomain (fixture securityagent-target-domain-tagging-and-errors, step setup#1).
AWS: Smithy: CreateTargetDomain -> CreateTargetDomainOutput.targetDomainId @required, target TargetDomainId, documented '<p>Unique identifier of the target domain.</p>'; verificationDetails -> VerificationDetails with dnsTxt (DnsVerification.token, '<p>The verification token to include in the DNS record value.</p>') and httpRoute (HttpVerification.token, '<p>The verification token to serve at the specified route path.</p>'). AWS mints both a unique target-domain id AND a fresh ownership-verification token per target domain - the token is the proof-of-control secret for that one domain, so it is never shared between domains or between verification methods.
Emulator: returns 200 with $.targetDomainId = the constant '00000000000000000000000000000001' (fixture expected the captured {{target_domain_id}}), and $.verificationDetails.httpRoute.token = the constant '0778a98ec3c7bfd032e1266778cc3d244a3db4cc51b7638568e2a01afaffd780' (fixture expected the captured {{http_route_token_2}}). That same token literal is what the emulator also serves as the dnsTxt token for a DIFFERENT domain in securityagent-agent-space-target-domain-association, so the token is a single baked-in constant rather than per-domain state, and it is not derived from the domain name either (the two domains differ).
Why: every domain is told to publish the identical challenge value, so domain-ownership verification is not actually scoped to a domain - a client cannot distinguish the record it must publish for one domain from another's, and a test that verifies two domains in sequence publishes one record and appears to verify both. The duplicated targetDomainId additionally collides with the agent-space and pack ids above.
Fix: the CreateTargetDomain handler - allocate the target-domain id from the shared unique-id generator, and generate the verification token per target domain when the domain record is created, storing it on that record so VerifyTargetDomain compares against the domain's own token; the dnsTxt and httpRoute tokens are per-domain state, not serializer constants.
Traces: securityagent-target-domain-tagging-and-errors#setup#1.

Related open issues (none of which own these steps): 20261029T203228Z-securityagent-batchgetagentspaces-returns-stale-incomplete-agent-space-state.md owns the BatchGetAgentSpaces read-path divergences, and 20261029T203228Z-securityagent-error-messages-diverge-from-aws-wording.md owns the error-wording divergences; neither covers identifier or verification-token generation on the create paths.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 105 steps, 27 passed). Four net-new divergence clusters, all verdict=mismatch with expected status == actual status, so only values diverge:

1) securityagent-agent-space-lifecycle#setup#1 CreateAgentSpace, 200 -> 200:
   $.id: value_mismatch (expected '{{agent_space_id}}', got '00000000000000000000000000000001')
   $.agentSpaceId: value_mismatch (expected '{{agent_space_id}}', got '00000000000000000000000000000001')

2) securityagent-artifact-batch-and-error-paths#task#1 CreateAgentSpace, 200 -> 200 (cluster also covers securityagent-artifact-lifecycle#setup#1):
   $.id: value_mismatch (expected '{{agent_space_id_err}}', got '00000000000000000000000000000001')
   $.agentSpaceId: value_mismatch (expected '{{agent_space_id_err}}', got '00000000000000000000000000000001')
   $.arn: value_mismatch (expected 'arn:aws:securityagent:us-west-2:000000000000:agent-instance/{{agent_space_id_err}}', got 'arn:aws:securityagent:us-west-2:000000000000:agent-instance/00000000000000000000000000000001')

3) securityagent-security-requirement-pack-lifecycle#task#1 CreateSecurityRequirementPack, 201 -> 201:
   $.packId: value_mismatch (expected '{{pack_id}}', got '00000000000000000000000000000001')

4) securityagent-target-domain-tagging-and-errors#setup#1 CreateTargetDomain, 200 -> 200:
   $.targetDomainId: value_mismatch (expected '{{target_domain_id}}', got '00000000000000000000000000000001')
   $.verificationDetails.httpRoute.token: value_mismatch (expected '{{http_route_token_2}}', got '0778a98ec3c7bfd032e1266778cc3d244a3db4cc51b7638568e2a01afaffd780')

Cross-cluster evidence that these are constants and not per-run values: the id literal '00000000000000000000000000000001' is returned identically for three DIFFERENT resource types across four different fixtures, and the token literal '0778a98ec3c7bfd032e1266778cc3d244a3db4cc51b7638568e2a01afaffd780' is served for two different domain names (the tagging fixture's domain and sim2real-probe-securityagent-assocdom-*.example.com in securityagent-agent-space-target-domain-association). The specific literals above are this run's values; the requirement is uniqueness per resource, not any particular string.

Model basis: Smithy CreateAgentSpaceOutput.agentSpaceId @required (AgentSpaceId, 'Unique identifier of the agent space'); CreateSecurityRequirementPackOutput.packId @required (SecurityRequirementPackId), http code 201; CreateTargetDomainOutput.targetDomainId @required (TargetDomainId, 'Unique identifier of the target domain'), verificationDetails.dnsTxt.token / httpRoute.token ('The verification token to include in the DNS record value' / 'to serve at the specified route path').

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and inspect these four steps:
   - securityagent-agent-space-lifecycle, step setup#1 (CreateAgentSpace)
   - securityagent-artifact-batch-and-error-paths, step task#1 (CreateAgentSpace)
   - securityagent-security-requirement-pack-lifecycle, step task#1 (CreateSecurityRequirementPack)
   - securityagent-target-domain-tagging-and-errors, step setup#1 (CreateTargetDomain)
3. Equivalently, by hand against the emulator's data listener:
   a. POST /CreateAgentSpace {"name":"probe-space-a"} then POST /CreateAgentSpace {"name":"probe-space-b"} and compare agentSpaceId/id/arn of the two responses -> observe the colliding/constant id.
   b. POST /CreateSecurityRequirementPack {"name":"probe-pack"} -> observe packId equals the agent-space id from (a).
   c. POST /CreateTargetDomain {"targetDomainName":"probe-one.example.com","verificationMethod":"HTTP_ROUTE"} then again with "probe-two.example.com" -> observe both responses carry the same targetDomainId and the same verificationDetails.httpRoute.token.
4. Expected: each call returns an identifier unique to the resource it created (unique across resource types too), and each target domain gets its own freshly generated verification token. Observed: a single constant id for all three resource types and a single constant verification token for all domains.
