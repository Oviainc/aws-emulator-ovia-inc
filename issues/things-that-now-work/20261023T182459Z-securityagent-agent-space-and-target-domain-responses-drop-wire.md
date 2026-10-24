Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-agent-space-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-agent-space-lifecycle securityagent-artifact-batch-and-error-paths securityagent-artifact-lifecycle securityagent-agent-space-errors securityagent-agent-space-target-domain-association securityagent-target-domain-lifecycle securityagent-target-domain-tagging-and-errors
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.agentSpaceId
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.arn
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.createdAt
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.id
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.ownerId
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.updatedAt
Issue-Key: securityagent|BatchGetAgentSpaces|2xx->2xx|$.agentInstances
Issue-Key: securityagent|BatchGetAgentSpaces|2xx->2xx|$.ids
Issue-Key: securityagent|CreateTargetDomain|2xx->2xx|$.createdAt
Issue-Key: securityagent|CreateTargetDomain|2xx->2xx|$.status
Issue-Key: securityagent|CreateTargetDomain|2xx->2xx|$.verificationDetails.dnsTxt.dnsRecordName
Issue-Key: securityagent|CreateTargetDomain|2xx->2xx|$.verificationStatusReason
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.targetDomains
Issue-Key: securityagent|BatchGetTargetDomains|2xx->2xx|$.targetDomains[*].createdAt
Issue-Key: securityagent|BatchGetTargetDomains|2xx->2xx|$.targetDomains[*].status
Issue-Key: securityagent|BatchGetTargetDomains|2xx->2xx|$.targetDomains[*].verificationDetails.dnsTxt.dnsRecordName
Issue-Key: securityagent|BatchGetTargetDomains|2xx->2xx|$.targetDomains[*].verificationStatusReason
Issue-Key: securityagent|CreateTargetDomain|2xx->2xx|$.targetDomainId
Issue-Key: securityagent|CreateTargetDomain|2xx->2xx|$.verificationDetails.httpRoute.routePath
Issue-Key: securityagent|CreateTargetDomain|2xx->2xx|$.verificationDetails.httpRoute.token
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# Agent-space and target-domain responses drop wire members and serialize timestamps as epoch numbers

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentSpace

## Description
SIX causes, all in the securityagent response-projection/serialization path for agent spaces and target domains. Every step below answered the correct HTTP status; the bodies diverge. A fixer must close all six before submitting.

Cause 1 of 6 - CreateAgentSpace response omits id/ownerId/arn and renders timestamps as epoch numbers.
AWS: recorded behavior only for the extra identity members - the Smithy model (CreateAgentSpaceOutput: agentSpaceId, name, description, awsResources, targetDomainIds, codeReviewSettings, kmsKeyId, createdAt, updatedAt) does not declare them, but the recorded 200 additionally carries `id` (the same value as agentSpaceId), `ownerId` (the caller's 12-digit account id) and `arn` of the form arn:aws:securityagent:<region>:<account>:agent-instance/<agentSpaceId> (arnNamespace `securityagent`, per the model's awsService trait). createdAt and updatedAt are Timestamp members carrying @timestampFormat date-time (Smithy: CreateAgentSpaceOutput.createdAt/updatedAt), so under restJson1 they serialize as ISO-8601 instant strings, not numbers.
Emulator: the body carries no `id`, no `ownerId` and no `arn`, and createdAt/updatedAt are emitted as epoch-second floats (the replay shows a bare number where an ISO-8601 instant is expected).
Why: a client that reads the agent space's ARN or owning account from the create response gets nothing to read, and an SDK whose generated deserializer expects a date-time string for these members fails to parse a number - so the create response is unusable even though the call "succeeded".
Fix: CreateAgentSpace's handler plus the shared agent-space output projection, and the restJson1 output serializer, which must honour a member's @timestampFormat instead of defaulting to epoch-seconds.
Traces: securityagent-agent-space-lifecycle#setup#1 (same cluster: securityagent-artifact-batch-and-error-paths#task#1, securityagent-artifact-lifecycle#setup#1). Components: services/service-com-amazonaws-securityagent-securityagent/src/ops/create_agent_space.rs and services/service-com-amazonaws-securityagent-securityagent/src/state.rs.

Cause 2 of 6 - CreateAgentSpace with targetDomainIds omits ownerId and the embedded target-domain records.
AWS: recorded behavior only - the recorded 200 for a create that names target domains carries `ownerId` and a `targetDomains` array holding the full record for each associated id (targetDomainId, domainName, createdAt, verifiedAt, status, verificationStatus, verificationStatusReason, verificationDetails), i.e. the association is echoed resolved, not just as ids. createdAt/updatedAt again per @timestampFormat date-time.
Emulator: `ownerId` and `targetDomains` are absent entirely, and createdAt/updatedAt are epoch floats.
Why: a client that associates domains at create time cannot confirm from the response which domains were attached or what challenge each one is waiting on, and must guess that the association happened.
Fix: CreateAgentSpace's handler and the agent-space projection - it must resolve targetDomainIds against the stored target-domain records and emit them.
Traces: securityagent-agent-space-target-domain-association#setup#2.

