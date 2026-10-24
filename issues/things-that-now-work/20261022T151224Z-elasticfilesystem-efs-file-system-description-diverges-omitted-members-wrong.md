Issue-Source: trace-sweep-divergence
Issue-Trace: efs-access-point-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: efs-access-point-lifecycle efs-file-system-policy-workflow efs-lifecycle-configuration-workflow efs-tags-workflow efs-filesystem-lifecycle
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: elasticfilesystem|CreateFileSystem|2xx->2xx|$.Encrypted
Issue-Key: elasticfilesystem|CreateFileSystem|2xx->2xx|$.FileSystemProtection
Issue-Key: elasticfilesystem|CreateFileSystem|2xx->2xx|$.SizeInBytes.Timestamp
Issue-Key: elasticfilesystem|CreateFileSystem|2xx->2xx|$.SizeInBytes.ValueInArchive
Issue-Key: elasticfilesystem|CreateFileSystem|2xx->2xx|$.KmsKeyId
Issue-Key: elasticfilesystem|DescribeFileSystems|2xx->2xx|$.FileSystems[Name="sim2real-probe-efs-fs-lifecycle-93866a91"].FileSystemProtection
Issue-Key: elasticfilesystem|DescribeFileSystems|2xx->2xx|$.FileSystems[Name="sim2real-probe-efs-fs-lifecycle-93866a91"].KmsKeyId
Issue-Key: elasticfilesystem|DescribeFileSystems|2xx->2xx|$.FileSystems[Name="sim2real-probe-efs-fs-lifecycle-93866a91"].SizeInBytes.Timestamp
Issue-Key: elasticfilesystem|DescribeFileSystems|2xx->2xx|$.FileSystems[Name="sim2real-probe-efs-fs-lifecycle-93866a91"].SizeInBytes.ValueInArchive
Issue-Key: elasticfilesystem|DescribeFileSystems|2xx->2xx|$.FileSystems[Name="sim2real-probe-efs-fs-lifecycle-93866a91"].ThroughputMode
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# EFS file-system description diverges: omitted members, wrong KmsKeyId, unpersisted ThroughputMode

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFileSystem

## Description
The emulator's FileSystemDescription payload (shared by CreateFileSystem and DescribeFileSystems) is incomplete and partly wrong, and a throughput-mode mutation is not retained in state. Five causes, all on service elasticfilesystem (EFS, Smithy sdkId EFS / sigv4Name elasticfilesystem, api 2015-02-01, restJson1). THIS ISSUE HAS 5 CAUSES; all five must be fixed before it is closed.

