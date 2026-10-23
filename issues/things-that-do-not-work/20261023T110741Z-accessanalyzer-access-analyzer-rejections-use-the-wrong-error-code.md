Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer.delete-analyzer-removes-resource accessanalyzer.check-access-not-granted-invalid-parameter-rejected accessanalyzer.create-service-linked-analyzer-access-denied-rejected accessanalyzer.update-analyzer-config-rejected-for-external-access-type
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: accessanalyzer.delete-analyzer-removes-resource|run#14|AssertionError|assert tag_error == "NotFoundException", (
Issue-Key: accessanalyzer.check-access-not-granted-invalid-parameter-rejected|run#6|AssertionError|assert message == "The access object must have at least one action or one resource.", message
Issue-Key: accessanalyzer.create-service-linked-analyzer-access-denied-rejected|run#5|AssertionError|assert message == (
Issue-Key: accessanalyzer.update-analyzer-config-rejected-for-external-access-type|run#8|AssertionError|assert message == "Unsupported operation", message
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Access Analyzer rejections use the wrong error code on TagResource and replace AWS's fixed exception messages with emulator-authored text

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Four distinct causes are bundled here; all four are in the accessanalyzer service's rejection paths. A fixer must close all four.

Cause 1 of 4 — TagResource on a deleted analyzer answers the wrong error CODE.
AWS: recorded behavior only (the model does not declare this code for the operation). Smithy: TagResource is POST /tags/{resourceArn} and declares ResourceNotFoundException (HTTP 404) as its not-found error, but the recording shows real AWS answering TagResource against a deleted analyzer ARN with error code `NotFoundException` (recorded message example: "Resource was not found") — deliberately a DIFFERENT code from the `ResourceNotFoundException` that GetAnalyzer/DeleteAnalyzer return for the same dead id (that part of the same check passes against the emulator).
Emulator: TagResource on a deleted analyzer returns error code `ResourceNotFoundException`.
Why: a client that branches on the error code — catching aa.exceptions.NotFoundException around a tag-on-create retry, or distinguishing "tag target gone" from "analyzer lookup failed" — never matches, so the exception escapes the handler it was written for.
Fix: the TagResource handler's not-found path in the accessanalyzer service (and, if the code is shared, the tag-path error mapping rather than the analyzer lookup used by GetAnalyzer/DeleteAnalyzer, whose ResourceNotFoundException is correct).
Traces: accessanalyzer.delete-analyzer-removes-resource.

Cause 2 of 4 — CheckAccessNotGranted's ValidationException does not carry AWS's fixed text for an access entry with neither actions nor resources.
AWS: Smithy: CheckAccessNotGranted (POST /policy/check-access-not-granted) takes `access` (AccessList, @length max 1, required) and models ValidationException (HTTP 400). Recorded behavior: an access entry of {"actions": [], "resources": []} is refused with HTTP 400 `ValidationException` and the fixed, request-independent string "The access object must have at least one action or one resource." The rule is known: AWS emits this one constant for this one documented at-least-one-of constraint on the access object, so it does not vary with account, region or request.
Emulator: status 400 and code `ValidationException` are right, but the message is the emulator's own "access[0] must specify actions, resources, or both".
Why: clients and test suites that assert on this constant (and log scrapers keyed to it) see unknown text, and the index-prefixed phrasing implies a per-element contract AWS does not express.
Fix: the message produced by CheckAccessNotGranted's input validation in its handler.
Traces: accessanalyzer.check-access-not-granted-invalid-parameter-rejected.

Cause 3 of 4 — CreateServiceLinkedAnalyzer's AccessDeniedException message is emulator-authored and leaks the caller principal.
AWS: Smithy: CreateServiceLinkedAnalyzer (PUT /service-linked-analyzer) models AccessDeniedException (HTTP 403). Recorded behavior: a direct call with ordinary IAM credentials is refused HTTP 403 `AccessDeniedException` with the fixed string "The caller does not have sufficient permissions, or the service principal is not authorized to create analyzers" — a constant that names no caller identity.
Emulator: status and code match, but the message is "<caller arn> is not an authorized Amazon Web Services service principal: CreateServiceLinkedAnalyzer can only be invoked by the service that manages the analyzer, and a direct invocation by a customer is denied", i.e. authored text that additionally interpolates the emulator's synthetic caller ARN (an environment-specific value AWS's message never contains).
Why: the message becomes account/credential-dependent, so anything matching it is non-deterministic across environments, and a client reading AWS's constant does not recognise the refusal.
Fix: the AccessDeniedException message in the CreateServiceLinkedAnalyzer handler (drop the caller-ARN interpolation).
Traces: accessanalyzer.create-service-linked-analyzer-access-denied-rejected.

