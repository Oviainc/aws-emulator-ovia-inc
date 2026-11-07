// SPDX-License-Identifier: CC0-1.0

//! The TRAFFIC SHIFT an Lambda deployment performs: the alias move its revision
//! describes, carried out in AWS Lambda itself.
//!
//! A deployment of the `Lambda` compute platform deploys a revision that IS the
//! AppSpec file — "the content includes a Lambda function name, the alias for its
//! original version, and the alias for its replacement version. The deployment
//! shifts traffic from the original version of the Lambda function to the
//! replacement version"
//! (`smithy_query(query="shape",shape="AppSpecContent")`). So the shift, not the
//! status, is what such a deployment DOES: the terminal `Succeeded`
//! (`smithy_query(query="shape",shape="DeploymentStatus")`) says the shift the
//! revision described has happened, which for the all-at-once traffic routing is
//! the whole of it — the alias invokes the AppSpec's `TargetVersion` and splits
//! its invocations with nothing.
//!
//! This module is the one place that shift is read and applied:
//!
//! * [`declared_shift`] reads it out of the revision ONCE, when the deployment is
//!   recorded, so no later request re-parses caller-supplied content;
//! * [`apply_due_shifts`] applies it at the instant the clock owes the deployment
//!   its completed traffic routing ([`Deployment::traffic_routing_complete_at_ms`]),
//!   BEFORE the clock may give the deployment a successful outcome
//!   ([`super::with_state`]), and fails the deployment when the shift cannot be
//!   carried out — a deployment that moved no alias never reports success for one.
//!
//! The alias belongs to AWS Lambda, so the move is written into the Lambda
//! emulator's OWN function store through the shared engine — the very store
//! `CreateAlias` writes, `UpdateAlias` changes and `GetAlias` reads. Nothing about
//! a function or an alias is copied into this service's store, so one alias has one
//! home and a client reads the shift where a client reads an alias.
//!
//! Like [`super::alarm_gate`], it runs with THIS store unlocked while the other
//! service's store is locked: the two are never held at once.

use std::time::SystemTime;

use ::service_com_amazonaws_lambda_awsgirapiservice as lambda;
use ::yaml_rust2::Yaml;

use crate::ops::types::{ErrorInformation, RevisionLocation};

use super::{
    deployment_is_terminal, now_ms, service_state, ComAmazonawsCodedeployCodedeploy20141006State,
    Deployment, DEPLOYMENT_STATUS_FAILED, DEPLOYMENT_STATUS_IN_PROGRESS,
};

/// The `ComputePlatform` whose deployments shift traffic between two versions of a
/// Lambda function (`crate::ops::types::COMPUTE_PLATFORM_VALUES`: ECS | Lambda |
/// Server).
pub const COMPUTE_PLATFORM_LAMBDA: &str = "Lambda";

/// The `ErrorCode` a deployment that could not carry out its Lambda traffic shift
/// fails with, because the function, the alias or the version its AppSpec names is
/// not one AWS Lambda holds
/// (`smithy_query(query="shape",shape="ErrorCode")` carries it).
pub const ERROR_CODE_INVALID_LAMBDA_FUNCTION: &str = "INVALID_LAMBDA_FUNCTION";

/// The AppSpec mapping holding the resources a revision deploys.
const RESOURCES_KEY: &str = "Resources";
/// The member of one resource declaration naming what kind of resource it is.
const TYPE_KEY: &str = "Type";
/// The member of one resource declaration carrying what is deployed about it.
const PROPERTIES_KEY: &str = "Properties";
/// The resource type of the Lambda function an Lambda AppSpec deploys, which is
/// the only declaration that describes a traffic shift.
const LAMBDA_FUNCTION_RESOURCE_TYPE: &str = "AWS::Lambda::Function";
/// The property naming the function whose alias the deployment moves.
const NAME_PROPERTY: &str = "Name";
/// The property naming the alias the deployment moves.
const ALIAS_PROPERTY: &str = "Alias";
/// The property naming the version the alias invoked before the deployment.
const CURRENT_VERSION_PROPERTY: &str = "CurrentVersion";
/// The property naming the version the alias invokes after the deployment.
const TARGET_VERSION_PROPERTY: &str = "TargetVersion";

