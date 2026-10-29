Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: cbb9267c7abd2d15d6f906cf892ef291750ce9f0

# A ListVirtualMachines NextToken carries no record of the HypervisorArn filter it was issued under, so resuming with a different filter silently skips matching machines and reports an empty final page

_Filed: 2026-10-28T19:16:42Z_
_Operation: ListVirtualMachines | Class: pagination_

> _Note: this issue was identified against the code base at commit `cbb9267c7abd2d15d6f906cf892ef291750ce9f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListVirtualMachines` is the only listing of this service that takes a FILTER (`HypervisorArn`, optional) beside its page controls. The `NextToken` it mints encodes nothing but the ARN of the last record on the page (`crate::state::encode_page_token` = hex of that ARN), and the token it accepts is validated on SHAPE alone (`crate::state::decode_page_token` checks hex / UTF-8 / "is a `vm/<id>` ARN of this service"). The filter the token was issued under is neither encoded in it nor compared against the request that presents it.

Because the store is one ARN-keyed map for every hypervisor's machines and `page_of_where` turns the decoded ARN straight into `Bound::Excluded(..)`, a token minted by a listing filtered on hypervisor H2 is accepted by a listing filtered on H1 and resumes the H1 listing after an ARN that sorts ABOVE every machine of H1. Every matching machine is then dropped by the filter, so the handler answers HTTP 200 with `VirtualMachines: []` and NO `NextToken` — the shape of a correct final page. A client therefore learns "this hypervisor has no virtual machines" for a hypervisor whose inventory the very same store holds, with no error of any kind. The reverse pairing (a token from an H1 page presented with `HypervisorArn=H2`, or a token from a filtered page presented with no filter at all) truncates the listing the same way.

This is not the already-filed "token this service never issued" hole (ListGateways/pagination): the token here is one this service really did issue, for this very operation and this very resource type, so no shape check can ever catch it. The gap is that the token carries no identity of the query it resumes, while the state module promises that "a filtered listing pages exactly like an unfiltered one" with the same boundary contract as `page_of_gateways` — whose contract is explicitly "without ever repeating or skipping a record that stayed put".

## Evidence

The promise the crate makes for this listing — services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/state.rs:1929-1934 (doc of `page_of_virtual_machines`):

    /// ... Nor is a machine whose discovering sync has not fallen due yet
    /// ([`StoredVirtualMachine::discovered_as_of`]), or one of another hypervisor: every
    /// filter is applied BEFORE the page is cut, so a page holds up to `limit` MATCHING
    /// machines and carries a token only while a matching machine remains past its
    /// boundary, and a filtered listing pages exactly like an unfiltered one. Same boundary
    /// contract as [`Self::page_of_gateways`], whose implementation it shares.

and the boundary contract it inherits — src/state.rs:1381-1386 (doc of `page_of_gateways`):

    /// `after` is the ARN the previous page ended on (resolved from a `NextToken`), and
    /// the page starts at the next key AFTER it — so a record created, or deleted,
    /// between two pages changes what the second page holds without ever repeating or
    /// skipping a record that stayed put.

What the token actually carries — src/state.rs:2154-2162:

    /// The opaque pagination token for a page that ended on `arn`.
    ///
    /// The token is the page boundary itself (the last ARN returned), hex-encoded so a
    /// client has nothing to read in it and treats it as opaque, as the modelled `NextToken`
    /// requires. ...
    pub fn encode_page_token(arn: &str) -> String {
        hex::encode(arn)
    }

What is checked when one comes back — src/state.rs:2167-2176:

    /// `None` for anything this service cannot have issued for THIS listing: not hex, not
    /// UTF-8, not an ARN of this service, or an ARN naming another resource type — a token
    /// one listing handed out does not resume another, because the page boundary it encodes
    /// is a key of a different map. ...
    pub fn decode_page_token(token: &str, resource_type: &str) -> Option<String> {
        let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
        is_resource_arn(&decoded, resource_type).then_some(decoded)
    }

