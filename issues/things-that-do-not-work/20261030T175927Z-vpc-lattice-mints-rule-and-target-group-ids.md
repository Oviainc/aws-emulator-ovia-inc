Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-target-group-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-target-group-lifecycle vpclattice-listener-rule-lifecycle
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|CreateTargetGroup|2xx->2xx|$.arn
Issue-Key: vpc-lattice|CreateTargetGroup|2xx->2xx|$.id
Issue-Key: vpc-lattice|GetTargetGroup|2xx->2xx|$.arn
Issue-Key: vpc-lattice|CreateTargetGroup|4xx->4xx|$.resourceId
Issue-Key: vpc-lattice|CreateRule|4xx->4xx|$.resourceId
Issue-Commit: aa6eac2cd77e70e1001710733620f31a870d8297

# vpc-lattice mints rule and target-group ids from base36 instead of hex

> _Note: this issue was identified against the code base at commit `aa6eac2cd77e70e1001710733620f31a870d8297`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
BUNDLE OF 4 CAUSES, one root fault, all four must be closed before submitting.

Shared fault: VPC Lattice mints the 17-character suffix of a resource id from the LOWERCASE HEXADECIMAL alphabet [0-9a-f]; the emulator mints it from base36 [0-9a-z]. The wrong alphabet then propagates into every value that embeds the id: the resource's own `id`, its ARN, and the `resourceId` member of ConflictException. This issue covers the TARGET GROUP (tg-) and RULE (rule-) prefixes.

Shared AWS reference: the recorded AWS responses mint hex-only suffixes — recorded target group `tg-061d6752b311751ac` and recorded rule `rule-05dfcacbf071a1a5c`, each 17 characters drawn only from [0-9a-f]. Smithy constrains only the looser outer form (TargetGroupId @length 20..20 @pattern ^tg-[0-9a-z]{17}$; TargetGroupArn @pattern ^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:targetgroup/tg-[0-9a-z]{17}$; RuleId @pattern ^rule-[0-9a-z]{17}$; ConflictException.resourceId -> String, required), so the hex restriction is RECORDED BEHAVIOR ONLY, not modeled — which is why base36 ids still satisfy the model yet diverge on the wire. Minting hex keeps every modeled pattern and length satisfied, so the fix is safe.

Shared Fix: the id minter in the service's state module (the alphabet it draws the 17-character suffix from). `arn_for` and the rule/listener ARN composers need no change once minting is fixed; they are named per cause only because that is where the wrong alphabet becomes visible on the wire.

RELATION TO AN EXISTING OPEN ISSUE: 20261030T043524Z-vpc-lattice-minted-resource-ids-use-a-base36.md reports the same minting fault for the svc-, listener- and snsa- prefixes. It does NOT carry the replay keys of the four clusters below (they are reported with an empty owned_by), so those clusters are not exercised by its replay pointer and are filed here. A single alphabet change in the minter is expected to clear both issues.

Note on how these render: on create paths the harness asserts the SHAPE of a first-seen value (printed as expected '<shaped-not-asserted>') and the shape fails on the non-hex suffix; the substantive difference is the id alphabet, never the region or the account id (000000000000 in both).

Cause 1 of 4 — CreateTargetGroup, fixture vpclattice-target-group-lifecycle step task#1 (HTTP 201 in both).
AWS: returns `id` matching ^tg-<17 hex>$ and `arn` arn:<partition>:vpc-lattice:<region>:<account>:targetgroup/tg-<17 hex> (Smithy: CreateTargetGroup POST /targetgroups, success 201, output id/arn/name/type/config/status). The recording shows tg-061d6752b311751ac.
Emulator: answers 201 but returns a base36 suffix — `$.arn` and `$.id` diverge (observed tg-kgr24ej5gqisv72vd, carrying letters outside [0-9a-f]).
Why: the returned id/ARN is the handle a client stores and later passes to GetTargetGroup/DeleteTargetGroup and embeds in rule actions; a client that validates or pattern-matches it against what AWS emits rejects a value AWS would never produce.
Fix: the state module's id minter (alphabet), consumed by CreateTargetGroup's handler via `mint_id(PREFIX_TARGET_GROUP)` and `arn_for`.
Traces: vpclattice-target-group-lifecycle#task#1; code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/create_target_group.rs:346-349.

Cause 2 of 4 — GetTargetGroup, fixture vpclattice-target-group-lifecycle step task#2 (HTTP 200 in both).
AWS: echoes the stored target group's `arn` exactly as CreateTargetGroup returned it, with the hex id suffix (Smithy: GetTargetGroup GET /targetgroups/{targetGroupIdentifier}, 200).
Emulator: `$.arn` diverges — the stored ARN embeds the base36 suffix minted in cause 1.
Why: the read path hands back an identifier a caller cannot match against anything AWS produced, so client-side id validation and recorded comparisons fail on read as well as on create.
Fix: same minting alphabet; GetTargetGroup's serializer needs no separate change.
Traces: vpclattice-target-group-lifecycle#task#2.

Cause 3 of 4 — CreateTargetGroup duplicate-name conflict, fixture vpclattice-target-group-lifecycle step task#3 (HTTP 409 in both).
AWS: ConflictException (HTTP 409; Smithy: required `message`, `resourceId`, `resourceType`) carries in `resourceId` the id of the CONFLICTING target group — the fixed text comes from the error class while `resourceId` is a value taken from emulator state, namely the existing target group's id, which matches ^tg-<17 hex>$.
Emulator: status, error code and message agree; `$.resourceId` diverges because the stored id is base36.
Why: a client that parses `resourceId` out of a 409 to look the conflicting target group up receives an id of a form AWS never emits.
Fix: same minting alphabet; the conflict-construction path already reports the right member (create_target_group.rs:315-324).
Traces: vpclattice-target-group-lifecycle#task#3.

