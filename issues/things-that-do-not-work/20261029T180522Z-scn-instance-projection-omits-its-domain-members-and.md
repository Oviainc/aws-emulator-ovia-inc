Issue-Source: trace-sweep-divergence
Issue-Trace: supplychain-bill-of-materials-import-job
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: supplychain-bill-of-materials-import-job supplychain-datalake-namespace-dataset supplychain-list-instances-pagination supplychain-tagging-workflow supplychain-instance-lifecycle
Issue-Shape-Id: com.amazonaws.supplychain#GalaxyPublicAPIGateway
Issue-Key: scn|CreateInstance|2xx->2xx|$.instance.idcPortalDomain
Issue-Key: scn|CreateInstance|2xx->2xx|$.instance.primaryDomain
Issue-Key: scn|CreateInstance|2xx->2xx|$.instance.versionNumber
Issue-Key: scn|DeleteInstance|2xx->2xx|$.instance.idcPortalDomain
Issue-Key: scn|DeleteInstance|2xx->2xx|$.instance.primaryDomain
Issue-Key: scn|DeleteInstance|2xx->2xx|$.instance.webAppDnsDomain
Issue-Key: scn|CreateInstance|2xx->2xx|$.instance.webAppDnsDomain
Issue-Key: scn|GetInstance|2xx->2xx|$.instance.idcPortalDomain
Issue-Key: scn|GetInstance|2xx->2xx|$.instance.primaryDomain
Issue-Key: scn|GetInstance|2xx->2xx|$.instance.versionNumber
Issue-Key: scn|GetInstance|2xx->2xx|$.instance.webAppDnsDomain
Issue-Key: scn|UpdateInstance|2xx->2xx|$.instance.idcPortalDomain
Issue-Key: scn|UpdateInstance|2xx->2xx|$.instance.primaryDomain
Issue-Key: scn|UpdateInstance|2xx->2xx|$.instance.webAppDnsDomain
Issue-Key: scn|ListInstances|2xx->2xx|$.instances[*]
Issue-Key: scn|DeleteInstance|2xx->2xx|$.instance.versionNumber
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# scn Instance projection omits its domain members and treats versionNumber as a mutation counter

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInstance

## Description
NINE (9) net-new divergence clusters, all on AWS Supply Chain (service token `scn`, sdkId `SupplyChain`, endpointPrefix/sigv4Name `scn`; bootstrap crate `service-com-amazonaws-supplychain-galaxypublicapigateway` is RESOLVED, so this is a fidelity finding, not a bootstrap gap). Every cluster is the SAME `Instance` structure answered by a different instance operation, and three defects in the single projection that builds it account for all nine. A fixer MUST close all 9 causes before submitting.

SHARED ROOT CAUSE (the defect; the nine causes below are its observations)

Defect A - the two domain members do not exist in the projection at all. The real service returns, on every operation whose output member is the `Instance` shape, a `primaryDomain` and an `idcPortalDomain` alongside `webAppDnsDomain`. The on-disk Smithy model's `Instance` structure (com.amazonaws.supplychain#Instance) declares `instanceId`, `awsAccountId`, `state`, `errorMessage`, `webAppDnsDomain`, `createdTime`, `lastModifiedTime`, `instanceName`, `instanceDescription`, `kmsKeyArn`, `versionNumber` and NOTHING else - so for these two members this is RECORDED BEHAVIOR ONLY: the recordings show the service emitting them and the model is behind the service. The recordings also show the rule relating them: both are derived from the instance's WebApp DNS domain - `primaryDomain` and `idcPortalDomain` are each that domain suffixed with the service's WebApp zone (recorded as `<webAppDnsDomain>.scn.global.on.aws`), so the only variable part is the instance's own domain label; the zone suffix is fixed text. The emulator never emits either member under any circumstance.

Defect B - `webAppDnsDomain` IS modelled (`Instance$webAppDnsDomain` -> `InstanceWebAppDnsDomain`, @pattern `^(?![-])[a-zA-Z0-9-]{1,62}[a-zA-Z0-9]$`) and is an accepted `CreateInstance` input member with the same target and pattern, so AWS echoes the domain the create request supplied (and mints one when the request omits it) and every later read of that instance keeps returning it. The emulator accepts the input and then drops it: `InstanceRecord::new` initialises `web_app_dns_domain: None` and nothing in the create path assigns the request's value, so `InstanceRecord::to_wire` projects `webAppDnsDomain` as absent forever.

