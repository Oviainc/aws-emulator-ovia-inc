Issue-Source: trace-sweep-divergence
Issue-Trace: amp-workspace-lifecycle-and-configuration
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amp-workspace-lifecycle-and-configuration amp-scraper-error-paths-and-defaults amp-logging-configuration-workflow amp-query-logging-configuration-workflow amp-rule-groups-namespace-workflow
Issue-Shape-Id: com.amazonaws.amp#AmazonPrometheusService
Issue-Key: aps|ListWorkspaces|2xx->2xx|$headers.content-type
Issue-Key: aps|CreateWorkspace|4xx->4xx|$.fieldList
Issue-Key: aps|CreateWorkspace|4xx->4xx|$.message
Issue-Key: aps|CreateWorkspace|4xx->4xx|$.reason
Issue-Key: aps|CreateWorkspace|4xx->4xx|$headers.content-type
Issue-Key: aps|ListScrapers|2xx->4xx|$.scrapers
Issue-Key: aps|ListScrapers|2xx->4xx|$headers.content-type
Issue-Key: aps|ListScrapers|2xx->4xx|status
Issue-Key: aps|DescribeWorkspace|4xx->4xx|$.message
Issue-Key: aps|DescribeWorkspace|4xx->4xx|$headers.content-type
Issue-Key: aps|DescribeLoggingConfiguration|4xx->4xx|$.message
Issue-Key: aps|DescribeLoggingConfiguration|4xx->4xx|$headers.content-type
Issue-Key: aps|DescribeQueryLoggingConfiguration|4xx->4xx|$.message
Issue-Key: aps|DescribeQueryLoggingConfiguration|4xx->4xx|$headers.content-type
Issue-Key: aps|CreateRuleGroupsNamespace|4xx->4xx|$.fieldList
Issue-Key: aps|CreateRuleGroupsNamespace|4xx->4xx|$.message
Issue-Key: aps|CreateRuleGroupsNamespace|4xx->4xx|$.reason
Issue-Key: aps|CreateRuleGroupsNamespace|4xx->4xx|$headers.content-type
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# aps wire fidelity: wrong content-type on every response, ValidationException missing reason/fieldList, ListScrapers rejects valid input, not-found message embeds the id

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListWorkspaces

## Description
Replaying the simparity-wire-aws-v1 corpus against the amp (Amazon Managed Service for Prometheus, sigv4Name/arnNamespace `aps`) emulator produced 16 divergence clusters over 41 steps, 0 passed. They reduce to FOUR independent causes in the aps response path. All four must be closed before this issue is submitted.

Cause 1 of 4 - every aps response is serialized with content-type `application/json` instead of `application/x-amz-json-1.1`.
AWS: recorded behavior only. The Smithy model declares protocol restJson1 (service AmazonPrometheusService, version 2020-08-01, sdkId `amp`), but every recorded real-AWS aps response in the corpus - successes and errors alike - carries the response header `content-type: application/x-amz-json-1.1`. This is observed on all 16 clusters across all 4 fixtures, on a 200 (ListWorkspaces), on 400s and on 404s, so it is the service's uniform response content-type and not an artifact of one recording.
Emulator: emits `content-type: application/json` on every aps response. This single header is the only divergence on the clean 200 cluster (ListWorkspaces, status 200 -> 200), which isolates the cause from the error-path causes below.
Why: a client that dispatches on the response media type - a proxy, a recorder, a logging/compliance middleware, or a hand-rolled HTTP client that content-sniffs before parsing - sees a different protocol family than real aps advertises and either mis-frames or refuses the body. Replayed captures taken against the emulator will also not byte-compare against captures taken against AWS.
Fix: the aps response serializer / content-type selection in the amp service crate's response layer, applied uniformly to success and error responses.
Traces: amp-workspace-lifecycle-and-configuration#task#7 (200, content-type is the sole mismatch); also present on all of amp-logging-configuration-workflow#task#1, amp-query-logging-configuration-workflow#task#1, amp-rule-groups-namespace-workflow#task#1, amp-scraper-error-paths-and-defaults#task#2.

