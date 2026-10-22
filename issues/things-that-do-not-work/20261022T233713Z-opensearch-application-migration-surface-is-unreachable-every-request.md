Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.list-migrations-most-recent-first-ordering opensearch.migration-progress-visibility-poll-window opensearch.representation-migration-timestamp-precision-preserved opensearch.start-migration-invalid-workspace-type-rejected
Issue-Key: opensearch|CreateApplication|AmbiguousService
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# OpenSearch application/migration surface is unreachable: every request signed for the 'es' endpoint prefix is refused with AmbiguousService

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Cause 1 of 1: two emulated services both claim the 'es' endpoint prefix, so the router refuses every OpenSearch request before any handler runs. Four behavioral checks of the OpenSearch application/migration surface fail identically on their first OpenSearch call (CreateApplication).

AWS: Smithy (com.amazonaws.opensearch#CreateApplication) binds POST /2021-01-01/opensearch/application with success code 200 returning {id (pattern ^[a-z0-9]{3,30}$), name, arn, dataSources, appConfigs, createdAt}; its modeled errors are AccessDeniedException (403), BaseException (400), ConflictException (409), DisabledOperationException (409), InternalException (500), ValidationException (400). From a client's point of view exactly one service sits behind the 'es' sigv4 name / es.<region>.amazonaws.com endpoint, so a well-formed CreateApplication is either served (200) or refused with one of those modeled codes.

Emulator: the call never reaches a handler. botocore reports 'An error occurred (AmbiguousService) when calling the CreateApplication operation (reached max retries: 3): 2 emulated services all claim requests for \'es\': es, es; the request is rejected rather than served by an arbitrary one'. AmbiguousService is not an AWS error code, and because botocore retries it the call also burns the whole retry budget.

Why: no OpenSearch application can be created, so nothing downstream of it is reachable — StartMigration/GetMigration/ListMigrations ordering, migration progress visibility, migration timestamp representation and the invalid-workspace-type rejection are all unobservable. A client pointed at the emulator sees an endpoint the emulator announces and then refuses, with a code no AWS SDK models.

Fix: the endpoint/service routing layer (request router / service registry) must disambiguate the two registrations claiming 'es' — the modern OpenSearch service crate and the legacy Elasticsearch Service crate — so one owns the prefix and the request is dispatched to it. This does not belong in any operation handler. A related open issue, 20261022T194351Z-firehose-iam-opensearch-s3-opensearch-is-unreachable-the-router-answers-ambiguousservice-for.md, reports the same router behavior from a wire trace on CreateDomain; it does not carry these behavioral check ids, so this issue records them.

Traces: opensearch.list-migrations-most-recent-first-ordering, opensearch.migration-progress-visibility-poll-window, opensearch.representation-migration-timestamp-precision-preserved, opensearch.start-migration-invalid-workspace-type-rejected.

## Evidence
Expected: opensearch.create_application(name=..., dataSources=[{dataSourceArn: <aoss collection arn>}]) -> HTTP 200 with {id, name, arn, createdAt}, or a modeled 4xx (ValidationException / ConflictException / DisabledOperationException).
Actual: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateApplication operation (reached max retries: 3): 2 emulated services all claim requests for 'es': es, es; the request is rejected rather than served by an arbitrary one.
All four checks abort at that first CreateApplication call, so none of the migration assertions are ever evaluated.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. c = client('opensearch'); c.create_application(name='sim2real-app', dataSources=[{'dataSourceArn': '<aoss collection arn>'}]).
3. Observe the request is refused with AmbiguousService naming two services claiming 'es', after botocore exhausts its retries, instead of a 200 CreateApplicationResponse.
4. Re-replay the four check ids in trace_ids.
