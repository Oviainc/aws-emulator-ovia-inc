Issue-Source: trace-sweep-divergence
Issue-Trace: launch-wizard-workload-catalog-discovery
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: launch-wizard-workload-catalog-discovery
Issue-Shape-Id: com.amazonaws.launchwizard#LaunchWizard
Issue-Key: launchwizard|ListDeploymentPatternVersions|2xx->2xx|$.deploymentPatternVersions[*]
Issue-Key: launchwizard|ListDeploymentPatternVersions|2xx->2xx|$.nextToken
Issue-Key: launchwizard|GetDeploymentPatternVersion|2xx->2xx|$.deploymentPatternVersion.featureFlags
Issue-Commit: 7f92290cd08a3e8e509afee1d943cf5282438c9c

# Deployment pattern version responses ignore pagination and omit featureFlags

> _Note: this issue was identified against the code base at commit `7f92290cd08a3e8e509afee1d943cf5282438c9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDeploymentPatternVersions

## Description
The launchwizard deployment-pattern-version reads both answer HTTP 200 with a body that does not match what the recorded service returns for the same request: the list operation serves its whole seeded version list in one page and never emits a continuation token, and the single-item read drops the featureFlags member of the returned deploymentPatternVersion. The shared fault is that the deployment-pattern-version projection is served straight out of the seed with no paging layer and with a member set narrower than the service's. 2 causes.

--- Cause 1 of 2: ListDeploymentPatternVersions ignores the paging inputs and never returns nextToken (fixture launch-wizard-workload-catalog-discovery, step task#5) ---
AWS: Smithy: ListDeploymentPatternVersions, POST /listDeploymentPatternVersions, success 200; input members workloadName (required), deploymentPatternName (required), maxResults: MaxWorkloadResults @range 1..100, nextToken: NextToken @length 1..2048, filters: FilterList; output members deploymentPatternVersions: DeploymentPatternVersionDataSummaryList and nextToken: NextToken @length 1..2048; @paginated {inputToken: nextToken, outputToken: nextToken, pageSize: maxResults, items: deploymentPatternVersions}. Being @paginated is the contract: the service answers with ONE page of deploymentPatternVersions sized by the request (not the full collection), and whenever more results remain it also serializes nextToken, an opaque continuation value the client must send back as the next request's nextToken. The recording for this step shows exactly that paged shape: the response carries a nextToken (the corpus sanitizes its value to 'opaque-pagination-token' because the token's bytes are per-run) and does NOT carry the three seeded version entries the emulator serves. The requirement's fixed part is "one page of items plus a nextToken while a further page exists, and the token round-trips to the next page"; the page contents, the page size and the token's literal bytes are request- and state-derived values, not part of the requirement. Hypothesis: the recorded request narrows the first page (via maxResults and/or filters) so that it carries no entry of its own while more results remain, which is legal and is why clients are required to loop on nextToken rather than on a non-empty page.
Emulator: 200, but $.deploymentPatternVersions holds three entries the recording's page does not contain - [0] deploymentPatternVersionName '1.0.27', [1] '1.0.1', [2] '1.0.0', all for workloadName 'SAP' / deploymentPatternName 'SapHanaHA', each with a synthesised 'Version <n> of this deployment pattern. ...' description - all flagged 'extra (expected None, got ...)', and $.nextToken is absent (expected 'opaque-pagination-token', got None). The paging inputs have no effect on the body: the whole seeded version list is emitted every call as a single terminal page.
Why: a client that paginates correctly gets the wrong answer in both directions. Because nextToken is never emitted, an SDK paginator stops after the first response and a client that genuinely has more versions than one page silently truncates its view of the catalog; because maxResults/filters do not bound the page, a client that asked for a bounded page is handed the entire collection and any assertion on page size, on page boundaries, or on a continuation round-trip fails. Pagination-loop code exercised against the emulator is therefore never exercised at all, so the bug it is meant to prevent escapes to a real endpoint.
Fix: the ListDeploymentPatternVersions handler and its state query in the launchwizard crate - apply maxResults (and filters) to slice the version collection into a page, mint an opaque nextToken whenever unserved entries remain, and resume from a supplied nextToken on the following call; the serializer must then emit nextToken only when a further page exists.
Traces: launch-wizard-workload-catalog-discovery#task#5.

--- Cause 2 of 2: GetDeploymentPatternVersion omits the featureFlags member of deploymentPatternVersion (fixture launch-wizard-workload-catalog-discovery, step task#6) ---
AWS: Smithy: GetDeploymentPatternVersion, POST /getDeploymentPatternVersion, success 200, output member deploymentPatternVersion: DeploymentPatternVersionDataSummary; errors ResourceNotFoundException (404), InternalServerException (500). The on-disk model's DeploymentPatternVersionDataSummary declares only deploymentPatternVersionName, description, documentationUrl, workloadName and deploymentPatternName - it does not declare featureFlags - so for this member: recorded behavior only. The recording shows the service returning deploymentPatternVersion.featureFlags as a present structure describing which optional deployment capabilities the version supports, with a per-capability status and, where a capability is configurable, its configuration: cloudFormationOutput {configuration: {fields: []}, status: 'DISABLED'}, modifyDeployments {status: 'DISABLED'}, postDeploymentSteps {status: 'DISABLED'}, and updateFromVersion: []. The requirement is that the member is present and carries one entry per capability with that capability's status (and an empty configuration/list where it has none); the particular statuses and the empty collections above are this version's state, not fixed text.
Emulator: 200 with $.deploymentPatternVersion.featureFlags absent from the body entirely (expected the structure above, got None).
Why: featureFlags is how a client decides what it may do with a deployment pattern version - whether it can read CloudFormation outputs, modify an existing deployment, run post-deployment steps, or upgrade from an earlier version. An absent member is not the same as 'no capabilities': a client that reads featureFlags.modifyDeployments.status without a null guard faults on the emulator while working against AWS, and one that treats absence as 'capability available' offers an action the version does not support. Either way the emulator cannot be used to test capability-gated client paths.
Fix: the GetDeploymentPatternVersion handler/serializer and the deployment-pattern-version seed in the launchwizard crate - carry a featureFlags projection on each seeded version and serialize it on the response, emitting each capability's status plus its empty configuration/list rather than skipping the member.
Traces: launch-wizard-workload-catalog-discovery#task#6.

Relation to open issues: open issue 20261109T152700Z-launchwizard-getworkload-serves-an-off-emulator-icon-url-and.md covers GetWorkload's iconUrl host and accountConstraints on step task#2 of the same fixture, and open issue 20261029T234833Z-launch-wizard-catalog-returns-placeholder-workload-data-listworkloads.md covers the placeholder workload/deployment-pattern catalog content on the launch-wizard token. Neither carries a replay key for task#5 or task#6, so neither covers the pagination or featureFlags defects filed here.

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 33 steps, 22 passed, 11 mismatched, score 66.7%).

Cause 1 - step launch-wizard-workload-catalog-discovery#task#5, operation ListDeploymentPatternVersions (service launchwizard), verdict mismatch, expected status 200 / actual status 200:
- $.deploymentPatternVersions[0]: extra - expected None, got {'deploymentPatternVersionName': '1.0.27', 'description': 'Version 1.0.27 of this deployment pattern. Deploys an SAP HANA database as a highly available pair across two Availability Zones.', 'workloadName': 'SAP', 'deploymentPatternName': 'SapHanaHA'}
- $.deploymentPatternVersions[1]: extra - expected None, got the same entry at version '1.0.1'
- $.deploymentPatternVersions[2]: extra - expected None, got the same entry at version '1.0.0'
- $.nextToken: missing - expected 'opaque-pagination-token', got None
The expected nextToken string is the corpus's sanitized stand-in for the real opaque token: the requirement is that SOME continuation token is emitted while a further page remains and that it resumes the listing, not that it equals that literal. Both sides answer HTTP 200, so no status, error code or error message is involved.

Cause 2 - step launch-wizard-workload-catalog-discovery#task#6, operation GetDeploymentPatternVersion (service launchwizard), verdict mismatch, expected status 200 / actual status 200:
- $.deploymentPatternVersion.featureFlags: missing - expected {'cloudFormationOutput': {'configuration': {'fields': []}, 'status': 'DISABLED'}, 'modifyDeployments': {'status': 'DISABLED'}, 'postDeploymentSteps': {'status': 'DISABLED'}, 'updateFromVersion': []}, got None (member absent from the body).
The member names and the presence of a status per capability are the requirement; the 'DISABLED' values and the empty fields/updateFromVersion collections are this version's state. Both sides answer HTTP 200, so no status, error code or error message is involved.

Neither divergence reports a stub_path (so neither operation is unimplemented) and neither reports an owned_by, so no already-open issue carries their replay keys.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, and point a Launch Wizard client at its data listener (the replay reached it on /__admin/health 200 plus the data listener).
2. Cause 1: POST /listDeploymentPatternVersions for the seeded workload/pattern pair, bounding the page with the modeled paging inputs - body {"workloadName":"SAP","deploymentPatternName":"SapHanaHA","maxResults":1}. Observe HTTP 200 whose $.deploymentPatternVersions contains the full seeded version list (1.0.27, 1.0.1, 1.0.0) instead of a single-entry page, and whose $.nextToken member is absent, so an SDK paginator terminates after one response.
3. Re-issue the same call passing any value as nextToken: the body is unchanged, confirming the continuation token is neither minted nor consumed.
4. Cause 2: POST /getDeploymentPatternVersion with body {"workloadName":"SAP","deploymentPatternName":"SapHanaHA","deploymentPatternVersionName":"1.0.27"}. Observe HTTP 200 whose $.deploymentPatternVersion has no featureFlags member at all; a client dereferencing featureFlags.modifyDeployments.status faults.
5. Replay launch-wizard-workload-catalog-discovery#task#5 and #task#6 to confirm both fixes; both must pass before this issue is closed.
