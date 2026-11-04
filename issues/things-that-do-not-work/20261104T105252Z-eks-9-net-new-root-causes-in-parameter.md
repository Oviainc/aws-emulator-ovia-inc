Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.create-cluster-malformed-name-characters-rejected eks.ekscc-create-cluster-with-invalid-name-rejected eks.create-cluster-single-az-subnet-rejected eks.ekscc-create-cluster-duplicate-subnet-one-az-rejected eks.create-nodegroup-disk-size-out-of-range-rejected eks.create-nodegroup-invalid-scaling-rejected eks.nodegroup-scaling-config-invalid-range-rejected eks.encryption-config-association-lifecycle eks.encryption-config-reassociation-rejected eks.encryption-config-second-associate-rejected eks.cluster-update-version-lifecycle eks.op-update-cluster-version-validation eks.addon-schema-invalid-configuration-rejected-before-conflict eks.describe-identity-provider-config-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.create-cluster-malformed-name-characters-rejected|run#7|AssertionError|assert message == (
Issue-Key: eks.ekscc-create-cluster-with-invalid-name-rejected|run#8|AssertionError|assert message == "The parameter name should not be greater than 100 characters.", (
Issue-Key: eks.create-cluster-single-az-subnet-rejected|run#7|AssertionError|assert msg == "Subnets specified must be in at least two different AZs", msg
Issue-Key: eks.ekscc-create-cluster-duplicate-subnet-one-az-rejected|_error#10|AssertionError|raise AssertionError(f"the call SUCCEEDED with {result!r}; real AWS rejects it")
Issue-Key: eks.create-nodegroup-disk-size-out-of-range-rejected|run#9|AssertionError|assert message == "diskSize must be > 0", message
Issue-Key: eks.create-nodegroup-invalid-scaling-rejected|run#13|AssertionError|assert create_message == "desiredSize can't be greater than maxSize", create_message
Issue-Key: eks.nodegroup-scaling-config-invalid-range-rejected|run#8|AssertionError|assert msg == "minSize can't be greater than desiredSize", f"unexpected message: {msg!r}"
Issue-Key: eks.encryption-config-association-lifecycle|run#16|AssertionError|assert bad_arn_message == expected, f"expected message {expected!r}, got {bad_arn_message!r}"
Issue-Key: eks.encryption-config-reassociation-rejected|run#16|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r}: {message}"
Issue-Key: eks.encryption-config-second-associate-rejected|run#9|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r}: {message!r}"
Issue-Key: eks.cluster-update-version-lifecycle|run#11|AssertionError|assert message == "unsupported Kubernetes version", (
Issue-Key: eks.op-update-cluster-version-validation|run#9|AssertionError|assert control_msg == (
Issue-Key: eks.addon-schema-invalid-configuration-rejected-before-conflict|run#3|AssertionError|assert got == ("InvalidParameterException", message), f"installing with {doc} gave {got!r}"
Issue-Key: eks.describe-identity-provider-config-type-mismatch-rejected|run#10|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# eks: 9 net-new root causes in parameter-validation refusals — generic framework wording, missing AZ check, wrong error class and wrong validation order

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Nine net-new root causes in how the emulator refuses invalid request parameters. In each case the emulator either emits its own diagnostic sentence (or the generic Smithy-constraint report) where EKS publishes a specific sentence, picks a different modeled error class, or runs its checks in a different order than EKS does. All quoted names/ids/numbers below are template slots filled from the request; only the fixed text is the requirement.

Cause 1 of 9: cluster-name refusals are the validation framework's generic report, not EKS's sentences.
  AWS: Smithy: CreateCluster.name -> ClusterName, required, @length 1..100, @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$. A name with illegal characters is refused InvalidParameterException 400 with 'The parameter name contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, it should start with a number or letter, and can contain numbers, letters, dashes, and underscores.' (all fixed text apart from the member name 'name'). A 101-character name is refused InvalidParameterException with 'The parameter name should not be greater than 100 characters.' (all fixed text).
  Emulator: "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z][A-Za-z0-9\-_]*$" and "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 100".
  Why: callers (and the CLI/console) surface EKS's explanation of the naming rule; the emulator exposes an escaped regex and a framework prefix, so message-matching and user-facing error reporting both break.
  Fix: CreateCluster's input validation — EKS-worded pattern and length refusals instead of the generic Smithy-constraint reporter. Open issue 20261104T090203Z-eks-createcluster-refuses-with-generic-framework-wording-instead.md already describes the pattern sentence (its Cause 2) from the wire corpus; the LENGTH sentence is named by neither it nor 20261102T083656Z-eks-cluster-scope-request-validation-missing-checks-wrong.md, and both checks here are reported unowned.
  Traces: eks.create-cluster-malformed-name-characters-rejected, eks.ekscc-create-cluster-with-invalid-name-rejected.

