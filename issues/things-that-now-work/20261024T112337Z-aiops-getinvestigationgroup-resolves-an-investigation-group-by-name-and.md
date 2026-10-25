Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: aiops.get-investigation-group-by-name-and-arn-equivalent
Issue-Shape-Id: com.amazonaws.aiops#AIOps
Issue-Key: aiops.get-investigation-group-by-name-and-arn-equivalent|run#12|AssertionError|raise AssertionError(f"expected GetInvestigationGroup(identifier=<name>) to 404, it returned {result!r}")
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# GetInvestigationGroup resolves an investigation group by NAME and returns 200, where real AWS 404s the name-identifier path

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInvestigationGroup

## Description
Cause 1 of 1: GetInvestigationGroup's `identifier` name form must NOT resolve an existing investigation group.

AWS: Smithy models `GetInvestigationGroupRequest.identifier` as `@httpLabel @required` targeting `InvestigationGroupIdentifier`, whose pattern `^(?:[\-_A-Za-z0-9]{1,512}|arn:(aws|aws-us-gov|aws-cn|aws-iso|aws-iso-b):aiops:[a-zA-Z0-9-]*:[0-9]{12}:investigation-group\/[A-Za-z0-9]{16})$` admits a bare label beside the ARN, and the member's prose claims "Specify either the name or the ARN of the investigation group that you want to view". The recorded live behavior contradicts that prose: for a group that demonstrably exists (its ARN form returns 200, and ListInvestigationGroups lists it under that same name), GetInvestigationGroup with the bare NAME as `identifier` is refused with HTTP 404 and error code `ResourceNotFoundException` (modelled: `ResourceNotFoundException`, errorKind client, httpError 404; recorded message, as an example, "InvestigationGroup does not exist."). The bare-label alternative in the pattern is therefore only a well-formedness rule, not an address — real AWS resolves a group from its ARN alone. This is a doc/behavior divergence that the replay pins as the contract: the model's prose is not authoritative here, the recording is.

Emulator: the name form resolves. `GetInvestigationGroup(identifier=<name>)` answers HTTP 200 with the full group body (createdBy/createdAt/lastModifiedBy/lastModifiedAt/name/arn/roleArn/encryptionConfiguration/retentionInDays/isCloudTrailEventHistoryEnabled) for the same group the check created, instead of the modelled 404 `ResourceNotFoundException`. The shared resolver is deliberately built this way — the state module documents "A group is addressed by EITHER of the two forms the modelled `InvestigationGroupIdentifier` admits: its ARN, or its own name ... Both forms resolve the same record" — so the same over-permissive addressing is reachable wherever that resolver is used (Update/Delete/Get-Put-DeleteInvestigationGroupPolicy), though only GetInvestigationGroup is pinned by a failing check here.

Why: the emulator is more permissive than the service, which is the direction that silently ships broken clients. Code, CLI flows and IaC providers developed against the emulator will keep and pass the group's NAME as `identifier`, see 200s throughout, and then get a hard 404 `ResourceNotFoundException` from real AWS for a group that exists — a failure that only appears in production. It also makes the emulator unusable for testing a caller's own 404 handling on this path, since the refusal can never be provoked by the name form.

Note on existing issues: the open issue `20261024T065507Z-aiops-getinvestigationgroup-wrong-behavior--r2.md` ("GetInvestigationGroup refuses the investigation group NAME as identifier and answers 404 for a group that exists") asks for the OPPOSITE of the recorded AWS behavior, reasoning from the model's prose rather than from a replay; the fixed issue `20261024T065507Z-aiops-deleteinvestigationgroup-wrong-behavior.md` made the same change for Delete. Those do not cover this divergence — they are its cause. Resolving this one means reverting the name-as-address rule for GetInvestigationGroup and closing that open r2 issue as invalid rather than implementing it.

Fix: the shared identifier resolver in the service's state module (`services/service-com-amazonaws-aiops-aiops/src/state.rs`, `resolve_group_id`, and its module doc asserting the name form is an address), consumed by the GetInvestigationGroup handler (`services/service-com-amazonaws-aiops-aiops/src/ops/get_investigation_group.rs`). The identifier must resolve from the ARN form only; a well-formed bare name that is not an ARN must fall through to the modelled 404 `ResourceNotFoundException`. The in-repo expectations that encode the name-as-address rule (`tests/all/investigation_group_singleton.rs`, `tests/all/error_message_rules.rs`) must be moved back alongside it.

Traces: aiops.get-investigation-group-by-name-and-arn-equivalent — the ARN lookup, the ListInvestigationGroups cross-check and the name/ARN association all pass; the check fails only on the final name-identifier assertion, where it expected a 404 and the emulator returned a 200 group body.

## Evidence
Failing check: aiops.get-investigation-group-by-name-and-arn-equivalent (mismatch).

AssertionError: expected GetInvestigationGroup(identifier=<name>) to 404, it returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'createdBy': 'arn:aws:iam::<acct>:user/EMULATOR...', 'createdAt': <ts>, 'lastModifiedBy': ..., 'lastModifiedAt': ..., 'name': '<the group name>', 'arn': 'arn:aws:aiops:<region>:<acct>:investigation-group/<16 chars>', 'roleArn': 'arn:aws:iam::<acct>:role/<role>', 'encryptionConfiguration': {'type': 'AWS_OWNED_KEY'}, 'retentionInDays': 90, 'isCloudTrailEventHistoryEnabled': True}

Expected (recorded live AWS, pinned by the check): HTTP 404, Error.Code == 'ResourceNotFoundException' (modelled error on GetInvestigationGroup, httpError 404); recorded Error.Message, as an example, 'InvestigationGroup does not exist.'
Actual (emulator): HTTP 200 with the group body.

Same-check controls that PASSED, proving the group exists and only the name path diverges:
- GetInvestigationGroup(identifier=<arn>) -> 200 with arn/name/roleArn all matching what create returned.
- ListInvestigationGroups(maxResults=50) -> exactly one entry with that arn, carrying that name.

Model: smithy_query(operation="GetInvestigationGroup") -> input identifier: target InvestigationGroupIdentifier, httpLabel, required, pattern ^(?:[\-_A-Za-z0-9]{1,512}|arn:(aws|aws-us-gov|aws-cn|aws-iso|aws-iso-b):aiops:[a-zA-Z0-9-]*:[0-9]{12}:investigation-group\/[A-Za-z0-9]{16})$; errors include ResourceNotFoundException (client, httpError 404); http GET /investigationGroups/{identifier}.

Emulator source asserting the diverging rule: services/service-com-amazonaws-aiops-aiops/src/state.rs lines 9-14 — "A group is addressed by EITHER of the two forms ... its ARN, or its own name ... Both forms resolve the same record (resolve_group_id); an identifier that is neither a stored group's ARN nor a stored group's name is a modelled 404."

## Steps to Reproduce
1. Boot the emulator and create an IAM role usable as the investigation-group roleArn.
2. CreateInvestigationGroup with a unique `name` and that `roleArn` -> note the returned `arn`.
3. GetInvestigationGroup(identifier=<the returned arn>) -> 200 with the group body (control: the group exists).
4. ListInvestigationGroups(maxResults=50) -> exactly one entry whose `arn` matches and whose `name` is the created name (control: the name/ARN association is visible).
5. GetInvestigationGroup(identifier=<the created NAME, not the ARN>).
   Expected (real AWS): HTTP 404, Error.Code 'ResourceNotFoundException'.
   Actual (emulator): HTTP 200 with the full group body.
