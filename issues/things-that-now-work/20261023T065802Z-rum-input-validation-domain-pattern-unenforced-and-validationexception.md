Issue-Source: trace-sweep-divergence
Issue-Trace: rum-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-error-paths rum-error-invalid-excluded-page-pattern rum-batch-create-validation-exception
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|CreateAppMonitor|4xx->2xx|$.message
Issue-Key: rum|CreateAppMonitor|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: rum|CreateAppMonitor|4xx->2xx|status
Issue-Key: rum|BatchCreateRumMetricDefinitions|4xx->4xx|$.message
Issue-Related: 20261023T065802Z-rum-not-found-and-conflict-errors-use-wrong.md
Issue-Commit: c036e6d6272f3d2084b4ffba5fd23a45ba05e284

# RUM input validation: Domain pattern unenforced and ValidationException messages built wrongly

> _Note: this issue was identified against the code base at commit `c036e6d6272f3d2084b4ffba5fd23a45ba05e284`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppMonitor

## Description
Three causes in the rum emulator's request validation, all in service rum; a fixer must close ALL THREE before submitting. Cause 1 is a missing rejection (the emulator creates a resource AWS refuses); causes 2 and 3 return the right status and error code but build the constraint-violation message by a different rule than AWS's. Related open issue '20261022T205048Z-rum-error-representation-and-input-validation-diverge-conflict.md' reports cause 1 from the BEHAVIORAL suite but carries no wire replay key, so the wire cluster is filed here; close them together.