Cause 2 of 9: CreateCluster counts subnets instead of requiring two distinct AZs.
  AWS: subnetIds that do not span at least two Availability Zones are refused InvalidParameterException with 'Subnets specified must be in at least two different AZs' — the whole sentence is fixed text. EKS resolves each subnet's AZ through EC2, so one subnet, the same subnet twice, or two subnets in one AZ are all refused with that sentence.
  Emulator: refuses a single subnet with its own count-based diagnostic 'resourcesVpcConfig.subnetIds must specify at least 2 subnets, got 1', and ACCEPTS two entries that resolve to one AZ (the same subnet id listed twice returns a CREATING cluster whose resourcesVpcConfig.subnetIds holds the duplicate).
  Why: the AZ requirement is EKS's real control-plane constraint; a caller is both told the wrong rule and allowed to create a cluster AWS would never create, so an infrastructure template that passes here fails against AWS.
  Fix: CreateCluster's resourcesVpcConfig validation — resolve each subnet's AZ and require two distinct AZs, with EKS's sentence.
  Traces: eks.create-cluster-single-az-subnet-rejected, eks.ekscc-create-cluster-duplicate-subnet-one-az-rejected.

Cause 3 of 9: the nodegroup diskSize refusal is the emulator's own sentence.
  AWS: Smithy: CreateNodegroup.diskSize -> BoxedInteger (no modeled range), errors include InvalidParameterException 400. A non-positive diskSize is refused InvalidParameterException with 'diskSize must be > 0' — all fixed text, no value echoed.
  Emulator: 'diskSize must be at least 1 GiB, got -1' — its own wording with the submitted value appended.
  Why: a client matching EKS's sentence does not recognise the refusal.
  Fix: CreateNodegroup's diskSize validation.
  Traces: eks.create-nodegroup-disk-size-out-of-range-rejected.

Cause 4 of 9: scalingConfig refusals collapse EKS's per-relation sentences into one generic one.
  AWS: EKS names the violated relation: desiredSize above maxSize -> InvalidParameterException 'desiredSize can't be greater than maxSize'; minSize above desiredSize -> InvalidParameterException "minSize can't be greater than desiredSize". Each sentence is entirely fixed text with no numbers in it.
  Emulator: one sentence for every case, 'scalingConfig must satisfy minSize <= desiredSize <= maxSize, got minSize <n>, desiredSize <n>, maxSize <n>'.
  Why: the caller cannot tell which relation it violated by matching the message, and the emulator's numbers are not part of any AWS contract.
  Fix: CreateNodegroup/UpdateNodegroupConfig scalingConfig validation — one refusal sentence per relation.
  Traces: eks.create-nodegroup-invalid-scaling-rejected, eks.nodegroup-scaling-config-invalid-range-rejected.

