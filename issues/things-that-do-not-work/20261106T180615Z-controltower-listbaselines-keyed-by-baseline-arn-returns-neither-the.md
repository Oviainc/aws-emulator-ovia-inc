Issue-Source: trace-sweep-divergence
Issue-Trace: controltower-catalog-list-baselines
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controltower-catalog-list-baselines
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/1QBGH2G48YVGDQ3Y"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/2V6DZ8QKR4JT7MWN"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/3WPD0NA6TJ9AOMU2"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/4T4HA1KMO10S6311"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/A6EEAE10F08193F2"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/APO9ATVPBKFRRGLK"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/H6C5JFCJJ3CPU3J5"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/J8HX46AHS5MIKQPD"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/LN25R72TTG6IGPTQ"].description
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/QXK3R9ZB7PN5TD2H"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/TR7NC4VPG9DH2KLM"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/WM8FJ6YHL3QS5XKB"]
Issue-Key: controltower|ListBaselines|2xx->2xx|$.baselines[arn="arn:aws:controltower:us-west-2::baseline/YX7VMZML5IG8EJUD"]
Issue-Commit: 1cef0270414d463e8e4cd5e638402f2f0968b2f1

# ListBaselines keyed by baseline ARN returns neither the AWS id segments nor the AWS descriptions

