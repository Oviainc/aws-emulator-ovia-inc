Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-controls-aws-config-configrule
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-controls-aws-config-configrule
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/0d316cbc7af46ffa3560a5c2b"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/4x11tie7h25sws4g7ilakhcb7"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/2120b6a563bd252dba0d3ec87"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/497wrm2xnk1wxlf4obrdo7mej"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/09f38e820aed77abaa51300ac"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/1ln29qglgkigm9ajfka6u2p2u"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/47188264464d7fb9d1e1a3e62"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/8sdhbqfe2fsmh3b1i2b24xaic"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/85z4kpyxbo5nepzfwbfwwxjx7"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/aba48cb0bdf5be25837b1be7d"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/503uicglhjkokaajywfpt6ros"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/a468fec312084f3ba91029bb2"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/8b02d41bc689a8a604bf87b19"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/9xv17a25orapqa36e19o5b8lp"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/c154d47842afa2389c072f3b8"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/dmb52g7tw004w9iede1pgb1fj"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/6044982be57ba804c4101c54d"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/9utgo6tl86bg3e9jaq073vmqw"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/0de143fa7f7421273cd450ecf"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/7z1uzm6s8qk7ym7m0sm56cq0u"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/6rilu41n0gb9w6mxrkyewoer4"]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/a8a1130a6d5dbe3a8fe6fb52d"]
Issue-Commit: 1cef0270414d463e8e4cd5e638402f2f0968b2f1

# ListControls replaces AWS Config catalog controls with emulator-minted summaries (identity, aliases, parameter summary)

