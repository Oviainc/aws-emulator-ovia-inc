Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 550c195752df5dc4e392cf21e5aee43bca093d20

# ARN-addressed tag operations resolve resourceArn by exact string match, so the `tunnel:<id>` ARN form the crate's own parse accepts reports an existing tunnel as nonexistent

_Filed: 2026-11-04T09:31:52Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `550c195752df5dc4e392cf21e5aee43bca093d20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three ARN-addressed operations (`ListTagsForResource`, `TagResource`, `UntagResource`) resolve `resourceArn` to a tunnel with `state::tunnel_id_for_arn`, which is a byte-for-byte lookup in `arn_index`. That index is populated only with the ARN this service minted, which `state::tunnel_arn` always renders in the slash form `.../tunnel/<id>`. So no other spelling of the same resource ARN ever resolves.

The crate ALREADY has the parse that resolves an ARN to a tunnel id — `state::tunnel_id_from_arn` — and the same three handlers already call it, but only on the error path (through `state::tunnel_not_found_message_for_arn`). Its own documentation states that it reads "the resource in both of the `<type>/<id>` and `<type>:<id>` forms AWS writes one in", and the shared parse it delegates to says the same. The result is that for a live tunnel whose id is `abc-123`, a request carrying `arn:aws:iot:eu-west-1:000000000000:tunnel:abc-123` is refused with `ResourceNotFoundException` and the message `"Tunnel abc-123 does not exist."` — a message built by lifting the id out of that very ARN, i.e. the handler proves it knows which existing tunnel the ARN names and then reports it as missing. `DescribeTunnel` with `tunnelId: "abc-123"` answers 200 against the same store and the same Region in the same breath.

So one path of this crate (the message) treats the colon form as addressing a tunnel, while the other path (the resolution) does not: `TagResource` silently stores none of its tags, `UntagResource` removes none of its keys, and `ListTagsForResource` reports no tags, for a resource the crate can resolve with a helper it already owns.

## Evidence

The resolver — an exact-string index lookup, with no ARN parse at all, `services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/state.rs:507-514`:

    /// The id of the tunnel `arn` names, if `region`'s endpoint holds one.
    ///
    /// An ARN names the Region of the resource it addresses, so an ARN minted in
    /// another Region resolves to nothing here, exactly as the tunnel it names is
    /// unreachable through this endpoint.
    pub fn tunnel_id_for_arn(&self, region: &str, arn: &str) -> Option<String> {
        self.arn_index.get(&tunnel_key(region, arn)).cloned()
    }

The only thing ever put in that index is the slash-form ARN this service mints, `src/state.rs:414-421` and `src/state.rs:760-767`:

    pub fn insert_tunnel(&mut self, region: &str, tunnel: StoredTunnel) {
        self.arn_index.insert(
            tunnel_key(region, &tunnel.tunnel_arn),
            tunnel.tunnel_id.clone(),
        );

    /// The ARN of the tunnel `tunnel_id` in `region`.
    pub fn tunnel_arn(region: &str, tunnel_id: &str) -> String {
        ::engine::emulator_arn(
            TUNNEL_ARN_SERVICE,
            region,
            &format!("{TUNNEL_ARN_RESOURCE_TYPE}/{tunnel_id}"),
        )
    }

The other side of the inconsistency — the parse this crate owns, which the SAME handlers call for the message, `src/state.rs:769-791`:

    /// The `tunnelId` a resource ARN names, or `None` when `arn` is not of the form
    /// `arn:<partition>:iot:<region>:<account>:tunnel/<tunnelId>`.
    ...
    /// The ARN itself is read through `service_common::arn` — the workspace's ONE
    /// caller-supplied-ARN parse — which decides the `arn` marker, the partition and
    /// the [`TUNNEL_ARN_SERVICE`] namespace a tunnel is addressed in, and reads the
    /// resource in both of the `<type>/<id>` and `<type>:<id>` forms AWS writes one
    /// in.
    pub fn tunnel_id_from_arn(arn: &str) -> Option<&str> {
        let parsed = ::service_common::arn::parse_of(arn, TUNNEL_ARN_SERVICE)?;

and the parse it delegates to, `services/common/src/arn.rs:134-135` and `:150-153`:

    /// AWS writes a resource in either form — `<type>/<id>` and `<type>:<id>` — so both
    /// separators end the type.
    ...
    pub fn resource_id_of_type<'a>(value: &'a str, resource_type: &str) -> Option<&'a str> {
        let rest = resource(value)?.strip_prefix(resource_type)?;
        rest.strip_prefix('/').or_else(|| rest.strip_prefix(':'))
    }

The handler that puts the two together — refuses with the exact-match resolver, then names the tunnel with the parse, `src/ops/list_tags_for_resource.rs:148-162`:

        if store
            .tunnel_id_for_arn(&region, &input.resource_arn)
            .is_none()
        {
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    // The tunnel the ARN names, named the way every other
                    // operation of this service names it.
                    message: Some(crate::state::tunnel_not_found_message_for_arn(
                        &input.resource_arn,
                    )),

(`src/ops/tag_resource.rs:159-170` and `src/ops/untag_resource.rs:145-156` do the same.)

Test coverage: `tests/all/arn_identifier_lifting.rs:174-180` is the ONLY row exercising the colon form, and it uses an id that names no tunnel, so it pins nothing about a live tunnel:

        (
            // `service_common::arn` reads a resource in both of the forms AWS
            // writes one in, `<type>/<id>` and `<type>:<id>`.
            "the resource written in the colon form",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel:abc-123"),
            Names::LiftedId("abc-123".to_string()),
        ),

`a_string_that_addresses_the_live_tunnel_tags_it_round_trip` (same file, lines 314-350) round-trips only the minted slash-form ARN, and `every_minted_arn_is_one_the_arn_addressed_operations_can_read_back` (lines 352-401) likewise only replays the minted string, so the colon form of a LIVE tunnel's ARN is uncovered by the suite.

## Steps to Reproduce

1. Mount the service on an `InMemoryEngine` and sign requests for one Region, as `tests/all/arn_identifier_lifting.rs` does.
2. Call `OpenTunnel` with `{"destinationConfig":{"thingName":"thing-1","services":["SSH"]}}` and note the returned `tunnelId` and `tunnelArn` (e.g. `arn:aws:iot:eu-west-1:000000000000:tunnel/<id>`).
3. Call `TagResource` with `{"resourceArn": "arn:aws:iot:eu-west-1:000000000000:tunnel:<id>", "tags":[{"key":"env","value":"test"}]}` — the same ARN with the resource written in the `<type>:<id>` form the crate's own `tunnel_id_from_arn`/`service_common::arn` documents as a form AWS writes.
4. Observe `ResourceNotFoundException` with `message: "Tunnel <id> does not exist."` — the message was built by lifting `<id>` out of that ARN, so the handler resolved the ARN to the tunnel for the message while refusing to resolve it for the work.
5. Call `DescribeTunnel` with `{"tunnelId": "<id>"}` in the same Region: HTTP 200, the tunnel exists. Repeat step 3 with `ListTagsForResource` (`{"resourceArn": "...tunnel:<id>"}`) and `UntagResource` (`{"resourceArn": "...tunnel:<id>", "tagKeys":["env"]}`) for the same refusal.