/// The longest revision content this emulator parses an AppSpec out of.
///
/// `RawStringContent` carries no modelled length
/// (`smithy_query(query="shape",shape="RawStringContent")`), and the content is
/// caller-supplied, so the bound is EMULATOR-CHOSEN: it sits far above the few
/// hundred bytes an AppSpec file takes, and keeps a document nobody could have
/// written as an AppSpec from reaching the parser at all.
const MAX_APP_SPEC_BYTES: usize = 64 * 1024;

/// The most nesting indicators this emulator parses an AppSpec with: the flow
/// collection openers (`[`, `{`) and the block sequence and complex-key
/// indicators (`-`, `?`) standing on their own.
///
/// Each one costs a single byte and a level of BOTH parsers' own recursion, so the
/// byte bound above does not bound the depth such a document reaches: this does,
/// before either parser sees the content. A block mapping needs its own
/// indentation per level instead, which costs a byte per level per level, so the
/// byte bound already holds its depth in the hundreds. Also EMULATOR-CHOSEN, an
/// order of magnitude above the handful an AppSpec opens.
const MAX_APP_SPEC_NESTING_INDICATORS: usize = 256;

/// The deepest AppSpec document this emulator reads, measured on the parser's own
/// event stream before the loader builds a tree of it.
///
/// EMULATOR-CHOSEN: an AppSpec's own nesting is the document, its `Resources`
/// list, one entry, that resource's body and its `Properties` — five levels — so
/// this sits well above any document that declares a traffic shift, and keeps the
/// loader (and the recursive drop of what it built) off a depth no AppSpec has.
const MAX_APP_SPEC_DEPTH: usize = 32;

/// The traffic shift one Lambda deployment performs, as its revision's AppSpec
/// declares it.
///
/// Read when the deployment is recorded and then held, so the shift a deployment
/// performs is fixed by the revision it was created with — editing nothing and
/// re-reading nothing later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LambdaTrafficShift {
    /// The function whose alias moves: the AppSpec's `Name`.
    pub function_name: String,
    /// The alias that moves: the AppSpec's `Alias`.
    pub alias_name: String,
    /// The version the alias invoked before the deployment: the AppSpec's
    /// `CurrentVersion`, when it states one.
    pub current_version: Option<String>,
    /// The version the alias invokes once the shift is complete: the AppSpec's
    /// `TargetVersion`.
    pub target_version: String,
}

/// How far a deployment's declared traffic shift has got.
///
/// What keeps a deployment from being given a successful outcome before its shift
/// has happened: the clock settles no deployment whose shift is
/// [`TrafficShiftProgress::InFlight`]
/// ([`ComAmazonawsCodedeployCodedeploy20141006State::advance_clock`]), so a
/// caller that reads `Succeeded` reads it after the alias moved — including a
/// caller racing the pass that moved it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficShiftProgress {
    /// Declared by the revision, and not yet owed: nothing has been shifted.
    Declared,
    /// Claimed by a pass that is carrying it out in AWS Lambda right now.
    InFlight,
    /// Carried out: the alias invokes the target version.
    Applied,
    /// Attempted and refused, which is what the deployment then fails for.
    Refused,
}

/// The traffic shift `revision` declares for a deployment of `compute_platform`,
/// or `None` when it declares none.
///
/// Only an Lambda deployment shifts traffic between function versions, so the
/// platform is what decides whether the revision's content is read as an Lambda
/// AppSpec at all ([`declared_in_revision`] is that read).
#[must_use]
pub fn declared_shift(
    compute_platform: &str,
    revision: &RevisionLocation,
) -> Option<LambdaTrafficShift> {
    if compute_platform != COMPUTE_PLATFORM_LAMBDA {
        return None;
    }
    declared_in_revision(revision)
}

/// The traffic shift `revision` declares, read out of the content it carries.
///
/// Only the revision forms that CARRY the AppSpec — `appSpecContent`, and the
/// `string` form the model describes as "the location of an Lambda deployment
/// revision stored as a RawString"
/// (`smithy_query(query="shape",shape="RevisionLocation")`) — carry it where this
/// emulator can read it. A revision that names an S3 object or a GitHub commit
/// keeps the AppSpec in that artifact, which this emulator does not fetch, so it
/// declares no shift here.
#[must_use]
pub fn declared_in_revision(revision: &RevisionLocation) -> Option<LambdaTrafficShift> {
    let content = revision
        .app_spec_content
        .as_ref()
        .and_then(|content| content.content.as_deref())
        .or_else(|| {
            revision
                .string
                .as_ref()
                .and_then(|content| content.content.as_deref())
        })?;
    declared_in_app_spec(content)
}

