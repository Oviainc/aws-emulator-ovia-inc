Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-parameter-group-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-parameter-group-lifecycle redshift-hsm-resources-lifecycle redshift-snapshot-copy-grant-and-authentication-profile redshift-authentication-profile-lifecycle
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|CreateClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeClusterParameters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeleteClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeClusterParameterGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeHsmClientCertificates|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeleteHsmClientCertificate|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CreateSnapshotCopyGrant|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeleteSnapshotCopyGrant|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DeleteAuthenticationProfile|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|ModifyAuthenticationProfile|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|ModifyAuthenticationProfile|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|ModifyAuthenticationProfile|4xx->4xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift parameter-group, HSM, snapshot-copy-grant and authentication-profile refusals are emulator-authored

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateClusterParameterGroup

## Description
10 root causes across four configuration-resource families. Nine are message-text divergences on the correct status/code; cause 10 is a wrong error class, wrong status and wrong message because a validation gate is missing entirely.

Cause 1 of 10: CreateClusterParameterGroup for a name that already exists. Smithy: ClusterParameterGroupAlreadyExistsFault @error("client"), httpError 400, awsQueryError ClusterParameterGroupAlreadyExists. AWS: 400, Message "Parameter group <ParameterGroupName> already exists" — fixed text "Parameter group " + " already exists", no trailing period, variable part is the request's ParameterGroupName. Emulator: "Cluster parameter group already exists: <ParameterGroupName>". Why: idempotency logic that matches AWS's conflict text treats the conflict as an unknown error. Fix: CreateClusterParameterGroup handler's duplicate-name refusal path. Traces: redshift-parameter-group-lifecycle#task#8.

Cause 2 of 10: DescribeClusterParameters for an unknown group. Smithy: ClusterParameterGroupNotFoundFault httpError 404. AWS: 404, "ClusterParameterGroup not found: <ParameterGroupName>" — note AWS uses the CamelCase shape name "ClusterParameterGroup" here. Emulator: "Cluster parameter group not found: <ParameterGroupName>". Fix: DescribeClusterParameters handler refusal path. Traces: redshift-parameter-group-lifecycle#task#9.

Cause 3 of 10: DeleteClusterParameterGroup for an unknown group. AWS: 404 ClusterParameterGroupNotFound, "ParameterGroup not found: <ParameterGroupName>" — a THIRD spelling ("ParameterGroup", not "ClusterParameterGroup"). Emulator: "Cluster parameter group not found: <ParameterGroupName>". Fix: DeleteClusterParameterGroup handler refusal path. Traces: redshift-parameter-group-lifecycle#task#10.

Cause 4 of 10: DescribeClusterParameterGroups filtered by an unknown group name. AWS: 404 ClusterParameterGroupNotFound, "ClusterParameterGroup <ParameterGroupName> not found." — identifier EMBEDDED and a trailing period, different again. Emulator: "Cluster parameter group not found: <ParameterGroupName>". Fix: DescribeClusterParameterGroups handler refusal path. Traces: redshift-parameter-group-lifecycle#teardown#12.

Cause 5 of 10: DescribeHsmClientCertificates filtered by an unknown certificate id. Smithy: HsmClientCertificateNotFoundFault @error("client"), httpError 400. AWS: 400, "HSM client certificate '<HsmClientCertificateIdentifier>' not found." — fixed text uses the human spelling "HSM client certificate" and SINGLE-QUOTES the identifier. Emulator: "HsmClientCertificate not found: <id>" (shape name, unquoted, no period). Fix: DescribeHsmClientCertificates handler refusal path. Traces: redshift-hsm-resources-lifecycle#task#6.

Cause 6 of 10: DeleteHsmClientCertificate for an unknown certificate id. AWS: 400 HsmClientCertificateNotFoundFault, "HSM client certificate not found: <HsmClientCertificateIdentifier>" — human spelling, colon form, unquoted, no period. Emulator: "HsmClientCertificate not found: <id>". Fix: DeleteHsmClientCertificate handler refusal path. Traces: redshift-hsm-resources-lifecycle#task#9.

Cause 7 of 10: CreateSnapshotCopyGrant for a grant name that already exists. Smithy: SnapshotCopyGrantAlreadyExistsFault @error("client"), httpError 400. AWS: 400, "Cannot create the snapshot copy grant because a grant with the identifier '<SnapshotCopyGrantName>' already exists." — the fixed text says "with the identifier" and single-quotes the name. Emulator: "Cannot create the snapshot copy grant because a grant with the name <SnapshotCopyGrantName> already exists." (says "the name", unquoted). Fix: CreateSnapshotCopyGrant handler's duplicate-name refusal path. Traces: redshift-snapshot-copy-grant-and-authentication-profile#task#3.

Cause 8 of 10: DeleteSnapshotCopyGrant for an unknown grant. AWS: 400 SnapshotCopyGrantNotFoundFault, "Snapshot copy grant not found: '<SnapshotCopyGrantName>'." — colon form, name single-quoted, trailing period. Emulator: "Snapshot copy grant '<name>' not found." (name embedded before "not found"). Fix: DeleteSnapshotCopyGrant handler refusal path. Traces: redshift-snapshot-copy-grant-and-authentication-profile#task#4.

