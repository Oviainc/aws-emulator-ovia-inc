Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer.create-service-linked-analyzer-access-denied-rejected accessanalyzer.check-access-not-granted-invalid-parameter-rejected accessanalyzer.update-analyzer-config-rejected-for-external-access-type
Issue-Shape-Id: com.amazonaws.accessanalyzer#AccessAnalyzer
Issue-Key: accessanalyzer.create-service-linked-analyzer-access-denied-rejected|run#4|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: accessanalyzer.check-access-not-granted-invalid-parameter-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: accessanalyzer.update-analyzer-config-rejected-for-external-access-type|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Commit: c6f9974c04d0d99d1dbb9b00d2d222f6d449381b

# accessanalyzer: three negative paths answer NotImplemented instead of the operation's modeled rejection (CreateServiceLinkedAnalyzer, CheckAccessNotGranted, UpdateAnalyzer)

> _Note: this issue was identified against the code base at commit `c6f9974c04d0d99d1dbb9b00d2d222f6d449381b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServiceLinkedAnalyzer

## Description
Three distinct accessanalyzer operations reject nothing: instead of the modeled client error AWS returns for the condition, the emulator answers its generic scaffold refusal (botocore surfaces error code `NotImplemented`, message "operation <Op> is scaffolded but not implemented in this emulator"). Each is a separate root cause in a separate handler; this issue bundles 3 causes, all in the accessanalyzer service.

NOTE on overlap: the suite already has auto-filed scaffold-stub issues for `CheckAccessNotGranted` and `UpdateAnalyzer` (those issues cover making the operations answer at all). Causes 2 and 3 below record the specific REJECTION semantics these checks assert, which the stub issues do not state; cause 1 (`CreateServiceLinkedAnalyzer`) has no issue of any kind filed for it. All three must be closed before the listed checks pass.

Cause 1 of 3: CreateServiceLinkedAnalyzer called with ordinary IAM credentials is not refused.
AWS: Smithy model (services/service-com-amazonaws-accessanalyzer-accessanalyzer/smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json) binds CreateServiceLinkedAnalyzer to `PUT /service-linked-analyzer` with modeled error `AccessDeniedException` (@error client, httpError 403, required member `message`); the shape doc is "You do not have sufficient access to perform this action." The operation is only callable by an authorized AWS service principal, so a direct call from ordinary caller credentials is rejected with HTTP 403 / error code `AccessDeniedException` and no service-linked analyzer is created. Recorded behavior shows the message "The caller does not have sufficient permissions, or the service principal is not authorized to create analyzers" (example text, not the requirement).
Emulator: the call is answered with error code `NotImplemented` (scaffold refusal), so the authorization gate does not exist at all.
Why: a client testing that it cannot self-create a service-linked analyzer sees an unmodeled error code its error handling has no branch for; code that asserts on AccessDeniedException (or that retries/falls back on it) takes the wrong path, and an over-permissive emulator cannot be used to validate that this operation is service-principal-only.
Fix: the CreateServiceLinkedAnalyzer handler (services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/create_service_linked_analyzer.rs) must implement the operation and its caller-principal authorization gate, returning the modeled AccessDeniedException for a non-service-principal caller while leaving analyzer state untouched.
Traces: accessanalyzer.create-service-linked-analyzer-access-denied-rejected (mismatch: "expected AccessDeniedException, got NotImplemented").

Cause 2 of 3: CheckAccessNotGranted does not validate an access entry that names neither actions nor resources.
AWS: Smithy: CheckAccessNotGranted (`POST /policy/check-access-not-granted`) takes required `policyDocument`, required `access` (AccessList, @length max 1) and required `policyType`; modeled errors include `ValidationException` (@error client, httpError 400, required members `message` and `reason`) and `InvalidParameterException` (400). An access entry with empty `actions` and empty `resources` constrains nothing, and AWS rejects the request with HTTP 400 / `ValidationException` rather than producing a result for it; recorded message "The access object must have at least one action or one resource." (example text).
Emulator: the call is answered with error code `NotImplemented`, so neither the validation nor the evaluation exists.
Why: a client that submits a malformed access entry gets an unmodeled code instead of 400 ValidationException, so input-validation tests pass/fail for the wrong reason, and the positive leg (a well-formed entry must evaluate to result PASS against a policy that does not grant the probed access) cannot be exercised at all.
Fix: the CheckAccessNotGranted handler (services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/check_access_not_granted.rs) — implement the operation plus request validation of each `access` member, emitting the modeled ValidationException (with its required `reason`) when an entry carries neither actions nor resources.
Traces: accessanalyzer.check-access-not-granted-invalid-parameter-rejected (mismatch: "expected ValidationException, got NotImplemented").

Cause 3 of 3: UpdateAnalyzer does not reject a configuration update against an external-access (ACCOUNT) analyzer.
AWS: Smithy: UpdateAnalyzer (`PUT /analyzer/{analyzerName}`) takes `analyzerName` (@httpLabel, required) and optional `configuration` (AnalyzerConfiguration), and models `ValidationException` (400), `ResourceNotFoundException` (404), `ConflictException` (409), `AccessDeniedException` (403). The `configuration` surface is only meaningful for unused-access analyzers, so sending an `unusedAccess.analysisRule` configuration to a plain ACCOUNT (external-access) analyzer is rejected with HTTP 400 / `ValidationException` and the analyzer's configuration stays empty; recorded message "Unsupported operation" (example text). The same shape of call against an ACCOUNT_UNUSED_ACCESS analyzer succeeds and the exclusions persist and are readable back through GetAnalyzer.
Emulator: the call is answered with error code `NotImplemented`, so the type-dependent rule is absent and the persisting positive leg is unreachable.
Why: clients cannot distinguish "this analyzer type has no configuration" from a transport/implementation failure; a config-management client relying on the 400 to detect an analyzer-type mismatch instead sees an unknown error, and unused-access exclusion configuration cannot be round-tripped.
Fix: the UpdateAnalyzer handler (services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/update_analyzer.rs) plus analyzer state — implement the operation, gate `configuration` on the stored analyzer's type with the modeled ValidationException, and persist accepted unused-access configuration so GetAnalyzer reflects it.
Traces: accessanalyzer.update-analyzer-config-rejected-for-external-access-type (mismatch: "expected ValidationException, got NotImplemented").

## Evidence
Replay of suite simparity-behavioral-aws-v1 (accessanalyzer 2/25 passed):

1) accessanalyzer.create-service-linked-analyzer-access-denied-rejected [mismatch]
   check line 25: assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
   AssertionError: expected AccessDeniedException, got NotImplemented
   Expected: HTTP 403, Error.Code = AccessDeniedException (modeled on CreateServiceLinkedAnalyzer), no managedBy analyzer created.
   Actual:   Error.Code = NotImplemented, message "operation CreateServiceLinkedAnalyzer is scaffolded but not implemented in this emulator".

2) accessanalyzer.check-access-not-granted-invalid-parameter-rejected [mismatch]
   check line 39: assert code == "ValidationException", f"expected ValidationException, got {code}"
   AssertionError: expected ValidationException, got NotImplemented
   Request: policyDocument = identity policy allowing s3:GetObject on arn:aws:s3:::<bucket>/*, access = [{"actions": [], "resources": []}], policyType = IDENTITY_POLICY.
   Expected: HTTP 400, Error.Code = ValidationException (example message "The access object must have at least one action or one resource."); a well-formed entry then evaluates to result PASS.
   Actual:   Error.Code = NotImplemented for both legs.

3) accessanalyzer.update-analyzer-config-rejected-for-external-access-type [mismatch]
   check line 38: assert code == "ValidationException", f"expected ValidationException, got {code}"
   AssertionError: expected ValidationException, got NotImplemented
   Request: update_analyzer(analyzerName=<ACCOUNT-type analyzer>, configuration={"unusedAccess": {"analysisRule": {"exclusions": [{"resourceTags": [{"k": "v"}]}]}}}).
   Expected: HTTP 400, Error.Code = ValidationException (example message "Unsupported operation"), analyzer configuration unchanged/empty; identical call on an ACCOUNT_UNUSED_ACCESS analyzer succeeds and the exclusions are readable via GetAnalyzer.
   Actual:   Error.Code = NotImplemented, message "operation UpdateAnalyzer is scaffolded but not implemented in this emulator".

## Steps to Reproduce
Boot the emulator and point an accessanalyzer client at it.

Cause 1: call CreateServiceLinkedAnalyzer(type="ACCOUNT") with ordinary credentials. Expect HTTP 403 AccessDeniedException; observe NotImplemented. (Replay accessanalyzer.create-service-linked-analyzer-access-denied-rejected.)

Cause 2: call CheckAccessNotGranted(policyDocument=<any valid identity policy>, access=[{"actions": [], "resources": []}], policyType="IDENTITY_POLICY"). Expect HTTP 400 ValidationException; observe NotImplemented. Then call it with access=[{"actions":["s3:DeleteObject"],"resources":["arn:aws:s3:::<bucket>/*"]}] against a policy that only allows s3:GetObject and expect result == "PASS". (Replay accessanalyzer.check-access-not-granted-invalid-parameter-rejected.)

Cause 3: CreateAnalyzer(type="ACCOUNT"), wait until it leaves CREATING, then UpdateAnalyzer on it with configuration={"unusedAccess":{"analysisRule":{"exclusions":[{"resourceTags":[{"k":"v"}]}]}}}. Expect HTTP 400 ValidationException and an unchanged (empty) configuration; observe NotImplemented. Repeat against an ACCOUNT_UNUSED_ACCESS analyzer and expect success with the exclusions persisted. (Replay accessanalyzer.update-analyzer-config-rejected-for-external-access-type.)