> _Note: this issue was identified against the code base at commit `1cef0270414d463e8e4cd5e638402f2f0968b2f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListControls

## Description
ListControls answers 200 but, for each control the AWS Control Catalog publishes with an AWS::Config::ConfigRule implementation, the page carries an emulator-authored ControlSummary in place of the catalog's: a different (emulator-minted) control ARN, a rewritten Name/Description, a dropped or altered Aliases list, ParameterRequirementSummary forced to NONE, and a synthesized CreateTime. The shape is right; the content is invented. TEN divergence clusters are bundled here, numbered as causes below; ALL TEN must be closed and re-replayed before this issue is done.

Shared AWS basis (cited once, applies to every cause): Smithy model services/service-com-amazonaws-controlcatalog-controlcatalog — ListControls: POST /list-controls, success code 200, @readonly, paginated (inputToken/outputToken NextToken, pageSize MaxResults @range 1..100, items Controls); output ListControlsResponse.Controls is @required. ControlSummary members: Arn @required (ControlArn @length 34..2048, @pattern ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$), Name @required, Description @required, Aliases ("alternative identifiers ... such as SH.S3.1"), Behavior (enum PREVENTIVE|PROACTIVE|DETECTIVE), Severity (enum LOW|MEDIUM|HIGH|CRITICAL), ParameterRequirementSummary (enum REQUIRED|OPTIONAL|NONE — "indicates whether the control requires parameters, accepts optional parameters, or does not support parameters"), Implementation (ImplementationSummary{Type @required, Identifier}), CreateTime ("the time when the control was released ... as a governance capability"), GovernedResources, GovernedProviders. The catalog is an AWS-published, account-independent read-only dataset (the ARNs carry no account or region), so each ControlSummary must reproduce the catalog's own identity and metadata for that control; the model gives the field set, and the recorded responses give the catalog content for the controls below.

Shared Emulator basis: status matches (200), and for each cause the catalog's summary is reported missing while an emulator-authored summary is reported extra — the paired missing/extra under one alias or one implementation identifier is the signature of a fabricated identity, not of a filter or ordering fault.

Shared Why: every control ARN a client harvests from ListControls against the emulator exists in no AWS account, so it is rejected by the real service and (see the related issue below) is the only id the emulator's own GetControl can resolve. ParameterRequirementSummary NONE where the catalog says OPTIONAL tells a client the control takes no parameters, so it never offers or sends them. A dropped alias breaks the alias-to-control lookup that Control Tower and Security Hub clients rely on, and a wrong Severity mis-ranks the control in any risk view.

Shared Fix: the ControlCatalog control seed state the service reads (services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs) and the ControlSummary projection in the ListControls handler (src/ops/list_controls.rs) — Arn, Aliases, Name, Description, Severity, ParameterRequirementSummary, CreateTime and GovernedResources must all be read from the catalog record instead of synthesized.

Cause 1 of 10 — alias CONFIG.ACCOUNT.DT.1 / rule ACCOUNT_PART_OF_ORGANIZATIONS (fixture controlcatalog-catalog-list-controls-aws-config-configrule, step task#1). AWS: the catalog entry is control/4x11tie7h25sws4g7ilakhcb7, Name 'Rule checks whether AWS account is part of AWS Organizations', ParameterRequirementSummary OPTIONAL, Severity HIGH, Implementation AWS::Config::ConfigRule/ACCOUNT_PART_OF_ORGANIZATIONS. Emulator: that entry is absent; an entry with an emulator-minted ARN (recorded run: control/0d316cbc7af46ffa3560a5c2b), a paraphrased Name, ParameterRequirementSummary NONE and a different CreateTime appears under the same alias.

Cause 2 of 10 — alias CONFIG.LOGS.DT.1 / rule CLOUDWATCH_LOG_GROUP_ENCRYPTED (step task#153). AWS: control/497wrm2xnk1wxlf4obrdo7mej, Aliases ['CONFIG.LOGS.DT.1'], ParameterRequirementSummary OPTIONAL, Severity MEDIUM. Emulator: replaced by an emulator-minted ARN whose Aliases list is EMPTY and whose ParameterRequirementSummary is NONE, with a rewritten Name/Description.

Cause 3 of 10 — two distinct Control Tower controls over rule CLOUD_TRAIL_ENABLED collapse (step task#156). AWS: the catalog publishes TWO separate controls sharing one Config rule — control/8sdhbqfe2fsmh3b1i2b24xaic (alias AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_MEMBER_ACCOUNTS, member-account scope) and control/1ln29qglgkigm9ajfka6u2p2u (alias AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_SHARED_ACCOUNTS, Security-OU shared-account scope), each Severity HIGH, ParameterRequirementSummary OPTIONAL. Emulator: BOTH are absent and emulator-minted entries appear instead, so the one-rule/many-controls relation the catalog models is lost and a client cannot distinguish the two scopes.

Cause 4 of 10 — alias CONFIG.CLOUDTRAIL.DT.9 / rule CLOUD_TRAIL_LOG_FILE_VALIDATION_ENABLED (step task#158). AWS: control/85z4kpyxbo5nepzfwbfwwxjx7, Aliases ['CONFIG.CLOUDTRAIL.DT.9'], Severity HIGH. Emulator: replaced by an emulator-minted ARN with EMPTY Aliases and Severity MEDIUM — a severity downgrade of a log-tampering detective control.

Cause 5 of 10 — alias AWS-GR_ENCRYPTED_VOLUMES / rule ENCRYPTED_VOLUMES (step task#362). AWS: control/503uicglhjkokaajywfpt6ros, Name 'Enable encryption for EBS volumes attached to EC2 instances', ParameterRequirementSummary OPTIONAL. Emulator: emulator-minted ARN, paraphrased Name/Description, ParameterRequirementSummary NONE.

Cause 6 of 10 — alias CONFIG.GUARDDUTY.DT.9 / rule GUARDDUTY_ENABLED_CENTRALIZED (step task#395). AWS: control/9xv17a25orapqa36e19o5b8lp, Aliases ['CONFIG.GUARDDUTY.DT.9'], ParameterRequirementSummary OPTIONAL, GovernedResources ['AWS::GuardDuty::Detector']. Emulator: emulator-minted ARN, EMPTY Aliases, ParameterRequirementSummary NONE.

Cause 7 of 10 — alias CONFIG.IAM.DT.1 / rule IAM_PASSWORD_POLICY (step task#407). AWS: control/dmb52g7tw004w9iede1pgb1fj, Aliases ['CONFIG.IAM.DT.1'], ParameterRequirementSummary OPTIONAL (this control is parameterised by the required password-policy strength), GovernedResources []. Emulator: emulator-minted ARN, EMPTY Aliases, ParameterRequirementSummary NONE.

Cause 8 of 10 — alias CONFIG.IAM.DT.13 / rule IAM_ROOT_ACCESS_KEY_CHECK (step task#411). AWS: control/9utgo6tl86bg3e9jaq073vmqw, Aliases ['CONFIG.IAM.DT.13'], Severity CRITICAL, GovernedResources [] (the catalog reports no representable governed resource type for this control). Emulator: emulator-minted ARN, EMPTY Aliases, and GovernedResources invented as ['AWS::IAM::AccessKey'] — the emulator adds a governed resource type the catalog does not publish, so a client filtering controls by governed resource gets a different answer than AWS gives.

Cause 9 of 10 — alias AWS-GR_IAM_USER_MFA_ENABLED / rule IAM_USER_MFA_ENABLED (step task#416). AWS: control/7z1uzm6s8qk7ym7m0sm56cq0u, Aliases ['AWS-GR_IAM_USER_MFA_ENABLED'], Name 'Disallow access to IAM users without MFA'. Emulator: emulator-minted ARN, EMPTY Aliases, rewritten Name/Description.

Cause 10 of 10 — alias AWS-GR_RESTRICTED_SSH / rule INCOMING_SSH_DISABLED (step task#423). AWS: control/6rilu41n0gb9w6mxrkyewoer4, Name 'Disallow Internet connection via SSH', Severity HIGH. Emulator: alias is carried over but the ARN is emulator-minted and the Name/Description are the emulator's own wording, so the entry still cannot be resolved by ARN anywhere else.

Related open issue, which does NOT carry the replay key of any cause above (the replay reports owned_by empty and stub_path empty for all ten clusters): 20261101T194508Z-controlcatalog-serves-a-fabricated-placeholder-control-dataset-getcontrol.md describes the same root dataset defect for GetControl and for two other ListControls clusters (its causes 2 and 3). Fixing the seed will likely close these ten too, but they are separate clusters and each must be re-replayed.

All ARNs, names, timestamps and ids quoted above are the recording's values filling the ControlSummary template; the requirement is that the catalog's own values are served, not these literals.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true (/__admin/health 200), 1408 steps, 4 passed, 1404 mismatched, 53 divergence clusters, score 0.3%. All ten clusters below: service controlcatalog, operation ListControls, verdict mismatch, expected_status 200, actual_status 200, stub_path empty, owned_by empty.

Cause 1 (fixture controlcatalog-catalog-list-controls-aws-config-configrule#task#1): $.Controls[Arn="arn:aws:controlcatalog:::control/4x11tie7h25sws4g7ilakhcb7"]: missing (expected {'Aliases': ['CONFIG.ACCOUNT.DT.1'], 'Behavior': 'DETECTIVE', 'CreateTime': 1594166400.0, 'Name': 'Rule checks whether AWS account is part of AWS Organizations', 'ParameterRequirementSummary': 'OPTIONAL', 'Severity': 'HIGH', 'Implementation': {'Identifier': 'ACCOUNT_PART_OF_ORGANIZATIONS', 'Type': 'AWS::Config::ConfigRule'}}, got None); $.Controls[Arn="arn:aws:controlcatalog:::control/0d316cbc7af46ffa3560a5c2b"]: extra (got {'Aliases': ['CONFIG.ACCOUNT.DT.1'], 'Name': 'Checks whether an Amazon Web Services account is part of an organization', 'ParameterRequirementSummary': 'NONE', 'CreateTime': 1701043200, ...}).

Cause 2 (#task#153): expected control/497wrm2xnk1wxlf4obrdo7mej with Aliases ['CONFIG.LOGS.DT.1'] and ParameterRequirementSummary 'OPTIONAL' — missing; extra control/2120b6a563bd252dba0d3ec87 with 'Aliases': [] and 'ParameterRequirementSummary': 'NONE'.

Cause 3 (#task#156): expected control/8sdhbqfe2fsmh3b1i2b24xaic (AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_MEMBER_ACCOUNTS) and control/1ln29qglgkigm9ajfka6u2p2u (AWS-GR_DETECT_CLOUDTRAIL_ENABLED_ON_SHARED_ACCOUNTS) — both missing; extra control/09f38e820aed77abaa51300ac and control/47188264464d7fb9d1e1a3e62.

Cause 4 (#task#158): expected control/85z4kpyxbo5nepzfwbfwwxjx7, Aliases ['CONFIG.CLOUDTRAIL.DT.9'], Severity 'HIGH' — missing; extra control/aba48cb0bdf5be25837b1be7d with 'Aliases': [] and 'Severity': 'MEDIUM'.

Cause 5 (#task#362): expected control/503uicglhjkokaajywfpt6ros, 'ParameterRequirementSummary': 'OPTIONAL' — missing; extra control/a468fec312084f3ba91029bb2 with 'ParameterRequirementSummary': 'NONE'.

Cause 6 (#task#395): expected control/9xv17a25orapqa36e19o5b8lp, Aliases ['CONFIG.GUARDDUTY.DT.9'], PRS 'OPTIONAL' — missing; extra control/8b02d41bc689a8a604bf87b19, 'Aliases': [], PRS 'NONE'.

Cause 7 (#task#407): expected control/dmb52g7tw004w9iede1pgb1fj, Aliases ['CONFIG.IAM.DT.1'], PRS 'OPTIONAL' — missing; extra control/c154d47842afa2389c072f3b8, 'Aliases': [], PRS 'NONE'.

Cause 8 (#task#411): expected control/9utgo6tl86bg3e9jaq073vmqw, Aliases ['CONFIG.IAM.DT.13'], 'GovernedResources': [] — missing; extra control/6044982be57ba804c4101c54d, 'Aliases': [], 'GovernedResources': ['AWS::IAM::AccessKey'].

Cause 9 (#task#416): expected control/7z1uzm6s8qk7ym7m0sm56cq0u, Aliases ['AWS-GR_IAM_USER_MFA_ENABLED'], Name 'Disallow access to IAM users without MFA' — missing; extra control/0de143fa7f7421273cd450ecf, 'Aliases': [].

Cause 10 (#task#423): expected control/6rilu41n0gb9w6mxrkyewoer4, Name 'Disallow Internet connection via SSH' — missing; extra control/a8a1130a6d5dbe3a8fe6fb52d with the emulator's own Name/Description.

No status or error-code divergence in any cause: all return the modeled 200, and the defect is entirely the content of the @required Controls list.

## Steps to Reproduce
1. Boot the emulator from this checkout (controlcatalog is bootstrapped; /__admin/health answers 200) and replay the simparity-wire-aws-v1 wire corpus.
2. Replay fixture controlcatalog-catalog-list-controls-aws-config-configrule at step task#1 — POST /list-controls with the implementation filter for AWS::Config::ConfigRule. The returned page omits control/4x11tie7h25sws4g7ilakhcb7 and carries an emulator-minted ARN under alias CONFIG.ACCOUNT.DT.1 with ParameterRequirementSummary NONE.
3. Repeat for the other nine cluster representatives of the same fixture: task#153, task#156, task#158, task#362, task#395, task#407, task#411, task#416, task#423. In each the catalog's control (named in the issue) is absent and an emulator-authored summary appears with a minted ARN and, depending on the cause, an emptied Aliases list, ParameterRequirementSummary NONE, a changed Severity or an invented GovernedResources entry.
4. Confirm the fix by re-replaying ALL TEN representatives (task#1, task#153, task#156, task#158, task#362, task#395, task#407, task#411, task#416, task#423 of controlcatalog-catalog-list-controls-aws-config-configrule): each page must carry the catalog's own control ARN, aliases, name, description, severity, parameter-requirement summary, create time and governed resources, including both distinct controls that share the CLOUD_TRAIL_ENABLED rule.
