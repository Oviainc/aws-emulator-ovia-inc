Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# ExecuteQuery's EntityGraph carries no entity-hierarchy relationships, so a parent/child MATCH selects nothing

_Filed: 2026-10-24T03:47:17Z_
_Operation: ExecuteQuery | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The graph ExecuteQuery matches against is built only from relationship PROPERTY VALUES; the entity hierarchy that `parentEntityId` records is not represented in it at all.

Expected (real IoT TwinMaker knowledge graph): the EntityGraph exposes the workspace's entity hierarchy as a built-in relationship (`isChildOf`) between a child entity node and its parent entity node, so `SELECT child, parent FROM EntityGraph MATCH (child)-[:isChildOf]->(parent)` returns one row per stored parent/child pair, and an unlabelled hop `MATCH (a)-[r]->(b)` over entities that are only related by hierarchy also returns those pairs (with `r.relationshipName` reporting the hierarchy relationship).

Actual (emulator): `Graph::build` walks each entity's components and emits an edge ONLY for a property whose stored value carries a `relationshipValue` with a resolvable `targetEntityId` (src/ops/execute_query.rs:1382-1416). `Entity::parent_entity_id` is never read by this handler — no file under src/ops reads it except get_entity/list_entities — so a hierarchy query answers HTTP 200 with `rows: []` (an empty, apparently-complete answer; relationship labels are not validated against the graph, so an unknown label is silently matched by nothing rather than refused). A workspace built with `CreateEntity(parentEntityId=...)` and no RELATIONSHIP properties therefore has an EntityGraph with zero edges.

## Evidence

Spec/grounding: the model binds `CreateEntity.parentEntityId` (ParentEntityId, with the `$ROOT` sentinel) and reports it on `GetEntity`/`EntitySummary` plus a derived `hasChildEntities`, so the hierarchy is first-class service state; ExecuteQuery is documented as querying that state ("Run queries to access information from your knowledge graph of entities within individual workspaces"), and AWS's knowledge-graph documentation/query examples traverse the entity hierarchy through the built-in `isChildOf` relationship — it is not a relationship the caller has to model as a property.

Handler: src/ops/execute_query.rs — `Graph::build` (edge loop at lines 1382-1416) derives `edges` exclusively from `component.properties[*].value.relationship_value`; `GraphEdge.relationship_name` is the property name. No hierarchy edge is ever pushed, and `Matcher`'s edge-label filter (lines ~1783-1789) only compares against those property-derived names.

Tests: tests/all/execute_query.rs `a_match_pattern_selects_the_stored_relationships` / `an_edge_column_reports_the_stored_relationship` seed relationship PROPERTY values only; no test exercises a hierarchy hop, so the gap is uncovered rather than asserted either way.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateEntity(workspaceId="ws", entityId="line", entityName="Line").
3. CreateEntity(workspaceId="ws", entityId="mixer", entityName="Mixer", parentEntityId="line") — neither entity carries any RELATIONSHIP property.
4. GetEntity("ws","line") confirms hasChildEntities=true, so the hierarchy is stored.
5. ExecuteQuery(workspaceId="ws", queryStatement="SELECT child.entityId, parent.entityId FROM EntityGraph MATCH (child)-[:isChildOf]->(parent)").
6. Expected (AWS): 200 with one row binding child=mixer, parent=line.
7. Actual (emulator): 200 with rows=[] (columnDescriptions reported, no rows), and the same empty answer for an unlabelled hop "SELECT a.entityId, r.relationshipName, b.entityId FROM EntityGraph MATCH (a)-[r]->(b)".