/// The traffic shift an AppSpec document declares, or `None` for content that
/// declares none — including content that is no YAML (or JSON, which YAML 1.2 is a
/// superset of) document at all, and content outside the bounds this emulator
/// parses one within.
///
/// Public so a test can hold the reading of a document to this rule without
/// standing a deployment up for every row.
#[must_use]
pub fn declared_in_app_spec(content: &str) -> Option<LambdaTrafficShift> {
    if !within_bounds(content) || !parses_within_depth(content) {
        return None;
    }
    ::yaml_rust2::YamlLoader::load_from_str(content)
        .ok()?
        .iter()
        .find_map(declared_in_document)
}

/// Whether the content is small enough and flat enough to hand to a parser at
/// all: the two bounds on work a caller chooses the size of
/// ([`MAX_APP_SPEC_BYTES`], [`MAX_APP_SPEC_NESTING_INDICATORS`]), checked on the
/// bytes before any parser sees them.
fn within_bounds(content: &str) -> bool {
    content.len() <= MAX_APP_SPEC_BYTES
        && nesting_indicators(content) <= MAX_APP_SPEC_NESTING_INDICATORS
}

/// How many nesting indicators the content carries, counted up to one past the
/// bound and no further.
///
/// A flow collection opens with `[` or `{`. A block sequence entry and a complex
/// key open with `-` or `?` STANDING ALONE — the byte is an indicator only when
/// what surrounds it is whitespace or an end of the content — so a hyphen inside a
/// function name or an ARN is not counted as nesting.
fn nesting_indicators(content: &str) -> usize {
    let bytes = content.as_bytes();
    let separates = |index: usize| {
        bytes
            .get(index)
            .is_none_or(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    };
    bytes
        .iter()
        .enumerate()
        .filter(|(index, byte)| match byte {
            b'[' | b'{' => true,
            b'-' | b'?' => {
                index.checked_sub(1).is_none_or(separates) && separates(index + 1)
            }
            _ => false,
        })
        .take(MAX_APP_SPEC_NESTING_INDICATORS + 1)
        .count()
}

/// Whether the document parses, uses no YAML alias, and nests no deeper than
/// [`MAX_APP_SPEC_DEPTH`] — all three read off the parser's own event stream,
/// which copies nothing and builds no tree.
///
/// The depth is measured HERE rather than on the loaded document because the
/// loader resolves an alias by COPYING the anchored node and builds (and then
/// recursively drops) the whole tree: by the time there is a tree to measure, the
/// expansion and the recursion have already happened.
fn parses_within_depth(content: &str) -> bool {
    use ::yaml_rust2::parser::Event;

    #[derive(Default)]
    struct Guard {
        /// Whether an alias was reported, whose expansion copies a node.
        saw_alias: bool,
        /// How many collections are open at this event.
        depth: usize,
        /// The most that were open at once.
        deepest: usize,
    }
    impl ::yaml_rust2::parser::EventReceiver for Guard {
        fn on_event(&mut self, event: Event) {
            match event {
                Event::Alias(_) => self.saw_alias = true,
                Event::SequenceStart(..) | Event::MappingStart(..) => {
                    self.depth = self.depth.saturating_add(1);
                    self.deepest = self.deepest.max(self.depth);
                }
                Event::SequenceEnd | Event::MappingEnd => {
                    self.depth = self.depth.saturating_sub(1);
                }
                _ => {}
            }
        }
    }

    let mut guard = Guard::default();
    let parsed = ::yaml_rust2::parser::Parser::new_from_str(content).load(&mut guard, true);
    parsed.is_ok() && !guard.saw_alias && guard.deepest <= MAX_APP_SPEC_DEPTH
}

/// The traffic shift one AppSpec document declares: the first `Resources` entry
/// that describes a Lambda function.
fn declared_in_document(document: &Yaml) -> Option<LambdaTrafficShift> {
    let resources = mapping_get(document, RESOURCES_KEY)?;
    match resources {
        Yaml::Array(declarations) => declarations.iter().find_map(declared_in_declaration),
        mapping => declared_in_declaration(mapping),
    }
}

/// The traffic shift one `Resources` entry declares.
///
/// An AppSpec names the resource it deploys, so an entry is a mapping from that
/// logical name to the resource's own body; an entry that IS the body carries the
/// `Properties` mapping itself, and is read the same way.
fn declared_in_declaration(declaration: &Yaml) -> Option<LambdaTrafficShift> {
    let Yaml::Hash(named) = declaration else {
        return None;
    };
    named
        .iter()
        .map(|(_, body)| body)
        .chain(::std::iter::once(declaration))
        .find_map(declared_in_body)
}

/// The traffic shift one resource body declares: its `Properties`, when the body
/// is a Lambda function at all.
///
/// The resource's `Type` is REQUIRED to be the Lambda function one: a body that
/// states another type describes no version shift, and a body that states no type
/// is no resource declaration this emulator reads a shift out of.
fn declared_in_body(body: &Yaml) -> Option<LambdaTrafficShift> {
    if mapping_get(body, TYPE_KEY)
        .and_then(scalar)
        .is_none_or(|declared| declared.trim() != LAMBDA_FUNCTION_RESOURCE_TYPE)
    {
        return None;
    }
    let properties = mapping_get(body, PROPERTIES_KEY)?;
    let property = |name: &str| {
        mapping_get(properties, name)
            .and_then(scalar)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    Some(LambdaTrafficShift {
        function_name: property(NAME_PROPERTY)?,
        alias_name: property(ALIAS_PROPERTY)?,
        current_version: property(CURRENT_VERSION_PROPERTY),
        target_version: property(TARGET_VERSION_PROPERTY)?,
    })
}

/// One key of a YAML mapping; `None` for a node that is no mapping, and for a
/// mapping that does not carry the key.
fn mapping_get<'a>(node: &'a Yaml, key: &str) -> Option<&'a Yaml> {
    let Yaml::Hash(mapping) = node else {
        return None;
    };
    mapping
        .iter()
        .find(|(stored, _)| stored.as_str() == Some(key))
        .map(|(_, value)| value)
}

