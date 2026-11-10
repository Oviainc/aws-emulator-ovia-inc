// SPDX-License-Identifier: CC0-1.0

//! `AWS::Logs::Destination`, as it lives in the CloudWatch Logs service's own store.
//!
//! A logical destination — "a cross-account destination that receives subscription log
//! events" — is a resource of the CloudWatch Logs service: `logs:PutDestination` creates
//! it, `logs:DescribeDestinations` reports it and `logs:DeleteDestination` removes it. So,
//! exactly as for a log group ([`super::log_groups`]), this provider holds a handle to THAT
//! store and nothing of its own, and the same rule follows: a destination this emulator
//! holds is one Cloud Control API resolves, whichever API created it.
//!
//! The resource model is the type's registry schema. Catalog source: the
//! `AWS::Logs::Destination` resource provider schema's `properties` — `DestinationName`,
//! `TargetArn`, `RoleArn`, `DestinationPolicy` and the read-only `Arn` — every one of which
//! the owning record carries.

use ::std::time::SystemTime;

use ::serde_json::{Map, Value};
use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

use super::{
    epoch_millis, require_value, text, window, LogsState, NativeRefusal, NativeResourceProvider,
};

/// The `TypeName` this provider stands behind.
pub const TYPE_NAME: &str = "AWS::Logs::Destination";

/// The property that carries the type's primary identifier
/// (`primaryIdentifier: ["/properties/DestinationName"]`).
const IDENTIFIER_PROPERTY: &str = "DestinationName";

/// The type's one read-only property: the destination's ARN
/// (`readOnlyProperties: ["/properties/Arn"]`).
const ARN: &str = "Arn";

/// "The ARN of the physical target where the log events are delivered" (`TargetArn`).
const TARGET_ARN: &str = "TargetArn";

/// "A role for impersonation, used when delivering log events to the target" (`RoleArn`).
const ROLE_ARN: &str = "RoleArn";

/// The access policy document attached to the destination (`DestinationPolicy`).
const DESTINATION_POLICY: &str = "DestinationPolicy";

/// The CloudWatch Logs logical destinations of one engine, as Cloud Control API resources.
pub struct DestinationProvider {
    /// The owning service's store. A handle, so every answer is read from the live store.
    state: ::engine::ServiceState<LogsState>,
}

impl DestinationProvider {
    /// The provider for the CloudWatch Logs store THIS engine carries, or `None` when it
    /// carries none.
    pub fn for_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<Self> {
        logs::service_state(engine).map(|state| Self { state })
    }
}

impl NativeResourceProvider for DestinationProvider {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn identifier_property(&self) -> &'static str {
        IDENTIFIER_PROPERTY
    }

    fn read(&self, region: &str, identifier: &str) -> Option<Value> {
        // One namespace per account, as for a log group ([`super::log_groups`]).
        let _ = region;
        self.state.lock().destinations.get(identifier).map(model)
    }

    fn list(&self, region: &str, after: Option<&str>, limit: usize) -> Vec<(String, Value)> {
        let _ = region;
        // As for a log group ([`super::log_groups`]): the owning store keys its
        // destinations by name, so its own ordered range is the window a listing reports.
        let store = self.state.lock();
        window(&store.destinations, after, limit)
            .map(|(name, destination)| (name.clone(), model(destination)))
            .collect()
    }

    fn create(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        // The name a destination is created under is the OWNING service's to accept: the
        // domain its model gives `destinationName`, read through the rule that service
        // exports for it, so nothing is created here under a name its own API would
        // refuse.
        if !logs::is_destination_name(identifier) {
            return Err(NativeRefusal::Invalid(format!(
                "{IDENTIFIER_PROPERTY} {}",
                logs::destination_name_requirement()
            )));
        }
        let desired = DesiredModel::read(
            identifier,
            properties,
            &logs::destination_arn(region, identifier),
        )?;
        let mut store = self.state.lock();
        if store.destinations.contains_key(identifier) {
            return Err(NativeRefusal::AlreadyExists);
        }
        // The owning service's own account quota bounds what this create may add.
        if store.destinations.len() >= logs::MAX_DESTINATIONS {
            return Err(NativeRefusal::AtCapacity(logs::destinations_quota_message()));
        }
        let mut destination = logs::DestinationRecord::new(
            region,
            identifier,
            &desired.target_arn,
            &desired.role_arn,
            epoch_millis(at),
        );
        desired.apply(&mut destination);
        let created = model(&destination);
        store
            .destinations
            .insert(identifier.to_string(), destination);
        Ok(created)
    }

    fn update(
        &self,
        region: &str,
        identifier: &str,
        properties: &Value,
        at: SystemTime,
    ) -> Result<Value, NativeRefusal> {
        let _ = (region, at);
        let mut store = self.state.lock();
        let held_arn = store
            .destinations
            .get(identifier)
            .ok_or(NativeRefusal::NotFound)?
            .arn
            .clone();
        let desired = DesiredModel::read(identifier, properties, &held_arn)?;
        let destination = store
            .destinations
            .get_mut(identifier)
            .ok_or(NativeRefusal::NotFound)?;
        desired.apply(destination);
        Ok(model(destination))
    }

    fn delete(&self, region: &str, identifier: &str, at: SystemTime) -> Result<(), NativeRefusal> {
        let _ = (region, at);
        let mut store = self.state.lock();
        store
            .destinations
            .remove(identifier)
            .map(|_| ())
            .ok_or(NativeRefusal::NotFound)
    }
}

