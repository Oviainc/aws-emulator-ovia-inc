Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 7027025c5fd5dfe34e91978241957b132a94afec

# The last page omits NextToken entirely instead of reporting the empty string the model documents

_Filed: 2026-10-28T19:04:44Z_
_Operation: ListConfigurationManagers | Class: pagination_

> _Note: this issue was identified against the code base at commit `7027025c5fd5dfe34e91978241957b132a94afec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both of this service's listings document their output `NextToken` as a string that is EMPTY when the listing is exhausted. The shared pager (`crate::state::paginate`) instead leaves the token as `None`, and the output member is an `Option<String>` that the restJson1 codec writes only when it is `Some` — so the last page of `ListConfigurationManagers` (and of `ListConfigurations`, the other caller of the same helper) answers a body with NO `NextToken` key at all rather than with the documented empty string. A client that reads the member to decide whether to page again sees a missing key where the model promises a present, empty string; the two listings are the only paginated operations of the service, so every paging client is affected on its final page.

The handler code states the opposite rule to the model in its own comment ("the last page carries none"), so the divergence is deliberate but contradicts the modelled contract.

## Evidence

Model (smithy_query(query="doc", shape="ListConfigurationManagersOutput", member="NextToken")): "The token to use when requesting the next set of configuration managers. If there are no additional operations to return, the string is empty."

Model (smithy_query(query="doc", shape="ListConfigurationsOutput", member="NextToken")): "The token to use when requesting the next set of items. If there are no additional items to return, the string is empty."

Code — the token stays absent, by design: `services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:1595` `let mut next_token = None;` and :1597-1601 "// The first record that does not fit becomes the next page's start, and only / // then is a token emitted — the last page carries none." … `next_token = Some(encode_page_token(kind, key));` — reached only on a break, so an exhausted listing returns `None`.

The handlers hand that `None` straight to the wire: `src/ops/list_configuration_managers.rs:212-219` `Ok(ListConfigurationManagersOutput { configuration_managers_list: Some(..), next_token, })` and `src/ops/list_configurations.rs:235-242` `Ok(ListConfigurationsOutput { configurations_list: Some(..), next_token, })`, with `pub next_token: Option<String>` on both output structs (`src/ops/types/list_configurations_types.rs:28`).

An `Option` member is omitted when `None`: `src/ops/types/date_time_wire.rs:177-188` — "/// Write a member only when the value is there: an `Option` one when it is `Some`, a / /// non-`Option` one (`@required`) always — the vendored codegen's own `is_option` split" … `if let ::std::option::Option::Some(__v) = $this.$field.as_ref() { qs_write_member!(..) }`.

Test coverage proving the member is absent today: `tests/all/region_scoping.rs:245-250` — "// A Region holding nothing still reports the list, and reports no token: there is no / // next page to resume from." `assert!(answer.get("NextToken").is_none_or(Value::is_null), ...)`; the paging loops in `tests/all/configuration_managers.rs:638-641` and `tests/all/deployment_targets.rs:813` also treat a missing key as the end of the listing.

## Steps to Reproduce

1. Create one configuration manager (POST /configurationManager with a definition whose Parameters name TargetOrganizationalUnits and TargetRegions), signed for us-east-1.
2. POST /listConfigurationManagers with {} (MaxItems unset, so the whole store fits on one page).
3. Observe the 200 body: it carries `ConfigurationManagersList` but no `NextToken` member at all, where the model documents the member as the empty string when there are no additional items to return.
4. POST /listConfigurations with {} and observe the same omission on the exhausted page.
