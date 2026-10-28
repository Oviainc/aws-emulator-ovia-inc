// SPDX-License-Identifier: CC0-1.0

//! The node REGISTRATION SCRIPT slice — the artifact
//! `CreateNodeRegistrationScript` issues for one cluster.
//!
//! One file for the whole slice — the record, its bound, its refusals, the
//! store's methods and the rendering — so the slice can be read, and merged, on
//! its own, the same way the node slice beside it is laid out.
//!
//! A registration script is NOT a node. "Create the Register Node script for
//! all the nodes intended for a specific Cluster. You will then run the script
//! on each hardware unit that is intended for that Cluster. The script creates a
//! Node in the specified Cluster", so ONE script registers as many nodes as the
//! Cluster has hardware units and the call that issues it creates none of them:
//! `CreateNode` is what puts a node in the store, and the script is what makes a
//! hardware unit run that call for itself. That is why this slice keeps scripts
//! rather than nodes, and why the operation's response carries no node.
//!
//! What a script is issued FOR has to be in the store: a script registers nodes
//! "in the specified Cluster", so the cluster is resolved here, and a
//! re-registration script names a node the cluster still holds ("If you're
//! generating a re-registration script for an already existing node, this is
//! where you provide the id").
//!
//! Each issued script is KEPT ([`NodeRegistrationScriptRecord`]), under an id
//! this slice mints, so a retried request — the same `@idempotencyToken`
//! `RequestId`, presented to the same cluster's endpoint — answers with the
//! script the first attempt issued rather than with a second script that would
//! register a second node per unit. The store is a ring bounded by
//! [`MAX_NODE_REGISTRATION_SCRIPTS`]: the operation models no resource to delete
//! a script through, so the oldest is dropped rather than letting the emulator's
//! heap grow without bound, and a token whose script has been dropped that way
//! issues a fresh one.
//!
//! The script's TEXT is composed here ([`rendered_registration_script`]), out of
//! the cluster and node ids the store holds and the parameters the request
//! carried, and it is the STORED text every response reports. The real service
//! publishes no specification of its own script's contents, so what this
//! emulator issues is a POSIX shell script built from this service's own public
//! operations — `create-node` for a unit that has no node yet, or `update-node`
//! for the node a re-registration names, and `update-node-state` to activate it
//! — parameterised exactly as the request asked. Nothing in it is invented: a
//! parameter the request did not carry is left out of the script, and every
//! value it does carry is shell-quoted ([`shell_quoted`]), so a name or an
//! interface name containing a quote cannot change what the script runs.

use serde_json::{Map, Value};

use crate::ops::types::NodeInterfaceMapping;

use super::{ComAmazonawsMedialiveMedialiveState, MAX_NODE_INTERFACE_MAPPINGS};

/// The id the first registration script of a process gets.
///
/// MediaLive resource ids are numeric strings, so they are minted from a counter
/// at a fixed eight-digit width, which keeps the `BTreeMap` key order equal to
/// the mint order — that is what lets the ring drop the OLDEST script. A range of
/// its own keeps the service's id spaces apart, so an id minted here never
/// collides with a node's or a cluster's.
const FIRST_NODE_REGISTRATION_SCRIPT_ID: u64 = 11_000_000;

/// The most registration scripts this emulator keeps at once.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a caller that
/// issues scripts in a loop cannot grow the emulator's heap without bound. The
/// store is a RING rather than a wall — no operation deletes a script, so
/// refusing once it is full would make the operation fail for good — and the
/// only visible consequence of an eviction is that a `RequestId` whose script
/// has been dropped issues a fresh script instead of answering with the old one.
pub const MAX_NODE_REGISTRATION_SCRIPTS: usize = 100;

/// The most `RequestId` entries kept for `CreateNodeRegistrationScript`
/// idempotency — the same bound, for the same reason, as
/// [`super::MAX_NODE_TOKENS`] on nodes.
pub const MAX_NODE_REGISTRATION_SCRIPT_TOKENS: usize = MAX_NODE_REGISTRATION_SCRIPTS;