Cause 2 of 4 - ValidationException omits the required `reason` member and the `fieldList` member, and does not build its message from the offending member and its constraint.
AWS: Smithy: error ValidationException, httpError 400, members message (String, @required), reason (ValidationExceptionReason, @required), fieldList (ValidationExceptionFieldList); ValidationExceptionField has @required name and @required message. So a 400 from aps is required by the model to carry `reason`, and the model gives the per-field breakdown a home. The recordings confirm aps populates all three: `reason` is set, `fieldList` carries one entry per rejected member as {name, message}, and the top-level `message` is composed from the request as `Invalid <memberName>: <constraint message>` where the constraint message is the member's modeled constraint rendered WITHOUT the regex anchors (e.g. for CreateLoggingConfiguration.logGroupArn, modeled pattern ^arn:aws[a-z0-9-]*:logs:[a-z0-9-]+:[0-9]{12}:log-group:[A-Za-z0-9\.\-\_\#/]{1,512}\:\*$, rendered as `Member must satisfy regular expression pattern: arn:aws[a-z0-9-]*:logs:...\:\*`; for CreateWorkspace.kmsKeyArn, modeled pattern ^arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/[-a-f0-9]+$, rendered as `...pattern: arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+`; for CreateRuleGroupsNamespace.name, @length max 128, rendered as `Member must have length less than or equal to 128`). That composition rule - name the member, state its constraint, repeat it per field in fieldList - is the requirement; the literal recorded strings are examples of it.
Emulator: returns the correct status 400, but the body has `reason: null/absent` and `fieldList: null/absent`, and the message is a generic envelope that does not match the aps composition rule: `1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must ...`, echoing the raw request value and leaking the anchored form of the pattern (^...$).
Why: `reason` is @required, so an AWS SDK deserializing this ValidationException is handed a structure missing a required member; strictly-typed SDKs and any consumer switching on reason (FIELD_VALIDATION_FAILED vs CANNOT_PARSE vs OTHER) cannot work, and any client that surfaces per-field errors to a user from fieldList gets nothing to show. Echoing the submitted value back into the message additionally reflects caller input (which may be sensitive) into an error string where aps does not.
Fix: the aps validation layer and its ValidationException serializer - populate reason, build fieldList entries ({name, message}) for each rejected member, and compose the top-level message from the member name plus its unanchored modeled constraint.
Traces: amp-workspace-lifecycle-and-configuration#task#1 (CreateWorkspace, kmsKeyArn pattern), amp-logging-configuration-workflow#task#2 (CreateLoggingConfiguration, logGroupArn pattern), amp-rule-groups-namespace-workflow#task#1 (CreateRuleGroupsNamespace, name @length 128).

Cause 3 of 4 - ListScrapers rejects a request AWS accepts, answering 400 instead of 200 with an empty list.
AWS: Smithy: ListScrapers, GET /scrapers, http code 200, output ListScrapersResponse with @required `scrapers` (ScraperSummaryList) and optional `nextToken`; input ListScrapersRequest has ONLY optional members - `filters` (@httpQueryParams map, @length 1..4), `nextToken` (@httpQuery, @length max 1000), `maxResults` (@httpQuery, @range 1..1000). The operation is @readonly and @paginated. A listing call with no matching scrapers is a success: 200 with `scrapers: []`. The recording shows exactly that.
Emulator: answers 400 to the same request and returns no `scrapers` member at all. Hypothesis: the handler treats the optional query input (the @httpQueryParams `filters` map and/or an omitted/defaulted maxResults) as invalid rather than optional, so the request never reaches the listing path; the model makes every ListScrapers input optional, so no modeled constraint justifies a 400 here.
Why: an empty-result listing is the normal state of a fresh account, so a client enumerating scrapers - including any SDK paginator, which must see `scrapers` to advance or terminate - gets a hard client error instead of an empty page. Code that treats 400 as non-retryable caller error will abort a workflow that succeeds against AWS.
Fix: the ListScrapers handler's input binding/validation in the amp service crate - accept the optional filters/nextToken/maxResults query input and return 200 with `scrapers: []` when nothing matches.
Traces: amp-scraper-error-paths-and-defaults#task#2 and #task#3 (both expected 200 with scrapers: [], both got 400).

