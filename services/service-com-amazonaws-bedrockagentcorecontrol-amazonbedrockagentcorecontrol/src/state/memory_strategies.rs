// SPDX-License-Identifier: CC0-1.0

//! What a requested memory strategy becomes before it is stored: the type and
//! members read off the `MemoryStrategyInput` union the request carried, the
//! namespace list the rule gives it, its validated record expiry, and the
//! `MemoryStrategy` every operation that reports one carries.
//!
//! One concern, kept beside the memory record rather than in it: a strategy list
//! is prepared the same way whether it creates a memory or is added to one, so
//! `CreateMemory` and `UpdateMemory` cannot word the same refusal — or default
//! the same namespace — two ways.

use std::time::SystemTime;

use crate::ops::types::{
    ConsolidationConfiguration, CustomConfigurationInput, CustomConsolidationConfiguration,
    CustomExtractionConfiguration, CustomReflectionConfiguration, EpisodicConsolidationOverride,
    EpisodicExtractionOverride, EpisodicReflectionConfiguration, EpisodicReflectionOverride,
    ExtractionConfiguration, InvocationConfiguration, MemoryExtractionHealth, MemoryRecordSchema,
    MemoryStrategy, MemoryStrategyInput, MessageBasedTrigger, RecordExpirySpecification,
    ReflectionConfiguration, SelfManagedConfiguration, SemanticConsolidationOverride,
    SemanticExtractionOverride, ServiceQuotaExceededException, StrategyConfiguration,
    SummaryConsolidationOverride, TimeBasedTrigger, TokenBasedTrigger, TriggerCondition,
    TriggerConditionInput, UserPreferenceConsolidationOverride, UserPreferenceExtractionOverride,
    ValidationException,
};

use super::{field_validation_failed, timestamp, MAX_STRATEGIES_PER_MEMORY};

/// The extraction health a memory read reports, for the memory and for each of
/// its strategies: how many of its long-term memory extraction jobs FAILED.
///
/// A memory extraction job is started and listed on the memory DATA plane
/// (`StartMemoryExtractionJob`, `ListMemoryExtractionJobs`), not here, so this
/// control plane's store holds no job of any memory and no operation of it can
/// fail one: the count every read reports is therefore zero, for the same reason
/// a memory reports no `failureReason`. It is built in one place rather than per
/// handler, so the memory level and the strategy level cannot disagree.
pub fn extraction_health() -> MemoryExtractionHealth {
    MemoryExtractionHealth {
        failed_memory_extraction_job_count: 0,
    }
}

/// The three namespace variables the service defines itself, which a caller may
/// therefore not define again: `NamespaceVariableKey`'s pattern
/// `^(?!memoryStrategyId$|actorId$|sessionId$)[a-z][a-z0-9]*$` forbids exactly
/// these names (`smithy_query shape=NamespaceVariableKey`), and a namespace
/// template resolves them without any `namespaceKeys` entry.
pub const SERVICE_NAMESPACE_VARIABLES: [&str; 3] = ["memoryStrategyId", "actorId", "sessionId"];

/// The `{variable}` names one namespace template references, in the order they
/// appear. An unclosed `{` references nothing.
pub fn namespace_variables(template: &str) -> impl Iterator<Item = &str> {
    template.split('{').skip(1).filter_map(|rest| {
        let (variable, _) = rest.split_once('}')?;
        Some(variable)
    })
}

/// `MemoryStrategyType`, one value per member of the `MemoryStrategyInput` union
/// a request carries. The enum's full value list is `SEMANTIC | SUMMARIZATION |
/// USER_PREFERENCE | CUSTOM | EPISODIC` (`smithy_query shape=MemoryStrategyType`).
pub const STRATEGY_TYPE_SEMANTIC: &str = "SEMANTIC";
pub const STRATEGY_TYPE_SUMMARIZATION: &str = "SUMMARIZATION";
pub const STRATEGY_TYPE_USER_PREFERENCE: &str = "USER_PREFERENCE";
pub const STRATEGY_TYPE_CUSTOM: &str = "CUSTOM";
pub const STRATEGY_TYPE_EPISODIC: &str = "EPISODIC";

