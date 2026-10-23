Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: aiops.singleton-second-create-conflict-rejected aiops.concurrent-create-investigation-group-race aiops.create-default-encryption-uses-aws-owned-key aiops.get-investigation-group-by-name-and-arn-equivalent aiops.put-investigation-group-policy-malformed-json-rejected aiops.update-cross-account-configurations-persist aiops.update-role-arn-persists aiops.tag-resource-exceeds-tag-limit-rejected
Issue-Shape-Id: com.amazonaws.aiops#AIOps
Issue-Key: aiops.singleton-second-create-conflict-rejected|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: aiops.concurrent-create-investigation-group-race|run#5|AssertionError|raise AssertionError(f"race did not converge after retries: winners={winners} losers={losers}")
Issue-Key: aiops.create-default-encryption-uses-aws-owned-key|run#5|KeyError|enc = got["encryptionConfiguration"]
Issue-Key: aiops.get-investigation-group-by-name-and-arn-equivalent|run#12|AssertionError|raise AssertionError(f"expected GetInvestigationGroup(identifier=<name>) to 404, it returned {result!r}")
Issue-Key: aiops.put-investigation-group-policy-malformed-json-rejected|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: aiops.update-cross-account-configurations-persist|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: aiops.update-role-arn-persists|run#11|AssertionError|assert after["lastModifiedAt"] > before["lastModifiedAt"], (
Issue-Key: aiops.tag-resource-exceeds-tag-limit-rejected|run#6|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# aiops investigation-group fidelity: 7 divergences (singleton quota, encryptionConfiguration, name lookup, policy validation, cross-account validation, lastModifiedAt, TagResource)

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvestigationGroup

## Description
Seven distinct root causes in the aiops (CloudWatch investigations) emulator, all grounded in failing behavioral checks of this suite. They are bundled because they belong to one service; each is an independent defect and each must be closed before this issue is submitted. NOTE: an unrelated open issue, 'CreateInvestigationGroup accepts an unassumable roleArn instead of returning ValidationException' (20261022T184922Z-aiops-createinvestigationgroup-accepts-an-unassumable-rolearn-...), covers roleArn assumability only and covers NONE of the causes below.

Cause 1 of 7: CreateInvestigationGroup does not enforce the one-investigation-group-per-account/Region quota.
AWS: an account/Region may hold exactly one investigation group; a create attempted while one already exists is refused with the modeled error ServiceQuotaExceededException (Smithy: CreateInvestigationGroup errors include ServiceQuotaExceededException, errorKind client, httpError 402; shape ServiceQuotaExceededException carries message/resourceId/resourceType/serviceCode/quotaCode). Recorded behavior of the suite: the second create is refused with code ServiceQuotaExceededException and only one group exists.
Emulator: a second CreateInvestigationGroup for a different name succeeds with HTTP 201 and a fresh investigation-group ARN while the first group still exists. Under five concurrent racers all five creates succeed (five winners, zero losers), so no serialization or quota check exists at all.
Why: a client that treats the group as an account-level singleton (handles 402/ServiceQuotaExceededException, or assumes ListInvestigationGroups returns at most one entry) sees multiple groups, never receives the quota error, and its contention/backoff path is never exercised.
Fix: CreateInvestigationGroup's handler plus the service state that holds the group collection — the create path must check for an existing group under one lock and return ServiceQuotaExceededException (402) instead of inserting a second record.
Traces: aiops.singleton-second-create-conflict-rejected, aiops.concurrent-create-investigation-group-race.

