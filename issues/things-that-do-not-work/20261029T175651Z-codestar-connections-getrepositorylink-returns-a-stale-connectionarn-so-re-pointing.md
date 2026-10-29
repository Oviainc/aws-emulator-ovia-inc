Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-connections-repository-link-lifecycle
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-connections-repository-link-lifecycle
Issue-Shape-Id: com.amazonaws.codestarconnections#CodeStar_connections_20191201
Issue-Key: codestar-connections|GetRepositoryLink|2xx->2xx|$.RepositoryLinkInfo.ConnectionArn
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# GetRepositoryLink returns a stale ConnectionArn, so re-pointing a repository link is not persisted

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepositoryLink

## Description
Cause 1 of 1: in the repository-link lifecycle, after the link is re-associated with a different connection, GetRepositoryLink still reports the connection the link was originally created with.

AWS: RepositoryLinkInfo.ConnectionArn is a required member documented as 'The Amazon Resource Name (ARN) of the connection associated with the repository link' (Smithy: RepositoryLinkInfo.ConnectionArn required, @length max 256, pattern ^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$). UpdateRepositoryLink accepts an optional ConnectionArn and returns the updated RepositoryLinkInfo (required output member), so once an update changes the association, every later read - GetRepositoryLink, ListRepositoryLinks - must report the NEW connection ARN. The recorded run shows exactly that: the link's ConnectionArn at this step is the second connection created in the fixture, not the first.
Emulator: HTTP 200 (the status and shape are correct) but RepositoryLinkInfo.ConnectionArn is the connection the link was created against, i.e. the earlier connection in the fixture, rather than the one it was last associated with. Both ARNs are emulator-assigned in the same deterministic sequence, so the difference is the IDENTITY of the referenced connection, not a per-run id. Hypothesis: the update path mutates a copy (or only validates the new ConnectionArn) and never writes the new value back to the stored repository-link record, so reads keep serving the creation-time association.
Why: a client that re-points a repository link to another connection - e.g. after rotating a connection or migrating a provider credential - reads back the old association and will keep driving syncs through the connection it just replaced, while believing the update took effect. Because the read returns 200 with a well-formed required field, nothing signals the loss.
Fix: the repository-link state in the codestar-connections service crate, plus the UpdateRepositoryLink handler that writes it - the mutation must persist ConnectionArn (and EncryptionKeyArn) onto the stored record so GetRepositoryLink/ListRepositoryLinks serve the current value.
Traces: codestar-connections-repository-link-lifecycle#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, score 91.8%).

codestar-connections-repository-link-lifecycle#task#10, GetRepositoryLink, expected 200 / actual 200, $.RepositoryLinkInfo.ConnectionArn value_mismatch:
  expected: 'arn:aws:codestar-connections:us-west-2:000000000000:connection/fd20b46c-60c4-46ef-ba2f-000000000002'
  actual:   'arn:aws:codestar-connections:us-west-2:000000000000:connection/a11f0c28-eb5e-4d25-b5b2-000000000000'

The expected value is the later connection in the fixture's deterministic sequence (suffix ...000000000002); the emulator returns the first one (suffix ...000000000000), i.e. the creation-time association. No other member of RepositoryLinkInfo diverged at this step.

Model references used: Smithy UpdateRepositoryLink (optional input ConnectionArn, required output RepositoryLinkInfo), GetRepositoryLink (required output RepositoryLinkInfo), RepositoryLinkInfo.ConnectionArn (required, documented as the connection associated with the repository link).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture codestar-connections-repository-link-lifecycle up to step task#10: create two connections, CreateRepositoryLink against the first, then UpdateRepositoryLink on that RepositoryLinkId with ConnectionArn set to the second connection.
3. Call GetRepositoryLink with the same RepositoryLinkId.
4. Expect RepositoryLinkInfo.ConnectionArn to equal the connection supplied to UpdateRepositoryLink; the emulator returns the connection used at CreateRepositoryLink time.
5. Also check ListRepositoryLinks for the same link - it must report the updated ConnectionArn.
