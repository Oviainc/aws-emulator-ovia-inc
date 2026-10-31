Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-certificates
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-certificates rds-certificate-management rds-maintenance-and-account-attributes rds-option-group-add-remove-option rds-option-group-mutation-envelopes
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeCertificates|2xx->2xx|$.DescribeCertificatesResponse.DescribeCertificatesResult.Certificates.Certificate[*]
Issue-Key: rds|ModifyCertificates|2xx->2xx|$.ModifyCertificatesResponse.ModifyCertificatesResult.Certificate.Thumbprint
Issue-Key: rds|DescribeAccountAttributes|2xx->2xx|$.DescribeAccountAttributesResponse.DescribeAccountAttributesResult.AccountQuotas.AccountQuota[*]
Issue-Key: rds|ModifyOptionGroup|2xx->2xx|$.ModifyOptionGroupResponse.ModifyOptionGroupResult.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].Description
Issue-Key: rds|DescribeOptionGroups|2xx->2xx|$.DescribeOptionGroupsResponse.DescribeOptionGroupsResult.OptionGroupsList.OptionGroup.Options.Option.OptionSettings.OptionSetting[*].Description
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS static reference catalogs diverge: certificates, account quotas, option-setting descriptions

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificates

## Description
Three static reference tables the emulator ships — the RDS CA certificate list, the account-quota list and the option-setting descriptions — do not match the recorded service. All calls answer 200; only the catalog contents differ. 6 causes.

Cause 1 of 6: DescribeCertificates.
AWS: recorded behavior only — the certificate list is ordered rds-ca-ecc384-g1, rds-ca-rsa4096-g1, rds-ca-rsa2048-g1, ... and each Certificate carries CertificateIdentifier, CertificateType 'CA', CertificateArn, CustomerOverride, ValidFrom, ValidTill and a Thumbprint.
Emulator: a different list in a different order (rds-ca-2019 first), different ValidFrom/ValidTill instants, and no Thumbprint member at all.
Why: a client enumerating CAs to pin a CACertificateIdentifier sees identifiers and validity windows RDS does not publish, and cannot read a thumbprint.
Fix: the RDS certificate catalog data and the DescribeCertificates serializer (add Thumbprint).
Traces: rds-catalog-describe-certificates#task#1.

Cause 2 of 6: ModifyCertificates.
AWS: the returned Certificate carries a Thumbprint.
Emulator: Thumbprint is absent.
Why: same missing member on the mutation path.
Fix: the ModifyCertificates response serializer.
Traces: rds-certificate-management#task#2.

Cause 3 of 6: DescribeAccountAttributes.
AWS: recorded behavior only — the AccountQuota list includes DBParameterGroups (Max 50), DBClusterParameterGroups (Max 50) and Integrations (Max 100) at the recorded positions, each with AccountQuotaName, Max and Used.
Emulator: those quota rows are absent (the list is shorter and differently ordered — the emulator has DBClusters where the recording has DBParameterGroups, ReadReplicasPerMaster where it has DBClusterParameterGroups, and nothing at all where it has Integrations).
Why: a client that reads a named quota to decide whether a create will fit cannot find it.
Fix: the RDS account-quota table behind DescribeAccountAttributes.
Traces: rds-maintenance-and-account-attributes#task#1.

Cause 4 of 6: ModifyOptionGroup adding an option (MARIADB_AUDIT_PLUGIN-style settings).
AWS: recorded behavior only — each OptionSetting's Description is the recorded sentence for that setting name, in the recorded order.
Emulator: the Description values are shifted/misassigned relative to the setting names (index 3 gets the query-length description where AWS has the uninstall-protection one, and so on).
Why: a client surfacing option-setting help text shows the wrong description for the setting.
Fix: the option-setting catalog behind the OptionGroup serializer — bind each Description to its own setting name.
Traces: rds-option-group-add-remove-option#task#2.

Cause 5 of 6: DescribeOptionGroups after that modify.
AWS: the same per-setting Descriptions on the read path.
Emulator: the same misassignment.
Why: same.
Fix: same catalog.
Traces: rds-option-group-add-remove-option#task#3.

Cause 6 of 6: ModifyOptionGroup in the option-group envelope fixture.
AWS: same per-setting Descriptions, at the indices that fixture's option produces.
Emulator: misassigned at indices 2, 4, 5 and 7.
Why: same.
Fix: same catalog.
Traces: rds-option-group-mutation-envelopes#task#5.

## Evidence
Replay cluster details (status 200 matched throughout):
1. rds-catalog-describe-certificates#task#1: Certificate[0] expected {CertificateIdentifier 'rds-ca-ecc384-g1', ValidTill '2121-05-24T23:06:59Z', Thumbprint '2ee3dcc06e50192559b13929e73484354f23387d', ValidFrom '2021-05-24T22:06:59Z', CertificateType 'CA', CertificateArn arn:aws:rds:us-west-2::cert:rds-ca-ecc384-g1}, got {CertificateIdentifier 'rds-ca-2019', ValidFrom '2019-08-22T00:00:00Z', ValidTill '2024-08-22T00:00:00Z', no Thumbprint}; rows [0]..[3] diverge.
2. rds-certificate-management#task#2 ModifyCertificates: Certificate.Thumbprint missing (expected '2ee3dcc06e50192559b13929e73484354f23387d').
3. rds-maintenance-and-account-attributes#task#1 DescribeAccountAttributes: AccountQuota[5] expected {AccountQuotaName 'DBParameterGroups', Max '50'} got {'DBClusters', Max '40'}; AccountQuota[15] expected {'DBClusterParameterGroups', Max '50'} got {'ReadReplicasPerMaster', Max '15'}; AccountQuota[18] expected {'Integrations', Max '100'} got None.
4. rds-option-group-add-remove-option#task#2 ModifyOptionGroup: OptionSetting[3].Description expected 'This setting is enabled for security and ensures that the plugin cannot be uninstalled at runtime' got 'Limit on the length of the query string in a record.'; [5] expected 'Number of log rotations to save' got 'Path to the log file.'; [6] expected 'File location for logs' got 'Types of actions to be logged (connect, query, or table)'; [7] expected 'Size limit of a log file before rotating' got 'Exclude specified users'.
5. rds-option-group-add-remove-option#task#3 DescribeOptionGroups: the same four Description mismatches at the same indices.
6. rds-option-group-mutation-envelopes#task#5 ModifyOptionGroup: the same misassignment at OptionSetting[2], [4], [5], [7].
(Thumbprints, ARNs and validity instants quoted here are the recorded catalog's contents, which is what these operations must publish.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-catalog-describe-certificates#task#1 and every step in trace_ids; each reads one of the three static catalogs and diffs it against the recording.