Cause 2 of 7: GetInvestigationGroup omits encryptionConfiguration for a group created without one.
AWS: encryptionConfiguration is a modeled member of the GetInvestigationGroup response (Smithy: GetInvestigationGroupResponse.encryptionConfiguration -> EncryptionConfiguration{type: EncryptionConfigurationType, kmsKeyId}); omitting encryptionConfiguration on create yields the AWS-owned-key default, i.e. type AWS_OWNED_KEY with no kmsKeyId (recorded behavior for the default, documented on EncryptionConfigurationType as 'encrypted by a customer managed key or an Amazon Web Services owned key').
Emulator: GetInvestigationGroup returns 200 with no encryptionConfiguration key at all (observed response contains createdBy/createdAt/lastModifiedBy/lastModifiedAt/name/arn/roleArn/retentionInDays/isCloudTrailEventHistoryEnabled only); the check fails with KeyError: 'encryptionConfiguration'.
Why: a client cannot tell how its investigation data is encrypted and crashes or mis-defaults on a missing member the model declares; the default-key state is unrepresentable.
Fix: CreateInvestigationGroup's handler/state (record the default AWS_OWNED_KEY encryption configuration when the input omits it) and GetInvestigationGroup's serializer (emit the member).
Traces: aiops.create-default-encryption-uses-aws-owned-key.

Cause 3 of 7: GetInvestigationGroup resolves an existing group by NAME.
AWS: the identifier label's pattern admits either a bare name or an investigation-group ARN (Smithy: GetInvestigationGroup.identifier @httpLabel, pattern '^(?:[\-_A-Za-z0-9]{1,512}|arn:...:investigation-group\/[A-Za-z0-9]{16})$'), but real AWS never resolves an existing group from its name: it answers 404 ResourceNotFoundException (Smithy: ResourceNotFoundException, errorKind client, httpError 404). Recorded message example: 'InvestigationGroup does not exist.'
Emulator: GetInvestigationGroup(identifier=<name>) returns HTTP 200 with the full group body (arn/name/roleArn/...), i.e. name lookup is implemented as an alias for the ARN.
Why: a client that depends on names NOT being addressable (name-collision detection, existence probes keyed by name, or treating a 200 as proof the caller held the ARN) gets a false positive and never sees the 404 path.
Fix: GetInvestigationGroup's handler lookup — resolve only by ARN and return ResourceNotFoundException (404) for a non-ARN identifier. Same resolution rule applies wherever the service shares one identifier-resolution helper.
Traces: aiops.get-investigation-group-by-name-and-arn-equivalent.

Cause 4 of 7: PutInvestigationGroupPolicy accepts a policy body that is not valid JSON.
AWS: the policy member's modeled constraints only bound length and the printable-character set (Smithy: PutInvestigationGroupPolicy.policy required, @length 1..32768, pattern '^[\u0009\u000A\u000D\u0020-\u00FF]+$'), so a non-JSON body passes modeled validation and is then refused by the operation's semantic check with the modeled ValidationException (errorKind client, httpError 400). Recorded message example: 'The specified resource policy document is invalid.'
Emulator: PutInvestigationGroupPolicy with the body '{not-valid-json:::' returns HTTP 200 and an investigationGroupArn, storing the garbage document.
Why: a client's malformed-policy path (and any UI that surfaces ValidationException) never fires, and the group is left carrying an unparseable resource policy that a later GetInvestigationGroupPolicy hands back as if valid.
Fix: PutInvestigationGroupPolicy's handler validation — parse the document as a JSON IAM policy and return ValidationException (400) without attaching it when parsing fails.
Traces: aiops.put-investigation-group-policy-malformed-json-rejected.

Cause 5 of 7: UpdateInvestigationGroup accepts a crossAccountConfigurations sourceRoleArn that belongs to the investigation group's OWN account.
AWS: crossAccountConfigurations is a list of CrossAccountConfiguration{sourceRoleArn: RoleArn} (Smithy: UpdateInvestigationGroup.crossAccountConfigurations @length 0..25; CrossAccountConfiguration.sourceRoleArn pattern '^arn:.*$'), and a source role in the investigating account itself is refused with the modeled ValidationException (errorKind client, httpError 400) — a cross-account configuration must name a different account. Recorded message example: 'Source role ARN cannot belong to the investigation account <account>. Found ARN: <arn>'.
Emulator: the same-account sourceRoleArn update returns HTTP 200 with an empty body and is stored; only the account-equality check is missing (the foreign-account positive control already persists correctly).
Why: a misconfiguration AWS rejects at write time is silently accepted, so the group's cross-account list contains an entry that can never work and the client's validation-error path is unreachable.
Fix: UpdateInvestigationGroup's handler validation (and the identical check on CreateInvestigationGroup's crossAccountConfigurations) — compare each sourceRoleArn's account id with the group's account and return ValidationException (400) on a match, leaving stored configuration untouched.
Traces: aiops.update-cross-account-configurations-persist.

