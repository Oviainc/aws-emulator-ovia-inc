Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-get-control
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-get-control controlcatalog-catalog-list-controls-aws-config-configrule controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol
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
Issue-Commit: 837306e76dca4544e8b76426022c8d4a2dc48510

# ControlCatalog control inventory is not served: GetControl 404s and ListControls omits the catalog

> _Note: this issue was identified against the code base at commit `837306e76dca4544e8b76426022c8d4a2dc48510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetControl

## Description
The emulator does not serve the AWS Control Catalog control inventory: reads of individual controls fail and listings return a different (near-empty) population than the service does. 4 causes, all on service controlcatalog.

Cause 1 of 4 - GetControl on a control ARN that exists in the catalog (fixture controlcatalog-catalog-get-control, step task#1; cluster covers task#1..task#20).
AWS: Smithy: GetControl, POST /get-control, input GetControlRequest.ControlArn required with @length 34..2048 and @pattern ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$; output GetControlResponse with required Arn, Name, Description, Behavior, RegionConfiguration plus Severity, Implementation, ParameterRequirementSummary, Parameters, CreateTime, GovernedResources, GovernedProviders; modeled error ResourceNotFoundException (404) for an ARN that is not in the catalog. The Control Catalog is a global, read-only catalog, so a control ARN that the catalog publishes resolves with HTTP 200 and the full control record.
Emulator: answers HTTP 404 and no response members at all for those ARNs.
Why: a client that discovered a control (from the catalog, from Control Tower, or from a hard-coded alias such as CONFIG.COGNITO-IDP.DT.1) and then calls GetControl is told the control does not exist, so control-detail views, drift/compliance tooling and any code that resolves an alias to Behavior/Severity/Implementation fail instead of reading metadata.
Fix: the GetControl handler plus the control-catalog data its lookup reads (see Traces for the crate).

Cause 2 of 4 - ListControls returns none of the catalog's controls (fixture controlcatalog-catalog-list-controls-aws-config-configrule, step task#1; cluster covers 1300 steps).
AWS: Smithy: ListControls, POST /list-controls, output ListControlsResponse.Controls required (list of ControlSummary: Arn, Aliases, Name, Description, Behavior, Severity, ParameterRequirementSummary, Implementation, CreateTime, GovernedResources, GovernedProviders), @paginated on Controls. AWS returns the catalog's controls, optionally narrowed by ControlFilter.
Emulator: answers HTTP 200 but the expected ControlSummary entries are absent from Controls (replay reports $.Controls[0] missing, got None).
Why: every consumer that enumerates controls - console-style catalog browsing, SDK paginators, compliance inventories - sees an empty or near-empty catalog, so downstream GetControl/ListControlMappings work is never attempted and pagination over the catalog never yields a page.
Fix: the ListControls handler and the control-catalog seed state it enumerates.

Cause 3 of 4 - ListControls narrowed to AWS::Config::ConfigRule implementations returns no entries (same fixture, step task#156; cluster also covers task#623, task#624).
AWS: as Cause 2; ControlFilter narrows the result set but AWS still returns the matching ControlSummary records (the recording expects controls with Implementation {Type: AWS::Config::ConfigRule, Identifier: CLOUD_TRAIL_ENABLED} and Behavior DETECTIVE).
Emulator: HTTP 200 with the expected ControlSummary entries missing.
Why: filter-by-implementation is the documented way to map a Config rule back to its governing control; it returns nothing, so the mapping silently comes back empty rather than erroring.
Fix: the ListControls filter evaluation and the seed data it filters over.

Cause 4 of 4 - ListControls returns a substitute control instead of the catalogued one for the same implementation identifier (same fixture, step task#499; cluster also covers task#626 and controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol#task#332).
AWS: as Cause 2. For implementation identifier MULTI_REGION_CLOUD_TRAIL_ENABLED the catalog publishes the control whose ARN ends in control/d5kdbugbadk3r481vtwnvoynv (alias CONFIG.CLOUDTRAIL.DT.10).
Emulator: that ARN is missing from Controls and a different ARN (control/4fd229eee2ca7d94166b64f5e, alias SH.CloudTrail.1) is returned in its place.
Why: control ARNs and aliases are stable identifiers that clients persist and compare; returning a different ARN/alias for the same implementation makes alias->ARN resolution and any stored reference wrong, and shows the emulator serves a hand-built substitute population rather than the catalog.
Fix: the control-catalog seed state (identity of the seeded controls) and the ListControls handler that projects it.

Traces: see the fixture#step list above; all live in the controlcatalog service crate (services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/ for the handlers and src/state.rs for the served catalog data).

## Evidence
Cause 1: fixture controlcatalog-catalog-get-control step task#1 - expected status 200 with Arn 'arn:aws:controlcatalog:::control/1038h7d21e6g8i10g6v3xsssi', Behavior 'DETECTIVE', Severity 'MEDIUM', Implementation {Type AWS::Config::ConfigRule, Identifier COGNITO_USER_POOL_ADVANCED_SECURITY_ENABLED}, Aliases ['CONFIG.COGNITO-IDP.DT.1'], RegionConfiguration.Scope 'REGIONAL'; got status 404 and every one of those members absent. 20 steps in the cluster.
Cause 2: fixture controlcatalog-catalog-list-controls-aws-config-configrule step task#1 - status 200 == 200, but $.Controls[0] missing (expected the ControlSummary for control/4x11tie7h25sws4g7ilakhcb7, alias CONFIG.ACCOUNT.DT.1, Severity HIGH; got None). 1300 steps in the cluster.
Cause 3: same fixture step task#156 - status 200 == 200, $.Controls[0] and $.Controls[1] missing (expected control/8sdhbqfe2fsmh3b1i2b24xaic and control/1ln29qglgkigm9ajfka6u2p2u, Implementation Identifier CLOUD_TRAIL_ENABLED; got None).
Cause 4: same fixture step task#499 - status 200 == 200, $.Controls[Arn="arn:aws:controlcatalog:::control/d5kdbugbadk3r481vtwnvoynv"] missing and $.Controls[Arn="arn:aws:controlcatalog:::control/4fd229eee2ca7d94166b64f5e"] extra (alias SH.CloudTrail.1).

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay fixture controlcatalog-catalog-get-control, step task#1 (POST /get-control with ControlArn arn:aws:controlcatalog:::control/1038h7d21e6g8i10g6v3xsssi): observe 404 instead of 200 with the control record.
3. Replay fixture controlcatalog-catalog-list-controls-aws-config-configrule, steps task#1, task#156 and task#499 (POST /list-controls, unfiltered and filtered by implementation): observe 200 with the expected ControlSummary entries absent, and at task#499 a different control ARN returned in place of the catalogued one.