> _Note: this issue was identified against the code base at commit `1cef0270414d463e8e4cd5e638402f2f0968b2f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBaselines

## Description
The AWS Control Tower baseline catalog is AWS-owned and identical in every account: each entry pairs a fixed baseline name with a fixed baseline ARN and a fixed description, and clients discover a baseline by ARN before calling GetBaseline/EnableBaseline on it. The emulator serves an invented catalog, so when the response is indexed BY ARN (rather than by name) the AWS-managed entries are simply not there and the entries that are there carry emulator-authored prose.

Cause 1 of 1: ListBaselines, fixture controltower-catalog-list-baselines, step task#1.
AWS: Smithy: ListBaselines (POST /list-baselines, http 200) -> ListBaselinesOutput.baselines (required, list of Baseline{arn,name,description}); the ARN member is BaselineArn with @pattern ^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$, and GetBaseline takes exactly that ARN as its required baselineIdentifier, which is what makes the id segment a stable public identifier rather than a generated value. The individual names, id segments and descriptions are recorded behavior only: the recording shows the catalog containing an entry at arn:aws:controltower:<region>::baseline/A6EEAE10F08193F2 named CentralSecurityRolesBaseline ('Sets up central resources for security monitoring within your organization.'), one at .../J8HX46AHS5MIKQPD named LogArchiveBaseline ('Sets up a central repository for logs of API activities and resource configurations from accounts in your organization.'), one at .../3WPD0NA6TJ9AOMU2 named BackupCentralVaultBaseline ('Sets up a central AWS Backup vault in your organization.'), plus .../4T4HA1KMO10S6311 = AuditBaseline ('Sets up resources to monitor security and compliance of accounts in your organization.'), .../LN25R72TTG6IGPTQ = IdentityCenterBaseline ('Sets up shared resources for AWS Identity Center, which prepares the AWSControlTowerBaseline to set up Identity Center access for accounts.') and .../17BSJV3IGJ2QSGA2 = AWSControlTowerBaseline ('Sets up resources and mandatory controls for member accounts within the target OU, required for AWS Control Tower governance.'). In those ARNs only the partition and region segments are request/environment derived and the account segment is empty for AWS-managed baselines; the 16-character uppercase id segment is fixed per baseline, and the description strings are wholly fixed text with no request- or state-derived parts.
Emulator: the call answers 200, but a lookup keyed on each AWS baseline ARN finds nothing for arn:aws:controltower:us-west-2::baseline/A6EEAE10F08193F2, .../J8HX46AHS5MIKQPD, .../3WPD0NA6TJ9AOMU2 and the other AWS id segments - reported missing - because the emulator mints its own id segments (e.g. .../2V6DZ8QKR4JT7MWN, .../WM8FJ6YHL3QS5XKB, .../TR7NC4VPG9DH2KLM, .../QXK3R9ZB7PN5TD2H, .../1QBGH2G48YVGDQ3Y, .../YX7VMZML5IG8EJUD, .../H6C5JFCJJ3CPU3J5, .../APO9ATVPBKFRRGLK). The few ARNs that do exist carry re-written descriptions: .../4T4HA1KMO10S6311 is described as 'Sets up the cross-account audit resources AWS Control Tower requires in the audit account.', .../LN25R72TTG6IGPTQ as 'Sets up the AWS IAM Identity Center resources that are a prerequisite for the AWSControlTowerBaseline.' and .../17BSJV3IGJ2QSGA2 as 'Sets up the resources and permissions AWS Control Tower requires to govern the accounts of a target organizational unit.'
Why: a client that embeds a documented baseline ARN (templates, landing-zone automation, policy checks) cannot find that baseline in the listing at all, so EnableBaseline/GetBaseline on it fails or - worse, since the emulator's ARN-to-name mapping differs too - resolves to a different baseline and the wrong baseline is applied to a target OU. Anything that surfaces the catalog to a user renders description text AWS never emits.
Fix: the baseline catalog seed (the BASELINE_CATALOG constant in the controltower crate's state module) that ListBaselines projects - the per-baseline id segments and description strings must be AWS's, not generated or re-worded - and the ARN-keyed lookup GetBaseline/EnableBaseline perform against it.
Traces: controltower-catalog-list-baselines#task#1.

Related open issue: '20261029T183207Z-controltower-control-tower-baseline-catalog-diverges-wrong-descriptions-wrong.md' describes the same catalog seed from the name-keyed side, but the replay reports this ARN-keyed cluster as unowned (its replay key is not carried by that issue), so it is filed here and must be re-replayed in its own right.

## Evidence
Replay of trace set simparity-wire-aws-v1 (service controltower, 1 unowned cluster; 7 of 20 steps mismatched, score 65%).

ListBaselines - fixture controltower-catalog-list-baselines, step task#1, expected status 200 / actual status 200:
- $.baselines[arn="arn:aws:controltower:us-west-2::baseline/A6EEAE10F08193F2"]: missing (expected {arn: arn:aws:controltower:us-west-2::baseline/A6EEAE10F08193F2, name: CentralSecurityRolesBaseline, description: 'Sets up central resources for security monitoring within your organization.'}, got None)
- $.baselines[arn="arn:aws:controltower:us-west-2::baseline/J8HX46AHS5MIKQPD"]: missing (expected name LogArchiveBaseline, description 'Sets up a central repository for logs of API activities and resource configurations from accounts in your organization.', got None)
- $.baselines[arn="arn:aws:controltower:us-west-2::baseline/3WPD0NA6TJ9AOMU2"]: missing (expected name BackupCentralVaultBaseline, description 'Sets up a central AWS Backup vault in your organization.', got None)
- $.baselines[arn="arn:aws:controltower:us-west-2::baseline/4T4HA1KMO10S6311"].description: expected 'Sets up resources to monitor security and compliance of accounts in your organization.', got 'Sets up the cross-account audit resources AWS Control Tower requires in the audit account.'
- $.baselines[arn="arn:aws:controltower:us-west-2::baseline/LN25R72TTG6IGPTQ"].description: expected 'Sets up shared resources for AWS Identity Center, which prepares the AWSControlTowerBaseline to set up Identity Center access for accounts.', got 'Sets up the AWS IAM Identity Center resources that are a prerequisite for the AWSControlTowerBaseline.'
- $.baselines[arn="arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2"].description: expected 'Sets up resources and mandatory controls for member accounts within the target OU, required for AWS Control Tower governance.', got 'Sets up the resources and permissions AWS Control Tower requires to govern the accounts of a target organizational unit.'
- additional emulator-only ARNs appear in place of the AWS ones: .../2V6DZ8QKR4JT7MWN, .../WM8FJ6YHL3QS5XKB, .../TR7NC4VPG9DH2KLM, .../QXK3R9ZB7PN5TD2H, .../1QBGH2G48YVGDQ3Y, .../YX7VMZML5IG8EJUD, .../H6C5JFCJJ3CPU3J5, .../APO9ATVPBKFRRGLK

Divergence key: service=controltower|ListBaselines|status=200->200|keys=$.baselines[arn=...].description,$.baselines[arn=...] missing. Smithy cited: ListBaselines http POST /list-baselines code 200, ListBaselinesOutput.baselines required; BaselineArn @pattern ^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$; GetBaseline.baselineIdentifier required BaselineArn.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1, fixture controltower-catalog-list-baselines, step task#1 (POST /list-baselines with the recorded ListBaselines input).
3. Index the returned baselines[] by its arn member and look up the AWS baseline ARNs recorded in the expected response - e.g. arn:aws:controltower:us-west-2::baseline/A6EEAE10F08193F2 (CentralSecurityRolesBaseline), .../J8HX46AHS5MIKQPD (LogArchiveBaseline), .../3WPD0NA6TJ9AOMU2 (BackupCentralVaultBaseline). None of them is present; the response instead lists emulator-minted id segments.
4. For the ARNs that are present (.../4T4HA1KMO10S6311, .../LN25R72TTG6IGPTQ, .../17BSJV3IGJ2QSGA2) compare the description member against the recorded text - each is emulator-authored prose, not AWS's fixed description.
5. Call GetBaseline with one of the AWS ARNs from step 3 to confirm the catalog cannot be addressed by its AWS identifiers.
