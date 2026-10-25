Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.update-index-insufficient-permission-denied
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.update-index-insufficient-permission-denied|_error_detail#26|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 38eb4bdb9d2631b608032d74dbaf86313a4fa366

# opensearch UpdateIndex performs no IAM authorization check: a principal without es:UpdateIndex gets 200 Status=UPDATED instead of 403 AccessDeniedException (1 cause)

> _Note: this issue was identified against the code base at commit `38eb4bdb9d2631b608032d74dbaf86313a4fa366`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIndex

## Description
Cause 1 of 1: UpdateIndex now answers successfully but never evaluates the caller's identity-based policy, so an under-privileged principal mutates the index mapping instead of being refused. Failing check: opensearch.update-index-insufficient-permission-denied.

NOTE — related open issue, different failure mode: 20261025T111158Z-opensearch-updateindex-answers-501-notimplemented-instead-of-403.md carries this same check id but records UpdateIndex answering 501 NotImplemented (the scaffold-stub response). The operation is no longer a stub — the emulator now returns HTTP 200 — so the refusal requirement in that issue is NOT what is failing here any more; the request is accepted. That issue is open, not fixed, so this is not a reopen; it is the authorization gap that remains once the operation is served, recorded here because the suite did not mark this check as owned by it.

AWS: Smithy (com.amazonaws.opensearch#AmazonOpenSearchService) models UpdateIndex as PUT /2021-01-01/opensearch/domain/{DomainName}/index/{IndexName}, success code 200, input DomainName (@httpLabel, required, @length 3-28, pattern ^[a-z][a-z0-9\-]+$), IndexName (@httpLabel, required, @length 1-255) and IndexSchema (document, required); output member Status (enum IndexStatus, required); modeled errors AccessDeniedException (httpError 403), DependencyFailureException (424), DisabledOperationException (409), InternalException (500), ResourceNotFoundException (409), ThrottlingException (429), ValidationException (400). The authorization semantics are recorded behavior: a caller whose IAM identity policy grants only es:DescribeDomain and es:GetIndex on the domain ARN (and <domain ARN>/*) is refused with HTTP 403 and Error.Code = "AccessDeniedException", carrying the standard IAM denial message form. That message's FIXED text is "User: " / " is not authorized to perform: " / " on resource: " / " because no identity-based policy allows the " / " action"; its VARIABLE parts come from the request and state — the calling principal's ARN (the assumed-role session ARN arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>), the action name es:UpdateIndex (fixed for this operation) and the target domain's ARN. Nothing from any single run (a particular account id, role name, session name or domain name) is part of the requirement. The denied call is a no-op: GetIndex afterwards still shows the pre-call mappings. The same request from a principal that does hold es:UpdateIndex returns HTTP 200 with Status = "UPDATED" and the new mapping property becomes visible through GetIndex.

Emulator: UpdateIndex issued with credentials from an assumed role whose only grants are es:DescribeDomain and es:GetIndex is ACCEPTED — HTTP 200, body {"Status": "UPDATED"} (content-type application/json, content-length 20) — so no AccessDeniedException is produced at all and the unauthorized caller's mapping change is applied. The check's helper that expects a rejection raises AssertionError: "expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Status': 'UPDATED'}". Because the call succeeded, the two follow-on requirements the check grades after the refusal — that the denied call left the stored schema untouched, and that the permitted principal's identical call is the one that flips Status to UPDATED — are not observable either: the privileged and the unprivileged caller are indistinguishable.

Why: a client relying on AWS's behavior loses the authorization boundary on the OpenSearch control plane. Least-privilege policies are silently ineffective — a role deliberately granted read-only access to a domain can rewrite index mappings, which is a privilege escalation against anything the emulator is used to validate (policy regression suites, CI guardrails, IaC least-privilege reviews all go green while the real deployment would be refused). Callers that branch on Error.Code = AccessDeniedException to surface "not authorized" never take that branch, and code paths that treat a 200 as proof the principal is permitted draw the opposite conclusion from the truth.

Fix: the UpdateIndex handler's authorization step — the IAM policy evaluation that must run BEFORE the index-schema mutation in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_index.rs, resolving the signing principal (including an sts:AssumeRole session), evaluating the identity-based policies attached to it against action es:UpdateIndex on the domain ARN, and returning the modeled AccessDeniedException with HTTP 403 and the message form above when no statement allows it; the handler must leave the stored domain/index state unmodified on that path, and keep the existing 200/Status=UPDATED path only for an allowed principal.

Traces: opensearch.update-index-insufficient-permission-denied (mismatch: AssertionError at check line 103/147 — "expected a rejection, but the call SUCCEEDED and returned ... 'Status': 'UPDATED'").

## Evidence
Check opensearch.update-index-insufficient-permission-denied (verdict: mismatch).

Expected (real AWS): UpdateIndex called with credentials of a role whose only grants are es:DescribeDomain and es:GetIndex on the domain -> HTTP 403, Error.Code = "AccessDeniedException", message of the form "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: es:UpdateIndex on resource: <domain ARN> because no identity-based policy allows the es:UpdateIndex action" (fixed text as quoted; principal ARN and domain ARN taken from the request/state). GetIndex afterwards shows no 'summary' property. The identical UpdateIndex from the fully-permissioned caller -> HTTP 200 with Status = "UPDATED", and GetIndex then shows 'summary'.

Actual (emulator): the unauthorized UpdateIndex returned HTTP 200 with body {'Status': 'UPDATED'} — no error code, no 403. Replay output: AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '20', ...}, 'RetryAttempts': 0}, 'Status': 'UPDATED'} (raised from the check's _error_detail helper at line 147, called from line 103). The assertions on the denial being a no-op and on the permitted positive control were never reached.

Smithy: UpdateIndex -> PUT /2021-01-01/opensearch/domain/{DomainName}/index/{IndexName}, success 200; errors AccessDeniedException(403), DependencyFailureException(424), DisabledOperationException(409), InternalException(500), ResourceNotFoundException(409), ThrottlingException(429), ValidationException(400); output.Status: IndexStatus (required).

Difference from the already-open issue 20261025T111158Z-opensearch-updateindex-answers-501-notimplemented-instead-of-403.md: there the emulator refused every UpdateIndex with Error.Code 'NotImplemented'; here it accepts the request with 200/Status=UPDATED. The refusal is gone, the authorization check is still absent.

## Steps to Reproduce
1. CreateDomain with EngineVersion=OpenSearch_2.19, ClusterConfig {InstanceType: t3.small.search, InstanceCount: 1, DedicatedMasterEnabled: false, ZoneAwarenessEnabled: false}, EBSOptions {EBSEnabled: true, VolumeType: gp3, VolumeSize: 10}; poll DescribeDomain until Created=true and Processing=false. Keep DomainStatus.ARN.
2. CreateIndex on that domain with IndexSchema {"mappings": {"properties": {"title": {"type": "text"}}}}.
3. iam:CreateRole with a trust policy allowing arn:aws:iam::<account>:root to sts:AssumeRole, then iam:PutRolePolicy granting ONLY ["es:DescribeDomain", "es:GetIndex"] on [<domain ARN>, "<domain ARN>/*"]. sts:AssumeRole into that role.
4. With the assumed-role credentials call UpdateIndex(DomainName=<domain>, IndexName=<index>, IndexSchema={"mappings": {"properties": {"summary": {"type": "text"}}}}).
   Expected: HTTP 403 with Error.Code "AccessDeniedException" naming action es:UpdateIndex and the domain ARN. Observed: HTTP 200 with {"Status": "UPDATED"}.
5. GetIndex with the privileged client: the mapping must still have no 'summary' property (the denied call must be a no-op).
6. Repeat step 4 with the fully-permissioned default client: expected HTTP 200 with Status="UPDATED", and GetIndex then shows the 'summary' property.