/// `StrategyConfiguration.type` for a custom strategy, one value per member of
/// the `CustomConfigurationInput` union the request carries. The enum's full
/// value list is `SEMANTIC_OVERRIDE | SUMMARY_OVERRIDE | USER_PREFERENCE_OVERRIDE
/// | SELF_MANAGED | EPISODIC_OVERRIDE` (`smithy_query shape=OverrideType`).
pub const OVERRIDE_TYPE_SEMANTIC: &str = "SEMANTIC_OVERRIDE";
pub const OVERRIDE_TYPE_SUMMARY: &str = "SUMMARY_OVERRIDE";
pub const OVERRIDE_TYPE_USER_PREFERENCE: &str = "USER_PREFERENCE_OVERRIDE";
pub const OVERRIDE_TYPE_EPISODIC: &str = "EPISODIC_OVERRIDE";
pub const OVERRIDE_TYPE_SELF_MANAGED: &str = "SELF_MANAGED";

/// The `fieldList` member name a strategy's record-expiry specification is
/// reported under: the path to it through the request, so a caller sees which
/// member of the nested union failed.
pub const RECORD_EXPIRY_MEMBER: &str = "memoryStrategies.recordExpirySpecification.action";
pub const RECORD_EXPIRY_DAYS_MEMBER: &str =
    "memoryStrategies.recordExpirySpecification.recordExpiryDurationDays";

/// The shortest record expiry a strategy may be given: `RecordExpiryDurationDays`
/// is `range 31..`, which the model also states in prose ("The minimum is 31
/// days", `smithy_query shape=RecordExpirySpecification`). The constraint sits on
/// a member of a union nested in a list, which the framework's boundary walk does
/// not descend into, so the handler refuses it — and `MemoryStrategy` reports the
/// same shape, so a shorter duration could not be rendered back either.
pub const MIN_RECORD_EXPIRY_DURATION_DAYS: i32 = 31;

/// Why a requested strategy list is refused, in the two modelled shapes every
/// operation that takes one declares: each handler lifts this into its OWN
/// error enum, so the refusal is worded once however many operations share it.
#[derive(Debug)]
pub enum StrategyRefusal {
    Validation(ValidationException),
    QuotaExceeded(ServiceQuotaExceededException),
}

/// What one requested strategy asked for, read off whichever member of the
/// `MemoryStrategyInput` union the request carried: the type that member names,
/// and the members every one of them declares.
struct RequestedStrategy {
    strategy_type: &'static str,
    name: String,
    description: Option<String>,
    /// The legacy `namespaces` member ("Use namespaceTemplates instead").
    namespaces: Option<Vec<String>>,
    namespace_templates: Option<Vec<String>>,
    configuration: Option<StrategyConfiguration>,
    memory_record_schema: Option<MemoryRecordSchema>,
    record_expiry: Option<RecordExpirySpecification>,
}

/// The namespace template a strategy gets when the caller supplies neither
/// `namespaceTemplates` nor the legacy `namespaces`, which `MemoryStrategy`
/// reports as `@required` (exactly one entry) either way.
///
/// The three namespace variables the service defines itself are
/// [`SERVICE_NAMESPACE_VARIABLES`] — exactly the names `NamespaceVariableKey`'s
/// pattern forbids a caller to redefine. A strategy whose records summarise or
/// recount ONE conversation (summarization, episodic) is therefore scoped down
/// to the session; one whose records are facts about the actor (semantic, user
/// preference, and a custom strategy, which overrides one of those) is scoped to
/// the actor.
fn default_namespace_template(strategy_type: &str) -> String {
    if strategy_type == STRATEGY_TYPE_SUMMARIZATION || strategy_type == STRATEGY_TYPE_EPISODIC {
        "/strategies/{memoryStrategyId}/actors/{actorId}/sessions/{sessionId}".to_string()
    } else {
        "/strategies/{memoryStrategyId}/actors/{actorId}".to_string()
    }
}

/// The namespace list a strategy is stored with: the caller's
/// `namespaceTemplates`, else the legacy `namespaces` they supplied instead,
/// else [`default_namespace_template`]. `MemoryStrategy` declares both members
/// `@required` with exactly one entry, so the stored strategy carries one list
/// under both names rather than an empty one the wire cannot render.
fn effective_namespaces(requested: &RequestedStrategy) -> Vec<String> {
    for namespaces in [&requested.namespace_templates, &requested.namespaces]
        .into_iter()
        .flatten()
    {
        if !namespaces.is_empty() {
            return namespaces.clone();
        }
    }
    vec![default_namespace_template(requested.strategy_type)]
}

