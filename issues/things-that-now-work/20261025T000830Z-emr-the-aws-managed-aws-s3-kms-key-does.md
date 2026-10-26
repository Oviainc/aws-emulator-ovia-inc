Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.security-configuration-sse-kms-applies-to-emrfs-writes
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr|DescribeKey|NotFoundException
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# The AWS-managed aws/s3 KMS key does not exist for EMR's SSE-KMS security-configuration path: DescribeKey(alias/aws/s3) answers NotFoundException and an SSE-KMS PutObject never materialises it (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKey

## Description
One root cause, grounded in one failing behavioral check of suite simparity-behavioral-ga-prod-v1.

Cause 1 of 1: the account has no AWS-managed KMS key behind alias/aws/s3, and an SSE-KMS write that would create it on AWS does not create it here, so an EMR security configuration cannot even name a KMS key to encrypt EMRFS writes with.

AWS: every account/region has AWS-managed keys reachable through alias/aws/* (alias/aws/s3, alias/aws/secretsmanager, alias/aws/dynamodb, ...). Smithy com.amazonaws.kms#DescribeKey accepts an alias name as KeyId and answers 200 with KeyMetadata (Arn, KeyManager distinguishing AWS from CUSTOMER); its modeled faults are NotFoundException/InvalidArnException/DependencyTimeoutException/KMSInternalException. The AWS-managed aws/s3 key is created on first use — an S3 PutObject with ServerSideEncryption='aws:kms' and no explicit SSEKMSKeyId materialises it — after which DescribeKey(KeyId='alias/aws/s3') resolves to its full key ARN. That ARN is what a client then passes as EncryptionConfiguration.AtRestEncryptionConfiguration.S3EncryptionConfiguration.AwsKmsKey to emr CreateSecurityConfiguration.

Emulator: kms.describe_key(KeyId='alias/aws/s3') is refused NotFoundException, example message 'Alias arn:aws:kms:<region>:<account>:alias/aws/s3 is not found.' The check's documented fallback then issues s3.put_object(..., ServerSideEncryption='aws:kms') with no key id — which the emulator accepts — and retries DescribeKey for 120 s; the alias is still absent, so the same NotFoundException is re-raised and the check dies before any EMR call. So two linked facts: there is no AWS-managed key catalogue, and the S3 SSE-KMS default path does not create one on demand.

Why: every integration that defaults to an AWS-managed key has nothing to resolve to. A client following AWS's normal pattern (resolve alias/aws/s3, hand the ARN to a service that must encrypt with it) gets a hard not-found from a call that always succeeds on AWS, and the whole EMR at-rest-encryption surface (security configuration -> EMRFS writes landing with ServerSideEncryption=aws:kms and the expected SSEKMSKeyId) is unreachable for reasons that have nothing to do with EMR.

Fix: KMS state seeding — reference records for the AWS-managed keys and their alias/aws/* aliases, created on demand the first time a service asks for one (and by an S3 SSE-KMS write with no explicit key id), readable by describe_key.rs/list_aliases.rs in the KMS crate. This is the same root cause as Cause 3 of the open issue 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md ('the AWS-managed key catalogue does not exist', grounded in kms.aws-managed-key-reference-catalogue), and the on-demand-materialisation half touches the S3 side recorded in 20261024T102105Z-kms-s3-s3-never-resolves-or-validates-ssekmskeyid-against-kms.md. Neither carries this check's replay key, so it is recorded here and can be closed together with them — whoever fixes the catalogue should re-replay this check id too.

Note on the rest of the check: emr.security-configuration-sse-kms-applies-to-emrfs-writes is a self-declared DRAFT whose EMRFS-vs-plain-S3 encryption expectations (H1-H3) are hypotheses that were never replayed against real AWS; this issue deliberately claims only the part that is not a hypothesis — DescribeKey on alias/aws/s3 must resolve in an account where an SSE-KMS write has happened — and makes no claim about the check's later EMRFS assertions.

Traces: emr.security-configuration-sse-kms-applies-to-emrfs-writes.

Service note: filed under service 'emr', the leading token of the failing check id; the diverging call itself is KMS DescribeKey and the fix belongs in the KMS crate (both emr and kms are bootstrapped).

## Evidence
Expected (AWS): kms.describe_key(KeyId='alias/aws/s3') -> 200 with KeyMetadata.Arn (KeyManager='AWS'), either outright or after one s3.put_object(..., ServerSideEncryption='aws:kms') with no SSEKMSKeyId materialises the key.

Actual: botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DescribeKey operation: Alias arn:aws:kms:us-west-2:000000000000:alias/aws/s3 is not found. Raised twice — on the initial describe and again from the 120 s retry poll that follows the warm-up s3.put_object(Bucket=<bucket>, Key='_kms-warmup', Body=b'warmup', ServerSideEncryption='aws:kms'), which the emulator accepted. The check never reached emr.create_security_configuration or RunJobFlow.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. kms.describe_key(KeyId='alias/aws/s3') -> expect 200 with KeyMetadata.Arn; observe NotFoundException.
3. s3.create_bucket(Bucket=b); s3.put_object(Bucket=b, Key='_kms-warmup', Body=b'warmup', ServerSideEncryption='aws:kms') -> 200.
4. kms.describe_key(KeyId='alias/aws/s3') again -> expect the AWS-managed key to now exist with a resolvable ARN; observe the same NotFoundException.
5. kms.list_aliases() -> expect alias/aws/s3 among the aliases.
6. Re-replay emr.security-configuration-sse-kms-applies-to-emrfs-writes.