Cause 4 of 4 - ResourceNotFoundException interpolates the workspace id into `message` instead of returning the service's constant message.
AWS: Smithy: error ResourceNotFoundException, httpError 404, members message (@required), resourceId (@required), resourceType (@required) - the identifier of the missing resource has its own modeled home in `resourceId`, which is why the message does not need to carry it. Recorded behavior confirms it: across 10 clusters spanning 4 fixtures and 10 different operations (DescribeWorkspace, UpdateWorkspaceAlias, DescribeWorkspaceConfiguration, UpdateWorkspaceConfiguration, DeleteWorkspace, DescribeLoggingConfiguration, CreateLoggingConfiguration, DescribeQueryLoggingConfiguration, CreateQueryLoggingConfiguration, CreateRuleGroupsNamespace, DescribeRuleGroupsNamespace), aps returns a constant workspace-not-found message that never embeds the requested workspace id. The requirement is that constant, id-free message; the exact recorded text is only an example and in fact varies in punctuation between operations ('Workspace not found.' on most, 'Workspace not found' on DescribeWorkspace), so do not assert one literal string.
Emulator: returns the right status (404) and the right modeled members, but builds the message by appending the requested id: `Workspace not found: <workspaceId>`.
Why: the message becomes request-dependent where aps's is constant, so any client, test or log-assertion that compares the not-found message across calls sees a value that changes per request, and the id is duplicated into a human-readable field that AWS keeps free of caller input (it is already available, structured, in `resourceId`).
Fix: the aps ResourceNotFoundException construction in the workspace lookup path (shared by all per-workspace operations) - emit the constant message and keep the identifier in `resourceId`.
Traces: amp-workspace-lifecycle-and-configuration#task#2 (DescribeWorkspace), #task#3 (UpdateWorkspaceAlias), #task#4, #task#5, #task#6; amp-logging-configuration-workflow#task#1, #task#3; amp-query-logging-configuration-workflow#task#1, #task#2; amp-rule-groups-namespace-workflow#task#2, #task#3.

Scope notes: replay reported state_reset_failures 0 and bootstrap_signal false, so the emulator was serving and each fixture ran against clean state - these are fidelity gaps, not replay artifacts. None of these clusters carried a stub_path or an owned_by. The open issue 20261022T110707Z-amp-3-fidelity-divergences-prometheusendpoint-announces-the-real covers different causes (prometheusEndpoint host, TagResource 404 error code, RCF query validation) and does not cover any of the four above. The open stub issue 20261022T110707Z-amp-createloggingconfiguration-stub names CreateLoggingConfiguration, but that operation now answers on the wire (400/404 with bodies), so causes 1, 2 and 4 are live divergences in its response path and are not resolved by implementing the stub.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, state_reset_failures 0, fixtures_replayed 8, steps_total 41, steps_passed 0, steps_mismatched 41, divergence_count 16, score_pct 0.0. All 16 clusters are service=aps with empty stub_path and empty owned_by.

Cause 1 (content-type) - amp-workspace-lifecycle-and-configuration#task#7, ListWorkspaces, status 200 -> 200, sole mismatch:
  $headers.content-type: expected 'application/x-amz-json-1.1', got 'application/json'
The same header mismatch is reported on all 16 clusters (every operation, every status).