/// `MemoryStrategy.recordExpiryDurationDays` for a strategy's requested
/// `recordExpirySpecification`, on the path that CREATES the strategy.
///
/// The union carries either a duration or an `action`, and the model says of
/// `action`: "Set to REMOVE to remove a previously configured time-to-live …
/// This value is not applicable on create paths" — there is no previously
/// configured expiry on a strategy being created, so it is refused.
fn record_expiry_duration_days(
    requested: Option<RecordExpirySpecification>,
) -> Result<Option<i32>, ValidationException> {
    match requested {
        None => Ok(None),
        Some(RecordExpirySpecification::recordExpiryDurationDays(days)) => {
            if days < MIN_RECORD_EXPIRY_DURATION_DAYS {
                return Err(field_validation_failed(
                    RECORD_EXPIRY_DAYS_MEMBER,
                    &format!("must be at least {MIN_RECORD_EXPIRY_DURATION_DAYS} days, got {days}"),
                ));
            }
            Ok(Some(days))
        }
        Some(RecordExpirySpecification::Action(action)) => Err(field_validation_failed(
            RECORD_EXPIRY_MEMBER,
            &format!("{action} is not applicable on create paths"),
        )),
    }
}

/// One requested strategy, read off the union member the request carried.
fn requested_strategy(requested: MemoryStrategyInput) -> RequestedStrategy {
    match requested {
        MemoryStrategyInput::semanticMemoryStrategy(strategy) => RequestedStrategy {
            strategy_type: STRATEGY_TYPE_SEMANTIC,
            name: strategy.name,
            description: strategy.description,
            namespaces: strategy.namespaces,
            namespace_templates: strategy.namespace_templates,
            // A built-in strategy carries no configuration of its own: the
            // overrides `StrategyConfiguration` reports are what a CUSTOM
            // strategy supplies.
            configuration: None,
            memory_record_schema: strategy.memory_record_schema,
            record_expiry: strategy.record_expiry_specification,
        },
        MemoryStrategyInput::summaryMemoryStrategy(strategy) => RequestedStrategy {
            strategy_type: STRATEGY_TYPE_SUMMARIZATION,
            name: strategy.name,
            description: strategy.description,
            namespaces: strategy.namespaces,
            namespace_templates: strategy.namespace_templates,
            configuration: None,
            memory_record_schema: strategy.memory_record_schema,
            record_expiry: strategy.record_expiry_specification,
        },
        MemoryStrategyInput::userPreferenceMemoryStrategy(strategy) => RequestedStrategy {
            strategy_type: STRATEGY_TYPE_USER_PREFERENCE,
            name: strategy.name,
            description: strategy.description,
            namespaces: strategy.namespaces,
            namespace_templates: strategy.namespace_templates,
            configuration: None,
            memory_record_schema: strategy.memory_record_schema,
            record_expiry: strategy.record_expiry_specification,
        },
        MemoryStrategyInput::episodicMemoryStrategy(strategy) => RequestedStrategy {
            strategy_type: STRATEGY_TYPE_EPISODIC,
            name: strategy.name,
            description: strategy.description,
            namespaces: strategy.namespaces,
            namespace_templates: strategy.namespace_templates,
            // The one built-in strategy with a configuration member of its own:
            // the reflections it creates.
            configuration: strategy.reflection_configuration.map(|reflection| {
                StrategyConfiguration {
                    r#type: None,
                    extraction: None,
                    consolidation: None,
                    reflection: Some(ReflectionConfiguration::episodicReflectionConfiguration(
                        EpisodicReflectionConfiguration {
                            namespaces: reflection.namespaces,
                            namespace_templates: reflection.namespace_templates,
                            memory_record_schema: reflection.memory_record_schema,
                        },
                    )),
                    self_managed_configuration: None,
                }
            }),
            memory_record_schema: strategy.memory_record_schema,
            record_expiry: strategy.record_expiry_specification,
        },
        MemoryStrategyInput::customMemoryStrategy(strategy) => RequestedStrategy {
            strategy_type: STRATEGY_TYPE_CUSTOM,
            name: strategy.name,
            description: strategy.description,
            namespaces: strategy.namespaces,
            namespace_templates: strategy.namespace_templates,
            configuration: strategy.configuration.map(custom_configuration),
            memory_record_schema: strategy.memory_record_schema,
            record_expiry: strategy.record_expiry_specification,
        },
    }
}

