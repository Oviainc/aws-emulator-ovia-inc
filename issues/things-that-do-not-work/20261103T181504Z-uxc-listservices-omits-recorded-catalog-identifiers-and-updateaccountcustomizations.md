Issue-Source: trace-sweep-divergence
Issue-Trace: uxc-catalog-list-services
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: uxc-catalog-list-services uxc-partial-update-preserves-other-fields
Issue-Shape-Id: com.amazonaws.uxc#AWSAccountUXSetting
Issue-Key: uxc|ListServices|2xx->2xx|$.services[*]
Issue-Key: uxc|UpdateAccountCustomizations|2xx->2xx|$.visibleServices[*]
Issue-Commit: d4f8ce799a785ffcb655def193d42db12684ea89

# uxc ListServices omits recorded catalog identifiers and UpdateAccountCustomizations resorts a preserved list echo

> _Note: this issue was identified against the code base at commit `d4f8ce799a785ffcb655def193d42db12684ea89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
Two net-new root causes in the uxc (AWSAccountUXSetting, restJson1) data plane, both 200-status bodies whose ARRAY CONTENT diverges from the recorded service: the membership of the service-identifier catalog ListServices publishes, and the member order UpdateAccountCustomizations echoes a preserved list setting in. The envelope, status, paging and error channels of both operations replay clean (34 of 36 corpus steps pass), so the divergence is purely in the values the service composes.

Cause 1 of 2 - ListServices publishes a service-identifier catalog that OMITS at least 51 identifiers the recorded catalog enumerates (fixture uxc-catalog-list-services, step task#1).
AWS: ListServices "Returns a paginated list of Amazon Web Services service identifiers that you can use as values for the visibleServices setting in UpdateAccountCustomizations. The available services vary by Amazon Web Services partition. Use pagination to retrieve all results." (Smithy: ListServices, GET /v1/services, output ListServicesOutput.services -> ServiceList @length(max 500), @paginated inputToken/outputToken=nextToken, items=services; member doc "The list of available Amazon Web Services service identifiers"). The model fixes no membership, so the catalog's CONTENT is recorded behavior only - and the recording NAMES the missing entries: the replay's per-index detail reports, for each diverging position, the identifier the recorded page carries there. The recorded page is ascending and carries, among others, airflow, comprehend, comprehendmedical, connect-health, datatransferterminal, deadline, ds, elasticfilesystem, elasticmapreduce, elemental-appliances-software, elemental-inference, end-user-messaging, es, finops-agent, flink, qdeveloper, quick, repostspace, rosa, rtbfabric, scn, security-ir, securityagent, securityhubcspm, servicediscovery, settings, snowball, sqlworkbench, ssm and sso, plus further entries at the indices listed under Evidence.
Emulator: ListServices answers 200 with the hand-seeded catalog in services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs (const SEED_SERVICE_CATALOG, sorted and de-duplicated in ComAmazonawsUxcAwsaccountuxsettingState::default). 51 positions of the recorded page diverge, each with verdict `missing`: the recorded identifier is absent from the served array anywhere, and the served array carries the next entry of the emulator's own seed at that index instead (at $.services[5] the recorded 'airflow' is absent and 'amazon-mq' sits there; 'ds' absent where 'docdb' sits; 'elasticfilesystem'/'elasticmapreduce' absent where 'elasticache'/'elasticbeanstalk' sit; 'deadline' absent where 'deadline-cloud' sits; 'ssm'/'sso' absent where 'supplychain'/'sup...' sit).
Why: this is the discovery call a console client uses to learn the legal values of visibleServices, and in this emulator the same catalog is also the ADMISSION list - src/ops/update_account_customizations.rs refuses any visibleServices member that fails state.is_catalog_service with ValidationException (HTTP 400, message "Invalid services provided"). So every recorded identifier the catalog omits is a value a client discovers on real AWS and is then REFUSED when it sets it here, and index-sensitive clients (page offsets, diffing a cached catalog) see a catalog of the wrong size at the wrong offsets.
Fix: the catalog seed and its membership rule in the ListServices state component (SEED_SERVICE_CATALOG in src/state.rs) - add the recorded identifiers the replay names so the served page agrees element-for-element, keeping the list ascending and de-duplicated so paging and the binary-search admission check stay valid. The paging/token code in src/ops/list_services.rs replays clean and needs no change.
Note for the fixer: the RESOLVED issue 20261029T184328Z-uxc-response-bodies-diverge-from-the-recorded-service covered this surface and DECLINED the residue on the grounds that "the replay reports the positions of the remaining divergences and withholds their values". That is not the case in this replay: every diverging position comes with the recorded identifier ("expected 'airflow', got 'amazon-mq'"), so the membership is readable off the recording rather than guessed. A resolved issue covers nothing, and this cause is live: the replay reports stub_path empty and owned_by empty for it.

Cause 2 of 2 - UpdateAccountCustomizations reports a PRESERVED visibleServices setting in ascending order where the recorded service reports it in the other order (fixture uxc-partial-update-preserves-other-fields, step task#2).
AWS: UpdateAccountCustomizations (Smithy: PATCH /v1/account-customizations, @idempotent, output UpdateAccountCustomizationsOutput.visibleServices -> ServiceList @length(max 500), no @uniqueItems and no sort trait) documents the output member only as "The current list of visible service identifiers after the update", and the input member as "Only the settings that you include in the request body are modified. Omitted settings remain unchanged." The order is therefore recorded behavior only, and the recording for this step is an HTTP 200 body reporting the two configured identifiers as ['s3','ec2'] - i.e. NOT ascending - while the GetAccountCustomizations of the same fixture (task#4, no write in between) reports the same pair ascending and replays clean. Hypothesis: the update's echo renders the record in the sequence it is stored/updated in, while the read path reports the setting sorted, so the two paths differ for one stored value.
Emulator: answers 200 with $.visibleServices[0]='ec2' and [1]='s3'. src/state.rs::canonical_members does `supplied.sort()` ON THE WRITE (AccountUxCustomization::set_visible_services, called from src/ops/update_account_customizations.rs), and the handler then echoes the record read back out of state, so the echo can only ever be ascending for any stored value.
Why: a client that applies an update and renders or diffs the echoed visibleServices sees its members in a different sequence than AWS returns, so a request-against-echo or echo-against-cache comparison that holds on AWS fails here; and because the sequence is collapsed at the WRITE, no later call can recover it.
Fix: the UpdateAccountCustomizations handler plus the list-setting write in src/state.rs - the update echo must report the sequence the recorded service reports, while the GetAccountCustomizations order stays as it is (that step replays clean and must keep passing).
Note for the fixer: the same resolved issue DECLINED this as per-response variation after measuring three rules (ascending everywhere; persist-and-echo the supplied sequence; the reverse of both). The fourth shape - preserve the supplied sequence IN STATE and sort only in the GetAccountCustomizations serializer, so the echo and the read legitimately differ for one stored value - was not among them, and it is the only one of the four that can satisfy both recorded bodies of this fixture. Measure that one before declining again.

Neither cause is owned: the replay reports empty stub_path and empty owned_by for both clusters. The one OPEN uxc fidelity issue, 20261029T192225Z-uxc-authorizes-every-sigv4-caller-updateaccountcustomizations-succeeds-for, is about authorization and carries no replay key for either step; 20261027T134113Z-uxc-updateaccountcustomizations-maintainability is about duplicated front-door helpers.

## Evidence
Replay of trace set simparity-wire-aws-v1: emulator_up=true, bootstrap_signal=false, 9 fixtures replayed, 36 steps total, 34 passed, 2 mismatched, 0 errored, 0 blocked, score_pct 94.4. Both mismatches are service 'uxc', verdict 'mismatch', stub_path empty, owned_by empty.

Cause 1 - fixture uxc-catalog-list-services, step task#1, ListServices, expected_status 200, actual_status 200, affected_count 1 (representative uxc-catalog-list-services#task#1). Divergence detail, verbatim excerpt (index: verdict (expected vs actual)):
  $.services[5]: missing (expected 'airflow', got 'amazon-mq')
  $.services[55]: missing (expected 'comprehend', got 'codeguru')
  $.services[56]: missing (expected 'comprehendmedical', got 'codepipeline')
  $.services[60]: missing (expected 'connect-health', got 'connect')
  $.services[65]: missing (expected 'datatransferterminal', got 'datasync')
  $.services[67]: missing (expected 'deadline', got 'deadline-cloud')
  $.services[75]: missing (expected 'ds', got 'docdb')
  $.services[84]: missing (expected 'elasticfilesystem', got 'elasticache')
  $.services[85]: missing (expected 'elasticmapreduce', got 'elasticbeanstalk')
  $.services[86]: missing (expected 'elemental-appliances-software', got 'elastictranscoder')
  $.services[87]: missing (expected 'elemental-inference', got 'emr')
  $.services[88]: missing (expected 'end-user-messaging', got 'entityresolution')
  $.services[90]: missing (expected 'es', got 'evs')
  $.services[93]: missing (expected 'finops-agent', got 'fis')
  $.services[96]: missing (expected 'flink', got 'frauddetector')
  $.services[167]: missing (expected 'qdeveloper', got 'qbusiness')
  $.services[168]: missing (expected 'quick', got 'qldb')
  $.services[174]: missing (expected 'repostspace', got 'redshift-serverless')
  $.services[178]: missing (expected 'rosa', got 'resource-groups')
  $.services[180]: missing (expected 'rtbfabric', got 'rolesanywhere')
  $.services[183]: missing (expected 'scn', got 's3')
  $.services[185]: missing (expected 'security-ir', got 'scheduler')
  $.services[186]: missing (expected 'securityagent', got 'secretsmanager')
  $.services[188]: missing (expected 'securityhubcspm', got 'securitylake')
  $.services[192]: missing (expected 'servicediscovery', got 'ses')
  $.services[195]: missing (expected 'settings', got 'simspaceweaver')
  $.services[197]: missing (expected 'snowball', got 'sns')
  $.services[199]: missing (expected 'sqlworkbench', got 'states')
  $.services[201]: missing (expected 'ssm', got 'supplychain')
  $.services[202]: missing (expected 'sso', got 'sup...   [detail truncated by the replay here]
The cluster's divergence_key enumerates all 51 diverging indices: $.services[5], [30], [34], [42], [55], [56], [60], [64], [65], [67], [68], [73], [75], [82], [84], [85], [86], [87], [88], [90], [93], [94], [96], [97], [99], [167], [168], [169], [174], [178], [179], [180], [182], [183], [185], [186], [188], [192], [193], [195], [196], [197], [199], [201], [202], [204], [206], [212], [213], [214], [222] (key: service=uxc|ListServices|status=200->200|keys=...). Re-running the replay re-prints the recorded identifier for each of them.
Emulator source of the served set: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs lines 214-438 (const SEED_SERVICE_CATALOG) and lines 451-464 (Default, which sorts and de-duplicates it).

Cause 2 - fixture uxc-partial-update-preserves-other-fields, step task#2, UpdateAccountCustomizations, expected_status 200, actual_status 200, affected_count 1 (representative uxc-partial-update-preserves-other-fields#task#2). Divergence detail, verbatim:
  $.visibleServices[0]: value_mismatch (expected 's3', got 'ec2')
  $.visibleServices[1]: value_mismatch (expected 'ec2', got 's3')
Key: service=uxc|UpdateAccountCustomizations|status=200->200|keys=$.visibleServices[0],$.visibleServices[1].
Emulator source: src/state.rs lines 120-123 (canonical_members, `supplied.sort()`) reached from AccountUxCustomization::set_visible_services (lines 28-30), written by src/ops/update_account_customizations.rs lines 277-292 and echoed from the stored record at lines 295-303. The same file's doc comment (state.rs lines 103-119) records that this exact step is the one recorded response the sort-on-write rule does not reproduce.

## Steps to Reproduce
1. git_clone() the repo and build/boot the emulator from that checkout (the uxc service is stood up: its bootstrap issue 20261021T215303Z-bootstrap-service-com-amazonaws-uxc-awsaccountuxsetting is resolved, and get_bootstrap_status() lists service-com-amazonaws-uxc-awsaccountuxsetting under bootstrapped; only athena and iot are pending).
2. Call replay_wire_corpus() on trace set simparity-wire-aws-v1. Line 1 reports 36 steps / 34 passed / 2 mismatched / score 94.4; the two divergence lines are the ones below.
3. Cause 1: replay fixture uxc-catalog-list-services, step task#1 - GET /v1/services (ListServices) with the fixture's maxResults - and compare the returned $.services array element-by-element with the recorded page. It diverges at 51 indices starting at $.services[5], where the recorded 'airflow' is absent and the emulator serves 'amazon-mq'; each diverging line names the recorded identifier that is missing.
4. Cause 2: replay fixture uxc-partial-update-preserves-other-fields, step task#2 - PATCH /v1/account-customizations (UpdateAccountCustomizations), the partial update that does not itself supply visibleServices. The recorded 200 body reports visibleServices as ['s3','ec2']; the emulator answers ['ec2','s3'] because the members were sorted when they were written. Step task#4 (GetAccountCustomizations) of the same fixture passes today and must still pass after the fix.