Cause 3 of 6 - BatchGetAgentSpaces answers an empty body instead of its two list members.
AWS: recorded behavior only for the wire names - the recorded 200 for a batch get that matched nothing carries BOTH list members present as empty arrays, under the wire names `agentInstances` and `ids` (the model names the members agentSpaces and notFound on BatchGetAgentSpacesOutput and sets no @jsonName, so the recording is the only evidence of the wire names).
Emulator: the body carries neither of the recorded members and no member of either name - both are simply omitted when empty.
Why: a client that iterates the found list and the not-found list unconditionally sees absent members rather than empty arrays, so it cannot distinguish "nothing matched" from "the service did not answer this part of the request"; strict deserializers treat the missing list as null and fault.
Fix: BatchGetAgentSpaces' handler/serializer - emit both lists always, with the recorded wire names.
Traces: securityagent-agent-space-errors#task#4. Component: services/service-com-amazonaws-securityagent-securityagent/src/ops/batch_get_agent_spaces.rs.

Cause 4 of 6 - CreateTargetDomain (DNS_TXT) omits `status`, names the challenge record with the wrong label, and renders createdAt as a number.
AWS: recorded behavior only - the recorded 200 carries a `status` member ('PENDING') alongside the modeled `verificationStatus`, and verificationDetails.dnsTxt.dnsRecordName is the requested domain prefixed with the label `_aws_securityagent-challenge.` (the model pins only the record TYPE: DNSRecordType admits TXT alone). verificationStatusReason is AWS's fixed sentence for a pending DNS_TXT domain, recorded as 'Target domain is pending ownership verification. Add the DNS TXT record to the DNS resolver associated with your domain before attempting verification' (one constant per verification method; the text above is the recorded example). createdAt carries @timestampFormat date-time (Smithy: CreateTargetDomainOutput.createdAt).
Emulator: no `status` member at all; the record name uses the label `_securityagent-challenge.` - which state.rs itself documents as CHOSEN, not cited (DNS_CHALLENGE_LABEL) - a different reason string, and createdAt as an epoch float.
Why: the DNS record name is the actionable part of the response: a client that publishes the TXT record the emulator names publishes it at a name AWS would never query, so domain verification can never succeed against real AWS with the same client code. Clients that branch on `status` see it missing.
Fix: CreateTargetDomain's state projection (the challenge-name derivation and the pending-reason text) and the target-domain output projection; plus the timestamp serializer as in Cause 1.
Traces: securityagent-agent-space-target-domain-association#setup#1 (same cluster: securityagent-target-domain-lifecycle#setup#1). Components: services/service-com-amazonaws-securityagent-securityagent/src/state.rs (DNS_CHALLENGE_LABEL, line ~123) and src/ops/create_target_domain.rs.

Cause 5 of 6 - CreateTargetDomain (HTTP_ROUTE) returns the wrong challenge route path.
AWS: recorded behavior only - verificationDetails.httpRoute.routePath is the fixed, token-free relative path '.well-known/aws/securityagent-domain-verification.json', with the token carried separately in verificationDetails.httpRoute.token; verificationStatusReason is AWS's fixed HTTP_ROUTE sentence ('Target domain is pending ownership verification. Add the HTTP route path and token directly under your configured domain before attempting verification', recorded example), and the response again carries `status` ('PENDING') and an ISO-8601 createdAt.
Emulator: routePath is '/.well-known/securityagent-domain-verification/<token>' - absolute, per-domain, with the token embedded as the last path segment (state.rs documents HTTP_CHALLENGE_ROUTE_PREFIX as CHOSEN, not cited) - `status` is absent and createdAt is an epoch float.
Why: same failure mode as Cause 4 on the HTTP path - a client serves the verification document at a path AWS does not fetch, so ownership verification is unreachable; the shape of the path (fixed .json file vs token-in-path) also changes how a client wires its web server.
Fix: the HTTP_ROUTE branch of CreateTargetDomain's challenge derivation in the service state, and the target-domain output projection.
Traces: securityagent-target-domain-tagging-and-errors#setup#1. Component: services/service-com-amazonaws-securityagent-securityagent/src/state.rs (HTTP_CHALLENGE_ROUTE_PREFIX, line ~136).