/// A custom strategy's requested `CustomConfigurationInput` as the
/// `StrategyConfiguration` every operation that reports a strategy carries:
/// `type` names the override the request chose, and each prompt/model override
/// lands under the member of that shape which reports it.
fn custom_configuration(requested: CustomConfigurationInput) -> StrategyConfiguration {
    let mut configuration = StrategyConfiguration::default();
    match requested {
        CustomConfigurationInput::semanticOverride(override_input) => {
            configuration.r#type = Some(OVERRIDE_TYPE_SEMANTIC.to_string());
            configuration.extraction = override_input.extraction.map(|extraction| {
                ExtractionConfiguration::customExtractionConfiguration(
                    CustomExtractionConfiguration::semanticExtractionOverride(
                        SemanticExtractionOverride {
                            append_to_prompt: extraction.append_to_prompt,
                            model_id: extraction.model_id,
                        },
                    ),
                )
            });
            configuration.consolidation = override_input.consolidation.map(|consolidation| {
                ConsolidationConfiguration::customConsolidationConfiguration(
                    CustomConsolidationConfiguration::semanticConsolidationOverride(
                        SemanticConsolidationOverride {
                            append_to_prompt: consolidation.append_to_prompt,
                            model_id: consolidation.model_id,
                        },
                    ),
                )
            });
        }
        CustomConfigurationInput::summaryOverride(override_input) => {
            configuration.r#type = Some(OVERRIDE_TYPE_SUMMARY.to_string());
            configuration.consolidation = override_input.consolidation.map(|consolidation| {
                ConsolidationConfiguration::customConsolidationConfiguration(
                    CustomConsolidationConfiguration::summaryConsolidationOverride(
                        SummaryConsolidationOverride {
                            append_to_prompt: consolidation.append_to_prompt,
                            model_id: consolidation.model_id,
                        },
                    ),
                )
            });
        }
        CustomConfigurationInput::userPreferenceOverride(override_input) => {
            configuration.r#type = Some(OVERRIDE_TYPE_USER_PREFERENCE.to_string());
            configuration.extraction = override_input.extraction.map(|extraction| {
                ExtractionConfiguration::customExtractionConfiguration(
                    CustomExtractionConfiguration::userPreferenceExtractionOverride(
                        UserPreferenceExtractionOverride {
                            append_to_prompt: extraction.append_to_prompt,
                            model_id: extraction.model_id,
                        },
                    ),
                )
            });
            configuration.consolidation = override_input.consolidation.map(|consolidation| {
                ConsolidationConfiguration::customConsolidationConfiguration(
                    CustomConsolidationConfiguration::userPreferenceConsolidationOverride(
                        UserPreferenceConsolidationOverride {
                            append_to_prompt: consolidation.append_to_prompt,
                            model_id: consolidation.model_id,
                        },
                    ),
                )
            });
        }
        CustomConfigurationInput::episodicOverride(override_input) => {
            configuration.r#type = Some(OVERRIDE_TYPE_EPISODIC.to_string());
            configuration.extraction = override_input.extraction.map(|extraction| {
                ExtractionConfiguration::customExtractionConfiguration(
                    CustomExtractionConfiguration::episodicExtractionOverride(
                        EpisodicExtractionOverride {
                            append_to_prompt: extraction.append_to_prompt,
                            model_id: extraction.model_id,
                        },
                    ),
                )
            });
            configuration.consolidation = override_input.consolidation.map(|consolidation| {
                ConsolidationConfiguration::customConsolidationConfiguration(
                    CustomConsolidationConfiguration::episodicConsolidationOverride(
                        EpisodicConsolidationOverride {
                            append_to_prompt: consolidation.append_to_prompt,
                            model_id: consolidation.model_id,
                        },
                    ),
                )
            });
            configuration.reflection = override_input.reflection.map(|reflection| {
                ReflectionConfiguration::customReflectionConfiguration(
                    CustomReflectionConfiguration::episodicReflectionOverride(
                        EpisodicReflectionOverride {
                            append_to_prompt: reflection.append_to_prompt,
                            model_id: reflection.model_id,
                            namespaces: reflection.namespaces,
                            namespace_templates: reflection.namespace_templates,
                            memory_record_schema: reflection.memory_record_schema,
                        },
                    ),
                )
            });
        }
        CustomConfigurationInput::selfManagedConfiguration(self_managed) => {
            configuration.r#type = Some(OVERRIDE_TYPE_SELF_MANAGED.to_string());
            configuration.self_managed_configuration = Some(SelfManagedConfiguration {
                trigger_conditions: self_managed
                    .trigger_conditions
                    .unwrap_or_default()
                    .into_iter()
                    .map(trigger_condition)
                    .collect(),
                invocation_configuration: InvocationConfiguration {
                    topic_arn: self_managed.invocation_configuration.topic_arn,
                    payload_delivery_bucket_name: self_managed
                        .invocation_configuration
                        .payload_delivery_bucket_name,
                },
                historical_context_window_size: self_managed.historical_context_window_size,
            });
        }
    }
    configuration
}

