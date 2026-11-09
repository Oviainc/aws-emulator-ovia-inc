Issue-Source: trace-sweep-divergence
Issue-Trace: wickr-network-create-and-validation
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wickr-network-create-and-validation wickr-network-lifecycle-and-not-found-errors
Issue-Shape-Id: com.amazonaws.wickr#WickrAdminApi
Issue-Key: wickr|GetNetwork|2xx->2xx|$.migrationState
Issue-Key: wickr|GetNetwork|2xx->2xx|$.standing
Issue-Key: wickr|GetNetwork|2xx->2xx|$.tags
Issue-Key: wickr|GetNetwork|2xx->2xx|$headers.content-type
Issue-Key: wickr|GetNetwork|2xx->2xx|$headers.etag
Issue-Key: wickr|ListNetworks|2xx->2xx|$.networks[*]
Issue-Key: wickr|ListNetworks|2xx->2xx|$headers.content-type
Issue-Key: wickr|ListNetworks|2xx->2xx|$headers.etag
Issue-Commit: 22941ee6d43df66d29bcadd7a62b7906e5868e84

# wickr: GetNetwork and ListNetworks network records drop tags and report wrong standing/migrationState

> _Note: this issue was identified against the code base at commit `22941ee6d43df66d29bcadd7a62b7906e5868e84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetwork

## Description
Four net-new causes in the Wickr network READ path. GetNetwork and ListNetworks serve the same stored record (services/service-com-amazonaws-wickr-wickradminapi/src/state.rs NetworkRecord), so every cause below shows up on both operations. A fixer must close all FOUR before submitting.

Cause 1 of 4: the network record omits the `tags` member.
  AWS: recorded behavior only — neither GetNetworkResponse nor the Network shape models a `tags` member, but the recorded AWS responses for both GetNetwork and ListNetworks carry a `tags` object on the network record whose entries are the network's tags, including the service-set `resourceArn` entry holding the network's own ARN (the same value the record reports as networkArn). The key names are fixed; the ARN value is derived from the account, region and networkId of the request/state, never a literal from one run.
  Emulator: the member is absent entirely ($.tags missing). NetworkRecord (src/state.rs lines 96-106) has no tags field, so neither reader can emit one.
  Why: a client reading the network's tag set gets null and concludes the network is untagged.
  Fix: the network record in the service state plus the GetNetwork and ListNetworks output serializers.

Cause 2 of 4: `standing` is reported as 1 for a live network, where AWS reports 2.
  AWS: Network$standing is documented only as "The current standing or status of the network" with no value list, so this is recorded behavior: a freshly created, usable network reports standing = 2 in both GetNetwork and ListNetworks.
  Emulator: reports 1 for every live network — a single hard-coded constant (src/state.rs line 77, NETWORK_STANDING_GOOD = 1).
  Why: a client that gates on the standing value a good network has treats every emulated network as being in a different standing than AWS reports, so standing-conditional logic takes the wrong branch.
  Fix: the value the state assigns to a live network's standing (src/state.rs), read by both readers.

Cause 3 of 4: `migrationState` is reported as 0, where AWS reports 2.
  AWS: Network$migrationState is documented "The SSO redirect URI migration state ... Values: 0 (not started), 1 (in progress), or 2 (completed)". The recorded responses report 2 (completed) for a newly created network.
  Emulator: reports 0, from the hard-coded MIGRATION_STATE_NOT_STARTED (src/state.rs line 69).
  Why: a client that checks whether the SSO redirect migration is complete before using the network's SSO configuration sees "not started" on every emulated network and will try to run a migration AWS reports as already done.
  Fix: the value the state assigns to migrationState (src/state.rs), read by both readers.

Cause 4 of 4: ListNetworks' network entries omit the members AWS serializes with an explicit null.
  AWS: recorded behavior only. In the recorded ListNetworks response each element of `networks` carries `freeTrialExpiration` and `translationStatus` / `translationIamRoleArn` as explicit JSON nulls when unset (the two translation* members are not in the model's Network shape at all, so they are recorded behavior only).
  Emulator: those keys are simply absent from each element.
  Why: a client that distinguishes "member present and null" from "member absent" — or that is generated against a Network shape carrying those members — reads a differently-shaped record than AWS serves.
  Fix: the ListNetworks output serializer (null-member emission) and the network record's member set.

Traces for all four causes: wickr-network-create-and-validation#task#2 (GetNetwork) and wickr-network-lifecycle-and-not-found-errors#task#3 (ListNetworks; cause 4 is witnessed here).

This issue excludes the service-wide content-type/ETag header divergence that also shows on these steps — that is filed as its own cause in the wickr write-path envelope issue.

## Evidence
wickr-network-create-and-validation#task#2 (GetNetwork, 200): $.tags missing (expected {'resourceArn': '<network ARN>'}, got None); $.standing value_mismatch (expected 2, got 1); $.migrationState value_mismatch (expected 2, got 0). Same cluster also covers wickr-network-lifecycle-and-not-found-errors#task#2.
wickr-network-lifecycle-and-not-found-errors#task#3 (ListNetworks, 200): $.networks[0] missing/mismatched — expected the record with tags, standing 2, migrationState 2 and explicit nulls for freeTrialExpiration/translationStatus/translationIamRoleArn; got a record with no tags, no translation*/freeTrialExpiration keys, standing 1 and migrationState 0.

## Steps to Reproduce
1. POST /networks to create a network.
2. GET /networks/{networkId} and inspect the body: AWS reports standing 2, migrationState 2 and a tags object containing resourceArn = the network ARN; the emulator reports standing 1, migrationState 0 and no tags.
3. GET /networks and inspect networks[0]: AWS additionally serializes freeTrialExpiration, translationStatus and translationIamRoleArn as explicit nulls; the emulator omits those keys.
4. Replay fixtures wickr-network-create-and-validation (task#2) and wickr-network-lifecycle-and-not-found-errors (task#3).
