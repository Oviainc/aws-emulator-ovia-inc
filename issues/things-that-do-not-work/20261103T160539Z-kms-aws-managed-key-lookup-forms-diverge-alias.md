Issue-Source: trace-sweep-divergence
Issue-Trace: kms-rbx-traj-vault-key-lookup-forms
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kms-rbx-traj-vault-key-lookup-forms kms-sseinv-aws-managed-s3-key-describe-and-alias-mistakes
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|ListAliases|4xx->4xx|$.__type
Issue-Key: kms|ListAliases|4xx->4xx|$.message
Issue-Key: kms|DescribeKey|2xx->2xx|$.KeyMetadata.Arn
Issue-Key: kms|DescribeKey|2xx->2xx|$.KeyMetadata.Description
Issue-Key: kms|ListAliases|2xx->2xx|$.AliasCount
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# KMS AWS-managed key lookup forms diverge: alias-ARN refusal class, empty managed-key Description, absent AliasCount

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAliases

## Description
Four net-new wire divergences, all on the same surface: how the KMS emulator answers the *lookup forms* clients use against AWS managed keys (alias ARN vs key ARN vs alias name) and what metadata it projects for those managed keys. 4 causes.

Related open issues that do NOT cover these clusters (they carry neither of these fixtures nor these steps as a replay key, and describe a different failure - a 400 where AWS answers 200 on the backup path): 20261103T061309Z-kms-listaliases-refuses-a-backup-vault-s-default.md, 20261027T205645Z-backup-a-backup-vault-s-default-encryptionkeyarn-names-a.md, 20261027T205606Z-backup-kms-s3-a-backup-vault-s-default-encryptionkeyarn-names-a.md. The 10 KeyOrigin clusters of this replay are already owned by 20261029T235222Z-kms-crypto-operation-responses-omit-the-keyorigin-projection.md and are not part of this issue.