Defect C - `versionNumber` (`Instance$versionNumber` -> Double, documented only as "The version number of the instance") is, in the recordings, a property OF THE INSTANCE and not a mutation counter: it is reported identically (recorded value 2.0) by CreateInstance, GetInstance, UpdateInstance, ListInstances and DeleteInstance for the same instance, and it does NOT change when UpdateInstance mutates the instance's name or description. The emulator instead uses it as an optimistic-concurrency counter: `INSTANCE_INITIAL_VERSION = 1.0` on create and `InstanceRecord::touch` does `version_number += 1.0` on every mutation, so a freshly created instance reports 1.0 (one below what the service reports) and the number then drifts upward with each write - after one UpdateInstance the emulator reports 3.0 where the service still reports its unchanged instance version. The specific recorded number is one environment's value and is not the requirement; the requirement is that the member is stable across mutations of the same instance and is not an edit count.

Why: a client cannot reach the application. `webAppDnsDomain`, `primaryDomain` and `idcPortalDomain` are the only way a caller learns the hostnames of the instance's web application and its IAM Identity Center portal, so any console-style flow, any DNS/CNAME or Identity-Center wiring step, and any assertion that a created instance is addressable reads `None` and fails or dereferences null. `versionNumber` is what a caller compares to detect that it holds a stale copy of the instance; because the emulator moves it on every write and starts it one below the service's value, a client that pins or compares the version sees spurious conflicts, and a client that round-trips a version it read from a create sees it invalidated by an unrelated update.

Fix: `InstanceRecord::to_wire` in services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/state.rs is the single projection all five instance operations answer from, so all three defects land there and in the record it projects: add the two domain members to the record/projection and derive them from the instance's WebApp DNS domain plus the fixed WebApp zone suffix; carry the `CreateInstance` request's `webAppDnsDomain` into `InstanceRecord` (and mint a conforming label when the request omits it) instead of leaving `InstanceRecord::new`'s `None`; and stop treating `version_number` as an edit counter - remove the bump from `InstanceRecord::touch` and seed the record with the instance's version so it is stable across mutations. ListInstances' ordering (cause 9) belongs in that operation's handler under src/ops/, not in the projection.

Cause 1 of 9: CreateInstance (fixture `supplychain-bill-of-materials-import-job`, step `setup#1`). AWS: 200 with `instance.primaryDomain` and `instance.idcPortalDomain` present (recorded behavior only - see Defect A) and `instance.versionNumber` the instance's own version. Emulator: 200 but `$.instance.idcPortalDomain` and `$.instance.primaryDomain` absent (expected shaped values, got None) and `$.instance.versionNumber` 1.0 where 2.0 was recorded. Defects A + C.

Cause 2 of 9: DeleteInstance (fixture `supplychain-bill-of-materials-import-job`, step `teardown#5`). AWS: 200, and the terminal `Instance` it echoes still carries `webAppDnsDomain` and both derived domains. Emulator: 200 but `$.instance.idcPortalDomain`, `$.instance.primaryDomain` and `$.instance.webAppDnsDomain` all absent. Defects A + B. Same cluster also observed on `supplychain-datalake-namespace-dataset#teardown#15`, `supplychain-list-instances-pagination#teardown#7`, `#teardown#8` and `supplychain-tagging-workflow#teardown#9`.

Cause 3 of 9: CreateInstance with a caller-supplied WebApp domain (fixture `supplychain-datalake-namespace-dataset`, step `setup#1`). AWS: 200 echoing the requested `webAppDnsDomain` plus both derived domains, and the instance's version. Emulator: 200 with `$.instance.webAppDnsDomain`, `$.instance.primaryDomain`, `$.instance.idcPortalDomain` absent and `$.instance.versionNumber` 1.0 vs 2.0. Defects A + B + C. Also observed on `supplychain-instance-lifecycle#task#1`, `supplychain-list-instances-pagination#setup#1`, `#setup#2`, `supplychain-tagging-workflow#setup#1`.

Cause 4 of 9: GetInstance read-back of a created instance (fixture `supplychain-instance-lifecycle`, step `task#2`). AWS: 200, the read reproduces exactly what create returned, domains included. Emulator: 200 with all three domain members absent and `$.instance.versionNumber` 1.0 vs 2.0 - i.e. the read faithfully reproduces the record, which is the thing that is wrong. Defects A + B + C. Also observed on `supplychain-list-instances-pagination#setup#3`, `#setup#4`, `supplychain-tagging-workflow#setup#2`.

Cause 5 of 9: UpdateInstance (fixture `supplychain-instance-lifecycle`, step `task#3`). AWS: 200 with the updated instance, domains unchanged by the update. Emulator: 200 with `$.instance.idcPortalDomain`, `$.instance.primaryDomain`, `$.instance.webAppDnsDomain` absent. Defects A + B.

Cause 6 of 9: GetInstance after an update (fixture `supplychain-instance-lifecycle`, step `task#4`). AWS: 200, domains still present after a mutation. Emulator: 200, same three members absent. Defects A + B.