/// One AppSpec scalar as the text the property carries.
///
/// A version number written unquoted is a YAML integer, and the member it fills —
/// Lambda's own `Version` (pattern `^(\$LATEST|[0-9]+)$`) — is text, so the
/// integer spelling is read as the digits it is.
fn scalar(node: &Yaml) -> Option<String> {
    match node {
        Yaml::String(text) | Yaml::Real(text) => Some(text.clone()),
        Yaml::Integer(number) => Some(number.to_string()),
        _ => None,
    }
}

/// Carry out the traffic shift of every deployment the clock owes one at this
/// instant, in AWS Lambda itself.
///
/// Runs before the clock may settle a deployment ([`super::with_state`]), so no
/// caller can read an Lambda deployment that reached `Succeeded` while the alias
/// its revision named still invokes the version it invoked before. The shift is a
/// transition the CLOCK owed — traffic routing was complete at that instant
/// whether or not anybody called — so carrying it out from a reporting operation
/// shows a caller nothing a later caller would not also see.
///
/// A store holding no deployment that owes a shift costs one read of this store
/// and no Lambda lock at all.
pub fn apply_due_shifts<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let now = now_ms();
    let claimed = {
        let mut store = state.lock();
        claim_due_shifts(&mut store, now)
    };
    if claimed.is_empty() {
        return;
    }
    // The alias is moved with THIS store unlocked, and what each attempt came to
    // is written back under it.
    let attempted: Vec<(String, Result<(), Denial>)> = claimed
        .into_iter()
        .map(|(deployment_id, shift)| (deployment_id, apply_shift(engine, &shift)))
        .collect();
    let mut store = state.lock();
    for (deployment_id, outcome) in &attempted {
        match outcome {
            Ok(()) => record_applied(&mut store, deployment_id),
            Err(denial) => fail_deployment(&mut store, deployment_id, denial, now),
        }
    }
}

/// Record that the shift of `deployment_id` has been carried out, which is what
/// lets the clock settle it.
fn record_applied(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    deployment_id: &str,
) {
    if let Some(deployment) = state.deployments.get_mut(deployment_id) {
        deployment.traffic_shift = TrafficShiftProgress::Applied;
    }
}

