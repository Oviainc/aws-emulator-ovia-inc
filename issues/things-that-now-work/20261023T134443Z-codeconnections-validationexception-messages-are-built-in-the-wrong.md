Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeconnections.connection-name-exceeds-max-length-rejected codeconnections.create-host-provider-endpoint-exceeding-max-length-rejected codeconnections.connection-malformed-host-arn-rejected codeconnections.create-repository-link-inaccessible-connection-rejected codeconnections.connection-invalid-provider-type-rejected
Issue-Shape-Id: com.amazonaws.codeconnections#CodeConnections_20231201
Issue-Key: codeconnections.connection-name-exceeds-max-length-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codeconnections.create-host-provider-endpoint-exceeding-max-length-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codeconnections.connection-malformed-host-arn-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codeconnections.create-repository-link-inaccessible-connection-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: codeconnections.connection-invalid-provider-type-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# codeconnections ValidationException messages are built in the wrong shape: rejected value echoed on length/pattern failures, patterns printed anchored, enum constraint phrased differently (4 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection

## Description
Four net-new root causes, all in how the codeconnections emulator renders the standard AWS input-validation message. These messages are requirements because their rule is known: they are built from the request and name the offending member and the constraint it violated (the "N validation error detected: ... at '<member>' failed to satisfy constraint: ..." framework format). The error code (ValidationException) is correct in every case; the message construction is not.

Cause 1 of 4: @length violations echo the rejected value, which AWS omits.
AWS: recorded behavior only for the exact wording — for a string member that violates its @length max, the refusal is ValidationException with "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must have length less than or equal to <max>": the member and the bound appear, the offending value does NOT. (Smithy: CreateConnection.ConnectionName @length max 32; CreateHost.ProviderEndpoint -> Url @length max 512.)
Emulator: emits "... Value '<the whole rejected value>' at '<member>' failed to satisfy constraint: Member must have length less than or equal to <max>", i.e. it interpolates the rejected value (and, for a 620-byte endpoint, a truncated form of it with a "... [620 bytes]" marker).
Why: clients that pattern-match or display validation messages, and recordings/snapshots of them, see an extra value segment; over-long and potentially sensitive request data is echoed back where AWS deliberately does not.
Fix: the shared input-validation message formatter for @length failures in the codeconnections validation layer must drop the value segment.
Traces: codeconnections.connection-name-exceeds-max-length-rejected, codeconnections.create-host-provider-endpoint-exceeding-max-length-rejected.

Cause 2 of 4: @pattern violations echo the rejected value AND print the regex with its ^...$ anchors.
AWS: recorded behavior only for the exact wording — "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern without leading ^ and trailing $>" (no value segment, anchors stripped). Smithy: HostArn @pattern "^arn:aws(-[\\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\\/.+$" — the recorded message shows exactly that expression minus the anchors.
Emulator: emits the value segment ("Value 'not-an-arn' at 'hostArn' ...") and prints the pattern verbatim including ^ and $.
Why: same client/snapshot impact as cause 1, plus the advertised pattern differs textually from the one AWS advertises, so a client that surfaces or re-compiles it sees a different expression.
Fix: the @pattern branch of the same validation message formatter — omit the value, strip the anchors.
Traces: codeconnections.connection-malformed-host-arn-rejected, codeconnections.create-repository-link-inaccessible-connection-rejected.

Cause 3 of 4: CreateRepositoryLink reports the generic ARN pattern for ConnectionArn instead of the connection-specific one the service enforces.
AWS: recorded behavior only — a malformed CreateRepositoryLink.ConnectionArn is refused naming the connection-scoped expression "arn:aws(-[\\w]+)*:(codestar-connections|codeconnections):[a-z0-9\\-]+:[0-9]{12}:connection\\/[\\w\\-]+". (The public Smithy model only carries the loose ConnectionArn @pattern "^arn:aws(-[\\w]+)*:.+:.+:[0-9]{12}:.+$", so the service validates more tightly than the model states.)
Emulator: reports the loose model pattern "^arn:aws(-[\\w]+)*:.+:.+:[0-9]{12}:.+$", i.e. it validates ConnectionArn only as a generic ARN.
Why: an ARN that is a valid ARN of some other service/resource type is accepted by the emulator where AWS refuses it, so a client bug (passing a host or repository-link ARN as ConnectionArn) is silently accepted; the refusal also advertises a different pattern.
Fix: ConnectionArn validation for the repository-link operations should use the connection-scoped pattern and report it.
Traces: codeconnections.create-repository-link-inaccessible-connection-rejected.

