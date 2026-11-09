Issue-Source: trace-sweep-divergence
Issue-Trace: directory-service-directory-scoped-reads-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: directory-service-directory-scoped-reads-not-found ds-madl-traj-enable-ldaps-request-mistakes
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds|DescribeCAEnrollmentPolicy|4xx->4xx|$.message
Issue-Key: ds|DescribeDirectoryDataAccess|4xx->4xx|$.message
Issue-Key: ds|DescribeSettings|4xx->4xx|$.message
Issue-Key: ds|DescribeLDAPSSettings|4xx->4xx|$.message
Issue-Key: ds|ListIpRoutes|4xx->4xx|$.message
Issue-Key: ds|ListCertificates|4xx->4xx|$.message
Issue-Key: ds|CreateLogSubscription|4xx->4xx|$.message
Issue-Key: ds|ListADAssessments|4xx->4xx|$.message
Issue-Key: ds|EnableLDAPS|4xx->4xx|$.message
Issue-Commit: 8339e0b79c5a3f0b6ab10000bb256c51d9f94d77

# ValidationException message omits regex anchors for directoryId pattern

> _Note: this issue was identified against the code base at commit `8339e0b79c5a3f0b6ab10000bb256c51d9f94d77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCAEnrollmentPolicy

## Description
This bundle has 9 causes, all the SAME emulator defect observed on 9 different operations: when a request's `directoryId` fails its modeled @pattern constraint, the emulator returns the correct status (400) and the correct error type (ValidationException), but the message renders the regular expression with its anchors STRIPPED — it prints `d-[0-9a-f]{10}` where AWS prints `^d-[0-9a-f]{10}$`.

Message template for this error class (AWS wording): `1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`. Fixed text is everything except the three variable parts: `<value>` (the rejected value from the request), `<memberPath>` (the input member's path, e.g. `directoryId`) and `<pattern>` — and `<pattern>` is the Smithy @pattern trait's string VERBATIM, anchors included. The emulator reproduces the fixed text and the first two variables correctly and corrupts only `<pattern>`. Values such as `not-a-directory-id` or `d-bad` below are the recordings' request values filling the template, not part of it.

AWS (all 9 causes): Smithy model services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json — shape com.amazonaws.directoryservice#DirectoryId has pattern `^d-[0-9a-f]{10}$`; a value violating it is refused 400 ValidationException with the template above carrying `^d-[0-9a-f]{10}$`.

Emulator (all 9 causes): 400 ValidationException with the identical sentence except the pattern reads `d-[0-9a-f]{10}` — leading `^` and trailing `$` dropped.

Why (all 9 causes): the message is a requirement of the error class. A client, test harness or SDK that asserts on or re-parses the constraint message sees a different regular expression from the one AWS publishes — an UNANCHORED one, which additionally means anything a reader derives from it (a client-side pre-validator regenerated from the message, a doc/diagnostic surface) will accept substring matches AWS rejects. Any assertion on AWS's exact sentence fails.

Fix (all 9 causes): the shared request-validation layer that formats a @pattern failure into the ValidationException message for ds inputs. It must emit the @pattern trait value exactly as the model states it, including `^` and `$`; today it normalizes/unwraps the anchors (the same formatter serves every operation below, so one fix closes all 9 causes — but each must be re-replayed).

Cause 1 of 9: DescribeCAEnrollmentPolicy — invalid `directoryId`. Fixture directory-service-directory-scoped-reads-not-found, step task#1.
Cause 2 of 9: DescribeDirectoryDataAccess — invalid `directoryId`. Fixture directory-service-directory-scoped-reads-not-found, step task#2.
Cause 3 of 9: DescribeSettings — invalid `directoryId`. Fixture directory-service-directory-scoped-reads-not-found, step task#3.
Cause 4 of 9: DescribeLDAPSSettings — invalid `directoryId`. Fixture directory-service-directory-scoped-reads-not-found, step task#4.
Cause 5 of 9: ListIpRoutes — invalid `directoryId`. Fixture directory-service-directory-scoped-reads-not-found, step task#5.
Cause 6 of 9: ListCertificates — invalid `directoryId`. Fixture directory-service-directory-scoped-reads-not-found, step task#6.
Cause 7 of 9: CreateLogSubscription — invalid `directoryId`. Fixture directory-service-directory-scoped-reads-not-found, step task#7.
Cause 8 of 9: ListADAssessments — invalid `directoryId`. Fixture directory-service-directory-scoped-reads-not-found, step task#8.
Cause 9 of 9: EnableLDAPS — invalid `directoryId`. Fixture ds-madl-traj-enable-ldaps-request-mistakes, step task#4.

Not covered by any open issue: the open ds issues 20261108T155909Z-ds-2-net-new-causes-describedirectories-omits-vpcsettings.md, 20261108T222906Z-ds-3-net-new-causes-deletedirectory-removes-the.md, 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md and 20261103T100504Z-ds-directory-service-certificate-and-directory-lookups-unvalidated-shortname.md concern other ds behaviours (missing fields, lifecycle stages, skipped validation, shortname/RegisterCertificate wording); none carries the replay key of any step listed here.

## Evidence
Replay of trace set simparity-wire-aws-v1 (service ds), verdict mismatch, expected_status 400 / actual_status 400, diverging key $.message, for each of the 9 clusters.

Representative — fixture directory-service-directory-scoped-reads-not-found step task#1, DescribeCAEnrollmentPolicy:
  expected $.message = "1 validation error detected: Value 'not-a-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$"
  actual   $.message = "1 validation error detected: Value 'not-a-directory-id' at 'directoryId' failed to satisfy constraint: Member must satisfy regular expression pattern: d-[0-9a-f]{10}"

Identical expected-vs-actual delta (pattern printed without `^`/`$`) on:
  directory-service-directory-scoped-reads-not-found#task#2 DescribeDirectoryDataAccess
  directory-service-directory-scoped-reads-not-found#task#3 DescribeSettings
  directory-service-directory-scoped-reads-not-found#task#4 DescribeLDAPSSettings
  directory-service-directory-scoped-reads-not-found#task#5 ListIpRoutes
  directory-service-directory-scoped-reads-not-found#task#6 ListCertificates
  directory-service-directory-scoped-reads-not-found#task#7 CreateLogSubscription
  directory-service-directory-scoped-reads-not-found#task#8 ListADAssessments
  ds-madl-traj-enable-ldaps-request-mistakes#task#4 EnableLDAPS (request value 'd-bad')

Model evidence: smithy_query shape DirectoryId -> {"pattern": "^d-[0-9a-f]{10}$"}. HTTP status (400) and error type (ValidationException) match in every cluster; only the pattern text inside the message diverges.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Call any of the 9 operations above with a `directoryId` that violates `^d-[0-9a-f]{10}$` (e.g. ds DescribeSettings with DirectoryId='not-a-directory-id', or EnableLDAPS with DirectoryId='d-bad').
3. Observe 400 ValidationException and read $.message: the trailing regular expression is printed as `d-[0-9a-f]{10}`.
4. AWS prints `^d-[0-9a-f]{10}$` — the @pattern trait value verbatim. Re-replay fixture directory-service-directory-scoped-reads-not-found steps task#1..task#8 and ds-madl-traj-enable-ldaps-request-mistakes step task#4 to confirm a fix.
