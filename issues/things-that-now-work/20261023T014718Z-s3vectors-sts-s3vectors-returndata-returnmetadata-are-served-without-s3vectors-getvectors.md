Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors__sts.list-vectors-return-data-requires-getvectors-permission s3vectors__sts.query-vectors-return-metadata-requires-getvectors-permission
Issue-Key: s3vectors__sts.list-vectors-return-data-requires-getvectors-permission|_refusal#37|AssertionError|raise AssertionError(f"{what} succeeded, expected a refusal")
Issue-Key: s3vectors__sts.query-vectors-return-metadata-requires-getvectors-permission|_poll#39|Timeout|time.sleep(interval_s)
Issue-Commit: c1cdf4b562f81ba1ac84179127e5df6b90873b05

# s3vectors returnData/returnMetadata are served without s3vectors:GetVectors: the parameter-conditional 403 is never raised for ListVectors or QueryVectors

> _Note: this issue was identified against the code base at commit `c1cdf4b562f81ba1ac84179127e5df6b90873b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListVectors

## Description
One root cause, reached through two STS-scoped sessions.

=== Cause 1 of 1: the GetVectors permission gate on returnData/returnMetadata (and on filter) is not enforced ===
AWS: ListVectors' Permissions block in this service's Smithy model states that the caller must hold s3vectors:ListVectors, and that "If you set returnData or returnMetadata to true, you must have both s3vectors:ListVectors and s3vectors:GetVectors permissions. The request fails with a 403 Forbidden error" if it does not. QueryVectors' own documentation states the matching rule: "The request fails with a 403 Forbidden error if you request metadata filtering or metadata without the s3vectors:GetVectors permission." So the gate is CONDITIONAL ON THE REQUEST PARAMETERS, not on the operation: with ListVectors/QueryVectors but not GetVectors, the keys-only call (returnData=false, returnMetadata=false, no filter) SUCCEEDS while the identical call with returnData=true, with returnMetadata=true, or with a filter is refused with HTTP 403. Smithy models AccessDeniedException (HTTP 403) on ListVectors; it is NOT listed among QueryVectors' modeled errors even though QueryVectors' prose documents the 403 — the code recorded for the s3vectors 403 elsewhere in this suite is AccessDeniedException, so that is the expectation (the model gap itself is worth noting while fixing).
Emulator: no refusal is raised at any point. For an AssumeRole session whose session policy allows only s3vectors:ListVectors, ListVectors(returnData=true) SUCCEEDS ("returnData=true without s3vectors:GetVectors succeeded, expected a refusal"). The QueryVectors twin never reaches a refusal either and the check runs out its wall clock (timed out after 60s) while the calls keep succeeding. This is the parameter-conditional half of the s3vectors authorization gap; the unconditional half (ListVectors served to a principal with no ListVectors grant at all) is filed separately against service s3vectors.
Why: returnData/returnMetadata/filter are exactly how vector payloads and tenant metadata leave the service. A client that relies on withholding s3vectors:GetVectors to let a principal enumerate keys but never read vectors or metadata sees that boundary silently removed: the emulator hands back the data and the metadata to a session AWS would refuse with 403, so a privilege-escalation regression in the application is undetectable here and the keys-only-vs-data distinction cannot be tested.
Fix: s3vectors request authorization — the policy evaluation that runs before the ListVectors and QueryVectors handlers must take the request parameters into account (returnData, returnMetadata, and QueryVectors' filter each additionally require s3vectors:GetVectors) and refuse with AccessDeniedException (HTTP 403); it must also honour an STS session policy as the intersection with the role's identity policy.
Traces: s3vectors__sts.list-vectors-return-data-requires-getvectors-permission, s3vectors__sts.query-vectors-return-metadata-requires-getvectors-permission.

## Evidence
Expected: HTTP 403 AccessDeniedException for ListVectors(returnData=true) and for ListVectors(returnMetadata=true) on a session allowed only s3vectors:ListVectors, and for QueryVectors(returnMetadata=true) / QueryVectors(filter=…) on a session allowed only s3vectors:QueryVectors — while the keys-only call on the same session succeeds, and a session that adds s3vectors:GetVectors serves all of them.
Actual: AssertionError: returnData=true without s3vectors:GetVectors succeeded, expected a refusal (s3vectors__sts.list-vectors-return-data-requires-getvectors-permission, raised from _refusal). The QueryVectors twin (s3vectors__sts.query-vectors-return-metadata-requires-getvectors-permission) produced no refusal and ended as "Check timed out after 60.0s" with its calls still succeeding.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite) against the built emulator.
2. Create a vector bucket and an index and PutVectors a couple of vectors WITH metadata, using the runner identity.
3. Create an IAM role trusted by arn:aws:iam::<account>:root with an s3vectors identity policy, and AssumeRole twice: once with a session policy allowing only s3vectors:ListVectors (resp. only s3vectors:QueryVectors), once with a session policy that also allows s3vectors:GetVectors.
4. On the narrow session, call ListVectors(vectorBucketName, indexName) with returnData=false/returnMetadata=false — it should succeed (positive control).
5. On the SAME narrow session, call ListVectors with returnData=true, then with returnMetadata=true; and call QueryVectors with returnMetadata=true, then with a filter. Each must be refused with 403 AccessDeniedException; observe that every one of them succeeds instead.
6. On the wide session (with s3vectors:GetVectors), the same calls succeed — confirming the refusal in step 5 is the missing parameter-conditional gate and not a blanket denial.
