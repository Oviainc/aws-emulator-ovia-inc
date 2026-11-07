Issue-Source: trace-sweep-divergence
Issue-Trace: ecr-batch-diag-empty-repository-latest-tag-lookups
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecr-batch-diag-empty-repository-latest-tag-lookups ecr-batch-diag-image-uri-passed-as-repository-name ecr-ecracc-cross-account-registry-id-reads
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|DescribeImages|4xx->4xx|$.message
Issue-Key: ecr|ListImages|4xx->4xx|$.message
Issue-Key: ecr|DescribeRepositories|4xx->4xx|$.message
Issue-Key: ecr|GetAuthorizationToken|4xx->4xx|$.message
Issue-Commit: ae442358c61414b2bbb5a7e8dd80b7eec77accf8

# ECR validation and not-found error messages diverge from AWS wording

> _Note: this issue was identified against the code base at commit `ae442358c61414b2bbb5a7e8dd80b7eec77accf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
4 causes. ECR answers these four client-error cases with the right HTTP status (400) and the right error code, but the human-readable `message` is not the string AWS produces: the imageId echo, the enum value set, the regex quoting and the list-member template are all built differently. Clients and CLI users read these messages verbatim, and test suites assert on them.

Cause 1 of 4: DescribeImages ImageNotFoundException renders the wrong imageId echo for a tag-only lookup.
AWS: Smithy: DescribeImages.imageIds -> ImageIdentifierList (members imageDigest/imageTag), error ImageNotFoundException ("The image requested does not exist in the specified repository", httpError 400). Message template, fixed text exactly: "The image with imageId {imageDigest:'<digest>', imageTag:'<tag>'} does not exist within the repository with name '<repositoryName>' in the registry with id '<registryId>'". Fixed text = everything outside the four substitutions; variable parts = the digest, the tag, the repository name from the request and the registry id of the target registry. Recorded behavior only for how AWS fills the first two on a request that carried ONLY an imageTag: AWS renders imageDigest as the all-zero sha256 placeholder (`sha256:` followed by 64 zeros) and imageTag as the literal four-letter string `null`. Hypothesis: AWS normalises the identifier through a fully-populated internal ImageIdentifier whose unset digest prints as the zero digest and whose tag is cleared once the tag lookup resolves to nothing.
Emulator: emits the two fields transposed relative to AWS: imageDigest:'null' and imageTag:'<the tag from the request>'. Status 400 and the surrounding sentence are correct.
Why: a client that parses the imageId out of the failure message to learn which identifier form was rejected reads a digest where AWS puts a tag and vice versa; a fixture or assertion written against AWS never matches.
Fix: the DescribeImages handler's ImageNotFoundException message formatter (the imageId rendering helper), not its lookup logic — the lookup correctly found nothing.
Traces: ecr-batch-diag-empty-repository-latest-tag-lookups#task#4.

Cause 2 of 4: ListImages invalid filter.tagStatus lists the enum value set in the wrong order.
AWS: Smithy: ListImages.filter -> ListImagesFilter.tagStatus -> enum TagStatus, error InvalidParameterException httpError 400. Message, fixed text exactly: "Invalid parameter at 'filter.tagStatus' failed to satisfy constraint: 'Member must satisfy enum value set: [UNTAGGED, ANY, TAGGED]'". The only variable part is the member path; the bracketed set is fixed text and AWS emits it in the order UNTAGGED, ANY, TAGGED. Recorded behavior only for the ordering: the model declares TagStatus as [TAGGED, UNTAGGED, ANY], so the wire order is NOT the model's declaration order and cannot be derived from it.
Emulator: emits "... enum value set: [TAGGED, UNTAGGED, ANY]'" — the model's declaration order.
Why: the message is part of the contract for this error class; a client or test comparing the refusal text against AWS's sees a mismatch even though the rejection itself is right.
Fix: the ListImages input validation's enum-set message (the constant value list used to build the constraint message), not the enum definition.
Traces: ecr-batch-diag-image-uri-passed-as-repository-name#task#4.

Cause 3 of 4: DescribeRepositories registryId pattern message wraps the regex in quotes.
AWS: Smithy: DescribeRepositories.registryId -> RegistryId @pattern ^[0-9]{12}$, error InvalidParameterException httpError 400. Message, fixed text exactly: "Invalid parameter at 'registryId' failed to satisfy constraint: 'must satisfy regular expression [0-9]{12}'". Variable parts = the member path and the pattern; the pattern appears BARE inside the single-quoted constraint clause, with no inner quoting.
Emulator: emits "... 'must satisfy regular expression '[0-9]{12}''" — the pattern is additionally wrapped in single quotes, producing doubled quotes.
Why: the extra quoting breaks any client that parses the constraint clause out of the message, and it does not match AWS byte-for-byte.
Fix: the shared pattern-constraint message formatter used by ECR input validation.
Traces: ecr-ecracc-cross-account-registry-id-reads#task#7.

Cause 4 of 4: GetAuthorizationToken validates the registryIds LIST with the scalar template and the wrong member name.
AWS: Smithy: GetAuthorizationToken.registryIds -> GetAuthorizationTokenRegistryIdList (list of RegistryId @pattern ^[0-9]{12}$, @length 1..10), error InvalidParameterException httpError 400. Because the offending member is a list, AWS uses the collection form, fixed text exactly: "Invalid parameter at 'registryIds' failed to satisfy constraint: 'Member must satisfy constraint: [Member must satisfy regular expression pattern: [0-9]{12}]'". Variable parts = the member path ('registryIds', the list member's own name) and the pattern.
Emulator: emits the scalar form under the wrong member name: "Invalid parameter at 'registryId' failed to satisfy constraint: 'must satisfy regular expression '[0-9]{12}''" — singular 'registryId' (a member GetAuthorizationToken does not even have) and no nested "Member must satisfy ..." clause.
Why: the message names a parameter the caller never sent, so a user cannot tell which input to correct, and list-vs-scalar validation is indistinguishable.
Fix: GetAuthorizationToken's input validation — validate each list element and emit the collection-member constraint message under the list member's name.
Traces: ecr-ecracc-cross-account-registry-id-reads#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (72 fixtures, 511 steps, 18 mismatched).

Cause 1 — ecr-batch-diag-empty-repository-latest-tag-lookups#task#4 DescribeImages, status 400 expected and 400 actual:
  $.message expected "The image with imageId {imageDigest:'sha256:0000000000000000000000000000000000000000000000000000000000000000', imageTag:'null'} does not exist within the repository with name 'sim2real-probe-ecr-openmp-3ba64a14' in the registry with id '000000000000'"
  $.message actual   "The image with imageId {imageDigest:'null', imageTag:'latest'} does not exist within the repository with name 'sim2real-probe-ecr-openmp-3ba64a14' in the registry with id '000000000000'"
  (repository name and registry id are per-environment values, not requirements; the digest/tag rendering is.)

Cause 2 — ecr-batch-diag-image-uri-passed-as-repository-name#task#4 ListImages, 400 -> 400:
  $.message expected "Invalid parameter at 'filter.tagStatus' failed to satisfy constraint: 'Member must satisfy enum value set: [UNTAGGED, ANY, TAGGED]'"
  $.message actual   "Invalid parameter at 'filter.tagStatus' failed to satisfy constraint: 'Member must satisfy enum value set: [TAGGED, UNTAGGED, ANY]'"

Cause 3 — ecr-ecracc-cross-account-registry-id-reads#task#7 DescribeRepositories, 400 -> 400:
  $.message expected "Invalid parameter at 'registryId' failed to satisfy constraint: 'must satisfy regular expression [0-9]{12}'"
  $.message actual   "Invalid parameter at 'registryId' failed to satisfy constraint: 'must satisfy regular expression '[0-9]{12}''"

Cause 4 — ecr-ecracc-cross-account-registry-id-reads#task#8 GetAuthorizationToken, 400 -> 400:
  $.message expected "Invalid parameter at 'registryIds' failed to satisfy constraint: 'Member must satisfy constraint: [Member must satisfy regular expression pattern: [0-9]{12}]'"
  $.message actual   "Invalid parameter at 'registryId' failed to satisfy constraint: 'must satisfy regular expression '[0-9]{12}''"

## Steps to Reproduce
1. Replay the wire corpus against the emulator (trace set simparity-wire-aws-v1).
2. Cause 1: create an ECR repository with no images, then DescribeImages(repositoryName=<repo>, imageIds=[{imageTag: "latest"}]). Compare $.message of the 400 ImageNotFoundException with AWS's template (zero digest + literal 'null' tag).
3. Cause 2: ListImages(repositoryName=<repo>, filter={tagStatus: "<not a TagStatus value>"}) and compare the bracketed enum set ordering.
4. Cause 3: DescribeRepositories(registryId="<not 12 digits>") and compare the regex quoting in $.message.
5. Cause 4: GetAuthorizationToken(registryIds=["<not 12 digits>"]) and compare the member name and the collection-member constraint clause.
