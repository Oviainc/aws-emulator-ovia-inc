Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.complete-layer-upload-empty-rejected ecr.get-download-url-for-layer ecr.list-pull-time-exclusions-invalid-parameter-rejected ecr.op-contract-set-repository-policy ecr.put-registry-scanning-configuration-invalid-filter-rejected ecr.replication-to-same-region-rejected
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.complete-layer-upload-empty-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ecr.get-download-url-for-layer|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ecr.list-pull-time-exclusions-invalid-parameter-rejected|run#6|AssertionError|assert message == (
Issue-Key: ecr.op-contract-set-repository-policy|run#17|AssertionError|assert message == (
Issue-Key: ecr.put-registry-scanning-configuration-invalid-filter-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ecr.replication-to-same-region-rejected|run#13|AssertionError|assert message == (
Issue-Commit: b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7

# ECR refusal messages are the emulator's own prose, not AWS's wording, on six rejection paths (6 causes)

> _Note: this issue was identified against the code base at commit `b4c1cbd51f8bf527a5a2f5b1db6a86929ff8c5f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteLayerUpload

## Description
REOPENS 20261024T014332Z-ecr-error-messages-are-emulator-authored-instead-of.md: its fix left the call refused. That issue (and 20261023T153036Z-ecr-validation-messages-assert-constraints-the-api-model.md) claimed ECR refusal text had been aligned with AWS; six ECR rejection paths still answer the right HTTP status and error code with an emulator-authored message. An error message is part of the contract, so each of these is a divergence even though the refusal itself is correct. SIX net-new root causes, one per refusal path; every cause is a message-construction defect, not a decision defect.

Cause 1 of 6 — CompleteLayerUpload on an upload that has no parts.
AWS: Smithy (ecr CompleteLayerUpload) models EmptyUploadException, client error, HTTP 400, doc "The specified layer upload does not contain any layer parts." The message text is recorded behavior only; the recording shows the fixed template "The upload with id '<uploadId>' in the repository with name '<repositoryName>' in registry with id '<registryId>' cannot be completed because it does not have any parts" — fixed text is everything outside the quotes; the variable parts are the request's uploadId and repositoryName and the registry id from state.
Emulator: "The layer upload with ID '<uploadId>' for the repository with name '<repositoryName>' in the registry with id '<registryId>' does not contain any layer parts" — different fixed text ("The layer upload with ID" / "for the repository" / "does not contain any layer parts").
Why: a client or test matching AWS's EmptyUploadException sentence (log scrapers, retry classifiers, docs-derived assertions) does not recognise the refusal.
Fix: the CompleteLayerUpload handler's empty-upload refusal message construction.
Traces: ecr.complete-layer-upload-empty-rejected.

Cause 2 of 6 — GetDownloadUrlForLayer for a digest the repository does not hold.
AWS: Smithy models LayersNotFoundException (client, HTTP 400, "The specified layers could not be found, or the specified layer is not valid for this repository."). Recorded message template: "The layer with digest '<layerDigest>' does not exist within the repository with name '<repositoryName>' in the registry with id '<registryId>'" — note the fixed word "within".
Emulator: identical except "does not exist in the repository" — the fixed text differs by one word.
Why: exact-match assertions on AWS's sentence fail; the one-word difference is in the fixed part of the template, not in a request-derived value.
Fix: the GetDownloadUrlForLayer handler's layer-not-found message construction.
Traces: ecr.get-download-url-for-layer.

Cause 3 of 6 — ListPullTimeUpdateExclusions with an undecodable nextToken.
AWS: Smithy (ListPullTimeUpdateExclusions) models InvalidParameterException (client, HTTP 400); nextToken carries no length/pattern constraint, so the service validates it. Recorded message: "Invalid parameter at 'NextToken' failed to satisfy constraint: 'Invalid next token provided'" — entirely fixed text in ECR's parameter-and-constraint form, with the member path 'NextToken' as the only variable slot.
Emulator: "nextToken 'not-a-real-continuation-token!!' is not a valid token" — abandons the parameter/constraint form and echoes the rejected token value, which AWS's message does not contain.
Why: the message no longer identifies the offending member in ECR's documented 'Invalid parameter at X failed to satisfy constraint: Y' form, and it leaks the raw request value into the refusal text.
Fix: the ListPullTimeUpdateExclusions handler's nextToken validation message (should route through the crate's shared ECR parameter-constraint message builder).
Traces: ecr.list-pull-time-exclusions-invalid-parameter-rejected.

Cause 4 of 6 — SetRepositoryPolicy with a lock-out policy and no force.
AWS: Smithy (SetRepositoryPolicy) models InvalidParameterException (client, HTTP 400) and input member force (@default false). Recorded message is wholly fixed text with no request-derived values: "You are about to set a repository policy that will prevent you from setting another one in the future. Use the force parameter to override this exception."
Emulator: "the policy denies 'ecr:SetRepositoryPolicy' to every principal, which would prevent another policy from being set on repository '<repositoryName>'; set force to true to apply it anyway" — emulator prose that also interpolates the repository name into a message AWS keeps value-free.
Why: the operator-facing remediation sentence AWS documents is replaced, so clients keying on it (and humans following AWS docs) do not match the refusal.
Fix: the SetRepositoryPolicy handler's lock-out guard message.
Traces: ecr.op-contract-set-repository-policy.

Cause 5 of 6 — PutRegistryScanningConfiguration with a malformed repository filter.
AWS: Smithy declares ScanningRepositoryFilterValue @length 1..255 @pattern ^[a-z0-9*](?:[._\-/a-z0-9*]?[a-z0-9*]+)*$, and PutRegistryScanningConfiguration models InvalidParameterException (client, HTTP 400). Recorded behavior shows AWS's service-side validator quotes a DIFFERENT regex from the model's: "Invalid parameter at 'rules.1.member.repositoryFilters.1.member.filter' failed to satisfy constraint: 'Member must satisfy regular expression pattern: ^(?:[a-z0-9*]+(?:[._-][a-z0-9*]+)*/)*[a-z0-9*]+(?:[._-][a-z0-9*]+)*$'". Fixed text: the "Invalid parameter at ... failed to satisfy constraint: 'Member must satisfy regular expression pattern: ...'" frame AND the quoted pattern; variable: the 1-based member path of the offending filter.
Emulator: same frame and same member path, but quotes the model's pattern ^[a-z0-9*](?:[._\-/a-z0-9*]?[a-z0-9*]+)*$ instead of the pattern AWS's validator states.
Why: the refusal tells the caller to satisfy a regular expression AWS does not police, so a caller correcting the filter against the emulator's text can still be refused by AWS (and vice versa).
Fix: the PutRegistryScanningConfiguration validation path — the quoted pattern for this member must be the one the service states, not the modelled member pattern.
Traces: ecr.put-registry-scanning-configuration-invalid-filter-rejected.

Cause 6 of 6 — PutReplicationConfiguration whose destination is the source registry's own region.
AWS: Smithy (PutReplicationConfiguration) models InvalidParameterException (client, HTTP 400). Recorded message is ECR's parameter-and-constraint form with no request values: "Invalid parameter at 'replicationConfiguration' failed to satisfy constraint: 'Replication destination cannot be the same as the source registry'".
Emulator: "replicationConfiguration.rules[0] names destination registry '<registryId>' in region '<region>', which is the source registry of this request; a registry cannot replicate to itself" — emulator prose, a 0-based bracket member path, and the registry id and region interpolated into a message AWS keeps value-free.
Why: the message drops ECR's documented constraint form and the member path convention, so refusal-text assertions and operator runbooks diverge.
Fix: the PutReplicationConfiguration handler's same-region guard message.
Traces: ecr.replication-to-same-region-rejected.

Every cause above is grounded in a replayed check that asserted the exact message after already matching the error code.

## Evidence
Replay output (verdict mismatch in all six; the error CODE matched in each case, only the message diverged):
1) ecr.complete-layer-upload-empty-rejected — expected "The upload with id 'aa1d1c24-…' in the repository with name 'sim2real-behav-ecr-repo-…' in registry with id '000000000000' cannot be completed because it does not have any parts"; got "The layer upload with ID 'aa1d1c24-…' for the repository with name 'sim2real-behav-ecr-repo-…' in the registry with id '000000000000' does not contain any layer parts".
2) ecr.get-download-url-for-layer — expected "The layer with digest 'sha256:4216…' does not exist within the repository with name '…' in the registry with id '000000000000'"; got the same sentence with "does not exist in the repository".
3) ecr.list-pull-time-exclusions-invalid-parameter-rejected — expected "Invalid parameter at 'NextToken' failed to satisfy constraint: 'Invalid next token provided'"; got "nextToken 'not-a-real-continuation-token!!' is not a valid token".
4) ecr.op-contract-set-repository-policy — expected "You are about to set a repository policy that will prevent you from setting another one in the future. Use the force parameter to override this exception."; got "the policy denies 'ecr:SetRepositoryPolicy' to every principal, which would prevent another policy from being set on repository '<repo>'; set force to true to apply it anyway".
5) ecr.put-registry-scanning-configuration-invalid-filter-rejected — expected "…constraint: 'Member must satisfy regular expression pattern: ^(?:[a-z0-9*]+(?:[._-][a-z0-9*]+)*/)*[a-z0-9*]+(?:[._-][a-z0-9*]+)*$'"; got the same frame quoting ^[a-z0-9*](?:[._\-/a-z0-9*]?[a-z0-9*]+)*$ (the modelled ScanningRepositoryFilterValue pattern).
6) ecr.replication-to-same-region-rejected — expected "Invalid parameter at 'replicationConfiguration' failed to satisfy constraint: 'Replication destination cannot be the same as the source registry'"; got "replicationConfiguration.rules[0] names destination registry '000000000000' in region 'us-west-2', which is the source registry of this request; a registry cannot replicate to itself".
Ids, digests, upload ids, repository names and region above are per-run values that fill the template; the divergence is in the fixed text (and, for cause 5, in the quoted pattern).

## Steps to Reproduce
1) CompleteLayerUpload: CreateRepository, InitiateLayerUpload, then CompleteLayerUpload with the upload id and a layerDigests entry but no UploadLayerPart call; read Error.Message.
2) GetDownloadUrlForLayer: CreateRepository, then GetDownloadUrlForLayer with a sha256 digest never uploaded; read Error.Message.
3) ListPullTimeUpdateExclusions(nextToken="not-a-real-continuation-token!!"); read Error.Message.
4) CreateRepository, SetRepositoryPolicy with a valid policy, then SetRepositoryPolicy with {Effect:Deny, Principal:"*", Action:"ecr:*"} and no force; read Error.Message.
5) PutRegistryScanningConfiguration(scanType=BASIC, rules=[{scanFrequency:SCAN_ON_PUSH, repositoryFilters:[<valid>, {filter:<uppercase/invalid>, filterType:WILDCARD}]}]); read Error.Message.
6) PutReplicationConfiguration with a destination {region:<the caller's own region>, registryId:<own account>}; read Error.Message.
