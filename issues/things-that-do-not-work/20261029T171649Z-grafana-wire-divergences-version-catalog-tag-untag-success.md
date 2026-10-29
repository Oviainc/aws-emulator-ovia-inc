Issue-Source: trace-sweep-divergence
Issue-Trace: grafana-catalog-list-versions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: grafana-catalog-list-versions grafana-describe-workspace-malformed-id-rejected grafana-tag-resource-roundtrip grafana-workspace-endpoint-is-a-bare-host
Issue-Shape-Id: com.amazonaws.grafana#AWSGrafanaControlPlane
Issue-Key: grafana|ListVersions|2xx->2xx|$.grafanaVersions[*]
Issue-Key: grafana|DescribeWorkspace|4xx->4xx|$.message
Issue-Key: grafana|TagResource|2xx->2xx|status
Issue-Key: grafana|UntagResource|2xx->2xx|status
Issue-Key: grafana|DescribeWorkspace|2xx->2xx|$.workspace.endpoint
Issue-Key: grafana|DescribeWorkspace|2xx->2xx|$.workspace.ipAddressType
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# grafana wire divergences: version catalog, tag/untag success status, workspace endpoint host, ipAddressType default, pattern-ValidationException message

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListVersions, DescribeWorkspace, TagResource, UntagResource

## Description
The Amazon Managed Grafana control plane answers the recorded wire corpus (14/19 steps pass), but FIVE distinct causes diverge from the recorded AWS responses. All five are in the `grafana` service and are filed as ONE work item with FIVE causes; a fixer must close ALL FIVE before submitting, and must re-replay every fixture/step listed in this issue's replay pointer (the representative plus the four tokens in trace_ids), not only the representative.

=== Cause 1 of 5: ListVersions publishes a stale supported-version catalog (service-wide constant, not the versions AWS offers) ===
AWS: Smithy `ListVersions` is `GET /versions`, success 200, output `grafanaVersions: GrafanaVersionList` (`@paginated(items: grafanaVersions)`); the model constrains only each member's length (1..255) and names no value set, so the supported set is recorded behavior only. The recording shows the catalog served oldest-first with FOUR entries whose newest members are current Grafana majors — the recorded list is `9.4`, `10.4`, `12.4`, `13.2`: the oldest entry served is `9.4`, and the two newest entries the emulator has no knowledge of at all are `12.4` and `13.2`.
Emulator: serves a three-entry catalog `8.4`, `9.4`, `10.4` — it serves an `8.4` AWS no longer offers at index 0, and serves nothing at all past `10.4` (index 3 is absent). Replay reports `$.grafanaVersions[0]: extra (expected '9.4', got '8.4')`, `$.grafanaVersions[2]: missing (expected '12.4', got '10.4')`, `$.grafanaVersions[3]: missing (expected '13.2', got None)`.
Why: `ListVersions` is what a client reads to pick a `grafanaVersion` for `CreateWorkspace`, and (with `workspaceId`) the set a workspace may be UPGRADED to. A client that picks the newest offered version gets a version AWS would reject as unsupported-newest/too-old, every upgrade path onto a current Grafana major is invisible, and a client that validates `8.4` is no longer offered sees it offered. The same constant decides `CreateWorkspace`'s default version, so the default workspace version is wrong too.
Fix: this service's state — the single supported-version catalog constant that `ListVersions` publishes and that the create-path default and the upgrade-target computation read off it. It must name the versions AWS currently offers, still oldest-first (the ordering the pagination token depends on). Not the `ListVersions` handler, which correctly serves whatever the catalog holds.
Traces: grafana-catalog-list-versions#task#1. Code: services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/state.rs:36 (SUPPORTED_GRAFANA_VERSIONS) and :43 (DEFAULT_GRAFANA_VERSION); src/ops/list_versions.rs:173-188 reads it.

=== Cause 2 of 5: the malformed-workspaceId ValidationException carries the generic boundary-validator sentence instead of Grafana's pattern message ===
AWS: Smithy `DescribeWorkspaceRequest$workspaceId` is `@required`, `@httpLabel`, pattern `^g-[0-9a-f]{10}$`; the operation models `ValidationException` (HTTP 400). Status (400) and error code (`ValidationException`) already match. The recorded message for a value failing that pattern is Grafana's own form: the fixed text is `The ` + <member name> + ` should satisfy the pattern ` + <the member's modeled pattern> + `.` — i.e. `The workspaceId should satisfy the pattern ^g-[0-9a-f]{10}$.` for this member. Its only variable parts are the member name and that member's modeled pattern; the rejected VALUE does not appear in it at all.
Emulator: answers 400 `ValidationException` with the centralized multi-service boundary form `1 validation error detected: Value '<rejected value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: ^g-[0-9a-f]{10}$` (replay: `$.message: value_mismatch`).
Why: the message is part of the rejection contract a client surfaces or matches on; the emulator's sentence has different fixed text and additionally echoes the rejected value back, which AWS's form never does (a caller relying on that for input scrubbing/logging sees the value leak).
Fix: this service's validation layer — override the generic pattern-violation message for Grafana's `WorkspaceId` members with the service's own `The <member> should satisfy the pattern <modeled pattern>.` form, keeping 400 / `ValidationException`. NOTE: the open behavioral issue 20261022T092712Z-grafana-workspace-control-plane-6-behavioral-divergences-delete.md describes this same message rule as its Cause 5, but it carries no wire replay key, so this wire divergence is unowned and is filed here with its fixture/step binding; the two should be closed together.
Traces: grafana-describe-workspace-malformed-id-rejected#task#2.