/// Why [`ComAmazonawsMedialiveMedialiveState::issue_node_registration_script`]
/// issued nothing. Each variant leaves the store exactly as it was.
#[derive(Debug)]
pub enum NodeRegistrationScriptRefusal {
    /// The account holds no cluster with the id the request was addressed to, so
    /// there is no cluster for the script to register nodes in.
    ClusterNotFound,
    /// The request asked for a re-registration script for a node the cluster
    /// does not hold — an id no node ever had, or one presented under another
    /// cluster. Carries the id it named.
    NodeNotFound(String),
    /// The request asked for a re-registration script for a node whose
    /// deregistration has already been accepted, which is on its way out of the
    /// account and so is nothing a hardware unit can register as. Carries the id
    /// it named.
    NodeDeleteAccepted(String),
    /// The request listed more interface mappings than
    /// [`MAX_NODE_INTERFACE_MAPPINGS`], the ceiling `CreateNode` stores a node's
    /// mappings under. Carries the count it asked for, so the refusal can name
    /// it.
    TooManyInterfaceMappings(usize),
}

/// What one `CreateNodeRegistrationScript` request asks for: the members it
/// carried, beside the `ClusterId` its URI named.
///
/// One struct rather than four positional parameters on
/// [`ComAmazonawsMedialiveMedialiveState::issue_node_registration_script`], so a
/// handler cannot pass the name where the role belongs.
#[derive(Debug, Clone, Default)]
pub struct NewNodeRegistrationScript {
    /// The `Id` of the node a RE-registration script is for; `None` for a script
    /// that registers units that have no node yet.
    pub node_id: Option<String>,
    /// The `Name` the request carried — a PATTERN, not a name: "Specify a
    /// pattern for MediaLive Anywhere to use to assign a name to each Node in
    /// the Cluster. The pattern can include the variables $hn (hostname of the
    /// node hardware) and $ts for the date and time that the Node is created, in
    /// UTC", so it is the running script that turns it into one name per unit.
    pub name: Option<String>,
    /// The `NodeRole` the request named, carried into the script as the role each
    /// node it registers starts in.
    pub role: Option<String>,
    /// The interface mappings the request listed, in the order it listed them.
    pub node_interface_mappings: Vec<NodeInterfaceMapping>,
}

/// One issued registration script, as the store keeps it.
///
/// The record is the SCRIPT and what it was issued for; the response member
/// (`NodeRegistrationScript`) is [`Self::script`] read straight back out, so a
/// retry and the first attempt cannot report one request's script two ways.
#[derive(Debug, Clone)]
pub struct NodeRegistrationScriptRecord {
    /// The id this slice minted for the script. Not a resource id the API
    /// reports — the operation's response carries none — it is what the
    /// idempotency map points at and what the script itself uses to make the
    /// `CreateNode` it runs idempotent per hardware unit.
    pub id: String,
    /// The cluster the script registers nodes in.
    pub cluster_id: String,
    /// The node a re-registration script is for, when it is one.
    pub node_id: Option<String>,
    /// The script, rendered once when it was issued.
    pub script: String,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// The next registration script id, as AWS shapes MediaLive resource ids: a
    /// numeric string.
    fn mint_node_registration_script_id(&mut self) -> String {
        (FIRST_NODE_REGISTRATION_SCRIPT_ID
            + self.mints.next(|mints| &mut mints.node_registration_script))
            .to_string()
    }

