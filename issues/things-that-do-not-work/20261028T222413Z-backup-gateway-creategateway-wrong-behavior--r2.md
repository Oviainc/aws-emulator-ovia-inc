Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 68144f2b1a9e2dba602611483c97527f7042e8b8

# The mint registry resets a region's identity counter when it is trimmed, so this store re-mints an ARN it already handed out

_Filed: 2026-10-28T22:24:13Z_
_Operation: CreateGateway | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `68144f2b1a9e2dba602611483c97527f7042e8b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every ARN this crate hands out comes from one place — `ComAmazonawsBackupgatewayBackuponpremisesV20210101State::mint_resource`, reached from `create_gateway` (CreateGateway), `import_hypervisor` (ImportHypervisorConfiguration) and `record_virtual_machine` (StartVirtualMachinesMetadataSync). The sequence behind an ARN lives in `next_resource_ids`, keyed by `(kind, region)`, and both the field's doc and `bound_mint_sequences`'s doc assert that an identity is never handed out twice.

`bound_mint_sequences` breaks that assertion. It does not evict entries; it `retain`s only the `(kind, region)` pairs the store currently holds a RECORD for and DROPS the rest. A dropped pair's counter is gone, so the next `mint_resource` for that pair takes `.entry(...).or_default()` = 0 and re-mints, from sequence zero, ARNs this same process already minted and already put on the wire. The region half of the key comes off the caller's SigV4 credential scope (`request_region` -> `arn_region`, charset-checked only), so which pairs exist is caller-driven.

Three consequences follow, all inside this crate:

1. A gateway keeps naming a hypervisor the store no longer holds — the crate does this deliberately (`remove_hypervisor`'s doc) and `gateway_for_hypervisor` is what resolves that dangling link. Once `(Hypervisor, R)` has been dropped and a new import in R re-mints the deleted hypervisor's ARN, that stale link silently re-binds to a NEW, unrelated hypervisor: `GetGateway` reports its `HypervisorId`, and `PutHypervisorPropertyMappings` for the new hypervisor passes its `GATEWAY_OUT_OF_DATE` gate on the strength of an association that was made to a different resource.
2. A `NextToken` a client is still holding is resolved by `is_minted_resource_arn`, which asks `sequence < next_resource_id(resource, region)`. Immediately after the drop, a token this store really issued for that region is refused 400 `ValidationException`; once a re-mint has happened, the token is accepted again but its boundary is now the ARN of a LIVE record, which `page_of_where` applies as `Bound::Excluded` — so the resumed listing silently skips that record. `bound_mint_sequences`'s own excuse for the drop ("a pair with no record lists nothing, so the page boundaries its sequence would have resumed name an empty listing anyway") stops being true the moment a new record is minted in that region.
3. Two different resources of one run share one ARN, which is the thing both doc comments say cannot happen.

The defect is the RESET of the identity counter on trim, not the size of the registry bound: nothing here argues the ceiling is too small and nothing is "lost state" that a bigger ceiling would keep.

## Evidence

The rule the crate states for itself — `src/state.rs:1414-1418` (the `next_resource_ids` field doc):

    /// The next sequence to mint for each (resource kind, region) pair — the number behind
    /// the `<kind>/<id>` ARN suffix.
    ///
    /// Monotonic per PAIR, so every minted ARN is unique within a run and the sequence a
    /// test sees is reproducible.

and `src/state.rs:2338-2343` (the doc directly above the code that breaks it):

    /// Keep the mint registry inside [`MAX_MINT_SEQUENCES`] by dropping the sequences of
    /// the (kind, region) pairs this store holds no record for.
    ///
    /// A pair a record is stored under keeps its sequence, so no minted ARN can ever be
    /// minted twice; a pair with no record lists nothing, so the page boundaries its
    /// sequence would have resumed name an empty listing anyway.

The code under it — `src/state.rs:2344-2350`:

    fn bound_mint_sequences(&mut self) {
        if self.next_resource_ids.len() < MAX_MINT_SEQUENCES {
            return;
        }
        let stored = self.stored_mint_sequence_keys();
        self.next_resource_ids.retain(|key, _| stored.contains(key));
    }

`stored_mint_sequence_keys` (`src/state.rs:2353-2371`) builds the retained set from the three record maps' KEYS, so a `(kind, region)` pair whose records have all been deleted is not in it and its counter is discarded. The very next mint then starts that pair again at zero — `src/state.rs:2325-2336`:

    fn mint_resource(&mut self, resource: MintedResource, region: &str) -> (String, String) {
        self.bound_mint_sequences();
        let region = arn_region(region);
        let sequence = self
            .next_resource_ids
            .entry((resource, region.clone()))
            .or_default();
        let id = minted_resource_id(resource.id_prefix(), *sequence);
        let arn = minted_resource_arn(resource, &region, *sequence);
        *sequence += 1;
        (id, arn)
    }

The dangling association a recycled hypervisor ARN re-binds — `src/state.rs:1844-1851` (`remove_hypervisor`'s doc):

    /// The ASSOCIATION a gateway holds to it does NOT go with it: the link is a fact about
    /// the GATEWAY ([`Self::associate_gateway_to_server`]), which stays its own resource,
    /// and `DisassociateGatewayFromServer` is the operation that clears it. So a gateway
    /// still names the server it was associated with, which is what
    /// [`Self::gateway_for_hypervisor`] resolves and why
    /// `PutHypervisorPropertyMappings` can tell a hypervisor the store no longer holds
    /// ([`MissingResource::Hypervisor`]) from one no gateway serves
    /// ([`GATEWAY_OUT_OF_DATE`]).

resolved by ARN equality alone — `src/state.rs:1935-1939`:

    pub fn gateway_for_hypervisor(&self, hypervisor_arn: &str) -> Option<&StoredGateway> {
        self.gateways
            .values()
            .find(|gateway| gateway.hypervisor_arn.as_deref() == Some(hypervisor_arn))
    }

and consulted as the gate of `src/ops/put_hypervisor_property_mappings.rs:236`:

        if store.gateway_for_hypervisor(&input.hypervisor_arn).is_none() {

The token side — `src/state.rs:2306-2307` (`is_minted_resource_arn`):

        sequence < self.next_resource_id(resource, parts.region)
            && arn == minted_resource_arn(resource, parts.region, sequence)

whose boundary is applied exclusively — `src/state.rs:2410-2413` (`page_of_where`):

    let lower = match after {
        Some(key) => Bound::Excluded(key.to_string()),
        None => Bound::Unbounded,
    };

and the caller-driven region half of the key — `src/state.rs:2835-2839`:

    pub fn request_region(parts: &::http::request::Parts) -> String {
        ::emulator_server::sigv4_credential_scope(&parts.headers)
            .map(|scope| arn_region(&scope.region))
            .unwrap_or_else(|| DEFAULT_REGION.to_string())
    }

Test coverage: none. A grep of `tests/` for `MAX_MINT_SEQUENCES`, `next_resource_ids` and `mint_sequence` returns nothing, while the crate's OTHER bounded registry has its eviction pinned over the wire (`tests/all/gateway_activation.rs:536-557` drives `MAX_PENDING_ACTIVATION_KEYS` issuances and asserts the registry's size), so the pattern is tested where the crate chose to test it and untested here. `tests/all/regional_listing_scope.rs` drives five ordinary regions and `tests/all/page_token_provenance.rs` never drains a region of its records, so nothing pins that a minted ARN is minted once.

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted and SigV4 scopes honoured.
2. In region r0: perform the activation handshake (GET /?activationRegion=r0&no_redirect), call CreateGateway with the issued key signing for r0, and keep that gateway G. Call ImportHypervisorConfiguration signing for r0 -> note HypervisorArn H0 (its id is hv-0000000000000000, sequence 0 of the (Hypervisor, r0) pair).
3. Call AssociateGatewayToServer{GatewayArn: G, ServerArn: H0} signing for r0 -> 200. Then DeleteHypervisor{H0} -> 200. G still names H0 (src/state.rs:1844-1851), so (Hypervisor, r0) now holds a sequence with no record behind it.
4. Drive the mint registry to MAX_MINT_SEQUENCES (= MAX_STORED_GATEWAYS + MAX_STORED_HYPERVISORS + MAX_STORED_VIRTUAL_MACHINES): for i in 1..=12000, sign an ImportHypervisorConfiguration for a distinct region name r<i> (charset-valid, so arn_region keeps it verbatim) and DeleteHypervisor the result, so each call adds one (Hypervisor, r<i>) pair while the stored count stays under MAX_STORED_HYPERVISORS.
5. Call ImportHypervisorConfiguration signing for r0 again. bound_mint_sequences now runs and retains only pairs with a stored record, dropping (Hypervisor, r0); the mint restarts at sequence 0, so the new hypervisor H1 is returned under the SAME ARN the deleted H0 had.
6. Call GetGateway{G} -> it reports HypervisorId = the id of H1, a hypervisor G was never associated with. Call PutHypervisorPropertyMappings{HypervisorArn: H1, ...} -> 200, because gateway_for_hypervisor(H1) resolves G through the stale link, where the same call before step 5 was refused 400 ValidationException/GatewayOutOfDate.
7. Pagination half: before step 4, page ListHypervisors in r0 at MaxResults=1 and keep the NextToken (its boundary is H0's ARN). After step 5, replay that NextToken against ListHypervisors in r0 -> the page starts strictly after the boundary, so the live record H1 (whose ARN equals the boundary) is silently skipped; replaying it between steps 4 and 5 is answered 400 ValidationException for a token this service itself issued.
