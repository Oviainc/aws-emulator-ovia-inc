Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.add-region-duplicate-rejected ds.connect-directory-invalid-vpc-settings-rejected ds.describe-directories-absent-id-rejected ds.directory-settings-reject-unknown-names-and-values-outside-allowed-values ds.remove-region-access-denied ds.simple-ad-small-lifecycle-to-active-then-deleted ds.disable-directory-data-access-recloses-the-door-without-losing-contents
Issue-Key: ds.add-region-duplicate-rejected|run#7|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: ds.connect-directory-invalid-vpc-settings-rejected|run#21|AssertionError|assert bad_message == (
Issue-Key: ds.describe-directories-absent-id-rejected|run#15|AssertionError|assert both_message == expected_both, (
Issue-Key: ds.directory-settings-reject-unknown-names-and-values-outside-allowed-values|_allowed#11|AssertionError|assert isinstance(parsed, list), (
Issue-Key: ds.remove-region-access-denied|run#20|AssertionError|assert denied_message == expected, (
Issue-Key: ds.simple-ad-small-lifecycle-to-active-then-deleted|run#31|AssertionError|assert re.fullmatch(r"[A-Za-z0-9.-]+(:\d+)?", access_url), (
Issue-Key: ds.disable-directory-data-access-recloses-the-door-without-losing-contents|_poll#32|Timeout|time.sleep(interval_s)
Issue-Commit: 12bd58142e134c788a6e6e722018c3787d666c1f

# ds: 7 net-new causes — four refusal/report strings are emulator-authored (AddRegion home-region, ConnectDirectory subnet rule, DescribeDirectories id list, IAM AccessDenied suffix), AllowedValues is not a JSON array, AccessUrl carries a scheme, and the data-access disable/re-enable lifecycle never completes

> _Note: this issue was identified against the code base at commit `12bd58142e134c788a6e6e722018c3787d666c1f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRegion, ConnectDirectory, DescribeDirectories, DescribeSettings, RemoveRegion, EnableDirectoryDataAccess/DisableDirectoryDataAccess

## Description
Seven distinct root causes in the Directory Service emulator, all grounded in behavioral checks the suite reports under `### ds` that are neither ALREADY FILED nor OWNED by an open issue. All seven are in one service, so they are bundled here; a fixer must close ALL SEVEN before submitting.

Cause 1 of 7: AddRegion targeting the directory's own home Region is refused with an emulator-authored sentence.
  AWS: recorded behavior only (the model declares UnsupportedOperationException for AddRegion but documents no message). The recording shows HTTP 400, code `UnsupportedOperationException`, and a message whose text is ENTIRELY FIXED — it quotes neither the Region nor the directory id: "The region you specified is the same region where the AWS Managed Microsoft AD directory was created. Specify a different region and try again." followed by this service's request-id suffix " : RequestId: <this call's request id>" (the only variable part).
  Emulator: answers the right code but its own sentence, which interpolates the Region and the directory id and drops the closing period: "Region <region> is the primary Region of directory <directory-id>; specify a different Region and try again : RequestId: <id>" (observed with region `us-west-2` and directory `d-003ba5fe5a` — those are values from the request/state, not part of the template).
  Why: a client (or an operator runbook) that matches on AWS's fixed sentence to distinguish "you named your own home Region" from the other UnsupportedOperationException cases never matches, and anyone parsing the message learns a different set of facts than AWS states.
  Fix: the AddRegion handler's home-Region guard and the diagnostic it builds (this service's shared refusal-message layer in state.rs).
  Traces: ds.add-region-duplicate-rejected.

