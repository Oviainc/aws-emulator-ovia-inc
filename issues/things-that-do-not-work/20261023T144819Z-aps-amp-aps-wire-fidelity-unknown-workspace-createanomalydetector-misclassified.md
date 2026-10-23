Issue-Source: trace-sweep-divergence
Issue-Trace: amp-anomaly-detector-workflow
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amp-anomaly-detector-workflow amp-workspace-lifecycle-and-configuration amp-scraper-error-paths-and-defaults
Issue-Shape-Id: com.amazonaws.amp#AmazonPrometheusService
Issue-Key: aps|CreateAnomalyDetector|4xx->4xx|$.message
Issue-Key: aps|CreateAnomalyDetector|4xx->4xx|$.resourceId
Issue-Key: aps|CreateAnomalyDetector|4xx->4xx|$.resourceType
Issue-Key: aps|CreateAnomalyDetector|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: aps|CreateAnomalyDetector|4xx->4xx|status
Issue-Key: aps|CreateWorkspace|4xx->4xx|$.fieldList[*].message
Issue-Key: aps|CreateWorkspace|4xx->4xx|$.message
Issue-Key: aps|GetDefaultScraperConfiguration|2xx->2xx|$.configuration
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# AMP (aps) wire fidelity: unknown-workspace CreateAnomalyDetector misclassified, kmsKeyArn pattern over-strict, default scraper document wrong

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAnomalyDetector

## Description
Three independent response-fidelity root causes found in the aps (Amazon Managed Service for Prometheus / amp) emulator by the wire corpus. The service IS stood up and serving — 22 of 41 replayed steps pass and these handlers are implemented, not scaffold stubs — so these are fidelity defects, not a bootstrap gap. THREE causes must be closed before submitting.

Cause 1 of 3: CreateAnomalyDetector against a workspaceId that does not exist is refused as 400 ValidationException instead of 404 ResourceNotFoundException.
  AWS: recorded behavior only (the Smithy model does NOT bind ResourceNotFoundException to CreateAnomalyDetector — its modeled errors are AccessDenied 403 / Conflict 409 / InternalServer 500 / ServiceQuotaExceeded 402 / Throttling 429 / Validation 400 — but the recording shows the live service answering HTTP 404 with header x-amzn-errortype: ResourceNotFoundException and a body carrying the resource identity members resourceId (the workspace id that did not resolve) and resourceType 'AWS::APS::Workspace'; recorded example message 'Workspace not found.'). The recorded status, error code and the presence of resourceId/resourceType are the requirement; the message text is an example. Hypothesis: the live service resolves the {workspaceId} path label to a workspace before request validation runs, so an unresolvable parent is a not-found, not a constraint failure.
  Emulator: answers HTTP 400 with x-amzn-errortype: ValidationException and a member-validation body ('Invalid workspaceId: Member must name a workspace this account holds'); resourceId and resourceType are absent entirely. The handler does this deliberately — services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/create_anomaly_detector.rs reasons that because the model binds no ResourceNotFoundException the refusal must be the modeled ValidationException — but the recording shows AWS does not behave that way.
  Why: clients branch on HTTP status class and error code, never on message. A caller whose detector-creation path handles a missing/deleted workspace (create-if-missing, retry-after-workspace-ACTIVE, `except ResourceNotFoundException`) sees a 400 constraint failure instead, so it treats a transient/absent-parent condition as a malformed request and does not recover; anything reading resourceId/resourceType to report WHICH workspace was missing gets nothing.
  Fix: the CreateAnomalyDetector handler's parent-resolution branch (its unknown-workspaceId path) and the error shape it serializes — it must emit the 404 ResourceNotFoundException form with resourceId and resourceType populated rather than the member-validation ValidationException.
  Traces: amp-anomaly-detector-workflow#task#1

