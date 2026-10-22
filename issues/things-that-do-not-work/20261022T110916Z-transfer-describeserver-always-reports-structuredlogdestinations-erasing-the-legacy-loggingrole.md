Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs__transfer.refarch-sftp-logging-role-and-structured-destinations-are-mutually-exclusive
Issue-Key: cloudwatch__logs__transfer.refarch-sftp-logging-role-and-structured-destinations-are-mutually-exclusive|run#4|AssertionError|assert "StructuredLogDestinations" not in legacy, (
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# DescribeServer always reports StructuredLogDestinations, erasing the legacy-LoggingRole vs structured-destination distinction

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServer

## Description
Cause 1 of 1: the DescribedServer projection emits the logging members unconditionally instead of only the one that was configured.
AWS: DescribedServer's LoggingRole and StructuredLogDestinations are both OPTIONAL members (Smithy: neither is @required; StructuredLogDestinations is a list @length 0..1 documented as "Specifies the log groups to which your server logs are sent"), and the two are mutually exclusive in a response: a server created with LoggingRole alone describes WITH LoggingRole and WITHOUT the StructuredLogDestinations key, and a server created with StructuredLogDestinations alone describes WITHOUT the LoggingRole key. That omission is load-bearing: a server configured with LoggingRole alone publishes to the SERVICE-managed group /aws/transfer/<server-id>, not to any declared group, which is exactly what the absent key tells the caller (recorded behavior; the model only establishes that both members are optional).
Emulator: a server declaring only LoggingRole describes with StructuredLogDestinations=[] present. The key is always serialized, so the "which one is absent" signal is gone even though every field-by-field comparison looks correct.
Why: a client (or a reference architecture's log-destination check) reads the presence of StructuredLogDestinations to decide where the server's logs land. With an always-present empty list it concludes "structured logging configured, zero destinations" instead of "service-managed group", so a metric filter is pointed at a declared group that never receives a line and the alarm watching it can never fire. The inverse leg is equally unobservable: the emulator cannot show the legacy field being omitted when the modern one is set.
Fix: the DescribeServer response projection/serializer for DescribedServer in services/service-com-amazonaws-transfer-transferservice/src/ops/ (describe_server) and the server state that records which logging mode was configured — unset optional members must be omitted, not defaulted to an empty list.
Traces: cloudwatch__logs__transfer.refarch-sftp-logging-role-and-structured-destinations-are-mutually-exclusive.

Attribution note: the grounding check id carries the composite service token `cloudwatch__logs__transfer` (it provisions a CloudWatch Logs group as the structured destination). The divergence itself is in Transfer's DescribeServer response, and both constituent services are bootstrapped, so this is filed under `transfer`.

## Evidence
AssertionError: a server declaring only LoggingRole reports StructuredLogDestinations=[]; real AWS omits the key entirely, and that omission is what says the server logs to the service-managed group /aws/transfer/<server-id> rather than to a declared one. (LoggingRole, IdentityProviderType=SERVICE_MANAGED and Protocols=['SFTP'] all echoed correctly; only the presence of the second logging member diverges. The control leg — a server declaring only StructuredLogDestinations must omit LoggingRole — is never reached.)

## Steps to Reproduce
1. Create an IAM role trusted by transfer.amazonaws.com for CloudWatch logging.
2. CreateServer(LoggingRole=<role arn>) with the default SERVICE_MANAGED IdP and Protocols=['SFTP'].
3. DescribeServer on it and inspect the returned Server dict: 'StructuredLogDestinations' must not be a key at all (the emulator returns []).
4. Control: create a log group, then CreateServer(StructuredLogDestinations=[<exact log-group ARN>]) and assert 'LoggingRole' is absent from DescribeServer.
Check: cloudwatch__logs__transfer.refarch-sftp-logging-role-and-structured-destinations-are-mutually-exclusive.