    /// Issue one registration script for the cluster `cluster_id`, store it and
    /// answer the id it was filed under — or refuse.
    ///
    /// The cluster the script registers nodes in, the node a re-registration
    /// names, the bound on the caller-chosen mapping list, the mint, the
    /// rendering and the insert live together here, so a handler cannot check one
    /// of them and issue a script past another; the handler's job is only to turn
    /// a [`NodeRegistrationScriptRefusal`] into the operation's own modelled
    /// error.
    ///
    /// Which STATE the cluster is in is not a precondition, for the same reason
    /// [`ComAmazonawsMedialiveMedialiveState::insert_node`] does not check it: a
    /// cluster that has not finished coming up takes nodes exactly as an active
    /// one does, and it is the node that has to register before anything encodes.
    pub fn issue_node_registration_script(
        &mut self,
        region: &str,
        cluster_id: &str,
        requested: NewNodeRegistrationScript,
    ) -> Result<String, NodeRegistrationScriptRefusal> {
        // Resolved through the LIVE cluster, like every other write addressed to
        // one: a cluster whose delete has been accepted is leaving the account,
        // so there is no endpoint for a new node of it to register against.
        if self.live_cluster(cluster_id).is_none() {
            return Err(NodeRegistrationScriptRefusal::ClusterNotFound);
        }
        if let Some(node_id) = requested.node_id.as_deref() {
            let node = self
                .node(cluster_id, node_id)
                .ok_or_else(|| NodeRegistrationScriptRefusal::NodeNotFound(node_id.to_string()))?;
            if node.is_delete_accepted() {
                return Err(NodeRegistrationScriptRefusal::NodeDeleteAccepted(
                    node_id.to_string(),
                ));
            }
        }
        if requested.node_interface_mappings.len() > MAX_NODE_INTERFACE_MAPPINGS {
            return Err(NodeRegistrationScriptRefusal::TooManyInterfaceMappings(
                requested.node_interface_mappings.len(),
            ));
        }

        // The ring is trimmed before the insert, so the store never holds more
        // than the bound. The ids are minted from a counter at a fixed width, so
        // the map's first key is the script issued longest ago.
        while self.node_registration_scripts.len() >= MAX_NODE_REGISTRATION_SCRIPTS {
            if self.node_registration_scripts.pop_first().is_none() {
                break;
            }
        }

        let id = self.mint_node_registration_script_id();
        let script = rendered_registration_script(&ScriptParameters {
            registration_id: &id,
            region,
            cluster_id,
            node_id: requested.node_id.as_deref(),
            name: requested.name.as_deref(),
            role: requested.role.as_deref(),
            interface_mappings: &requested.node_interface_mappings,
        });
        self.node_registration_scripts.insert(
            id.clone(),
            NodeRegistrationScriptRecord {
                id: id.clone(),
                cluster_id: cluster_id.to_string(),
                node_id: requested.node_id,
                script,
            },
        );
        Ok(id)
    }

    /// The script the store filed under `id` for the cluster `cluster_id`, or
    /// `None` when it holds no such script.
    ///
    /// The cluster has to match: a script registers nodes in ONE cluster, so an
    /// id presented under another names no script a caller of that cluster's
    /// endpoint may read.
    pub fn node_registration_script(&self, cluster_id: &str, id: &str) -> Option<&str> {
        self.node_registration_scripts
            .get(id)
            .filter(|record| record.cluster_id == cluster_id)
            .map(|record| record.script.as_str())
    }

    /// Record `token` as having issued the script `id` for `cluster_id`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_NODE_REGISTRATION_SCRIPT_TOKENS`].
    pub fn record_node_registration_script_token(
        &mut self,
        cluster_id: &str,
        token: String,
        id: &str,
    ) {
        self.node_registration_script_tokens.record(
            node_registration_script_token_key(cluster_id, &token),
            id,
            MAX_NODE_REGISTRATION_SCRIPT_TOKENS,
        );
    }

    /// The script a stored `RequestId` issued for `cluster_id`, if the ring still
    /// holds it.
    ///
    /// The cluster is part of the key AND re-checked on the record, exactly as
    /// the node slice re-checks it: a token presented to one cluster's endpoint
    /// says nothing about another's.
    pub fn node_registration_script_for_token(
        &self,
        cluster_id: &str,
        request_id: &str,
    ) -> Option<&str> {
        let id = self
            .node_registration_script_tokens
            .created(&node_registration_script_token_key(cluster_id, request_id))?;
        self.node_registration_script(cluster_id, id)
    }
}