Cause 4 of 4 — CreateRule duplicate-name conflict, fixture vpclattice-listener-rule-lifecycle step task#8 (HTTP 409 in both).
AWS: ConflictException's `resourceId` is the conflicting RULE's id, matching ^rule-<17 hex>$ (the recording mints rule-05dfcacbf071a1a5c on the create path).
Emulator: status, code and message agree; `$.resourceId` fails its shape assertion on the base36 rule id.
Why: same as cause 3, on the rule namespace — the id a caller extracts from the conflict cannot be matched or validated.
Fix: same minting alphabet, consumed by CreateRule via `mint_id(PREFIX_RULE)`.
Traces: vpclattice-listener-rule-lifecycle#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true; 110 steps, 53 passed, 57 mismatched, 0 errored, 0 blocked; score 48.2%; 22 divergence clusters). Four net-new vpc-lattice mismatch clusters, each with stub_path empty and owned_by empty:

1. CreateTargetGroup vpclattice-target-group-lifecycle#task#1 — expected 201, actual 201. Detail verbatim:
   $.arn: value_mismatch (expected 'arn:aws:vpc-lattice:us-west-2:000000000000:targetgroup/tg-061d6752b311751ac', got 'arn:aws:vpc-lattice:us-west-2:000000000000:targetgroup/tg-kgr24ej5gqisv72vd')
   $.id: value_mismatch (expected 'tg-061d6752b311751ac', got 'tg-kgr24ej5gqisv72vd')
2. GetTargetGroup vpclattice-target-group-lifecycle#task#2 — expected 200, actual 200. Detail verbatim:
   $.arn: value_mismatch (expected 'arn:aws:vpc-lattice:us-west-2:000000000000:targetgroup/tg-061d6752b311751ac', got 'arn:aws:vpc-lattice:us-west-2:000000000000:targetgroup/tg-kgr24ej5gqisv72vd')
3. CreateTargetGroup vpclattice-target-group-lifecycle#task#3 — expected 409, actual 409 (message and resourceType agree). Detail verbatim:
   $.resourceId: value_mismatch (expected 'tg-061d6752b311751ac', got 'tg-kgr24ej5gqisv72vd')
4. CreateRule vpclattice-listener-rule-lifecycle#task#8 — expected 409, actual 409 (message and resourceType agree). Detail verbatim:
   $.resourceId: value_mismatch (expected '<shaped-not-asserted>', got 'rule-birmvby5npf2496f1')

Expected vs actual, stated generally: AWS mints `<prefix><17 characters from [0-9a-f]>` (recorded: tg-061d6752b311751ac, rule-05dfcacbf071a1a5c); the emulator mints `<prefix><17 characters from [0-9a-z]>` (observed: tg-kgr24ej5gqisv72vd, rule-birmvby5npf2496f1 — both containing letters outside the hex range). HTTP statuses, error codes and error messages agree in all four clusters; the divergence is confined to the generated identifiers and the ARNs/resourceIds that embed them. The specific strings above are one run's values and are not themselves the requirement — the alphabet and the composition template are.

Model facts used: Smithy TargetGroupId @length 20..20 @pattern ^tg-[0-9a-z]{17}$; TargetGroupArn @pattern ^arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:targetgroup/tg-[0-9a-z]{17}$; RuleId @length 5..22 @pattern ^rule-[0-9a-z]{17}$; ConflictException{message,resourceId,resourceType} all required, httpError 409; CreateTargetGroup http {POST /targetgroups, 201}; GetTargetGroup http {GET /targetgroups/{targetGroupIdentifier}, 200}; CreateRule http {POST /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, 201}. Every modeled pattern and length stays satisfied under a hex alphabet, so no other member's validation changes.

Code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/create_target_group.rs:346 mints the id via crate::state::mint_id(crate::state::PREFIX_TARGET_GROUP) and line 349 composes the ARN via crate::state::arn_for; the alphabet itself lives in the state module's mint_id.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or drive the fixtures by hand against the vpc-lattice endpoint:
   a. POST /targetgroups with {"name":"sim2real-probe-lat-tgl-<suffix>","type":"LAMBDA","config":{"lambdaEventStructureVersion":"V1"}} (fixture vpclattice-target-group-lifecycle task#1) and inspect `id`/`arn`: the 17-character suffix contains characters outside [0-9a-f], where the recording has tg-061d6752b311751ac.
   b. GET /targetgroups/{id} (task#2): the echoed `arn` carries the same non-hex suffix.
   c. POST /targetgroups again with the SAME name (task#3): the 409 ConflictException's `resourceId` is that non-hex target-group id.
   d. POST /services, POST /services/{svcId}/listeners, then POST /services/{svcId}/listeners/{listenerId}/rules twice with the same rule name (fixture vpclattice-listener-rule-lifecycle task#6 then task#8): the 409 ConflictException's `resourceId` is a rule-<17 non-hex chars> id.
3. Confirm the fix by re-replaying the representative step vpclattice-target-group-lifecycle#task#1 and every step listed in trace_ids; all four clusters must come back clean.
