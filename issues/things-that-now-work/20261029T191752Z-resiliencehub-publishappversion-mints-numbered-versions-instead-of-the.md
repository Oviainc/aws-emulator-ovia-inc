Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehub.published-app-version-freezes-resource-mapping resiliencehub.assessment-produces-policy-compliance-verdict resiliencehub.draft-app-version-assessment-rejected resiliencehub.publish-of-empty-app-template-rejected resiliencehub.app-without-mapped-resources-rejects-publish resiliencehub.policy-delete-blocked-while-app-associated resiliencehub.policy-deletion-blocked-while-attached-to-app
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub.published-app-version-freezes-resource-mapping|run#20|AssertionError|assert published["appVersion"] == "release", (
Issue-Key: resiliencehub.assessment-produces-policy-compliance-verdict|run#14|AssertionError|assert rh.publish_app_version(appArn=app_arn)["appVersion"] == "release", (
Issue-Key: resiliencehub.draft-app-version-assessment-rejected|run#14|AssertionError|assert published["appVersion"] == "release", (
Issue-Key: resiliencehub.publish-of-empty-app-template-rejected|run#10|AssertionError|assert err.get("Message") == "Invalid app template. App template must have at least 1 resource", (
Issue-Key: resiliencehub|PublishAppVersion|ConflictException
Issue-Key: resiliencehub.policy-delete-blocked-while-app-associated|run#15|AssertionError|assert err.get("Message") == in_use_message, (
Issue-Key: resiliencehub|DeleteResiliencyPolicy|ConflictException
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# resiliencehub: PublishAppVersion mints numbered versions instead of the 'release' alias, and both ConflictException refusal messages diverge from AWS wording

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishAppVersion

## Description
Three distinct root causes in the resiliencehub emulator, all surfaced by the behavioral suite (7 failing checks, 0/11 passed for this service). No open issue covers any of them (list_existing_issues for 'resiliencehub' returns nothing); the separately-filed ResolveAppVersionResources scaffold issue is unrelated to all three.

=== Cause 1 of 3: PublishAppVersion returns a numbered appVersion instead of the 'release' alias ===

AWS: PublishAppVersion snapshots the DRAFT and addresses the newest published snapshot by the fixed alias string `release`, while the monotonically increasing publish sequence number is carried separately in `identifier`. Smithy: PublishAppVersion output has both `appVersion` (EntityVersion, pattern ^\S{1,50}$) and `identifier` (LongOptional) — two separate members precisely because the version label and the sequence number are not the same value. Recorded behavior: the first publish of a non-empty draft answers `appVersion="release"`, `identifier=1`; ListAppVersions then reports exactly two entries, `draft` and `release`, with the `release` entry's `identifier` equal to 1 — the newest published version is addressed by the alias and is NOT additionally listed under its number. A second publish replaces what `release` resolves to and advances `identifier` to 2.

Emulator: the handler sets `app_version = identifier.to_string()` and inserts the snapshot under that numeric key, so the first publish answers `appVersion='1'` instead of `'release'`, and the version is stored and listed under the number rather than under the alias. Replay shows `AssertionError: first publish returned appVersion '1', expected 'release'`; the two assessment checks fail at the same call with `publishing the draft did not produce a 'release' version, so there is nothing to assess`.

Why: a client cannot address its published application at all. Every downstream call that takes an appVersion — DescribeAppVersionTemplate, ListAppVersionResources, ResolveAppVersionResources, StartAppAssessment — is documented and used with `appVersion="release"`, and against this emulator that string names no version, so all post-publish reads and every assessment of a published version fail. ListAppVersions also reports a version set the client does not expect ({draft, 1} rather than {draft, release}), breaking any logic that distinguishes the draft from the current release.

Fix: the PublishAppVersion handler and the app-version keying in this service's state — the published snapshot must be stored/addressed under the constant alias `release` (with the sequence number carried only in `identifier`, and a re-publish overwriting the alias's contents), not under `identifier.to_string()`.

