Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-theme-for-stack-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-theme-for-stack-lifecycle
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|CreateThemeForStack|2xx->2xx|$.Theme.ThemeFaviconURL
Issue-Key: appstream|CreateThemeForStack|2xx->2xx|$.Theme.ThemeOrganizationLogoURL
Issue-Key: appstream|CreateThemeForStack|2xx->2xx|$.Theme.faviconURL
Issue-Key: appstream|CreateThemeForStack|2xx->2xx|$.Theme.organizationLogoURL
Issue-Key: appstream|CreateThemeForStack|2xx->2xx|$.Theme.stylingName
Issue-Key: appstream|CreateThemeForStack|2xx->2xx|$.Theme.stylingProperty
Issue-Key: appstream|CreateThemeForStack|2xx->2xx|$.Theme.themeName
Issue-Key: appstream|CreateThemeForStack|2xx->2xx|$.Theme.titleText
Issue-Key: appstream|DescribeThemeForStack|2xx->2xx|$.Theme.ThemeFaviconURL
Issue-Key: appstream|DescribeThemeForStack|2xx->2xx|$.Theme.ThemeOrganizationLogoURL
Issue-Key: appstream|DescribeThemeForStack|2xx->2xx|$.Theme.faviconURL
Issue-Key: appstream|DescribeThemeForStack|2xx->2xx|$.Theme.organizationLogoURL
Issue-Key: appstream|DescribeThemeForStack|2xx->2xx|$.Theme.stylingName
Issue-Key: appstream|DescribeThemeForStack|2xx->2xx|$.Theme.stylingProperty
Issue-Key: appstream|DescribeThemeForStack|2xx->2xx|$.Theme.themeName
Issue-Key: appstream|DescribeThemeForStack|2xx->2xx|$.Theme.titleText
Issue-Key: appstream|UpdateThemeForStack|2xx->2xx|$.Theme.ThemeFaviconURL
Issue-Key: appstream|UpdateThemeForStack|2xx->2xx|$.Theme.ThemeOrganizationLogoURL
Issue-Key: appstream|UpdateThemeForStack|2xx->2xx|$.Theme.faviconURL
Issue-Key: appstream|UpdateThemeForStack|2xx->2xx|$.Theme.organizationLogoURL
Issue-Key: appstream|UpdateThemeForStack|2xx->2xx|$.Theme.stylingName
Issue-Key: appstream|UpdateThemeForStack|2xx->2xx|$.Theme.stylingProperty
Issue-Key: appstream|UpdateThemeForStack|2xx->2xx|$.Theme.themeName
Issue-Key: appstream|UpdateThemeForStack|2xx->2xx|$.Theme.titleText
Issue-Commit: 1573814604c649bc1493e5e75c073e3af34827d4

# appstream Theme projection omits the six lowercase branding members AWS serves on every theme response

