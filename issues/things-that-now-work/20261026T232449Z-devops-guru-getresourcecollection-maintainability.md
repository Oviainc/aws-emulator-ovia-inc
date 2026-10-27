Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# The stale/foreign-NextToken refusal is hand-rolled in six paginated reads although `FilterRejection::stale_cursor` exists and reproduces four of them verbatim

_Filed: 2026-10-26T23:24:49Z_
_Operation: GetResourceCollection | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::FilterRejection::stale_cursor` is the crate's single home for the "this NextToken names no page of the collection it was replayed against" refusal, and its own documentation says every paginated read of this store answers a stale or foreign cursor the same way. Seven handlers call it. Six others build the identical `ValidationException` by hand — same `reason` ("FIELD_VALIDATION_FAILED"), same single `ValidationExceptionField` named "NextToken", same two sentences — so the one rule about how a cursor refusal is worded and shaped lives in seven places and the copies can drift independently.

Four of the six hand-rolled copies are byte-for-byte what `stale_cursor` already produces for the right arguments, so they can be replaced with a call with no change to any response:

- `get_resource_collection.rs` == `stale_cursor("GetResourceCollection", "this resource collection", "ResourceCollectionType")`
- `describe_resource_collection_health.rs` == `stale_cursor("DescribeResourceCollectionHealth", "this resource collection's health", "ResourceCollectionType")`
- `describe_organization_resource_collection_health.rs` == `stale_cursor("DescribeOrganizationResourceCollectionHealth", "this organization resource collection's health", "OrganizationResourceCollectionType, AccountIds and OrganizationalUnitIds")`
- `list_monitored_resources.rs` == `stale_cursor("ListMonitoredResources", "the monitored resources these filters select", "Filters")`

The remaining two (`list_notification_channels.rs`, `get_cost_estimation.rs`) differ only in that their field message has no "for the same <members>" clause, because those two operations bind no filter member; they can keep their text or the helper can grow an unscoped form. Either way the fix is text-preserving.

No behavioural change is intended or needed: the existing tests assert the error NAME and status only, never the message.

## Evidence

The existing single home — `src/state.rs:1166-1182`:
```rust
    /// The refusal of a `NextToken` that names no page of `collection`.
    ///
    /// Every paginated read of this store answers a stale or foreign cursor the same
    /// way: a token is bound to the selection it was issued for ...
    pub fn stale_cursor(operation: &str, collection: &str, scoped_by: &str) -> Self {
        Self {
            message: format!("NextToken does not name a page of {collection}"),
            reason: "FIELD_VALIDATION_FAILED",
            field: "NextToken".to_string(),
            field_message: format!(
                "must be a token returned by an earlier {operation} for the same {scoped_by}"
            ),
        }
    }
```
Called by seven handlers: `src/ops/list_insights.rs:198`, `src/ops/search_insights.rs:219`, `src/ops/list_organization_insights.rs:262`, `src/ops/search_organization_insights.rs:274`, `src/ops/list_events.rs:250`, `src/ops/list_recommendations.rs:260`, `src/ops/list_anomalous_log_groups.rs:226`.

The hand-rolled copy that the helper reproduces exactly — `src/ops/get_resource_collection.rs:182-194`:
```rust
        let invalid_next_token = || -> GetResourceCollectionError {
            GetResourceCollectionErrorModeled::ValidationException(ValidationException {
                message: "NextToken does not name a page of this resource collection"
                    .to_string(),
                reason: Some("FIELD_VALIDATION_FAILED".to_string()),
                fields: Some(vec![ValidationExceptionField {
                    name: "NextToken".to_string(),
                    message: "must be a token returned by an earlier GetResourceCollection for the same ResourceCollectionType"
                        .to_string(),
                }]),
            })
            .into()
        };
```
The other copies: `src/ops/describe_resource_collection_health.rs:212-224`, `src/ops/describe_organization_resource_collection_health.rs:270-286`, `src/ops/list_monitored_resources.rs:198-212`, `src/ops/list_notification_channels.rs:175-187`, `src/ops/get_cost_estimation.rs:186-197`.

Tests that cover these paths and stay unchanged (they assert the error name/status, not the message): `tests/all/configuration_lifecycle.rs:592` (GetResourceCollection stale/foreign cursor), `tests/all/resource_collection_health.rs:556`, `tests/all/organization_resource_collection_health.rs:777`, `tests/all/monitored_resources.rs:708,735`, `tests/all/cost_estimation.rs:862,884`.

## Steps to Reproduce

1. Open `src/state.rs:1166` and read `FilterRejection::stale_cursor`, whose doc states every paginated read of this store answers a stale or foreign cursor the same way.
2. grep the crate for `stale_cursor|NextToken does not name` under `src/ops/`: seven handlers call the helper, six build the same exception inline.
3. Compare `src/ops/get_resource_collection.rs:184,189` with the two `format!`s in `stale_cursor` — the strings are identical for arguments ("GetResourceCollection", "this resource collection", "ResourceCollectionType"); the same holds for describe_resource_collection_health.rs, describe_organization_resource_collection_health.rs and list_monitored_resources.rs.
4. Note that the cursor tests only assert the error name (e.g. `tests/all/configuration_lifecycle.rs:592`), so collapsing the copies onto the existing helper leaves every response and test unchanged.