Cause 4 of 4 — UpdateAnalyzer's ValidationException message is emulator-authored for an external-access analyzer.
AWS: Smithy: UpdateAnalyzer (PUT /analyzer/{analyzerName}) takes `configuration` (AnalyzerConfiguration) and models ValidationException (HTTP 400). Recorded behavior: updating the configuration of a plain type=ACCOUNT (external-access) analyzer is refused HTTP 400 `ValidationException` with the fixed string "Unsupported operation"; the same call against an ACCOUNT_UNUSED_ACCESS analyzer succeeds (that positive control passes).
Emulator: status 400 and code `ValidationException` are right, message is "UpdateAnalyzer is not supported for an analyzer of type ACCOUNT".
Why: as above — the constant AWS returns for this condition is not reproduced, so message-keyed client logic and fixtures diverge.
Fix: the message on UpdateAnalyzer's unsupported-analyzer-type refusal in its handler.
Traces: accessanalyzer.update-analyzer-config-rejected-for-external-access-type.

## Evidence
All four checks reached the emulator and were answered; statuses/codes below are what the suite observed.

1) accessanalyzer.delete-analyzer-removes-resource [mismatch]
   AssertionError: TagResource on deleted analyzer expected NotFoundException, got ResourceNotFoundException
   Expected: error code `NotFoundException` (message example "Resource was not found").
   Actual:   error code `ResourceNotFoundException`.
   (The earlier GetAnalyzer assertions in the same check — code ResourceNotFoundException, "Analyzer not found", and removal from ListAnalyzers — all passed.)

2) accessanalyzer.check-access-not-granted-invalid-parameter-rejected [mismatch]
   assert message == "The access object must have at least one action or one resource."
   AssertionError: access[0] must specify actions, resources, or both
   Expected message (AWS constant): "The access object must have at least one action or one resource."
   Actual message: "access[0] must specify actions, resources, or both"  (code ValidationException matched)

3) accessanalyzer.create-service-linked-analyzer-access-denied-rejected [mismatch]
   Expected message (AWS constant): "The caller does not have sufficient permissions, or the service principal is not authorized to create analyzers"
   Actual message: "arn:aws:iam::000000000000:user/EMULATORDFF87A85DAB6 is not an authorized Amazon Web Services service principal: CreateServiceLinkedAnalyzer can only be invoked by the service that manages the analyzer, and a direct invocation by a customer is denied"  (code AccessDeniedException matched; the caller ARN in the actual text is an example of the interpolated, environment-specific value)

4) accessanalyzer.update-analyzer-config-rejected-for-external-access-type [mismatch]
   assert message == "Unsupported operation"
   AssertionError: UpdateAnalyzer is not supported for an analyzer of type ACCOUNT
   Expected message (AWS constant): "Unsupported operation"
   Actual message: "UpdateAnalyzer is not supported for an analyzer of type ACCOUNT"  (code ValidationException matched)

## Steps to Reproduce
Cause 1: CreateAnalyzer(analyzerName=N, type=ACCOUNT); poll GetAnalyzer to ACTIVE; DeleteAnalyzer(N); then TagResource(resourceArn=<the analyzer arn>, tags={'probe':'post-delete'}). Emulator answers error code ResourceNotFoundException; AWS answers NotFoundException.

Cause 2: CheckAccessNotGranted(policyDocument=<any valid identity policy>, policyType='IDENTITY_POLICY', access=[{'actions': [], 'resources': []}]). Emulator returns 400 ValidationException with "access[0] must specify actions, resources, or both"; AWS returns "The access object must have at least one action or one resource."

Cause 3: CreateServiceLinkedAnalyzer(type='ACCOUNT') with ordinary credentials. Emulator returns 403 AccessDeniedException whose message interpolates the caller ARN; AWS returns the fixed "The caller does not have sufficient permissions, or the service principal is not authorized to create analyzers".

Cause 4: CreateAnalyzer(analyzerName=E, type='ACCOUNT'); poll to ACTIVE; UpdateAnalyzer(analyzerName=E, configuration={'unusedAccess': {'analysisRule': {'exclusions': [{'resourceTags': [{'k':'v'}]}]}}}). Emulator returns 400 ValidationException "UpdateAnalyzer is not supported for an analyzer of type ACCOUNT"; AWS returns "Unsupported operation".