Cause 6 of 6 - BatchGetTargetDomains repeats the projection defects on the read path.
AWS: the read-back of a pending DNS_TXT domain carries, per entry, `status` ('PENDING'), the `_aws_securityagent-challenge.<domain>` record name, AWS's pending reason text, and createdAt as an ISO-8601 instant (Smithy: TargetDomain.createdAt @timestampFormat date-time).
Emulator: targetDomains[0] has no `status`, the `_securityagent-challenge.<domain>` record name, its own reason text and an epoch-float createdAt - i.e. the same projection as the create path, so fixing only the create response would leave the read path diverging.
Why: a client that re-reads a domain to find the challenge to publish gets the wrong record name on every subsequent read as well.
Fix: the shared TargetDomain projection used by BatchGetTargetDomains (one fix point with Causes 4 and 5).
Traces: securityagent-target-domain-lifecycle#task#3. Component: services/service-com-amazonaws-securityagent-securityagent/src/ops/batch_get_target_domains.rs.

## Evidence
Cause 1 (securityagent-agent-space-lifecycle#setup#1, CreateAgentSpace, 200 vs 200): expected $.id = the created agent space id, got missing; expected $.ownerId = '000000000000' (12-digit account), got missing; expected $.arn = 'arn:aws:securityagent:us-west-2:000000000000:agent-instance/<id>', got missing; expected $.createdAt/$.updatedAt = '2026-09-06T21:00:56.418651+00:00' (ISO-8601), got 1792779620.5213242 (epoch float).
Cause 2 (securityagent-agent-space-target-domain-association#setup#2, CreateAgentSpace, 200 vs 200): expected $.ownerId = '000000000000', got missing; expected $.targetDomains = [ {targetDomainId, domainName, createdAt, verifiedAt, status:'PENDING', verificationStatus:'PENDING', verificationStatusReason, verificationDetails{...}} ], got missing; expected $.createdAt/$.updatedAt ISO-8601, got 1792779621.2147644.
Cause 3 (securityagent-agent-space-errors#task#4, BatchGetAgentSpaces, 200 vs 200): expected $.agentInstances = [] and $.ids = [], got None for both (no such members, and none of the modeled agentSpaces/notFound reported as extra either).
Cause 4 (securityagent-agent-space-target-domain-association#setup#1, CreateTargetDomain, 200 vs 200): expected $.status = 'PENDING', got missing; expected $.verificationDetails.dnsTxt.dnsRecordName = '_aws_securityagent-challenge.<domain>', got '_securityagent-challenge.<domain>'; expected $.createdAt = '2026-09-06T21:01:01.588372+00:00', got 1792779621.199954; verificationStatusReason expected AWS's pending-DNS sentence, got 'Awaiting domain ownership verification: publish a TXT record named ...'.
Cause 5 (securityagent-target-domain-tagging-and-errors#setup#1, CreateTargetDomain, 200 vs 200): expected $.verificationDetails.httpRoute.routePath = '.well-known/aws/securityagent-domain-verification.json', got '/.well-known/securityagent-domain-verification/<token>'; expected $.status = 'PENDING', got missing; expected $.createdAt ISO-8601, got 1792779630.641294.
Cause 6 (securityagent-target-domain-lifecycle#task#3, BatchGetTargetDomains, 200 vs 200): expected $.targetDomains[0].status = 'PENDING', got missing; expected $.targetDomains[0].verificationDetails.dnsTxt.dnsRecordName = '_aws_securityagent-challenge.<domain>', got '_securityagent-challenge.<domain>'; expected $.targetDomains[0].createdAt ISO-8601, got 1792779629.9134464.
Model cited via smithy_query on services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json: protocol restJson1; CreateAgentSpaceOutput/CreateTargetDomainOutput/TargetDomain timestamp members all carry timestampFormat=date-time; BatchGetAgentSpacesOutput members are agentSpaces/notFound with no jsonName; ResourceConfigurationId and the challenge label/path are unconstrained in the model (recorded behavior only).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus, no arguments).
3. Cause 1: fixture securityagent-agent-space-lifecycle, step setup#1 - POST /CreateAgentSpace and compare the 200 body's id/ownerId/arn/createdAt/updatedAt against the recording.
4. Cause 2: fixture securityagent-agent-space-target-domain-association, step setup#2 - CreateAgentSpace naming targetDomainIds; check ownerId and targetDomains.
5. Cause 3: fixture securityagent-agent-space-errors, step task#4 - POST /BatchGetAgentSpaces with ids that match nothing; check that both list members are present as empty arrays.
6. Cause 4: fixture securityagent-agent-space-target-domain-association, step setup#1 - POST /CreateTargetDomain with verificationMethod DNS_TXT; check status and verificationDetails.dnsTxt.dnsRecordName.
7. Cause 5: fixture securityagent-target-domain-tagging-and-errors, step setup#1 - CreateTargetDomain with verificationMethod HTTP_ROUTE; check verificationDetails.httpRoute.routePath.
8. Cause 6: fixture securityagent-target-domain-lifecycle, step task#3 - POST /BatchGetTargetDomains for the created domain; check the same three fields on targetDomains[0].
