// SPDX-License-Identifier: CC0-1.0

//! How this service classifies and words a refusal, for the rules more than one operation
//! holds a request to.
//!
//! A refusal is decided by WHAT makes the call impossible: what the request itself carries
//! (a value outside the domain a parameter accepts, or members that cannot hold together),
//! the resource a required identifier names not existing, or the addressed resource's own
//! lifecycle state. The sentences and the shared checks live here so the handlers that
//! answer the same refusal cannot drift from each other — the deletion-protection sentence
//! is one wording for the three resources that carry the setting, and the Aurora
//! Serverless v2 capacity rule is one check for the create that provisions a cluster and
//! the modification that rescales it.

use super::*;

/// The DB instance statuses the instance never leaves for a state a request can act in:
/// the one an accepted delete puts it in, which this service's clock follows with the
/// instance's removal ([`DB_INSTANCE_STATUS_DELETING`]). Every other status this service
/// reports is one the instance comes back out of.
pub const TERMINAL_DB_INSTANCE_STATUSES: [&str; 1] = [DB_INSTANCE_STATUS_DELETING];

/// Whether an instance reporting `status` can never leave it; see
/// [`TERMINAL_DB_INSTANCE_STATUSES`].
pub fn is_terminal_db_instance_status(status: &str) -> bool {
    TERMINAL_DB_INSTANCE_STATUSES.contains(&status)
}

/// The sentence `ModifyDBInstance` answers an `InvalidDBInstanceStateFault` with for an
/// instance in `status`.
///
/// Amazon RDS words the refusal by whether the status is one the instance will LEAVE or one
/// it cannot: for a transient status it states the state a modification needs
/// ([`MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE`]), which is what a caller polls on, and for
/// a terminal one ([`TERMINAL_DB_INSTANCE_STATUSES`]) it names the state the instance is in
/// — taken from the instance's own record — and that waiting for it to change is futile.
pub fn modify_db_instance_invalid_state_message(status: &str) -> String {
    if is_terminal_db_instance_status(status) {
        return format!(
            "The specified database instance is currently in {status} state, which is a \
             terminal state."
        );
    }
    MODIFY_DB_INSTANCE_INVALID_STATE_MESSAGE.to_string()
}

/// What a delete is refused with when the resource it names has deletion protection
/// enabled: `subject` is the resource as Amazon RDS names it in this sentence (see
/// [`DELETION_PROTECTED_DB_INSTANCE`] and its siblings).
///
/// Deletion protection is a setting the REQUEST conflicts with rather than a state the
/// resource is passing through — "the database can't be deleted when deletion protection is
/// enabled", and the way out is to turn the setting off and call again, which is exactly
/// what the sentence says. One wording for every resource that carries the setting, so no
/// two deletes can drift.
///
/// It has no variable part beyond the resource KIND. The recorded answers this front door
/// is held to carry exactly `Cannot delete protected Cluster, please disable deletion
/// protection and try again.` for a DB cluster and the same sentence with `DB Instance` for
/// a DB instance — the identifier the request named is NOT in them — so neither the
/// identifier nor anything else of the request or the store is substituted here.
pub fn deletion_protection_refusal_message(subject: &str) -> String {
    format!(
        "Cannot delete protected {subject}, please disable deletion protection and try \
         again."
    )
}

/// How [`deletion_protection_refusal_message`] names a DB instance.
pub const DELETION_PROTECTED_DB_INSTANCE: &str = "DB Instance";

/// How [`deletion_protection_refusal_message`] names a DB cluster.
pub const DELETION_PROTECTED_CLUSTER: &str = "Cluster";

/// How [`deletion_protection_refusal_message`] names a global cluster.
pub const DELETION_PROTECTED_GLOBAL_CLUSTER: &str = "Global Cluster";

/// What a delete that names BOTH of its final-snapshot members is refused with.
///
/// Specifying `FinalDBSnapshotIdentifier` "and also skip the creation of a final DB
/// snapshot with the SkipFinalShapshot parameter" "results in an error": exactly one of the
/// two may be given, which is what the sentence states, naming both members and neither
/// value. One constant for every delete that carries the pair, for the same reason as
/// [`FINAL_SNAPSHOT_MEMBER_REQUIRED`], which answers the other direction of the rule.
pub const ONLY_ONE_FINAL_SNAPSHOT_MEMBER: &str =
    "Only one of SkipFinalSnapshot and FinalDBSnapshotIdentifier can be specified";

