Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1ce4aa1a23419b16e4e6f0ae0f19fcef3b94726a

# The minted HypervisorArn is built from the unbounded signing region, so an import can store a record under an ARN its own API then refuses

_Filed: 2026-10-28T18:26:54Z_
_Operation: ImportHypervisorConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1ce4aa1a23419b16e4e6f0ae0f19fcef3b94726a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ImportHypervisorConfiguration` mints the store KEY of the new hypervisor from the region it reads out of the request's SigV4 credential scope. That value is caller-supplied and is length-unbounded on this path: `crate::state::request_region` passes it through `crate::state::arn_region`, which checks only the CHARACTER CLASS (`[a-zA-Z-0-9]`) and substitutes `DEFAULT_REGION` for anything else — it never bounds the length. The ARN the import then persists is `arn:aws:backup-gateway:<region>:000000000000:hypervisor/hv-<16 hex>`, i.e. 67 characters plus the region, so a request signed for a region longer than 433 characters produces a stored key longer than 500 characters.

500 is exactly the maximum the model puts on `ServerArn`/`ResourceArn`, the shape every operation that takes a hypervisor ARN as INPUT targets. Input constraints are enforced by the boundary validator before any handler runs (the handlers say so themselves), so the ARN the import returns is one that `GetHypervisor`, `UpdateHypervisor`, `DeleteHypervisor`, `StartVirtualMachinesMetadataSync`, `PutHypervisorPropertyMappings`, `GetHypervisorPropertyMappings` and the three tagging operations all refuse with 400 `ValidationException` ("Member must have length less than or equal to 500"). The record is therefore unreadable, un-updatable and — the part that matters — UNDELETABLE: it stays in the store for the life of the process, is still reported by `ListHypervisors` for that region (whose output member is not validated, so the over-long ARN goes out on the wire), and permanently occupies one of the `MAX_STORED_HYPERVISORS` slots the sibling ceiling check counts.

The crate already knows this hazard and guards the OTHER path where a caller-supplied region is kept on a record: the activation handshake bounds `activationRegion` to `MAX_ACTIVATION_REGION_LENGTH` (64) for exactly the stated reason, and because `CreateGateway` only redeems a key some handshake issued for that same region, no gateway ARN can exceed its own modelled 180. The hypervisor import has no such bound: it is the one minting path that writes the raw signing region into a store key.

## Evidence

The mint — `src/ops/import_hypervisor_configuration.rs:178-179` and `:213-222`:

        // A hypervisor is regional, so its ARN carries the region the caller signed for.
        let region = crate::state::request_region(ctx.parts());
    ...
        let imported = store.import_hypervisor(
            &region,
            crate::state::HypervisorConfiguration { ... },
            ::std::time::SystemTime::now(),
        );

which becomes the store's key — `src/state.rs:1579-1585` (`import_hypervisor`):

        let hypervisor_id = format!("hv-{:016x}", self.next_hypervisor_id);
        self.next_hypervisor_id += 1;
        let arn = ::engine::emulator_arn(
            ARN_SERVICE,
            &arn_region(region),
            &format!("{HYPERVISOR_RESOURCE_TYPE}/{hypervisor_id}"),
        );

The only normalisation applied — `src/state.rs:2386-2394`:

    /// The region to mint an ARN in, falling back to [`DEFAULT_REGION`] for anything that
    /// would not satisfy the modelled ARN pattern's `[a-zA-Z-0-9]+` region segment.
    pub fn arn_region(region: &str) -> String {
        if is_arn_segment(region) {
            region.to_string()
        } else {
            DEFAULT_REGION.to_string()
        }
    }

and `src/state.rs:2427-2431`:

    pub fn request_region(parts: &::http::request::Parts) -> String {
        ::emulator_server::sigv4_credential_scope(&parts.headers)
            .map(|scope| arn_region(&scope.region))
            .unwrap_or_else(|| DEFAULT_REGION.to_string())
    }

`is_arn_segment` (`src/state.rs:2366-2371`) is charset-only: `!segment.is_empty() && segment.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')`. The scope field it is handed is unbounded — `vendor/server/src/lib.rs:1800-1808` rejects only an EMPTY region: `if access_key_id.is_empty() || date.is_empty() || region.is_empty() || service.is_empty() { return None; }`.

