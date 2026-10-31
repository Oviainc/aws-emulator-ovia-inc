Issue-Source: trace-sweep-divergence
Issue-Trace: shield-attacks-and-tags-read-paths
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: shield-attacks-and-tags-read-paths
Issue-Shape-Id: com.amazonaws.shield#AWSShield_20160616
Issue-Key: shield|DescribeAttackStatistics|2xx->2xx|$.DataItems[*]
Issue-Commit: aede39749e16b7ea04942c6f83e8cac2192d58fe

# shield DescribeAttackStatistics omits AttackVolume from the data item

> _Note: this issue was identified against the code base at commit `aede39749e16b7ea04942c6f83e8cac2192d58fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAttackStatistics

## Description
Cause 1 of 1: DescribeAttackStatistics drops the AttackStatisticsDataItem$AttackVolume member entirely when no counted attack reported a volume unit, instead of serializing it as a present-but-empty structure. Fixture shield-attacks-and-tags-read-paths, step task#3.

AWS: Smithy (com.amazonaws.shield#DescribeAttackStatistics) — output DescribeAttackStatisticsResponse { TimeRange (required), DataItems: AttackStatisticsDataList (required) }; AttackStatisticsDataItem has AttackCount (Long, required, default 0) and AttackVolume (AttackVolume, optional), documented as "Information about the volume of attacks during the time period. If the accompanying AttackCount is zero, this setting might be empty." AttackVolume itself has three optional members (BitsPerSecond, PacketsPerSecond, RequestsPerSecond), all AttackVolumeStatistics. The recording shows what AWS does with the "empty" case for an account with no attacks in the one-year window: HTTP 200 with DataItems[0] = {"AttackCount": 0, "AttackVolume": {}} — the AttackVolume KEY is present and its value is an empty JSON object, i.e. the structure is emitted with none of its three unit members set. "Might be empty" means an empty AttackVolume structure, not an absent member (recorded behavior for the no-unit case; the model only says the setting may be empty).

Emulator: HTTP 200, but DataItems[0] = {"AttackCount": 0} — the AttackVolume key is absent from the item. Replay detail: "$.DataItems[0]: missing (expected {'AttackCount': 0, 'AttackVolume': {}}, got {'AttackCount': 0})". Status, TimeRange and AttackCount all match; only the AttackVolume member diverges.

Why: a client that reads the attack-statistics data item for the volume block gets a null/absent member where AWS always hands it a structure. Code that unconditionally dereferences dataItem.attackVolume (then probes its per-unit members for None) — the shape AWS's own response guarantees — sees a missing optional and either throws or takes an "unknown volume" branch that AWS never produces, so a zero-attack account is reported as "no volume data available" rather than "volume present, all units unreported". The divergence is invisible on an account that did have attacks, so it only bites the quiet-account path that monitoring code hits most often.

Fix: the operation's state projection. services/service-com-amazonaws-shield-awsshield-20160616/src/state.rs, ComAmazonawsShieldAwsshield20160616State::attack_statistics_in, gates the member on whether any unit was reported (`attack_volume: reported_any_unit.then_some(volume)`), collapsing the all-units-unset AttackVolume to None so the serializer omits the key. The data item should carry the AttackVolume structure unconditionally, with each of its three unit members set only when a counted attack reported that unit — the per-unit `peak(..)` logic above it is already correct; only the outer Option is wrong. The handler (src/ops/describe_attack_statistics.rs) passes the vector through unchanged and needs no edit.

Traces: shield-attacks-and-tags-read-paths#task#3 (DescribeAttackStatistics, expected 200 / actual 200, body mismatch at $.DataItems[0]). Code: services/service-com-amazonaws-shield-awsshield-20160616/src/state.rs:607-619; services/service-com-amazonaws-shield-awsshield-20160616/src/ops/describe_attack_statistics.rs:135-145. No open shield issue covers this operation: the five other shield divergences in this replay are owned by 20261030T110306Z-shield-protection-and-drt-error-paths-report-the.md and 20261030T110306Z-shield-createprotectiongroup-invalidparameterexception-omits-reason-and-fields.md, both of which concern error bodies on protection/DRT paths, not the DescribeAttackStatistics success body.

## Evidence
Fixture shield-attacks-and-tags-read-paths, step task#3, operation DescribeAttackStatistics.

Expected (recorded AWS): HTTP 200, DataItems[0] == {"AttackCount": 0, "AttackVolume": {}}.
Actual (emulator):       HTTP 200, DataItems[0] == {"AttackCount": 0}.

Replay divergence detail, verbatim:
  $.DataItems[0]: missing (expected {'AttackCount': 0, 'AttackVolume': {}}, got {'AttackCount': 0})
  divergence_key: service=shield|DescribeAttackStatistics|status=200->200|keys=$.DataItems[0]

Fixed vs variable: the JSON member name "AttackVolume" and the fact that it is present are fixed requirements; its contents are variable — each of BitsPerSecond/PacketsPerSecond/RequestsPerSecond appears only when some counted attack in the window reported that unit, and its Max is that peak from state. AttackCount (0 here) and TimeRange's From/To instants are likewise values derived from the window and the stored attack log, not literals of this recording.

Smithy consulted: operation DescribeAttackStatistics (output DataItems: AttackStatisticsDataList required, TimeRange required; only error InternalErrorException/500); shapes AttackStatisticsDataItem (AttackCount Long required default 0; AttackVolume optional, "If the accompanying AttackCount is zero, this setting might be empty"), AttackVolume (BitsPerSecond/PacketsPerSecond/RequestsPerSecond, all optional AttackVolumeStatistics), AttackVolumeStatistics (Max Double required default 0).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture shield-attacks-and-tags-read-paths.
2. At step task#3 the fixture calls shield DescribeAttackStatistics (POST /, X-Amz-Target AWSShield_20160616.DescribeAttackStatistics, empty JSON body {}) against an account whose attack log holds no attack inside the one-year reporting window.
3. Read the 200 response body and inspect DataItems[0]: the emulator returns {"AttackCount": 0} with no AttackVolume key, while the recording returns {"AttackCount": 0, "AttackVolume": {}}.
4. Equivalently, assert that the AttackVolume member is present on every element of DataItems regardless of AttackCount; the assertion fails on the emulator.
