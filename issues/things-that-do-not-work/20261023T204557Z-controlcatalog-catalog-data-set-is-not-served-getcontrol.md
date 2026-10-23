Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-get-control
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-get-control controlcatalog-catalog-list-controls-aws-config-configrule controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol controlcatalog-catalog-list-domains controlcatalog-catalog-list-objectives
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Aliases
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Arn
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Behavior
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.CreateTime
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Description
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.GovernedProviders
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.GovernedResources
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Implementation
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Name
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.ParameterRequirementSummary
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Parameters
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.RegionConfiguration
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Severity
Issue-Key: controlcatalog|GetControl|2xx->4xx|status
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[*]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn
Issue-Key: controlcatalog|ListDomains|2xx->2xx|$.Domains[Arn="arn
Issue-Key: controlcatalog|ListObjectives|2xx->2xx|$.Objectives[Arn="arn
Issue-Commit: 9b28682be3d06b0e1ca8f56d8f0bc182f6004def

# controlcatalog catalog data set is not served: GetControl 404s and ListControls/ListDomains/ListObjectives answer with empty collections

> _Note: this issue was identified against the code base at commit `9b28682be3d06b0e1ca8f56d8f0bc182f6004def`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetControl

## Description
The ControlCatalog emulator does not serve the AWS-managed Control Catalog data set. Six causes, all on the read-only catalog surface (6 of 6 must be closed). NOTE: the resolved issues 20261021T234107Z-controlcatalog-control-inventory-is-not-served-getcontrol-404s and 20261021T234107Z-controlcatalog-listdomains-and-listobjectives-return-empty-ontology-lists describe the same surface but are CLOSED, so they do not cover these replays — the corpus still diverges.

Cause 1 of 6: GetControl on a control ARN that exists in the catalog answers 404 instead of the control record.
  AWS: GetControl (Smithy: restJson1, POST /get-control; output GetControlResponse; only modelled error ResourceNotFoundException @httpError 404, doc 'The requested resource does not exist.') returns 200 with Arn, Aliases, Name, Description, Behavior, Severity, ParameterRequirementSummary, Implementation {Type, Identifier}, CreateTime, GovernedResources, GovernedProviders, Parameters and RegionConfiguration {Scope, DeployableRegions} for every AWS-managed control. ControlSummary/GetControlResponse mark Arn, Name and Description required.
  Emulator: status 404 and no body members at all; every one of the 20 ARNs exercised by the fixture is reported unknown.
  Why: a client cannot read any control's metadata — Behavior/Severity/Implementation/RegionConfiguration driven logic (Control Tower enablement, compliance tooling) sees every well-known control ARN as nonexistent.
  Fix: the GetControl handler and the catalog seed data it reads.
  Traces: controlcatalog-catalog-get-control#task#1 (cluster covers 20 steps, task#1..task#20).

Cause 2 of 6: ListControls returns none of the AWS::Config::ConfigRule-implemented controls.
  AWS: ListControls (POST /list-controls) returns a paginated Controls list of ControlSummary records; the recording shows the first page carrying e.g. Arn arn:aws:controlcatalog:::control/4x11tie7h25sws4g7ilakhcb7, Aliases ['CONFIG.ACCOUNT.DT.1'], Implementation {Type: AWS::Config::ConfigRule, Identifier: ACCOUNT_PART_OF_ORGANIZATIONS}, Severity HIGH.
  Emulator: status 200 but $.Controls[0] is absent — the expected records are missing from the served list.
  Why: control discovery by implementation identifier or alias returns nothing, so any catalog enumeration is empty-handed while still reporting success (worse than an error, because the client cannot tell).
  Fix: the ListControls handler and the seeded control catalog.
  Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#1 (cluster covers 1300 steps).

Cause 3 of 6: the CloudTrail-family controls are likewise absent from ListControls pages.
  AWS: the recording expects Controls entries for arn:aws:controlcatalog:::control/8sdhbqfe2fsmh3b1i2b24xaic (alias AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_MEMBER_ACCOUNTS) and .../1ln29qglgkigm9ajfka6u2p2u, both Implementation {Type: AWS::Config::ConfigRule, Identifier: CLOUD_TRAIL_ENABLED}, Severity HIGH (recorded behavior only for the exact membership).
  Emulator: status 200 with $.Controls[0] and $.Controls[1] missing.
  Why: alias-based lookups of Control Tower 'AWS-GR_*' controls resolve to nothing.
  Fix: the seeded control catalog consumed by the ListControls handler.
  Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#156 (cluster covers task#623, task#624).

Cause 4 of 6: ListControls serves a SYNTHETIC control record whose Arn is not a catalog control id, in place of the real one.
  AWS: the recorded page contains Arn arn:aws:controlcatalog:::control/d5kdbugbadk3r481vtwnvoynv (alias CONFIG.CLOUDTRAIL.DT.10, Name 'Checks that there is at least one multi-region AWS CloudTrail', ParameterRequirementSummary OPTIONAL) and contains no SH.* control for that implementation. ControlArn is constrained by Smithy pattern ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$ with @length min 34.
  Emulator: the expected entry is missing and an EXTRA entry is returned instead — Arn arn:aws:controlcatalog:::control/4fd229eee2ca7d94166b64f5e, Aliases ['SH.CloudTrail.1'], Name 'Require a multi-Region CloudTrail trail', ParameterRequirementSummary NONE, CreateTime 1701216000 — i.e. a locally invented id for the same MULTI_REGION_CLOUD_TRAIL_ENABLED implementation.
  Why: a client that keys controls by ARN (the catalog's only stable identity) stores an ARN that no other AWS surface recognises, and sees the wrong alias/parameter-requirement for the control.
  Fix: the ListControls handler's catalog seed — the record's identity and alias set, not the response shape.
  Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#499 (cluster covers task#626 and controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol#task#332).

Cause 5 of 6: ListDomains returns no domains.
  AWS: ListDomains (POST /domains) returns the Domains list of the control ontology; the recording expects arn:aws:controlcatalog:::domain/d4msesd9vvmzmmuvlv06m92uq 'Asset management', .../33mjpzadrlwo1by3c1012ai5i 'Business continuity and recovery', .../884eyhr622uy73h6i1mjb9lfb 'Data protection', .../avh1ncrx6oyndrbsv0rfywvat and the remaining domains, each with Description, CreateTime and LastUpdateTime.
  Emulator: status 200 with every expected $.Domains[Arn=...] entry missing.
  Why: the ontology is undiscoverable, so the domain-filtered calls a client builds next (ListObjectives with ObjectiveFilter.Domains, ListCommonControls) have no ARN to pass.
  Fix: the ListDomains handler and the seeded ontology state.
  Traces: controlcatalog-catalog-list-domains#task#1.

Cause 6 of 6: ListObjectives returns no objectives.
  AWS: ListObjectives (POST /objectives) returns Objectives, each with Arn, Name, Description, Domain {Arn, Name}, CreateTime, LastUpdateTime — e.g. arn:aws:controlcatalog:::objective/ad11p1961s8erra9m185wa1nn 'Asset inventory management' under domain d4msesd9vvmzmmuvlv06m92uq.
  Emulator: status 200 with every expected $.Objectives[Arn=...] entry missing.
  Why: the domain -> objective -> common-control hierarchy cannot be walked at all.
  Fix: the ListObjectives handler and the seeded ontology state.
  Traces: controlcatalog-catalog-list-objectives#task#1.

## Evidence
Cause 1: expected 200 + full control record, got 404 and $.Aliases/$.Arn/$.Behavior/$.CreateTime/$.Description/$.GovernedProviders/$.GovernedResources/$.Implementation/$.Name/$.ParameterRequirementSummary/$.Parameters/$.RegionConfiguration/$.Severity all None (controlcatalog-catalog-get-control#task#1, 20 affected steps).
Cause 2: expected 200 with $.Controls[0] = {Aliases:['CONFIG.ACCOUNT.DT.1'], Arn:...4x11tie7h25sws4g7ilakhcb7, Implementation:{Identifier:ACCOUNT_PART_OF_ORGANIZATIONS,...}}, got 200 with $.Controls[0] missing (1300 affected steps).
Cause 3: expected $.Controls[0]/$.Controls[1] = the two CLOUD_TRAIL_ENABLED controls, got both missing (3 affected steps).
Cause 4: $.Controls[Arn="arn:aws:controlcatalog:::control/d5kdbugbadk3r481vtwnvoynv"] missing AND $.Controls[Arn="arn:aws:controlcatalog:::control/4fd229eee2ca7d94166b64f5e"] extra (Aliases ['SH.CloudTrail.1'], ParameterRequirementSummary NONE).
Cause 5: all $.Domains[Arn=...] entries missing on a 200.
Cause 6: all $.Objectives[Arn=...] entries missing on a 200.
Replay summary for the corpus: steps_total 1408, steps_passed 3, steps_mismatched 1405, score 0.2%, emulator_up true, bootstrap_signal false, state_reset_failures 0.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. POST /get-control with ControlArn arn:aws:controlcatalog:::control/1038h7d21e6g8i10g6v3xsssi (fixture controlcatalog-catalog-get-control, step task#1) -> 404 instead of 200 with the control record.
3. POST /list-controls with no filter (fixture controlcatalog-catalog-list-controls-aws-config-configrule, steps task#1, task#156, task#499) -> 200 whose Controls list omits the recorded Config-rule controls and, at task#499, carries a synthetic SH.CloudTrail.1 record with ARN .../4fd229eee2ca7d94166b64f5e.
4. POST /domains (controlcatalog-catalog-list-domains#task#1) and POST /objectives (controlcatalog-catalog-list-objectives#task#1) -> 200 with empty ontology lists.
