Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: securityagent.batch-delete-aws-managed-security-requirement-rejected
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent.batch-delete-aws-managed-security-requirement-rejected|run#6|AssertionError|raise AssertionError(
Issue-Commit: 99dd7eb41b296722799c7b8575de84efccd4dcdf

# AWS-managed security requirement packs are seeded with zero security requirements

> _Note: this issue was identified against the code base at commit `99dd7eb41b296722799c7b8575de84efccd4dcdf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSecurityRequirements

## Description
One net-new root cause.

Cause 1 of 1: the emulator seeds the AWS-managed security requirement pack catalog as pack records only — it never seeds the security requirements those packs ship with — so every read of an AWS-managed pack's requirement contents reports an empty pack. Failing check: securityagent.batch-delete-aws-managed-security-requirement-rejected.

AWS: `ListSecurityRequirements` is documented as "Lists security requirements within a pack" and returns the `@required` member `securityRequirementSummaries` (`SecurityRequirementSummaryList`), whose entries are `SecurityRequirementSummary {packId, name, description, createdAt, updatedAt}` (all `@required`). `ManagementType` is the enum `[AWS_MANAGED, CUSTOMER_MANAGED]`, and `CreateSecurityRequirementPack` documents that it creates a *customer managed* pack — so an `AWS_MANAGED` pack is one AWS itself ships, content included. Recorded behavior only for the contents: the model does not enumerate which requirements AWS ships, but the recording this check was taken from shows the AWS-managed pack reported by `ListSecurityRequirementPacks` (the enabled base pack, `managementType=AWS_MANAGED`) answering `ListSecurityRequirements` with at least one `SecurityRequirementSummary`, which is what makes an AWS-managed requirement name available to probe the protected-pack rejection with. An AWS-managed pack that lists no requirements is not a state real AWS presents.

Emulator: `ListSecurityRequirements(packId=<the seeded AWS_MANAGED pack>)` returns HTTP 200 with `securityRequirementSummaries: []`. The pack itself resolves (so the call is not a 404) but holds nothing. The replay fails at the point where it needs an AWS-managed requirement name: "AWS_MANAGED pack srp-am-base has no security requirements to probe the protected-pack rejection against." The cause is in the state seed: `ComAmazonawsSecurityagentSecurityagentState::default()` seeds `security_requirement_packs` from the `AWS_MANAGED_PACKS` catalog table but initialises the requirement store empty, so no requirement is ever associated with an AWS-managed packId. `ListSecurityRequirements` itself is implemented correctly and simply walks that empty store.

Why: a client that reads the AWS-managed catalog to discover the requirements AWS ships — to display them, to evaluate against them, or to assert that AWS-managed content cannot be mutated — sees an enabled AWS-managed pack with nothing in it. It concludes the pack is empty rather than that the emulator did not seed it, and any flow that selects a requirement name out of an AWS-managed pack (read, batch-get, or an attempted modification that AWS must refuse) has nothing to select and cannot run at all. The whole protected-content behavior of AWS-managed packs is unreachable.

Fix: the service's seeded state — the AWS-managed catalog in `crate::state` (the `AWS_MANAGED_PACKS` table and `ComAmazonawsSecurityagentSecurityagentState::default()`), which must seed the requirements each AWS-managed pack ships keyed by `(packId, name)` in the same requirement store `BatchCreateSecurityRequirements` writes and `ListSecurityRequirements`/`BatchGetSecurityRequirements` read, so the seeded entries are projected through exactly the same path as customer-created ones (full `SecurityRequirementSummary`: packId, name, description, createdAt, updatedAt).

Note for the fixer: once requirements exist for an AWS-managed pack, the rejection this check goes on to assert becomes reachable. AWS refuses `BatchDeleteSecurityRequirements` against an AWS-managed pack at the call level with `ValidationException` (HTTP 400) and the message `Only customer managed packs support deleting security requirements. Pack ID: <packId>` — fixed text `Only customer managed packs support deleting security requirements. Pack ID: ` plus the `packId` taken verbatim from the request. The state helper currently present (`aws_managed_pack_conflict_message`, which words it `Security requirements cannot be modified for AWS managed pack: {pack_id}`) does not match that wording. `BatchDeleteSecurityRequirements` is still an unimplemented scaffold stub and is already filed as 20261108T031300Z-securityagent-batchdeletesecurityrequirements-stub.md; this issue covers only the missing seeded content, not that stub.

Traces: securityagent.batch-delete-aws-managed-security-requirement-rejected.

## Evidence
Check: securityagent.batch-delete-aws-managed-security-requirement-rejected [mismatch]

Expected (recorded AWS): ListSecurityRequirements(packId=<AWS_MANAGED pack from ListSecurityRequirementPacks>) returns a non-empty securityRequirementSummaries list, each entry a SecurityRequirementSummary with packId/name/description/createdAt/updatedAt.

Actual (emulator): HTTP 200 with securityRequirementSummaries == [] for the seeded AWS_MANAGED pack, producing:
  AssertionError: AWS_MANAGED pack srp-am-base has no security requirements to probe the protected-pack rejection against.
  (at check_securityagent_batch_delete_aws_managed_security_requirement_rejected.py line 33)

Source of the divergence (citations):
  services/service-com-amazonaws-securityagent-securityagent/src/state.rs:371 — AWS_MANAGED_PACKS catalog table: pack identity/name/status only, no requirement content.
  services/service-com-amazonaws-securityagent-securityagent/src/state.rs:1338-1363 — Default::default() seeds security_requirement_packs from AWS_MANAGED_PACKS but leaves the requirement store empty.
  services/service-com-amazonaws-securityagent-securityagent/src/ops/list_security_requirements.rs:262 — handler walks state.security_requirements_from(packId, ..), which yields nothing for a seeded AWS-managed packId.

Smithy: ListSecurityRequirements doc "Lists security requirements within a pack"; output member securityRequirementSummaries (SecurityRequirementSummaryList) @required; SecurityRequirementSummary members packId/name/description/createdAt/updatedAt all @required; ManagementType enum [AWS_MANAGED, CUSTOMER_MANAGED]; CreateSecurityRequirementPack documents creating a *customer managed* pack.

## Steps to Reproduce
1. Boot the emulator (fresh state).
2. sa = boto3 client("securityagent")
3. packs = sa.list_security_requirement_packs()["securityRequirementPackSummaries"]; pick the first pack with managementType == "AWS_MANAGED" (the emulator seeds e.g. the enabled base pack).
4. reqs = sa.list_security_requirements(packId=<that packId>)["securityRequirementSummaries"]
5. Observe reqs == [] — real AWS returns the requirements the AWS-managed pack ships, so there is a requirement name available at this step.
6. Consequently any follow-on call that needs an AWS-managed requirement name (e.g. BatchGetSecurityRequirements or the BatchDeleteSecurityRequirements protected-pack rejection) cannot be issued.
