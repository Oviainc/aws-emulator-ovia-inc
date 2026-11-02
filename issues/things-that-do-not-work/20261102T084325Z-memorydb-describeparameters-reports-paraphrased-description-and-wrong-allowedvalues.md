Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-parameter-group-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-parameter-group-lifecycle
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="search-enabled"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="search-query-timeout-ms"].AllowedValues
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="search-query-timeout-ms"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-log-slower-than"].AllowedValues
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-log-slower-than"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-max-len"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="tracking-table-max-keys"].Description
Issue-Commit: 2df443d36cca12daf6920410c04ddab8da453d13

# MemoryDB DescribeParameters reports paraphrased Description and wrong AllowedValues for engine default parameters

> _Note: this issue was identified against the code base at commit `2df443d36cca12daf6920410c04ddab8da453d13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeParameters

## Description
Cause 1 of 1: the engine-default parameter catalog the emulator answers DescribeParameters from carries hand-written paraphrases of each parameter's Description and, for several parameters, a value domain that does not match the one MemoryDB publishes, so the per-parameter metadata in the 200 response is wrong even though the response status, shape and parameter name set are all correct.

AWS: DescribeParameters returns Parameters, a list of Parameter structures (Smithy: com.amazonaws.memorydb#Parameter, members Name/Value/Description/DataType/AllowedValues/MinimumEngineVersion; DescribeParameters input ParameterGroupName required, errors InvalidParameterCombinationException, InvalidParameterValueException, ParameterGroupNotFoundFault, ServiceLinkedRoleNotFoundFault). The model documents Parameter$Description as "A description of the parameter" and Parameter$AllowedValues as "The valid range of values for the parameter" but does not enumerate the per-parameter text or ranges, so the requirement here is recorded behavior only: MemoryDB publishes one FIXED description string and one FIXED allowed-value expression per engine parameter, keyed by the parameter's Name and the parameter group's engine family, and returns that exact published text. The recording shows, for the Valkey/Redis family of the group under test: search-enabled -> Description "When set to yes, it enables search capabilities."; search-query-timeout-ms -> Description "The maximum amount of time in milliseconds that a search query is allowed to run." and AllowedValues "1-60000"; slowlog-log-slower-than -> Description "The execution time, in microseconds, to exceed in order for the command to get logged. Note that a negative number disables the slow log, while a value of zero forces the logging of every command" and AllowedValues "-" (unbounded in both directions, because a negative value is meaningful: it disables the slow log); slowlog-max-len -> Description "The length of the slow log. There is no limit to this length. Just be aware that it will consume memory. You can reclaim memory used by the slow log with SLOWLOG RESET."; tracking-table-max-keys -> Description "The maximum number of keys allowed for the tracking table for client side caching". None of these strings is per-run or environment-dependent: there are no ids, hosts, timestamps or generated names in them, so the whole string is the fixed requirement for that Name.

Emulator: for the same parameters it answers its own paraphrases and two different domains. Observed: search-enabled.Description = "Enables or disables search"; search-query-timeout-ms.Description = "The timeout in milliseconds for search queries" and AllowedValues = "1-" (lower bound only, no upper bound); slowlog-log-slower-than.Description = "The execution time threshold (in microseconds) above which a command is logged in the slow log" and AllowedValues = "0-1000000" (a non-negative, capped range); slowlog-max-len.Description = "The maximum length of the slow log"; tracking-table-max-keys.Description = "The maximum number of keys in the invalidation table". HTTP status is 200 as expected and no other member of these Parameter entries diverges.

Why: a client that reads this metadata is misled twice. The Description strings are what consoles, generated docs and parameter-editor UIs display, so every one of these parameters is presented with text no AWS caller would ever see; a test that asserts on the documented description fails against the emulator but passes against AWS. The AllowedValues divergences are worse than cosmetic because they are the machine-readable domain: a client that validates before calling UpdateParameterGroup will reject slowlog-log-slower-than = -1 (which AWS accepts, and which is the documented way to DISABLE the slow log) because the emulator advertises "0-1000000", and will accept search-query-timeout-ms = 600000 because the emulator advertises an unbounded "1-" where AWS caps it at 60000. The emulator's own UpdateParameterGroup validation is driven by the same domains, so the accept/reject decision for these parameters diverges from AWS in both directions.

Fix: the service's state — the static engine-parameter catalog that DescribeParameters/DescribeParameterGroups answer from, not the DescribeParameters handler, which correctly reads the family's parameters out of state and pages them. Each catalog entry's description text must be MemoryDB's published string for that parameter name, and each entry's domain must render the published AllowedValues expression (including an unbounded/negative-permitting domain rendering as "-").

Traces: memorydb-parameter-group-lifecycle#task#6 (representative) and memorydb-parameter-group-lifecycle#task#11, same cluster, both DescribeParameters. Catalog entries involved: services/service-com-amazonaws-memorydb-amazonmemorydb/src/state.rs around lines 1319-1327 (slowlog-log-slower-than), 1330-1336 (slowlog-max-len), 1374-1380 (tracking-table-max-keys), 1530-1543 (search-enabled), 1545-1553 (search-query-timeout-ms); the handler that reads them is services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/describe_parameters.rs lines 196-227.

## Evidence
Fixture memorydb-parameter-group-lifecycle, step task#6, operation DescribeParameters, verdict mismatch, expected status 200 / actual status 200 (status and shape agree; only Parameter member VALUES diverge). Per-field expected vs actual as replayed:

$.Parameters[Name="search-enabled"].Description
  expected: 'When set to yes, it enables search capabilities.'
  actual:   'Enables or disables search'
$.Parameters[Name="search-query-timeout-ms"].AllowedValues
  expected: '1-60000'
  actual:   '1-'
$.Parameters[Name="search-query-timeout-ms"].Description
  expected: 'The maximum amount of time in milliseconds that a search query is allowed to run.'
  actual:   'The timeout in milliseconds for search queries'
$.Parameters[Name="slowlog-log-slower-than"].AllowedValues
  expected: '-'
  actual:   '0-1000000'
$.Parameters[Name="slowlog-log-slower-than"].Description
  expected: 'The execution time, in microseconds, to exceed in order for the command to get logged. Note that a negative number disables the slow log, while a value of zero forces the logging of every command'
  actual:   'The execution time threshold (in microseconds) above which a command is logged in the slow log'
$.Parameters[Name="slowlog-max-len"].Description
  expected: 'The length of the slow log. There is no limit to this length. Just be aware that it will consume memory. You can reclaim memory used by the slow log with SLOWLOG RESET.'
  actual:   'The maximum length of the slow log'
$.Parameters[Name="tracking-table-max-keys"].Description
  expected: 'The maximum number of keys allowed for the tracking table for client side caching'
  actual:   'The maximum number of keys in the invalidation table'

Affected steps in this cluster: memorydb-parameter-group-lifecycle#task#6, memorydb-parameter-group-lifecycle#task#11 (affected_count 2).

Emulator-side source of the diverging values: services/service-com-amazonaws-memorydb-amazonmemorydb/src/state.rs, e.g. line 1321 description: "The execution time threshold (in microseconds) above which a command is logged in the slow log" with domain ParameterDomain::Integer { min: 0, max: Some(1_000_000) } (line 1325), and line 1550 domain ParameterDomain::Integer { min: 1, max: None } for search-query-timeout-ms.

No open issue covers this; the only other open memorydb issue is 20261029T201322Z-memorydb-acl-membership-refusals-use-the-wrong-error.md (ACL membership error codes, unrelated), and the open DescribeParameters issues are for ssm and dax.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus() and look at fixture memorydb-parameter-group-lifecycle, step task#6 (also task#11).
2. Equivalently, by hand against the emulator: CreateParameterGroup with a Valkey/Redis family that carries the search parameters, then call DescribeParameters with that ParameterGroupName (or call DescribeParameters directly on the built-in default.<family> group, which the store materialises on first touch).
3. In the 200 response, read the Parameter entries whose Name is search-enabled, search-query-timeout-ms, slowlog-log-slower-than, slowlog-max-len and tracking-table-max-keys.
4. Compare each entry's Description against MemoryDB's published description for that parameter name, and compare AllowedValues for search-query-timeout-ms (should be the capped range 1-60000, emulator returns the uncapped 1-) and for slowlog-log-slower-than (should be the unbounded -, emulator returns the non-negative capped 0-1000000).
5. For the AllowedValues half, confirm the behavioral consequence: UpdateParameterGroup with slowlog-log-slower-than = -1 is accepted by AWS (it disables the slow log) but is outside the domain the emulator advertises and enforces.
