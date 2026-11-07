Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event
Issue-Key: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event|_await_event#35|TimeoutError|raise TimeoutError(f"an event for {reason}: the run budget ended first; not graded")
Issue-Commit: d1b341c944550181b7ac630538402367c666ba11

# CodeArtifact emits no 'CodeArtifact Package Version State Change' event: an npm publish that now succeeds and is visible to DescribePackageVersion delivers nothing to an EventBridge rule (1 cause)

> _Note: this issue was identified against the code base at commit `d1b341c944550181b7ac630538402367c666ba11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishPackageVersion

## Description
Cause 1 of 1: a successful npm publish into a CodeArtifact repository produces no EventBridge event, so a rule matching source 'aws.codeartifact' / detail-type 'CodeArtifact Package Version State Change' with an SQS target receives nothing.

AWS: event emission is not described by the Smithy model (recorded behavior only; the control-plane shapes used around it are Smithy GetAuthorizationToken POST /v1/authorization-token, GetRepositoryEndpoint GET /v1/repository/endpoint, DescribePackageVersion GET /v1/package/version). Per https://docs.aws.amazon.com/eventbridge/latest/ref/events-ref-codeartifact.html, each package-version state change emits ONE event to the default bus in the domain owner's account: source 'aws.codeartifact', detail-type 'CodeArtifact Package Version State Change', resources naming the package ARN of the form arn:aws:codeartifact:<region>:<account>:package/<domain>/<repository>/<format>/<namespace>/<package> (an un-scoped npm package has an empty namespace, hence the doubled slash), and a detail carrying domainName, repositoryName, packageName, packageVersion, packageVersionRevision equal to the revision DescribePackageVersion reports for that version, packageVersionState 'Published', operationType 'Created' for a version that did not previously exist, changes {assetsAdded: 1, assetsRemoved: 0, assetsUpdated: 0, statusChanged: true, metadataUpdated: true} for a single-tarball npm publish, a positive integer sequenceNumber, and an eventDeduplicationId. Exactly one event is delivered when the domain owner and the repository administrator are the same account. An upstream fetch that retains a copy in a downstream repository emits its own event naming that downstream repository. Delivery is near-real-time, not synchronous.

Emulator: the npm publish now succeeds — PUT <npm endpoint>/<package> returns HTTP 200 and DescribePackageVersion immediately reports a revision for <package>@1.0.0 — but no event ever arrives at the SQS target of an ENABLED rule whose pattern is {source: ['aws.codeartifact'], detail-type: ['CodeArtifact Package Version State Change'], detail: {domainName: [<domain>]}}. The check's bounded wait on the publish event never completes and the run budget ends. All EventBridge/SQS plumbing is verified live in the same run (DescribeRule reports ENABLED, ListTargetsByRule returns exactly the queue ARN), so the absence is CodeArtifact's event emission, not the bus or the target. This is a new failure point for this check: open issue 20261025T024028Z-codeartifact-events-sqs-codeartifact-package-manager-npm-data-plane-is-unrouted.md carries its earlier signature, the publish itself being refused HTTP 404 {"__type":"UnknownService"}; the publish now returns 200 and the check advances past it to the event wait, which is the divergence reported here.

Why: every event-driven CodeArtifact integration — scanning or signing a newly published version, mirroring, approval gates, notifying on upstream retention — is wired to this event. Against the emulator those rules never fire, so a pipeline tested on it appears correctly configured while silently doing nothing in production; and because the publish returns 200, a client has no signal that the state change was not announced. The retention leg (a downstream repository's own event) is equally unobservable.

Fix: the CodeArtifact publish/retention path (the npm data-plane publish handler and the upstream-retention path that materialises a downstream copy) must, after committing the package version to state, publish one 'CodeArtifact Package Version State Change' event with source 'aws.codeartifact' to the default event bus so the EventBridge rule matcher can route it to its targets. The event's detail must be built from the state the publish actually created — packageVersionRevision must be the same revision DescribePackageVersion returns, operationType/packageVersionState/changes must reflect whether the version was created or updated and how many assets changed, sequenceNumber must be a positive, monotonically usable integer, and the resources entry must be the package ARN including the empty-namespace segment for un-scoped npm packages. Exactly one event per trigger in a single-account domain.

Traces: codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event.

## Evidence
Replay of codeartifact__events__sqs.publish-and-upstream-retention-each-emit-a-package-version-state-change-event (error):

  TimeoutError: an event for the publish of <package>@1.0.0 into <upstream-repository>: the run budget ended first; not graded
  (raised from _await_event, called immediately after the publish and the DescribePackageVersion revision read)

Everything before the event wait answered successfully in the same run:
  - create_domain; create_repository(UPSTREAM); create_repository(DOWNSTREAM, upstreams=[UPSTREAM])
  - events.put_rule with pattern {"source":["aws.codeartifact"],"detail-type":["CodeArtifact Package Version State Change"],"detail":{"domainName":[<domain>]}}; put_targets with the SQS queue; describe_rule -> State 'ENABLED'; list_targets_by_rule -> exactly [queue ARN]
  - get_authorization_token(durationSeconds=43200) -> 200; get_repository_endpoint(format='npm') for both repositories -> 200
  - npm PUT <up_endpoint>/<package> with the token as bearer and a publish envelope -> HTTP 200 (the assertion `status == 200` passes)
  - describe_package_version(..., packageVersion='1.0.0') -> returns a revision

expected (AWS): within the delivery budget, one message on the queue whose body is an EventBridge envelope with source 'aws.codeartifact', detail-type 'CodeArtifact Package Version State Change', resources containing arn:aws:codeartifact:<region>:<account>:package/<domain>/<upstream>/npm//<package>, and detail {domainName: <domain>, repositoryName: <upstream>, packageName: <package>, packageVersion: '1.0.0', packageVersionRevision: <the revision DescribePackageVersion returned>, packageVersionState: 'Published', operationType: 'Created', changes: {assetsAdded: 1, assetsRemoved: 0, assetsUpdated: 0, statusChanged: true, metadataUpdated: true}, sequenceNumber: >=1}; then no duplicate within the dedup window; then an analogous event for the downstream retention naming the downstream repository.
got (emulator): zero matching messages ever arrive, so the wait exhausts the budget. The envelope, revision-match, dedup and retention legs are all unverified because no event exists at all. (Domain/repository/package names and the account id in the run are per-run values filling the templates above, not part of the requirement.)

## Steps to Reproduce
1. codeartifact.create_domain(domain=D); create_repository(domain=D, repository=U); create_repository(domain=D, repository=DN, upstreams=[{'repositoryName': U}]).
2. events.put_rule(Name=RULE, EventPattern={"source":["aws.codeartifact"],"detail-type":["CodeArtifact Package Version State Change"],"detail":{"domainName":[D]}}); create an SQS queue with a policy allowing the rule ARN; events.put_targets(Rule=RULE, Targets=[{Id:'t1', Arn: <queue arn>}]). Confirm describe_rule -> ENABLED and list_targets_by_rule -> [queue arn].
3. token = codeartifact.get_authorization_token(domain=D, durationSeconds=43200)['authorizationToken']; ep = codeartifact.get_repository_endpoint(domain=D, repository=U, format='npm')['repositoryEndpoint'].
4. HTTP PUT ep + '<package>' with 'Authorization: Bearer <token>' and an npm publish envelope (package.json metadata + base64 tarball) -> HTTP 200; codeartifact.describe_package_version(domain=D, repository=U, format='npm', package=<package>, packageVersion='1.0.0') -> returns a revision.
5. sqs.receive_message on the queue with long polling, for minutes -> expect exactly one 'CodeArtifact Package Version State Change' event whose detail.packageVersionRevision equals the revision from step 4; observe that no message ever arrives.
6. Fetch <package>@1.0.0 through DN's npm endpoint so the version is retained downstream -> expect a second event naming repositoryName = DN; observe none.
