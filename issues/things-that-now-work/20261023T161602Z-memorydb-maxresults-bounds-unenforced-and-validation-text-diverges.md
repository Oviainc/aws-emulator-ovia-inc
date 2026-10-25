Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-describe-maxresults-bounds
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-describe-maxresults-bounds memorydb-parameter-group-lifecycle memorydb-parameter-update-and-reset-rules memorydb-events-updates-and-offerings-validation
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|DescribeParameterGroups|4xx->2xx|$.__type
Issue-Key: memorydb|DescribeParameterGroups|4xx->2xx|$.message
Issue-Key: memorydb|DescribeParameterGroups|4xx->2xx|status
Issue-Key: memorydb|DescribeParameters|4xx->4xx|$.message
Issue-Key: memorydb|DescribeClusters|4xx->4xx|$.message
Issue-Key: memorydb|DescribeEvents|4xx->2xx|$.__type
Issue-Key: memorydb|DescribeEvents|4xx->2xx|$.message
Issue-Key: memorydb|DescribeEvents|4xx->2xx|status
Issue-Key: memorydb|DescribeEngineVersions|4xx->2xx|$.__type
Issue-Key: memorydb|DescribeEngineVersions|4xx->2xx|$.message
Issue-Key: memorydb|DescribeEngineVersions|4xx->2xx|status
Issue-Key: memorydb|DescribeParameters|4xx->2xx|$.__type
Issue-Key: memorydb|DescribeParameters|4xx->2xx|$.message
Issue-Key: memorydb|DescribeParameters|4xx->2xx|status
Issue-Key: memorydb|CreateParameterGroup|4xx->4xx|$.message
Issue-Commit: 205916506edbd7602d3640584e73a4ccd0a8f366

# MemoryDB MaxResults bounds unenforced and validation text diverges

> _Note: this issue was identified against the code base at commit `205916506edbd7602d3640584e73a4ccd0a8f366`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeParameterGroups

## Description
Seven causes, one shared fault: the emulator does not implement MemoryDB's MaxResults/MaxRecords page-size bound of 20..100 on the paginated Describe* operations — on four of them it accepts an out-of-range value and answers 200, and on two it rejects with a different range (1..50), i.e. it accepts 1..19 that AWS refuses and refuses 51..100 that AWS accepts. The seventh cause is the same validation layer emitting an identifier-constraint message that does not name the request member AWS names. Smithy models MaxResults as IntegerOptional with no @range on these operations (DescribeParameterGroups, DescribeParameters, DescribeClusters, DescribeEvents, DescribeEngineVersions all paginated with pageSize=MaxResults), so the 20..100 bound and the recorded wording are recorded behavior only; each operation declares InvalidParameterValueException @httpError 400. All seven causes must be closed before submitting.

Cause 1 of 7: DescribeParameterGroups with MaxResults outside 20..100. AWS: HTTP 400, __type 'InvalidParameterValueException', message of the form 'Invalid value for max results. Must be between 20 and 100'. Emulator: HTTP 200 with a page. Why: a client's paging bug that AWS surfaces as a hard fault succeeds against the emulator and its page size silently differs. Fix: DescribeParameterGroups input validation. Traces: memorydb-describe-maxresults-bounds#task#9.

Cause 2 of 7: DescribeParameters MaxResults bound is the wrong range. AWS: HTTP 400 InvalidParameterValueException, bound 20..100 ('Invalid value for max results. Must be between 20 and 100'). Emulator: HTTP 400 but enforcing 1..50 ('Invalid value for max results. Must be between 1 and 50'). The rule itself — which values are valid — is what diverges, not merely the text. Fix: DescribeParameters MaxResults validation bound. Traces: memorydb-describe-maxresults-bounds#task#10.

Cause 3 of 7: DescribeClusters MaxResults bound is the wrong range; AWS rejects with the 'max records' wording and bound 20..100, the emulator enforces 1..50. Fix: DescribeClusters MaxResults validation bound. Traces: memorydb-describe-maxresults-bounds#task#12.

Cause 4 of 7: DescribeEvents with MaxResults outside 20..100. AWS: HTTP 400 InvalidParameterValueException ('Invalid value for max records. Must be between 20 and 100'). Emulator: HTTP 200. Fix: DescribeEvents input validation. Traces: memorydb-describe-maxresults-bounds#task#13.

Cause 5 of 7: DescribeEngineVersions with MaxResults outside 20..100. AWS: HTTP 400 InvalidParameterValueException. Emulator: HTTP 200. Fix: DescribeEngineVersions input validation. Traces: memorydb-describe-maxresults-bounds#task#14.

Cause 6 of 7: DescribeParameters with an out-of-range MaxResults in the parameter-group lifecycle fixture is accepted outright. AWS: HTTP 400 InvalidParameterValueException. Emulator: HTTP 200 — so the operation both mis-bounds (cause 2) and, on this path, skips the check entirely. Fix: apply the single shared MaxResults bound on every paginated Describe* entry point. Traces: memorydb-parameter-group-lifecycle#task#7.

Cause 7 of 7: CreateParameterGroup identifier-validation message does not name the request member. AWS builds the message from the request member it rejected: 'The parameter ParameterGroupName is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.' The emulator emits 'Parameter group name must begin with a letter; ...' — same status 400 and same __type, but the member-naming rule is lost. Why: clients and operators that key on the 'The parameter <Member> is not a valid identifier' form (and tooling that extracts the offending member) cannot. Fix: the memorydb identifier validator's message construction, so it names the input member. Traces: memorydb-parameter-update-and-reset-rules#task#15.

## Evidence
Cause 1: expected 400, got 200; $.__type missing (expected 'InvalidParameterValueException'), $.message missing (expected 'Invalid value for max results. Must be between 20 and 100') — memorydb-describe-maxresults-bounds task#9; cluster also covers memorydb-parameter-group-lifecycle#teardown#15 and memorydb-parameter-update-and-reset-rules#task#17. Cause 2: $.message expected 'Invalid value for max results. Must be between 20 and 100', got 'Invalid value for max results. Must be between 1 and 50' (task#10). Cause 3: expected 'Invalid value for max records. Must be between 20 and 100', got 'Invalid value for max results. Must be between 1 and 50' (task#12). Cause 4: expected 400, got 200 (task#13; also memorydb-events-updates-and-offerings-validation task#3 and task#8). Cause 5: expected 400, got 200 (task#14). Cause 6: expected 400, got 200 (memorydb-parameter-group-lifecycle task#7). Cause 7: $.message expected 'The parameter ParameterGroupName is not a valid identifier. ...', got 'Parameter group name must begin with a letter; ...' (memorydb-parameter-update-and-reset-rules task#15).

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1. Call DescribeParameterGroups, DescribeParameters, DescribeClusters, DescribeEvents and DescribeEngineVersions with MaxResults below 20 and above 100: AWS answers HTTP 400 InvalidParameterValueException with the 20..100 bound; the emulator answers 200 (DescribeParameterGroups, DescribeEvents, DescribeEngineVersions, DescribeParameters in memorydb-parameter-group-lifecycle) or 400 quoting a 1..50 bound (DescribeParameters, DescribeClusters). Then call CreateParameterGroup with an identifier containing two consecutive hyphens and compare the message form. Fixtures/steps: memorydb-describe-maxresults-bounds#task#9, #task#10, #task#12, #task#13, #task#14, memorydb-parameter-group-lifecycle#task#7, memorydb-parameter-update-and-reset-rules#task#15.
