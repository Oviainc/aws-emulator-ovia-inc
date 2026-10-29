Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr-public.complete-layer-upload-not-found
Issue-Shape-Id: com.amazonaws.ecrpublic#SpencerFrontendService
Issue-Key: ecr-public.complete-layer-upload-not-found|run#16|AssertionError|assert malformed_message == (
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# ECR Public: the @pattern-constraint InvalidParameterException message omits AWS's 'Member must satisfy regular expression pattern:' wording and quotes the pattern instead

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteLayerUpload

## Description
Behavioral replay of the simparity-behavioral-aws-v1 ecr-public suite (85/92 passed; ecr-public 84/91). The emulator IS serving and ecr-public IS bootstrapped (service-com-amazonaws-ecrpublic-spencerfrontendservice is in the bootstrapped set), so this is not a bootstrap gap. Six of the seven ecr-public failures are marked OWNED by an open issue and are skipped; exactly ONE net-new root cause survives and is filed here.

=== Cause 1 of 1: the InvalidParameterException raised for a modelled @pattern violation uses a non-AWS message template ===

AWS: Smithy CompleteLayerUpload input member uploadId -> com.amazonaws.ecrpublic#UploadId, required, @pattern "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"; the operation models InvalidParameterException (client, httpError 400, doc "The specified parameter is invalid. Review the available parameters for the API request."). An uploadId that is not a UUID fails input validation BEFORE any upload lookup, so it is refused 400 InvalidParameterException and never reaches the UploadNotFoundException path. ECR Public renders the refusal from the request as a fixed template naming the member path and the violated rule:
  "Invalid parameter at '<member>' failed to satisfy constraint: 'Member must satisfy regular expression pattern: <pattern>'"
Its FIXED text is "Invalid parameter at '", "' failed to satisfy constraint: 'Member must satisfy regular expression pattern: " and the closing "'". Its VARIABLE parts are (a) <member>, the offending input member's path, taken from the request, and (b) <pattern>, the member's modelled @pattern rendered WITHOUT the ^ and $ anchors and WITHOUT any surrounding quotes. For this operation that instantiates as "Invalid parameter at 'uploadId' failed to satisfy constraint: 'Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}'". The template is the requirement; the uploadId value sent by any one run is not.

Emulator: HTTP status 400 and error code InvalidParameterException are both CORRECT, and the positive control passes (a real InitiateLayerUpload -> UploadLayerPart -> CompleteLayerUpload completes 200 echoing layerDigest and the layer reads back AVAILABLE from BatchCheckLayerAvailability), so the operation is implemented and only the error body diverges. The message is a different template:
  "Invalid parameter at 'uploadId' failed to satisfy constraint: 'must satisfy regular expression '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}''"
Three divergences in the fixed text: the rule clause drops the leading "Member " qualifier; it says "regular expression" where AWS says "regular expression pattern:"; and it wraps the pattern in single quotes (producing a doubled trailing quote) where AWS emits the bare pattern after a colon. The member path and the pattern body themselves are correct.

Why: this message is the only place the service states which member and which rule failed. A client that distinguishes a malformed uploadId (fix the caller: re-mint a UUID) from a well-formed but unissued one (restart the upload session) parses the InvalidParameterException body to do so, and SDK wrappers, CLIs and conformance suites that match AWS's "Member must satisfy regular expression pattern:" clause extract nothing from the emulator's wording. The doubled quote also makes the pattern unparseable by a consumer that unquotes the constraint clause. Because the check aborts on this assertion, the second leg of the same operation — a well-formed UUID that was never issued, which must answer 400 UploadNotFoundException with "The upload with id '<uploadId>' in the repository with name '<repo>' in registry with id '<account>' does not exist" — is left UNVERIFIED by this replay and should be confirmed once this cause is fixed.

Fix: the shared modelled-@pattern -> InvalidParameterException rendering for this service (the input-validation layer in front of the ecr-public operation handlers, the same layer that renders @length violations) must emit AWS's clause verbatim: "Member must satisfy regular expression pattern: " followed by the unanchored, unquoted pattern. Related but DISTINCT: cause 3 of the open issue 20261023T055032Z-ecr-public-getauthorizationtoken-is-not-authorization-gated-stored.md reports the @LENGTH branch of this same validation layer emitting the generic "N validation error detected: Value '<value>' at '<member>' ..." framework preamble; that issue does not carry the @pattern branch or this check's replay key, so if the validation layer is corrected wholesale this check should be re-replayed to confirm this cause closes with it.

Traces: ecr-public.complete-layer-upload-not-found.

