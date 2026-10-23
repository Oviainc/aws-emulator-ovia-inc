Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-security-requirement-pack-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-security-requirement-pack-error-paths securityagent-security-requirement-pack-lifecycle securityagent-agent-space-target-domain-association securityagent-target-domain-lifecycle securityagent-catalog-list-security-requirement-packs securityagent-private-connection-error-paths
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|GetSecurityRequirementPack|4xx->2xx|$.message
Issue-Key: securityagent|GetSecurityRequirementPack|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: securityagent|GetSecurityRequirementPack|4xx->2xx|status
Issue-Key: securityagent|BatchGetTargetDomains|2xx->2xx|$.notFound[*]
Issue-Key: securityagent|BatchGetTargetDomains|2xx->2xx|$.targetDomains[*]
Issue-Key: securityagent|GetSecurityRequirementPack|2xx->2xx|$.description
Issue-Key: securityagent|GetSecurityRequirementPack|2xx->2xx|$.status
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="sim2real-probe-securityagent-secpack-0aa03782"].createdAt
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="sim2real-probe-securityagent-secpack-0aa03782"].description
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="sim2real-probe-securityagent-secpack-0aa03782"].status
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[name="sim2real-probe-securityagent-secpack-0aa03782"].updatedAt
Issue-Key: securityagent|ListSecurityRequirementPacks|2xx->2xx|$.securityRequirementPackSummaries[*]
Issue-Key: securityagent|CreatePrivateConnection|4xx->4xx|$.message
Issue-Key: securityagent|CreatePrivateConnection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: securityagent|CreatePrivateConnection|4xx->4xx|status
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# SecurityRequirementPack and target-domain writes not reflected on read, catalog unseeded, malformed id refused as not-found

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecurityRequirementPack

## Description
SIX causes in the securityagent state and refusal paths: deletes and updates are not reflected on read, the AWS-managed pack catalog is not seeded, and a malformed identifier is refused as a missing resource. A fixer must close all six before submitting.

Cause 1 of 6 - GetSecurityRequirementPack on a deleted pack answers 200 with the pack instead of 404.
AWS: once DeleteSecurityRequirementPack has removed a pack, GetSecurityRequirementPack refuses the id with the modeled ResourceNotFoundException (Smithy: GetSecurityRequirementPack errors include ResourceNotFoundException, client error, httpError 404). The recorded refusal sets header x-amzn-errortype: ResourceNotFoundException and a message naming the requested id, recorded as 'No security requirement pack found for ID: <packId>' - request-derived, so the rule is "name the packId that was not found" (the text above is the recorded example).
Emulator: answers HTTP 200 with a pack body, no message and no x-amzn-errortype header - the delete did not remove the record (or the read does not consult deletion).
Why: a client cannot tell a deleted pack from a live one; delete appears to succeed and then the resource is still readable, so cleanup logic loops or double-frees, and error handling built on ResourceNotFoundException never fires.
Fix: the DeleteSecurityRequirementPack/GetSecurityRequirementPack pair - the pack store must drop the record on delete and the read handler must raise ResourceNotFoundException for an unknown id.
Traces: securityagent-security-requirement-pack-error-paths#task#3 (same cluster: securityagent-security-requirement-pack-lifecycle#teardown#7). Components: services/service-com-amazonaws-securityagent-securityagent/src/ops/get_security_requirement_pack.rs and src/state.rs.

Cause 2 of 6 - BatchGetTargetDomains still returns a deleted target domain and leaves notFound empty.
AWS: after the domain is deleted, its id is unknown, so the batch read returns it in `notFound` and omits it from `targetDomains` (Smithy: BatchGetTargetDomainsOutput.notFound -> TargetDomainIdList, documented as "the list of target domain identifiers that were not found").
Emulator: targetDomains still carries the full deleted record and notFound is empty - the same stale-after-delete fault as Cause 1, on the target-domain store.
Why: a teardown that deletes a domain and then verifies the deletion by batch-get concludes the domain still exists; a client can also keep acting on a domain AWS has already removed.
Fix: DeleteTargetDomain's state mutation and BatchGetTargetDomains' found/not-found partitioning.
Traces: securityagent-agent-space-target-domain-association#teardown#8 (same cluster: securityagent-target-domain-lifecycle#teardown#7). Components: src/ops/batch_get_target_domains.rs, src/state.rs.

