use codegraph_core::error::GraphError;
use codegraph_core::types::DddModelGraph;

use super::gql::CountingSession;
use super::gql::{escape_gql, opt_str};
use crate::engine::GrafeoEngine;

fn json_string<T: serde::Serialize>(value: &T) -> Result<String, GraphError> {
    serde_json::to_string(value).map_err(|e| GraphError::Ingest(e.to_string()))
}

impl GrafeoEngine {
    pub(super) async fn insert_ddd_model(&self, model: &DddModelGraph) -> Result<(), GraphError> {
        let session = CountingSession::new(self.db().session());

        let gql = format!(
            "INSERT (:DddApplication {{ name: '{}', base: {}, source_path: '{}' }})",
            escape_gql(&model.application.name),
            opt_str(&model.application.base),
            escape_gql(&model.application.source_path),
        );
        session
            .execute(&gql)
            .map_err(|e| GraphError::Ingest(format!("ingest_ddd_model application failed: {e}")))?;

        for module in &model.modules {
            let gql = format!(
                "INSERT (:DddModule {{ application: '{}', name: '{}', ordinal: {} }})",
                escape_gql(&module.application),
                escape_gql(&module.name),
                module.ordinal,
            );
            session
                .execute(&gql)
                .map_err(|e| GraphError::Ingest(format!("ingest_ddd_model module failed: {e}")))?;
            let edge = format!(
                "MATCH (a:DddApplication {{name: '{}'}}), \
                 (m:DddModule {{application: '{}', name: '{}', ordinal: {}}}) \
                 INSERT (a)-[:DddHasModule]->(m)",
                escape_gql(&model.application.name),
                escape_gql(&module.application),
                escape_gql(&module.name),
                module.ordinal,
            );
            session.execute(&edge).map_err(|e| {
                GraphError::Ingest(format!("ingest_ddd_model DddHasModule failed: {e}"))
            })?;
        }

        for design in &model.designs {
            let flags_json = json_string(&design.flags)?;
            let gql = format!(
                "INSERT (:DddDesign {{ application: '{}', module: '{}', class: '{}', \
                 resolved_title: {}, stereotype: '{}', is_abstract: {}, \
                 flags_json: '{}', ordinal: {} }})",
                escape_gql(&design.application),
                escape_gql(&design.module),
                escape_gql(&design.class),
                opt_str(&design.resolved_title),
                escape_gql(&design.stereotype),
                design.is_abstract,
                escape_gql(&flags_json),
                design.ordinal,
            );
            session
                .execute(&gql)
                .map_err(|e| GraphError::Ingest(format!("ingest_ddd_model design failed: {e}")))?;
            let edge = format!(
                "MATCH (m:DddModule {{application: '{}', name: '{}'}}), \
                 (d:DddDesign {{application: '{}', module: '{}', class: '{}', ordinal: {}}}) \
                 INSERT (m)-[:DddHasDesign]->(d)",
                escape_gql(&design.application),
                escape_gql(&design.module),
                escape_gql(&design.application),
                escape_gql(&design.module),
                escape_gql(&design.class),
                design.ordinal,
            );
            session.execute(&edge).map_err(|e| {
                GraphError::Ingest(format!("ingest_ddd_model DddHasDesign failed: {e}"))
            })?;
            if let Some(title) = &design.resolved_title {
                // Advisory: a MATCH with no rows inserts nothing, so an
                // unresolved title silently skips the edge.
                let edge = format!(
                    "MATCH (d:DddDesign {{application: '{}', module: '{}', class: '{}', ordinal: {}}}), \
                     (s:Schema {{title: '{}'}}) \
                     INSERT (d)-[:DddBindsClass]->(s)",
                    escape_gql(&design.application),
                    escape_gql(&design.module),
                    escape_gql(&design.class),
                    design.ordinal,
                    escape_gql(title),
                );
                session.execute(&edge).map_err(|e| {
                    GraphError::Ingest(format!("ingest_ddd_model DddBindsClass failed: {e}"))
                })?;
            }
        }

        for repository in &model.repositories {
            let gql = format!(
                "INSERT (:DddRepository {{ application: '{}', name: '{}', design_class: '{}' }})",
                escape_gql(&repository.application),
                escape_gql(&repository.name),
                escape_gql(&repository.design_class),
            );
            session.execute(&gql).map_err(|e| {
                GraphError::Ingest(format!("ingest_ddd_model repository failed: {e}"))
            })?;
            let edge = format!(
                "MATCH (d:DddDesign {{application: '{}', class: '{}'}}), \
                 (r:DddRepository {{application: '{}', name: '{}'}}) \
                 INSERT (d)-[:DddHasRepository]->(r)",
                escape_gql(&repository.application),
                escape_gql(&repository.design_class),
                escape_gql(&repository.application),
                escape_gql(&repository.name),
            );
            session.execute(&edge).map_err(|e| {
                GraphError::Ingest(format!("ingest_ddd_model DddHasRepository failed: {e}"))
            })?;

            for op in &repository.operations {
                let return_type = op.return_type.as_ref().map(json_string).transpose()?;
                let return_multiplicity = op
                    .return_multiplicity
                    .as_ref()
                    .map(json_string)
                    .transpose()?;
                let params_json = json_string(&op.params)?;
                let gql = format!(
                    "INSERT (:DddRepositoryOperation {{ application: '{}', repository_name: '{}', \
                     name: '{}', builtin: {}, return_type_json: {}, \
                     return_multiplicity: {}, params_json: '{}', ordinal: {}, \
                     is_protected: {} }})",
                    escape_gql(&repository.application),
                    escape_gql(&repository.name),
                    escape_gql(&op.name),
                    opt_str(&op.builtin),
                    opt_str(&return_type),
                    opt_str(&return_multiplicity),
                    escape_gql(&params_json),
                    op.ordinal,
                    op.is_protected,
                );
                session.execute(&gql).map_err(|e| {
                    GraphError::Ingest(format!("ingest_ddd_model repository operation failed: {e}"))
                })?;
                let edge = format!(
                    "MATCH (r:DddRepository {{application: '{}', name: '{}'}}), \
                     (o:DddRepositoryOperation {{application: '{}', repository_name: '{}', \
                      name: '{}', ordinal: {}}}) \
                     INSERT (r)-[:DddHasOperation]->(o)",
                    escape_gql(&repository.application),
                    escape_gql(&repository.name),
                    escape_gql(&repository.application),
                    escape_gql(&repository.name),
                    escape_gql(&op.name),
                    op.ordinal,
                );
                session.execute(&edge).map_err(|e| {
                    GraphError::Ingest(format!("ingest_ddd_model DddHasOperation failed: {e}"))
                })?;
            }
        }

        for service in &model.services {
            let dependencies_json = json_string(&service.dependencies)?;
            let operations_json = json_string(&service.operations)?;
            let gql = format!(
                "INSERT (:DddService {{ application: '{}', module: '{}', name: '{}', \
                 description: {}, dependencies_json: '{}', operations_json: '{}', ordinal: {} }})",
                escape_gql(&service.application),
                escape_gql(&service.module),
                escape_gql(&service.name),
                opt_str(&service.description),
                escape_gql(&dependencies_json),
                escape_gql(&operations_json),
                service.ordinal,
            );
            session
                .execute(&gql)
                .map_err(|e| GraphError::Ingest(format!("ingest_ddd_model service failed: {e}")))?;
            let edge = format!(
                "MATCH (m:DddModule {{application: '{}', name: '{}'}}), \
                 (s:DddService {{application: '{}', module: '{}', name: '{}', ordinal: {}}}) \
                 INSERT (m)-[:DddHasService]->(s)",
                escape_gql(&service.application),
                escape_gql(&service.module),
                escape_gql(&service.application),
                escape_gql(&service.module),
                escape_gql(&service.name),
                service.ordinal,
            );
            session.execute(&edge).map_err(|e| {
                GraphError::Ingest(format!("ingest_ddd_model DddHasService failed: {e}"))
            })?;
        }

        for search in &model.searches {
            let text_json = json_string(&search.text)?;
            let filters_json = json_string(&search.filters)?;
            let sorts_json = json_string(&search.sorts)?;
            let document_json = json_string(&search.document)?;
            let pagination_json = search.pagination.as_ref().map(json_string).transpose()?;
            let capabilities_json = json_string(&search.capabilities)?;
            let gql = format!(
                "INSERT (:DddSearch {{ application: '{}', module: '{}', name: '{}', \
                 description: {}, entity_class: '{}', entity_title: {}, \
                 text_json: '{}', filters_json: '{}', sorts_json: '{}', document_json: '{}', \
                 ranking: {}, analyzer: {}, pagination_json: {}, capabilities_json: '{}', \
                 ordinal: {} }})",
                escape_gql(&search.application),
                escape_gql(&search.module),
                escape_gql(&search.name),
                opt_str(&search.description),
                escape_gql(&search.entity_class),
                opt_str(&search.entity_title),
                escape_gql(&text_json),
                escape_gql(&filters_json),
                escape_gql(&sorts_json),
                escape_gql(&document_json),
                opt_str(&search.ranking),
                opt_str(&search.analyzer),
                opt_str(&pagination_json),
                escape_gql(&capabilities_json),
                search.ordinal,
            );
            session
                .execute(&gql)
                .map_err(|e| GraphError::Ingest(format!("ingest_ddd_model search failed: {e}")))?;
            let edge = format!(
                "MATCH (m:DddModule {{application: '{}', name: '{}'}}), \
                 (x:DddSearch {{application: '{}', module: '{}', name: '{}', ordinal: {}}}) \
                 INSERT (m)-[:DddHasSearch]->(x)",
                escape_gql(&search.application),
                escape_gql(&search.module),
                escape_gql(&search.application),
                escape_gql(&search.module),
                escape_gql(&search.name),
                search.ordinal,
            );
            session.execute(&edge).map_err(|e| {
                GraphError::Ingest(format!("ingest_ddd_model DddHasSearch failed: {e}"))
            })?;
            if let Some(title) = &search.entity_title {
                let edge = format!(
                    "MATCH (x:DddSearch {{application: '{}', module: '{}', name: '{}', ordinal: {}}}), \
                     (s:Schema {{title: '{}'}}) \
                     INSERT (x)-[:DddBindsClass]->(s)",
                    escape_gql(&search.application),
                    escape_gql(&search.module),
                    escape_gql(&search.name),
                    search.ordinal,
                    escape_gql(title),
                );
                session.execute(&edge).map_err(|e| {
                    GraphError::Ingest(format!("ingest_ddd_model DddBindsClass failed: {e}"))
                })?;
            }
        }

        Ok(())
    }
}