Traces: resiliencehub.published-app-version-freezes-resource-mapping, resiliencehub.assessment-produces-policy-compliance-verdict, resiliencehub.draft-app-version-assessment-rejected.

=== Cause 2 of 3: PublishAppVersion's empty-draft ConflictException carries the wrong message ===

AWS: publishing an application whose draft maps no resources is refused with ConflictException (Smithy: PublishAppVersion errors include ConflictException, httpError 409) whose message is the fixed text `Invalid app template. App template must have at least 1 resource` — the message has NO variable parts; it names the template requirement rather than the app. Recorded behavior: HTTP 409, code ConflictException, that exact message, and the refusal creates no release and does not consume a version number.

Emulator: the status (409) and the code (ConflictException) are right, but `empty_draft_conflict()` emits `the draft version of application <appArn> is empty; add to it before publishing` — different fixed text, and it interpolates the appArn, which AWS's message does not contain at all.

Why: the message is the only thing distinguishing "your template needs a resource" from the many other conflicts this operation models (a concurrent AppVersionModel modification, for instance). A client or test that matches on AWS's wording reads this refusal as an unrelated conflict and either retries with backoff forever or reports the wrong cause to the user.

Fix: the empty-draft refusal message in the PublishAppVersion handler's error construction.

Traces: resiliencehub.publish-of-empty-app-template-rejected, resiliencehub.app-without-mapped-resources-rejects-publish.

=== Cause 3 of 3: DeleteResiliencyPolicy's in-use ConflictException carries the wrong message ===

AWS: deleting a resiliency policy an application still references is refused with ConflictException (Smithy: DeleteResiliencyPolicy errors include ConflictException, httpError 409). Recorded behavior: HTTP 409, code ConflictException, message of the form `Cannot delete policy <policyName> (<policyArn>). Policy currently associated with app(s) <appName> (<appArn>)`. Fixed text: `Cannot delete policy `, ` (`, `). Policy currently associated with app(s) `, ` (`, `)`. Variable parts taken from the request and from state: the policy's NAME and ARN, and the NAME and ARN of each associated application. Note the message carries the human-readable policy and app NAMES alongside the ARNs.

Emulator: status and code are right, but `still_referenced()` emits `resiliency policy <policyArn> is still used by application <appArn>; point that application at another policy or delete it first` — different fixed text, and it carries only ARNs, never the policy name or the app name.

Why: a client cannot tell the user which policy or which application blocks the delete without a second DescribeApp round-trip, and any consumer matching AWS's wording (or extracting the app name from it) misclassifies the refusal. One failing check asserts byte equality with AWS's message; the other asserts only that the app's NAME appears in the message, and even that weaker assertion fails, so the defect is in the message template and not merely in whitespace.

Fix: the in-use refusal message in the DeleteResiliencyPolicy handler's error construction (it must resolve the policy's and the referencing application's names from state, not just their ARNs).

Traces: resiliencehub.policy-delete-blocked-while-app-associated, resiliencehub.policy-deletion-blocked-while-attached-to-app.

## Evidence
Suite report: resiliencehub 0/11 (0%), 11 failed (7 mismatched, 4 errored — the 4 errors are the ALREADY FILED ResolveAppVersionResources scaffold cluster and are not part of this issue).

Cause 1 — expected vs actual:
  expected: PublishAppVersion -> appVersion="release", identifier=1; ListAppVersions -> {"draft", "release"} with release.identifier==1
  actual:   PublishAppVersion -> appVersion="1"
  resiliencehub.published-app-version-freezes-resource-mapping:
    AssertionError: first publish returned appVersion '1', expected 'release'
  resiliencehub.assessment-produces-policy-compliance-verdict:
    AssertionError: publishing the draft did not produce a 'release' version, so there is nothing to assess
  resiliencehub.draft-app-version-assessment-rejected:
    AssertionError: publishing the draft did not produce a 'release' version, so the positive control below would be untestable
  Source: services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/publish_app_version.rs:205-221 (`let identifier = store.next_published_app_version(...); let app_version = identifier.to_string();` then `store.insert_app_version(&input.app_arn, &app_version, published)` and `app_version: Some(app_version)` in the output).