Cause 1 of 5 - CreateFileSystem response omits Encrypted, FileSystemProtection and SizeInBytes.ValueInArchive and invents SizeInBytes.Timestamp (fixture efs-access-point-lifecycle, step setup#1).
AWS: Smithy FileSystemDescription (CreateFileSystem output_shape, http 201) models Encrypted (boolean), FileSystemProtection -> FileSystemProtectionDescription{ReplicationOverwriteProtection}, and SizeInBytes -> FileSystemSize{Value(required), Timestamp, ValueInIA, ValueInStandard, ValueInArchive}. FileSystemProtectionDescription's doc states "Replication overwrite protection is ENABLED by default". The recording shows AWS serializing Encrypted even when the file system is unencrypted (recorded value false), FileSystemProtection as {"ReplicationOverwriteProtection": "ENABLED"}, SizeInBytes.ValueInArchive as a number (recorded 0), and NO SizeInBytes.Timestamp on a freshly created file system.
Emulator: Encrypted absent, FileSystemProtection absent, SizeInBytes.ValueInArchive absent, and SizeInBytes.Timestamp present (type_mismatch: expected absent, emulator emitted an epoch-seconds value).
Why: a client that reads fs.Encrypted or fs.FileSystemProtection.ReplicationOverwriteProtection straight off a create response gets null/None instead of a value, so encryption and replication-overwrite-protection checks silently take the "unknown" branch; archive-tier size accounting reads as missing rather than zero; and a Timestamp on a brand-new file system makes the metered size look already measured.
Fix: the EFS FileSystemDescription serializer - it must always emit Encrypted and FileSystemProtection, emit the full FileSystemSize member set including ValueInArchive, and emit SizeInBytes.Timestamp only when a metering time actually exists in the stored file-system record.
Traces: efs-access-point-lifecycle#setup#1.

Cause 2 of 5 - the same three omissions recur on every other create path (cluster representative efs-file-system-policy-workflow#setup#1; cluster also covers efs-lifecycle-configuration-workflow#setup#1 and efs-tags-workflow#setup#1).
AWS: as Cause 1 - Encrypted, FileSystemProtection and SizeInBytes.ValueInArchive are present on the recorded 201 FileSystemDescription.
Emulator: 201 with Encrypted missing, FileSystemProtection missing, SizeInBytes.ValueInArchive missing (SizeInBytes.Timestamp matched here, so the defect is the omission set, not the timestamp).
Why: the gap is not fixture-specific - every workflow that creates a file system then inspects it is handed an under-populated description, so policy, lifecycle-configuration and tagging flows all build on a partial resource view.
Fix: same EFS FileSystemDescription serializer as Cause 1; the members must be unconditional rather than populated only on some code paths.
Traces: efs-file-system-policy-workflow#setup#1, efs-lifecycle-configuration-workflow#setup#1, efs-tags-workflow#setup#1.

Cause 3 of 5 - on an encrypted file system CreateFileSystem returns a KMS ALIAS ARN where the recording asserts a resolved key identifier, plus the Cause 1 omissions (fixture efs-filesystem-lifecycle, step setup#1).
AWS: Smithy FileSystemDescription.KmsKeyId doc: "The ID of an KMS key used to protect the encrypted file system", @length 0..2048 with a pattern admitting a bare key id, mrk-, alias/NAME or a kms ARN. Recorded behavior only: the recording shape-asserts KmsKeyId (expected '<shaped-not-asserted>'), i.e. the recorded value is a concrete resolved key identifier, and the emulator's value fails that shape assertion. Hypothesis: AWS resolves the service default EFS key to its key ARN (arn:aws:kms:<region>:<account-id>:key/<uuid>) rather than echoing the alias.
Emulator: KmsKeyId = 'arn:aws:kms:us-west-2:000000000000:alias/aws/elasticfilesystem' - an alias ARN, which is what fails the recorded shape - together with FileSystemProtection missing, SizeInBytes.ValueInArchive missing and an invented SizeInBytes.Timestamp.
Why: a client that passes the returned KmsKeyId to KMS (DescribeKey, grant and policy checks) or compares it against a configured CMK ARN cannot resolve or match an alias ARN where a key identifier is expected, so key-pinning and compliance assertions on encrypted file systems fail against the emulator.
Fix: the EFS CreateFileSystem handler's key-resolution step - resolve the default/requested key to its concrete key identifier when storing the file-system record, instead of persisting the alias ARN - plus the serializer fixes from Cause 1.
Traces: efs-filesystem-lifecycle#setup#1.

Cause 4 of 5 - DescribeFileSystems reproduces the same mis-serialized description on read (fixture efs-filesystem-lifecycle, step task#2).
AWS: DescribeFileSystems (GET /2015-02-01/file-systems, 200) returns FileSystems: list<FileSystemDescription>, the same shape as the create response, so each element carries FileSystemProtection, the resolved KmsKeyId and the full SizeInBytes member set.
Emulator: 200, but the listed file system has FileSystemProtection missing, KmsKeyId = the alias ARN, SizeInBytes.ValueInArchive missing and SizeInBytes.Timestamp populated.
Why: the defect is not confined to the create response - a client that re-reads the file system to confirm its configuration sees the same holes, so there is no path by which it can ever observe protection state or a usable key id.
Fix: have DescribeFileSystems (and every other file-system read) share one corrected FileSystemDescription serializer and read the corrected stored record, so create and describe cannot drift.
Traces: efs-filesystem-lifecycle#task#2.

Cause 5 of 5 - a throughput-mode change made earlier in the fixture is not reflected by a later DescribeFileSystems (fixture efs-filesystem-lifecycle, step task#5).
AWS: Smithy ThroughputMode is an enum with values bursting | provisioned | elastic, and FileSystemDescription.ThroughputMode "Displays the file system's throughput mode". The recording shows the same file system reported as bursting at task#2 and as elastic at task#5, i.e. the throughput-mode mutation performed in between is durable and observable on the next read.
Emulator: at task#5 ThroughputMode is still 'bursting' (value_mismatch, expected 'elastic'), while the FileSystemProtection / KmsKeyId / SizeInBytes divergences of Cause 4 persist on the same element.
Why: the mutation is accepted but dropped, so a client that updates throughput mode and then polls the file system never observes the new mode - a read-after-write that loops until ThroughputMode flips never terminates, and configuration-drift checks report the wrong mode.
Fix: the EFS file-system state - apply the throughput-mode update to the stored file-system record (and keep ProvisionedThroughputInMibps consistent with it) so subsequent reads serve the mutated value.
Traces: efs-filesystem-lifecycle#task#5 (contrast with the matching bursting value at efs-filesystem-lifecycle#task#2).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0), 5 divergence clusters, all verdict=mismatch, service=elasticfilesystem, no stub_path and no owned_by:

1) efs-access-point-lifecycle#setup#1 CreateFileSystem expected 201 / actual 201:
   $.Encrypted: missing (expected False, got None)
   $.FileSystemProtection: missing (expected {'ReplicationOverwriteProtection': 'ENABLED'}, got None)
   $.SizeInBytes.Timestamp: type_mismatch (expected None, got 1792681717)
   $.SizeInBytes.ValueInArchive: missing (expected 0, got None)

2) efs-file-system-policy-workflow#setup#1 (also efs-lifecycle-configuration-workflow#setup#1, efs-tags-workflow#setup#1) CreateFileSystem 201/201:
   $.Encrypted: missing (expected False, got None)
   $.FileSystemProtection: missing (expected {'ReplicationOverwriteProtection': 'ENABLED'}, got None)
   $.SizeInBytes.ValueInArchive: missing (expected 0, got None)

3) efs-filesystem-lifecycle#setup#1 CreateFileSystem 201/201:
   $.FileSystemProtection: missing (expected {'ReplicationOverwriteProtection': 'ENABLED'}, got None)
   $.KmsKeyId: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:kms:us-west-2:000000000000:alias/aws/elasticfilesystem')
   $.SizeInBytes.Timestamp: type_mismatch (expected None, got 1792681735)
   $.SizeInBytes.ValueInArchive: missing (expected 0, got None)

4) efs-filesystem-lifecycle#task#2 DescribeFileSystems 200/200, on $.FileSystems[Name="sim2real-probe-efs-fs-lifecycle-93866a91"]:
   .FileSystemProtection: missing (expected {'ReplicationOverwriteProtection': 'ENABLED'}, got None)
   .KmsKeyId: value_mismatch (expected '<shaped-not-asserted>', got 'arn:aws:kms:us-west-2:000000000000:alias/aws/elasticfilesystem')
   .SizeInBytes.Timestamp: type_mismatch (expected None, got 1792681735)
   .SizeInBytes.ValueInArchive: missing (expected 0, got None)

5) efs-filesystem-lifecycle#task#5 DescribeFileSystems 200/200, same element:
   .ThroughputMode: value_mismatch (expected 'elastic', got 'bursting')
   plus the four divergences listed under (4)

