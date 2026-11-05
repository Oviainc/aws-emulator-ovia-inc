Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-theme-for-stack-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-theme-for-stack-lifecycle
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|DescribeThemeForStack|2xx->4xx|$.Theme
Issue-Key: appstream|DescribeThemeForStack|2xx->4xx|status
Issue-Key: appstream|UpdateThemeForStack|2xx->4xx|$.Theme
Issue-Key: appstream|UpdateThemeForStack|2xx->4xx|status
Issue-Key: appstream|DeleteThemeForStack|2xx->4xx|$
Issue-Key: appstream|DeleteThemeForStack|2xx->4xx|status
Issue-Key: appstream|DescribeThemeForStack|4xx->4xx|status
Issue-Key: appstream|UpdateThemeForStack|4xx->4xx|status
Issue-Commit: 717842165548985788beee9e72dfb452182d2228

# appstream theme-for-stack lifecycle diverges: a created theme is invisible to every theme read/update/delete, and theme not-found refusals answer HTTP 404

> _Note: this issue was identified against the code base at commit `717842165548985788beee9e72dfb452182d2228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeThemeForStack

## Description
Five net-new causes, all on the SAME fault surface: AppStream's theme-for-stack (custom branding) lifecycle. The theme a successful CreateThemeForStack must have registered is not there for any later theme operation (Causes 1-3), and when a theme genuinely is absent the refusal leaves with the wrong HTTP status (Causes 4-5). Every cause below is a replay cluster with EMPTY owned_by and EMPTY stub_path, so no filed issue covers it.

Related open issues that do NOT cover these: 20261103T084027Z-appstream-read-after-create-on-a-stack-answers.md is the same CLASS of defect (read-after-create) but is scoped to the STACK registry and to DescribeStacks/DeleteStack, not to the theme store or any theme operation; 20261102T092115Z-appstream-stack-not-found-refusals-diverge-wrong-http.md is the same CLASS of status defect but is scoped to stack-scoped not-found refusals on DescribeStacks/UpdateStack/DeleteStack. A fixer should expect Causes 1-3 and Causes 4-5 to share a mechanism with those two issues respectively, but the theme operations are not exercised or asserted by either.

Cause 1 of 5: DescribeThemeForStack on a stack whose theme the trajectory already created answers a not-found refusal instead of the theme.
AWS: DescribeThemeForStack returns HTTP 200 with a `Theme` describing the branding stored for the named stack (Smithy: appstream#DescribeThemeForStack, input {StackName: Name required, pattern ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$}, output {Theme: Theme}, errors [OperationNotPermittedException 400, ResourceNotFoundException]). ResourceNotFoundException is reserved for a stack that does not exist or a stack that has no branding; a theme established by an accepted CreateThemeForStack must be reported until it is deleted. The recording shows the theme reported with State ENABLED, the StackName, CreatedTime, ThemeStyling, ThemeTitleText and the asset URLs derived from the S3 locations supplied at create time.
Emulator: answers HTTP 404 with ResourceNotFoundException and NO `Theme` member (replay detail: "status: expected 200, got 404" / "$.Theme: missing"). The handler reaches its not-found branch only because the theme store has no entry under the requested stack name, i.e. the record a successful create must have left was never inserted or did not survive to this step.
Hypothesis (mechanism, not observed directly): the CreateThemeForStack that establishes the branding in this trajectory never lands on the emulator or its write does not survive into the next step - the fixture's successful create produces no CreateThemeForStack divergence cluster of its own, and the emulator's own refusal on the later DeleteThemeForStack carries the THEME-not-found sentence, which that handler emits purely from a miss in the theme map. The requirement being broken either way is read-after-create on the theme: branding the service accepted must be readable until it is deleted.
Why: the standard provisioning flow - create branding, then read it back to confirm State ENABLED - never converges; a client that probes for existing branding before creating reads "absent" for branding that exists, and an HTTP-level client sees 404 and treats the call as a routing failure rather than a modeled service answer.
Fix: the theme registry write/read path - CreateThemeForStack's insertion into the service store and the key DescribeThemeForStack resolves against (services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/create_theme_for_stack.rs, src/ops/describe_theme_for_stack.rs, src/state.rs `themes`); the describe handler must reach its not-found branch only for a stack no accepted create ever branded.
Traces: appstream-theme-for-stack-lifecycle#task#8 (representative; the cluster also covers appstream-theme-for-stack-lifecycle#task#10).

Cause 2 of 5: UpdateThemeForStack on that same existing theme answers a not-found refusal instead of applying the update.
AWS: UpdateThemeForStack returns HTTP 200 with the UPDATED `Theme` (Smithy: appstream#UpdateThemeForStack, input {StackName required, FooterLinks, TitleText len 1..300, ThemeStyling enum, OrganizationLogoS3Location, FaviconS3Location, State: ThemeState enum, AttributesToDelete: ThemeAttributes}, output {Theme}, errors [ConcurrentModificationException 400, InvalidAccountStatusException 400, InvalidParameterCombinationException 400, LimitExceededException 400, OperationNotPermittedException 400, ResourceNotFoundException]). The recording shows the request setting `State` and the response reporting the theme with State DISABLED, every other member unchanged from what the create persisted.
Emulator: answers HTTP 404 with ResourceNotFoundException and NO `Theme` member (replay detail: "status: expected 200, got 404" / "$.Theme: missing"). src/ops/update_theme_for_stack.rs refuses at its theme lookup before applying any member, for the same missing registry record as Cause 1.
Why: branding cannot be enabled/disabled or re-styled at all - every update is rejected as if the stack had never been branded, so a client's configuration drift-correction loop fails permanently rather than converging.
Fix: same theme registry write/read path, exercised from the update handler (src/ops/update_theme_for_stack.rs theme lookup against src/state.rs `themes`).
Traces: appstream-theme-for-stack-lifecycle#task#9.

Cause 3 of 5: DeleteThemeForStack on that same existing theme answers a not-found refusal instead of deleting it.
AWS: DeleteThemeForStack returns HTTP 200 with an EMPTY result body (Smithy: appstream#DeleteThemeForStack, input {StackName required}, output {}, errors [ConcurrentModificationException 400, OperationNotPermittedException 400, ResourceNotFoundException]).
Emulator: answers HTTP 404 with a ResourceNotFoundException body where AWS sends 200 and `{}` (replay detail: "status: expected 200, got 404" / "$: value_mismatch (expected {}, got {'__type': 'ResourceNotFoundException', 'Message': ...})"). The refusal's fixed text is "The theme for stack <stack-name> for account <account-id>  does not exist." with the stack name taken from the request's `StackName` and the account id from the caller's account - correct wording for branding that is absent, but it is not supposed to be absent here. src/ops/delete_theme_for_stack.rs refuses when its removal from the theme map misses, i.e. the same missing record as Causes 1-2.
Why: teardown is unreliable in the wrong direction - a client cleaning up its own branding is told it never existed, so it cannot distinguish a leaked resource from a completed delete, and a 404 drives retry/branch logic differently from AWS's 200.
Fix: same theme registry write/read path, exercised from the delete handler (src/ops/delete_theme_for_stack.rs removal against src/state.rs `themes`).
Traces: appstream-theme-for-stack-lifecycle#task#11.

Cause 4 of 5: DescribeThemeForStack on a stack that genuinely has no theme answers HTTP 404 instead of HTTP 400.
AWS: AppStream 2.0 is awsJson1_1 and the recorded wire status for this refusal is HTTP 400 carrying `__type: ResourceNotFoundException` and the `Message` sentence above; note the on-disk model's `httpError` for ResourceNotFoundException is the protocol default 404, so the RECORDED status (400) is the requirement, not the defaulted trait (Smithy: DescribeThemeForStack errors [OperationNotPermittedException 400, ResourceNotFoundException]).
Emulator: emits the CORRECT body - the replay reports only "status: expected 400, got 404", so `__type` and the message text already match - but with HTTP 404.
Why: an SDK retry/branch policy keyed on status sees a 404 where AWS sends 400; HTTP-level clients and proxies treat the response as a routing failure rather than a modeled service refusal, so the modeled ResourceNotFoundException is never surfaced as such.
Fix: the status mapping for appstream's ResourceNotFoundException in the service's error serializer, so every theme-scoped not-found refusal leaves with 400 and `__type: ResourceNotFoundException` (the same mapping named by 20261102T092115Z-appstream-stack-not-found-refusals-diverge-wrong-http.md for the stack-scoped refusals, which does not assert any theme operation).
Traces: appstream-theme-for-stack-lifecycle#task#12.

Cause 5 of 5: UpdateThemeForStack on a stack that genuinely has no theme answers HTTP 404 instead of HTTP 400.
AWS: Smithy: UpdateThemeForStack errors include ResourceNotFoundException; the recorded wire status for that refusal is HTTP 400 with `__type: ResourceNotFoundException` and the theme-not-found sentence (fixed text "The theme for stack <stack-name> for account <account-id>  does not exist.", variable parts the request's StackName and the caller's account id).
Emulator: body matches; status is 404 (replay detail: "status: expected 400, got 404").
Why: same as Cause 4 - status-keyed client logic mis-classifies a modeled refusal as a transport/routing failure.
Fix: the same error-serializer status mapping, exercised from the update handler.
Traces: appstream-theme-for-stack-lifecycle#task#13.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, detail "/__admin/health answered 200 and the :44571 data listener accepts"; 18 fixtures replayed, 162 steps, 38 passed, 71 mismatched, 53 blocked, 0 errored, score 23.5%, 20 divergence clusters). All five clusters below carry owned_by [] and stub_path "".

Cause 1 - DescribeThemeForStack, fixture appstream-theme-for-stack-lifecycle, step task#8:
  expected_status 200, actual_status 404.
  detail: "status: expected 200, got 404" and "$.Theme: missing (expected {'CreatedTime': 1793132718.422, 'StackName': 'sim2real-probe-appstream-theme-stack-d53526fa', 'State': 'ENABLED', 'ThemeFaviconURL': '<shaped-not-asserted>', 'ThemeOrganizationLogoURL': '<shaped-not-asserted>', 'ThemeStyling': 'BLUE', 'ThemeTitleText': 'Sim2Real Probe Theme', 'faviconURL': '<shaped-not-asserted>', 'organizationLogoURL': '<shaped-not-asserted>', 'stylingName': 'Blue (#0070BA)', 'stylingProperty': {...}, 'themeName': 'theme_<stack name>', 'titleText': 'Sim2Real Probe Theme'}, got None)".
  The stack name, CreatedTime and title text above are this run's values filling the recorded shape; the requirement is that an existing theme is returned at all, with State ENABLED and the styling/title the create supplied.
  divergence_key service=appstream|DescribeThemeForStack|status=200->404|keys=$.Theme,status; affected_count 2 (appstream-theme-for-stack-lifecycle#task#8, appstream-theme-for-stack-lifecycle#task#10).

Cause 2 - UpdateThemeForStack, fixture appstream-theme-for-stack-lifecycle, step task#9:
  expected_status 200, actual_status 404.
  detail: "status: expected 200, got 404" / "$.Theme: missing (expected {... 'State': 'DISABLED' ...}, got None)".
  divergence_key service=appstream|UpdateThemeForStack|status=200->404|keys=$.Theme,status; affected_count 1.

Cause 3 - DeleteThemeForStack, fixture appstream-theme-for-stack-lifecycle, step task#11:
  expected_status 200, actual_status 404.
  detail: "status: expected 200, got 404" / "$: value_mismatch (expected {}, got {'__type': 'ResourceNotFoundException', 'Message': 'The theme for stack sim2real-probe-appstream-theme-stack-d53526fa for account 000000000000  does not exist.'})" (the stack name and account id are this run's values in the fixed sentence; the doubled space before "does not exist" is AWS's own template, see src/state.rs theme_not_found_message).
  divergence_key service=appstream|DeleteThemeForStack|status=200->404|keys=$,status; affected_count 1.

Cause 4 - DescribeThemeForStack, fixture appstream-theme-for-stack-lifecycle, step task#12:
  expected_status 400, actual_status 404. detail: "status: expected 400, got 404" ONLY - the error body (__type ResourceNotFoundException and the theme-not-found message) already matches the recording.
  divergence_key service=appstream|DescribeThemeForStack|status=400->404|keys=status; affected_count 1.

Cause 5 - UpdateThemeForStack, fixture appstream-theme-for-stack-lifecycle, step task#13:
  expected_status 400, actual_status 404. detail: "status: expected 400, got 404" ONLY; body matches.
  divergence_key service=appstream|UpdateThemeForStack|status=400->404|keys=status; affected_count 1.

Code evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/src):
  ops/create_theme_for_stack.rs lines 199-219 - the ONLY insertion into `state.themes`, keyed by the request's StackName, after a stack-exists check (lines 173-183) and a ResourceAlreadyExists check (lines 187-197).
  ops/describe_theme_for_stack.rs lines 145-164 - stack-not-found then theme-not-found branches; the theme branch is a plain miss on `state.themes`.
  ops/update_theme_for_stack.rs lines 201-205 - `state.themes.get_mut(...)` miss raises ResourceNotFoundException before any member is applied.
  ops/delete_theme_for_stack.rs lines 143-150 - `state.themes.remove(...)` miss raises ResourceNotFoundException; this handler does NOT consult `state.stacks`, so its refusal is evidence of a theme-store miss specifically.
  state.rs lines 109-138 - THEME_KIND, theme_not_found_message (fixed sentence with the doubled space) and THEME_STATE_CREATED = "ENABLED", i.e. the emulator already intends State ENABLED for a created theme.

Smithy evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): CreateThemeForStack input {StackName required, TitleText len 1..300, ThemeStyling enum, OrganizationLogoS3Location required, FaviconS3Location required, FooterLinks} -> output {Theme}, errors [... ResourceAlreadyExistsException 400, ResourceNotFoundException]; DescribeThemeForStack input {StackName required} -> output {Theme}, errors [OperationNotPermittedException 400, ResourceNotFoundException]; UpdateThemeForStack input adds {State: ThemeState, AttributesToDelete} -> output {Theme}, errors [... InvalidParameterCombinationException 400, ResourceNotFoundException]; DeleteThemeForStack input {StackName required} -> output {} , errors [ConcurrentModificationException 400, OperationNotPermittedException 400, ResourceNotFoundException]. ResourceNotFoundException's httpError in the model is the protocol default 404, while the recorded wire status for the refusal is 400.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture appstream-theme-for-stack-lifecycle and stop at step task#8 - a DescribeThemeForStack for the stack whose theme an earlier step created. Observe HTTP 404 with ResourceNotFoundException and no Theme, where the fixture expects HTTP 200 with Theme.State ENABLED. The same cluster reproduces at task#10.
4. Cause 2: same fixture, step task#9 - an UpdateThemeForStack setting State on that theme. Observe HTTP 404 not-found where the fixture expects HTTP 200 with the theme reported State DISABLED.
5. Cause 3: same fixture, step task#11 - a DeleteThemeForStack for that theme. Observe HTTP 404 with the theme-not-found ResourceNotFoundException body where the fixture expects HTTP 200 and an empty body {}.
6. Cause 4: same fixture, step task#12 - a DescribeThemeForStack for a stack that now has no theme. Observe the correct ResourceNotFoundException body served with HTTP 404 where the fixture expects HTTP 400.
7. Cause 5: same fixture, step task#13 - an UpdateThemeForStack for that same unbranded stack. Observe the correct refusal body served with HTTP 404 instead of HTTP 400.
8. Cross-check scope: the stack-scoped equivalents of Causes 1-3 are already filed as 20261103T084027Z-appstream-read-after-create-on-a-stack-answers.md and of Causes 4-5 as 20261102T092115Z-appstream-stack-not-found-refusals-diverge-wrong-http.md; neither replays any theme step, and all five clusters above are reported with empty owned_by.