Cause 7 of 9: ListInstances projection (fixture `supplychain-instance-lifecycle`, step `task#5`). AWS: 200 and each element of `instances` is the SAME `Instance` shape the single-instance operations return, so every listed element carries `webAppDnsDomain`, `primaryDomain` and `idcPortalDomain`. Emulator: `$.instances[0]` diverges because the element is missing all three domain members. Defects A + B - confirming the list path shares the one projection.

Cause 8 of 9: DeleteInstance after an update (fixture `supplychain-instance-lifecycle`, step `teardown#6`). AWS: 200 and `instance.versionNumber` is still the instance's unchanged version. Emulator: 200 with `$.instance.versionNumber` 3.0 where 2.0 was recorded - the counter has now been bumped twice by the preceding writes - plus the three absent domain members. This cause is the direct evidence for Defect C: the member tracks edits in the emulator and does not in the service. Defects A + B + C.

Cause 9 of 9: ListInstances ordering (fixture `supplychain-list-instances-pagination`, step `task#5`). AWS: recorded behavior only - the model's `ListInstances` declares @paginated(items: instances) and `maxResults` (default 10, range 0..20) but states no sort order; the recording shows the two instances returned with the MORE RECENTLY CREATED one first, i.e. `instances[0]` is instance "b" (created second) and `instances[1]` is instance "a". Emulator: returns them in ascending `instanceId` order - `instances[0]` is "a" and `instances[1]` is "b" - because state.rs keys `instances` in a `BTreeMap` by `instanceId` and the handler pages that key order directly. Hypothesis: the service orders the page by creation time descending; the emulator's order is an artefact of its storage key and is unrelated to any property a caller can reason about. The two elements also each carry the Defect A/B absences. Also observed on `supplychain-list-instances-pagination#task#6`. NOTE: the recorded instance ids are per-run values, not requirements - the requirement is the relative order of the two instances by creation.

No open issue covers any scn/SupplyChain divergence (list_existing_issues finds none for `scn` or `supplychain`), so none of these nine causes is owned elsewhere; no cluster carried a `stub_path` or `owned_by`.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=101, passed=14, mismatched=82, blocked=5, divergence_count=11, score_pct=13.9. All 11 clusters are service=scn; the 9 bundled here are every cluster whose diff is on the `Instance` shape.

Cause 1 - supplychain-bill-of-materials-import-job#setup#1, CreateInstance, expected 200 / actual 200:
  $.instance.idcPortalDomain: missing (expected '<shaped-not-asserted>', got None)
  $.instance.primaryDomain: missing (expected '<shaped-not-asserted>', got None)
  $.instance.versionNumber: value_mismatch (expected 2.0, got 1.0)

Cause 2 - supplychain-bill-of-materials-import-job#teardown#5, DeleteInstance, 200/200 (5 steps affected):
  $.instance.idcPortalDomain: missing (expected '{{webapp_domain}}.scn.global.on.aws', got None)
  $.instance.primaryDomain: missing (expected '{{webapp_domain}}.scn.global.on.aws', got None)
  $.instance.webAppDnsDomain: missing (expected '{{webapp_domain}}', got None)

Cause 3 - supplychain-datalake-namespace-dataset#setup#1, CreateInstance, 200/200 (5 steps affected):
  $.instance.idcPortalDomain / $.instance.primaryDomain / $.instance.webAppDnsDomain: missing, got None
  $.instance.versionNumber: value_mismatch (expected 2.0, got 1.0)

Cause 4 - supplychain-instance-lifecycle#task#2, GetInstance, 200/200 (4 steps affected):
  $.instance.idcPortalDomain: missing (expected '{{webapp_domain}}.scn.global.on.aws', got None)
  $.instance.primaryDomain: missing (expected '{{webapp_domain}}.scn.global.on.aws', got None)
  $.instance.webAppDnsDomain: missing (expected '{{webapp_domain}}', got None)
  $.instance.versionNumber: value_mismatch (expected 2.0, got 1.0)

Cause 5 - supplychain-instance-lifecycle#task#3, UpdateInstance, 200/200: idcPortalDomain, primaryDomain, webAppDnsDomain all missing (got None).

Cause 6 - supplychain-instance-lifecycle#task#4, GetInstance, 200/200: same three members missing (got None).

Cause 7 - supplychain-instance-lifecycle#task#5, ListInstances, 200/200:
  $.instances[0]: expected element carries idcPortalDomain '{{webapp_domain}}.scn.global.on.aws', primaryDomain '{{webapp_domain}}.scn.global.on.aws', webAppDnsDomain '{{webapp_domain}}'; actual element is {instanceId, awsAccountId, state, createdTime, lastModifiedTime, instanceName, instanceDescription, versionNumber} with no domain members at all.

Cause 8 - supplychain-instance-lifecycle#teardown#6, DeleteInstance, 200/200:
  $.instance.versionNumber: value_mismatch (expected 2.0, got 3.0)   <-- emulator bumped the counter on each preceding write
  plus idcPortalDomain / primaryDomain / webAppDnsDomain missing.