Cause 6 of 7: UpdateInvestigationGroup does not advance lastModifiedAt.
AWS: lastModifiedAt/lastModifiedBy are modeled members of the GetInvestigationGroup response (Smithy: GetInvestigationGroupResponse.lastModifiedAt Long, lastModifiedBy); a mutating UpdateInvestigationGroup re-stamps them, so lastModifiedAt after a successful update is strictly greater than before (recorded behavior).
Emulator: after a successful UpdateInvestigationGroup that changed roleArn, GetInvestigationGroup reports the SAME lastModifiedAt value as before the update (observed: expected lastModifiedAt to advance past 1792725044, got 1792725044), i.e. the field is set once at create and never touched again.
Why: clients and drift detectors that poll lastModifiedAt to notice a configuration change, or order concurrent writers by it, see a group that never changes even though its roleArn did.
Fix: UpdateInvestigationGroup's handler/state mutation — stamp lastModifiedAt (and lastModifiedBy) from request time on every successful update.
Traces: aiops.update-role-arn-persists.

Cause 7 of 7: TagResource is a scaffold stub, so tagging and the 50-tag limit do not exist.
AWS: TagResource applies tags to an investigation group (Smithy: TagResource, POST /tags/{resourceArn}, resourceArn @httpLabel required, tags: Tags map required, success 200; modeled errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400), and the model documents 'You can associate as many as 50 tags with a resource'. A call whose resulting tag set would exceed 50 user tags is refused — recorded behavior: error code BadRequestException with the message 'The resultant tag set must not have more than 50 user tags.' — and no tag from the rejected call is applied.
Emulator: TagResource answers HTTP 501 with error code NotImplemented ('operation TagResource is scaffolded but not implemented in this emulator'), so neither the valid-tag path nor the over-limit rejection exists. This check was NOT reported as already filed by the suite, so it is filed here.
Why: every tag-based workflow against aiops fails outright, the over-limit guard cannot be exercised, and clients see a 501/NotImplemented that is not a modeled error for this operation.
Fix: implement services/service-com-amazonaws-aiops-aiops/src/ops/tag_resource.rs (replace the not_implemented return, remove the SCAFFOLDED marker) with tag storage in the service state, plus the over-limit refusal and its 400-class error code; ListTagsForResource, tracked by its own already-filed stub issue, must read the same state.
Traces: aiops.tag-resource-exceeds-tag-limit-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, aiops 8/21 passed (38%), 13 failed.

Cause 1 — aiops.singleton-second-create-conflict-rejected [mismatch]: expected a ClientError (ServiceQuotaExceededException, HTTP 402) from the second CreateInvestigationGroup; got HTTP 201 {'arn': 'arn:aws:aiops:us-west-2:000000000000:investigation-group/FtQJOVqdFu7f20hS'}. aiops.concurrent-create-investigation-group-race [mismatch]: 'race did not converge after retries: winners=[5 distinct (name, arn) pairs] losers=[]' — expected exactly 1 winner and 4 losers all with code ServiceQuotaExceededException.

Cause 2 — aiops.create-default-encryption-uses-aws-owned-key [error]: KeyError: 'encryptionConfiguration' at got = aiops.get_investigation_group(identifier=arn); enc = got['encryptionConfiguration']. Expected enc['type'] == 'AWS_OWNED_KEY' and no kmsKeyId.