Cause 2 of 3: CreateWorkspace enforces a stricter kmsKeyArn pattern than the service does, so it over-rejects KMS key ARNs AWS accepts.
  AWS: recorded behavior only for the effective constraint. The on-disk model constrains CreateWorkspace$kmsKeyArn (KmsKeyArn, len 20..2048) with @pattern '^arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/[-a-f0-9]+$', but the recorded live refusal (400 ValidationException, fieldList entry plus message) quotes the requirement it actually applies: 'Member must satisfy regular expression pattern: arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+' — i.e. the key-id segment is unconstrained ('.+'), not restricted to lowercase hex and dashes. This is a validation message built from the request (it names the member and the constraint it failed), so the CONSTRAINT it reports is the requirement here, not the literal recorded text.
  Emulator: refuses with the model's narrower pattern and reports it verbatim in both $.message and $.fieldList[0].message ('... :key/[-a-f0-9]+'). The status (400) and error code (ValidationException) already match; the diverging part is which key ARNs pass. Under [-a-f0-9]+ any key-id outside lowercase hex is rejected — notably multi-region keys (key/mrk-<hex>) and any non-hex key-id form — all of which the live service admits.
  Why: a client that encrypts a workspace with a multi-region or otherwise non-plain-hex CMK cannot create the workspace at all against the emulator, and gets a ValidationException naming a pattern no AWS response ever states; the emulator is strictly less permissive than the service, so a configuration that works in production fails under test.
  Fix: CreateWorkspace's request validation for kmsKeyArn — the KmsKeyArn @pattern as it is enforced (the model's pattern for that shape and/or the validation layer that renders the member-validation requirement string). Loosen the key-id segment to the service's effective 'key/.+' so the enforced constraint and the reported requirement agree with the live service.
  Traces: amp-workspace-lifecycle-and-configuration#task#1

Cause 3 of 3: GetDefaultScraperConfiguration returns a default scraper document that is not the one AWS serves.
  AWS: Smithy: GetDefaultScraperConfiguration (GET /scraperconfiguration, @readonly, 200) returns required output member configuration (Blob) documented as 'The configuration file. Base 64 encoded.' It is a fixed service-wide document with no inputs and nothing account- or environment-specific in it, so its content is deterministic and is a requirement. The recorded 200 body decodes to a document whose global block has external_labels COMMENTED OUT with a placeholder ('  # external_labels:' / '    # clusterArn: <REPLACE_ME>'), whose cadvisor job carries authorization.credentials_file and whose kubernetes-apiservers job carries bearer_token_file (both the in-cluster service-account token path), and whose kubernetes-apiservers job declares only 'scheme: https' with no tls_config.
  Emulator: returns a different document (DEFAULT_SCRAPER_CONFIGURATION in services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/get_default_scraper_configuration.rs): the global external_labels block is ACTIVE rather than commented out and carries clusterArn: apiserver-url-here instead of the commented <REPLACE_ME> placeholder, the two token-path members are absent, and the kubernetes-apiservers job gains an extra 'tls_config: insecure_skip_verify: false' that the recording does not contain. Status 200 and the response shape are correct; only the blob's content diverges. Note: that file documents the two token-path omissions as deliberate (the repo's exfil scan refuses a host secret path in source), so the squarely actionable divergences are the commented-vs-active external_labels/clusterArn block and the extra tls_config stanza; if the token paths must stay out, say so in the fix rather than leaving the document silently different.
  Why: the whole point of this operation is that a caller feeds the blob straight to a Prometheus/EKS scraper. An ACTIVE external_labels.clusterArn with the literal value 'apiserver-url-here' is a live setting that stamps a bogus label onto every scraped series, where AWS ships it commented out for the user to fill in; the extra insecure_skip_verify changes the apiserver job's TLS behavior. Anyone diffing or templating the default config against AWS's published document gets a different file.
  Fix: the GetDefaultScraperConfiguration handler's embedded default document constant — bring it back to the document the recording carries (external_labels block commented out with the <REPLACE_ME> placeholder, no added tls_config on the kubernetes-apiservers job).
  Traces: amp-scraper-error-paths-and-defaults#task#1

