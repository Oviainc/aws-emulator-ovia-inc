Issue-Severity: HIGH
Issue-Gap-Class: framework

# `vendor/smithy-schema-server` lets a greedy `@httpLabel` swallow the literal segments its `@http` pattern declares after it, so every `/{label+}/<sub-resource>` operation is unroutable

_Filed: 2026-10-22T06:00:00Z_
_Operation: vendor/smithy-schema-server (REST routing and HTTP binding) | Class: framework_

## Description

A Smithy `@http` URI pattern may place literal segments AFTER a greedy label, e.g.
WorkSpaces Web's `PUT /portals/{portalArn+}/userSettings` and
`GET /trustStores/{trustStoreArn+}/certificates`. The vendored REST layer resolves a
greedy label by consuming EVERY remaining path segment, in two places:

- `vendor/smithy-schema-server/src/rest_xml.rs::route_lookup` (shared by `rest_json`):
  `Segment::Greedy` sets `seg_idx = path_segments.len()`, so the next pattern segment —
  the trailing literal — has no segment left to match, `matched` becomes false, and the
  route is skipped. The lookup then falls through to the route whose pattern stops at the
  label (`PUT /portals/{portalArn+}` → `UpdatePortal`,
  `DELETE /portals/{portalArn+}` → `DeletePortal`, `GET /portals/{portalArn+}` →
  `GetPortal`).
- `vendor/smithy-schema-server/src/http_binding.rs::match_path_labels`: the greedy branch
  binds the label to `path_segs[si..].join("/")`, i.e. the ARN PLUS `/userSettings`.

So such a request reaches the wrong operation, and the label value it carries does not
satisfy the member's modelled constraints (for an `ARN` member, the anchored `@pattern`
rejects it), which the boundary validator answers as HTTP 400 `ValidationException`.

## Observable effect

Against the WorkSpaces Web emulator before the workaround below:

- `PUT /portals/<portal ARN>/userSettings?userSettingsArn=<arn>` → 400
  `ValidationException` ("Value 'arn:…:portal/<uuid>/userSettings' at 'portalArn' failed
  to satisfy constraint: Member must satisfy regular expression pattern: ^arn:…$")
  instead of 200.
- `DELETE /portals/<portal ARN>/userSettings` → reached `DeletePortal` instead of
  `DisassociateUserSettings`.
- `GET /portals/<portal ARN>/identityProviders` → reached `GetPortal`, answering a portal
  where a client expects an identity-provider list.

## Scope

Every operation whose pattern has this shape, in every service. In WorkSpaces Web alone
that is 19 operations: the 16 `PUT|DELETE /portals/{portalArn+}/<sub-resource>`
`Associate*`/`Disassociate*` operations, `GET /portals/{portalArn+}/identityProviders`,
and `GET /trustStores/{trustStoreArn+}/certificate(s)`. Other REST services with
sub-resource routes under a greedy label (S3-style `{Key+}` suffixes, any
`/{resourceArn+}/<thing>`) are affected the same way.

## Fix

In `route_lookup`, match a greedy segment non-greedily with respect to the pattern
segments that follow it: reserve the number of trailing non-greedy pattern segments and
let the greedy label consume only `path_segments[si .. len - trailing]`. In
`match_path_labels`, bind the greedy label to the same sub-slice rather than to
`path_segs[si..]`. Both functions already know the full pattern, so the trailing count is
a local computation and no caller changes.

This cannot be done from inside the emulator repo: `vendor/` is read-only here and the
merge gate refuses a change to it, so it needs a change to the vendored core library.

## Current workaround, and why it should be removed

`services/service-com-amazonaws-workspacesweb-awserminecontrolplaneservice/src/service.rs`
works around it inside the one seam the framework offers, `Service::override_operation`:
a `GreedyLabelSuffixRoute` adapter names the operation by matching the request against
the `@http` route on each affected operation's own input schema, then re-runs the
modelled operation against the request with the trailing literal segments removed. Even
read off the schemas, that is a per-service re-implementation of a router rule, and every
other affected service has to repeat it. It should be deleted once
`route_lookup`/`match_path_labels` are fixed.

## Evidence

`vendor/smithy-schema-server/src/rest_xml.rs` — `route_lookup`'s `Segment::Greedy` arm
(`seg_idx = path_segments.len()`) followed by the literal arm's
`if seg_idx >= path_segments.len() … matched = false`.
`vendor/smithy-schema-server/src/http_binding.rs` — `match_path_labels`' greedy arm
(`let rest = path_segs[si..].join("/")`), whose own unit test
`match_greedy_label` only covers a pattern that ENDS at the greedy label.
`services/service-com-amazonaws-workspacesweb-awserminecontrolplaneservice/tests/all/portal_lifecycle.rs`
— `an_association_sub_resource_reaches_its_own_operation` drives all 19 affected
WorkSpaces Web routes and fails without the workaround.