=== Cause 3 of 5: TagResource answers 200 instead of the recorded 204 No Content ===
AWS: Smithy `TagResource` is `POST /tags/{resourceArn}` with an EMPTY output structure and NO explicit `@http(code:)` — the model's 200 is the framework default (`codeDefaulted: true`), so the success status is recorded behavior only. The recording shows the tag write answered with HTTP 204 and no body.
Emulator: answers HTTP 200 (replay: `status: expected 204, got 200`).
Why: a client or SDK test that distinguishes 204 from 200 for an empty-output operation (status assertions, HTTP-layer caching/retry policy, proxies and recorders that treat 200-with-no-body as a malformed response) sees the wrong status for every successful tag write.
Fix: this operation's success-status declaration in its serializer seam (`SUCCESS_STATUS` for the operation), which must be 204 rather than the scaffold's defaulted 200.
Traces: grafana-tag-resource-roundtrip#task#4. Code: services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/tag_resource.rs (SUCCESS_STATUS).

=== Cause 4 of 5: UntagResource answers 200 instead of the recorded 204 No Content ===
AWS: Smithy `UntagResource` is `DELETE /tags/{resourceArn}`, `@idempotent`, with an EMPTY output structure and no explicit `@http(code:)` (`codeDefaulted: true`), so its success status is recorded behavior only. The recording shows the tag removal answered with HTTP 204 and no body.
Emulator: answers HTTP 200 (replay: `status: expected 204, got 200`).
Why: same as cause 3 — the untag leg of any tag round-trip reports a status AWS does not use for it, so status-sensitive clients and conformance assertions diverge on every successful untag.
Fix: this operation's success-status declaration in its serializer seam (`SUCCESS_STATUS` for the operation), which must be 204. It is a sibling of cause 3 but a separate operation and a separate recorded step, so both must be changed.
Traces: grafana-tag-resource-roundtrip#task#6. Code: services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/untag_resource.rs (SUCCESS_STATUS).

=== Cause 5 of 5: WorkspaceDescription$endpoint is announced as a URL with scheme/host:port/path instead of the workspace's bare host, and ipAddressType is absent ===
AWS: Smithy `WorkspaceDescription$endpoint` is `@required`, length 1..2048, documented "The URL that users can use to access the Grafana console in the workspace"; `WorkspaceDescription$ipAddressType` targets `IPAddressType` with values `IPv4` | `DualStack` ("The type of IP addresses supported for connection to the workspace"). `DescribeWorkspace` is `GET /workspaces/{workspaceId}`, 200. The recording shows `endpoint` is a BARE HOST NAME with no scheme, no port and no path — of the form `<workspaceId>.grafana-workspace.<region>.amazonaws.com`, whose fixed text is the `.grafana-workspace.` label plus `.amazonaws.com` and whose variable parts are the workspace's own id and the region the workspace lives in — and that `ipAddressType` is PRESENT on a workspace created without naming one, carrying the default `IPv4`.
Emulator: reports `endpoint` as a full URL naming the emulator's own serving address and a path segment per workspace (`https://<host>:<port>/grafana-workspaces/<workspaceId>`), and omits `ipAddressType` entirely (replay: `$.workspace.endpoint: value_mismatch`, `$.workspace.ipAddressType: missing (expected 'IPv4', got None)`). The stored record keeps `ip_address_type` as an unset `Option` when the create named none, so no read ever reports a default.
Why: the endpoint is the value every client splices into a Grafana console/API address. A client that builds `https://<endpoint>/api/...` (the documented shape, since the field is a host) produces a double-scheme, double-path address and cannot reach the workspace; a client that treats it as a hostname (DNS, certificate/SNI checks, allow-lists, Host headers) gets a value that is not a hostname at all. The absent `ipAddressType` makes a required-in-practice discriminator invisible: a client that branches on IPv4 vs DualStack sees nothing and cannot tell a single-stack workspace from a dual-stack one, and code that reads the field unconditionally fails on a workspace AWS always reports it for.
Why (host form specifically): the requirement is the SHAPE of the value — a bare, scheme-less, path-less host whose first label is the workspace id — not any particular host, port or address, which differ per environment.
Fix: the workspace record's endpoint composition in this service's state (the per-workspace endpoint stored at create time and reported by `DescribeWorkspace` / `ListWorkspaces`), which must compose a bare host from the workspace id and region rather than the serving URL plus a path prefix; and the create path's defaulting of `ipAddressType` to `IPv4` when the request names none, so every read reports it.
Traces: grafana-workspace-endpoint-is-a-bare-host#task#3. Code: services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/state.rs:61 (LOOPBACK_HOST), :73 (WORKSPACE_API_PATH_PREFIX), :267-273 (Workspace::endpoint), :283 (Workspace::ip_address_type).

