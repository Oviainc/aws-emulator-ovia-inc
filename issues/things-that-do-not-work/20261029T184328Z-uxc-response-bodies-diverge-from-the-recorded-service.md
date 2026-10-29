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
Issue-Commit: f20bf72de740b5a2b8804db1d8ff681c13106e79

# uxc response bodies diverge from the recorded service: ListServices catalog membership and the UpdateAccountCustomizations list echo order

> _Note: this issue was identified against the code base at commit `f20bf72de740b5a2b8804db1d8ff681c13106e79`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
Two net-new root causes in the uxc (AWSAccountUXSetting, restJson1) data plane. Both are 200-status bodies whose ARRAY CONTENT diverges from the recorded service: the service-identifier catalog ListServices publishes, and the order UpdateAccountCustomizations echoes a list setting in. Both are fixed-shape JSON responses, so the divergence is in the values the service composes, not in the envelope.

Cause 1 of 2 — ListServices publishes a service-identifier catalog whose membership and identifier spellings are not the recorded console catalog (fixture uxc-catalog-list-services, step task#1).
AWS: ListServices "Returns a paginated list of Amazon Web Services service identifiers that you can use as values for the visibleServices setting" (Smithy: ListServices, GET /v1/services, output ListServicesOutput.services -> ServiceList @length(max 500), paginated inputToken/outputToken nextToken, items services). The model fixes no membership, so the catalog's CONTENT is recorded behavior only: the recorded response enumerates the console's own service-home identifiers, a set of 225+ entries in ascending order, spelled the way a console home is addressed — e.g. 'airflow' (Amazon MWAA), 'ds' (Directory Service), 'elasticfilesystem' (EFS), 'elasticmapreduce' (EMR), 'es' (OpenSearch legacy home), 'kafka' (MSK), 'mq' (Amazon MQ), 'deadline' (Deadline Cloud), 'comprehend', 'comprehendmedical', 'connect-health', 'end-user-messaging', 'gameliftservers', 'gameliftstreams', 'geo', 'health-dashboard', 'infrastructure-composer', 'iq', 'managedservices', 'medical-imaging', 'mgh', 'network-security-manager', 'nova-act', 'odb', 'partnercentral', plus further entries past index 158.
Emulator: ListServices answers 200 with a DIFFERENT, hand-seeded catalog (services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs, SEED_SERVICE_CATALOG), so the arrays misalign from index 5 onward: at $.services[5] the recorded 'airflow' is absent and 'amazon-mq' appears instead; likewise 'comprehend'/'comprehendmedical' absent where 'codeguru'/'codepipeline' sit, 'ds' absent where 'docdb' sits, 'elasticfilesystem'/'elasticmapreduce' absent where 'elasticache'/'elasticbeanstalk' sit, 'kafka' absent where 'keyspaces' sits, 'mq' absent where 'msk' sits. The replay reports ~85 diverging indices in one page. The emulator both OMITS recorded identifiers and publishes identifiers the recorded catalog does not name, and it spells several services under an alias of its own ('amazon-mq' for 'mq', 'deadline-cloud' for 'deadline', 'directoryservice' for 'ds', 'efs' for 'elasticfilesystem', 'emr' for 'elasticmapreduce', 'msk' for 'kafka', 'mwaa' for 'airflow').
Why: this is the discovery call a console client uses to learn the legal values of visibleServices, and in this emulator it is also the ADMISSION list — UpdateAccountCustomizations refuses any visibleServices member the catalog does not name with ValidationException 'Invalid services provided' (ops/update_account_customizations.rs, is_catalog_service). So every recorded identifier the catalog omits is a value a client discovers on real AWS and is REFUSED for here, while identifiers that exist only in the emulator's seed are accepted and then unknown to any real console. Index-sensitive clients (page offsets, diffing a cached catalog) also see a catalog of the wrong size and order.
Fix: the catalog seed and its membership rule in the ListServices state component — the seeded identifier set must be the recorded console-identifier namespace (membership AND spelling), kept ascending and de-duplicated so paging and the binary-search admission check stay valid. Not a change to the serializer or to pagination, both of which replay clean.
Traces: uxc-catalog-list-services#task#1.

Cause 2 of 2 — UpdateAccountCustomizations rewrites the order of a visibleServices echo instead of reporting the order the recorded service reports (fixture uxc-partial-update-preserves-other-fields, step task#2).
AWS: UpdateAccountCustomizations (Smithy: PATCH /v1/account-customizations, output UpdateAccountCustomizationsOutput.visibleServices -> ServiceList @length(max 500), no @uniqueItems and no sort trait) documents the output member only as "The current list of visible service identifiers after the update", so the member order is recorded behavior only. The recorded response at this step reports the two configured identifiers as ['s3','ec2'] — i.e. NOT ascending — with HTTP 200. Hypothesis: that is the sequence the request named them in, the update echo reflecting the request while the later GetAccountCustomizations at task#4 reports the pair in the other order.
Emulator: answers 200 with $.visibleServices[0]='ec2' and [1]='s3'. state.rs::canonical_members sorts the supplied members ascending ON THE WRITE (set_visible_services), so the echo can only ever be ascending; the handler's own doc comment acknowledges this step is the one recorded response the rule does not reproduce.
Why: a client that applies a visibleServices update and renders or diffs the echoed list sees its members reordered relative to AWS, so a round-trip comparison of request-against-echo fails on the emulator and succeeds on AWS; any ordering the caller expressed (console display order) is lost at the write, which also means it can never be recovered by a later read.
Fix: the UpdateAccountCustomizations handler and the list-setting write in state — the echo must report the setting in the order the recorded service reports for an update (preserve the supplied sequence rather than canonicalizing on write), while keeping the read path's reported order intact, since the GetAccountCustomizations step of the same fixture replays clean. Status, error channel and the catalog-admission refusal are unaffected.
Traces: uxc-partial-update-preserves-other-fields#task#2.

Neither cause is owned by an open issue: the replay reports empty owned_by and empty stub_path for both. The closest prior filings are all RESOLVED and therefore cover neither — '20261023T115937Z-uxc-listservices-diverges-from-aws-seeded-service-catalog' (fixed; the catalog has since grown but still diverges in membership and spelling) and '20261022T220228Z-uxc-updateaccountcustomizations-reports-list-settings-in-the-wrong-member' / '20261024T101253Z-uxc-account-customization-visibleservices-not-round-tripped-valid-identifiers' (fixed; the current sort-on-write is what now diverges). The one OPEN uxc issue, '20261027T134113Z-uxc-updateaccountcustomizations-maintainability', is about duplicated front-door helpers and carries no replay key for either step.

## Evidence
Replay of trace set simparity-wire-aws-v1: 36 steps, 34 passed, 2 mismatched (score 94.4%), emulator_up=true, bootstrap_signal=false. Both mismatches are service 'uxc', verdict 'mismatch', stub_path empty, owned_by empty.

Cause 1 — fixture uxc-catalog-list-services, step task#1, ListServices, expected_status 200, actual_status 200. Divergence detail (excerpt, index: expected vs actual):
  $.services[5]: missing (expected 'airflow', got 'amazon-mq')
  $.services[55]: missing (expected 'comprehend', got 'codeguru')
  $.services[56]: missing (expected 'comprehendmedical', got 'codepipeline')
  $.services[60]: missing (expected 'connect-health', got 'connect')
  $.services[65]: missing (expected 'datatransferterminal', got 'datasync')
  $.services[67]: missing (expected 'deadline', got 'deadline-cloud')
  $.services[75]: missing (expected 'ds', got 'docdb')
  $.services[84]: missing (expected 'elasticfilesystem', got 'elasticache')
  $.services[85]: missing (expected 'elasticmapreduce', got 'elasticbeanstalk')
  $.services[90]: missing (expected 'es', got 'evs')
  $.services[120]: missing (expected 'kafka', got 'keyspaces')
  $.services[149]: missing (expected 'mq', got 'msk')
  ... ~85 diverging indices in total, up to $.services[224].
Emulator source of the served set: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs, const SEED_SERVICE_CATALOG (sorted and de-duplicated in ComAmazonawsUxcAwsaccountuxsettingState::default).

Cause 2 — fixture uxc-partial-update-preserves-other-fields, step task#2, UpdateAccountCustomizations, expected_status 200, actual_status 200. Divergence detail:
  $.visibleServices[0]: value_mismatch (expected 's3', got 'ec2')
  $.visibleServices[1]: value_mismatch (expected 'ec2', got 's3')
Emulator source: src/state.rs::canonical_members (`supplied.sort()`) reached from AccountUxCustomization::set_visible_services, called by src/ops/update_account_customizations.rs in the write path; the response is then read back out of the stored record.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from it (the uxc service is bootstrapped: service-com-amazonaws-uxc-awsaccountuxsetting is in get_bootstrap_status().bootstrapped).
2. Replay the wire corpus (replay_wire_corpus on trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture uxc-catalog-list-services, step task#1 — GET /v1/services (ListServices) with the fixture's maxResults. Compare the returned $.services array element-by-element with the recorded one: it diverges from index 5 onward (recorded 'airflow' absent, emulator serves 'amazon-mq'), ~85 indices in the page.
4. Cause 2: replay fixture uxc-partial-update-preserves-other-fields, step task#2 — PATCH /v1/account-customizations (UpdateAccountCustomizations) with the fixture's visibleServices pair. The recorded body reports ['s3','ec2']; the emulator answers ['ec2','s3'] because the members are sorted on write. Step task#4 (GetAccountCustomizations) of the same fixture passes and must keep passing.