The modelled bound the minted key can exceed — `OPERATIONS.md:125` (import output) and `:131`, `:144` (the inputs that must resolve it):

    - **Output:** `HypervisorArn` (string, len 50..500, pattern ^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$)
    ## GetHypervisor
    - **Input:** `HypervisorArn` (string, len 50..500, ... required)
    ## DeleteHypervisor
    - **Input:** `HypervisorArn` (string, len 50..500, ... required)

and that such an input never reaches a handler — `src/ops/get_hypervisor.rs:152-154`:

        // `HypervisorArn`'s length and pattern are refused by the boundary validator
        // before `handle` runs, so reaching here means the ARN is well formed; whether it
        // names a hypervisor is a fact of the STORE, which is what this lookup asks.

(the framework validates INPUT only — `vendor/smithy-server-runtime/src/operation.rs:412-424` runs `constraints::validate(&input, O::INPUT_SCHEMA)` and then serialises the output with no validate step.)

The sibling path that DOES bound the same caller-supplied region, with the reason stated — `src/service/activation.rs:71-77` and `:156-160`:

    /// The longest `activationRegion` a handshake may name.
    ///
    /// The value is caller-supplied and is KEPT on the activation record, so its length is
    /// bounded here rather than stored as sent: an unsigned request must not be able to grow
    /// this service's memory by the size of its own query string. An EMULATOR bound, well past
    /// the longest region name AWS publishes.
    const MAX_ACTIVATION_REGION_LENGTH: usize = 64;
    ...
    fn names_a_region(&self) -> bool {
        !self.region.is_empty()
            && self.region.len() <= MAX_ACTIVATION_REGION_LENGTH
            && crate::state::arn_region(&self.region) == self.region
    }

which is what keeps the gateway ARN inside its own modelled 180, because `src/ops/create_gateway.rs:197` only mints a gateway for a region some handshake issued a key for:

        if !store.activation_key_is_redeemable(&input.activation_key, &region) {

Test coverage: `tests/all/regional_listing_scope.rs` drives five regions, all ordinary region names, and `tests/all/hypervisor_reads.rs` / `hypervisor_listing.rs` never vary the credential-scope region's LENGTH — so no test exercises a region long enough to push a minted ARN past the modelled bound, and nothing pins that a minted ARN is always resolvable by the operations that take it.

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted.
2. POST / with header `X-Amz-Target: BackupOnPremises_v20210101.ImportHypervisorConfiguration`, body {"Name":"hv-long-region","Host":"10.0.0.1"}, and header `Authorization: AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/<434 copies of the letter a>/backup-gateway/aws4_request, SignedHeaders=host, Signature=x` (the region segment is charset-valid, so `arn_region` keeps it verbatim).
3. Observe HTTP 200 whose `HypervisorArn` is 501 characters long — longer than the 500 the modelled `ServerArn` allows for every input that takes it.
4. Call GetHypervisor (or UpdateHypervisor, DeleteHypervisor, StartVirtualMachinesMetadataSync, TagResource, ListTagsForResource) with that exact ARN -> HTTP 400 ValidationException, "Member must have length less than or equal to 500": the record cannot be read, updated, tagged or deleted.
5. Call ListHypervisors signed for the same long region -> the record is reported (with the over-long ARN), proving it is really in the store, and it stays there for the life of the process while counting against MAX_STORED_HYPERVISORS.
6. Contrast with the gateway path: GET /?activationRegion=<the same 434-character string>&no_redirect -> 400 "The activation region must name a region: at most 64 characters…", so no gateway can be minted under an unusable ARN.