Cause 1 of 4: ListAliases given an ALIAS ARN as KeyId is refused as not-found instead of as an invalid ARN (fixture kms-rbx-traj-vault-key-lookup-forms, step task#3).
AWS: Smithy kms#ListAliases.KeyId (KeyIdType, @length 1-2048) is documented as 'Specify the key ID or key ARN of the KMS key' - an alias identifier is not an accepted form for this parameter. The modelled client errors are InvalidArnException (@httpError 400, awsQueryError InvalidArn), InvalidMarkerException (400) and NotFoundException (@httpError 404). An alias ARN is therefore a bad ARN *form*, not a missing resource: the recorded response is HTTP 400 with __type InvalidArnException and the message 'Key Aliases are not supported for this operation.' That message is entirely fixed text - it has no variable part, so it must be reproduced verbatim.
Emulator: HTTP 400 but with __type 'NotFoundException' and the message 'Alias <alias-arn> is not found.' (recorded run: 'Alias arn:aws:kms:us-west-2:000000000000:alias/aws/backup is not found.'); i.e. it resolves the alias ARN as if it were a lookupable alias and then reports it absent. Fixed text 'Alias ' / ' is not found.' with the request's KeyId interpolated - neither the class nor the text AWS uses here. Note also that NotFoundException is modelled at HTTP 404, so emitting it at 400 is self-inconsistent.
Why: a client that probes a key identifier through several forms (console-style reference chains, inventory tools that are handed an opaque identifier) branches on the error class: InvalidArn means 'retry with the key ARN / resolve the alias first', NotFound means 'the key is gone'. The emulator sends such a client down the deleted-resource branch for a request that is merely in the wrong form, and an SDK matching on the modelled error shape sees a 400 carrying a shape AWS only emits at 404.
Fix: ListAliases' input validation in the kms crate - classify an alias id/ARN supplied in KeyId as InvalidArnException with AWS's fixed message before any alias/key state lookup.
Traces: kms-rbx-traj-vault-key-lookup-forms#task#3.

Cause 2 of 4: DescribeKey reached through the aws/backup managed alias reports an empty Description and a key ARN other than the alias's target (fixture kms-rbx-traj-vault-key-lookup-forms, step task#4).
AWS: Smithy kms#DescribeKey returns KeyMetadata, whose Description member (DescriptionType, @length 0-8192) is 'The description of the KMS key'. For an AWS managed key the description is not empty: AWS seeds each managed key with its service's fixed default-key description, which for the Backup managed key is 'Default key that protects my Backup data when no other key is defined' (recorded behavior; the Smithy model states only that the member exists). KeyMetadata.Arn is the ARN of the KMS key resolved from the identifier - there is exactly one AWS managed key per service per account/Region, so every lookup form of that alias (alias name, alias ARN, the EncryptionKeyArn a Backup vault hands out, and the alias's TargetKeyId in ListAliases) must resolve to the SAME key ARN.
Emulator: HTTP 200, but $.KeyMetadata.Description is '' (expected 'Default key that protects my Backup data when no other key is defined'), and $.KeyMetadata.Arn names a different key than the one the recording's alias targets (recorded run: expected .../key/18e49478-3869-3362-0000-000000000001, got .../key/093449a3-1478-4f4f-b048-ca36f2b858c8). The uuid in a key ARN is an environment-generated value and is not itself a requirement; the requirement is that this lookup resolves the account's single aws/backup managed key, the same one the other lookup forms in the fixture resolve. Hypothesis: the managed key is minted on demand per lookup path instead of being seeded once per service alias, which is also why it carries no description.
Why: a client that reads a managed key to display or audit it gets a blank description where AWS always shows the service's default-key text, and - worse - two lookup paths for the same managed key disagree on the key ARN, so caching/deduplicating by key ARN produces two phantom managed keys for one service and any key-policy or grant decision keyed on that ARN is made against the wrong key.
Fix: the kms crate's key state/seeding for AWS managed keys - seed one managed key per service alias with AWS's fixed default-key Description, and have DescribeKey's identifier resolution (alias name, alias ARN, key ARN) return that one key.
Traces: kms-rbx-traj-vault-key-lookup-forms#task#4.

Cause 3 of 4: the ListAliases 200 body omits the AliasCount member the recorded wire carries (fixture kms-rbx-traj-vault-key-lookup-forms, step task#5).
AWS: recorded behavior only - AliasCount is NOT a modelled member of kms#ListAliasesResponse (modelled: Aliases, NextMarker, Truncated with default false), checked against services/service-com-amazonaws-kms-trentservice/smithy-model. The recorded 200 response carries AliasCount alongside Aliases, whose value is the number of aliases in the returned page (1 in the recording, where the single alias/aws/backup alias is returned). The number itself is a function of the request and of state, not a constant.
Emulator: HTTP 200 with $.AliasCount missing (expected 1, got None). Aliases and Truncated match.
Why: a client that reads the count off the response rather than measuring the list (paging heuristics, console-style summaries) gets no value at all and treats the page as empty.
Fix: the ListAliases response serializer in the kms crate - project the count of the returned alias page onto the output.
Traces: kms-rbx-traj-vault-key-lookup-forms#task#5 (also kms-sseinv-aws-managed-s3-key-describe-and-alias-mistakes#task#4).

Cause 4 of 4: DescribeKey on the aws/s3 managed key reports an empty Description (fixture kms-sseinv-aws-managed-s3-key-describe-and-alias-mistakes, step task#3).
AWS: same requirement as cause 2, for the S3 managed key: KeyMetadata.Description carries that service's fixed default-key text, 'Default key that protects my S3 objects when no other key is defined' (recorded behavior; the model only types the member). The text is fixed per service - only the service noun differs between managed keys.
Emulator: HTTP 200 with $.KeyMetadata.Description = '' (expected 'Default key that protects my S3 objects when no other key is defined'), on both the describe-by-identifier and the follow-up describe in the same fixture.
Why: any inventory/compliance read that identifies an AWS managed key by its default-key description (and any console-style rendering of it) sees a blank field, so the key is indistinguishable from an undescribed customer-managed key.
Fix: the kms crate's AWS-managed-key seeding/state - give each managed key its service's default-key Description, surfaced unchanged by DescribeKey's KeyMetadata projection.
Traces: kms-sseinv-aws-managed-s3-key-describe-and-alias-mistakes#task#3 (also #task#5).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up true, 503 steps, 476 passed, 27 mismatched, 14 divergence clusters; 10 of them owned by 20261029T235222Z-kms-crypto-operation-responses-omit-the-keyorigin-projection.md). The 4 net-new clusters, verbatim:

1) service=kms|ListAliases|status=400->400|keys=$.__type,$.message - fixture kms-rbx-traj-vault-key-lookup-forms, step task#3: $.__type: value_mismatch (expected 'InvalidArnException', got 'NotFoundException'); $.message: value_mismatch (expected 'Key Aliases are not supported for this operation.', got 'Alias arn:aws:kms:us-west-2:000000000000:alias/aws/backup is not found.').

2) service=kms|DescribeKey|status=200->200|keys=$.KeyMetadata.Arn,$.KeyMetadata.Description - fixture kms-rbx-traj-vault-key-lookup-forms, step task#4: $.KeyMetadata.Description: value_mismatch (expected 'Default key that protects my Backup data when no other key is defined', got ''); $.KeyMetadata.Arn: value_mismatch (expected 'arn:aws:kms:us-west-2:000000000000:key/18e49478-3869-3362-0000-000000000001', got 'arn:aws:kms:us-west-2:000000000000:key/093449a3-1478-4f4f-b048-ca36f2b858c8') - the uuid is an environment-generated value; what the mismatch shows is that this lookup form does not resolve the same managed key the fixture's other lookup forms resolve.

3) service=kms|ListAliases|status=200->200|keys=$.AliasCount - fixture kms-rbx-traj-vault-key-lookup-forms, step task#5: $.AliasCount: missing (expected 1, got None); affected_count 2 (also kms-sseinv-aws-managed-s3-key-describe-and-alias-mistakes#task#4).