/// What a `RebootDBInstance` that enables `ForceFailover` over an instance that is not
/// configured for Multi-AZ is refused with: "Constraint: You can't enable force failover if
/// the instance isn't configured for Multi-AZ."
///
/// Amazon RDS names the member and states the configuration it requires, naming the two
/// deployment options that provide it; it reports neither the instance the request
/// addressed nor the value it carried, so nothing of the request or the store is
/// substituted into it.
pub const FORCE_FAILOVER_REQUIRES_MULTI_AZ_MESSAGE: &str =
    "ForceFailover cannot be specified since the instance is not configured for either \
     MultiAZ or High Availability";

/// What a request naming an `EngineMode` the resolved engine VERSION does not run is
/// refused with.
///
/// Which modes a version runs is the domain of the member for that version — the catalogue
/// publishes it as `SupportedEngineModes` — so a mode outside it is a value the parameter
/// does not accept. Amazon RDS states that the mode the request asked for is not available,
/// naming that mode and nothing else of the request: it is how `serverless` (Aurora
/// Serverless v1) is refused on an engine version that publishes `provisioned` alone.
pub fn unavailable_engine_mode_message(engine_mode: &str) -> String {
    format!("The engine mode {engine_mode} you requested is currently unavailable.")
}

/// Why a `ServerlessV2ScalingConfiguration` a request carries cannot be served, carrying
/// the message naming the rule it broke. The two variants are the two Query API codes the
/// refusal belongs to: a capacity VALUE outside what the member accepts, and a pair of
/// values — or a value and the cluster's engine version — that cannot hold together.
#[derive(Debug, Clone)]
pub enum ServerlessV2Rejection {
    /// A value outside the domain the member accepts — `InvalidParameterValue`.
    Value(String),
    /// Members that cannot hold together — `InvalidParameterCombination`.
    Combination(String),
}

/// Refuse an Aurora Serverless v2 scaling configuration a cluster cannot run, whichever
/// operation carries it: the create that provisions the cluster and the modification that
/// rescales it hold one rule, so neither can drift from the other.
///
/// `capacity` is the Aurora capacity unit range the cluster's engine VERSION publishes
/// (`DescribeDBEngineVersions`' capacity profile), `None` for a version that runs no Aurora
/// Serverless v2 capacity at all; `engine` and `version` name that version in the refusal.
/// A capacity value is checked against the published range and against the half-step
/// increments the member documents ("You can specify ACU values in half-step increments,
/// such as 8, 8.5, 9, and so on"), and the pair against each other (the floor cannot exceed
/// the ceiling).
pub fn check_serverless_v2_scaling(
    engine: &str,
    version: &str,
    capacity: Option<(f64, f64)>,
    min_capacity: Option<f64>,
    max_capacity: Option<f64>,
    seconds_until_auto_pause: Option<i32>,
) -> Result<(), ServerlessV2Rejection> {
    let Some((min_acu, max_acu)) = capacity else {
        return Err(ServerlessV2Rejection::Combination(format!(
            "ServerlessV2ScalingConfiguration is not supported by engine {engine} version \
             {version}: that version runs no Aurora Serverless v2 capacity"
        )));
    };
    for (member, value) in [
        ("MinCapacity", min_capacity),
        ("MaxCapacity", max_capacity),
    ] {
        let Some(value) = value else { continue };
        if !(min_acu..=max_acu).contains(&value) {
            return Err(ServerlessV2Rejection::Value(format!(
                "invalid value {value} for ServerlessV2ScalingConfiguration.{member}: engine \
                 {engine} version {version} serves {min_acu} to {max_acu} Aurora capacity \
                 units"
            )));
        }
        if (value * 2.0).fract() != 0.0 {
            return Err(ServerlessV2Rejection::Value(format!(
                "invalid value {value} for ServerlessV2ScalingConfiguration.{member}: Aurora \
                 capacity units are specified in half-step increments"
            )));
        }
    }
    if let (Some(min), Some(max)) = (min_capacity, max_capacity) {
        if min > max {
            return Err(ServerlessV2Rejection::Combination(format!(
                "ServerlessV2ScalingConfiguration.MinCapacity ({min}) must not exceed \
                 MaxCapacity ({max})"
            )));
        }
    }
    if let Some(seconds) = seconds_until_auto_pause {
        if !SECONDS_UNTIL_AUTO_PAUSE_RANGE.contains(&seconds) {
            return Err(ServerlessV2Rejection::Value(format!(
                "invalid value {seconds} for \
                 ServerlessV2ScalingConfiguration.SecondsUntilAutoPause: must be a value from \
                 {} to {} seconds",
                SECONDS_UNTIL_AUTO_PAUSE_RANGE.start(),
                SECONDS_UNTIL_AUTO_PAUSE_RANGE.end()
            )));
        }
    }
    Ok(())
}