Cause 2 — expected vs actual (both HTTP 409 / ConflictException):
  expected Message: "Invalid app template. App template must have at least 1 resource"
  actual   Message: "the draft version of application arn:aws:resiliencehub:us-west-2:000000000000:app/<app-id> is empty; add to it before publishing"
  resiliencehub.publish-of-empty-app-template-rejected:
    AssertionError: the rejection does not carry the empty-template message: 'the draft version of application ... is empty; add to it before publishing'
  resiliencehub.app-without-mapped-resources-rejects-publish:
    AssertionError: the refusal message '...' does not say the template needs a resource, so it is rejecting the publish for some other reason
  Source: services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/publish_app_version.rs:249-259 (`empty_draft_conflict`).

Cause 3 — expected vs actual (both HTTP 409 / ConflictException):
  expected Message: "Cannot delete policy <policyName> (<policyArn>). Policy currently associated with app(s) <appName> (<appArn>)"
  actual   Message: "resiliency policy <policyArn> is still used by application <appArn>; point that application at another policy or delete it first"
  resiliencehub.policy-delete-blocked-while-app-associated:
    AssertionError: the refusal returned message 'resiliency policy arn:...:resiliency-policy/<id> is still used by application arn:...:app/<id>; point that application at another policy or delete it first', expected 'Cannot delete policy <name> (<arn>). Policy currently associated with app(s) <app-name> (<app-arn>)'
  resiliencehub.policy-deletion-blocked-while-attached-to-app:
    AssertionError: the refusal message '...' does not name the app '<app-name>' that holds the policy, so it is not reporting the referential conflict
  Source: services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/delete_resiliency_policy.rs:239-249 (`still_referenced`) — it formats only `policy_arn` and `app_arn`.

(Identifiers, ARNs and generated names above are per-run values filling the message templates; only the fixed text and the identity of each variable slot are requirements.)

## Steps to Reproduce
Cause 1 and 2 (PublishAppVersion):
1. CreateResiliencyPolicy(policyName=<name>, tier="NonCritical", policy={Software/Hardware/AZ with rtoInSecs/rpoInSecs}) -> policyArn.
2. CreateApp(name=<name>, policyArn=<policyArn>) -> appArn. ListAppVersions(appArn) returns only {"draft"}.
3. PublishAppVersion(appArn) on the still-empty draft. Observe HTTP 409 ConflictException but with message "the draft version of application <appArn> is empty; add to it before publishing" instead of AWS's "Invalid app template. App template must have at least 1 resource" (Cause 2).
4. CreateAppVersionAppComponent(appArn, name=<component>, type="AWS::ResilienceHub::StorageAppComponent"), then CreateAppVersionResource(appArn, resourceName=<res>, logicalResourceId={identifier:<bucket>}, physicalResourceId=<bucket>, resourceType="AWS::S3::Bucket", appComponents=[<component>]).
5. PublishAppVersion(appArn) again. Observe appVersion=="1" instead of "release" (Cause 1); ListAppVersions(appArn) returns {"draft", "1"} instead of {"draft", "release"}, and any subsequent call with appVersion="release" (DescribeAppVersionTemplate / ListAppVersionResources / StartAppAssessment) cannot address the published version.

Cause 3 (DeleteResiliencyPolicy):
1. CreateResiliencyPolicy(policyName=<policyName>) -> policyArn.
2. CreateApp(name=<appName>, policyArn=<policyArn>) -> appArn.
3. DeleteResiliencyPolicy(policyArn=<policyArn>). Observe HTTP 409 ConflictException whose Message is "resiliency policy <policyArn> is still used by application <appArn>; point that application at another policy or delete it first" rather than "Cannot delete policy <policyName> (<policyArn>). Policy currently associated with app(s) <appName> (<appArn>)" — the emulator's message never contains <policyName> or <appName>.

Full replay: run_behavioral_suite() and re-run the seven check ids in trace_ids.