/// The shift of every deployment that is owed one at `now_ms`, CLAIMED as this
/// pass's to carry out: a deployment's shift is marked
/// [`TrafficShiftProgress::InFlight`] before the alias is touched, so two callers
/// racing through here cannot move the same alias twice, and neither can settle
/// the deployment until the pass that claimed it has written back what the shift
/// came to.
///
/// A deployment is owed its shift once the clock owes it completed traffic routing
/// and while it has not reached an outcome — one a caller stopped, or the alarms
/// its group polls stopped, is owed nothing further.
fn claim_due_shifts(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    now_ms: i64,
) -> Vec<(String, LambdaTrafficShift)> {
    let pending: Vec<String> = state.pending_deployments.clone();
    let mut claimed = Vec::new();
    for deployment_id in pending {
        let Some(deployment) = state.deployments.get_mut(&deployment_id) else {
            continue;
        };
        if !shift_is_due(deployment, now_ms) {
            continue;
        }
        let Some(shift) = deployment.lambda_traffic_shift.clone() else {
            continue;
        };
        deployment.traffic_shift = TrafficShiftProgress::InFlight;
        claimed.push((deployment_id, shift));
    }
    claimed
}

/// Whether `deployment` is owed its traffic shift at `now_ms`; see
/// [`claim_due_shifts`].
fn shift_is_due(deployment: &Deployment, now_ms: i64) -> bool {
    deployment.lambda_traffic_shift.is_some()
        && deployment.traffic_shift == TrafficShiftProgress::Declared
        && deployment.complete_time_ms.is_none()
        && !deployment_is_terminal(&deployment.status)
        && deployment.traffic_routing_complete_at_ms() <= now_ms
}

/// Move the alias `shift` names to its target version, in the Lambda emulator's
/// own function store.
///
/// The move is the one `UpdateAlias` performs for the same change: the version the
/// alias invokes becomes the target version, the weighted routing it was splitting
/// its invocations with is gone — an all-at-once shift leaves none behind — and the
/// alias's revision id changes, because "a unique identifier that changes when you
/// update the alias" changed on this write.
///
/// What is checked before the write is that the Lambda resources the AppSpec names
/// EXIST: the function, the version traffic moves to, and the alias that moves. The
/// AppSpec's `CurrentVersion` is NOT a precondition here — the shift is applied to
/// whatever version the alias invokes when it happens — because this emulator has
/// no measurement of what the real service answers for a deployment whose
/// `CurrentVersion` has been overtaken, and inventing a failure for it would report
/// an outcome nothing established.
fn apply_shift<Eng: Send + Sync + 'static>(
    engine: &Eng,
    shift: &LambdaTrafficShift,
) -> Result<(), Denial> {
    // No Lambda beside this service is the case where the function the AppSpec
    // names cannot be resolved at all.
    let Some(state) = lambda::state::service_state(engine) else {
        return Err(Denial::function(&shift.function_name));
    };
    let reference = lambda::state::parse_function_reference(&shift.function_name)
        .ok_or_else(|| Denial::function(&shift.function_name))?;
    let mut store = state.lock();
    let Some(record) = store.functions.get_mut(&reference.name) else {
        return Err(Denial::function(&shift.function_name));
    };
    // The transitions Lambda's own clock already owed that function, so the
    // versions this shift is checked against are the ones a client would read.
    record.materialize(SystemTime::now());
    if record.config_of(&shift.target_version).is_none() {
        return Err(Denial::target_version(
            &shift.function_name,
            &shift.target_version,
        ));
    }
    let Some(alias) = record.aliases.get_mut(&shift.alias_name) else {
        return Err(Denial::alias(&shift.function_name, &shift.alias_name));
    };
    alias.function_version = shift.target_version.clone();
    alias.additional_version_weights.clear();
    alias.revision_id = lambda::state::mint_revision_id();
    Ok(())
}

/// Why a deployment's traffic shift could not be carried out, as CodeDeploy
/// reports it on `DeploymentInfo.errorInformation`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Denial {
    /// An `ErrorCode`; see [`ERROR_CODE_INVALID_LAMBDA_FUNCTION`].
    code: &'static str,
    /// The accompanying message, naming the resource the AppSpec asked for.
    message: String,
}