Model consulted: services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json - FileSystemDescription, FileSystemSize, FileSystemProtectionDescription, ThroughputMode enum (bursting|provisioned|elastic), CreateFileSystem (POST /2015-02-01/file-systems, 201), DescribeFileSystems (GET /2015-02-01/file-systems, 200). Generated ids, timestamps, hosts and probe names above are environment-specific examples, not requirements.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Representative step: fixture efs-access-point-lifecycle, step setup#1 - CreateFileSystem (POST /2015-02-01/file-systems) with a fresh CreationToken; compare the 201 FileSystemDescription against the recording: Encrypted, FileSystemProtection and SizeInBytes.ValueInArchive are absent and SizeInBytes.Timestamp is present.
4. Repeat on efs-file-system-policy-workflow#setup#1 (same omissions) and on efs-filesystem-lifecycle#setup#1, where KmsKeyId additionally comes back as the alias ARN arn:aws:kms:<region>:<account>:alias/aws/elasticfilesystem instead of a resolved key identifier.
5. Replay efs-filesystem-lifecycle#task#2 (DescribeFileSystems) - the same description defects appear on the read path.
6. Replay efs-filesystem-lifecycle#task#5 (DescribeFileSystems after the fixture's throughput-mode change) - ThroughputMode is still 'bursting' where the recording reports 'elastic'.
7. All five clusters report verdict=mismatch with matching HTTP status, so the divergence is purely in the payload/state, not the routing.
