Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-client-certificate-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-client-certificate-lifecycle apigw-patch-clientcert-invalid-path apigateway-test-invoke-lambda-proxy-echo apigateway-usage-plan-key-association-and-usage-tracking
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetClientCertificate|2xx->2xx|$.tags
Issue-Key: apigateway|UpdateClientCertificate|2xx->2xx|$.tags
Issue-Key: apigateway|TestInvokeMethod|2xx->2xx|$.headers.X-Amzn-Trace-Id
Issue-Key: apigateway|TestInvokeMethod|2xx->2xx|$.multiValueHeaders.X-Amzn-Trace-Id
Issue-Key: apigateway|GetUsage|4xx->4xx|$.message
Issue-Related: 20261029T200808Z-apigateway-api-gateway-authorizer-representation-and-testinvokemethod-execution-9.md
Issue-Commit: c01090bc0e97483ffdf7a7efd2d0a67727486686

# API Gateway REST responses omit modeled members and reword validation failures

> _Note: this issue was identified against the code base at commit `c01090bc0e97483ffdf7a7efd2d0a67727486686`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetClientCertificate

## Description
4 causes. Three REST representations drop members AWS always serializes, and one 400 refusal replaces AWS's sentence with emulator-authored wording.

Cause 1 of 4 - GetClientCertificate for a client certificate that carries no tags.
AWS: 200 with the ClientCertificate representation including tags as an empty JSON object ({}), not omitted. Smithy: GetClientCertificate GET /clientcertificates/{clientCertificateId}, output shape ClientCertificate with member tags -> MapOfStringToString; the recorded response shows tags: {} for an untagged certificate.
Emulator: 200 but tags is absent from the body.
Why: a client that reads response['tags'] to enumerate or merge tags hits a missing key instead of an empty map, so tag-reconciliation code raises KeyError/NPE on every untagged certificate rather than seeing 'no tags'.
Fix: the ClientCertificate serializer - emit tags always, as {} when the certificate has none.
Traces: apigateway-client-certificate-lifecycle#task#2 (2 steps in the cluster).

Cause 2 of 4 - UpdateClientCertificate (PATCH) on a certificate with no tags: the same omission on the write path.
AWS: 200 with tags: {} present in the returned ClientCertificate. Smithy: UpdateClientCertificate PATCH /clientcertificates/{clientCertificateId}, output shape ClientCertificate (member tags -> MapOfStringToString).
Emulator: 200 with tags absent.
Why: same as cause 1 - the representation returned by the update is not round-trippable with the one a client expects, so read-modify-write loops over tags break.
Fix: the same ClientCertificate serializer, exercised from UpdateClientCertificate's handler.
Traces: apigateway-client-certificate-lifecycle#task#4 (2 steps in the cluster).

Cause 3 of 4 - TestInvokeMethod against an AWS_PROXY (Lambda proxy) method.
AWS: 200 and the TestInvokeMethodResponse headers/multiValueHeaders maps include an X-Amzn-Trace-Id entry (the tracing header API Gateway always attaches; its value is per-run and the fixture asserts only its presence/shape). Smithy: TestInvokeMethod POST /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}, output members headers -> MapOfStringToString and multiValueHeaders -> MapOfStringToList.
Emulator: 200 but neither headers.X-Amzn-Trace-Id nor multiValueHeaders.X-Amzn-Trace-Id is present; the emitted $.log also diverges in shape from the execution log AWS returns for an AWS_PROXY invocation (it never records the endpoint request/response transformation lines).
Why: a client or test harness that reads the trace id out of a test invocation to correlate it with downstream logs finds nothing, and header-propagation assertions on proxy integrations fail against the emulator while passing on AWS.
Fix: TestInvokeMethod's response assembly - attach the tracing header to both header maps and have the execution log cover the integration request/response phases. (The $.log shape divergence of other TestInvokeMethod clusters is owned by the open issue '20261029T200808Z-apigateway-api-gateway-authorizer-representation-and-testinvokemethod-execution-9.md'; this cluster was reported unowned because of the missing trace-id members, so it is filed here.)
Traces: apigateway-test-invoke-lambda-proxy-echo#task#11.

Cause 4 of 4 - GetUsage with startDate after endDate.
AWS: 400 BadRequestException with message 'Usage end date must be after start date' - wholly fixed text, no request value interpolated. Smithy: GetUsage GET /usageplans/{usagePlanId}/usage with startDate and endDate as @required httpQuery members, errors BadRequestException @httpError 400, NotFoundException 404. Status and text taken from the recorded response.
Emulator: 400 (status correct) but message 'Invalid usage interval specified. The start date must not follow the end date', which is emulator-authored wording.
Why: clients and test suites that match on AWS's message text to tell an inverted interval apart from other 400s see an unrecognised string and treat the refusal as an unknown error.
Fix: GetUsage's date-range validation message.
Traces: apigateway-usage-plan-key-association-and-usage-tracking#task#9.

## Evidence
Cause 1 (GetClientCertificate, apigateway-client-certificate-lifecycle#task#2): $.tags missing - expected {}, got None. Cluster also covers #task#5.
Cause 2 (UpdateClientCertificate, apigateway-client-certificate-lifecycle#task#4): $.tags missing - expected {}, got None. Cluster also covers apigw-patch-clientcert-invalid-path#task#4.
Cause 3 (TestInvokeMethod, apigateway-test-invoke-lambda-proxy-echo#task#11): $.headers.X-Amzn-Trace-Id missing (expected present/shaped, got None); $.multiValueHeaders.X-Amzn-Trace-Id missing (expected ['<shaped>'], got None); $.log value does not match the expected execution-log shape.
Cause 4 (GetUsage, apigateway-usage-plan-key-association-and-usage-tracking#task#9): status 400 on both sides; $.message expected 'Usage end date must be after start date' (entirely fixed text), got 'Invalid usage interval specified. The start date must not follow the end date'.

## Steps to Reproduce
1. Replay the corpus: replay_wire_corpus().
2. Causes 1-2: POST /clientcertificates to generate a certificate with no tags, then GET /clientcertificates/{id} (fixture apigateway-client-certificate-lifecycle, step task#2) and PATCH /clientcertificates/{id} with a description patch (step task#4). Expect 'tags': {} in both bodies; observe the member absent.
3. Cause 3: create a REST API with an AWS_PROXY method and POST /restapis/{id}/resources/{resourceId}/methods/POST (fixture apigateway-test-invoke-lambda-proxy-echo, step task#11). Expect X-Amzn-Trace-Id in headers and multiValueHeaders; observe neither.
4. Cause 4: GET /usageplans/{usagePlanId}/usage?startDate=<later>&endDate=<earlier> (fixture apigateway-usage-plan-key-association-and-usage-tracking, step task#9). Expect 400 'Usage end date must be after start date'; observe the emulator's own wording.