Relation to already-open work: the open issue 20261022T110707Z-amp-3-fidelity-divergences-prometheusendpoint-announces-the-real.md covers three DIFFERENT amp causes (prometheusEndpoint host, TagResource not-found error code, and standing up server-side RCF query validation — which is now implemented and whose status/code match). None of the three causes above is described there, and the replay reported no owning issue for any of these steps.

## Evidence
Cause 1 — fixture amp-anomaly-detector-workflow, step task#1, CreateAnomalyDetector:
  status: expected 404, got 400
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException', got 'ValidationException'
  $.resourceId: missing (expected the unresolved workspace id, e.g. 'ws-00000000000000000000000000', got None)
  $.resourceType: missing (expected 'AWS::APS::Workspace', got None)
  $.message: expected (example) 'Workspace not found.', got 'Invalid workspaceId: Member must name a workspace this account holds'

Cause 2 — fixture amp-workspace-lifecycle-and-configuration, step task#1, CreateWorkspace (both sides 400 ValidationException):
  $.message: expected 'Invalid kmsKeyArn: Member must satisfy regular expression pattern: arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+', got '... :key/[-a-f0-9]+'
  $.fieldList[0].message: expected 'Member must satisfy regular expression pattern: arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+', got '... :key/[-a-f0-9]+'
  Consequence: key-ids outside [-a-f0-9]+ (e.g. key/mrk-<hex> multi-region keys) are rejected by the emulator and accepted by AWS.

Cause 3 — fixture amp-scraper-error-paths-and-defaults, step task#1, GetDefaultScraperConfiguration (both sides 200):
  $.configuration: value_mismatch. Expected blob decodes to a document beginning
    global:\n  scrape_interval: 30s\n  # external_labels:\n    # clusterArn: <REPLACE_ME>\nscrape_configs:
  with authorization.credentials_file on the cadvisor job, bearer_token_file on the kubernetes-apiservers job, and no tls_config on that job.
  Actual blob (DEFAULT_SCRAPER_CONFIGURATION) begins
    global:\n  scrape_interval: 30s\n  external_labels:\n    clusterArn: apiserver-url-here\nscrape_configs:
  omits both token-path members and adds 'tls_config:\n      insecure_skip_verify: false' to the kubernetes-apiservers job.

Not filed (reported for completeness, message-text only, status and error code both match): amp-anomaly-detector-workflow#task#6 (unwrapped RCF query: 400 ValidationException both sides, only the prose differs) and amp-workspace-lifecycle-and-configuration#task#2 (DescribeWorkspace 404 both sides, 'Workspace not found' vs 'Workspace not found.').

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the emulator (replay_wire_corpus), or by hand against the aps endpoint:

Cause 1 (fixture amp-anomaly-detector-workflow, step task#1): POST /workspaces/ws-00000000000000000000000000/anomalydetectors with a well-formed body (alias, configuration={"randomCutForest":{"query":"sum(up)"}}, evaluationIntervalInSeconds=60) for a workspaceId that was never created. Observe 400 / x-amzn-errortype ValidationException with no resourceId or resourceType, instead of 404 / ResourceNotFoundException carrying resourceId and resourceType 'AWS::APS::Workspace'.

Cause 2 (fixture amp-workspace-lifecycle-and-configuration, step task#1): POST /workspaces with kmsKeyArn set to a syntactically malformed ARN. Observe the reported requirement quotes pattern '...:key/[-a-f0-9]+' rather than the service's '...:key/.+'; then POST /workspaces with a valid multi-region key ARN 'arn:aws:kms:us-west-2:123456789012:key/mrk-0123456789abcdef0123456789abcdef' and observe the emulator rejects it with 400 ValidationException where AWS accepts it.

Cause 3 (fixture amp-scraper-error-paths-and-defaults, step task#1): GET /scraperconfiguration, base64-decode $.configuration, and diff against the recorded document — the global external_labels/clusterArn block is active with value 'apiserver-url-here' instead of commented out with '<REPLACE_ME>', the service-account token paths are absent, and the kubernetes-apiservers job has an extra tls_config.insecure_skip_verify.
