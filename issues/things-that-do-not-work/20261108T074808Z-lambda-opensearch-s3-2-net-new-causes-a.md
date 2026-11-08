Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative
Issue-Key: lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify|run#27|AssertionError|assert status_a1 == 200 and resp_a1.get("found") is True, (
Issue-Key: lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative|_poll_until#24|AssertionError|raise AssertionError(
Issue-Key: lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative|run#19|AssertionError|assert payload1["status"] in (200, 201), f"expected 200/201 pre-detach write, got {payload1}"
Issue-Commit: 3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e

# lambda__opensearch__s3: 2 net-new causes — a domain's published endpoint is not routed to the OpenSearch data plane (404 UnknownService for 'es'), and an authorized role's index write is refused 403 security_exception

> _Note: this issue was identified against the code base at commit `3f1b01ed3b5d1c2056caeeb85ce9541e8ea3070e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
Two net-new root causes in lambda__opensearch__s3, both on the OpenSearch domain data plane reached from a Lambda handler.

Cause 1 of 2: the endpoint a created OpenSearch domain publishes does not serve the OpenSearch data plane — a document write/read against it is answered 404 with an emulator routing error naming no service for 'es'. Failing checks: lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify, lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative.
AWS: recorded behavior only for the routing; the endpoint is a modelled output (DescribeDomain/DescribeDomains return DomainStatus.Endpoint). The host AWS publishes there serves the domain's HTTP data plane: `PUT /<index>/_doc/<id>` indexes a document and answers 200/201, and `GET /<index>/_doc/<id>` answers 200 with `"found": true`. The index name and document id come from the request.
Emulator: the published host answers nothing of the sort. Both checks observe HTTP 404 with body `{'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}` — the fixed text being `no emulated service is registered for '<name>'`, with the name being the service the host resolved to. So the domain exists in the control plane but its advertised endpoint addresses no data plane.
Why: a client that reads Endpoint from DescribeDomain and then indexes or searches — the normal way every OpenSearch workload is wired, including an S3-notification-driven indexer — gets a 404 from the very address the service told it to use. No document can be written or read, so no search behaviour, and no IAM/domain-policy behaviour on the data plane, can be modelled.
Fix: the OpenSearch domain's endpoint publication and request routing — the host returned in DomainStatus.Endpoint must be routed to the domain's HTTP data plane handler rather than falling through to the generic 'no emulated service is registered' responder.

Cause 2 of 2: a write by a role that DOES hold the domain-access grant is refused with 403 security_exception, so the positive control of the detach-blocks-access pair fails before anything is detached. Failing check: lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative.
AWS: recorded behavior only — the ES HTTP authorization envelope is not a Smithy shape. With the execution role holding es:ESHttpPost on the domain (and the domain access policy admitting it), `POST /<index>/_doc` answers 200/201 and indexes the document; only after the policy is detached does AWS answer 403 with a security_exception whose reason names the principal, the action and the resource.
Emulator: the authorized write is refused. The pre-detach write answers `{'status': 403, 'error': '{"error":{"root_cause":[{"type":"security_exception","reason":"User: <assumed-role principal ARN> is not authorized to perform: es:ESHttpPost on resource: <domain ARN>/<index> because no resource-based policy allows the es:ESHttpPost action"}], ...},"status":403}'}` — the correct refusal shape, but produced for a caller that is in fact authorized.
Why: the authorization decision is inverted for the granted case, so a client sees a correctly-configured role denied. A negative test can no longer prove anything (deny before and after a detach look identical), and any workload whose role was granted access is simply broken.
Fix: the OpenSearch data plane's authorization evaluation — it must admit a caller whose identity policy grants the es: action on the domain/index and whose domain access policy allows it, and refuse only when neither does.

Note: 20261107T173234Z-lambda-opensearch-s3-an-s3-event-notification-with.md and 20261105T111951Z-lambda-opensearch-s3-the-lambda-python-runtime-has.md are open issues in this service; they own other checks here and carry neither of these three replay keys.

Traces: the three checks named above.

## Evidence
Cause 1 — Expected (AWS): against the host in DomainStatus.Endpoint, `PUT /<index>/_doc/<id>` answers 200/201 and `GET /<index>/_doc/<id>` answers 200 with `"found": true`.
Actual (emulator): HTTP 404 with `{'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}`.
  - cli-update-lambda-env-opensearch-endpoint-verify: `AssertionError: expected doc-1 to be found on the OLD domain (written before the env update): 404 {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}`
  - s3-notification-lambda-opensearch-write-failure-negative: `AssertionError: doc indexed once role is granted domain access not met within 120s. Last result: (404, {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"})`

Cause 2 — Expected (AWS): with es:ESHttpPost granted, the pre-detach `POST /<index>/_doc` answers 200 or 201.
Actual (emulator): 403 with a security_exception body whose reason is `User: <principal> is not authorized to perform: es:ESHttpPost on resource: <domain ARN>/<index> because no resource-based policy allows the es:ESHttpPost action`.
  - cli-lambda-role-policy-detach-blocks-opensearch-access-negative: `AssertionError: expected 200/201 pre-detach write, got {'status': 403, ...}`. The principal ARN, domain ARN and index name in the recorded body are this run's values; the requirement is that the authorized write is admitted, and that the 403 appears only after the grant is removed.

## Steps to Reproduce
Cause 1: create an OpenSearch domain, read DomainStatus.Endpoint from DescribeDomain, and from a Lambda handler PUT a document to `https://<endpoint>/<index>/_doc/<id>` then GET it back. Observe 404 `no emulated service is registered for 'es'` instead of 200/201 and found=true. Replay lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify and lambda__opensearch__s3.s3-notification-lambda-opensearch-write-failure-negative.

Cause 2: grant the Lambda execution role es:ESHttpPost on the domain and admit it in the domain access policy; from the handler POST a document to `https://<endpoint>/<index>/_doc`. Observe 403 security_exception instead of 200/201. Then detach the policy and confirm the 403 is indistinguishable from the authorized case. Replay lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative.