/// One requested `TriggerConditionInput` as the `TriggerCondition` a stored
/// self-managed configuration reports.
pub fn trigger_condition(requested: TriggerConditionInput) -> TriggerCondition {
    match requested {
        TriggerConditionInput::messageBasedTrigger(trigger) => {
            TriggerCondition::messageBasedTrigger(MessageBasedTrigger {
                message_count: Some(trigger.message_count),
            })
        }
        TriggerConditionInput::tokenBasedTrigger(trigger) => {
            TriggerCondition::tokenBasedTrigger(TokenBasedTrigger {
                token_count: Some(trigger.token_count),
            })
        }
        TriggerConditionInput::timeBasedTrigger(trigger) => {
            TriggerCondition::timeBasedTrigger(TimeBasedTrigger {
                idle_session_timeout: Some(trigger.idle_session_timeout),
            })
        }
    }
}

/// One requested strategy after everything that does NOT need the store has
/// been resolved: the type and members to report, the namespace list the rule
/// gives it, and its validated record expiry.
///
/// Prepared BEFORE the state guard is taken, so none of the caller-sized work a
/// strategy list implies runs while this service's one store is locked.
pub struct PreparedStrategy {
    pub strategy_type: &'static str,
    pub name: String,
    pub description: Option<String>,
    pub configuration: Option<StrategyConfiguration>,
    pub memory_record_schema: Option<MemoryRecordSchema>,
    pub namespaces: Vec<String>,
    pub record_expiry_duration_days: Option<i32>,
}

/// Every requested strategy, read off its union member and validated — the same
/// walk whether the list creates a memory or is added to one.
///
/// A list longer than [`MAX_STRATEGIES_PER_MEMORY`] is refused BEFORE any entry
/// is processed, so the per-request work (and the strategies a stored memory
/// retains) is bounded by that ceiling rather than by the request body's size.
pub fn prepared_strategies(
    requested: Option<Vec<MemoryStrategyInput>>,
) -> Result<Vec<PreparedStrategy>, StrategyRefusal> {
    let requested = requested.unwrap_or_default();
    if requested.len() > MAX_STRATEGIES_PER_MEMORY {
        return Err(StrategyRefusal::QuotaExceeded(ServiceQuotaExceededException {
            message: Some(format!(
                "a memory holds at most {} strategies, and this request asked for {}",
                MAX_STRATEGIES_PER_MEMORY,
                requested.len()
            )),
        }));
    }
    let mut prepared = Vec::with_capacity(requested.len());
    for one in requested {
        let one = requested_strategy(one);
        let namespaces = effective_namespaces(&one);
        prepared.push(PreparedStrategy {
            strategy_type: one.strategy_type,
            name: one.name,
            description: one.description,
            configuration: one.configuration,
            memory_record_schema: one.memory_record_schema,
            namespaces,
            record_expiry_duration_days: record_expiry_duration_days(one.record_expiry)
                .map_err(StrategyRefusal::Validation)?,
        });
    }
    Ok(prepared)
}

/// A prepared strategy as the `MemoryStrategy` the record stores and the wire
/// reports, under the `strategy_id` minted for it.
///
/// `status` is left unset: it is the clock's, computed on every read of the
/// memory from the ids its last mutation minted and removed.
pub fn memory_strategy(
    prepared: PreparedStrategy,
    strategy_id: String,
    now: SystemTime,
) -> MemoryStrategy {
    MemoryStrategy {
        strategy_id,
        name: prepared.name,
        description: prepared.description,
        configuration: prepared.configuration,
        r#type: prepared.strategy_type.to_string(),
        namespaces: prepared.namespaces.clone(),
        namespace_templates: prepared.namespaces,
        created_at: Some(timestamp(now)),
        updated_at: Some(timestamp(now)),
        status: None,
        memory_record_schema: prepared.memory_record_schema,
        record_expiry_duration_days: prepared.record_expiry_duration_days,
        // Carried only by a READ of the memory ([`MemoryRecord::memory_read`]).
        extraction_health: None,
    }
}