/// The resource model one destination renders as — the schema's properties the owning
/// record carries, and the access policy only where one has been attached, which is the
/// condition under which the owning service's own reader reports it.
fn model(destination: &logs::DestinationRecord) -> Value {
    let mut model = Map::new();
    model.insert(
        IDENTIFIER_PROPERTY.to_string(),
        Value::from(destination.destination_name.clone()),
    );
    model.insert(ARN.to_string(), Value::from(destination.arn.clone()));
    model.insert(
        TARGET_ARN.to_string(),
        Value::from(destination.target_arn.clone()),
    );
    model.insert(
        ROLE_ARN.to_string(),
        Value::from(destination.role_arn.clone()),
    );
    if let Some(policy) = &destination.access_policy {
        model.insert(DESTINATION_POLICY.to_string(), Value::from(policy.clone()));
    }
    Value::Object(model)
}

/// What a resource model asks a destination to be, having been checked against the owning
/// service's own rules.
struct DesiredModel {
    /// Where the log events are delivered.
    target_arn: String,
    /// The role the delivery impersonates.
    role_arn: String,
    /// The access policy the destination is to carry, or `None` for none.
    access_policy: Option<String>,
}

impl DesiredModel {
    /// Read `properties` as the model of the destination `identifier`, whose own ARN is
    /// `arn` — or state why the owning service would not take it.
    fn read(identifier: &str, properties: &Value, arn: &str) -> Result<Self, NativeRefusal> {
        let Some(model) = properties.as_object() else {
            return Err(NativeRefusal::Invalid(format!(
                "{TYPE_NAME} requires a resource model of its properties"
            )));
        };
        let mut target_arn = None;
        let mut role_arn = None;
        let mut access_policy = None;
        for (property, value) in model {
            match property.as_str() {
                IDENTIFIER_PROPERTY => require_value(property, value, identifier)?,
                ARN => require_value(property, value, arn)?,
                TARGET_ARN => target_arn = Some(text(property, value)?),
                ROLE_ARN => role_arn = Some(text(property, value)?),
                DESTINATION_POLICY => {
                    let document = text(property, value)?;
                    // The size the owning service holds an access policy document to.
                    if document.len() > logs::MAX_ACCESS_POLICY_BYTES {
                        return Err(NativeRefusal::Invalid(format!(
                            "{DESTINATION_POLICY} must be at most {} bytes",
                            logs::MAX_ACCESS_POLICY_BYTES
                        )));
                    }
                    access_policy = Some(document);
                }
                other => {
                    return Err(NativeRefusal::Invalid(format!(
                        "{other} is not a property of {TYPE_NAME}"
                    )));
                }
            }
        }
        // A destination IS the pair of ARNs the owning service parses together — the target
        // the events are delivered to and the role the delivery assumes — and that service
        // reports the failure of either without naming which, which is also what a model
        // that names neither leaves it nothing to parse for.
        let (Some(target_arn), Some(role_arn)) = (target_arn, role_arn) else {
            return Err(NativeRefusal::Invalid(
                logs::DESTINATION_ARN_PARSE_FAILED.to_string(),
            ));
        };
        if [target_arn.as_str(), role_arn.as_str()]
            .iter()
            .any(|value| logs::arn_service_and_resource(value).is_none())
        {
            return Err(NativeRefusal::Invalid(
                logs::DESTINATION_ARN_PARSE_FAILED.to_string(),
            ));
        }
        Ok(Self {
            target_arn,
            role_arn,
            access_policy,
        })
    }

    /// Write this model onto `destination`. A property the model does not carry is
    /// CLEARED, so the record holds exactly the model that was applied.
    fn apply(self, destination: &mut logs::DestinationRecord) {
        destination.target_arn = self.target_arn;
        destination.role_arn = self.role_arn;
        destination.access_policy = self.access_policy;
    }
}
