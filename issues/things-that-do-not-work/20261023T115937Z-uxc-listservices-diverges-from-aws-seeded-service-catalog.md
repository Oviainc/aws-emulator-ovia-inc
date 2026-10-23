Issue-Source: trace-sweep-divergence
Issue-Trace: uxc-catalog-list-services
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: uxc-catalog-list-services uxc-baseline-read-and-list-services uxc-configure-visible-services-roundtrip uxc-error-list-services-invalid-token-pattern uxc-list-services-pagination
Issue-Shape-Id: com.amazonaws.uxc#AWSAccountUXSetting
Issue-Key: uxc|ListServices|2xx->2xx|$.services[*]
Issue-Key: uxc|ListServices|4xx->4xx|$.message
Issue-Commit: 982a07ada62dfbeb289a85399a86f8ade68b0ada

# uxc ListServices diverges from AWS: seeded service catalog is a 45-entry endpoint-prefix stub and the nextToken refusal echoes the anchored pattern

> _Note: this issue was identified against the code base at commit `982a07ada62dfbeb289a85399a86f8ade68b0ada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
SIX causes, all on uxc ListServices (GET /v1/services, Smithy: ListServices -> ListServicesOutput.services : ServiceList @length(max 500), member Service @length(1,64) @pattern ^[a-z0-9]+(-[a-z0-9]+)*$; paginated inputToken/outputToken nextToken, items services). Causes 1-5 are the same underlying defect observed on five different fixtures and must each be re-replayed; cause 6 is the operation's ValidationException message form. All six must be closed before submitting.

Shared root cause for causes 1-5: the catalog ListServices pages over is a hand-written seed list of 45 AWS endpoint prefixes (access-analyzer, account, acm, amplify, ... xray) held in this service's state, not the service-identifier catalog the recorded service enumerates. Every recorded page shows a far larger, differently-populated catalog, so the emulator's entries are wrong from index 0 onwards and every positional comparison fails.

Cause 1 of 6: fixture uxc-catalog-list-services, step task#1 (representative).
AWS: ListServices "returns a paginated list of Amazon Web Services service identifiers that you can use as values for the visibleServices setting" (model doc). Recorded behavior only for the catalog's membership: the recording's 200 body carries a sorted catalog of 224+ identifiers that includes entries the emulator has none of - account-access, acm-pca, activate, aidevops, airflow, appfabric, appflow, applicationrecoverycontroller, appmesh, apprunner, appstream, appstudio, appsync, artifact, auditmanager, aws-agent-registry, aws-marketplace, awsforsap, awsglobalview, b2bi, bedrock-agentcore, bedrock-mantle, billingconductor, biodiscovery, braket, ce, chatbot, chime, chimesdk, claude-platform-on-aws, cleanrooms, ... Hypothesis: these are console service identifiers rather than SigV4 endpoint prefixes, which is why the emulator's prefix-derived seed cannot reproduce them.
Emulator: answers 200 with its 45-entry seed, so the compared elements diverge at ~180 indices in one page ($.services[0] expected 'account-access' got 'access-analyzer'; $.services[2] expected 'acm-pca' got 'acm'; $.services[3] expected 'activate' got 'amplify'; $.services[4] expected 'aidevops' got 'apigateway'; and so on through $.services[223]).
Why: a client calling ListServices to discover which identifiers it may put in visibleServices gets a catalog missing ~80% of the real entries, so legitimate identifiers look nonexistent and any UI or validation built on the listing is wrong. Pagination is also affected: with 45 items no second page is ever offered where AWS offers several.
Fix: the seeded catalog in this service's state (the SEED_SERVICE_CATALOG constant and the service_catalog it builds), not the ListServices handler's paging logic, which pages the catalog it is given correctly.
Traces: uxc-catalog-list-services#task#1.

Cause 2 of 6: fixture uxc-baseline-read-and-list-services, step task#3.
AWS: same operation and same catalog contract as cause 1; the recorded 200 body's first page begins account-access, account, acm-pca, activate, aidevops, ...
Emulator: 200 with the seed catalog - $.services[0] expected 'account-access' got 'access-analyzer', $.services[2] expected 'acm-pca' got 'acm', $.services[3] expected 'activate' got 'amplify', $.services[4] expected 'aidevops' got 'apigateway'.
Why: as cause 1 - the baseline read-then-list flow reports a catalog a client cannot match against AWS.
Fix: same component as cause 1 (the seeded catalog in this service's state).
Traces: uxc-baseline-read-and-list-services#task#3.

Cause 3 of 6: fixture uxc-configure-visible-services-roundtrip, step setup#1.
AWS: same catalog contract; the recorded setup listing's first element is 'account-access'.
Emulator: 200, $.services[0] expected 'account-access' got 'access-analyzer'.
Why: the roundtrip that configures visibleServices from a discovered identifier starts from a catalog whose head entry does not exist in AWS.
Fix: same component as cause 1.
Traces: uxc-configure-visible-services-roundtrip#setup#1.

Cause 4 of 6: fixture uxc-error-list-services-invalid-token-pattern, step task#2.
AWS: the recovery listing after a refused token returns the same AWS catalog (first elements account-access, account, acm-pca, ...).
Emulator: 200, $.services[0] expected 'account-access' got 'access-analyzer', $.services[2] expected 'acm-pca' got 'acm'.
Why: as cause 1.
Fix: same component as cause 1.
Traces: uxc-error-list-services-invalid-token-pattern#task#2.

Cause 5 of 6: fixture uxc-list-services-pagination, steps task#1 and task#2.
AWS: the recorded sized page's elements are 'account-access' at index 0 and 'acm' at index 1.
Emulator: 200, $.services[0] expected 'account-access' got 'access-analyzer', $.services[1] expected 'acm' got 'account' - i.e. the page boundaries land on the wrong elements because the underlying ordered catalog is not AWS's.
Why: a paginating client walks a different sequence than AWS serves and, with only 45 items, stops after one page.
Fix: same component as cause 1; once the catalog is the recorded one the existing offset-bearing token paging is what produces the recorded pages.
Traces: uxc-list-services-pagination#task#1 (cluster also covers uxc-list-services-pagination#task#2).

Cause 6 of 6: fixture uxc-error-list-services-invalid-token-pattern, step task#1 - nextToken pattern refusal echoes the model's anchored regex.
AWS: Smithy: ListServices.nextToken targets NextToken @length(32,512) @pattern ^[-A-Za-z0-9+/_]+=*$, refused with the modeled ValidationException (HTTP 400). The refusal message is built from the request and the violated constraint - the standard template "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" - and the recorded response renders <pattern> WITHOUT the regex anchors: [-A-Za-z0-9+/_]+=*. Status 400 and the error class both match; only the rendering of the pattern in that known template diverges.
Emulator: answers 400 ValidationException with the same template but renders the anchored model pattern verbatim: "... Member must satisfy regular expression pattern: ^[-A-Za-z0-9+/_]+=*$".
Why: a client (or a test) that reads the advertised pattern out of the validation message to build or repair a token gets a regex with anchors AWS never emits; the message is the only thing distinguishing constraint violations here, so the rendering is part of the contract.
Fix: the validation-message rendering for pattern constraints on this operation's input (strip the leading ^ and trailing $ when echoing a modeled @pattern), in the shared validation/message-formatting path rather than in the catalog state.
Traces: uxc-error-list-services-invalid-token-pattern#task#1.

Not covered by any open issue: the only open uxc issue, 20261022T220228Z-uxc-updateaccountcustomizations-reports-list-settings-in-the-wrong-member.md, is about UpdateAccountCustomizations list-member order and owns the two UpdateAccountCustomizations clusters in this run; it says nothing about the ListServices catalog or the pattern-message rendering. The resolved issue 20261022T041007Z-uxc-updateaccountcustomizations-wrong-error-code-for-body-validation-refusals.md touched catalog membership only as an admission check on UpdateAccountCustomizations input, not the catalog's contents.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up true, bootstrap_signal false, state_reset_failures 0; 36 steps, 27 passed, 9 mismatched):

Cause 1 - uxc-catalog-list-services#task#1, ListServices, expected 200 / actual 200: $.services[0] missing (expected 'account-access', got 'access-analyzer'); $.services[2] missing (expected 'acm-pca', got 'acm'); $.services[3] missing (expected 'activate', got 'amplify'); $.services[4] missing (expected 'aidevops', got 'apigateway'); $.services[5] expected 'airflow' got 'appconfig'; $.services[8] expected 'appfabric' got 'backup'; ... ~180 mismatching indices up to $.services[223].
Cause 2 - uxc-baseline-read-and-list-services#task#3, 200/200: $.services[0] expected 'account-access' got 'access-analyzer'; [2] 'acm-pca' vs 'acm'; [3] 'activate' vs 'amplify'; [4] 'aidevops' vs 'apigateway'.
Cause 3 - uxc-configure-visible-services-roundtrip#setup#1, 200/200: $.services[0] expected 'account-access' got 'access-analyzer'.
Cause 4 - uxc-error-list-services-invalid-token-pattern#task#2, 200/200: $.services[0] expected 'account-access' got 'access-analyzer'; $.services[2] expected 'acm-pca' got 'acm'.
Cause 5 - uxc-list-services-pagination#task#1 (and #task#2), 200/200: $.services[0] expected 'account-access' got 'access-analyzer'; $.services[1] expected 'acm' got 'account'.
Cause 6 - uxc-error-list-services-invalid-token-pattern#task#1, expected 400 / actual 400, $.message value_mismatch:
  expected: "1 validation error detected: Value '!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [-A-Za-z0-9+/_]+=*"
  actual:   "1 validation error detected: Value '!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[-A-Za-z0-9+/_]+=*$"
  (the quoted value is the fixture's example input; the requirement is the template and the unanchored pattern rendering, not that exact string.)

Supporting source: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs lines 87-133 define the 45-entry SEED_SERVICE_CATALOG that lines 146-159 sort into service_catalog; services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/list_services.rs lines 270-298 page that catalog. The state.rs comment at lines 80-86 already concedes the catalog's contents are a separate defect.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Causes 1-5: replay uxc-catalog-list-services#task#1, uxc-baseline-read-and-list-services#task#3, uxc-configure-visible-services-roundtrip#setup#1, uxc-error-list-services-invalid-token-pattern#task#2 and uxc-list-services-pagination#task#1/#task#2 - i.e. GET /v1/services (optionally with maxResults/nextToken) against the uxc endpoint - and compare $.services element by element with the recorded body: the emulator returns 45 endpoint prefixes where the recording returns the 224+ identifier catalog, so index 0 is 'access-analyzer' instead of 'account-access'.
3. Cause 6: replay uxc-error-list-services-invalid-token-pattern#task#1 - GET /v1/services?nextToken=!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!! (40 characters that satisfy NextToken @length but violate its @pattern). The emulator answers 400 ValidationException whose $.message echoes '^[-A-Za-z0-9+/_]+=*$'; the recording echoes '[-A-Za-z0-9+/_]+=*' with no anchors.
4. After a fix, re-replay all six bindings listed in Issue-Trace/Issue-Step and trace_ids.
