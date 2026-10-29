Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-connections-host-connection-dependency-and-filtering
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-connections-host-connection-dependency-and-filtering codestar-connections-repository-link-error-paths codestar-connections-repository-link-lifecycle codestar-connections-repository-link-id-and-connection-validation
Issue-Shape-Id: com.amazonaws.codestarconnections#CodeStar_connections_20191201
Issue-Key: codestar-connections|DeleteHost|4xx->4xx|$.Message
Issue-Key: codestar-connections|GetRepositoryLink|4xx->4xx|$.Message
Issue-Key: codestar-connections|CreateRepositoryLink|4xx->4xx|$.Message
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# codestar-connections error messages diverge from AWS wording on dependency, not-found and invalid-connection paths

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteHost

## Description
Three codestar-connections error paths return the correct HTTP status and the correct error code but an error MESSAGE the emulator authored itself instead of the message AWS words for that error class. The status/code pair is right in every case, so only the Message member diverges. SDK clients, CLI output and any client that branches on message text see text that never appears on real CodeConnections. This bundle has THREE causes; all three must be fixed.

Cause 1 of 3 - DeleteHost on a host that still has connections (fixture codestar-connections-host-connection-dependency-and-filtering, step task#6).
AWS: DeleteHost rejects deletion of a host that still has associated connections with ResourceUnavailableException (Smithy: DeleteHost errors = ResourceNotFoundException httpError 404, ResourceUnavailableException httpError 400; ResourceUnavailableException.Message is ErrorMessage, @length max 600). Recorded response: HTTP 400 with Message 'There exist connections for host <hostArn>, host cannot be deleted'. Fixed text: 'There exist connections for host ', then ', host cannot be deleted'. Variable part: <hostArn> only - the HostArn taken verbatim from the request. The message states NO connection count.
Emulator: HTTP 400 (status and code match) with Message 'The host <hostArn> cannot be deleted because 1 connection(s) are still associated with it. Delete those connections first.' - entirely emulator-authored prose, and it additionally leaks a count ('1 connection(s)') that AWS's message does not contain.
Why: a client that asserts on, logs or surfaces the dependency-violation message gets text AWS never emits; the injected count also makes the message vary with emulator state where AWS's does not.
Fix: the DeleteHost handler's dependency check in the codestar-connections service crate - the place that constructs the ResourceUnavailableException message. The status and code are already correct; only the message template changes.
Traces: codestar-connections-host-connection-dependency-and-filtering#task#6.

Cause 2 of 3 - GetRepositoryLink for an unknown repository link id (fixture codestar-connections-repository-link-error-paths, step teardown#7; same cluster also covers codestar-connections-repository-link-lifecycle#teardown#12).
AWS: GetRepositoryLink answers ResourceNotFoundException when the RepositoryLinkId names no link (Smithy: GetRepositoryLink errors include ResourceNotFoundException httpError 404; RepositoryLinkId is required with the uuid pattern). The recorded message is the wrapped CodeConnections form 'Repository not found for accountId: <accountId> and uuid: <repositoryLinkId> (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <requestId>; Proxy: null)'. Fixed text includes the lowercase token 'and uuid: '. Variable parts: <accountId> (the caller's account), <repositoryLinkId> (from the request) and <requestId> (per-run).
Emulator: emits the same wrapper but capitalises the label as 'and UUID: ' - 'Repository not found for accountId: 000000000000 and UUID: 93b802a0-f501-4546-88a6-000000000001 (...)'. The Request ID also differs, but a request id is a per-run value and is NOT part of this finding; the casing of the fixed label 'uuid' is.
Why: a client or test that matches the documented not-found message fails on the casing difference even though the resource really is absent.
Fix: the GetRepositoryLink handler's not-found path / the shared message formatter for repository-link lookups in the codestar-connections crate - emit the fixed token as lowercase 'uuid:'.
Traces: codestar-connections-repository-link-error-paths#teardown#7, codestar-connections-repository-link-lifecycle#teardown#12.

Cause 3 of 3 - CreateRepositoryLink with a ConnectionArn that names no connection (fixture codestar-connections-repository-link-id-and-connection-validation, step task#9).
AWS: CreateRepositoryLink validates the required ConnectionArn (Smithy: CreateRepositoryLink.ConnectionArn required, @length max 256, pattern ^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$; errors include InvalidInputException httpError 400, whose doc is 'The input is not valid. Verify that the action is typed correctly.'). Recorded response: HTTP 400 with Message 'Connection <connectionArn> does not exist'. Fixed text: 'Connection ' then ' does not exist'. Variable part: <connectionArn> only, copied from the request. No 'The input is not valid.' preamble and no member name appear in the message.
Emulator: HTTP 400 (status and code match) with Message 'The input is not valid. ConnectionArn names no connection in this account: <connectionArn>' - it prepends the exception's documentation sentence and names the member, neither of which AWS puts in this message.
Why: clients that parse or assert the 'Connection ... does not exist' message to distinguish a dangling connection reference from a malformed ARN cannot match it.
Fix: the CreateRepositoryLink handler's connection-existence validation in the codestar-connections crate - stop reusing the generic InvalidInputException doc text as a message prefix and emit AWS's template.
Traces: codestar-connections-repository-link-id-and-connection-validation#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, score 91.8%).

Cause 1 - codestar-connections-host-connection-dependency-and-filtering#task#6, DeleteHost, expected 400 / actual 400, $.Message value_mismatch:
  expected: 'There exist connections for host arn:aws:codestar-connections:us-west-2:000000000000:host/500612f7-3610-4d84-82ea-000000000000, host cannot be deleted'
  actual:   'The host arn:aws:codestar-connections:us-west-2:000000000000:host/500612f7-3610-4d84-82ea-000000000000 cannot be deleted because 1 connection(s) are still associated with it. Delete those connections first.'

Cause 2 - codestar-connections-repository-link-error-paths#teardown#7 (also codestar-connections-repository-link-lifecycle#teardown#12), GetRepositoryLink, expected 400 / actual 400, $.Message value_mismatch:
  expected: 'Repository not found for accountId: 000000000000 and uuid: 93b802a0-f501-4546-88a6-000000000001 (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: ...; Proxy: null)'
  actual:   'Repository not found for accountId: 000000000000 and UUID: 93b802a0-f501-4546-88a6-000000000001 (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: ...; Proxy: null)'
  Diverging fixed text: 'uuid:' vs 'UUID:'. The differing Request ID is a per-run value and is not claimed as a defect.

Cause 3 - codestar-connections-repository-link-id-and-connection-validation#task#9, CreateRepositoryLink, expected 400 / actual 400, $.Message value_mismatch:
  expected: 'Connection arn:aws:codestar-connections:us-west-2:000000000000:connection/00000000-0000-0000-0000-000000000000 does not exist'
  actual:   'The input is not valid. ConnectionArn names no connection in this account: arn:aws:codestar-connections:us-west-2:000000000000:connection/00000000-0000-0000-0000-000000000000'

Model references used: Smithy DeleteHost (errors ResourceNotFoundException/ResourceUnavailableException), GetRepositoryLink (errors incl. ResourceNotFoundException 404), CreateRepositoryLink (ConnectionArn required; errors incl. InvalidInputException 400); ResourceUnavailableException/InvalidInputException/ResourceNotFoundException each carry a single Message member of ErrorMessage.

Related already-resolved issue (does NOT cover these): 'codestar-notifications error responses carry non-AWS message text' - same class of defect in a different service.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture codestar-connections-host-connection-dependency-and-filtering to step task#6 - create a host, create a connection on that host, then call DeleteHost on the host. Compare the 400 response's Message against 'There exist connections for host <hostArn>, host cannot be deleted'.
3. Cause 2: replay fixture codestar-connections-repository-link-error-paths to step teardown#7 (or codestar-connections-repository-link-lifecycle to teardown#12) - call GetRepositoryLink with a well-formed RepositoryLinkId that has been deleted / never existed, and compare the Message's fixed label, which must read 'and uuid:' in lowercase.
4. Cause 3: replay fixture codestar-connections-repository-link-id-and-connection-validation to step task#9 - call CreateRepositoryLink with a syntactically valid ConnectionArn naming no connection, and compare the Message against 'Connection <connectionArn> does not exist'.
5. All three must match AWS's message templates, with only the ARNs/ids/account filled from the request and state.