/// The key the registration script token map records a token against.
///
/// A script is issued against ONE cluster's endpoint (`POST
/// /prod/clusters/{ClusterId}/nodeRegistrationScript`), so the cluster is part of
/// the key: two clusters using the same `RequestId` each keep their own entry
/// instead of overwriting one another's. The cluster is re-checked on the record
/// the token resolves to
/// ([`ComAmazonawsMedialiveMedialiveState::node_registration_script_for_token`]),
/// so the key's shape carries no invariant of its own.
fn node_registration_script_token_key(cluster_id: &str, token: &str) -> String {
    format!("{cluster_id}/{token}")
}

/// Everything one rendered script carries, resolved out of the store and the
/// request before any of it is written.
struct ScriptParameters<'a> {
    registration_id: &'a str,
    /// The region the request was signed for, which is the region the cluster and
    /// its nodes live in, so the script addresses that one.
    region: &'a str,
    cluster_id: &'a str,
    /// The node a re-registration script is for; `None` for a script that
    /// registers units that have no node yet.
    node_id: Option<&'a str>,
    name: Option<&'a str>,
    role: Option<&'a str>,
    interface_mappings: &'a [NodeInterfaceMapping],
}

/// One registration script, rendered from `params`.
///
/// Two shapes, which is the one difference the request's `Id` makes: a script
/// for a unit that has no node yet CREATES the node (the name pattern, the role
/// and the interface mappings the request carried are that create's arguments),
/// while a re-registration script names the node it is for and only applies the
/// settings the request asked to change. Both end by activating the node, which
/// is what the operation documents the script does ("It then binds the Node to
/// this hardware unit, and activates the node hardware for use with MediaLive
/// Anywhere").
///
/// The arguments are named as the parameters they come from: the `--request-id`
/// the create runs with is this script's own id and the unit's hostname, so
/// running one script on many units registers one node per unit, and re-running
/// it on the same unit registers that unit's node again rather than a second one.
fn rendered_registration_script(params: &ScriptParameters<'_>) -> String {
    use std::fmt::Write as _;

    let mut script = String::from("#!/bin/sh\n");
    // Infallible: a `String` is the one `fmt::Write` that cannot fail, and none
    // of the formats here can raise an error of its own.
    let _ = writeln!(
        script,
        "# MediaLive Anywhere node registration script for cluster {}.",
        params.cluster_id
    );
    script.push_str(
        "#\n\
         # Run this script on each hardware unit that is to run as a node of that\n\
         # cluster. It registers the unit with MediaLive Anywhere and activates the\n\
         # node for encoding.\n\
         set -eu\n\
         \n",
    );

    push_assignment(&mut script, "AWS_DEFAULT_REGION", params.region);
    script.push_str("export AWS_DEFAULT_REGION\n");
    push_assignment(&mut script, "CLUSTER_ID", params.cluster_id);
    push_assignment(&mut script, "REGISTRATION_ID", params.registration_id);
    if let Some(node_id) = params.node_id {
        push_assignment(&mut script, "NODE_ID", node_id);
    }
    if let Some(name) = params.name {
        push_assignment(&mut script, "NODE_NAME_PATTERN", name);
    }
    if let Some(role) = params.role {
        push_assignment(&mut script, "NODE_ROLE", role);
    }
    // A node's interface mappings are recorded by the create, and a
    // re-registration runs none, so they are part of a script that registers a
    // unit that has no node yet and left out of one that does not.
    let mappings = (params.node_id.is_none() && !params.interface_mappings.is_empty())
        .then(|| interface_mappings_argument(params.interface_mappings));
    if let Some(mappings) = mappings.as_deref() {
        push_assignment(&mut script, "NODE_INTERFACE_MAPPINGS", mappings);
    }

    // The hostname is what tells one unit's registration from another's, so
    // every script that runs a create reads it; a script carrying a name pattern
    // reads the clock beside it, because those are the two variables the pattern
    // names.
    if params.node_id.is_none() || params.name.is_some() {
        script.push_str("\nhn=\"$(hostname)\"\n");
    }
    if params.name.is_some() {
        script.push_str(
            r#"ts="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
NODE_NAME="$(printf '%s\n' "$NODE_NAME_PATTERN" | sed -e 's/[$]hn/'"$hn"'/g' -e 's/[$]ts/'"$ts"'/g')"
"#,
        );
    }

    match params.node_id {
        None => {
            let mut arguments = vec![
                "--cluster-id \"$CLUSTER_ID\"",
                "--request-id \"$REGISTRATION_ID-$hn\"",
            ];
            if params.name.is_some() {
                arguments.push("--name \"$NODE_NAME\"");
            }
            if params.role.is_some() {
                arguments.push("--role \"$NODE_ROLE\"");
            }
            if mappings.is_some() {
                arguments.push("--node-interface-mappings \"$NODE_INTERFACE_MAPPINGS\"");
            }
            arguments.push("--query Id --output text");
            let _ = writeln!(
                script,
                "\nNODE_ID=\"$({})\"",
                invocation("create-node", &arguments)
            );
        }
        Some(_) => {
            let mut arguments = vec!["--cluster-id \"$CLUSTER_ID\"", "--node-id \"$NODE_ID\""];
            if params.name.is_some() {
                arguments.push("--name \"$NODE_NAME\"");
            }
            if params.role.is_some() {
                arguments.push("--role \"$NODE_ROLE\"");
            }
            // Nothing to apply when the request asked for neither of the two
            // settings a re-registration can carry: the script then only
            // activates the node it names.
            if arguments.len() > 2 {
                let _ = writeln!(script, "\n{}", invocation("update-node", &arguments));
            }
        }
    }

    let _ = writeln!(
        script,
        "\n{}",
        invocation(
            "update-node-state",
            &[
                "--cluster-id \"$CLUSTER_ID\"",
                "--node-id \"$NODE_ID\"",
                "--state ACTIVE",
            ],
        )
    );
    script
}

