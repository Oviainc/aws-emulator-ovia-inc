Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password opensearch.domain-lifecycle-create-active-delete opensearch.cluster-settings-flat-form-only-and-auto-create-index-false-refuses-implicit-index opensearch.nodes-apis-redact-service-internals-while-reporting-the-configured-topology opensearch.ingest-pipeline-rewrites-stored-source-and-simulate-predicts-it opensearch.ism-policy-transition-sets-read-only-and-unmet-condition-stays-writable opensearch.fgac-document-level-security-hides-document-from-results-and-get opensearch.fgac-field-level-security-hides-field-from-hit-and-from-query opensearch.fgac-role-mapping-denies-unmapped-index-for-internal-user opensearch.osm-fgac-role-pattern-read-outside-pattern-denied opensearch.upgrade-domain-server-managed-progress-tracking
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password|run#6|AssertionError|assert cc == {"InstanceType": "t3.small.search", "InstanceCount": 1, "ZoneAwarenessEnabled": False,
Issue-Key: opensearch|CreateDomain|ResourceAlreadyExistsException
Issue-Key: opensearch.cluster-settings-flat-form-only-and-auto-create-index-false-refuses-implicit-index|run#9|AssertionError|assert status == 200, (
Issue-Key: opensearch.nodes-apis-redact-service-internals-while-reporting-the-configured-topology|run#9|KeyError|os_section = node["os"]
Issue-Key: opensearch.ingest-pipeline-rewrites-stored-source-and-simulate-predicts-it|run#7|AssertionError|assert status == 200, f"creating pipeline {pipeline!r} returned HTTP {status!r}: {put!r}"
Issue-Key: opensearch.ism-policy-transition-sets-read-only-and-unmet-condition-stays-writable|run#17|AssertionError|assert locked_action.get("retry") == _DEFAULT_ACTION_RETRY, (
Issue-Key: opensearch.fgac-document-level-security-hides-document-from-results-and-get|_wait_admin_auth#6|Timeout|time.sleep(2)
Issue-Key: opensearch.fgac-field-level-security-hides-field-from-hit-and-from-query|_wait_admin_auth#6|Timeout|time.sleep(2)
Issue-Key: opensearch.fgac-role-mapping-denies-unmapped-index-for-internal-user|_wait_admin_auth#7|Timeout|time.sleep(2)
Issue-Key: opensearch.osm-fgac-role-pattern-read-outside-pattern-denied|_wait_admin_auth#6|Timeout|time.sleep(2)
Issue-Key: opensearch.upgrade-domain-server-managed-progress-tracking|_wait_domain_active#8|Timeout|time.sleep(interval_s)
Issue-Commit: 87f5308bc0aabe3f00012e198d39bc9d2e6e6ae4

# opensearch: eight more divergences — the FGAC security-plugin data plane never answers (4 checks hang), _nodes omits the os section, _cat/indices/{index} and the convert ingest processor are unserved, ISM policies are stored verbatim, DescribeDomain omits DedicatedMasterEnabled, repeat CreateDomain is not idempotent, and UpgradeDomain progress never advances (8 causes)