Cause 3 of 6 - UpdateSecurityRequirementPack's description and status are not persisted; GetSecurityRequirementPack serves the pre-update values.
AWS: UpdateSecurityRequirementPack takes description and status (Smithy: UpdateSecurityRequirementPackInput.description -> String, .status -> SecurityRequirementPackStatus) and the mutation is durable, so a following GetSecurityRequirementPack returns the updated description and status.
Emulator: after an update that set a new description and status DISABLED, the read returns the original description and status ENABLED - the update response is accepted but nothing is written to the pack record.
Why: every read-after-write on a pack is wrong, so a client that disables a pack still sees it enabled and will treat it as in force.
Fix: UpdateSecurityRequirementPack's handler - it must apply the mutable members to the stored record (and advance updatedAt).
Traces: securityagent-security-requirement-pack-lifecycle#task#4. Component: src/ops/update_security_requirement_pack.rs.

Cause 4 of 6 - ListSecurityRequirementPacks shows the same stale fields and emits summary timestamps as epoch numbers.
AWS: the listing projects the stored record, so the updated description and status DISABLED appear in the summary; createdAt and updatedAt are required Timestamp members carrying @timestampFormat date-time (Smithy: SecurityRequirementPackSummary.createdAt/updatedAt), so under restJson1 they serialize as ISO-8601 instant strings, and updatedAt reflects the update (it is later than createdAt in the recording).
Emulator: the summary carries the pre-update description and ENABLED, and both timestamps are epoch-second floats, identical to each other.
Why: the listing is the normal way to enumerate packs, so the stale projection is visible to every client; and an SDK whose generated deserializer expects a date-time string for these required members cannot parse a number, so the whole page fails to deserialize.
Fix: the pack-summary projection (shared with Cause 3's state write) and the restJson1 timestamp serializer, which must honour the member's @timestampFormat.
Traces: securityagent-security-requirement-pack-lifecycle#task#5. Components: src/ops/list_security_requirement_packs.rs, src/state.rs.

Cause 5 of 6 - the AWS-managed security requirement pack catalog is not seeded, so ListSecurityRequirementPacks is empty on a fresh account.
AWS: recorded behavior only (the model declares the shapes but not the catalog contents) - a fresh account lists four AWS_MANAGED packs with vendorName 'Amazon Web Services' and managementType AWS_MANAGED (Smithy: ManagementType admits AWS_MANAGED): packId srp-am-base ('ASA Base Pack') with status ENABLED, and srp-am-nist-csf ('NIST CSF Pack'), srp-am-pci-dss ('PCI DSS Pack') and srp-am-well-architected ('AWS Well-Architected Pack') with status DISABLED, each carrying createdAt/updatedAt.
Emulator: the listing returns none of the four - securityRequirementPackSummaries holds no AWS-managed entry at all.
Why: a client that enables or references a built-in pack by its well-known id cannot find it, so any flow that starts from the managed catalog (the common case - the base pack is enabled by default on AWS) has nothing to work with.
Fix: the service state's seeded pack catalog, read by ListSecurityRequirementPacks and GetSecurityRequirementPack.
Traces: securityagent-catalog-list-security-requirement-packs#task#1. Component: src/state.rs (pack store initialisation).

Cause 6 of 6 - CreatePrivateConnection answers 404 ResourceNotFoundException for a malformed resourceConfigurationId where AWS answers 400 ValidationException.
AWS: a mode.selfManaged.resourceConfigurationId that is not a VPC Lattice resource-configuration id or ARN is a syntactic violation, refused at validation with HTTP 400 and x-amzn-errortype: ValidationException (Smithy: CreatePrivateConnection errors include both ValidationException, httpError 400, and ResourceNotFoundException, httpError 404; ResourceConfigurationId itself carries no @pattern, so the id grammar is recorded behavior only). The recorded message is request-derived - '1 validation error detected: Value \'<value>\' at \'mode.selfManaged.resourceConfigurationId\' failed to satisfy constraint: Member must satisfy regular expression pattern: ((rcfg-[0-9a-z]{17})|(arn:[a-z0-9\\-]+:vpc-lattice:[a-zA-Z0-9\\-]+:\\d{12}:resourceconfiguration/rcfg-[0-9a-z]{17}))' - so the rule is "name the member and the pattern it missed"; the text above is the recorded example. The requirement is the 400 status and the ValidationException code.
Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException, reporting the id as a resource that does not exist in the account - i.e. it resolves the id before validating its syntax.
Why: the two classes mean opposite things to a client: ValidationException is a permanent caller bug to be surfaced, while ResourceNotFoundException invites a client to create the resource or retry after provisioning. SDK error handling that branches on the modeled error type takes the wrong branch, and the 404 also hides a typo as a missing resource.
Fix: CreatePrivateConnection's input validation - the resourceConfigurationId grammar must be checked (and refused as ValidationException/400) before the handler attempts to resolve the id in state.
Traces: securityagent-private-connection-error-paths#task#1. Component: src/ops/create_private_connection.rs.

## Evidence
Cause 1 (securityagent-security-requirement-pack-error-paths#task#3, GetSecurityRequirementPack): expected status 404, got 200; expected $.message = 'No security requirement pack found for ID: <packId>', got none; expected header x-amzn-errortype = 'ResourceNotFoundException', got none. Same cluster: securityagent-security-requirement-pack-lifecycle#teardown#7.
Cause 2 (securityagent-agent-space-target-domain-association#teardown#8, BatchGetTargetDomains, 200 vs 200): expected $.notFound[0] = the deleted targetDomainId, got missing; $.targetDomains[0] was extra - the full deleted record (targetDomainId, domainName, verificationStatus PENDING, verificationDetails, createdAt) was still returned. Same cluster: securityagent-target-domain-lifecycle#teardown#7.
Cause 3 (securityagent-security-requirement-pack-lifecycle#task#4, GetSecurityRequirementPack, 200 vs 200): expected $.description = 'Updated pack description', got 'Initial pack description'; expected $.status = 'DISABLED', got 'ENABLED'.
Cause 4 (securityagent-security-requirement-pack-lifecycle#task#5, ListSecurityRequirementPacks, 200 vs 200): for the pack's summary, expected description 'Updated pack description', got 'Initial pack description'; expected status 'DISABLED', got 'ENABLED'; expected createdAt '2026-09-06T21:09:36.695697630Z' and updatedAt '2026-09-06T21:09:36.882766620Z' (distinct ISO-8601 instants), got 1792779629.2809017 for both.
Cause 5 (securityagent-catalog-list-security-requirement-packs#task#1, ListSecurityRequirementPacks, 200 vs 200): expected securityRequirementPackSummaries[0..3] = the four AWS_MANAGED packs (srp-am-base/ENABLED, srp-am-nist-csf/DISABLED, srp-am-pci-dss/DISABLED, srp-am-well-architected/DISABLED, vendorName 'Amazon Web Services'), got None for all four entries.
Cause 6 (securityagent-private-connection-error-paths#task#1, CreatePrivateConnection): expected status 400 with header x-amzn-errortype = 'ValidationException' and a '1 validation error detected: Value ... at \'mode.selfManaged.resourceConfigurationId\' failed to satisfy constraint: Member must satisfy regular expression pattern: ...' message; got status 404 with x-amzn-errortype = 'ResourceNotFoundException' and a message claiming the resource was not found in the account.
Model cited via smithy_query on services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json: protocol restJson1; GetSecurityRequirementPack and CreatePrivateConnection both model ResourceNotFoundException (404) and ValidationException (400); SecurityRequirementPackSummary.createdAt/updatedAt are required with timestampFormat=date-time; BatchGetTargetDomainsOutput.notFound is the not-found id list; ManagementType admits AWS_MANAGED; ResourceConfigurationId has no pattern constraint.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus, no arguments).
3. Cause 1: fixture securityagent-security-requirement-pack-error-paths, step task#3 - delete a pack, then POST /GetSecurityRequirementPack with its id; expect 404 ResourceNotFoundException.
4. Cause 2: fixture securityagent-agent-space-target-domain-association, step teardown#8 - delete the target domain, then POST /BatchGetTargetDomains with its id; expect it in notFound and absent from targetDomains.
5. Cause 3: fixture securityagent-security-requirement-pack-lifecycle, step task#4 - create a pack, POST /UpdateSecurityRequirementPack with a new description and status DISABLED, then POST /GetSecurityRequirementPack.
6. Cause 4: same fixture, step task#5 - POST /ListSecurityRequirementPacks and inspect that pack's summary description, status, createdAt and updatedAt.
7. Cause 5: fixture securityagent-catalog-list-security-requirement-packs, step task#1 - POST /ListSecurityRequirementPacks on a fresh state and look for the four AWS_MANAGED packs.
8. Cause 6: fixture securityagent-private-connection-error-paths, step task#1 - POST /CreatePrivateConnection with mode.selfManaged.resourceConfigurationId = 'not-a-valid-resource-configuration-id'; expect 400 ValidationException.