so the only thing a `vm/<id>` token is checked against is the resource TYPE — never the `HypervisorArn` of the request presenting it.

The handler, which decodes the token and then applies the filter independently of it — src/ops/list_virtual_machines.rs:162-180 and :192-209:

        // A `NextToken` resumes after the page boundary it encodes. One this service did
        // not issue for a virtual-machine listing is refused, never quietly treated as
        // "start from the top".
        let after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                crate::state::decode_page_token(token, crate::state::VIRTUAL_MACHINE_RESOURCE_TYPE)
                    .ok_or_else(|| { ... })?,
            ),
        };
    ...
        // `HypervisorArn` is an optional FILTER on the listing, not a resource this
        // operation resolves: ... so an ARN the store holds no hypervisor for selects no
        // machine and the listing is empty rather than refused.
    ...
        let page = state.lock().page_of_virtual_machines(
            &region,
            input.hypervisor_arn.as_deref(),
            after.as_deref(),
            limit,
            ::std::time::SystemTime::now(),
        );

The boundary is applied before the filter in the shared pager — src/state.rs:2081-2093:

        let lower = match after {
            Some(key) => Bound::Excluded(key.to_string()),
            None => Bound::Unbounded,
        };
        let mut remaining = records
            .range((lower, Bound::Unbounded))
            .filter(|(arn, record)| keep(arn, record));
        let page: Vec<(&String, &T)> = remaining.by_ref().take(limit).collect();
        // A token only when a record really remains past this page: the final page must
        // not carry one, or a paginating client never stops.
        let next_token = remaining
            .next()
            .and_then(|_| page.last().map(|(key, _)| encode_page_token(key)));

so a boundary above every matching record yields `items: []` and `next_token: None` — indistinguishable from a legitimate last page.

Machine ARNs are minted from one monotonic counter shared by all hypervisors (src/state.rs:1869-1874, `format!("vm-{:016x}", self.next_virtual_machine_id)`), so the machines of the hypervisor synced first all sort BELOW the machines of the one synced second — which is what makes the skip reachable with ordinary, service-issued tokens.

Test coverage: the paging walk always re-sends the same filter, so the cross-filter case is untested — tests/all/virtual_machine_listing.rs:306-324:

    async fn walk_pages(
        router: &MultiServiceRouter,
        hypervisor_arn: Option<&str>,
        max_results: Option<i64>,
    ) -> Vec<String> {
        ...
            if let Some(arn) = hypervisor_arn {
                body["HypervisorArn"] = json!(arn);
            }
            ...
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }

and the bad-token table (tests/all/virtual_machine_listing.rs:617-637) holds only tokens of other operations or malformed strings — no row presents a token this listing issued under a different `HypervisorArn`.

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted.
2. Call ImportHypervisorConfiguration twice (distinct Name/Host) -> HypervisorArn H1 (hypervisor/hv-...01) and H2 (hypervisor/hv-...02).
3. Call StartVirtualMachinesMetadataSync on H1, then on H2, and wait for both syncs to fall due (METADATA_SYNC_SUCCEEDS_AFTER = 500ms). H1's inventory is now vm-...01..03 and H2's is vm-...04..06.
4. Call ListVirtualMachines with {"HypervisorArn": H1} -> 200 with three machines, so H1's inventory is non-empty.
5. Call ListVirtualMachines with {"HypervisorArn": H2, "MaxResults": 1} -> 200 with one machine (vm-...04) and a NextToken T, which is hex of that ARN.
6. Call ListVirtualMachines with {"HypervisorArn": H1, "NextToken": T} -> HTTP 200 with "VirtualMachines": [] and NO NextToken: every machine of H1 sorts below the boundary T encodes and is dropped by the filter, so a client paginating H1 with a token it was legitimately handed concludes H1 has no virtual machines.
7. Repeat the other way round: take a token from {"HypervisorArn": H1, "MaxResults": 1} and present it with no HypervisorArn at all -> the unfiltered listing silently omits every machine below that boundary instead of reporting the whole inventory or refusing the token.
