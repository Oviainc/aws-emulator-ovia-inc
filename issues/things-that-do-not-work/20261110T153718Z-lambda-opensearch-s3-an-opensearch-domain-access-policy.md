Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative
Issue-Key: lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative|_invoke#19|Timeout|resp = lam.invoke(FunctionName=function_name, Payload=json.dumps(payload).encode("utf-8"))
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# lambda__opensearch__s3: an OpenSearch domain access policy that Denies a role's ESHttpPut is never enforced (and never answers) — the identity-vs-resource-policy boundary is unobservable

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDomainConfig (AccessPolicies) / ESHttpPut at the domain endpoint

## Description
One net-new root cause in the lambda__opensearch__s3 suite. It is NOT covered by the open issues for this heading (20261109T062930Z-lambda-opensearch-s3-the-opensearch-domain-data-plane.md, 20261105T111951Z-lambda-opensearch-s3-the-lambda-python-runtime-has.md, 20261104T232822Z, 20261108T074808Z, 20261103T020000Z, 20261109T160051Z), which carry other checks' replay keys; this check carries none.

Cause 1 of 1: a domain access policy whose explicit Deny targets a role's es:ESHttpPut on the domain's resource never produces an answer, so the rule that a resource-policy Deny outranks a broad identity-policy Allow cannot be observed.
AWS: recorded behavior only — domain access-policy evaluation is not modelled on the wire. With the caller role's own IAM policy allowing es:ESHttp* broadly, a domain AccessPolicies document that explicitly Denies es:ESHttpPut for that role principal on arn:aws:es:<region>:<account>:domain/<name>/* makes the write fail at the domain endpoint with an authorization refusal, even though identity policy allows it; replacing the domain access policy with one that does not deny (identity permissions untouched) makes the identical write succeed.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached — no response ever arrives from the domain endpoint for either the denied or the repaired configuration, so neither the refusal nor the subsequent success was ever compared.
Why: the domain access policy is the only control that scopes an OpenSearch domain independently of IAM identity policy. With it unenforced and unanswering, a client cannot confine a domain at all, and a configuration that denies nothing is indistinguishable from one that denies correctly — so the negative control passes vacuously and a real misconfiguration is invisible.
Fix: the OpenSearch domain data plane's authorization step — it must evaluate the domain's AccessPolicies document against the signed caller for each ESHttp* action, refuse when an explicit Deny matches regardless of identity-policy allows, and answer rather than hang in both directions.
Traces: lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative.

## Evidence
Check lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The awaited requirement, from the check's own docstring: "A domain access policy that explicitly Denies a role's ESHttpPut is enforced even though the role's own IAM policy broadly allows es:ESHttp*; fixing the domain access policy (identity/IAM permissions unchanged) makes the same write succeed." The check builds domain_arn = f"arn:aws:es:{ctx.region}:{account}:domain/{domain_name}" and resource = f"{domain_arn}/*".

Expected: the ESHttpPut through the Lambda is refused while the Deny is in place and succeeds after the domain access policy is replaced, with the role's identity policy unchanged throughout. Actual: nothing answers inside the run budget, so the check is killed with no status or error compared. Domain names, account ids and index names are per-run values.

## Steps to Reproduce
1. Create an OpenSearch domain and wait for it to be reachable; create a role whose IAM policy allows es:ESHttp* on the domain.
2. UpdateDomainConfig with an AccessPolicies document containing an explicit Deny of es:ESHttpPut for that role principal on arn:aws:es:<region>:<account>:domain/<name>/*.
3. From a Lambda using that role, issue an ESHttpPut (index a document) at the domain endpoint: AWS refuses on the domain access policy.
4. Replace AccessPolicies with a document that does not deny (identity policy unchanged) and repeat the same write: AWS now succeeds.
5. The emulator answers neither call. Re-replay lambda__opensearch__s3.lambda-sdk-opensearch-domain-access-policy-denies-negative to confirm the fix.