> _Note: this issue was identified against the code base at commit `87f5308bc0aabe3f00012e198d39bc9d2e6e6ae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
Eight net-new root causes beyond the already-filed data-plane issues (20261029T182036Z-opensearch-the-domain-rest-data-plane-answers-404.md and 20261101T140814Z-opensearch-the-domain-rest-data-plane-serves-only.md): each of the checks below fails on a response those issues' replay keys do not cover. Each cause has its own failing check ids.

Cause 1 of 8 — DescribeDomain's ClusterConfig omits DedicatedMasterEnabled.
AWS: Smithy: DomainStatus$ClusterConfig is a ClusterConfig whose DedicatedMasterEnabled member is documented as "Indicates whether dedicated master nodes are enabled for the cluster. True if the cluster will use a dedicated master node. False if the cluster will not." For a domain created with a single data node and no dedicated masters, DescribeDomain reports ClusterConfig with InstanceType, InstanceCount, ZoneAwarenessEnabled=false AND DedicatedMasterEnabled=false — the flag is always materialised, not only when true.
Emulator: ClusterConfig reads {'InstanceType': 't3.small.search', 'InstanceCount': 1, 'ZoneAwarenessEnabled': False, 'MultiAZWithStandbyEnabled': False} — DedicatedMasterEnabled is absent entirely (it reads as None), while an unrequested MultiAZWithStandbyEnabled is emitted.
Why: a client that branches on DedicatedMasterEnabled to decide whether a domain is highly available (or that renders a config diff) sees a missing key instead of an explicit false, and strongly typed SDK consumers get an unset optional where AWS always sets one.
Fix: the DomainStatus/ClusterConfig serializer and the defaults applied when a domain record is created, in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/describe_domain.rs and the crate's domain state.
Traces: opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password

Cause 2 of 8 — a repeat CreateDomain for an existing domain is refused instead of resolving to the same domain.
AWS: Smithy: CreateDomain declares ResourceAlreadyExistsException (client, httpError 409), but the recorded behaviour for a repeat CreateDomain with the SAME DomainName and the SAME configuration (EngineVersion OpenSearch_2.11, ClusterConfig {t3.small.search, 1}, EBSOptions gp3/10) is a 200 whose DomainStatus.DomainName is that domain, and ListDomainNames still shows exactly one entry with that name — the retried create resolves to the existing resource rather than forking or failing. Recorded behavior only.
Emulator: the repeat call answers ResourceAlreadyExistsException "Domain already exists: <domainName>" (recorded: sim2real-behav-os-46ea61fca7), aborting the caller.
Why: every SDK retry, CloudFormation re-drive and idempotent provisioning loop that re-issues CreateDomain after a timed-out or retried first attempt fails hard in the emulator where AWS lets it converge; the whole domain lifecycle after that point is unreachable.
Fix: the existing-domain branch of the CreateDomain handler in .../src/ops/create_domain.rs — resolve a repeat create of an identical configuration to the existing domain's DomainStatus instead of refusing.
Traces: opensearch.domain-lifecycle-create-active-delete

Cause 3 of 8 — GET /_cat/indices/{index} is unrouted on the domain data plane.
AWS: the managed service serves the _cat APIs; GET /_cat/indices/<index>?format=json answers HTTP 200 with a one-element JSON array describing that index (health, status, index, pri, rep, docs.count, ...). Recorded behavior only for the emulator's wording.
Emulator: HTTP 400 {'error': 'no handler found for uri [/_cat/indices/<index>] and method [GET]'}. Fixed text: "no handler found for uri [", "] and method ["; variable parts: the request URI (carrying the caller's index name) and the method.
Why: the per-index form of _cat/indices is how a caller confirms that an implicitly auto-created index exists and what its shard/doc counts are; a 400 with no engine error.type also breaks clients that branch on it. Open issue 20261101T140814Z-...-serves-only.md covers the bare /_cat/indices and /_cat/nodes routes but not this per-index form, so this check is not covered by it.
Fix: the domain REST data-plane router in .../src/service/domain_rest.rs — route /_cat/indices/{index} to the same listing handler, filtered to the named index.
Traces: opensearch.cluster-settings-flat-form-only-and-auto-create-index-false-refuses-implicit-index

Cause 4 of 8 — GET /_nodes answers without the per-node os section.
AWS: Amazon OpenSearch Service serves GET /_nodes with one entry per node under "nodes", each carrying an "os" object that reports os.available_processors (a positive integer) while REDACTING os.name, os.version and os.pretty_name (https://docs.aws.amazon.com/opensearch-service/latest/developerguide/supported-operations.html). GET /_nodes/stats/fs likewise reports fs.total.total_in_bytes for the volume with the data path redacted.
Emulator: the response's node objects have no "os" key at all — the check reading node["os"] raises KeyError: 'os'.
Why: a client that sizes thread pools or validates its topology from os.available_processors gets nothing, and "redaction" is indistinguishable from "the section does not exist" — the managed-service shape is not reproduced.
Fix: the _nodes (and _nodes/stats/fs) handler in the data-plane router in .../src/service/domain_rest.rs — emit a populated, redacted os section derived from the domain's ClusterConfig instance type/count.
Traces: opensearch.nodes-apis-redact-service-internals-while-reporting-the-configured-topology

Cause 5 of 8 — the ingest-pipeline API rejects the documented `convert` processor.
AWS: the engine's ingest API accepts PUT /_ingest/pipeline/<name> with a processors list including the built-in `convert` processor (field/type/target_field) and answers HTTP 200 {"acknowledged":true}; a subsequent index request through that pipeline rewrites the stored _source, and POST /_ingest/pipeline/<name>/_simulate predicts the same document. Recorded behavior only for the emulator's wording.
Emulator: the PUT answers HTTP 400 {'error': {'root_cause': [{'type': 'parse_exception', 'reason': 'No processor type exists with name [convert]'}], 'type': 'parse_exception', 'reason': 'No processor type exists with name [convert]'}, 'status': 400} — the processor type is not implemented, so the pipeline cannot be created at all.
Why: ingest pipelines are the supported way to normalise documents at write time; a caller cannot register a standard pipeline, and the refusal claims a documented built-in processor does not exist.
Fix: the ingest-pipeline processor registry used by the data plane in .../src/service/domain_rest.rs (pipeline definition parsing and the per-processor executors), including the matching _simulate path.
Traces: opensearch.ingest-pipeline-rewrites-stored-source-and-simulate-predicts-it

Cause 6 of 8 — an ISM policy is stored verbatim: the engine's default action retry is never injected.
AWS: PUT /_plugins/_ism/policies/<name> stores the policy and the ISM plugin NORMALISES it — every action in every state gets the default retry block the engine injects, {"count": 3, "backoff": "exponential", "delay": "1m"}, visible when the policy is read back; the policy then actually transitions the managed index (read_only applied when its condition is met, left writable when it is not). Recorded behavior only for the stored shape.
Emulator: the policy reads back exactly as submitted — the stored read_only action carries retry None — so a policy stored verbatim has no retry semantics at all, and the state machine it describes is not driven.
Why: a client that reads back an ISM policy to confirm the engine accepted and normalised it sees a document AWS never returns, and index-state management (the documented way to roll over/shrink/freeze indexes) has no observable effect.
Fix: the ISM policy handler in the data plane in .../src/service/domain_rest.rs — apply the engine's action defaults on write and drive the policy's state transitions from the managed-index state.
Traces: opensearch.ism-policy-transition-sets-read-only-and-unmet-condition-stays-writable

Cause 7 of 8 — the FGAC security-plugin data plane never answers, so four fine-grained-access-control behaviours never reach a verdict.
AWS: on a domain created with AdvancedSecurityOptions {Enabled:true, InternalUserDatabaseEnabled:true, MasterUserOptions:{MasterUserName,MasterUserPassword}}, the published endpoint accepts HTTP basic auth for the master user and serves the security plugin REST APIs — PUT /_plugins/_security/api/roles/<role> (with index_permissions, allowed_actions, dls, fls), PUT /_plugins/_security/api/internalusers/<user>, PUT /_plugins/_security/api/rolesmapping/<role> — each answering 200/201; afterwards a mapped user searching an index outside its role is refused HTTP 403 with error.type security_exception naming the missing permission ("no permissions for [indices:data/read/search]") while the admin user still reads it 200, and document-level/field-level security hide the document and the field from both _search and _doc GET.
Emulator: none of these four checks ever gets an answer — each exhausts the harness budget ("Check timed out after 60.0s") while polling for the master user's basic-auth login and for a just-written security configuration to take effect, so no 200/201 and no 403 security_exception is ever produced. Hypothesis: the domain data plane implements neither basic auth for the internal user database nor the /_plugins/_security/api/* routes, so the bounded polls never see a success.
Why: fine-grained access control is the service's authorization model; a client cannot create a role, an internal user or a role mapping, and no document-level, field-level or index-level denial is observable — code that relies on FGAC refusals passes vacuously or hangs instead of being told anything.
Fix: the domain REST data plane in .../src/service/domain_rest.rs — HTTP-basic authentication against the internal user database, the /_plugins/_security/api/{roles,internalusers,rolesmapping} routes backed by the domain's security state, and the per-request authorization gate that must answer 403 security_exception.
Traces: opensearch.fgac-document-level-security-hides-document-from-results-and-get, opensearch.fgac-field-level-security-hides-field-from-hit-and-from-query, opensearch.fgac-role-mapping-denies-unmapped-index-for-internal-user, opensearch.osm-fgac-role-pattern-read-outside-pattern-denied

Cause 8 of 8 — an UpgradeDomain that was accepted never advances: GetUpgradeStatus never reaches a terminal UPGRADE step.
AWS: Smithy: UpgradeDomain (POST /2021-01-01/opensearch/upgradeDomain) starts a service-managed engine upgrade; progress is observable purely through GetUpgradeStatus (UpgradeStep reaching UPGRADE with StepStatus SUCCEEDED or SUCCEEDED_WITH_ISSUES, plus a non-empty UpgradeName) and GetUpgradeHistory (a newest entry whose UpgradeStatus is terminal with a non-empty StepsList), with no further client call required to advance it; DescribeDomain then reports EngineVersion equal to the requested TargetVersion. Recorded behavior only for the settling itself.
Emulator: after an accepted UpgradeDomain for a compatible OpenSearch target, the GetUpgradeStatus poll never observes UpgradeStep=UPGRADE in a terminal StepStatus and the check exhausts its budget ("Check timed out after 60.0s") — nothing advances the upgrade server-side.
Why: a caller that waits for its upgrade to finish (the only supported way to know) waits forever, and the domain's EngineVersion never changes, so an upgrade appears accepted but is never performed.
Fix: the upgrade state machine behind UpgradeDomain/GetUpgradeStatus/GetUpgradeHistory in .../src/ops/upgrade_domain.rs and the crate's upgrade state — advance the accepted upgrade to a terminal step without a client-driven call and apply the new EngineVersion to the domain record.
Traces: opensearch.upgrade-domain-server-managed-progress-tracking

## Evidence
Cause 1: expected ClusterConfig {'InstanceType': 't3.small.search', 'InstanceCount': 1, 'ZoneAwarenessEnabled': False, 'DedicatedMasterEnabled': False}; got {'InstanceType': 't3.small.search', 'InstanceCount': 1, 'ZoneAwarenessEnabled': False, 'MultiAZWithStandbyEnabled': False} (DedicatedMasterEnabled absent).
Cause 2: a second CreateDomain with the same name and identical config — expected a 200 whose DomainStatus.DomainName is that domain (ListDomainNames showing exactly one such entry); got botocore.errorfactory.ResourceAlreadyExistsException: An error occurred (ResourceAlreadyExistsException) when calling the CreateDomain operation: Domain already exists: sim2real-behav-os-46ea61fca7 (the name is a per-run value).
Cause 3: expected HTTP 200 and a one-element JSON array from GET /_cat/indices/<index>; got HTTP 400 {'error': "no handler found for uri [/_cat/indices/sim2real-behav-autocreate-on] and method [GET]"}.
Cause 4: expected each entry of GET /_nodes to carry an os object with a positive available_processors and without name/version/pretty_name; got node objects with no 'os' key — KeyError: 'os'.
Cause 5: expected HTTP 200 {'acknowledged': true} for PUT /_ingest/pipeline/<name> with a convert processor; got HTTP 400 {'error': {'root_cause': [{'type': 'parse_exception', 'reason': 'No processor type exists with name [convert]'}], 'type': 'parse_exception', 'reason': 'No processor type exists with name [convert]'}, 'status': 400}.
Cause 6: expected the stored read_only action to carry the engine-injected default retry {'count': 3, 'backoff': 'exponential', 'delay': '1m'}; got retry None ('a policy stored verbatim has no retry semantics at all').
Cause 7: four checks produce no response at all — 'Check timed out after 60.0s' — while polling for master-user basic auth and for a written /_plugins/_security configuration to take effect; the expected 200/201 on /_plugins/_security/api/{roles,internalusers,rolesmapping} and the expected HTTP 403 security_exception naming 'no permissions for [indices:data/read/search]' are never produced.
Cause 8: after an accepted UpgradeDomain to a compatible OpenSearch target, GetUpgradeStatus never reports UpgradeStep='UPGRADE' with StepStatus in (SUCCEEDED, SUCCEEDED_WITH_ISSUES, FAILED); the check produced no verdict and was cut off at the 60 s cap.

## Steps to Reproduce
1. Cause 1: CreateDomain (OpenSearch_2.11, ClusterConfig {t3.small.search, 1}, EBSOptions gp3/10, NodeToNodeEncryptionOptions enabled, AdvancedSecurityOptions enabled with an internal master user) and read DescribeDomain.DomainStatus.ClusterConfig — DedicatedMasterEnabled is missing.
2. Cause 2: CreateDomain <name>; DescribeDomain it; then CreateDomain again with the same <name> and the same ClusterConfig/EBSOptions/EngineVersion -> ResourceAlreadyExistsException instead of a DomainStatus for that domain.
3. Cause 3: on an active domain, index a document into a not-yet-existing index (auto-create), then SigV4-sign GET <endpoint>/_cat/indices/<index>?format=json -> HTTP 400 'no handler found'.
4. Cause 4: GET <endpoint>/_nodes -> node entries with no 'os' section; also GET <endpoint>/_nodes/stats/fs.
5. Cause 5: PUT <endpoint>/_ingest/pipeline/<name> {"processors":[{"convert":{"field":"<f>","type":"integer"}}]} -> HTTP 400 parse_exception 'No processor type exists with name [convert]'.
6. Cause 6: PUT <endpoint>/_plugins/_ism/policies/<name> with a state whose action is {"read_only":{}}, then GET it back -> the action has no retry block.
7. Cause 7: CreateDomain with AdvancedSecurityOptions {Enabled:true, InternalUserDatabaseEnabled:true, MasterUserOptions:{MasterUserName,MasterUserPassword}}, EncryptionAtRestOptions/NodeToNodeEncryptionOptions enabled and EnforceHTTPS; wait for the endpoint; GET <endpoint>/ with HTTP basic auth as the master user, then PUT <endpoint>/_plugins/_security/api/roles/<role>, /internalusers/<user>, /rolesmapping/<role>; then search an unmapped index as that user -> nothing answers within the budget instead of 200/201 and a 403 security_exception.
8. Cause 8: GetCompatibleVersions, pick an OpenSearch_x.y -> OpenSearch_x.y pair, CreateDomain at the source version, wait until Processing=False, UpgradeDomain(TargetVersion=<target>), then poll GetUpgradeStatus -> UpgradeStep/StepStatus never become terminal and DescribeDomain.EngineVersion never changes.
