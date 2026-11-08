Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-controls-aws-config-configrule
Issue-Step: task#499
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-controls-aws-config-configrule
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/4f5e63ab34148fd6fce9149d0"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/d5kdbugbadk3r481vtwnvoynv"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/1h4eyqyyonp19dlrreqf1i3w0"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/df6deb8755c6f736d8d74b2a3"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/9d62ac19c32f3953f39475ad8"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/e34kieahgkm0lggs5g0s412jt"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/8sw3pbid15t9cbww8d2w2qwgf"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/9jzlkwys54gzgsx3pmkqnx8l1"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/d2b5e7c9fda5a309668d72a33"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/1a3d2be92f97bc625913e7de7"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/9j9nwxj789d82sypnukhyyowy"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/ci9ov6vzjoi6vzkajgademvvo"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/91811566af35e4122a39af54a"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/d4wgeffz6izrb627c2yb8nq8d"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/c93043a8e919aa8e4e1580538"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/e5efjudq0b347dgxn86o8fg2o"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/22484xhzccf9twis5gmdt7uic"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/33ae872ca987492988e8baa4d"]
Issue-Commit: 1cef0270414d463e8e4cd5e638402f2f0968b2f1

# ListControls drops CloudTrail, RDS, S3 and VPC Config controls and mis-reports their replacements