Cause 2 of 7: ConnectDirectory's one-subnet refusal names the rule in the emulator's own words.
  AWS: recorded behavior only; Smithy: ConnectDirectory models InvalidParameterException (HTTP 400). The recorded refusal is 400 `InvalidParameterException` with the FIXED text "Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones." plus the " : RequestId: <id>" suffix; it quotes no subnet id and no count.
  Emulator: same code, but the second sentence is rewritten: "Invalid subnet ID(s). The two subnets must be in different Availability Zones."
  Why: the two sentences are not interchangeable to a client that string-matches the subnet-count rule, and the divergence is in fixed text, not in a per-run value.
  Fix: the shared subnet-rule diagnostic constant `INVALID_SUBNET_IDS` used by every create that places servers in a VPC (ConnectDirectory, CreateDirectory, CreateMicrosoftAD, AddRegion). NOTE: open issues 20261106T210535Z-ds-createmicrosoftad-subnet-pair-refusal-sentence-is-emulator.md and 20261106T203453Z-ds-ec2-ds-2-net-new-causes-describedirectories-vpcsettings-omits.md describe the same constant as reached through CreateMicrosoftAD; neither covers the ConnectDirectory check below, which is still failing, and the fix is one constant.
  Traces: ds.connect-directory-invalid-vpc-settings-rejected.

Cause 3 of 7: DescribeDirectories joins unresolvable ids with ", " instead of ",".
  AWS: recorded behavior only; Smithy: DescribeDirectories models EntityDoesNotExistException (HTTP 400). The recorded message is "Invalid directory ID(s): <id>,<id>." — the fixed prefix "Invalid directory ID(s): ", then every id the store could not resolve in filter order separated by a BARE COMMA with NO space, closed by a period, then the " : RequestId: <id>" suffix. The ids themselves are values from the request.
  Emulator: emits the same template but joins with ", " (comma + space): "Invalid directory ID(s): d-2ee16dd93c, d-52de831cee."
  Why: a client splitting the message on "," to recover which ids were rejected gets ids with a leading space, and any exact-match assertion on AWS's wording fails.
  Fix: the `invalid_directory_ids` diagnostic in this service's shared refusal layer (state.rs) — the list separator.
  Traces: ds.describe-directories-absent-id-rejected.

Cause 4 of 7: DescribeSettings reports `AllowedValues` as a bare comma-separated list instead of a JSON array.
  AWS: Smithy: `SettingEntry$AllowedValues` — "The valid range of values for the directory setting. These values depend on the DataType of your directory" (the model does not state the encoding). Recorded behavior: AWS serializes it as a JSON-encoded array of strings, so the TLS_1_0 protocol setting reports `["Enable","Disable"]`, which `json.loads` parses to a list of strings.
  Emulator: reports the string `Enable,Disable` for the same setting, which is not JSON at all.
  Why: a client that JSON-decodes AllowedValues to enumerate the legal values of a setting before calling UpdateSettings raises a parse error (or, worse, treats the whole string as one allowed value), so the settings surface is unusable programmatically.
  Fix: the directory-settings catalogue's allowed-values representation in state.rs (the `SETTING_VALUES_ENABLE_DISABLE` constant and `DirectorySettingRecord::to_setting_entry`), and UpdateSettings' validation must accept the values that encoding admits.
  Traces: ds.directory-settings-reject-unknown-names-and-values-outside-allowed-values.

Cause 5 of 7: the IAM AccessDeniedException message carries this service's request-id suffix, which AWS's authorization refusal does not.
  AWS: Smithy: RemoveRegion models `AccessDeniedException` (HTTP 400). Recorded behavior: the message is EXACTLY IAM's sentence and nothing more — "User: <caller session ARN> is not authorized to perform: ds:<OperationName> on resource: <directory ARN> with an explicit deny in an identity-based policy". The principal ARN, the action, the resource ARN and the refusing-rule clause are values taken from the request and the stored policies; the request id travels in ResponseMetadata.RequestId only and is NOT appended to the message.
  Emulator: appends its own " : RequestId: <id>" suffix to the IAM sentence, so the message ends "...with an explicit deny in an identity-based policy : RequestId: 00000000-0000-4000-8000-000000000001". (The suffix IS correct for this service's own refusals — the other checks in this suite expect it — so the defect is that the authorization gate's AccessDeniedException is built through the same suffixing helper.)
  Why: a client that compares the denial message against IAM's documented sentence (the standard way to tell an explicit deny from an implicit one) never matches, and the per-call request id leaks into a message clients log and diff.
  Fix: the service authorization gate (src/service/authorization.rs, `not_authorized`) must build its AccessDeniedException WITHOUT the request-id suffix the service's shared `refusal` helper appends.
  Traces: ds.remove-region-access-denied.

