Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3ebf0e2f135a02ae75ffd55898f9ee21fc25614a

# UpdateFlowSource moves EntitlementArn and IngestPort independently, so a source can be stored entitled AND holding an ingest endpoint — a record the add path's one rule cannot produce

_Filed: 2026-10-28T20:12:49Z_
_Operation: UpdateFlowSource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3ebf0e2f135a02ae75ffd55898f9ee21fc25614a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states one rule about an entitled source: a source reached through another account's entitlement is fed by that account's flow, so this service assigns it no ingest endpoint of its own. `StoredSource::from_set_request` — "the crate's ONE materialisation" used by CreateFlow and AddFlowSources — implements that rule by deriving BOTH `ingest_ip` and `ingest_port` from `entitlement_arn.is_some()`.

UpdateFlowSource never re-applies the rule. It copies `EntitlementArn` and `IngestPort` onto the stored record as two independent member-wise merges, so it can leave the source in either of two states that no create or add path can ever produce:

1. Setting `EntitlementArn` on a previously OWNED source leaves the stored `ingest_ip` and `ingest_port` in place. DescribeFlow then reports a source that is entitled (ListFlows reports the flow's `SourceType` as `ENTITLED`, read off the same `entitlement_arn` marker) while still advertising an ingest IP and port of THIS account that nothing feeds.
2. Setting `IngestPort` on an ENTITLED source (which was stored with `ingest_ip: None` and `ingest_port: None`) stores a port with no ingest address beside it — half an ingest endpoint.

The source's identity members (`Name`, `SourceArn`) are deliberately carried over untouched in this handler, and `MaxBitrate` IS re-resolved through the add path's `assigned_source_max_bitrate` precisely so that "a source whose protocol moves onto a smoothing protocol reports the same bitrate it would have been created with". The entitled/ingest-endpoint pair is the one service-derived relation the update skips.

## Evidence

The crate's ONE rule, on the add path — services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:1487-1522:
```
    /// Every operation that adds a source to a flow (CreateFlow's `Source` / `Sources`,
    /// AddFlowSources) materialises it HERE, so one request shape always becomes the same
    /// stored record: the service mints the `SourceArn`, assigns the ingest endpoint an
    /// owned source listens on, and folds the request's transport members into
    /// `Source.Transport`.
...
        // A source reached through another account's entitlement is fed by that account's
        // flow, so this service assigns it no ingest endpoint of its own; an owned source
        // listens on the address and port the service assigns it.
        let entitled = request.entitlement_arn.is_some();
...
            ingest_ip: (!entitled).then(|| assigned_ingest_ip(suffix)),
            ingest_port: (!entitled).then(|| {
                request
                    .ingest_port
                    .unwrap_or_else(|| assigned_ingest_port(suffix))
            }),
            whitelist_cidr: request.whitelist_cidr.clone(),
            entitlement_arn: request.entitlement_arn.clone(),
```
The update that breaks it — src/ops/update_flow_source.rs:471-487:
```
        // What each setting becomes once this request is applied: the request's value
        // where it carries one, the stored value where it does not.
        if let Some(description) = input.description.clone() {
            updated.description = Some(description);
        }
        if let Some(ingest_port) = input.ingest_port {
            updated.ingest_port = Some(ingest_port);
        }
        if let Some(whitelist_cidr) = input.whitelist_cidr.clone() {
            updated.whitelist_cidr = Some(whitelist_cidr);
        }
        if let Some(entitlement_arn) = input.entitlement_arn.clone() {
            updated.entitlement_arn = Some(entitlement_arn);
        }
```
`entitlement_arn` is the crate's one marker of an entitled source, which a reader already depends on — src/state.rs:2529-2541:
```
    /// `ListedFlow.SourceType`: `ENTITLED` when the flow ingests through an entitlement
    /// granted by another account, `OWNED` otherwise.
    pub fn source_type(&self) -> &'static str {
        if self
            .sources
            .iter()
            .any(|source| source.entitlement_arn.is_some())
        {
            SOURCE_TYPE_ENTITLED
```
And the same handler DOES re-resolve the other service-derived transport member through the add path's rule — src/ops/update_flow_source.rs:233-247:
```
/// `MaxBitrate` is resolved through [`crate::state::assigned_source_max_bitrate`], the one
/// place this crate turns a requested bitrate into the one a source reports, so a source
/// whose protocol moves onto a smoothing protocol reports the same bitrate it would have
/// been created with.
...
    merged.max_bitrate = crate::state::assigned_source_max_bitrate(
        &merged.protocol,
        input.max_bitrate.or(merged.max_bitrate),
    );
```
Test coverage: tests/all/update_flow_source.rs:218-230 updates a source with only `entitlementArn` and asserts nothing but that member being echoed back — it does not pin what happens to `ingestIp`/`ingestPort`:
```
            case: "an entitlement ARN",
            body: json!({
                "entitlementArn":
                    "arn:aws:mediaconnect:us-east-1:111122223333:entitlement:1-0000beef:shared"
            }),
            expect: vec![(
                "entitlementArn",
```

## Steps to Reproduce

1. POST /v1/flows with sources [{"name": "primary", "protocol": "rtp", "whitelistCidr": "0.0.0.0/0"}] — 201; note that the source reports `ingestIp` and `ingestPort` assigned by the service. Keep FlowArn and SourceArn.
2. PUT /v1/flows/{FlowArn}/source/{SourceArn} with {"entitlementArn": "arn:aws:mediaconnect:us-east-1:111122223333:entitlement:1-0000beef:shared"} — 202.
3. Observe the returned source, and GET /v1/flows/{FlowArn}, reporting BOTH `entitlementArn` and the old `ingestIp`/`ingestPort`; GET /v1/flows reports `sourceType: ENTITLED` for that flow while its source still advertises this account's ingest endpoint. CreateFlow/AddFlowSources can never produce such a source.
4. Now the other direction: POST /v1/flows whose source names an `entitlementArn` — 201; the source reports no `ingestIp` and no `ingestPort`.
5. PUT /v1/flows/{FlowArn}/source/{SourceArn} with {"ingestPort": 5101} — 202, and observe the source now reporting `ingestPort: 5101` with no `ingestIp` beside it.
