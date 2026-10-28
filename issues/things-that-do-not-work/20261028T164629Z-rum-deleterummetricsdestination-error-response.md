Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0b60124a277040298c4e92ee09b21d27ce53d94f

# A request that omits the required `destination` query parameter answers 501 NotImplemented instead of the modelled refusal, although the sibling route the crate DOES resolve answers 400

_Filed: 2026-10-28T16:46:29Z_
_Operation: DeleteRumMetricsDestination | Class: error-response_

> _Note: this issue was identified against the code base at commit `0b60124a277040298c4e92ee09b21d27ce53d94f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The framework's REST router rejects a candidate route when a `@required` `@httpQuery` member of that operation is not on the request. `crate::service` knows this and works around it for exactly TWO routes (`DELETE /tags/{ResourceArn}` and `DELETE /rummetrics/{AppMonitorName}/metrics`), which it resolves by method+path and serves through its own gate, so the operation's own refusal is produced instead of a routing failure.

But `Destination` is `@required` + `@httpQuery("destination")` on TWO further operations — `DeleteRumMetricsDestination` (`DELETE /rummetrics/{AppMonitorName}/metricsdestination`) and `BatchGetRumMetricDefinitions` (`GET /rummetrics/{AppMonitorName}/metrics`) — and neither route is in the workaround. A request to either that binds no `destination` value therefore matches no route, `override_operation` answers `None`, and the router renders the REST routing error: HTTP 501 `NotImplemented`, which this very file calls "a status and code the operation does not model". The modelled answer for an absent `@required` member is the service's common `ValidationException` (HTTP 400), which is exactly what the crate produces for the same omission on the route it does resolve: `BatchDeleteRumMetricDefinitionsWireInput` returns `missing_member(DESTINATION_MEMBER)` so a `DELETE /rummetrics/{name}/metrics` carrying neither query member is refused 400.

So two sibling routes that differ only in the sub-resource segment answer the same client mistake with 400 and 501 respectively, and `crate::service`'s stated reason for stopping at two routes is not true of this service's model.

## Evidence

The claim in the crate, `services/service-com-amazonaws-rum-rum/src/service.rs:176-179`:
```
/// It is one of the two such routes in this service: every operation other than it and
/// [`BATCH_DELETE_METRIC_DEFINITIONS_ROUTE`] either binds no `@required` member to the
/// query string at all, or binds only members a request always carries a value for, so no
/// other method+path pair can fail to resolve this way.
```
and the consequence it names, `services/service-com-amazonaws-rum-rum/src/service.rs:171-174`:
```
/// A call that binds no `tagKeys` value ... then matches NO route and is answered
/// "unknown operation" (HTTP 501 `NotImplemented`, a status and code the operation does
/// not model) instead of being served.
```
The model contradicts the claim — `smithy_query(query="operation", operation="DeleteRumMetricsDestination,BatchGetRumMetricDefinitions")` / OPERATIONS.md:131 and :107:
- `DeleteRumMetricsDestination`: "`Destination` (string, one of: CloudWatch | Evidently, @httpQuery(destination), required)"
- `BatchGetRumMetricDefinitions`: "`Destination` (string, one of: CloudWatch | Evidently, @httpQuery(destination), required)"

The framework rule, `vendor/smithy-schema-server/src/rest_xml.rs:368` (shared by `rest_json`, per :378-380): "a candidate route is **rejected** when any of its [`RequiredBinding`]s is absent from the request", implemented at `vendor/smithy-schema-server/src/rest_xml.rs:569`: `RequiredBinding::Query(name) => request_query.iter().any(|&(k, _)| k == *name),`.

The workaround that skips them, `services/service-com-amazonaws-rum-rum/src/service.rs:243-245` and `:575-581`:
```
fn is_query_unmatched_route(req: &aws_smithy_runtime_api::http::Request) -> bool {
    is_untag_resource_route(req) || is_batch_delete_metric_definitions_route(req)
}
...
    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        let gate = &self.announced_media_type as &dyn ErasedOperation;
        if modeled_operation(self, req).is_some() || is_query_unmatched_route(req) {
            return Some(gate);
        }
        None
    }
```
The 400 the resolved sibling produces for the SAME omission, `services/service-com-amazonaws-rum-rum/src/ops/batch_delete_rum_metric_definitions.rs:180-187`:
```
            // `AppMonitorName` is a path label and `Destination` a single query value:
            // either is absent only from a request that named no target at all, which
            // stays the framework's refusal.
            ...
            destination: destination
                .ok_or_else(|| ::aws_smithy_schema_server::missing_member(DESTINATION_MEMBER))?,
```
The crate's own test standard for this class of request, `services/service-com-amazonaws-rum-rum/tests/all/tagging_no_content.rs:489-493`:
```
    // is served by `UntagResource`, so it answers that
    // operation's own refusals for a bad request rather than a routing failure. ... each one is refused with
    // the 400 the tagging frontend words a bad request with — never the 501 an unrouted
    // request would answer.
```
Test coverage of the gap: none. `tests/all/constraint_refusals.rs:584-593` sends `DeleteRumMetricsDestination` with `destination=Nowhere` (an invalid VALUE, refused 400) and `tests/all/metric_definition_validation.rs:135` always appends `?destination=CloudWatch`; no test omits the parameter, so nothing pins the 501 with a stated reason.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutRumMetricsDestination(AppMonitorName="m", Destination="CloudWatch")
3. Send `DELETE /rummetrics/m/metricsdestination` with NO query string at all. Observed: HTTP 501 with the routing-error envelope ("unknown operation"). Expected: the modelled ValidationException (HTTP 400) for the absent @required member `Destination`, the same refusal `DELETE /rummetrics/m/metrics` (no query) produces today.
4. Send `GET /rummetrics/m/metrics` with NO query string. Observed: HTTP 501; expected the same HTTP 400 ValidationException.
5. For contrast, send `DELETE /rummetrics/m/metrics` with NO query string: HTTP 400 ValidationException, because that route is listed in `is_query_unmatched_route`.
