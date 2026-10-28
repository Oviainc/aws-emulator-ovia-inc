Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 21d30b53374928fd663edcc8ae486344ab510a4e

# A NextToken this service never issued is served as a page, and one sorting below every record silently restarts the listing from the top

_Filed: 2026-10-28T04:27:45Z_
_Operation: ListGateways | Class: pagination_

> _Note: this issue was identified against the code base at commit `21d30b53374928fd663edcc8ae486344ab510a4e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three listings (ListGateways, ListHypervisors, ListVirtualMachines) resolve their `NextToken` through the shared helper `crate::state::decode_page_token`, which accepts ANY hex-encoded string that happens to parse as a well-formed backup-gateway ARN of the listing's own resource type. Whether this service ever minted that ARN — whether any record is stored under it at all — is never checked: the decoded string is handed straight to `page_of_where` as `Bound::Excluded(...)`.

Two consequences, both of which the crate's own text says must not happen:

1. A forged token that is a well-formed ARN of the right resource type (e.g. hex of `arn:aws:backup-gateway:us-east-1:<account>:gateway/bgw-ffffffffffffffff`) is ACCEPTED and answered 200 with an arbitrary suffix of the listing, instead of the modelled `ValidationException` the handler comment promises.
2. A forged token whose ARN sorts BELOW every stored key (e.g. hex of `arn:aws:backup-gateway:a:0:gateway/0` — the region segment `a` sorts before `us-east-1`) makes the range bound include every record, so the listing hands back PAGE ONE plus a fresh token. A client that mangled a token therefore loops over page one forever, which is exactly the outcome `decode_page_token`'s doc says it must not produce.

The only ill-formedness the helper detects is "not hex", "not UTF-8", "not an ARN of this service" and "an ARN naming another resource type" — and the existing tests only exercise those four shapes, so the never-issued-but-well-formed form is both unhandled and untested.

## Evidence

The rule the crate states for itself — services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/state.rs:1975-1987:

    /// `None` for anything this service cannot have issued for THIS listing: not hex, not
    /// UTF-8, not an ARN of this service, or an ARN naming another resource type — a token
    /// one listing handed out does not resume another, because the page boundary it encodes
    /// is a key of a different map. The caller answers `None` with the modelled
    /// `ValidationException`, never by silently restarting the listing from the top: a
    /// client that mangled a token must learn that, not receive page one again.
    pub fn decode_page_token(token: &str, resource_type: &str) -> Option<String> {
        let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
        is_resource_arn(&decoded, resource_type).then_some(decoded)
    }

Nothing in that body consults the store, so "this service cannot have issued it" is decided by SHAPE alone. `is_resource_arn` (state.rs:2030-2043) only requires the partition, the `backup-gateway` literal, the matching resource type and `[a-zA-Z-0-9]`-only segments.

The handler comment that is broken — src/ops/list_gateways.rs:150-151 (verbatim siblings at list_hypervisors.rs:152-154 and list_virtual_machines.rs:159-161):

        // A `NextToken` resumes after the page boundary it encodes. One this service did
        // not issue is refused, never quietly treated as "start from the top".

How the decoded value is used — src/state.rs:1903-1909:

        let lower = match after {
            Some(key) => Bound::Excluded(key.to_string()),
            None => Bound::Unbounded,
        };
        let mut remaining = records
            .range((lower, Bound::Unbounded))
            .filter(|(_, record)| keep(record));

so a forged key that sorts below every stored ARN yields the whole store, i.e. page one.

Test coverage proves the hole is untested, while stating the stronger rule — tests/all/hypervisor_listing.rs:456-503:

    /// A `NextToken` this listing did not issue is refused with HTTP 400
    /// `ValidationException`, whatever form it takes — including a token another listing of
    /// this service handed out, whose boundary is a key of a different map.
    ...
    let bad_tokens: [(&str, String); 6] = [
        ("not hex at all", ...),
        ("hex of text that is no ARN", ...),
        ("hex of an ARN of another service", ...),
        ("hex of a gateway ARN of this service", ...),      // wrong resource type
        ("a token a ListGateways page issued", gateway_token),
        ("hex of a hypervisor ARN with an empty id", ...),   // malformed segment
    ];

Every row is either the wrong resource type or malformed; none is a well-formed hypervisor ARN of the listing's own type that was never issued. tests/all/virtual_machine_listing.rs:617-637 has the same seven-row table with the same hole.

## Steps to Reproduce

1. Start the emulator and perform the activation handshake twice (GET / with ?activationRegion=us-east-1), then CreateGateway with each issued key, so two gateways are stored.
2. Call ListGateways with no NextToken and observe the two gateways in ARN order.
3. Call ListGateways with NextToken = hex("arn:aws:backup-gateway:a:0:gateway/0") — a well-formed gateway ARN of this service that was never issued, whose region segment sorts below the stored ARNs.
4. Observe HTTP 200 carrying the FIRST page of gateways again (plus a fresh NextToken), instead of the 400 ValidationException the handler comment and state.rs:1975-1986 promise for a token this service did not issue.
5. Repeat against ListHypervisors with hex("arn:aws:backup-gateway:a:0:hypervisor/0") and ListVirtualMachines with hex("arn:aws:backup-gateway:a:0:vm/0") — both are accepted the same way, since all three share decode_page_token.