Cause 6 of 7: `DirectoryDescription$AccessUrl` is reported with a URL scheme instead of a host.
  AWS: recorded behavior. The model's prose example reads "http://<alias>.awsapps.com", but the recording shows AWS reporting a BARE HOST in this member — `<alias>.awsapps.com`, i.e. matching `[A-Za-z0-9.-]+(:\d+)?` with no scheme and no path — where `<alias>` is the directory's alias, or its directory id when no alias exists.
  Emulator: reports `http://<alias>.awsapps.com` (observed `http://d-003ba5fe5a.awsapps.com`; the id is a per-run value, the `http://` prefix is the defect).
  Why: a client that builds a URL from AccessUrl (scheme + AccessUrl + path) produces `https://http://d-....awsapps.com/...`, and anything validating the member as a hostname rejects it.
  Fix: `Directory::access_url` in state.rs — the single place every operation renders a stored directory's AccessUrl — must emit the host without the scheme.
  Traces: ds.simple-ad-small-lifecycle-to-active-then-deleted.

Cause 7 of 7: the data-access disable / re-enable lifecycle never completes within the check's budget, so no assertion in it is ever reached.
  AWS: Smithy: EnableDirectoryDataAccess / DisableDirectoryDataAccess are asynchronous and `DescribeDirectoryDataAccess$DataAccessStatus` is one of Disabled | Disabling | Enabled | Enabling | Failed; AWS settles each transition on its own and a caller polls it. Recorded behavior: each status reaches its terminal value and ds-data objects (a user, a group, the membership edge and their SIDs) survive a disable/re-enable cycle unchanged.
  Emulator: the check dies with "Check timed out after 60.0s" having produced no response divergence — it never reaches a single assertion about the refusals or the surviving objects.
  Hypothesis: the lifecycle walks four clock-owed transitions (directory create, enable, disable, re-enable) and each is still reported in its transient value on the probe the check makes immediately after the starting call, because the transition only falls due TRANSITION_SPAN (500 ms) later; the check therefore burns one full poll interval per transition (20 s + 10 s + 15 s + 10 s) and exhausts the 60 s budget before the body runs. The alternative is that a ds-data object does not survive the cycle and the final DescribeUser poll never answers.
  Why: a client cannot observe the DATA_DISABLED refusal path, nor that disabling the data plane preserves directory contents — the whole disable/re-enable contract is unverifiable against this emulator.
  Fix: the data-access transition settling for this service — `advance_clock`'s data-access branch plus the enable/disable handlers and the span they schedule — so a status read after its starting call reflects the clock a caller can poll, and the ds-data store's survival of the cycle.
  Traces: ds.disable-directory-data-access-recloses-the-door-without-losing-contents.

## Evidence
All quotes are from the behavioral suite replay against the emulator (76/178 passed; ds 73/155).

Cause 1 — ds.add-region-duplicate-rejected:
  expected 'The region you specified is the same region where the AWS Managed Microsoft AD directory was created. Specify a different region and try again. : RequestId: 00000000-0000-4000-8000-000000000001'
  got      'Region us-west-2 is the primary Region of directory d-003ba5fe5a; specify a different Region and try again : RequestId: 00000000-0000-4000-8000-000000000001'
  (code UnsupportedOperationException matched; only the message diverges.)

Cause 2 — ds.connect-directory-invalid-vpc-settings-rejected:
  expected 'Invalid subnet ID(s). They must correspond to two subnets in different Availability Zones. : RequestId: <id>'
  got      'Invalid subnet ID(s). The two subnets must be in different Availability Zones. : RequestId: 00000000-0000-4000-8000-000000000001'
  Source of the emulator text: services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/state.rs lines 234-235 (`INVALID_SUBNET_IDS`).