impl Denial {
    /// The function the AppSpec names is not one AWS Lambda holds.
    fn function(function_name: &str) -> Self {
        Self {
            code: ERROR_CODE_INVALID_LAMBDA_FUNCTION,
            message: lambda_function_missing_message(function_name),
        }
    }

    /// The function holds no alias of the name the AppSpec gives.
    fn alias(function_name: &str, alias_name: &str) -> Self {
        Self {
            code: ERROR_CODE_INVALID_LAMBDA_FUNCTION,
            message: lambda_alias_missing_message(function_name, alias_name),
        }
    }

    /// The function has no such version to shift traffic to.
    fn target_version(function_name: &str, target_version: &str) -> Self {
        Self {
            code: ERROR_CODE_INVALID_LAMBDA_FUNCTION,
            message: lambda_target_version_missing_message(function_name, target_version),
        }
    }
}

/// The message a deployment whose AppSpec names a function AWS Lambda does not
/// hold fails with, naming that function.
#[must_use]
pub fn lambda_function_missing_message(function_name: &str) -> String {
    format!("The Lambda function {function_name} specified in the AppSpec file does not exist")
}

/// The message a deployment whose AppSpec names an alias the function does not
/// hold fails with, naming both.
#[must_use]
pub fn lambda_alias_missing_message(function_name: &str, alias_name: &str) -> String {
    format!(
        "The alias {alias_name} specified in the AppSpec file does not exist on the Lambda \
         function {function_name}"
    )
}

/// The message a deployment whose AppSpec targets a version the function has not
/// published fails with, naming both.
#[must_use]
pub fn lambda_target_version_missing_message(function_name: &str, target_version: &str) -> String {
    format!(
        "The TargetVersion {target_version} specified in the AppSpec file is not a published \
         version of the Lambda function {function_name}"
    )
}

/// Fail `deployment_id` for a shift that could not be carried out, with the error
/// information CodeDeploy reports for it.
///
/// This store was unlocked while the alias was reached, so what the claim was made
/// from is checked again: a deployment another caller stopped in between keeps the
/// outcome it already has. A deployment the clock had not yet started also gets the
/// start it was owed, so a deployment that failed before anybody read it still
/// reports the instant it began.
fn fail_deployment(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    deployment_id: &str,
    denial: &Denial,
    now_ms: i64,
) {
    let Some(deployment) = state.deployments.get_mut(deployment_id) else {
        return;
    };
    deployment.traffic_shift = TrafficShiftProgress::Refused;
    if deployment.complete_time_ms.is_some() || deployment_is_terminal(&deployment.status) {
        return;
    }
    if deployment.start_time_ms.is_none() {
        deployment.start_time_ms = Some(deployment.starts_at_ms);
        deployment.status = DEPLOYMENT_STATUS_IN_PROGRESS.to_string();
    }
    state.settle_deployment(
        deployment_id,
        DEPLOYMENT_STATUS_FAILED,
        Some(ErrorInformation {
            code: Some(denial.code.to_string()),
            message: Some(denial.message.clone()),
        }),
        now_ms,
    );
}

#[cfg(test)]
mod tests {
    //! What [`declared_in_app_spec`] reads out of a revision's content, over the
    //! forms an AppSpec is written in — cheap here, where a row costs a parse
    //! rather than a function, three versions, an alias and a deployment.

    use super::*;

    /// A shift with no `CurrentVersion`, for the rows that state none.
    fn shift(function_name: &str, alias_name: &str, target_version: &str) -> LambdaTrafficShift {
        LambdaTrafficShift {
            function_name: function_name.to_string(),
            alias_name: alias_name.to_string(),
            current_version: None,
            target_version: target_version.to_string(),
        }
    }