=== Cause 1 of 3: CreateAppMonitor accepts a Domain that violates the modeled pattern — HTTP 200 instead of ValidationException/400 ===
AWS: Smithy: CreateAppMonitor.Domain targets AppMonitorDomain with @length 1..253 and @pattern ^(localhost)$|^((25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$|(?=^[a-zA-Z0-9\.\*-]{4,253}$)(?!.*\.-)(?!.*-\.)(?!.*\.\.)(?!.*[^\.]{64,})^(\*\.)?(?![-\.\*])[^\*]{1,}\.(\*|(?!.*--)(?=.*[a-zA-Z])[^\*]{1,}[^\*-])$ ; CreateAppMonitor models ValidationException (httpError 400). A value such as '-not-a-valid-domain-' fails that pattern, so AWS refuses with 400, header x-amzn-errortype: ValidationException, and creates nothing. The recorded message follows AWS's standard constraint-violation rule, built from the request: "1 validation error detected: Value '<value>' at 'domain' failed to satisfy constraint: Member must satisfy regular expression pattern: <the modeled pattern>".
Emulator: answers HTTP 200, creates the app monitor, and emits no message and no x-amzn-errortype header.
Why: an overly permissive data path. A client relying on AWS rejecting a malformed domain silently succeeds and is left with a resource real RUM would never create (and later reads return a monitor instead of ResourceNotFoundException), so negative tests pass for the wrong reason and misconfiguration reaches production undetected.
Fix: input validation in the CreateAppMonitor handler — enforce the modeled @pattern/@length on Domain (and on the shape-sharing UpdateAppMonitor.Domain and the DomainList members) before any state mutation, raising ValidationException/400.
Traces: rum-error-paths#task#3.

=== Cause 2 of 3: the ValidationException message for a list member reports a different path and omits the list's other constraints ===
AWS: Smithy: CreateAppMonitor.AppMonitorConfiguration.ExcludedPages is a list of Pages whose member carries @length 1..1260 and the URL @pattern .*https?:\/\/(www\.)?[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6}\b([-a-zA-Z0-9()@:%_\+.~#?&*//=]*).* ; ValidationException is modeled (400). Recorded behavior: when one entry is not a URL, RUM reports the violation against the LIST member — "Value '[<the whole list>]' at 'appMonitorConfiguration.excludedPages' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1260, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: <pattern>]" — i.e. the rendered list value, the list's own path, and ALL of the member's modeled constraints enumerated in a bracketed set.
Emulator: returns 400 ValidationException (status and code correct) with "Value '/admin' at 'appMonitorConfiguration.excludedPages.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — the offending element rather than the list, an indexed '.1.member' path AWS never emits, and only the one constraint that failed.
Why: the message is produced by a rule the model and the recording together pin down (which path a collection violation is reported at, and that the full constraint set is listed), not by environment-specific text. Clients and operators that parse the member path out of a ValidationException to map it back to an input field get a path ('...excludedPages.1.member') that does not exist in the API, so field-level error mapping in SDK wrappers and forms breaks.
Fix: the shared constraint-validation/ValidationException message builder in the rum crate — report collection-member violations at the collection's path with the rendered collection value and the member's full modeled constraint list.
Traces: rum-error-invalid-excluded-page-pattern#task#1.

=== Cause 3 of 3: the enum ValidationException message lists the allowed values in the wrong order ===
AWS: Smithy: BatchCreateRumMetricDefinitions.Destination is @required and targets MetricDestination, an enum declared with values [CloudWatch, Evidently]; ValidationException is modeled (400). Recorded behavior: for an out-of-enum destination RUM answers 400 with "1 validation error detected: Value '<value>' at 'destination' failed to satisfy constraint: Member must satisfy enum value set: [Evidently, CloudWatch]" — the enum set is rendered in that fixed order.
Emulator: returns 400 ValidationException (status and code correct) with the same sentence but the set rendered as '[CloudWatch, Evidently]'.
Why: the message is wholly derived from the request and the model's enum, so its rendering is part of the contract; a harness or a user-facing form that shows/compares the allowed-value list sees a different string than AWS produces. This is the lowest-impact cause in this bundle (ordering of a rendered set) but shares the validation-message builder with cause 2.
Fix: the enum-constraint branch of the same ValidationException message builder — render the enum value set in the order RUM reports.
Traces: rum-batch-create-validation-exception#task#2.

## Evidence
Cause 1 — service=rum|CreateAppMonitor|status=400->200|keys=$.message,$headers.x-amzn-errortype,status (rum-error-paths#task#3):
  status: expected 400, got 200
  $.message: missing (expected "1 validation error detected: Value '-not-a-valid-domain-' at 'domain' failed to satisfy constraint: Member must satisfy regular expression pattern: (localhost)$|^((25[0-5]|...)$")
  $headers.x-amzn-errortype: missing (expected 'ValidationException', got None)

Cause 2 — service=rum|CreateAppMonitor|status=400->400|keys=$.message (rum-error-invalid-excluded-page-pattern#task#1):
  expected "1 validation error detected: Value '[/admin]' at 'appMonitorConfiguration.excludedPages' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1260, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: .*https?:\\/\\/(www\\.)?...]"
  got      "1 validation error detected: Value '/admin' at 'appMonitorConfiguration.excludedPages.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: https?:\\/\\/(www\\.)?..."

Cause 3 — service=rum|BatchCreateRumMetricDefinitions|status=400->400|keys=$.message (rum-batch-create-validation-exception#task#2):
  expected "...Value 'NotARealDestination' at 'destination' failed to satisfy constraint: Member must satisfy enum value set: [Evidently, CloudWatch]"
  got      "...Value 'NotARealDestination' at 'destination' failed to satisfy constraint: Member must satisfy enum value set: [CloudWatch, Evidently]"

## Steps to Reproduce
Boot the emulator and point a botocore rum client at it.
1) Cause 1: rum.create_app_monitor(Name='badDomain', Domain='-not-a-valid-domain-') -> HTTP 200 with an Id (and rum.get_app_monitor(Name='badDomain') then returns a monitor); AWS refuses with ValidationException/400, x-amzn-errortype: ValidationException, and creates nothing.
2) Cause 2: rum.create_app_monitor(Name='badPages', Domain='example.com', AppMonitorConfiguration={'ExcludedPages': ['/admin']}) -> 400 whose message reports path 'appMonitorConfiguration.excludedPages.1.member' and only the pattern constraint; AWS reports path 'appMonitorConfiguration.excludedPages' with value '[/admin]' and the full bracketed constraint set.
3) Cause 3: create a monitor, then rum.batch_create_rum_metric_definitions(AppMonitorName=<name>, Destination='NotARealDestination', MetricDefinitions=[...]) -> 400 whose message renders the enum set as '[CloudWatch, Evidently]'; AWS renders '[Evidently, CloudWatch]'.
Or replay fixtures rum-error-paths (task#3), rum-error-invalid-excluded-page-pattern (task#1), rum-batch-create-validation-exception (task#2).
