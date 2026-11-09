Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-sftp-connector-sends-file-to-own-server
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-sftp-connector-sends-file-to-own-server
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|DescribeConnector|2xx->2xx|$.Connector.IpAddressType
Issue-Key: transfer|DescribeConnector|2xx->2xx|$.Connector.SecurityPolicyName
Issue-Key: transfer|DescribeConnector|2xx->2xx|$.Connector.ServiceManagedEgressIpAddresses
Issue-Key: transfer|DescribeConnector|2xx->2xx|$.Connector.SftpConfig.OrderedUserSecretVersionStages
Issue-Key: transfer|DescribeConnector|2xx->2xx|$.Connector.SftpConfig.TrustedHostKeys
Issue-Key: transfer|TestConnection|2xx->2xx|$.StatusMessage
Issue-Commit: 67d35df6c1acce919d84587dd79dfbb9f77fdc15

# Transfer SFTP connector read paths diverge: DescribeConnector omits connector defaults and TestConnection's StatusMessage is emulator-authored (2 causes)

> _Note: this issue was identified against the code base at commit `67d35df6c1acce919d84587dd79dfbb9f77fdc15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConnector

## Description
TWO distinct root causes on the Transfer SFTP-connector read surface, both exercised by the same fixture. A fixer must close BOTH before submitting. In both cases the HTTP status matches (200); what diverges is the content the connector reads back.

These are the only two transfer wire divergences in this replay that the report marked neither already-filed (stub) nor OWNED by an open issue. Related OPEN issue that does NOT carry these wire keys: 20261104T104849Z-transfer-connector-plane-updateconnector-replaces-sftpconfig-wholesale-so.md (behavioral corpus; its causes are UpdateConnector's whole-structure SftpConfig replacement and the OnUpload workflow callback) — it does not cover either cause below, so re-replay the two bindings named here independently of it.

=== Cause 1 of 2: DescribeConnector reports a connector WITHOUT the members the service assigns itself, so a connector created with only the required members reads back incomplete ===
AWS: Smithy (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json) DescribeConnector -> {Connector: DescribedConnector @required}. DescribedConnector carries IpAddressType (ConnectorsIpAddressType enum; doc: "When set to IPV4, the connector uses IPv4 addresses only"), SecurityPolicyName (ConnectorSecurityPolicyName, len 0..100, pattern ^TransferSFTPConnectorSecurityPolicy-[A-Za-z0-9-]+$, doc: "The text name of the security policy for the specified connector"), and ServiceManagedEgressIpAddresses (doc: "The list of egress IP addresses of this connector. These IP addresses are assigned automatically when you create the connector."). SftpConnectorConfig carries TrustedHostKeys (list, len 0..10) and OrderedUserSecretVersionStages (list, len 1..2, uniqueItems; doc: "An ordered list of Amazon Web Services Secrets Manager version stages ... the connector attempts to retrieve the SFTP user's credentials from each version stage in the order listed"). For the concrete default values the model is silent, so: recorded behavior only — a connector created with Url/AccessRole/SftpConfig{UserSecretId} and nothing else reads back with IpAddressType = the service default IPV4, SecurityPolicyName = the service's default SFTP-connector policy name (recorded as TransferSFTPConnectorSecurityPolicy-2024-03), ServiceManagedEgressIpAddresses = the non-empty list of IPv4 addresses the service assigned at create time, SftpConfig.OrderedUserSecretVersionStages = the default single-stage order ["AWSCURRENT"], and SftpConfig.TrustedHostKeys = an EMPTY list (present and empty, not absent) when the caller supplied none. The three recorded IP literals are this account/region's assigned addresses and are NOT part of the requirement; the requirement is that a non-empty list of IPv4 addresses is reported and stays stable for the connector's life.
Emulator: all five members are absent from the 200 DescribeConnector body — IpAddressType, SecurityPolicyName, ServiceManagedEgressIpAddresses, SftpConfig.OrderedUserSecretVersionStages and SftpConfig.TrustedHostKeys all come back null/omitted. ServiceManagedEgressIpAddresses is omitted deliberately (services/service-com-amazonaws-transfer-transferservice/src/ops/describe_connector.rs:179 sets service_managed_egress_ip_addresses: None with a comment that no egress runs here); the other four are echoed straight off the stored record (lines 162-195), so the gap is that CreateConnector never materialises the service-assigned defaults into state.
Why: a client that reads a connector back to learn how the service configured it gets nothing to read. It cannot discover the connector's egress IP addresses to allow-list them on the partner's SFTP server (the documented purpose of that member), cannot learn which security policy governs the connector's ciphers, cannot tell IPV4 from DUALSTACK, and cannot distinguish "no trusted host keys" (empty list) from "this field is unknown" (absent) — so the AWS-documented bootstrap of reading TrustedHostKeys back after pinning one has no stable shape. Any read-modify-write round trip through DescribeConnector -> UpdateConnector also silently drops the secret version-stage order.
Fix: the CreateConnector handler/connector state must assign the service-owned defaults at creation (IpAddressType, SecurityPolicyName, the egress IP address list, SftpConfig.OrderedUserSecretVersionStages, and an empty TrustedHostKeys list), and the DescribeConnector projection (services/service-com-amazonaws-transfer-transferservice/src/ops/describe_connector.rs) must serialize them, including a non-empty ServiceManagedEgressIpAddresses rather than a hard-coded None. ListConnectors/UpdateConnector read-backs must agree.
Traces: transfer-sftp-connector-sends-file-to-own-server#task#14.

=== Cause 2 of 2: TestConnection's StatusMessage for a connector with no trusted host keys is an emulator-authored paragraph instead of AWS's one-line verdict ===
AWS: Smithy TestConnection(ConnectorId @required) -> {ConnectorId, Status, StatusMessage (Message), SftpConnectionDetails}. The model documents no value set for StatusMessage, so: recorded behavior only — when the probe reaches the remote server but the connector configuration trusts no host key, AWS answers HTTP 200 with StatusMessage exactly 'No TrustedHostKeys provided.' That is wholly FIXED text: it has no variable part, no request-derived name, id or value interpolated into it, and no remediation prose.
Emulator: HTTP 200 with StatusMessage 'The connector configuration carries no TrustedHostKeys, so the remote server cannot be identified. Verify that the trusted host key in the connector configuration matches the ssh-keyscan output.' — three sentences of emulator-authored fixed text that AWS never emits. Status and the rest of the response match. The refusal is produced in services/service-com-amazonaws-transfer-transferservice/src/state/sftp_target.rs:186-188 (Probe::refused(host_key, SFTP_HOST_KEY_NOT_TRUSTED)) and reported by src/ops/test_connection.rs:214-228.
Why: StatusMessage is the only diagnostic TestConnection returns, and clients and CI assertions match on it to branch between the distinct connector failure modes (unreachable endpoint, unusable secret, rejected credentials, untrusted host key). A client keying on AWS's 'No TrustedHostKeys provided.' never matches here, so an untrusted-host-key connector is indistinguishable from any other ERROR verdict, and remediation automation written against real Transfer Family does not fire.
Fix: the message constants behind the TestConnection probe verdicts (services/service-com-amazonaws-transfer-transferservice/src/state/sftp_target.rs, the SFTP_HOST_KEY_NOT_TRUSTED constant and its siblings) must carry AWS's verdict text for each condition — for this condition the fixed string 'No TrustedHostKeys provided.' with nothing appended.
Traces: transfer-sftp-connector-sends-file-to-own-server#task#15.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 304/345 steps passed, 41 mismatched, score 88.1%, emulator_up true (health 200, data listener accepting). Both clusters below are size 1, verdict 'mismatch', with EMPTY owned_by and EMPTY stub_path.

Cause 1 — fixture transfer-sftp-connector-sends-file-to-own-server, step task#14, operation DescribeConnector, service transfer, expected_status 200, actual_status 200.
detail:
  $.Connector.IpAddressType: missing (expected 'IPV4', got None)
  $.Connector.SecurityPolicyName: missing (expected 'TransferSFTPConnectorSecurityPolicy-2024-03', got None)
  $.Connector.ServiceManagedEgressIpAddresses: missing (expected ['34.208.15.97', '34.210.56.38', '54.186.149.248'], got None)
  $.Connector.SftpConfig.OrderedUserSecretVersionStages: missing (expected ['AWSCURRENT'], got None)
  $.Connector.SftpConfig.TrustedHostKeys: missing (expected [], got None)
divergence_key: service=transfer|DescribeConnector|status=200->200|keys=$.Connector.IpAddressType,$.Connector.SecurityPolicyName,$.Connector.ServiceManagedEgressIpAddresses,$.Connector.SftpConfig.OrderedUserSecretVersionStages,$.Connector.SftpConfig.TrustedHostKeys
The three recorded IPv4 literals are the addresses that account/region assigned; only the presence of a non-empty IPv4 list is the requirement.

Cause 2 — fixture transfer-sftp-connector-sends-file-to-own-server, step task#15, operation TestConnection, service transfer, expected_status 200, actual_status 200.
detail:
  $.StatusMessage: value_mismatch (expected 'No TrustedHostKeys provided.', got 'The connector configuration carries no TrustedHostKeys, so the remote server cannot be identified. Verify that the trusted host key in the connector configuration matches the ssh-keyscan output.')
divergence_key: service=transfer|TestConnection|status=200->200|keys=$.StatusMessage
Both strings are fixed text end to end; no part of either is taken from the request or from state, so this is a wording defect and not a per-run value difference.

Model evidence: smithy_query query=operation operation=DescribeConnector,TestConnection and query=shape shape=DescribedConnector,SftpConnectorConfig on services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json — DescribedConnector.{IpAddressType, SecurityPolicyName, ServiceManagedEgressIpAddresses}, SftpConnectorConfig.{TrustedHostKeys, OrderedUserSecretVersionStages}, TestConnectionResponse.StatusMessage -> Message.
Source evidence: services/service-com-amazonaws-transfer-transferservice/src/ops/describe_connector.rs:162-195 (projection; line 179 hard-codes service_managed_egress_ip_addresses: None), src/state/sftp_target.rs:151-188 (probe verdicts and their fixed messages), src/ops/test_connection.rs:214-228 (StatusMessage serialization).
Related-but-not-covering open issue: 20261104T104849Z-transfer-connector-plane-updateconnector-replaces-sftpconfig-wholesale-so.md.

## Steps to Reproduce
1. git_clone, build and boot the emulator from this checkout, then replay simparity-wire-aws-v1 and re-run the bindings in Issue-Trace/Issue-Step and Issue-Trace-Ids: transfer-sftp-connector-sends-file-to-own-server#task#14 (cause 1) and transfer-sftp-connector-sends-file-to-own-server#task#15 (cause 2).
2. Cause 1 by hand (region us-west-2): create an S3 bucket and an IAM role trusted by transfer.amazonaws.com; transfer create-server --protocols SFTP --identity-provider-type SERVICE_MANAGED --domain S3 --endpoint-type PUBLIC, start it and poll describe-server until State=ONLINE; create a user with an SSH public key and a Secrets Manager secret holding {Username, PrivateKey}; then transfer create-connector --url sftp://<serverId>.server.transfer.us-west-2.amazonaws.com --access-role <roleArn> --sftp-config '{"UserSecretId":"<secretArn>"}' — naming NO IpAddressType, NO SecurityPolicyName, NO TrustedHostKeys and NO OrderedUserSecretVersionStages.
3. transfer describe-connector --connector-id c-... and inspect the 200 body: Connector.IpAddressType must be 'IPV4', Connector.SecurityPolicyName must be the service's default SFTP-connector policy name, Connector.ServiceManagedEgressIpAddresses must be a non-empty list of IPv4 addresses, Connector.SftpConfig.OrderedUserSecretVersionStages must be ['AWSCURRENT'] and Connector.SftpConfig.TrustedHostKeys must be present and empty. Observe all five absent.
4. Cause 2: with that same connector (no TrustedHostKeys), transfer test-connection --connector-id c-... and read StatusMessage from the 200 response: it must be exactly 'No TrustedHostKeys provided.' Observe the emulator's three-sentence 'The connector configuration carries no TrustedHostKeys, so the remote server cannot be identified. Verify that the trusted host key in the connector configuration matches the ssh-keyscan output.' instead.