Cause 9 - supplychain-list-instances-pagination#task#5, ListInstances, 200/200 (2 steps affected):
  $.instances[0]: expected instanceId 'ef5550e8-...' (instance "b", created second); actual instanceId '41159285-...' (instance "a", created first)
  $.instances[1]: expected instanceId '41159285-...' (instance "a"); actual instanceId 'ef5550e8-...' (instance "b")
  i.e. the recorded order is newest-first and the emulator's is ascending instanceId.

Model evidence (Smithy, services/service-com-amazonaws-supplychain-galaxypublicapigateway/smithy-model/com-amazonaws-supplychain-galaxypublicapigateway.json):
- service GalaxyPublicAPIGateway v2024-01-01, restJson1, awsService{sdkId: SupplyChain, endpointPrefix: scn, arnNamespace: scn}, sigv4Name scn.
- Instance$webAppDnsDomain -> InstanceWebAppDnsDomain @pattern ^(?![-])[a-zA-Z0-9-]{1,62}[a-zA-Z0-9]$ (MODELLED, and an accepted CreateInstance input member with the same target/pattern).
- Instance$versionNumber -> Double, doc "The version number of the instance." (no statement that it increments).
- Instance declares NO primaryDomain and NO idcPortalDomain -> recorded behavior only for those two.
- CreateInstance POST /api/instance 200; GetInstance GET /api/instance/{instanceId} 200; UpdateInstance PATCH /api/instance/{instanceId} 200; DeleteInstance DELETE /api/instance/{instanceId} 200; ListInstances GET /api/instance 200 @paginated(items: instances, pageSize: maxResults). All five declare output member `instance`/`instances` targeting the one Instance shape.

Emulator source evidence (services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/state.rs):
- L123-137 InstanceRecord::to_wire builds Instance with exactly 11 members - no primaryDomain, no idcPortalDomain (Defect A).
- L104-120 InstanceRecord::new sets web_app_dns_domain: None and the create path never assigns the request value (Defect B).
- L39 INSTANCE_INITIAL_VERSION: f64 = 1.0 and L141-144 touch() does version_number += 1.0 on every mutation (Defect C).
- L155-157 instances: BTreeMap<String, InstanceRecord> keyed by instanceId, "so ListInstances pages in one stable order" - the source of cause 9's ordering.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo (mainline) and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus). All nine causes below are scn clusters in that output.
3. Causes 1 and 3 (CreateInstance): POST /api/instance with an instanceName, an instanceDescription and a webAppDnsDomain. Inspect the 200 body's `instance` object: `primaryDomain` and `idcPortalDomain` are absent entirely, `webAppDnsDomain` is absent even though the request supplied it, and `versionNumber` is 1.0 where the recording has the instance's own version (2.0). Replay `supplychain-bill-of-materials-import-job#setup#1` (no domain supplied) and `supplychain-datalake-namespace-dataset#setup#1` (domain supplied) to see both variants.
4. Cause 4 and 6 (GetInstance): GET /api/instance/{instanceId} for the instance just created - once immediately (`supplychain-instance-lifecycle#task#2`) and once after an update (`#task#4`). The read reproduces the stored record, so the same three domain members are absent and versionNumber is still wrong.
5. Cause 5 (UpdateInstance): PATCH /api/instance/{instanceId} changing instanceDescription (`supplychain-instance-lifecycle#task#3`). The 200 echo is missing webAppDnsDomain, primaryDomain and idcPortalDomain.
6. Cause 8 (Defect C isolated): after that one PATCH, DELETE /api/instance/{instanceId} (`supplychain-instance-lifecycle#teardown#6`) and read `instance.versionNumber`. The emulator now reports 3.0 - it has incremented once per write via InstanceRecord::touch - while the recording still reports the instance's unchanged version. Repeat the PATCH to watch the number keep climbing; that drift is the defect, not the particular number.
7. Cause 2 (DeleteInstance projection): DELETE /api/instance/{instanceId} on a freshly created instance (`supplychain-bill-of-materials-import-job#teardown#5`). The terminal `instance` echo is missing the same three domain members.
8. Causes 7 and 9 (ListInstances): create two instances in a known order, each with its own webAppDnsDomain, then GET /api/instance (`supplychain-instance-lifecycle#task#5` and `supplychain-list-instances-pagination#task#5`). Each element of `instances` is missing all three domain members (cause 7), and the page is ordered ascending by instanceId - the emulator's BTreeMap key order - instead of returning the more recently created instance first as the recording does (cause 9).
9. Diff each response against the fixture's expected body. Every status already matches (200 throughout), so the divergence is entirely in the `Instance` projection and in ListInstances' element order.