> _Note: this issue was identified against the code base at commit `1573814604c649bc1493e5e75c073e3af34827d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateThemeForStack

## Description
Three net-new causes on ONE fault surface: the `Theme` structure this service projects into a 200 response. Every theme operation that returns a `Theme` renders only the members the Smithy model declares and drops the SIX additional lowercase members AppStream actually puts on the wire (`themeName`, `titleText`, `stylingName`, `stylingProperty`, `organizationLogoURL`, `faviconURL`). The three causes are the same omission observed through the three operations that project a theme, so one projection fix closes all three; each is a separate replay cluster with EMPTY owned_by and EMPTY stub_path, so no filed issue covers any of them.

Shared AWS requirement (recorded behavior only for the six members): the on-disk model's `com.amazonaws.appstream#Theme` declares EXACTLY eight members — StackName (Name, pattern ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$), State (ThemeState enum), ThemeTitleText (@length 1..300, pattern ^[-@./#&+\w\s]*$), ThemeStyling (enum values LIGHT_BLUE|BLUE|PINK|RED), ThemeFooterLinks (list), ThemeOrganizationLogoURL (String @length min 1), ThemeFaviconURL (String @length min 1), CreatedTime (timestamp). The RECORDED AWS response for the same operations carries those eight AND six more that the model does not declare, so the model says nothing about them and the recording is the only authority:
  - `themeName`: the service-assigned name of the branding record, built as the fixed prefix `theme_` followed by the stack's name (variable part: the request's `StackName`).
  - `titleText`: the same text as `ThemeTitleText` (variable part: the request's `TitleText`).
  - `stylingName`: the human-readable name of the selected `ThemeStyling` enum value, as a fixed per-enum label with its hex colour in parentheses (the recording shows the `BLUE` styling rendered as `Blue (#0070BA)`); it is a function of the enum value, not a per-run value.
  - `stylingProperty`: the colour map the selected styling expands to, with the fixed keys `backgroundColor`, `bodyTextColor`, `buttonColor`, `linkColor`, `spinnerColor` and the hex values that styling defines (the recording shows the `BLUE` styling expanding to backgroundColor #FFFFFF, bodyTextColor #666666, buttonColor #8AC53E, linkColor #0070BA, spinnerColor #666666).
  - `organizationLogoURL` / `faviconURL`: the asset URLs, the same values as `ThemeOrganizationLogoURL` / `ThemeFaviconURL`.
All three causes are 200-status responses; no status or error code diverges, so nothing here is an error-mapping finding.

Scope note on the asset URLs: the replay also reports `$.Theme.ThemeFaviconURL` and `$.Theme.ThemeOrganizationLogoURL` as value_mismatch against the recorded placeholder `<shaped-not-asserted>`, i.e. the corpus does NOT assert those URLs' text — the concrete host/bucket in the emulator's value is environment-specific and is NOT a requirement. The requirement these clusters establish is only that the lowercase `organizationLogoURL`/`faviconURL` members are PRESENT alongside the capitalised ones; their URL text is whatever the emulator's asset-URL rule produces.

Related open issue that does NOT cover these: 20261105T091442Z-appstream-theme-for-stack-lifecycle-diverges-a-created.md is scoped to the theme lifecycle answering not-found (HTTP 404 with ResourceNotFoundException) for a theme an accepted create established, and to theme not-found refusals leaving with 404 instead of 400. Its Issue-Keys are all `2xx->4xx` and `4xx->4xx` status keys. The three clusters below are `status=200->200` keyed on `$.Theme.*` members: the status is now correct and the theme IS found, and what remains is the body's member set. That issue's fix closes the lookup, not the projection, so these causes survive it.

Cause 1 of 3: CreateThemeForStack's 200 response projects a Theme missing the six lowercase members.
AWS: returns HTTP 200 with the created `Theme` carrying both the modelled members and the six above (Smithy: appstream#CreateThemeForStack input {StackName required, TitleText, ThemeStyling, OrganizationLogoS3Location required, FaviconS3Location required, FooterLinks} -> output {Theme}; the six members are recorded behavior only, with `themeName` = `theme_` + the request's StackName, `titleText` = the request's TitleText, and `stylingName`/`stylingProperty` derived from the requested `ThemeStyling`).
Emulator: answers HTTP 200 but with `$.Theme.themeName`, `$.Theme.titleText`, `$.Theme.stylingName`, `$.Theme.stylingProperty`, `$.Theme.faviconURL` and `$.Theme.organizationLogoURL` all absent (replay detail: each reported `missing (... got None)`).
Why: a client that reads the branding back from the create response to confirm what the service stored — the console-style branding flow keys on `titleText` and `stylingProperty` to render the theme it just saved — gets null for every one of them and cannot tell an accepted create from one that stored nothing; code that derives the theme's identity from `themeName` has no identity to use.
Fix: the `Theme` projection shared by the theme handlers — `ThemeRecord::to_theme` in services/service-com-amazonaws-appstream-photonadminproxyservice/src/state.rs, which builds the response structure from the stored record. The generated `Theme` type is derived from the Smithy model and therefore has no fields for the six undeclared members, so the projection (or the operation's output serializer) must emit them alongside the modelled ones; `stylingName`/`stylingProperty` belong in a single styling-to-label/colour-map table keyed by the `ThemeStyling` enum value so all three operations agree.
Traces: appstream-theme-for-stack-lifecycle#task#7.

Cause 2 of 3: DescribeThemeForStack's 200 response projects the same incomplete Theme.
AWS: returns HTTP 200 with the stored `Theme` (Smithy: appstream#DescribeThemeForStack input {StackName required} -> output {Theme}, errors [OperationNotPermittedException 400, ResourceNotFoundException]), including the six lowercase members as recorded.
Emulator: answers HTTP 200 with the same six members absent (replay detail: `$.Theme.themeName`, `$.Theme.titleText`, `$.Theme.stylingName`, `$.Theme.stylingProperty`, `$.Theme.faviconURL`, `$.Theme.organizationLogoURL` all `missing`).
Why: the read-back path a client uses to inspect existing branding reports a theme whose title, styling and asset URLs are all null, so drift detection against a desired configuration always sees a difference and the correction loop never converges.
Fix: same projection (`ThemeRecord::to_theme` in src/state.rs), reached from src/ops/describe_theme_for_stack.rs.
Traces: appstream-theme-for-stack-lifecycle#task#8 (the cluster also covers appstream-theme-for-stack-lifecycle#task#10).

Cause 3 of 3: UpdateThemeForStack's 200 response projects the same incomplete Theme.
AWS: returns HTTP 200 with the UPDATED `Theme` (Smithy: appstream#UpdateThemeForStack input {StackName required, FooterLinks, TitleText @length 1..300, ThemeStyling enum, OrganizationLogoS3Location, FaviconS3Location, State: ThemeState, AttributesToDelete: ThemeAttributes (enum ThemeAttribute values [FOOTER_LINKS])} -> output {Theme}), again including the six lowercase members, with `titleText`/`stylingName`/`stylingProperty` reflecting the members the update applied.
Emulator: answers HTTP 200 with the same six members absent.
Why: an update cannot be verified from its own response — the caller that just set a title or a styling reads null back for both, so it cannot confirm the write took effect and must guess.
Fix: same projection (`ThemeRecord::to_theme` in src/state.rs), reached from src/ops/update_theme_for_stack.rs after the update is applied, so the recomputed `stylingName`/`stylingProperty` follow the new `ThemeStyling`.
Traces: appstream-theme-for-stack-lifecycle#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator built from this checkout (emulator_up true, detail "/__admin/health answered 200 and the :44685 data listener accepts"; 18 fixtures replayed, 162 steps: 54 passed, 73 mismatched, 35 blocked, 0 errored; score 33.3%; 20 divergence clusters). All three clusters below carry owned_by [] and stub_path "", i.e. the replay reports them as unowned and not blocked by a stub.

Cause 1 - CreateThemeForStack, fixture appstream-theme-for-stack-lifecycle, step task#7:
  expected_status 200, actual_status 200 (status agrees).
  divergence_key: service=appstream|CreateThemeForStack|status=200->200|keys=$.Theme.ThemeFaviconURL,$.Theme.ThemeOrganizationLogoURL,$.Theme.faviconURL,$.Theme.organizationLogoURL,$.Theme.stylingName,$.Theme.stylingProperty,$.Theme.themeName,$.Theme.titleText; affected_count 1.
  detail:
    $.Theme.faviconURL: missing (expected '<shaped-not-asserted>', got None)
    $.Theme.organizationLogoURL: missing (expected '<shaped-not-asserted>', got None)
    $.Theme.stylingName: missing (expected 'Blue (#0070BA)', got None)
    $.Theme.stylingProperty: missing (expected {'backgroundColor': '#FFFFFF', 'bodyTextColor': '#666666', 'buttonColor': '#8AC53E', 'linkColor': '#0070BA', 'spinnerColor': '#666666'}, got None)
    $.Theme.themeName: missing (expected 'theme_sim2real-probe-appstream-theme-stack-240d451f', got None)
    $.Theme.titleText: missing (expected 'Sim2Real Probe Theme', got None)
    $.Theme.ThemeFaviconURL: value_mismatch (expected '<shaped-not-asserted>', got 'https://sim2real-probe-ast-<uuid>.s3.us-west-2.amazonaws.com/theme/favicon.ico')
    $.Theme.ThemeOrganizationLogoURL: value_mismatch (expected '<shaped-not-asserted>', got 'https://sim2real-probe-ast-<uuid>.s3.us-west-2.amazonaws.com/theme/logo.png')
  Reading of the values: `theme_sim2real-probe-appstream-theme-stack-240d451f` is this run's stack name filling the fixed `theme_` prefix; `Sim2Real Probe Theme` is this run's requested TitleText; `Blue (#0070BA)` and the five-key colour map are the fixed expansion of the requested `ThemeStyling` (BLUE) and are NOT per-run values. The two `<shaped-not-asserted>` expectations are unasserted placeholders, so the bucket host and object keys in the emulator's URLs are environment-specific and are not a requirement - only the PRESENCE of the lowercase `faviconURL`/`organizationLogoURL` members is.

Cause 2 - DescribeThemeForStack, fixture appstream-theme-for-stack-lifecycle, step task#8:
  expected_status 200, actual_status 200.
  divergence_key: service=appstream|DescribeThemeForStack|status=200->200|keys=$.Theme.ThemeFaviconURL,$.Theme.ThemeOrganizationLogoURL,$.Theme.faviconURL,$.Theme.organizationLogoURL,$.Theme.stylingName,$.Theme.stylingProperty,$.Theme.themeName,$.Theme.titleText; affected_count 2 (appstream-theme-for-stack-lifecycle#task#8, appstream-theme-for-stack-lifecycle#task#10).
  detail: byte-for-byte the same six `missing` members and the same two unasserted URL value_mismatches as Cause 1.

Cause 3 - UpdateThemeForStack, fixture appstream-theme-for-stack-lifecycle, step task#9:
  expected_status 200, actual_status 200.
  divergence_key: service=appstream|UpdateThemeForStack|status=200->200|keys=$.Theme.ThemeFaviconURL,$.Theme.ThemeOrganizationLogoURL,$.Theme.faviconURL,$.Theme.organizationLogoURL,$.Theme.stylingName,$.Theme.stylingProperty,$.Theme.themeName,$.Theme.titleText; affected_count 1.
  detail: the same six `missing` members and the same two unasserted URL value_mismatches.

That the three clusters share an identical key list and identical detail across create, describe and update is the evidence that this is ONE projection defect rather than three handler defects.

Smithy evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): shape com.amazonaws.appstream#Theme declares exactly {StackName -> Name (pattern ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$), State -> ThemeState, ThemeTitleText (@length min 1 max 300, pattern ^[-@./#&+\w\s]*$), ThemeStyling -> ThemeStyling, ThemeFooterLinks -> ThemeFooterLinks, ThemeOrganizationLogoURL -> String @length min 1, ThemeFaviconURL -> String @length min 1, CreatedTime -> Timestamp}; com.amazonaws.appstream#ThemeStyling values [LIGHT_BLUE, BLUE, PINK, RED]; com.amazonaws.appstream#ThemeAttribute values [FOOTER_LINKS]. None of themeName/titleText/stylingName/stylingProperty/organizationLogoURL/faviconURL appears in the model, which is why they are cited as recorded behavior only.

Code evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/src): ops/create_theme_for_stack.rs lines 199-219 build a `crate::state::ThemeRecord` (stack_name, state, title_text, styling, footer_links, organization_logo_url/favicon_url via crate::state::theme_asset_url, created_time) and return `record.to_theme()` as the sole response projection; src/state.rs `ThemeRecord::to_theme` is that projection and is the single place all three operations' bodies come from; the stored record already holds the title and styling the six missing members are derived from, so the data exists and only the projection drops it.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture appstream-theme-for-stack-lifecycle to step task#7 - the CreateThemeForStack that brands the trajectory's stack. Observe HTTP 200 whose `Theme` carries the capitalised modelled members but NO themeName, titleText, stylingName, stylingProperty, organizationLogoURL or faviconURL, where the recording carries all six.
4. Cause 2: same fixture, step task#8 - DescribeThemeForStack for that stack. Observe HTTP 200 with the identical six members absent. The same cluster reproduces at task#10.
5. Cause 3: same fixture, step task#9 - UpdateThemeForStack setting State on that theme. Observe HTTP 200 with the identical six members absent, including after the update applied.
6. Confirm the fix by re-replaying the fixture and checking that all three steps (task#7, task#8, task#9) plus task#10 pass: each `Theme` must carry themeName (`theme_` + StackName), titleText (the stored title), stylingName and stylingProperty (the label and five-key colour map for the stored ThemeStyling enum value), and organizationLogoURL/faviconURL alongside the existing ThemeOrganizationLogoURL/ThemeFaviconURL.
7. Scope cross-check: 20261105T091442Z-appstream-theme-for-stack-lifecycle-diverges-a-created.md asserts only status transitions (2xx->4xx and 4xx->4xx) on the theme operations; these three clusters are status=200->200 body-member divergences and are reported by the replay with empty owned_by, so closing that issue does not close these.
