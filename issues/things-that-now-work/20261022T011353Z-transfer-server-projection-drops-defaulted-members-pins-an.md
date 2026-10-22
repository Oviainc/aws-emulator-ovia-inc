Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-custom-identity-provider-server-config
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-custom-identity-provider-server-config transfer-multiple-servers-isolation transfer-server-lifecycle-start-stop-delete transfer-server-security-policy-and-tags
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|DescribeServer|2xx->2xx|$.Server.HostKeyFingerprint
Issue-Key: transfer|DescribeServer|2xx->2xx|$.Server.PreAuthenticationLoginBanner
Issue-Key: transfer|DescribeServer|2xx->2xx|$.Server.ProtocolDetails
Issue-Key: transfer|DescribeServer|2xx->2xx|$.Server.S3StorageOptions
Issue-Key: transfer|DescribeServer|2xx->2xx|$.Server.SecurityPolicyName
Issue-Key: transfer|DescribeServer|2xx->2xx|$.Server.Tags
Issue-Key: transfer|DescribeServer|2xx->2xx|$
Issue-Key: transfer|DescribeServer|2xx->2xx|$.Server.State
Issue-Commit: 872e0ddf6cb614931de58689afe8393c02abd052

# Transfer server projection drops defaulted members, pins an old SecurityPolicyName, and never reaches OFFLINE

> _Note: this issue was identified against the code base at commit `872e0ddf6cb614931de58689afe8393c02abd052`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServer

## Description
DescribeServer answers 200 but the DescribedServer it projects is incomplete and partly wrong: service-computed and defaulted members are absent, the default SecurityPolicyName is an old value, a member supplied at CreateServer is dropped, and the server's State never leaves ONLINE after StopServer. 4 divergence clusters, one projection/state-tracking defect family in the transfer server model. No open transfer issue covers this.

