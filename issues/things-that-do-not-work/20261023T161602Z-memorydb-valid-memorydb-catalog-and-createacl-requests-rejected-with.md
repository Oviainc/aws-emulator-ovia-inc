Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-catalog-describe-engine-versions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-catalog-describe-engine-versions memorydb-catalog-describe-parameter-groups memorydb-default-user-and-open-access-guards
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|DescribeEngineVersions|2xx->4xx|$.EngineVersions
Issue-Key: memorydb|DescribeEngineVersions|2xx->4xx|status
Issue-Key: memorydb|DescribeParameterGroups|2xx->4xx|$.ParameterGroups
Issue-Key: memorydb|DescribeParameterGroups|2xx->4xx|status
Issue-Key: memorydb|CreateACL|2xx->4xx|$.ACL
Issue-Key: memorydb|CreateACL|2xx->4xx|status
Issue-Commit: 205916506edbd7602d3640584e73a4ccd0a8f366

# Valid MemoryDB catalog and CreateACL requests rejected with 400

> _Note: this issue was identified against the code base at commit `205916506edbd7602d3640584e73a4ccd0a8f366`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEngineVersions

## Description
Three causes, one shared fault: requests AWS answers successfully are rejected by the emulator with HTTP 400 and no response body, so the operation is unusable rather than merely imprecise. All three causes must be closed before submitting.

Cause 1 of 3: DescribeEngineVersions as issued by the catalog fixture. AWS: HTTP 200 with an EngineVersions list (Smithy: DescribeEngineVersionsResponse.EngineVersions targets EngineVersionInfoList of EngineVersionInfo{Engine, EngineVersion, EnginePatchVersion, ParameterGroupFamily}; all inputs optional). Emulator: HTTP 400 with no EngineVersions member — the request is refused outright. Hypothesis: an input combination the emulator's validator rejects that AWS accepts (the same operation returns 200 in other fixtures). Why: a client enumerating supported engine versions — the normal precursor to CreateCluster — cannot proceed at all. Fix: DescribeEngineVersions input validation / handler.
Traces: memorydb-catalog-describe-engine-versions#task#1.

Cause 2 of 3: DescribeParameterGroups listing the default parameter groups. AWS: HTTP 200 with a ParameterGroups list containing the default.memorydb-* groups (ARN, Description, Family, Name per entry). Emulator: HTTP 400, no ParameterGroups member. Why: a client discovering the default parameter group to pass to CreateCluster cannot. Fix: DescribeParameterGroups input validation / handler.
Traces: memorydb-catalog-describe-parameter-groups#task#1.

Cause 3 of 3: CreateACL for a valid new ACL with no users. AWS: HTTP 200 returning the ACL (Name, ARN, Status, empty UserNames, empty Clusters, MinimumEngineVersion). Emulator: HTTP 400 with no ACL member. Why: ACL creation — the prerequisite for CreateCluster, which requires ACLName — fails for a request AWS accepts, blocking the whole cluster-creation path. Fix: the CreateACL handler's validation (it rejects a request AWS accepts; see also the related guard defects filed separately for CreateACL with a missing user).
Traces: memorydb-default-user-and-open-access-guards#task#7.

## Evidence
Cause 1: status expected 200, got 400; $.EngineVersions missing (expected 5 entries: redis 6.2/7.0/7.1 and valkey 7.2/7.3 with their ParameterGroupFamily values) — memorydb-catalog-describe-engine-versions task#1. Cause 2: status expected 200, got 400; $.ParameterGroups missing (expected the six default.memorydb-redis6/redis7/redis7.search/redis7.search.preview/valkey7/valkey7.search groups) — memorydb-catalog-describe-parameter-groups task#1. Cause 3: status expected 200, got 400; $.ACL missing (expected {ARN, Clusters: [], MinimumEngineVersion: '6.2', Name, Status, UserNames: []}) — memorydb-default-user-and-open-access-guards task#7.

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1: (1) DescribeEngineVersions as recorded in memorydb-catalog-describe-engine-versions#task#1 — expect HTTP 200 with EngineVersions, emulator returns 400; (2) DescribeParameterGroups as recorded in memorydb-catalog-describe-parameter-groups#task#1 — expect HTTP 200 with the default parameter groups, emulator returns 400; (3) CreateACL with a fresh ACLName and no UserNames as recorded in memorydb-default-user-and-open-access-guards#task#7 — expect HTTP 200 with the new ACL, emulator returns 400.
