Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-ettoi6ktqubiulxud5s5jsf24b"].deletes
Issue-Commit: e4b73bef21735acad04abca14d336dea5ddc072f

# AppSync ListApiKeys reports the wrong ApiKey deletes (deletion-window offset)

> _Note: this issue was identified against the code base at commit `e4b73bef21735acad04abca14d336dea5ddc072f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
1 cause.

Cause 1 of 1: ListApiKeys projects each da2 API key's `deletes` with a shorter deletion grace period than AWS applies, so the listed deletion time is earlier than the one AWS reports for the same `expires`.

AWS: `ApiKey$deletes` is "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour." (Smithy: com.amazonaws.appsync#ApiKey member `deletes`, target Long, default 0; ListApiKeys output member `apiKeys` -> list of ApiKey; http GET /v1/apis/{apiId}/apikeys, code 200). The ApiKey docs further state that for key version **da2** "ListApiKeys returns the expiration time and deletion time in seconds". AWS derives `deletes` from the key's `expires` plus the service's fixed deletion grace period, so for a given `expires` the value is determined rather than arbitrary. In the recording the listed key's `deletes` sits exactly 5,184,000 seconds (60 days) beyond the value the emulator returns, i.e. AWS adds a 60-day post-expiry retention window. Hypothesis: the emulator adds a shorter window (or none) on top of `expires`.

Emulator: the key returned by ListApiKeys carries a `deletes` that is 5,184,000 s short of the recorded one ($.apiKeys[id="..."].deletes expected 1803945600, got 1798761600). Status is 200 on both sides and every other member of the listed ApiKey matches; only the derived deletion timestamp diverges. The id in the JSONPath and the two epoch numbers themselves are per-run values (the key id is generated and the window is anchored on the create time) - the requirement is the fixed 60-day offset from `expires`, not these literals.

Why: a client that schedules key rotation, renewal or cleanup off `deletes` computes a reclaim date that does not exist on AWS. It believes keys are already reclaimed while AWS still serves them (or renews 60 days too early), and inventory/report tooling that lists keys and diffs the window against AWS disagrees with the real service.

Fix: the ListApiKeys handler's ApiKey projection - specifically the shared `deletes` derivation it reads from state (expires + deletion grace period). The grace period must be the same 60-day window AWS applies, and ListApiKeys must project the stored value rather than recomputing it with a different constant.

Traces: appsync-api-key-lifecycle#task#4 (ListApiKeys, 200 vs 200, $.apiKeys[id=...].deletes).

Related open issue that does NOT cover this cluster: 20261029T200818Z-appsync-api-key-deletion-window-and-tagging-responses.md describes the same deletion-window arithmetic and even names a ListApiKeys cause, but its recorded replay key is pinned to a different, per-run API-key id (`appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-dq5e45sgwr3mamypsk3zabjbfx"].deletes`), so this replay's ListApiKeys cluster (key id `da2-ettoi6ktqubiulxud5s5jsf24b`) is not covered by it and re-appears unowned on every sweep. A fixer closing that issue should treat this one as the same root cause and re-replay appsync-api-key-lifecycle#task#4 to confirm.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false).

Fixture appsync-api-key-lifecycle, step task#4, operation ListApiKeys, verdict mismatch:
- status: expected 200, got 200 (status agrees)
- $.apiKeys[id="da2-ettoi6ktqubiulxud5s5jsf24b"].deletes: value_mismatch - expected 1803945600, got 1798761600
- difference: 1803945600 - 1798761600 = 5184000 s = exactly 60 days

Cluster: divergence_key service=appsync|ListApiKeys|status=200->200|keys=$.apiKeys[id="da2-ettoi6ktqubiulxud5s5jsf24b"].deletes; affected_count 1 (appsync-api-key-lifecycle#task#4); stub_path empty; owned_by empty (no open issue carries this replay key).

Model evidence: Smithy com.amazonaws.appsync#ApiKey.deletes - "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour."; ApiKey docs, da2 section - "ListApiKeys returns the expiration time and deletion time in seconds."; ListApiKeys http GET /v1/apis/{apiId}/apikeys code 200, output apiKeys: ApiKeys.

## Steps to Reproduce
1. Clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus; fixture appsync-api-key-lifecycle mismatches at step task#4.
3. By hand: CreateGraphqlApi, then CreateApiKey with an explicit `expires` (seconds since epoch, rounded down to the hour).
4. Call ListApiKeys on that apiId and read the listed key's `deletes`.
5. Observe `deletes` is short of `expires` + 60 days (the emulator applies a shorter retention window); AWS returns `expires` plus the fixed 60-day deletion window, rounded down to the hour.
