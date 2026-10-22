Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amp.remote-write-query-round-trip amp.tag-resource-nonexistent-arn-rejected amp.anomaly-detector-evaluation-interval-range-rejected
Issue-Shape-Id: com.amazonaws.amp#AmazonPrometheusService
Issue-Key: amp.remote-write-query-round-trip|_signed_request#12|EgressRefused|with urllib.request.urlopen(urlreq, timeout=30) as resp:
Issue-Key: amp.tag-resource-nonexistent-arn-rejected|run#9|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code}"
Issue-Key: amp.anomaly-detector-evaluation-interval-range-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException for unwrapped RCF query, got {code}"
Issue-Commit: 92ad2730926ec3062b662e938e8ed9f3fac68577

# AMP: 3 fidelity divergences (prometheusEndpoint announces the real AWS host, TagResource 404 uses the wrong error code, no server-side RCF query validation)

> _Note: this issue was identified against the code base at commit `92ad2730926ec3062b662e938e8ed9f3fac68577`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeWorkspace (also TagResource, CreateAnomalyDetector)

## Description
Three independent root causes found in the amp (Amazon Prometheus Service) emulator by the behavioral suite. The emulator does serve amp (its bootstrap issue is resolved and other amp checks pass), so these are fidelity defects, not bootstrap gaps. THREE causes must be closed before submitting.

Cause 1 of 3: DescribeWorkspace returns a prometheusEndpoint pointing at the real AWS production host instead of the endpoint the emulator is reached on.
  AWS: Smithy: WorkspaceDescription.prometheusEndpoint is documented as "The Prometheus endpoint available for this workspace. For example, https://aps-workspaces.<region>.amazonaws.com/workspaces/ws-.../api/v1/" — i.e. the URL at which THIS workspace's Prometheus data plane (remote_write, instant query) is actually served, and in real AWS it is reachable and answers SigV4-signed remote_write/query traffic.
  Emulator: the returned prometheusEndpoint names host 'aps-workspaces.<region>.amazonaws.com' — an address other than the endpoint the emulator is served on (127.0.0.1). The replay's egress guard refused the follow-up data-plane call: "egress refused: 'aps-workspaces.us-west-2.amazonaws.com' is not the endpoint under test (127.0.0.1); ... If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on". So the host came out of the emulator's own response.
  Why: any client that follows the advertised endpoint (every Prometheus remote_write agent, Grafana datasource, or SDK caller that writes/queries series) leaves the emulator entirely and tries to talk to real AWS; the workspace's data plane is unusable even if implemented. The round-trip check could not exercise remote_write/query at all because of this.
  Fix: the DescribeWorkspace/CreateWorkspace/ListWorkspaces response construction for the workspace description (the handler/serializer that builds prometheusEndpoint) must derive the URL from the request's own host/endpoint under which the emulator is reached, keeping only the '/workspaces/<workspaceId>/api/v1/' path shape. If the Prometheus data plane (POST /api/v1/remote_write, GET /api/v1/query) is not routed at all, it must also be stood up for the advertised endpoint to be meaningful.
  Traces: amp.remote-write-query-round-trip

Cause 2 of 3: TagResource on a nonexistent workspace ARN is refused with the wrong error code.
  AWS: recorded behavior only (the on-disk Smithy model lists ResourceNotFoundException @httpError 404 on TagResource, but the recorded AWS response for a nonexistent AMP resource ARN uses error code NotFoundException, 404; recorded example message "Resource was not found"). The recorded code is the requirement.
  Emulator: refuses with error code ResourceNotFoundException instead of NotFoundException (status 404).
  Why: clients branch on the error code, not the message — botocore/SDK exception classes, retry/"create-if-missing" logic and `except NotFoundException` handlers never match, so a tag attempt on a deleted/absent workspace surfaces as an unexpected error instead of the modelled not-found path.
  Fix: the TagResource (and the matching UntagResource/ListTagsForResource) handler's not-found rejection in the amp service, plus the error shape it serializes.
  Traces: amp.tag-resource-nonexistent-arn-rejected