Cause 1 of 4 — defaulted and computed DescribedServer members are omitted, and an input member is dropped.
AWS: Smithy: DescribedServer members HostKeyFingerprint (@cfnExcludeProperty, 'Base64-encoded SHA256 fingerprint of the server's host key'), ProtocolDetails, S3StorageOptions, SecurityPolicyName, Tags, PreAuthenticationLoginBanner (@length 0..4096). Recorded behavior for a SERVICE_MANAGED/S3/PUBLIC server: HostKeyFingerprint present (value environment-specific, asserted only as present), ProtocolDetails defaulted to {PassiveIp AUTO, ProxyConfig {SftpMode NONE}, SetStatOption DEFAULT, SftpPorts [{CommunicationMode SERVER_TALK_FIRST, SftpPort 22}], TlsSessionResumptionMode ENFORCED}, S3StorageOptions defaulted to {DirectoryListingOptimization DISABLED}, Tags present as an empty list when none were set, and PreAuthenticationLoginBanner echoed back exactly as passed to CreateServer.
Emulator: all of HostKeyFingerprint, ProtocolDetails, S3StorageOptions and Tags are absent from $.Server, and PreAuthenticationLoginBanner supplied at CreateServer is absent too (the create input is not persisted).
Why: SDK clients reading server configuration get null for protocol/storage/banner settings they just configured, so configuration round-trips and drift detection report the server as unconfigured; code that fingerprints the host key has nothing to read.
Fix: the transfer DescribeServer response projection plus the CreateServer handler's state write (defaults must be materialized at create time and optional banner/detail members retained).
Traces: transfer-custom-identity-provider-server-config#task#4.

Cause 2 of 4 — same omissions on a plain SERVICE_MANAGED server, plus a stale default SecurityPolicyName.
AWS: Smithy: SecurityPolicyName @length 0..100 @pattern ^Transfer[A-Za-z0-9]*SecurityPolicy-[A-Za-z0-9-]+$. Recorded behavior: a server created without SecurityPolicyName reports 'TransferSecurityPolicy-2024-01'.
Emulator: reports 'TransferSecurityPolicy-2018-11-05', and again omits HostKeyFingerprint, ProtocolDetails and S3StorageOptions.
Why: a client that reads back the policy to decide whether it must call UpdateServer sees a policy the account no longer defaults to, so it takes the wrong branch; TLS/cipher expectations derived from the policy name are wrong.
Fix: the transfer CreateServer default for SecurityPolicyName and the DescribeServer projection.
Traces: transfer-multiple-servers-isolation#task#3, also #task#4, transfer-server-lifecycle-start-stop-delete#task#2/#5.

Cause 3 of 4 — the same three members are missing even when the server was created with an explicit security policy and tags.
AWS: recorded behavior: HostKeyFingerprint present, ProtocolDetails and S3StorageOptions defaulted as in Cause 1, alongside the explicitly requested SecurityPolicyName and Tags.
Emulator: HostKeyFingerprint, ProtocolDetails and S3StorageOptions absent.
Why: confirms the omission is unconditional in the projection rather than a side effect of defaulting, so no DescribeServer caller can rely on those members.
Fix: same projection as Cause 1.
Traces: transfer-server-security-policy-and-tags#task#2.

Cause 4 of 4 — StopServer does not transition State, so DescribeServer polling for OFFLINE never terminates.
AWS: Smithy: StopServer input {ServerId required}, output Unit; DescribedServer.State is the State enum [OFFLINE, ONLINE, STARTING, STOPPING, START_FAILED, STOP_FAILED], documented as OFFLINE meaning the server cannot perform file transfer operations. Recorded behavior: after StopServer, DescribeServer reports State OFFLINE (via STOPPING).
Emulator: State stays 'ONLINE' indefinitely — the fixture's poll for OFFLINE failed after 13 attempts and was capped at 60s — and the same step also shows the Cause 1/2 omissions (HostKeyFingerprint, ProtocolDetails, S3StorageOptions, SecurityPolicyName).
Why: StopServer is a no-op on observable state, so any client that waits for OFFLINE before updating an immutable-while-online property (e.g. IpAddressType) or before deleting hangs until its own timeout and then fails.
Fix: the transfer StopServer/StartServer handlers must mutate the stored server State (and DescribeServer must report it).
Traces: transfer-server-lifecycle-start-stop-delete#task#8.

## Evidence
Cause 1: transfer-custom-identity-provider-server-config#task#4 DescribeServer (200/200) — $.Server.HostKeyFingerprint missing (expected present, value not asserted); $.Server.PreAuthenticationLoginBanner missing (expected the banner passed to CreateServer); $.Server.ProtocolDetails missing (expected {PassiveIp AUTO, ProxyConfig {SftpMode NONE}, SetStatOption DEFAULT, SftpPorts [{CommunicationMode SERVER_TALK_FIRST, SftpPort 22}], TlsSessionResumptionMode ENFORCED}); $.Server.S3StorageOptions missing (expected {DirectoryListingOptimization DISABLED}); $.Server.Tags missing (expected []); $.Server.SecurityPolicyName expected 'TransferSecurityPolicy-2024-01', got 'TransferSecurityPolicy-2018-11-05'.
Cause 2: transfer-multiple-servers-isolation#task#3 DescribeServer (200/200) — HostKeyFingerprint, ProtocolDetails, S3StorageOptions missing; SecurityPolicyName expected 'TransferSecurityPolicy-2024-01', got 'TransferSecurityPolicy-2018-11-05' (4 steps in cluster).
Cause 3: transfer-server-security-policy-and-tags#task#2 DescribeServer (200/200) — HostKeyFingerprint, ProtocolDetails, S3StorageOptions missing.
Cause 4: transfer-server-lifecycle-start-stop-delete#task#8 DescribeServer — $.Server.State expected 'OFFLINE', got 'ONLINE'; poll 'not in [OFFLINE] after 13 attempt(s)', capped at 60s; same step also missing HostKeyFingerprint/ProtocolDetails/S3StorageOptions and reporting SecurityPolicyName 'TransferSecurityPolicy-2018-11-05'.

## Steps to Reproduce
1. CreateServer with Domain S3, EndpointType PUBLIC, Protocols [SFTP], IdentityProviderType SERVICE_MANAGED and a PreAuthenticationLoginBanner.
2. DescribeServer on the returned ServerId -> $.Server has no HostKeyFingerprint, ProtocolDetails, S3StorageOptions or Tags, no PreAuthenticationLoginBanner, and SecurityPolicyName 'TransferSecurityPolicy-2018-11-05' instead of 'TransferSecurityPolicy-2024-01' (Causes 1/2).
3. Repeat with an explicit SecurityPolicyName and Tags -> the three computed/defaulted members are still absent (Cause 3).
4. Call StopServer on the ServerId, then poll DescribeServer -> State stays ONLINE and never reaches OFFLINE (Cause 4).
Replay: fixtures transfer-custom-identity-provider-server-config, transfer-multiple-servers-isolation, transfer-server-security-policy-and-tags, transfer-server-lifecycle-start-stop-delete.
