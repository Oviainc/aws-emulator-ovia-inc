Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.update-index-insufficient-permission-denied
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.update-index-insufficient-permission-denied|run#27|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Commit: 85a3e609e0397cc56b94fbca2b474c505c96ae80

# opensearch UpdateIndex answers 501 NotImplemented instead of 403 AccessDeniedException for an unauthorized principal (and never applies a permitted mapping update)

> _Note: this issue was identified against the code base at commit `85a3e609e0397cc56b94fbca2b474c505c96ae80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIndex

## Description
Cause 1 of 1: UpdateIndex is not served, so neither its authorization refusal nor its success path can happen. Failing check: opensearch.update-index-insufficient-permission-denied.

AWS: Smithy model (com-amazonaws-opensearch-amazonopensearchservice) models UpdateIndex as PUT /2021-01-01/opensearch/domain/{DomainName}/index/{IndexName} with required DomainName (@length 3-28, pattern ^[a-z][a-z0-9\-]+$), IndexName (@length 1-255) and IndexSchema (document), output member Status (enum IndexStatus, required, success code 200), and errors AccessDeniedException (httpError 403), DependencyFailureException (424), DisabledOperationException (409), InternalException (500), ResourceNotFoundException (409), ThrottlingException (429), ValidationException (400). NotImplemented is not a modeled error of this operation. Recorded behavior: a caller whose IAM identity policy grants only es:DescribeDomain and es:GetIndex on the domain is refused with error code AccessDeniedException; the recorded message is an example of the standard IAM denial form built from the request ("User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: es:UpdateIndex on resource: <domain ARN> because no identity-based policy allows the es:UpdateIndex action"), the requirement being the code/status, the named action es:UpdateIndex and the named resource. The denied call leaves the index mapping untouched, and the same request issued by a principal that does hold es:UpdateIndex returns Status=UPDATED and the new mapping property is visible through GetIndex.

Emulator: every UpdateIndex request is answered with error code 'NotImplemented' (the unimplemented-scaffold response), so the check's first assertion fails: expected AccessDeniedException, got 'NotImplemented'. Because the operation never runs, the two follow-on requirements are unreachable as well: there is no evidence the denied call is a no-op on the stored schema, and the permitted positive control can never return Status=UPDATED.

Why: a client cannot update an index mapping through the control-plane API at all, and — more damaging for anything that tests policy — an under-privileged principal gets an infrastructure-looking NotImplemented instead of the modeled 403 AccessDeniedException. SDK retry/error handling branches on the error code, so callers that expect to distinguish "not authorized" from "service fault" mis-handle the response, and least-privilege regression tests pass or fail for the wrong reason.

Fix: the UpdateIndex handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_index.rs) plus the domain/index state it must mutate and the IAM authorization check that must precede the mutation; the serializer must render the modeled AccessDeniedException with HTTP 403 and the message form above. Note: a scaffold-stub issue naming UpdateIndex and that same handler file is already filed for the sibling check opensearch.update-index-optimistic-lock-conflicting-mapping-writes; it covers only the operation being a stub and does NOT carry this check's authorization requirement (403 AccessDeniedException naming es:UpdateIndex, the denied call being a no-op, and the permitted control returning Status=UPDATED), so that requirement is recorded here.

Traces: opensearch.update-index-insufficient-permission-denied (mismatch: AssertionError at check line 108, "expected AccessDeniedException, got 'NotImplemented'").

## Evidence
Check opensearch.update-index-insufficient-permission-denied (verdict: mismatch).

Expected (real AWS): UpdateIndex by a role whose only grants are es:DescribeDomain and es:GetIndex -> HTTP 403, Error.Code = "AccessDeniedException", message of the form "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: es:UpdateIndex on resource: <domain ARN> because no identity-based policy allows the es:UpdateIndex action" (example text; the code/status, the named action and the named resource are the requirement). Index mapping unchanged afterwards (GetIndex shows no 'summary' property). Same UpdateIndex by the fully-permissioned caller -> HTTP 200 with Status = "UPDATED", and GetIndex then shows the 'summary' property.

Actual (emulator): UpdateIndex returns Error.Code = "NotImplemented" ("operation UpdateIndex is scaffolded but not implemented in this emulator"). Assertion failure: `assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"` -> AssertionError: expected AccessDeniedException, got 'NotImplemented'. The later assertions (no-op on denial, Status=UPDATED for the permitted call) were never reached.

Smithy: UpdateIndex errors = AccessDeniedException(403), DependencyFailureException(424), DisabledOperationException(409), InternalException(500), ResourceNotFoundException(409), ThrottlingException(429), ValidationException(400); output.Status: IndexStatus (required). No NotImplemented error is modeled.

## Steps to Reproduce
1. CreateDomain with EngineVersion=OpenSearch_2.19, ClusterConfig {InstanceType: t3.small.search, InstanceCount: 1}, EBSOptions {EBSEnabled: true, VolumeType: gp3, VolumeSize: 10}; wait until DescribeDomain reports Created=true and Processing=false.
2. CreateIndex on that domain with IndexSchema {"mappings": {"properties": {"title": {"type": "text"}}}}.
3. iam:CreateRole with a trust policy allowing the account root to sts:AssumeRole, then PutRolePolicy granting ONLY ["es:DescribeDomain", "es:GetIndex"] on [<domain ARN>, <domain ARN>/*]. sts:AssumeRole into it.
4. With the assumed-role credentials call UpdateIndex(DomainName=<domain>, IndexName=<index>, IndexSchema={"mappings": {"properties": {"summary": {"type": "text"}}}}).
   Expected: HTTP 403 AccessDeniedException naming es:UpdateIndex and the domain ARN. Observed: NotImplemented.
5. GetIndex with the privileged client: the mapping must still have no 'summary' property.
6. Repeat step 4 with the fully-permissioned client: expected HTTP 200 with Status="UPDATED", and GetIndex then shows the 'summary' property.
