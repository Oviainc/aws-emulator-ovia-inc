Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-catalog-list-security-requirements
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-catalog-list-security-requirements
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|ListSecurityRequirements|2xx->2xx|$.securityRequirementSummaries[*]
Issue-Commit: 77fc38481d0944218df9b5c2964437ecad54bbf8

# AWS-managed security requirement packs return no requirements on the wire

> _Note: this issue was identified against the code base at commit `77fc38481d0944218df9b5c2964437ecad54bbf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecurityRequirements

## Description
One net-new root cause, observed on all four AWS-managed packs the catalog advertises.

Cause 1 of 1: the emulator seeds the AWS-managed security requirement pack catalog as pack records only and never seeds the requirements those packs ship with, so ListSecurityRequirements on any AWS-managed packId answers HTTP 200 with an empty securityRequirementSummaries list. The corpus exercises one pack per step: srp-am-base (task#1), srp-am-nist-csf (task#2), srp-am-pci-dss (task#3), srp-am-well-architected (task#4), and all four come back empty.

AWS: Smithy: ListSecurityRequirements (POST /ListSecurityRequirements, 200) takes packId (SecurityRequirementPackId, @required) and returns securityRequirementSummaries (SecurityRequirementSummaryList, @required) plus nextToken; it is @paginated on items=securityRequirementSummaries, and @readonly. SecurityRequirementSummary has packId, name, description, createdAt and updatedAt, ALL @required (createdAt/updatedAt @timestampFormat date-time). The doc for the operation is "Lists security requirements within a pack". Recorded behavior only for the contents: the model does not enumerate what AWS ships, and the recording shows each AWS-managed pack answering with a populated list of SecurityRequirementSummary entries whose packId is the requested AWS-managed pack id and whose createdAt/updatedAt are the pack content's fixed publication timestamps (the same value on every entry of a pack, not a per-run clock reading). The requirement names and descriptions are the catalog content of that pack, not per-run values.

Emulator: every one of the four calls returns HTTP 200 with securityRequirementSummaries == [] (the replay reports each expected entry as `missing (expected {...}, got None)`). The pack itself resolves, so the call is not a 404 — the pack is simply empty.

Why: a client that reads the AWS-managed catalog to discover, display or evaluate against the requirements AWS ships sees four enabled AWS-managed packs with no content, and concludes the packs are empty rather than unseeded. Any flow that has to pick a requirement name out of an AWS-managed pack (BatchGetSecurityRequirements against managed content, or asserting that managed content cannot be mutated) has nothing to select and cannot run at all. Pagination over managed packs is likewise unreachable.

Fix: the service's seeded state — the AWS-managed catalog in crate::state (the AWS_MANAGED_PACKS table and ComAmazonawsSecurityagentSecurityagentState::default()), which must seed each AWS-managed pack's requirements into the same requirement store BatchCreateSecurityRequirements writes and ListSecurityRequirements/BatchGetSecurityRequirements read, keyed by (packId, name), with the pack content's fixed createdAt/updatedAt rather than the boot clock, so the seeded entries project through the same path as customer-created ones. The ListSecurityRequirements handler itself is correct and merely walks an empty store.

Related open issue: 20261108T073854Z-securityagent-aws-managed-security-requirement-packs-are-seeded-with reports the same seed gap from the BEHAVIORAL corpus; it carries no wire replay key for these fixture steps (the replay reports owned_by empty for all four), so this issue is what binds the gap to the wire corpus. It must be re-replayed against these four steps, not just the behavioral check.

Traces: securityagent-catalog-list-security-requirements#task#1 (srp-am-base), #task#2 (srp-am-nist-csf), #task#3 (srp-am-pci-dss), #task#4 (srp-am-well-architected).

## Evidence
Fixture securityagent-catalog-list-security-requirements, verdict mismatch on four steps, expected status 200 / actual status 200 in each case.

task#1 ListSecurityRequirements(packId=srp-am-base): expected securityRequirementSummaries[0..9+] = entries such as {packId: 'srp-am-base', name: 'audit-logging-best-practices', description: 'Ensure the system supports security monitoring.', createdAt: '2025-12-01T00:00:00Z', updatedAt: '2025-12-01T00:00:00Z'}; actual: each index reported `missing (... got None)` — the list is empty.

task#2 (packId=srp-am-nist-csf): expected 37 summaries (indices 0..36), e.g. name 'adverse-event-analysis-and-threat-intelligence-integration' / 'ai-component-risk-assessment-and-security-integration' with createdAt=updatedAt='2026-05-11T21:16:07Z'; actual empty.

task#3 (packId=srp-am-pci-dss): expected 50 summaries (indices 0..49), e.g. 'account-data-classification-and-retention-controls', 'anti-malware-protection-for-workload-components' with createdAt=updatedAt='2026-06-15T19:28:54Z'; actual empty.

task#4 (packId=srp-am-well-architected): expected 32 summaries (indices 0..31), e.g. 'authenticated-network-communication-architecture', 'automated-certificate-management-architecture' with createdAt=updatedAt='2026-05-11T21:16:07Z'; actual empty.

Smithy: ListSecurityRequirements output securityRequirementSummaries @required (SecurityRequirementSummaryList), @paginated items=securityRequirementSummaries; SecurityRequirementSummary.packId/name/description/createdAt/updatedAt all @required.

Citations: services/service-com-amazonaws-securityagent-securityagent/src/state.rs (AWS_MANAGED_PACKS table and Default::default(), which seed pack records only); services/service-com-amazonaws-securityagent-securityagent/src/ops/list_security_requirements.rs (handler walks the empty requirement store).

## Steps to Reproduce
1. Boot the emulator from this checkout (fresh state).
2. sa = boto3.client("securityagent", endpoint_url=<emulator>)
3. packs = sa.list_security_requirement_packs()["securityRequirementPackSummaries"] — note the four AWS_MANAGED pack ids (srp-am-base, srp-am-nist-csf, srp-am-pci-dss, srp-am-well-architected).
4. For each of those pack ids: sa.list_security_requirements(packId=<packId>).
5. Observe HTTP 200 with securityRequirementSummaries == [] for every one. Real AWS returns the requirement set that pack ships, each entry a full SecurityRequirementSummary with the pack content's fixed createdAt/updatedAt.
6. Replay fixture securityagent-catalog-list-security-requirements steps task#1..task#4 to confirm.