Cause 2 (ValidationException shape) - status 400 -> 400 in all three, body diverges:
  amp-workspace-lifecycle-and-configuration#task#1, CreateWorkspace:
    $.reason: missing (expected 'Validation error', got None)
    $.fieldList: missing (expected [{'name': 'kmsKeyArn', 'message': 'Member must satisfy regular expression pattern: arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+'}], got None)
    $.message: expected 'Invalid kmsKeyArn: Member must satisfy regular expression pattern: arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+', got "1 validation error detected: Value 'not-a-valid-kms-arn-at-all' at 'kmsKeyArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/[-a-f0-9]+$"
  amp-logging-configuration-workflow#task#2, CreateLoggingConfiguration:
    $.reason: missing (expected 'Validation error', got None)
    $.fieldList: missing (expected [{'name': 'logGroupArn', 'message': 'Member must satisfy regular expression pattern: arn:aws[a-z0-9-]*:logs:[a-z0-9-]+:[0-9]{12}:log-group:[A-Za-z0-9\\.\\-\\_\\#/]{1,512}\\:\\*'}], got None)
    $.message: expected 'Invalid logGroupArn: Member must satisfy regular expression pattern: arn:aws[a-z0-9-]*:logs:...', got "1 validation error detected: Value 'not-a-valid-arn' at 'logGroupArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[a-z0-9-]*:logs:...$"
  amp-rule-groups-namespace-workflow#task#1, CreateRuleGroupsNamespace:
    $.reason: missing (expected 'Validation error', got None)
    $.fieldList: missing (expected [{'name': 'name', 'message': 'Member must have length less than or equal to 128'}], got None)
    $.message: expected 'Invalid name: Member must have length less than or equal to 128', got "1 validation error detected: Value 'aaaa...<129 chars>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 128"
Model: ValidationException.reason is @required (ValidationExceptionReason), fieldList targets ValidationExceptionFieldList of {@required name, @required message}.

Cause 3 (ListScrapers) - amp-scraper-error-paths-and-defaults#task#2 (cluster also covers #task#3):
    status: expected 200, got 400
    $.scrapers: missing (expected [], got None)
Model: ListScrapers GET /scrapers, http code 200, @readonly, output @required scrapers; every input member (filters/nextToken/maxResults) is optional.

Cause 4 (not-found message) - status 404 -> 404 in all ten; representative amp-workspace-lifecycle-and-configuration#task#2, DescribeWorkspace:
    $.message: expected 'Workspace not found', got 'Workspace not found: ws-00000000000000000000000000'
and on the other nine (UpdateWorkspaceAlias, DescribeWorkspaceConfiguration, UpdateWorkspaceConfiguration, DeleteWorkspace, DescribeLoggingConfiguration, CreateLoggingConfiguration, DescribeQueryLoggingConfiguration, CreateQueryLoggingConfiguration, CreateRuleGroupsNamespace, DescribeRuleGroupsNamespace):
    $.message: expected 'Workspace not found.', got 'Workspace not found: ws-00000000000000000000000000'
The id-free message is constant across 10 operations in 4 fixtures; the trailing-period difference between recordings is why the literal text is cited as an example and the id-interpolation is the defect.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator mainline checkout and build/boot the emulator from it.
2. Replay the simparity-wire-aws-v1 wire corpus against it (replay_wire_corpus). 41/41 steps mismatch, 16 clusters, all service=aps.
3. Cause 1: replay amp-workspace-lifecycle-and-configuration step task#7 (ListWorkspaces). The call returns 200 with the expected body; inspect the response headers - content-type is application/json where the recording has application/x-amz-json-1.1. Repeat on any other aps step to see the same header on 400s and 404s.
4. Cause 2: replay amp-workspace-lifecycle-and-configuration task#1 (CreateWorkspace with a kmsKeyArn that fails the modeled pattern), amp-logging-configuration-workflow task#2 (CreateLoggingConfiguration with a non-ARN logGroupArn) and amp-rule-groups-namespace-workflow task#1 (CreateRuleGroupsNamespace with a 129-character name). Each returns 400; the JSON body has no `reason` and no `fieldList`, and `message` is the generic '1 validation error detected: ...' envelope with the anchored pattern instead of 'Invalid <member>: <constraint>'.
5. Cause 3: replay amp-scraper-error-paths-and-defaults steps task#2 and task#3 (ListScrapers on a workspace set with no scrapers). Both return 400 with no `scrapers` member; the recording returns 200 with scrapers: [].
6. Cause 4: replay amp-workspace-lifecycle-and-configuration task#2 (DescribeWorkspace on an absent workspace id) and any of the other nine per-workspace 404 steps listed in the evidence. Status is 404 as expected, but `message` has the requested workspace id appended, where aps returns a constant id-free message and carries the id in `resourceId`.
7. Re-replay the six fixture steps named in Issue-Trace/Issue-Step and trace_ids after fixing; all four causes must be closed.