Cause 3 of 3: CreateAnomalyDetector performs no server-side validation of the RandomCutForest PromQL query (it answers NotImplemented).
  AWS: Smithy: CreateAnomalyDetector takes a required `configuration` (AnomalyDetectorConfiguration, e.g. randomCutForest.query) and models ValidationException @httpError 400. Recorded behavior: a syntactically valid but unwrapped RCF query (query "up", whose root is not a supported aggregation operator) is rejected with ValidationException and NO detector is created, while the same alias with a wrapped query ("sum(up)") is accepted and reaches ACTIVE. Recorded example message: "Unsupported PromQL expression for RandomCutForest, expression's root must be a supported aggregation operator". This rule is server-side only — it passes every client-side type/length/pattern check.
  Emulator: the call returns error code NotImplemented ("operation CreateAnomalyDetector is scaffolded but not implemented in this emulator"), so neither the rejection nor the positive control happens.
  Why: a client testing its handling of a bad anomaly-detector query sees NotImplemented instead of ValidationException; once the operation is merely implemented without this semantic rule, the emulator becomes overly permissive and accepts detector configurations AWS refuses, hiding the bug until production.
  Fix: CreateAnomalyDetector's handler and its request validation (services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/create_anomaly_detector.rs) must implement the operation AND reject a randomCutForest.query whose PromQL root is not a supported aggregation operator with ValidationException (400), creating no detector (ListAnomalyDetectors for that alias must stay empty). Note: a separate already-filed issue covers standing the scaffold stub up; this cause is the additional server-side query rule, which that issue does not describe.
  Traces: amp.anomaly-detector-evaluation-interval-range-rejected

## Evidence
amp.remote-write-query-round-trip [error]:
  simparity.egress.EgressRefused: egress refused: 'aps-workspaces.us-west-2.amazonaws.com' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on
  (host taken from describe_workspace(...)['workspace']['prometheusEndpoint'])
  Expected: prometheusEndpoint resolves to the emulator's own endpoint and serves POST /api/v1/remote_write (200/202/204) and GET /api/v1/query returning the written sample.
  Actual: endpoint points at the real AWS aps-workspaces host; data plane never reached.

amp.tag-resource-nonexistent-arn-rejected [mismatch]:
  AssertionError: expected NotFoundException, got ResourceNotFoundException
  Expected: TagResource(resourceArn=<nonexistent ws ARN>) -> 404, Code=NotFoundException (recorded example message "Resource was not found").
  Actual: 404-class refusal with Code=ResourceNotFoundException.

amp.anomaly-detector-evaluation-interval-range-rejected [mismatch]:
  AssertionError: expected ValidationException for unwrapped RCF query, got NotImplemented
  Expected: CreateAnomalyDetector(configuration={"randomCutForest": {"query": "up"}}) -> 400 ValidationException, no detector created; same alias with {"query": "sum(up)"} -> accepted, status reaches ACTIVE.
  Actual: error code NotImplemented ("operation CreateAnomalyDetector is scaffolded but not implemented in this emulator").

## Steps to Reproduce
Cause 1: amp.create_workspace(alias=...); poll describe_workspace until status ACTIVE; read workspace['prometheusEndpoint'] — observe the host is aps-workspaces.<region>.amazonaws.com rather than the emulator's own endpoint; any SigV4-signed POST <endpoint>/api/v1/remote_write therefore leaves the emulator.

Cause 2: create a workspace, read its ARN via describe_workspace, mutate the last 8 ARN characters to 'ffffffff', then amp.tag_resource(resourceArn=<bogus arn>, tags={'k':'v'}) — observe Error.Code ResourceNotFoundException instead of NotFoundException.

Cause 3: create a workspace and wait for ACTIVE, then amp.create_anomaly_detector(workspaceId=..., alias=..., configuration={'randomCutForest': {'query': 'up'}}, evaluationIntervalInSeconds=60) — observe NotImplemented instead of a 400 ValidationException; then repeat with query 'sum(up)' and observe it is not accepted either.