    /// RULE: the AppSpec's Lambda function declaration is what names the function,
    /// the alias and the versions of the shift — in either of the two formats the
    /// model says the content is written in, with the version numbers written as
    /// text or as numbers.
    #[test]
    fn the_app_spec_declares_the_shift() {
        let yaml = "version: 0.0\n\
                    Resources:\n\
                    \x20 - handler:\n\
                    \x20     Type: AWS::Lambda::Function\n\
                    \x20     Properties:\n\
                    \x20       Name: handler\n\
                    \x20       Alias: live\n\
                    \x20       CurrentVersion: \"1\"\n\
                    \x20       TargetVersion: \"2\"\n";
        let unquoted = "version: 0.0\n\
                        Resources:\n\
                        \x20 - handler:\n\
                        \x20     Type: AWS::Lambda::Function\n\
                        \x20     Properties:\n\
                        \x20       Name: handler\n\
                        \x20       Alias: live\n\
                        \x20       CurrentVersion: 1\n\
                        \x20       TargetVersion: 2\n";
        let json = r#"{"version": 0.0, "Resources": [{"handler": {
            "Type": "AWS::Lambda::Function",
            "Properties": {"Name": "handler", "Alias": "live",
                           "CurrentVersion": "1", "TargetVersion": "2"}}}]}"#;
        let mapping = "version: 0.0\n\
                       Resources:\n\
                       \x20 handler:\n\
                       \x20   Type: AWS::Lambda::Function\n\
                       \x20   Properties:\n\
                       \x20     Name: orders\n\
                       \x20     Alias: blue\n\
                       \x20     TargetVersion: 7\n";
        let ecs = "version: 0.0\n\
                   Resources:\n\
                   \x20 - target-service:\n\
                   \x20     Type: AWS::ECS::Service\n\
                   \x20     Properties:\n\
                   \x20       TaskDefinition: arn:aws:ecs:us-east-1:1234:task-definition/web:1\n";
        let aliased = "version: &anchor 0.0\n\
                       Resources: *anchor\n";

        let declared = shift("handler", "live", "2");
        let with_current = LambdaTrafficShift {
            current_version: Some("1".to_string()),
            ..declared
        };
        let cases: &[(&str, &str, Option<LambdaTrafficShift>)] = &[
            ("a YAML AppSpec", yaml, Some(with_current.clone())),
            (
                "a YAML AppSpec whose versions are unquoted numbers",
                unquoted,
                Some(with_current.clone()),
            ),
            ("a JSON AppSpec", json, Some(with_current)),
            (
                "a YAML AppSpec whose Resources is a mapping",
                mapping,
                Some(shift("orders", "blue", "7")),
            ),
            ("an AppSpec that declares no resource", "version: 0.0", None),
            ("an AppSpec of another compute platform", ecs, None),
            ("content that is no document at all", "\tnot: yaml\n", None),
            ("an AppSpec that uses a YAML alias", aliased, None),
            ("empty content", "", None),
        ];
        for (case, content, expected) in cases {
            let read = declared_in_app_spec(content);
            assert_eq!(
                read.as_ref(),
                expected.as_ref(),
                "case {case:?}: expected the declared shift {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: content outside the bounds this emulator parses an AppSpec within is
    /// not parsed at all, so it declares no shift — each bound held at its own
    /// value and one step past it.
    #[test]
    fn content_outside_the_parsing_bounds_declares_no_shift() {
        let document = "version: 0.0\n\
                        Resources:\n\
                        \x20 - handler:\n\
                        \x20     Type: AWS::Lambda::Function\n\
                        \x20     Properties:\n\
                        \x20       Name: handler\n\
                        \x20       Alias: live\n\
                        \x20       TargetVersion: 2\n";
        // The same AppSpec, padded with a comment line to exactly the byte bound
        // and to one byte past it: the comment is `# `, the padding, and the
        // newline that ends it, so three bytes beside the padding itself.
        let padded = |len: usize| {
            let mut content = String::from("# ");
            content.push_str(&"=".repeat(len - document.len() - 3));
            content.push('\n');
            content.push_str(document);
            content
        };
        let at_bytes = padded(MAX_APP_SPEC_BYTES);
        let past_bytes = padded(MAX_APP_SPEC_BYTES + 1);
        // A block sequence of `n` entries, which is `n` nesting indicators: the
        // spelling the byte bound alone would not hold, since each costs two
        // bytes.
        let entries = |count: usize| {
            let mut content = String::from("Resources:\n");
            for _ in 0..count {
                content.push_str("  - a\n");
            }
            content
        };
        let at_indicators = entries(MAX_APP_SPEC_NESTING_INDICATORS);
        let past_indicators = entries(MAX_APP_SPEC_NESTING_INDICATORS + 1);
        // A document nested to a chosen depth: the mapping it opens with, plus one
        // level per flow collection inside it.
        let nested = |depth: usize| {
            format!(
                "Resources: {}{}",
                "[".repeat(depth - 1),
                "]".repeat(depth - 1)
            )
        };
        let at_depth = nested(MAX_APP_SPEC_DEPTH);
        let past_depth = nested(MAX_APP_SPEC_DEPTH + 1);

        let declared = shift("handler", "live", "2");
        let cases: &[(&str, &str, bool, bool, Option<&LambdaTrafficShift>)] = &[
            (
                "an AppSpec padded to exactly the byte bound",
                &at_bytes,
                true,
                true,
                Some(&declared),
            ),
            (
                "an AppSpec padded one byte past the byte bound",
                &past_bytes,
                false,
                true,
                None,
            ),
            (
                "a document at the nesting-indicator bound",
                &at_indicators,
                true,
                true,
                None,
            ),
            (
                "a document one indicator past the nesting-indicator bound",
                &past_indicators,
                false,
                true,
                None,
            ),
            (
                "a document nested to exactly the depth bound",
                &at_depth,
                true,
                true,
                None,
            ),
            (
                "a document nested one level past the depth bound",
                &past_depth,
                true,
                false,
                None,
            ),
        ];

        for (case, content, bounded, within_depth, expected) in cases {
            let within = within_bounds(content);
            assert_eq!(
                within, *bounded,
                "case {case:?}: expected the content to be within bounds: {bounded}, got {within}"
            );
            let depth = parses_within_depth(content);
            assert_eq!(
                depth, *within_depth,
                "case {case:?}: expected the content to be within the depth bound: \
                 {within_depth}, got {depth}"
            );
            let read = declared_in_app_spec(content);
            assert_eq!(
                read.as_ref(),
                *expected,
                "case {case:?}: expected the declared shift {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: only an Lambda deployment shifts traffic between function versions,
    /// and only the revision forms that carry the AppSpec carry the shift.
    #[test]
    fn only_a_lambda_revision_that_carries_the_app_spec_declares_a_shift() {
        let content = "version: 0.0\n\
                       Resources:\n\
                       \x20 - handler:\n\
                       \x20     Type: AWS::Lambda::Function\n\
                       \x20     Properties:\n\
                       \x20       Name: handler\n\
                       \x20       Alias: live\n\
                       \x20       TargetVersion: 2\n";
        let app_spec_content = RevisionLocation {
            revision_type: Some("AppSpecContent".to_string()),
            app_spec_content: Some(crate::ops::types::AppSpecContent {
                content: Some(content.to_string()),
                sha256: None,
            }),
            ..RevisionLocation::default()
        };
        let raw_string = RevisionLocation {
            revision_type: Some("String".to_string()),
            string: Some(crate::ops::types::RawString {
                content: Some(content.to_string()),
                sha256: None,
            }),
            ..RevisionLocation::default()
        };
        let s3 = RevisionLocation {
            revision_type: Some("S3".to_string()),
            s3_location: Some(crate::ops::types::S3Location {
                bucket: Some("artifacts".to_string()),
                key: Some("app.zip".to_string()),
                ..crate::ops::types::S3Location::default()
            }),
            ..RevisionLocation::default()
        };

        let declared = shift("handler", "live", "2");
        let cases: &[(&str, &str, &RevisionLocation, Option<LambdaTrafficShift>)] = &[
            (
                "an AppSpecContent revision of a Lambda deployment",
                COMPUTE_PLATFORM_LAMBDA,
                &app_spec_content,
                Some(declared.clone()),
            ),
            (
                "a String revision of a Lambda deployment",
                COMPUTE_PLATFORM_LAMBDA,
                &raw_string,
                Some(declared),
            ),
            (
                "an S3 revision of a Lambda deployment",
                COMPUTE_PLATFORM_LAMBDA,
                &s3,
                None,
            ),
            (
                "the same AppSpec on an EC2/on-premises deployment",
                "Server",
                &app_spec_content,
                None,
            ),
            (
                "the same AppSpec on an Amazon ECS deployment",
                "ECS",
                &app_spec_content,
                None,
            ),
        ];

        for (case, platform, revision, expected) in cases {
            let read = declared_shift(platform, revision);
            assert_eq!(
                read.as_ref(),
                expected.as_ref(),
                "case {case:?}: expected the declared shift {expected:?}, got {read:?}"
            );
        }
    }
}