/// One `VARIABLE='value'` line, with the value shell-quoted.
fn push_assignment(script: &mut String, variable: &str, value: &str) {
    script.push_str(variable);
    script.push('=');
    script.push_str(&shell_quoted(value));
    script.push('\n');
}

/// One `aws medialive <operation>` invocation, one argument per line.
fn invocation(operation: &str, arguments: &[&str]) -> String {
    let mut rendered = format!("aws medialive {operation}");
    for argument in arguments {
        rendered.push_str(" \\\n  ");
        rendered.push_str(argument);
    }
    rendered
}

/// One value as a single shell WORD: wrapped in single quotes, which quote every
/// character of a POSIX shell word except the single quote itself, so an embedded
/// one is spliced in as `'\''` — closing the quoted run, escaping the quote and
/// opening the next.
///
/// Every value the script carries goes through this, so a name, a role or an
/// interface name containing a quote, a newline or a `$` is DATA in the script
/// rather than something it runs.
fn shell_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The request's interface mappings as the `--node-interface-mappings` argument
/// of the create the script runs: a JSON list, built with the JSON library rather
/// than by hand.
///
/// The keys are the MEMBER names the model declares for
/// `NodeInterfaceMappingCreateRequest` (`smithy_query(query="shape",
/// shape="NodeInterfaceMappingCreateRequest")`), which is how the command line
/// spells them. `PhysicalInterfaceIpAddresses` — the one member the request's own
/// `NodeInterfaceMapping` shape carries that the create's does not — is left out,
/// because the real service learns a node's addresses from the hardware when it
/// registers rather than taking them from a create.
fn interface_mappings_argument(mappings: &[NodeInterfaceMapping]) -> String {
    let entries: Vec<Value> = mappings
        .iter()
        .map(|mapping| {
            let mut entry = Map::new();
            for (member, value) in [
                ("LogicalInterfaceName", mapping.logical_interface_name.as_ref()),
                ("NetworkInterfaceMode", mapping.network_interface_mode.as_ref()),
                ("PhysicalInterfaceName", mapping.physical_interface_name.as_ref()),
            ] {
                if let Some(value) = value {
                    entry.insert(member.to_string(), Value::String(value.clone()));
                }
            }
            Value::Object(entry)
        })
        .collect();
    Value::Array(entries).to_string()
}