Cause 3 — ds.describe-directories-absent-id-rejected:
  expected 'Invalid directory ID(s): d-2ee16dd93c,d-52de831cee. : RequestId: 00000000-0000-4000-8000-000000000002'
  got      'Invalid directory ID(s): d-2ee16dd93c, d-52de831cee. : RequestId: 00000000-0000-4000-8000-000000000002'
  Source: state.rs lines 3608-3613, `invalid_directory_ids` uses `directory_ids.join(", ")`.

Cause 4 — ds.directory-settings-reject-unknown-names-and-values-outside-allowed-values:
  AssertionError: expected AllowedValues to parse as a JSON array of strings, got 'Enable,Disable' for setting 'TLS_1_0'
  Source: state.rs line 1598 (`const SETTING_VALUES_ENABLE_DISABLE: &str = "Enable,Disable";`), surfaced through `DirectorySettingRecord::to_setting_entry` (state.rs ~1705-1713).

Cause 5 — ds.remove-region-access-denied:
  expected 'User: arn:aws:sts::000000000000:assumed-role/<role>/<session> is not authorized to perform: ds:RemoveRegion on resource: arn:aws:ds:us-west-2:000000000000:directory/d-9074657a01 with an explicit deny in an identity-based policy'
  got      the same sentence with ' : RequestId: 00000000-0000-4000-8000-000000000001' appended.
  Source: src/service/authorization.rs builds the denial through this service's suffixing `refusal` helper (state.rs ~3548-3556 documents the suffix contract).

Cause 6 — ds.simple-ad-small-lifecycle-to-active-then-deleted:
  AssertionError: AccessUrl is not a well-formed host: 'http://d-003ba5fe5a.awsapps.com' (the check asserts re.fullmatch(r"[A-Za-z0-9.-]+(:\d+)?", access_url)).
  Source: state.rs lines 1874-1878, `Directory::access_url` formats "http://{alias}.awsapps.com"; reported at state.rs line 2240.

Cause 7 — ds.disable-directory-data-access-recloses-the-door-without-losing-contents:
  Error: Check timed out after 60.0s — the body never reached an assertion. Its sequential polls are create (interval 20 s), enable (10 s), disable (15 s), re-enable (10 s), then a DescribeUser read-back (5 s). The emulator schedules each transition TRANSITION_SPAN = 500 ms after the starting call (state.rs line 85; advance_clock's data-access branch at state.rs ~2955-2965), so the probe issued immediately after each call still observes Requested/Enabling/Disabling and costs a full interval.

## Steps to Reproduce
1. git_clone the repo and run the behavioral suite (run_behavioral_suite); the emulator boots and ds answers (73/155 checks pass), so these are fidelity divergences, not a bootstrap gap.
2. Re-replay exactly the checks in trace_ids:
   - ds.add-region-duplicate-rejected — CreateMicrosoftAD, wait for Active, AddRegion with the directory's OWN home Region and its own VPC/subnets; compare Error.Message to AWS's fixed sentence.
   - ds.connect-directory-invalid-vpc-settings-rejected — create a VPC with two subnets in distinct AZs, then ConnectDirectory naming that VPC and only ONE of the subnets; compare the InvalidParameterException message.
   - ds.describe-directories-absent-id-rejected — DescribeDirectories(DirectoryIds=[two ids matching ^d-[0-9a-f]{10}$ that name nothing]); compare the EntityDoesNotExistException message's separator.
   - ds.directory-settings-reject-unknown-names-and-values-outside-allowed-values — DescribeSettings on an Active Managed Microsoft AD and json.loads each SettingEntry.AllowedValues.
   - ds.remove-region-access-denied — assume a role whose identity policy explicitly denies ds:RemoveRegion, call RemoveRegion, and compare Error.Message against IAM's sentence (no request-id suffix).
   - ds.simple-ad-small-lifecycle-to-active-then-deleted — create a managed directory, wait for Active, and assert DescribeDirectories' AccessUrl is a bare host.
   - ds.disable-directory-data-access-recloses-the-door-without-losing-contents — Enable data access, create a ds-data user/group/membership, DisableDirectoryDataAccess, assert the DATA_DISABLED refusals, re-enable and assert the same SIDs and membership come back; today the check exhausts its 60 s budget in the transition polls.
3. Each must match AWS's recorded status, error code, message template and member encoding.