4) service=kms|DescribeKey|status=200->200|keys=$.KeyMetadata.Description - fixture kms-sseinv-aws-managed-s3-key-describe-and-alias-mistakes, step task#3: $.KeyMetadata.Description: value_mismatch (expected 'Default key that protects my S3 objects when no other key is defined', got ''); affected_count 2 (also #task#5).

Model checked (services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json): kms#ListAliases input {KeyId @length 1-2048, doc 'Specify the key ID or key ARN of the KMS key', Limit, Marker}, output {Aliases, NextMarker, Truncated default false} - NO AliasCount member; errors InvalidArnException @httpError 400 / InvalidMarkerException 400 / NotFoundException @httpError 404 / KMSInternalException 500 / DependencyTimeoutException 503. kms#DescribeKey output KeyMetadata with Description (DescriptionType @length 0-8192) and Arn (ArnType @length 20-2048); errors InvalidArnException 400, NotFoundException 404.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
3. Cause 1 - fixture kms-rbx-traj-vault-key-lookup-forms, step task#3: call kms:ListAliases with KeyId set to an ALIAS ARN (e.g. the alias ARN of an AWS managed key, arn:aws:kms:<region>:<account>:alias/aws/backup). The emulator answers 400 NotFoundException 'Alias <arn> is not found.' where AWS answers 400 InvalidArnException 'Key Aliases are not supported for this operation.'.
4. Cause 2 - step task#4 of the same fixture: DescribeKey through the aws/backup managed-key identifier. KeyMetadata.Description comes back '' instead of 'Default key that protects my Backup data when no other key is defined', and KeyMetadata.Arn names a different key than the one the fixture's other lookup forms (ListAliases TargetKeyId / the vault's EncryptionKeyArn) resolve.
5. Cause 3 - step task#5 of the same fixture: ListAliases for that managed key returns 200 with Aliases and Truncated, but no AliasCount member (recorded response carries AliasCount = size of the returned page). Same at kms-sseinv-aws-managed-s3-key-describe-and-alias-mistakes#task#4.
6. Cause 4 - fixture kms-sseinv-aws-managed-s3-key-describe-and-alias-mistakes, steps task#3 and task#5: DescribeKey on the aws/s3 managed key returns KeyMetadata.Description = '' instead of 'Default key that protects my S3 objects when no other key is defined'.
7. Re-replay each fixture#step listed under Traces to confirm a fix.
