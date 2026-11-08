Issue-Source: trace-sweep-divergence
Issue-Trace: workspaces-operate-on-terminated-workspace-errors
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: workspaces-operate-on-terminated-workspace-errors
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces|DescribeWorkspaceBundles|2xx->2xx|$.NextToken
Issue-Commit: 3869fc9114db7d726b979830511566e7886d3900

# DescribeWorkspaceBundles NextToken is an internal positional offset, not an opaque service-minted pagination token

> _Note: this issue was identified against the code base at commit `3869fc9114db7d726b979830511566e7886d3900`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeWorkspaceBundles

## Description
One net-new root cause.

Cause 1 of 1: the DescribeWorkspaceBundles response's NextToken is rendered as the emulator's own zero-padded decimal list index instead of an opaque token the service mints.

AWS: Smithy (com-amazonaws-workspaces-workspacesservice) models DescribeWorkspaceBundles with input/output member NextToken -> PaginationToken, @length min 1 max 2048, and @paginated {inputToken: NextToken, outputToken: NextToken, items: Bundles}. The model puts no @pattern on PaginationToken, i.e. the token is declared opaque and carries no client-readable structure; recorded behavior only for its concrete form - the recorded AWS response returns a UUID-form token (lowercase hex in 8-4-4-4-12 groups, 36 characters) that bears no relation to the number of bundles already returned. The token's literal characters are a per-run value and are NOT the requirement; what is a requirement is that the token is a service-minted opaque handle to a cut point, not a rendering of the listing's internal position.

Emulator: the same step answers 200 with NextToken set to a fixed-width 16-character zero-padded decimal - replay reports `$.NextToken: value_mismatch (expected 'ad69e425-8bf1-4e13-8d28-ba6dd53e156b', got '0000000000000019')`. The value is the offset into the emulator's in-memory bundle list, formatted as digits, so the token's character class and semantics both differ from the service's: it is an index the client can read, decrement or synthesize rather than a handle the service issued.

Why: a client that only round-trips the token still pages, so the user-visible break is narrow, but the opacity contract is gone. Hypothesis (not directly observed in this step, which only exercises the minting side): because the token IS the offset, a decimal a client writes by hand - or a token minted by a DescribeWorkspaceBundles call with a different Owner/BundleIds filter - decodes to a valid cut point and is served as a page instead of being refused with InvalidParameterValuesException, so a page boundary can silently skip or repeat bundles after the catalog changes. Clients and recorded-response comparisons that key on the token's form (a UUID-shaped handle) also see a body that no real WorkSpaces endpoint produces.

Fix: the workspaces crate's DescribeWorkspaceBundles handler and the pagination helper it mints NextToken through. Mint an opaque token that the service stores or signs, bound to the request's bundle-selection arguments and to the cut point, rather than formatting the internal index; validate an incoming NextToken against that binding (length 1..2048 plus "issued by this service for this listing") and refuse anything else as InvalidParameterValuesException instead of parsing it as a number.

Traces: workspaces-operate-on-terminated-workspace-errors#task#10 (DescribeWorkspaceBundles, expected status 200, actual 200, body key $.NextToken).

Related open issues, none of which covers this: 20261029T185818Z-workspaces-createworkspaces-failedrequests-entries-diverge-request-echo-drops-workspaceproperties.md and 20261105T054826Z-workspaces-resourcenotfoundexception-for-a-missing-ip-group-carries-the.md own the other workspaces divergences in this replay (CreateWorkspaces FailedRequests echo, IP-group ResourceNotFoundException wording) and say nothing about pagination tokens.

## Evidence
Replay of fixture workspaces-operate-on-terminated-workspace-errors, step task#10 (operation DescribeWorkspaceBundles, service workspaces):
  expected_status 200, actual_status 200
  detail: $.NextToken: value_mismatch (expected 'ad69e425-8bf1-4e13-8d28-ba6dd53e156b', got '0000000000000019')

Expected (recorded AWS): NextToken is a 36-character UUID-form opaque handle; its digits/hex are a per-run value, its FORM and its independence from the listing position are the requirement.
Actual (emulator): NextToken is '0000000000000019' - a 16-digit zero-padded rendering of the internal offset into the bundle list, readable and constructible by the client.

Smithy evidence: DescribeWorkspaceBundles input.NextToken and output.NextToken both target PaginationToken with @length {min: 1, max: 2048}; @paginated {inputToken: NextToken, outputToken: NextToken, items: Bundles}; modelled error InvalidParameterValuesException (client, HTTP 400). No @pattern is modelled, i.e. the token is opaque to clients and its structure is the service's to choose - and the service does not choose a list index.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture workspaces-operate-on-terminated-workspace-errors drives the workspaces data plane; at step task#10 it calls DescribeWorkspaceBundles with a page size smaller than the bundle catalog so the response is truncated.
3. Inspect the 200 response body's NextToken: the emulator returns '0000000000000019' (zero-padded decimal offset) where the recorded service returns an opaque UUID-form token such as 'ad69e425-8bf1-4e13-8d28-ba6dd53e156b'.
4. Hypothesis check for the input side: re-issue DescribeWorkspaceBundles with a hand-written decimal NextToken (e.g. '0000000000000003'), or with a NextToken minted by a DescribeWorkspaceBundles call using a different Owner/BundleIds filter. AWS refuses a token it never issued with InvalidParameterValuesException (400); the emulator serves it as a cut point.