Cause 9 of 10: DeleteAuthenticationProfile for an unknown profile. Smithy: AuthenticationProfileNotFoundFault httpError 404. AWS: 404, fixed message with NO variable part: "The authentication profile does not exist. Check the name and try the operation again." Emulator: "Authentication profile not found: <AuthenticationProfileName>", echoing a value AWS does not echo. Fix: DeleteAuthenticationProfile handler refusal path. Traces: redshift-authentication-profile-lifecycle#task#6.

Cause 10 of 10: ModifyAuthenticationProfile called with an AuthenticationProfileContent that is not valid JSON (for a profile name that also does not exist). Smithy: ModifyAuthenticationProfile declares both InvalidAuthenticationProfileRequestFault (httpError 400) and AuthenticationProfileNotFoundFault (httpError 404); AWS validates the content document FIRST and refuses 400 InvalidAuthenticationProfileRequestFault with the fixed message "The profile map file doesn't contain valid content in JSON format. Edit the map file to add valid content in JSON format or specify another map file that contains valid content in JSON format. You can include a path to the file. Instead of a file, you can specify a JSON string." The emulator performs no content validation at all and falls through to the name lookup, answering 404 AuthenticationProfileNotFoundFault "Authentication profile not found: <name>". Why: a client submitting a malformed profile document is told the profile is missing, so it retries creation instead of fixing the document; the status class (400 permanent input error vs 404 not-found) is also wrong. Fix: add AuthenticationProfileContent JSON validation to ModifyAuthenticationProfile's input validation, ahead of the profile lookup. Traces: redshift-authentication-profile-lifecycle#task#5.

## Evidence
Replay of simparity-wire-aws-v1; all ten clusters verdict=mismatch.

CreateClusterParameterGroup (redshift-parameter-group-lifecycle#task#8), 400 vs 400:
  expected 'Parameter group sim2real-probe-redshift-sim2real-probe-pg-51626322 already exists', got 'Cluster parameter group already exists: sim2real-probe-redshift-sim2real-probe-pg-51626322'
DescribeClusterParameters (#task#9), 404 vs 404: expected 'ClusterParameterGroup not found: sim2real-probe-pg-missing', got 'Cluster parameter group not found: sim2real-probe-pg-missing'
DeleteClusterParameterGroup (#task#10), 404 vs 404: expected 'ParameterGroup not found: sim2real-probe-pg-missing', got 'Cluster parameter group not found: sim2real-probe-pg-missing'
DescribeClusterParameterGroups (#teardown#12), 404 vs 404: expected 'ClusterParameterGroup sim2real-probe-redshift-sim2real-probe-pg-51626322 not found.', got 'Cluster parameter group not found: sim2real-probe-redshift-sim2real-probe-pg-51626322'
DescribeHsmClientCertificates (redshift-hsm-resources-lifecycle#task#6), 400 vs 400: expected "HSM client certificate 'sim2real-probe-hsmcert-missing' not found.", got 'HsmClientCertificate not found: sim2real-probe-hsmcert-missing'
DeleteHsmClientCertificate (#task#9), 400 vs 400: expected 'HSM client certificate not found: sim2real-probe-hsmcert-missing', got 'HsmClientCertificate not found: sim2real-probe-hsmcert-missing'
CreateSnapshotCopyGrant (redshift-snapshot-copy-grant-and-authentication-profile#task#3), 400 vs 400: expected "Cannot create the snapshot copy grant because a grant with the identifier 'sim2real-probe-redshift-scg-d10e8098' already exists.", got 'Cannot create the snapshot copy grant because a grant with the name sim2real-probe-redshift-scg-d10e8098 already exists.'
DeleteSnapshotCopyGrant (#task#4), 400 vs 400: expected "Snapshot copy grant not found: 'sim2real-probe-scg-missing'.", got "Snapshot copy grant 'sim2real-probe-scg-missing' not found."
DeleteAuthenticationProfile (redshift-authentication-profile-lifecycle#task#6), 404 vs 404: expected 'The authentication profile does not exist. Check the name and try the operation again.', got 'Authentication profile not found: sim2real-probe-ap-missing'
ModifyAuthenticationProfile (redshift-authentication-profile-lifecycle#task#5):
  status: expected 400, got 404
  Error.Code: expected 'InvalidAuthenticationProfileRequestFault', got 'AuthenticationProfileNotFoundFault'
  Error.Message: expected "The profile map file doesn't contain valid content in JSON format. Edit the map file to add valid content in JSON format or specify another map file that contains valid content in JSON format. You can include a path to the file. Instead of a file, you can specify a JSON string.", got 'Authentication profile not found: sim2real-probe-ap-missing'

Group/certificate/grant/profile names shown are request values; the fixed wording, quoting and punctuation around them are the requirements.

## Steps to Reproduce
1. Replay fixture redshift-parameter-group-lifecycle; inspect task#8 (duplicate CreateClusterParameterGroup), task#9, task#10 and teardown#12 (unknown parameter-group name).
2. Replay fixture redshift-hsm-resources-lifecycle; inspect task#6 and task#9 (unknown HsmClientCertificateIdentifier).
3. Replay fixture redshift-snapshot-copy-grant-and-authentication-profile; inspect task#3 (duplicate grant name) and task#4 (unknown grant).
4. Replay fixture redshift-authentication-profile-lifecycle; inspect task#6 (DeleteAuthenticationProfile, unknown profile) and task#5 (ModifyAuthenticationProfile with non-JSON AuthenticationProfileContent) and compare status, Error.Code and Error.Message.