> _Note: this issue was identified against the code base at commit `1cef0270414d463e8e4cd5e638402f2f0968b2f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListControls

## Description
Continuing the same ListControls dataset defect over the AWS Config-implemented part of the catalog, these eight clusters cover the CloudTrail, RDS, S3 and VPC controls. Here the emulator additionally COLLAPSES pairs of distinct catalog controls that share one Config rule into a single invented entry, misspells an alias, and downgrades a severity. EIGHT divergence clusters are bundled here, numbered as causes below; ALL EIGHT must be closed and re-replayed before this issue is done.

Shared AWS basis (cited once, applies to every cause): Smithy model services/service-com-amazonaws-controlcatalog-controlcatalog — ListControls: POST /list-controls, success code 200, @readonly, paginated (NextToken/NextToken, pageSize MaxResults @range 1..100, items Controls); ListControlsResponse.Controls is @required. ControlSummary: Arn @required (@pattern ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$), Name @required, Description @required, Aliases, Behavior, Severity (enum LOW|MEDIUM|HIGH|CRITICAL), ParameterRequirementSummary (enum REQUIRED|OPTIONAL|NONE), Implementation{Type @required, Identifier}, CreateTime, GovernedResources, GovernedProviders. The catalog is AWS-published and account-independent, so each summary must reproduce the catalog's own control identity and metadata; the recorded responses supply the catalog content named per cause. Note the catalog deliberately publishes SEPARATE controls that share one Config rule identifier (an account-wide one and a log-archive/audit-bucket one), distinguished by alias, name and description.

Shared Emulator basis: status matches (200); the catalog's summary is reported missing and an emulator-authored summary is reported extra, with an emulator-minted control ARN and rewritten metadata.

Shared Why: a client enumerating the catalog cannot find the control it is looking for by ARN or by alias, and when two catalog controls collapse into one it loses the distinction between "any S3 bucket" and "the log-archive bucket" governance, under-reporting the controls it must enable. A severity downgrade mis-ranks a critical public-exposure control; a misspelled alias makes alias lookup fail outright.

Shared Fix: the ControlCatalog control seed state (services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs) must hold one record per published catalog control (including both members of each shared-rule pair, with their real ARNs, aliases, names, descriptions, severities, parameter summaries, create times and governed resources), and the ControlSummary projection in the ListControls handler (src/ops/list_controls.rs) must emit those record values rather than synthesizing them.

Cause 1 of 8 — alias CONFIG.CLOUDTRAIL.DT.10 / rule MULTI_REGION_CLOUD_TRAIL_ENABLED (fixture controlcatalog-catalog-list-controls-aws-config-configrule, step task#499). AWS: control/d5kdbugbadk3r481vtwnvoynv, Name 'Checks that there is at least one multi-region AWS CloudTrail', Severity HIGH, ParameterRequirementSummary OPTIONAL. Emulator: emulator-minted ARN, rewritten Description, ParameterRequirementSummary NONE.

Cause 2 of 8 — alias AWS-GR_RDS_SNAPSHOTS_PUBLIC_PROHIBITED / rule RDS_SNAPSHOTS_PUBLIC_PROHIBITED (step task#567). AWS: control/1h4eyqyyonp19dlrreqf1i3w0, Aliases ['AWS-GR_RDS_SNAPSHOTS_PUBLIC_PROHIBITED'] (plural SNAPSHOTS), Name 'Disallow public access to RDS database snapshots', Severity CRITICAL, GovernedResources []. Emulator: emulator-minted ARN, alias misspelled as 'AWS-GR_RDS_SNAPSHOT_PUBLIC_PROHIBITED' (singular), Severity downgraded to HIGH, and GovernedResources invented as ['AWS::RDS::DBSnapshot'].

Cause 3 of 8 — alias AWS-GR_RDS_STORAGE_ENCRYPTED / rule RDS_STORAGE_ENCRYPTED (step task#571). AWS: control/e34kieahgkm0lggs5g0s412jt, Name 'Disallow RDS database instances without storage encryption', ParameterRequirementSummary OPTIONAL. Emulator: emulator-minted ARN, rewritten Name/Description, ParameterRequirementSummary NONE.

Cause 4 of 8 — the two public-READ S3 controls over rule S3_BUCKET_PUBLIC_READ_PROHIBITED collapse (step task#623). AWS: control/8sw3pbid15t9cbww8d2w2qwgf (alias AWS-GR_S3_BUCKET_PUBLIC_READ_PROHIBITED, 'Disallow public read access to S3 buckets') AND control/9jzlkwys54gzgsx3pmkqnx8l1 (alias AWS-GR_AUDIT_BUCKET_PUBLIC_READ_PROHIBITED, 'Disallow public read access to log archive'), both Severity CRITICAL. Emulator: both missing; a single emulator-minted entry appears carrying only the audit-bucket alias, so the account-wide control vanishes from the catalog.

Cause 5 of 8 — the two public-WRITE S3 controls over rule S3_BUCKET_PUBLIC_WRITE_PROHIBITED collapse (step task#624). AWS: control/9j9nwxj789d82sypnukhyyowy (alias AWS-GR_S3_BUCKET_PUBLIC_WRITE_PROHIBITED) AND control/ci9ov6vzjoi6vzkajgademvvo (alias AWS-GR_AUDIT_BUCKET_PUBLIC_WRITE_PROHIBITED), both Severity CRITICAL. Emulator: both missing; one emulator-minted entry with only the audit-bucket alias.

Cause 6 of 8 — alias CONFIG.S3.DT.8 / rule S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED (step task#626). AWS: control/d4wgeffz6izrb627c2yb8nq8d, Aliases ['CONFIG.S3.DT.8'], Severity MEDIUM. Emulator: emulator-minted ARN with EMPTY Aliases and the emulator's own Name/Description.

Cause 7 of 8 — alias CONFIG.S3.DT.15 / rule S3_BUCKET_SSL_REQUESTS_ONLY (step task#627). AWS: control/e5efjudq0b347dgxn86o8fg2o, Aliases ['CONFIG.S3.DT.15']. Emulator: emulator-minted ARN with EMPTY Aliases.

Cause 8 of 8 — alias CONFIG.EC2.DT.48 / rule VPC_FLOW_LOGS_ENABLED (step task#711). AWS: control/22484xhzccf9twis5gmdt7uic, Aliases ['CONFIG.EC2.DT.48'], ParameterRequirementSummary OPTIONAL, GovernedResources ['AWS::EC2::VPC']. Emulator: emulator-minted ARN, EMPTY Aliases, ParameterRequirementSummary NONE.

Related open issue, which does NOT carry the replay key of any cause above (the replay reports owned_by empty and stub_path empty for all eight clusters): 20261101T194508Z-controlcatalog-serves-a-fabricated-placeholder-control-dataset-getcontrol.md covers the same seed defect for GetControl and two other ListControls clusters. The shared-rule collapse (causes 4 and 5) and the alias/severity faults here are not described there.

All ARNs, names, timestamps and ids quoted above are the recording's values filling the ControlSummary template; the requirement is that the catalog's own values are served, not these literals.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, 1408 steps, 4 passed, 53 divergence clusters. All eight clusters: service controlcatalog, operation ListControls, fixture controlcatalog-catalog-list-controls-aws-config-configrule, verdict mismatch, expected_status 200, actual_status 200, stub_path empty, owned_by empty.

Cause 1 (#task#499): $.Controls[Arn="...control/d5kdbugbadk3r481vtwnvoynv"]: missing (expected {'Aliases': ['CONFIG.CLOUDTRAIL.DT.10'], 'Name': 'Checks that there is at least one multi-region AWS CloudTrail', 'ParameterRequirementSummary': 'OPTIONAL', 'Severity': 'HIGH', 'Implementation': {'Identifier': 'MULTI_REGION_CLOUD_TRAIL_ENABLED', 'Type': 'AWS::Config::ConfigRule'}}, got None); extra control/4f5e63ab34148fd6fce9149d0 with 'ParameterRequirementSummary': 'NONE'.

Cause 2 (#task#567): expected control/1h4eyqyyonp19dlrreqf1i3w0 {'Aliases': ['AWS-GR_RDS_SNAPSHOTS_PUBLIC_PROHIBITED'], 'Name': 'Disallow public access to RDS database snapshots', 'Severity': 'CRITICAL', 'GovernedResources': []} — missing; extra control/df6deb8755c6f736d8d74b2a3 {'Aliases': ['AWS-GR_RDS_SNAPSHOT_PUBLIC_PROHIBITED'], 'Severity': 'HIGH', 'GovernedResources': ['AWS::RDS::DBSnapshot']}.

Cause 3 (#task#571): expected control/e34kieahgkm0lggs5g0s412jt, PRS 'OPTIONAL' — missing; extra control/9d62ac19c32f3953f39475ad8, PRS 'NONE'.

Cause 4 (#task#623): expected control/8sw3pbid15t9cbww8d2w2qwgf ('Disallow public read access to S3 buckets') AND control/9jzlkwys54gzgsx3pmkqnx8l1 ('Disallow public read access to log archive') — both missing; extra control/d2b5e7c9fda5a309668d72a33 {'Aliases': ['AWS-GR_AUDIT_BUCKET_PUBLIC_READ_PROHIBITED'], 'Implementation': {'Identifier': 'S3_BUCKET_PUBLIC_READ_PROHIBITED', ...}}.

Cause 5 (#task#624): expected control/9j9nwxj789d82sypnukhyyowy AND control/ci9ov6vzjoi6vzkajgademvvo — both missing; extra control/1a3d2be92f97bc625913e7de7 {'Aliases': ['AWS-GR_AUDIT_BUCKET_PUBLIC_WRITE_PROHIBITED']}.

Cause 6 (#task#626): expected control/d4wgeffz6izrb627c2yb8nq8d, Aliases ['CONFIG.S3.DT.8'] — missing; extra control/91811566af35e4122a39af54a, 'Aliases': [].

Cause 7 (#task#627): expected control/e5efjudq0b347dgxn86o8fg2o, Aliases ['CONFIG.S3.DT.15'] — missing; extra control/c93043a8e919aa8e4e1580538, 'Aliases': [].

Cause 8 (#task#711): expected control/22484xhzccf9twis5gmdt7uic, Aliases ['CONFIG.EC2.DT.48'], PRS 'OPTIONAL' — missing; extra control/33ae872ca987492988e8baa4d, 'Aliases': [], PRS 'NONE'.

No status or error-code divergence: every step returns the modeled 200 and only the @required Controls content diverges.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus (service controlcatalog).
2. Replay fixture controlcatalog-catalog-list-controls-aws-config-configrule at step task#499 — POST /list-controls filtered to AWS::Config::ConfigRule. The page omits control/d5kdbugbadk3r481vtwnvoynv and carries an emulator-minted replacement with ParameterRequirementSummary NONE.
3. Repeat for task#567 (RDS snapshot control: misspelled alias, Severity HIGH instead of CRITICAL, invented GovernedResources), task#571, task#623 and task#624 (two catalog controls collapse into one invented entry per S3 public-access rule), task#626, task#627 and task#711.
4. Confirm the fix by re-replaying ALL EIGHT representatives (task#499, task#567, task#571, task#623, task#624, task#626, task#627, task#711 of controlcatalog-catalog-list-controls-aws-config-configrule): each page must carry the catalog's own control ARNs — including BOTH controls of each shared-rule pair — with their published aliases, names, descriptions, severities, parameter-requirement summaries and governed resources.