Cause 3 — aiops.get-investigation-group-by-name-and-arn-equivalent [mismatch]: 'expected GetInvestigationGroup(identifier=<name>) to 404, it returned {HTTPStatusCode: 200, ... name: sim2real-ig-..., arn: arn:aws:aiops:us-west-2:000000000000:investigation-group/8DgdY6W4buk2n37i, roleArn: ...}'. Expected code ResourceNotFoundException (404).

Cause 4 — aiops.put-investigation-group-policy-malformed-json-rejected [mismatch]: policy='{not-valid-json:::' -> 'expected a ClientError, but the call SUCCEEDED and returned {HTTPStatusCode: 200, ... investigationGroupArn: arn:aws:aiops:us-west-2:000000000000:investigation-group/ZjUU3qBzcOKZhixk}'. Expected ValidationException (400).

Cause 5 — aiops.update-cross-account-configurations-persist [mismatch]: UpdateInvestigationGroup(crossAccountConfigurations=[{sourceRoleArn: <role in the caller's own account>}]) -> 'expected a ClientError, but the call SUCCEEDED and returned {HTTPStatusCode: 200, content-length: 2}'. Expected ValidationException (400). The preceding foreign-account positive control persisted correctly.

Cause 6 — aiops.update-role-arn-persists [mismatch]: AssertionError: expected lastModifiedAt to advance past 1792725044, got 1792725044 (read via GetInvestigationGroup before and after a successful UpdateInvestigationGroup that changed roleArn).

Cause 7 — aiops.tag-resource-exceeds-tag-limit-rejected [mismatch]: TagResource(resourceArn=<group arn>, tags={51 pairs}) -> AssertionError: expected BadRequestException, got 'NotImplemented' (HTTP 501, 'operation TagResource is scaffolded but not implemented in this emulator'); handler file services/service-com-amazonaws-aiops-aiops/src/ops/tag_resource.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and returns Scaffolded::not_implemented("TagResource").

Error messages quoted above from recordings are examples except where the rule is stated in the description (status codes and error codes are requirements).

## Steps to Reproduce
Boot the emulator and point a botocore aiops client at it (region us-west-2).

Cause 1: iam.create_role(...); aiops.create_investigation_group(name='a', roleArn=<role>); aiops.create_investigation_group(name='b', roleArn=<role>) -> emulator returns 201; AWS returns 402 ServiceQuotaExceededException. Concurrent variant: fire 5 create_investigation_group calls in a ThreadPoolExecutor -> all 5 succeed; AWS yields 1 success and 4 ServiceQuotaExceededException.

Cause 2: aiops.create_investigation_group(name=..., roleArn=<role>) with NO encryptionConfiguration; then aiops.get_investigation_group(identifier=<arn>) -> response has no 'encryptionConfiguration' key.

Cause 3: create a group, then aiops.get_investigation_group(identifier=<the group's NAME>) -> 200 with the group body; AWS returns 404 ResourceNotFoundException.

Cause 4: create a group, then aiops.put_investigation_group_policy(identifier=<arn>, policy='{not-valid-json:::') -> 200; AWS returns 400 ValidationException and GetInvestigationGroupPolicy still 404s.

Cause 5: create two IAM roles in the caller's account and a group; aiops.update_investigation_group(identifier=<arn>, crossAccountConfigurations=[{'sourceRoleArn': <role in the SAME account>}]) -> 200; AWS returns 400 ValidationException.

Cause 6: create a group; g1 = aiops.get_investigation_group(identifier=<arn>); aiops.update_investigation_group(identifier=<arn>, roleArn=<a different role arn>); g2 = aiops.get_investigation_group(identifier=<arn>) -> g2['lastModifiedAt'] == g1['lastModifiedAt'].

Cause 7: create a group, then aiops.tag_resource(resourceArn=<arn>, tags={f'key{i}': f'value{i}' for i in range(51)}) -> 501 NotImplemented; AWS refuses with a 400-class BadRequestException and applies no tags, and a 1-tag call succeeds.
