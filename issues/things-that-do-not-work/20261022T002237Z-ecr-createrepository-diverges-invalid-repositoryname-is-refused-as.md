Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.create-repository-invalid-name-rejected ecr.op-contract-putImageScanningConfiguration
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.create-repository-invalid-name-rejected|run#9|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: ecr.op-contract-putImageScanningConfiguration|run#4|KeyError|assert repo["imageScanningConfiguration"]["scanOnPush"] is False, (
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# ECR CreateRepository diverges: invalid repositoryName is refused as ValidationException instead of InvalidParameterException, and the returned Repository omits imageScanningConfiguration

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
Two distinct defects in the implemented CreateRepository path. Both must be closed before this issue is submitted.

Cause 1 of 2: a repositoryName that violates the modeled pattern is refused with the wrong error code.
AWS: Smithy CreateRepository.repositoryName is @required with length 2-256 and pattern ^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$ (uppercase is not allowed). CreateRepository's modeled error set is InvalidParameterException, InvalidTagParameterException, KmsException, LimitExceededException, RepositoryAlreadyExistsException, TooManyTagsException, ServerException - it does NOT include ValidationException. So a name containing uppercase characters must be refused with error code InvalidParameterException at httpError 400. Because the message here is built from the request and its constraint, the message rule is also a requirement: it names the member and the regular expression it failed, e.g. "Invalid parameter at 'repositoryName' failed to satisfy constraint: 'must satisfy regular expression '[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*''" (recorded example).
Emulator: the call is refused, but with error code ValidationException - a code that is not in CreateRepository's modeled error set for this service.
Why: SDK typed-exception handling catches ecr.exceptions.InvalidParameterException and never fires; code that branches on InvalidParameterException to fall back to a sanitized name treats the refusal as an unknown/generic validation failure instead. The same wrong code is returned by the follow-up DescribeRepositories probe on the bad name, so the "it was not created as a side effect" contract is also unobservable.
Fix: the CreateRepository operation handler's input validation / the generic constraint-validation layer that maps member-pattern failures to an error shape - it must emit ECR's InvalidParameterException (with the member-and-constraint message rule), not a generic ValidationException, for operations whose modeled error set lacks ValidationException.
Traces: ecr.create-repository-invalid-name-rejected.

Cause 2 of 2: the Repository projection omits imageScanningConfiguration.
AWS: Smithy shape com.amazonaws.ecr#Repository has members repositoryArn, registryId, repositoryName, repositoryUri, createdAt, imageTagMutability, imageTagMutabilityExclusionFilters, imageScanningConfiguration (-> ImageScanningConfiguration{scanOnPush: boolean, default false}) and encryptionConfiguration. CreateRepository's output is {repository: Repository}, so a freshly created repository reports imageScanningConfiguration.scanOnPush = false, and DescribeRepositories must report the same member (true after PutImageScanningConfiguration flips it).
Emulator: the dict returned as CreateRepositoryResponse.repository has no `imageScanningConfiguration` key at all - the check fails with KeyError: 'imageScanningConfiguration' on the very first read of the created repository.
Why: a client reading repo['imageScanningConfiguration']['scanOnPush'] to decide whether scan-on-push is enabled crashes rather than reading false; the default-false contract of ScanOnPushFlag is unobservable and scan configuration appears unsupported even where the state is tracked.
Fix: the CreateRepository (and DescribeRepositories) response serializer / Repository projection in ECR state - populate imageScanningConfiguration with scanOnPush defaulting to false.
Traces: ecr.op-contract-putImageScanningConfiguration.

## Evidence
Cause 1 - ecr.create-repository-invalid-name-rejected [mismatch]:
  check_ecr_create_repository_invalid_name_rejected.py line 49: assert code == "InvalidParameterException"
  AssertionError: expected InvalidParameterException, got ValidationException
  (the positive control in the same check - CreateRepository + DescribeRepositories with a valid lowercase name - succeeded, so the operation is reachable and implemented; only the error classification diverges)
  Expected: error code InvalidParameterException, httpError 400, message naming 'repositoryName' and the failed regular expression.
  Actual: error code ValidationException.

Cause 2 - ecr.op-contract-putImageScanningConfiguration [error]:
  check_ecr_op_contract_putImageScanningConfiguration.py line 22:
    assert repo["imageScanningConfiguration"]["scanOnPush"] is False
  KeyError: 'imageScanningConfiguration'
  where repo = ecr.create_repository(repositoryName=<valid name>)["repository"].
  Expected: repository.imageScanningConfiguration = {"scanOnPush": false} on a freshly created repository, and the same member present in DescribeRepositories.
  Actual: the member is absent from the response.

## Steps to Reproduce
Cause 1:
1. aws ecr create-repository --repository-name sim2real-behav-ecr-repo-INVALID-Upper
2. Observe the error code: the emulator returns ValidationException; AWS returns InvalidParameterException (400) with a message naming 'repositoryName' and the regular expression it failed.
3. aws ecr describe-repositories --repository-names <that same bad name> -> must also be InvalidParameterException.

Cause 2:
1. aws ecr create-repository --repository-name sim2real-ecr-<unique>
2. Read repository.imageScanningConfiguration.scanOnPush from the response -> expected false, observed: the member is missing.
3. aws ecr describe-repositories --repository-names <that name> and read the same member.

Then re-replay ecr.create-repository-invalid-name-rejected and ecr.op-contract-putImageScanningConfiguration.
