// SPDX-License-Identifier: CC0-1.0

//! The Lambda alias a deployment moves, written where that alias LIVES.
//!
//! An alias belongs to AWS Lambda, so a traffic shift is written into the Lambda
//! emulator's OWN function store through the shared engine — the very store
//! `CreateAlias` writes, `UpdateAlias` changes and `GetAlias` reads. Nothing about
//! a function or an alias is copied into this service's store, so one alias has
//! one home and a client reads a shift where a client reads an alias.
//!
//! This module is only the WRITE, and the read of what the alias held before it:
//! which state an alias must hold at an instant is decided by
//! [`super::traffic_shift`], and when and for which deployment by
//! [`super::lambda_traffic_shift`]. Call it with this service's own store guard
//! released: the two stores are never locked at once.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_com_amazonaws_lambda_awsgirapiservice as lambda;

/// What one write to an alias CHANGES about it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AliasMove {
    /// The version the alias invokes once the move is done, or `None` to leave it
    /// invoking whatever it invokes now — which is what an INTERMEDIATE traffic
    /// increment does: it puts a share of the traffic on the version being
    /// shifted to and leaves the alias on the version it is shifting from.
    pub function_version: Option<String>,
    /// The weighted routing the alias splits its invocations by once the move is
    /// done; empty clears it, which is what the last increment of a shift does.
    ///
    /// These are the members of the model's `AliasConfiguration` a traffic shift
    /// moves: `FunctionVersion` and `RoutingConfig.AdditionalVersionWeights`.
    pub additional_version_weights: BTreeMap<String, f64>,
}

/// Which Lambda resource an alias move named and AWS Lambda does not hold.
///
/// What the deployment then FAILS for; the sentence each case carries is
/// [`super::lambda_traffic_shift`]'s, which is where a deployment's error
/// information is worded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unresolved {
    /// The function the AppSpec names is not one AWS Lambda holds — including the
    /// case of no Lambda service beside this one at all.
    Function,
    /// The function holds no alias of the name the AppSpec gives.
    Alias,
    /// The function has no such version for traffic to sit on.
    Version,
}

/// Move the alias `alias_name` of `function_name` as `move_to` asks.
///
/// The move is the one `UpdateAlias` performs for the same change: the members it
/// states become what the alias reports, the members it leaves out stay, and the
/// alias's revision id changes, because "a unique identifier that changes when you
/// update the alias" changed on this write.
///
/// What is checked before the write is that the Lambda resources the move names
/// EXIST: the function, the alias that moves, and every version traffic is put on.
/// The AppSpec's `CurrentVersion` is NOT a precondition — an increment is applied
/// to whatever version the alias invokes when it happens — because this emulator
/// has no measurement of what the real service answers for a deployment whose
/// `CurrentVersion` has been overtaken, and inventing a failure for it would
/// report an outcome nothing established.
pub fn move_alias<Eng: Send + Sync + 'static>(
    engine: &Eng,
    function_name: &str,
    alias_name: &str,
    move_to: &AliasMove,
) -> Result<(), Unresolved> {
    // No Lambda beside this service is the case where the function the AppSpec
    // names cannot be resolved at all.
    let Some(store) = lambda::state::service_state(engine) else {
        return Err(Unresolved::Function);
    };
    let reference =
        lambda::state::parse_function_reference(function_name).ok_or(Unresolved::Function)?;
    let mut store = store.lock();
    let Some(record) = store.functions.get_mut(&reference.name) else {
        return Err(Unresolved::Function);
    };
    // The transitions Lambda's own clock already owed that function, so the
    // versions this move is checked against are the ones a client would read.
    record.materialize(SystemTime::now());
    let holds_every_version = move_to
        .function_version
        .iter()
        .chain(move_to.additional_version_weights.keys())
        .all(|version| record.config_of(version).is_some());
    if !holds_every_version {
        return Err(Unresolved::Version);
    }
    let Some(alias) = record.aliases.get_mut(alias_name) else {
        return Err(Unresolved::Alias);
    };
    if let Some(function_version) = move_to.function_version.as_ref() {
        alias.function_version = function_version.clone();
    }
    alias
        .additional_version_weights
        .clone_from(&move_to.additional_version_weights);
    alias.revision_id = lambda::state::mint_revision_id();
    Ok(())
}
