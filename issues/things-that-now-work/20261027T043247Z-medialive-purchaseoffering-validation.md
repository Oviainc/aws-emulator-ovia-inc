Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# PurchaseOffering stores tag keys in the reserved `aws:` namespace, skipping the guard every other tag-carrying create applies

_Filed: 2026-10-27T04:32:47Z_
_Operation: PurchaseOffering | Class: validation_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PurchaseOfferingRequest` carries a `Tags` map, and the handler writes it straight into the new `ReservationRecord` without ever consulting `crate::state::reserved_tag_key_in`. Every other operation in this crate that stores a caller-supplied tag map at create/update time refuses a key in the reserved `aws:` namespace with its own modelled `BadRequestException` first (17 call sites), and the two generic tag writers (`CreateTags` / `DeleteTags`) own the same rule through `TagRefusal::ReservedKeyPrefix`. PurchaseOffering is the one tag-carrying create that bypasses it, so `POST /prod/offerings/{OfferingId}/purchase` with `{"count":1,"tags":{"aws:owner":"x"}}` answers 201 and the reserved key is then observable both in `Reservation$Tags` (DescribeReservation / ListReservations) and through `ListTagsForResource` on the reservation ARN — `tagged_resource` resolves reservation ARNs (state.rs:3965) and `resource_tags` returns the reservation's own map (state.rs:4009-4011).

## Evidence

The crate states the rule for exactly this path — `services/service-com-amazonaws-medialive-medialive/src/state.rs:163-168`:

    //! The two writers own the per-resource tag bounds ([`MAX_RESOURCE_TAGS`],
    //! [`MAX_TAG_KEY_LEN`], [`MAX_TAG_VALUE_LEN`]) and the reserved key namespace
    //! ([`RESERVED_TAG_KEY_PREFIX`]), which is why the map itself is only ever
    //! handed out mutably inside this module. An operation that stores a tag map a
    //! request carried without going through them (a create, an update) refuses the
    //! reserved namespace with [`reserved_tag_key_in`] before it writes.

The sibling that obeys it — `src/ops/create_channel.rs:183-186`:

        if let Some(key) = crate::state::reserved_tag_key_in(input.tags.as_ref()) {
            return Err(CreateChannelErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::reserved_tag_key_message(key)),

(the identical block appears in create_input.rs:182, create_input_security_group.rs:156, create_multiplex.rs:163, create_network.rs:178, create_node.rs:189, create_cluster.rs:186, create_channel_placement_group.rs:186, create_sdi_source.rs:170, create_signal_map.rs:187, create_partner_input.rs:149, create_cloud_watch_alarm_template.rs:192, create_cloud_watch_alarm_template_group.rs:160, create_event_bridge_rule_template.rs:178, create_event_bridge_rule_template_group.rs:160, update_input_security_group.rs:167)

The path that skips it — `src/ops/purchase_offering.rs:250-261`:

        let id = state
            .insert_reservation(
                &region,
                crate::state::NewReservation {
                    offering_index,
                    count: input.count,
                    name: input.name,
                    start,
                    renewal_settings: input.renewal_settings,
                    tags: input.tags.unwrap_or_default(),
                },

`purchase_offering.rs` contains no occurrence of `reserved_tag_key_in` at all (grep over `src/` lists 17 call sites, none in this file).

Test coverage: `tests/all/reserved_tag_namespace.rs:54-62` enumerates the creates it exercises — "One create per resource kind whose create carries `Tags` … the nested creates (a node, the two monitor templates, a signal map, a partner input) carry the same `Tags` member through the same guard" — and the reservation is in neither list. `tests/all/reservation_purchase.rs:834` only purchases with the ordinary key `{"owner": "finance"}`, so nothing pins the current acceptance.

## Steps to Reproduce

1. GET /prod/offerings to pick an OfferingId from the catalogue
2. POST /prod/offerings/{OfferingId}/purchase with body {"count": 1, "tags": {"aws:internal": "x"}}
3. Observe 201 and `reservation.tags` containing `aws:internal` (every sibling create answers 400 BadRequestException with the reserved-prefix message for the same key)
4. GET /prod/reservations/{ReservationId} and GET /prod/tags/{url-encoded reservation ARN} — the reserved key is reported by both