Cause 4 of 4: enum violations are phrased with a different constraint clause and value order.
AWS: recorded behavior only for the wording — an out-of-range ProviderType is refused with "1 validation error detected: Value 'NotARealProvider' at 'providerType' failed to satisfy constraint: Member must be one of: [GitLab, GitHub, GitHubEnterpriseServer, AzureDevOps, GitLabSelfManaged, Bitbucket]" (note: for the enum case the rejected value IS echoed). Smithy: ProviderType enum values = [Bitbucket, GitHub, GitHubEnterpriseServer, GitLab, GitLabSelfManaged, AzureDevOps].
Emulator: "... failed to satisfy constraint: Member must satisfy enum value set: [Bitbucket, GitHub, GitHubEnterpriseServer, GitLab, GitLabSelfManaged, AzureDevOps]" — the constraint clause is "Member must satisfy enum value set" rather than "Member must be one of", and the values are listed in model-declaration order. The clause wording is the requirement here; the bracketed order is given as the recorded example.
Why: clients and test snapshots that match on the documented constraint clause do not match; the rejection itself is correct.
Fix: the enum branch of the codeconnections validation message formatter.
Traces: codeconnections.connection-invalid-provider-type-rejected.

## Evidence
Cause 1 — connection-name-exceeds-max-length-rejected: expected "1 validation error detected: Value at 'connectionName' failed to satisfy constraint: Member must have length less than or equal to 32", got "1 validation error detected: Value 'sim2real-conn-4253492d70efxxxx...' at 'connectionName' failed to satisfy constraint: Member must have length less than or equal to 32". create-host-provider-endpoint-exceeding-max-length-rejected: expected "Value at 'providerEndpoint' ... less than or equal to 512", got "Value 'https://aaaa...[620 bytes]' at 'providerEndpoint' ...".
Cause 2 — connection-malformed-host-arn-rejected: expected "Value at 'hostArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[\\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\\/.+", got "Value 'not-an-arn' at 'hostArn' ... pattern: ^arn:aws(-[\\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\\/.+$".
Cause 3 — create-repository-link-inaccessible-connection-rejected: expected pattern "arn:aws(-[\\w]+)*:(codestar-connections|codeconnections):[a-z0-9\\-]+:[0-9]{12}:connection\\/[\\w\\-]+", got "^arn:aws(-[\\w]+)*:.+:.+:[0-9]{12}:.+$".
Cause 4 — connection-invalid-provider-type-rejected: expected "... Member must be one of: [GitLab, GitHub, GitHubEnterpriseServer, AzureDevOps, GitLabSelfManaged, Bitbucket]", got "... Member must satisfy enum value set: [Bitbucket, GitHub, GitHubEnterpriseServer, GitLab, GitLabSelfManaged, AzureDevOps]". In all four, the error code returned was ValidationException, as expected.

## Steps to Reproduce
1. Boot the emulator and point a botocore codeconnections client at it.
2. Cause 1: cc.create_connection(ConnectionName='x'*40, ProviderType='GitHub'); cc.create_host(Name='h', ProviderType='GitHubEnterpriseServer', ProviderEndpoint='https://'+'a'*600+'.example.com') — inspect Error.Message.
3. Cause 2: cc.create_connection(ConnectionName='ok', HostArn='not-an-arn') — inspect Error.Message.
4. Cause 3: cc.create_repository_link(ConnectionArn='not-an-arn', OwnerId='o', RepositoryName='r') — inspect which pattern the message advertises.
5. Cause 4: cc.create_connection(ConnectionName='ok', ProviderType='NotARealProvider') — inspect the constraint clause.
