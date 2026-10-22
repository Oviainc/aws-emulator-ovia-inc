Issue-Source: trace-sweep-divergence
Issue-Trace: uxc-catalog-list-services
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: uxc-catalog-list-services uxc-baseline-read-and-list-services uxc-configure-visible-services-roundtrip uxc-error-list-services-invalid-token-pattern uxc-list-services-pagination
Issue-Shape-Id: com.amazonaws.uxc#AWSAccountUXSetting
Issue-Key: uxc|ListServices|2xx->2xx|$.services[*]
Issue-Key: uxc|ListServices|4xx->4xx|$.message
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# ListServices returns wrong service catalog and validation message

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
SIX causes, all on uxc ListServices. Causes 1-5 are the SAME root cause (the emulator's seeded service catalog) observed through five different fixtures and page sizes; Cause 6 is an independent defect in how a pattern-constraint refusal is rendered. All six must be closed before submitting, and each names its own fixture and step so each can be re-replayed.

Cause 1 of 6: catalog contents/ordering, full listing (fixture uxc-catalog-list-services, step task#1).
AWS: recorded behavior only -- the Smithy model constrains ListServices' output shape (ServiceList, @length max 500; member Service @length 1..64, @pattern ^[a-z0-9]+(-[a-z0-9]+)*$; paginated inputToken=nextToken/outputToken=nextToken/items=services) but says nothing about which identifiers the catalog holds. The recording shows HTTP 200 with a console service catalog of 224+ entries in ascending order whose first elements are 'account-access', 'acm', 'acm-pca', 'activate', 'aidevops', 'airflow', and which includes console-only identifiers ('aws-marketplace', 'awsglobalview', 'claude-platform-on-aws', 'bedrock-agentcore', ...) alongside ordinary service names.
Emulator: HTTP 200 with a much smaller fixed catalog of 45 endpoint-prefix-style identifiers ('access-analyzer', 'account', 'acm', 'amplify', 'apigateway', ...). Positionally the two lists disagree from index 0 onward -- the replay reports $.services[0] expected 'account-access' got 'access-analyzer', $.services[2] expected 'acm-pca' got 'acm', and so on through $.services[223].
Why: ListServices is documented as the source of valid values for UpdateAccountCustomizations' visibleServices. A client that enumerates the catalog and then makes a service visible cannot reach any identifier AWS offers but the emulator omits, and a client that asserts on catalog membership or on a specific page's contents sees an entirely different set. Pagination over the catalog is also mis-sized: the emulator's catalog is short enough that pages and nextToken issuance differ from AWS's for the same maxResults.
Fix: the service's state seed -- the catalog the ListServices handler pages over, not the handler's paging logic (which correctly sorts, dedups, slices and issues an offset token).

Cause 2 of 6: same root cause via the baseline read-and-list fixture (fixture uxc-baseline-read-and-list-services, step task#3). AWS: HTTP 200, catalog beginning 'account-access', with 'acm-pca', 'activate', 'aidevops' at indices 2-4. Emulator: HTTP 200 with 'access-analyzer', 'acm', 'amplify', 'apigateway' at those positions. Why/Fix: as Cause 1.

Cause 3 of 6: same root cause on the first page of a roundtrip fixture (fixture uxc-configure-visible-services-roundtrip, step setup#1). AWS: $.services[0] = 'account-access'. Emulator: 'access-analyzer'. Why/Fix: as Cause 1.

Cause 4 of 6: same root cause after a rejected token (fixture uxc-error-list-services-invalid-token-pattern, step task#2). AWS: $.services[0] = 'account-access', $.services[2] = 'acm-pca'. Emulator: 'access-analyzer', 'acm'. Why/Fix: as Cause 1.

Cause 5 of 6: same root cause across a paginated listing, affecting two consecutive steps of one fixture (fixture uxc-list-services-pagination, step task#1; the cluster also covers uxc-list-services-pagination#task#2). AWS: $.services[0] = 'account-access', $.services[1] = 'acm'. Emulator: 'access-analyzer', 'account'. Why: the page boundaries and therefore the nextToken-resumed second page describe different elements than AWS's. Fix: as Cause 1.

Cause 6 of 6: pattern-constraint refusal quotes the anchored regex (fixture uxc-error-list-services-invalid-token-pattern, step task#1).
AWS: a nextToken violating its @pattern is refused with HTTP 400 and the standard AWS constraint-violation message, which is built from the request and the member's constraint: "1 validation error detected: Value '<value>' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". The recording shows AWS rendering that pattern WITHOUT the anchor characters -- 'Member must satisfy regular expression pattern: [-A-Za-z0-9+/_]+=*'. Status 400 and the error code agree with the emulator; only the rendering of the constraint inside this generated message diverges. Smithy: ListServices.nextToken @length(32,512) @pattern ^[-A-Za-z0-9+/_]+=*$; error ValidationException (httpError 400).
Emulator: HTTP 400 with the same generated message but the anchored form, 'Member must satisfy regular expression pattern: ^[-A-Za-z0-9+/_]+=*$'.
Why: because this message is generated from the request and the constraint rather than being free prose, clients and test harnesses parse and assert on it; the emulator's rendering never matches AWS's for any anchored pattern in this service, so every pattern refusal reads as a different failure.
Fix: the shared boundary validation layer's pattern-violation message formatter (the component that renders a @pattern constraint into the "1 validation error detected" text), not the ListServices handler.

Traces: see Traces section below; Causes 1-5 are the catalog, Cause 6 the message rendering.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 36 steps, 26 passed, 10 mismatched).

Cause 1 -- uxc-catalog-list-services#task#1, ListServices, expected 200 / actual 200: "$.services[0]: missing (expected 'account-access', got 'access-analyzer')", "$.services[2]: missing (expected 'acm-pca', got 'acm')", "$.services[3]: missing (expected 'activate', got 'amplify')", "$.services[4]: missing (expected 'aidevops', got 'apigateway')", ... continuing through $.services[223] (expected entries include 'appfabric', 'appflow', 'applicationrecoverycontroller', 'appmesh', 'apprunner', 'appstream', 'appstudio', 'appsync', 'artifact', 'auditmanager', 'aws-agent-registry', 'aws-marketplace', 'awsforsap', 'awsglobalview', 'b2bi', 'bedrock-agentcore', 'bedrock-mantle', 'billingconductor', 'biodiscovery', 'braket', 'ce', 'chatbot', 'chime', 'chimesdk', 'claude-platform-on-aws', 'cleanrooms', ...).

Cause 2 -- uxc-baseline-read-and-list-services#task#3: "$.services[0]: missing (expected 'account-access', got 'access-analyzer')", "$.services[2]: missing (expected 'acm-pca', got 'acm')", "$.services[3]: missing (expected 'activate', got 'amplify')", "$.services[4]: missing (expected 'aidevops', got 'apigateway')".

Cause 3 -- uxc-configure-visible-services-roundtrip#setup#1: "$.services[0]: missing (expected 'account-access', got 'access-analyzer')".

Cause 4 -- uxc-error-list-services-invalid-token-pattern#task#2: "$.services[0]: missing (expected 'account-access', got 'access-analyzer')", "$.services[2]: missing (expected 'acm-pca', got 'acm')".

Cause 5 -- uxc-list-services-pagination#task#1 (cluster also covers #task#2): "$.services[0]: missing (expected 'account-access', got 'access-analyzer')", "$.services[1]: missing (expected 'acm', got 'account')".

Cause 6 -- uxc-error-list-services-invalid-token-pattern#task#1, expected 400 / actual 400, error code matches, message diverges: expected "1 validation error detected: Value '!!!!...' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [-A-Za-z0-9+/_]+=*" vs actual "... Member must satisfy regular expression pattern: ^[-A-Za-z0-9+/_]+=*$".

Emulator source backing Causes 1-5: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs lines 37-83 define SEED_SERVICE_CATALOG as 45 entries, and lines 96-108 sort/dedup it into ComAmazonawsUxcAwsaccountuxsettingState::service_catalog; services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/list_services.rs lines 240-273 page over exactly that vector.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture uxc-catalog-list-services, step task#1 -- GET /v1/services (no nextToken) and compare $.services positionally with the recorded 224+ entry catalog.
3. Causes 2-5: replay uxc-baseline-read-and-list-services#task#3, uxc-configure-visible-services-roundtrip#setup#1, uxc-error-list-services-invalid-token-pattern#task#2, and uxc-list-services-pagination#task#1 (plus task#2, same cluster) -- each GET /v1/services page disagrees with the recording from index 0.
4. Cause 6: replay uxc-error-list-services-invalid-token-pattern#task#1 -- GET /v1/services?nextToken=<40 '!' characters> and read $.message; the emulator renders the nextToken pattern with leading ^ and trailing $, the recording renders it without them.