## Evidence
Replay: simparity-behavioral-aws-v1. Total 85/92 passed (92.4%); ecr-public 84/91 (92%) with 7 failures, ecr-public__iam 1/1. Six failures are marked "OWNED by" an open issue (catalog-logo-blob-is-input-only-and-size-capped -> 20261025T010839Z-...putrepositorycatalogdata-accepts-an-under-sized-logoimageblob.md; catalog-logo-blob-is-stored-and-served-as-a-real-image, repository-catalog-data-base64-logo-round-trip, repository-policy-document-byte-preserved, repository-policy-self-lockout-with-force-actually-blocks-set-repository-policy -> 20261023T055032Z-...getauthorizationtoken-is-not-authorization-gated-stored.md; registry-catalog-display-name-round-trip-and-limit -> 20261029T104628Z-...putregistrycatalogdata-still-refuses-the-suite-s.md) and are excluded. No failure was marked ALREADY FILED.

Cause 1 — [mismatch] ecr-public.complete-layer-upload-not-found (GROUP error-semantics; tags axis:rejection, cap:c1, concern:negative-classes, plane:control):

The check first runs a positive control that PASSES: CreateRepository, InitiateLayerUpload, UploadLayerPart of a 51-byte blob, CompleteLayerUpload returning layerDigest equal to the sha256 of the uploaded bytes, then BatchCheckLayerAvailability reporting that digest AVAILABLE. CompleteLayerUpload is therefore implemented and succeeding (the formerly open scaffold-stub issue 20261023T055032Z-ecr-public-completelayerupload-stub.md is fixed); this finding is NOT a regression of that fix and the call is not wrongly refused — AWS refuses this leg too, with a different message.

It then sends CompleteLayerUpload(repositoryName=<repo>, uploadId="bogus-upload-id-does-not-exist", layerDigests=["sha256:" + "2"*64]). Error Code is InvalidParameterException as required. The assertion at check_ecr_public_complete_layer_upload_not_found.py line 92 fails on the message:
  expected: "Invalid parameter at 'uploadId' failed to satisfy constraint: 'Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}'"
  observed: "Invalid parameter at 'uploadId' failed to satisfy constraint: 'must satisfy regular expression '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}''"
  harness: AssertionError: InvalidParameterException message: "Invalid parameter at 'uploadId' failed to satisfy constraint: 'must satisfy regular expression '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}''"

Model corroboration (smithy_query on services/service-com-amazonaws-ecrpublic-spencerfrontendservice/smithy-model/com-amazonaws-ecrpublic-spencerfrontendservice.json): CompleteLayerUpload input uploadId -> UploadId, required, pattern "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"; errors include InvalidParameterException (client, 400) and UploadNotFoundException (client, 400).

Because the check aborts at this assertion, the subsequent UploadNotFoundException leg (a well-formed uuid4 never issued by InitiateLayerUpload, expecting "The upload with id '<uuid>' in the repository with name '<repo>' in registry with id '<account>' does not exist") was never exercised and its fidelity is unknown from this run.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, and replay the simparity-behavioral-aws-v1 ecr-public suite (run_behavioral_suite).
2. Replay ecr-public.complete-layer-upload-not-found.
3. Cause 1: CreateRepository a repository, then call CompleteLayerUpload(repositoryName=<repo>, uploadId="bogus-upload-id-does-not-exist", layerDigests=["sha256:"+"2"*64]) — an uploadId that violates the modelled @pattern on UploadId. Assert the response is HTTP 400 with Error.Code InvalidParameterException (the emulator already satisfies this) AND Error.Message exactly "Invalid parameter at 'uploadId' failed to satisfy constraint: 'Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}'". The emulator instead omits the "Member " qualifier, says "regular expression" rather than "regular expression pattern:", and wraps the pattern in single quotes.
4. Confirm the shared fix generalizes: send any other ecr-public request that violates a modelled @pattern (e.g. CreateRepository with a repositoryName that breaks ^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$) and check the same template is emitted with that member's path and unanchored pattern, with no value interpolation.
5. Verify the cause did not mask the next leg: in the same check, CompleteLayerUpload with a well-formed uuid4 that InitiateLayerUpload never issued must answer 400 UploadNotFoundException with "The upload with id '<uploadId>' in the repository with name '<repo>' in registry with id '<account>' does not exist". This leg is unverified until the message above is corrected and the check gets past line 92.
6. Regression guard: the check's positive control (InitiateLayerUpload -> UploadLayerPart -> CompleteLayerUpload echoing layerDigest -> BatchCheckLayerAvailability reporting AVAILABLE) must keep passing, so the validation change must not start refusing legitimate uploadIds.
