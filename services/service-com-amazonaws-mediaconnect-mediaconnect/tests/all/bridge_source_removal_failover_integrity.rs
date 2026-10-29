// SPDX-License-Identifier: CC0-1.0

//! A source failover priority an UPDATE wrote binds the removal rule exactly as one a create
//! wrote, end to end over HTTP.
//!
//! The rule is the one `crate::remove_source_keeps_failover_resolvable` states and whose
//! helpers this suite reuses: a stored `SourceFailoverConfig` names its primary source by NAME
//! (`SourcePriority$PrimarySource`, "the name of the source you choose as the primary source"
//! — `smithy_query(query="doc",shape="SourcePriority",member="PrimarySource")`), so it is a
//! reference to one of its resource's own sources, and a call that would leave it naming a
//! source the resource has not got is refused with the modelled semantic-error exception
//! (`BadRequestException`, HTTP 400) and writes nothing.
//!
//! What this suite adds is the axis that rule is indifferent to and the table there does not
//! vary: WHICH call wrote the stored priority. There the priority is always the one CreateBridge
//! stored; here the bridge is created carrying none and UpdateBridge writes it afterwards
//! (`UpdateFailoverConfig$SourcePriority`, merged into the stored setting), over both kinds of
//! source a bridge's direction admits. So the removal refusal follows the STORED reference
//! rather than the call that happened to put it there.
//!
//! Every call travels the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what is asserted is what a MediaConnect client sees
//! on the wire.

use serde_json::json;

use crate::bridge_lifecycle::{create_bridge, gateway_with_networks};
use crate::flow_lifecycle::Emulator;
use crate::remove_source_keeps_failover_resolvable::{
    assert_failover_is_resolvable, assert_refused, bridge_state, failover, primary_of, Kind,
};

/// The names the bridges here address their pair of sources by.
const NAMES: [&str; 2] = ["src-a", "src-b"];

/// RemoveBridgeSource refuses to take away the source the priority an UpdateBridge wrote names,
/// and removes the other source of the pair — for a bridge holding network sources and for one
/// holding flow sources.
#[tokio::test]
async fn a_priority_written_by_update_bridge_binds_the_removal_rule_too() {
    let emulator = Emulator::new();
    let placement =
        gateway_with_networks(&emulator, "gw-failover-update-removal", &["net-1"]).await;

    for (index, kind) in [Kind::Network, Kind::Flow].into_iter().enumerate() {
        let case = format!("{kind:?} sources, the priority written by UpdateBridge");
        let case = case.as_str();

        let mut sources = Vec::with_capacity(NAMES.len());
        for (position, name) in NAMES.iter().enumerate() {
            sources.push(kind.source(&emulator, index, position, name).await);
        }
        let (type_member, type_configuration) = kind.bridge_type();
        let mut request = json!({
            "name": format!("bridge-failover-update-removal-{index}"),
            "placementArn": placement,
            "sources": sources,
            // A bridge holds a pair of sources only while its failover state is active ("if
            // the state is inactive, [it] can have only one source. If the state is active,
            // [it] can have one or two sources" — `FailoverConfig$State`), so the create
            // states that, and NO priority: writing the priority is the update's job here.
            "sourceFailoverConfig": failover(None),
        });
        request[type_member] = type_configuration;
        let (status, code, created) = create_bridge(&emulator, request).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: the bridge under test must be created, got {status} {code:?}: \
             {created}"
        );
        let bridge_arn = created["bridge"]["bridgeArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();
        let uri = format!("/v1/bridges/{bridge_arn}");

        // Before any priority is written the bridge holds the pair, in the order the create
        // named them: the state the update below points the priority into.
        let (_, before) = bridge_state(&emulator, case, &bridge_arn).await;
        assert_eq!(
            before,
            NAMES.map(str::to_string).to_vec(),
            "case {case:?}: the bridge under test holds the pair of sources, got {before:?}"
        );

        // The write the axis is about: the priority is stored by the UPDATE path.
        let (status, code, updated) = emulator
            .send(
                "PUT",
                &uri,
                Some(json!({ "sourceFailoverConfig": { "sourcePriority": {
                    "primarySource": NAMES[1],
                }}})),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: the update may point the priority at a source the bridge holds, got \
             {status} {code:?}: {updated}"
        );
        let (stored, held) = bridge_state(&emulator, case, &bridge_arn).await;
        assert_eq!(
            primary_of(&stored).as_deref(),
            Some(NAMES[1]),
            "case {case:?}: the bridge reports the primary source the update named, got {stored}"
        );

        // The source that priority now names cannot be taken away, exactly as if the create had
        // written the same priority.
        let (status, code, answered) = emulator
            .send("DELETE", &format!("{uri}/sources/{}", NAMES[1]), None)
            .await;
        assert_refused(case, NAMES[1], status, code.as_deref(), &answered);
        let (after_refusal, remaining) = bridge_state(&emulator, case, &bridge_arn).await;
        assert_eq!(
            (remaining, after_refusal.clone()),
            (held, stored.clone()),
            "case {case:?}: a refused remove changes neither the bridge's sources nor its \
             failover setting, got {after_refusal}"
        );

        // The other source of the pair, which the stored priority does not name, is removable.
        let (status, code, removed) = emulator
            .send("DELETE", &format!("{uri}/sources/{}", NAMES[0]), None)
            .await;
        assert_eq!(
            (status, code.as_deref(), removed["sourceName"].as_str()),
            (202, None, Some(NAMES[0])),
            "case {case:?}: the source the stored priority does not name is removable, got \
             {status} {code:?}: {removed}"
        );
        let (config, names) = bridge_state(&emulator, case, &bridge_arn).await;
        assert_eq!(
            (names.clone(), primary_of(&config)),
            (vec![NAMES[1].to_string()], Some(NAMES[1].to_string())),
            "case {case:?}: the bridge keeps the source its priority names, got {names:?} and \
             {config}"
        );
        assert_failover_is_resolvable(&emulator, case, &uri, &config, &names).await;
    }
}
