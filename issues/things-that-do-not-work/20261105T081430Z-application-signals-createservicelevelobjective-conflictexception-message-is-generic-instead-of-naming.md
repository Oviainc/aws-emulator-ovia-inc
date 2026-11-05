Issue-Source: trace-sweep-divergence
Issue-Trace: application-signals-slo-error-paths
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-signals-slo-error-paths
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals|CreateServiceLevelObjective|4xx->4xx|$.Message
Issue-Commit: 56ff870e65d226f4952f88c474cf52081981448e

# CreateServiceLevelObjective ConflictException message is generic instead of naming the existing SLO ARN

> _Note: this issue was identified against the code base at commit `56ff870e65d226f4952f88c474cf52081981448e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServiceLevelObjective

## Description
This issue carries ONE cause.

Cause 1 of 1: CreateServiceLevelObjective answers a duplicate-name create with the right status and the wrong ConflictException message.

AWS: Smithy (services/service-com-amazonaws-applicationsignals-applicationsignals/smithy-model/com-amazonaws-applicationsignals-applicationsignals.json): CreateServiceLevelObjective POST /slo, success code 200, input Name @required (pattern ^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$), output Slo @required; modeled errors AccessDeniedException(403), ConflictException(409), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400). ConflictException is a client error with httpError 409 and a single @required member Message, documented "This operation attempted to create a resource that already exists." The model does not state the message text, so for the wording this is recorded behavior only: re-creating an SLO whose Name already exists is answered HTTP 409 with error code ConflictException and Message of the form

    Slo with arn '<arn-of-the-existing-slo>' already exists.

The fixed text of that template is "Slo with arn '" ... "' already exists." (including the single quotes around the ARN and the trailing period). The only variable part is the ARN of the SLO that already exists, which the emulator already holds in its own state for that Name: arn:aws:application-signals:<region>:<account-id>:slo/<the request's Name>. The recorded ARN value from one run is an example of that substitution, not part of the requirement.

Emulator: the status (409) and the error code match, but the Message is the generic, resource-agnostic string 'A resource with the same identifier already exists' — no mention of an SLO, no ARN of the conflicting resource, and no trailing period. It is evidently a shared conflict helper reused across resource types rather than the SLO-specific message.

Why: the Message is the only part of a ConflictException that identifies WHICH resource conflicted — the modeled shape carries no Arn/Name/ResourceId member, so a client has nothing else to go on. Tooling that reconciles SLOs (CDK/Terraform-style create-or-adopt flows, idempotent provisioners, log scrapers and operator runbooks) parses or matches on the AWS wording to extract the existing SLO's ARN and then adopt or update it instead of failing; against the emulator that extraction yields nothing, so the conflict cannot be attributed to a resource and the create surfaces as an opaque failure. Human diagnosis of a duplicate-name create is likewise degraded.

Fix: the error construction in the CreateServiceLevelObjective handler (the already-exists branch of the application-signals CreateServiceLevelObjective op, which currently raises the service's shared/default conflict error) — it must build the ConflictException Message from the SLO ARN it looked up when detecting the duplicate, rather than delegating to the generic same-identifier conflict text. Nothing else about the response needs to change: the 409 status and the ConflictException code are already correct.

Traces: application-signals-slo-error-paths#task#5.

Note: the open issue 20261029T215138Z-application-signals-success-responses-diverge-fabricated-and-omitted.md bundles seven OTHER application-signals clusters (all on success/non-refusal paths) and does not cover this ConflictException message cluster; the replay reports this cluster with an empty owned_by.

## Evidence
Wire replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 62 steps, 52 passed, 10 mismatched, score 83.9%. This cluster is verdict=mismatch with an empty stub_path and an empty owned_by (no issue owns its replay key).

application-signals-slo-error-paths#task#5 (CreateServiceLevelObjective), expected status 409 / actual status 409 — status and error code agree, body message diverges:

  $.Message: value_mismatch
    expected "Slo with arn 'arn:aws:application-signals:<region>:<account-id>:slo/<the-slo-name-from-the-request>' already exists."
    got      'A resource with the same identifier already exists'

In the expected string, "Slo with arn '" and "' already exists." are AWS's fixed text; the quoted ARN is the variable part, resolved from the request's Name plus the emulator's own region/account and the SLO record it already stores. (One recorded run rendered it as arn:aws:application-signals:us-west-2:000000000000:slo/sim2real-probe-application-signals-slo-errors-a7ce4fa4 — that literal is an example of the substitution, not a required value.)

Model citations: CreateServiceLevelObjective POST /slo, code 200, output Slo @required, errors include ConflictException(409); ConflictException = structure, errorKind client, httpError 409, member Message @required (String), doc "This operation attempted to create a resource that already exists."

## Steps to Reproduce
Boot the emulator and point an application-signals client at its data listener.

1. CreateServiceLevelObjective(Name=N, SliConfig=<any valid SLI config>, Goal=<any valid goal>) — succeeds 200 and returns Slo with Arn = arn:aws:application-signals:<region>:<account>:slo/N.
2. CreateServiceLevelObjective again with the SAME Name=N.

Observed: HTTP 409 ConflictException with $.Message = 'A resource with the same identifier already exists'.
Expected: HTTP 409 ConflictException with $.Message = "Slo with arn '<the Arn returned in step 1>' already exists."

Or replay application-signals-slo-error-paths#task#5.