## Evidence
Wire replay of trace set simparity-wire-aws-v1: emulator_up=true, bootstrap_signal=false, 19 steps replayed, 14 passed, 5 mismatched, score 73.7%. All 5 divergences are service=grafana, none carries a stub_path and none is owned_by an existing issue.

1. grafana-catalog-list-versions#task#1 — ListVersions, expected 200 / actual 200.
   detail: $.grafanaVersions[2]: missing (expected '12.4', got '10.4'); $.grafanaVersions[3]: missing (expected '13.2', got None); $.grafanaVersions[0]: extra (expected '9.4', got '8.4').
   Expected catalog (recorded, oldest-first): 9.4, 10.4, 12.4, 13.2. Actual: 8.4, 9.4, 10.4.

2. grafana-describe-workspace-malformed-id-rejected#task#2 — DescribeWorkspace, expected 400 / actual 400.
   detail: $.message: value_mismatch.
   Expected message: 'The workspaceId should satisfy the pattern ^g-[0-9a-f]{10}$.'
   Actual message: "1 validation error detected: Value 'not-a-valid-workspace-id' at 'workspaceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^g-[0-9a-f]{10}$"
   (Fixed text differs AND the actual echoes the rejected value, which AWS's form omits.)

3. grafana-tag-resource-roundtrip#task#4 — TagResource. detail: status: expected 204, got 200.

4. grafana-tag-resource-roundtrip#task#6 — UntagResource. detail: status: expected 204, got 200.

5. grafana-workspace-endpoint-is-a-bare-host#task#3 — DescribeWorkspace, expected 200 / actual 200.
   detail: $.workspace.endpoint: value_mismatch (expected 'g-0000000001.grafana-workspace.us-west-2.amazonaws.com', got 'https://127.0.0.1:46179/grafana-workspaces/g-0000000001'); $.workspace.ipAddressType: missing (expected 'IPv4', got None).
   (The recorded host/port in the actual value and the recorded id/region in the expected value are per-environment; the divergence is that the value is a URL with scheme+port+path rather than a bare host whose first label is the workspace id, and that ipAddressType is absent.)

Model facts cited: ListVersions GET /versions 200, output grafanaVersions: GrafanaVersionList (member length 1..255, no value set modeled); DescribeWorkspaceRequest$workspaceId @required @httpLabel pattern ^g-[0-9a-f]{10}$, error ValidationException httpError 400; TagResource POST /tags/{resourceArn} with empty output and code 200 codeDefaulted=true; UntagResource DELETE /tags/{resourceArn} @idempotent with empty output and code 200 codeDefaulted=true; WorkspaceDescription$endpoint @required length 1..2048 ("The URL that users can use to access the Grafana console in the workspace"); WorkspaceDescription$ipAddressType -> IPAddressType values [IPv4, DualStack].

## Steps to Reproduce
Boot the emulator from this checkout and replay the grafana wire fixtures; per cause:

Cause 1 — replay grafana-catalog-list-versions, step task#1: GET /versions with no workspace-id (grafana.ListVersions()) and compare grafanaVersions element-by-element against the recorded oldest-first catalog; the emulator serves 8.4 at index 0 and nothing past 10.4.

Cause 2 — replay grafana-describe-workspace-malformed-id-rejected, step task#2: GET /workspaces/not-a-valid-workspace-id (grafana.DescribeWorkspace(workspaceId='not-a-valid-workspace-id')); assert status 400, code ValidationException and message exactly 'The workspaceId should satisfy the pattern ^g-[0-9a-f]{10}$.'

Cause 3 — replay grafana-tag-resource-roundtrip, step task#4: CreateWorkspace, then POST /tags/<workspace ARN> with a tag map (grafana.TagResource); assert the HTTP status is 204 with no body.

Cause 4 — replay grafana-tag-resource-roundtrip, step task#6: on the same workspace, DELETE /tags/<workspace ARN>?tagKeys=<key> (grafana.UntagResource); assert the HTTP status is 204 with no body.

Cause 5 — replay grafana-workspace-endpoint-is-a-bare-host, step task#3: CreateWorkspace without naming ipAddressType, wait for ACTIVE, then GET /workspaces/{workspaceId} (grafana.DescribeWorkspace); assert workspace.endpoint is a bare host (no '://', no ':<port>', no '/') whose first DNS label is the workspace id and whose remainder is grafana-workspace.<region>.amazonaws.com, and assert workspace.ipAddressType == 'IPv4'.