Cause 5 of 9: an invalid encryptionConfig keyArn is refused with the emulator's own diagnostic.
  AWS: Smithy: AssociateEncryptionConfig errors include InvalidParameterException 400. A keyArn that is not a KMS key reference is refused InvalidParameterException with 'The keyArn for encryptionConfig is invalid.' — all fixed text, nothing echoed.
  Emulator: 'encryptionConfig[0].provider.keyArn must be a KMS key ARN (arn:<partition>:kms:<region>:<account>:key/<id> or :alias/<name>) or an alias name (alias/<name>), got "<value>"'.
  Why: the emulator publishes an internal member path and its own grammar where AWS publishes one short sentence.
  Fix: AssociateEncryptionConfig (and CreateCluster's encryptionConfig) validation.
  Traces: eks.encryption-config-association-lifecycle.

Cause 6 of 9: a second AssociateEncryptionConfig is refused with the wrong error class and message.
  AWS: on a cluster that already has encryption enabled the call is refused InvalidParameterException (httpError 400) with 'Encryption is already enabled on the cluster' — all fixed text, no cluster name.
  Emulator: InvalidRequestException with 'Cluster <name> already has encryption enabled'.
  Why: the error CLASS differs, so a client branching on InvalidParameterException (a parameter the caller must change) versus InvalidRequestException (a state problem to retry) takes the wrong branch; the message is unrecognisable too.
  Fix: AssociateEncryptionConfig's already-enabled guard — classify as InvalidParameterException with EKS's sentence.
  Traces: eks.encryption-config-reassociation-rejected, eks.encryption-config-second-associate-rejected.

Cause 7 of 9: UpdateClusterVersion's unsupported-version refusal invents a sentence and leaks the emulator's version catalogue.
  AWS: UpdateClusterVersion with an unsupported target version is refused InvalidParameterException with exactly 'unsupported Kubernetes version' — the whole message is that fixed lower-case phrase, with no version and no list of offered versions. (CreateCluster's sibling sentence is 'unsupported Kubernetes version <version>', i.e. the create path appends the rejected version and the update path does not.)
  Emulator: 'Unsupported Kubernetes version <version>. Amazon EKS offers <v1>, <v2>, ... .' — capitalised, with the rejected version and the emulator's whole supported-version list appended.
  Why: a client matching EKS's phrase fails, and the appended catalogue is emulator state presented as an AWS contract.
  Fix: UpdateClusterVersion's version validation.
  Traces: eks.cluster-update-version-lifecycle, eks.op-update-cluster-version-validation.

Cause 8 of 9: CreateAddon checks cluster status before validating configurationValues.
  AWS: Smithy: CreateAddon errors include InvalidParameterException 400 and InvalidRequestException 400. EKS validates configurationValues against the add-on's JSON schema FIRST, even while the cluster is still CREATING, and refuses InvalidParameterException with 'ConfigurationValue provided in request is not supported: Json schema validation failed with error: [$.<member>: <schema diagnosis>]' — fixed text 'ConfigurationValue provided in request is not supported: Json schema validation failed with error: [' plus the JSON-pointer and schema diagnosis of the offending member (e.g. '$.useFIPS: string found, boolean expected', '$.useFips: is not defined in the schema and the schema does not allow additional properties').
  Emulator: refuses on cluster state first — InvalidRequestException 'Cluster <name> is in CREATING status; it must be ACTIVE to create an add-on.' — so a schema-invalid document is never diagnosed.
  Why: the caller is told to wait for the cluster when its request document is permanently wrong; it retries until a deadline instead of fixing the configuration. Validation order is observable behavior and clients depend on it.
  Fix: CreateAddon's validation ordering — schema validation of configurationValues before the cluster-status and already-exists guards. (Related open issue 20261102T083656Z-eks-add-on-request-validation-runs-in-the.md describes add-on validation ordering; this check is reported unowned.)
  Traces: eks.addon-schema-invalid-configuration-rejected-before-conflict.

Cause 9 of 9: DescribeIdentityProviderConfig with a mismatched type refuses at the parameter layer instead of as not-found.
  AWS: asking for an identity-provider config under a type it was not created with is a resource that does not exist: ResourceNotFoundException (404).
  Emulator: InvalidParameterException (400).
  Why: both the status and the class differ, so a client that treats 404 as 'absent, create it' and 400 as 'my request is malformed' mis-handles a routine probe.
  Fix: DescribeIdentityProviderConfig's lookup — a type/name pair that matches no stored config is a not-found.
  Traces: eks.describe-identity-provider-config-type-mismatch-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (service eks 142/306 passed).

Cause 1: eks.create-cluster-malformed-name-characters-rejected asserted message == 'The parameter name contains invalid characters. It should conform to the regular expression "^[0-9A-Za-z][A-Za-z0-9-_]*"In other words, ...'; got "1 validation error detected: Value 'sim2real bad cluster name !@#$' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z][A-Za-z0-9\-_]*$". eks.ekscc-create-cluster-with-invalid-name-rejected asserted message == 'The parameter name should not be greater than 100 characters.'; got "1 validation error detected: Value '<101-char name>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 100".
Cause 2: eks.create-cluster-single-az-subnet-rejected asserted msg == 'Subnets specified must be in at least two different AZs'; got 'resourcesVpcConfig.subnetIds must specify at least 2 subnets, got 1'. eks.ekscc-create-cluster-duplicate-subnet-one-az-rejected: 'the call SUCCEEDED ... real AWS rejects it' — the response carried status CREATING with resourcesVpcConfig.subnetIds ['subnet-...03e', 'subnet-...03e'].
Cause 3: eks.create-nodegroup-disk-size-out-of-range-rejected asserted message == 'diskSize must be > 0'; got 'diskSize must be at least 1 GiB, got -1'.
Cause 4: eks.create-nodegroup-invalid-scaling-rejected asserted "desiredSize can't be greater than maxSize"; got 'scalingConfig must satisfy minSize <= desiredSize <= maxSize, got minSize 2, desiredSize 9, maxSize 3'. eks.nodegroup-scaling-config-invalid-range-rejected asserted "minSize can't be greater than desiredSize"; got 'scalingConfig must satisfy minSize <= desiredSize <= maxSize, got minSize 5, desiredSize 3, maxSize 2'.
Cause 5: eks.encryption-config-association-lifecycle — "expected message 'The keyArn for encryptionConfig is invalid.', got 'encryptionConfig[0].provider.keyArn must be a KMS key ARN (...) or an alias name (alias/<name>), got \"not-a-kms-arn\"'".
Cause 6: eks.encryption-config-second-associate-rejected / eks.encryption-config-reassociation-rejected — "expected InvalidParameterException, got 'InvalidRequestException': 'Cluster <name> already has encryption enabled'"; the check requires message 'Encryption is already enabled on the cluster'.
Cause 7: eks.cluster-update-version-lifecycle asserted message == 'unsupported Kubernetes version'; got 'Unsupported Kubernetes version 9.99. Amazon EKS offers 1.37, 1.36, 1.35, 1.34, 1.33, 1.32, 1.31, 1.30.'; eks.op-update-cluster-version-validation reports the same sentence for version 1.15.
Cause 8: eks.addon-schema-invalid-configuration-rejected-before-conflict — 'installing with {"useFIPS": "true"} gave (InvalidRequestException, Cluster <name> is in CREATING status; it must be ACTIVE to create an add-on.)'; expected (InvalidParameterException, 'ConfigurationValue provided in request is not supported: Json schema validation failed with error: [$.useFIPS: string found, boolean expected]').
Cause 9: eks.describe-identity-provider-config-type-mismatch-rejected — 'an unsupported type must be refused as not-found, not at the parameter layer, got InvalidParameterException'.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — CreateCluster.name -> ClusterName required @length 1..100 @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$, errors InvalidParameterException 400 / ClientException 400 / ResourceInUseException 409 / ResourceLimitExceededException 400 / UnsupportedAvailabilityZoneException 400; CreateNodegroup.diskSize -> BoxedInteger, scalingConfig -> NodegroupScalingConfig, errors include InvalidParameterException 400; AssociateEncryptionConfig (POST /clusters/{clusterName}/encryption-config/associate) errors InvalidParameterException 400, InvalidRequestException 400; CreateAddon errors InvalidParameterException 400, InvalidRequestException 400, ResourceInUseException 409, ResourceNotFoundException 404.

## Steps to Reproduce
1. git clone the project repository and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run the check ids in trace_ids.
3. Manually, against the emulator's EKS endpoint:
   a. CreateCluster with name 'sim2real bad cluster name !@#$' -> expect InvalidParameterException 'The parameter name contains invalid characters. ...'; with a 101-character name -> expect 'The parameter name should not be greater than 100 characters.'
   b. CreateCluster with one subnet, and with the same subnet id twice -> expect InvalidParameterException 'Subnets specified must be in at least two different AZs' in both cases.
   c. CreateNodegroup with diskSize=-1 -> expect 'diskSize must be > 0'; with minSize=2,desiredSize=9,maxSize=3 -> expect "desiredSize can't be greater than maxSize"; with minSize=5,desiredSize=3,maxSize=2 -> expect "minSize can't be greater than desiredSize".
   d. AssociateEncryptionConfig with keyArn 'not-a-kms-arn' -> expect InvalidParameterException 'The keyArn for encryptionConfig is invalid.'; on a cluster created with encryptionConfig already set -> expect InvalidParameterException 'Encryption is already enabled on the cluster'.
   e. UpdateClusterVersion with version '9.99' -> expect InvalidParameterException 'unsupported Kubernetes version'.
   f. CreateAddon on a CREATING cluster with configurationValues '{"useFIPS": "true"}' -> expect InvalidParameterException 'ConfigurationValue provided in request is not supported: Json schema validation failed with error: [$.useFIPS: string found, boolean expected]'.
   g. DescribeIdentityProviderConfig naming an existing config under the wrong type -> expect 404 ResourceNotFoundException.
