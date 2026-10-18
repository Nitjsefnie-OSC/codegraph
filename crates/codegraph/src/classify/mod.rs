pub mod auto_config;
pub mod naming_rules;
pub mod output;
pub mod scoring;

use codegraph_classifier::config::NamingRule;
use codegraph_config::config::DomainEntry;
use codegraph_core::types::SchemaClassificationData;
use scoring::{AutoClassification, ClassificationScore};
use std::collections::{HashMap, HashSet};

/// Result of auto-classification for an entire domain.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DomainClassificationResult {
    pub domain: String,
    pub entities: Vec<ClassificationScore>,
    pub value_objects: Vec<ClassificationScore>,
    pub excluded: Vec<String>,
}

/// The unified auto-classifier. Combines structural scoring, naming rules,
/// and manual overrides from domains.toml.
pub struct AutoClassifier {
    /// Types already handled by classifier.toml (primitive/array/range/composite wrappers)
    pub classifier_types: HashSet<String>,
    /// Naming-based classification rules from classifier.toml
    pub naming_rules: HashMap<String, NamingRule>,
}

impl AutoClassifier {
    pub fn new(
        classifier_types: HashSet<String>,
        naming_rules: HashMap<String, NamingRule>,
    ) -> Self {
        Self {
            classifier_types,
            naming_rules,
        }
    }

    /// Classify all schemas for a domain, applying overrides.
    #[cfg_attr(feature = "hotpath", hotpath::measure)]
    pub fn classify_domain(
        &self,
        domain_name: &str,
        domain_entry: &DomainEntry,
        schemas: &[SchemaClassificationData],
    ) -> DomainClassificationResult {
        let force_entities: HashSet<&str> = domain_entry
            .force_entities
            .iter()
            .map(|s| s.as_str())
            .collect();
        let force_vos: HashSet<&str> = domain_entry
            .force_value_objects
            .iter()
            .map(|s| s.as_str())
            .collect();
        let excludes: HashSet<&str> = domain_entry.exclude.iter().map(|s| s.as_str()).collect();

        let mut entities = Vec::new();
        let mut value_objects = Vec::new();
        let mut excluded = Vec::new();

        // Priority order is intentional: mox provenance → exclude → path → classifier →
        // force_entities → force_value_objects → scoring.
        // Mox-authored schemas are author-declared (issue #229): the `.mox`
        // class hierarchy already decides entity vs value object, so the
        // classifier must not re-derive it. They still join the entity /
        // value-object lists (with a provenance reason) so downstream
        // consumers see consistent counts.
        // Excludes are checked BEFORE force overrides so that explicitly excluded types
        // are never promoted back by a stale force_entities/force_value_objects entry.
        // If you need to un-exclude a type, remove it from `exclude` first.
        for data in schemas {
            let title = &data.title;

            // Priority 0: mox-authored (author-declared classification)
            if data.source.as_deref() == Some(codegraph_core::types::MOX_SOURCE) {
                let mut score = scoring::score_structural(data);
                score.reasons.push("override:source=mox".to_string());
                if data.is_entity {
                    score.classification = AutoClassification::Entity;
                    entities.push(score);
                } else {
                    score.classification = AutoClassification::ValueObject;
                    value_objects.push(score);
                }
                continue;
            }

            // Priority 1: Manual exclude
            if excludes.contains(title.as_str()) {
                excluded.push(title.clone());
                continue;
            }

            // Priority 2: Path-based exclusion
            if naming_rules::should_exclude_by_path(&data.rel_path) {
                excluded.push(title.clone());
                continue;
            }

            // Priority 3: classifier.toml types excluded
            if self.classifier_types.contains(title.as_str()) {
                excluded.push(title.clone());
                continue;
            }

            // Priority 4: Manual force_entities
            if force_entities.contains(title.as_str()) {
                let mut score = scoring::score_structural(data);
                score.classification = AutoClassification::Entity;
                score.reasons.push("override:force_entities".to_string());
                entities.push(score);
                continue;
            }

            // Priority 5: Manual force_value_objects
            if force_vos.contains(title.as_str()) {
                let mut score = scoring::score_structural(data);
                score.classification = AutoClassification::ValueObject;
                score
                    .reasons
                    .push("override:force_value_objects".to_string());
                value_objects.push(score);
                continue;
            }

            // Priority 6: Naming rules + structural scoring
            let mut score = scoring::score_structural(data);

            // If structural scoring already decided (hard VO), skip naming rules
            if score.classification == AutoClassification::ValueObject
                && score.reasons.iter().any(|r| r.starts_with("hard:"))
            {
                value_objects.push(score);
                continue;
            }

            // Apply naming rules
            if let Some(rule) = naming_rules::apply_naming_rules(title, &self.naming_rules) {
                if rule.is_hard {
                    score.vo_score += rule.vo_score;
                    score.net_score = score.entity_score - score.vo_score;
                    score.classification = AutoClassification::ValueObject;
                    score.reasons.push(rule.reason);
                    value_objects.push(score);
                    continue;
                }
                // Soft rule: add to VO score, re-evaluate threshold
                score.vo_score += rule.vo_score;
                score.net_score = score.entity_score - score.vo_score;
                score.reasons.push(rule.reason);
                score.classification = if score.net_score >= 4 {
                    AutoClassification::Entity
                } else {
                    AutoClassification::ValueObject
                };
            }

            match score.classification {
                AutoClassification::Entity => entities.push(score),
                AutoClassification::ValueObject => value_objects.push(score),
                AutoClassification::Excluded => excluded.push(title.clone()),
            }
        }

        DomainClassificationResult {
            domain: domain_name.to_string(),
            entities,
            value_objects,
            excluded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_data(
        title: &str,
        domain: &str,
        rel_path: &str,
        in_degree: usize,
        field_count: usize,
    ) -> SchemaClassificationData {
        SchemaClassificationData {
            namespace: None,
            title: title.to_string(),
            domain: Some(domain.to_string()),
            rel_path: rel_path.to_string(),
            schema_type: "object".to_string(),
            is_codelist: false,
            is_primitive_wrapper: false,
            has_all_of: false,
            composes_noun_type: true,
            field_count,
            required_field_count: field_count / 2,
            ref_count: 0,
            in_degree,
            is_enum: false,
            is_string_type: false,
            is_entity: false,
            source: None,
        }
    }

    fn make_domain_entry() -> DomainEntry {
        DomainEntry {
            label: "Test".to_string(),
            schema_dir: "test".to_string(),
            postgres_schema: "test".to_string(),
            depends_on: vec![],
            dependencies: Vec::new(),
            entities: vec![],
            entity_config: HashMap::new(),
            auto_discover: Some(true),
            exclude_entities: vec![],
            force_entities: vec!["ForcedEntity".to_string()],
            force_value_objects: vec!["ForcedVO".to_string()],
            exclude: vec!["ExcludedType".to_string()],
            auditable: None,
            tier: "extended".to_string(),
            worker_name: None,
            custom_domain: None,
            service_bindings: None,
            hyperdrive_binding: None,
            cron_triggers: None,
            remote_include_mode: None,
            webhooks: None,
            queue_name: None,
            queue_binding: None,
            queue_max_retries: None,
            queue_max_concurrency: None,
            observability: None,
            custom_routes: false,
        }
    }

    #[test]
    fn force_entities_overrides_low_score() {
        let classifier = AutoClassifier::new(HashSet::new(), HashMap::new());
        let schemas = vec![make_data(
            "ForcedEntity",
            "test",
            "test/json/ForcedEntity.json",
            0,
            2,
        )];
        let result = classifier.classify_domain("test", &make_domain_entry(), &schemas);
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].title, "ForcedEntity");
    }

    #[test]
    fn force_value_objects_overrides_high_score() {
        let classifier = AutoClassifier::new(HashSet::new(), HashMap::new());
        let schemas = vec![make_data(
            "ForcedVO",
            "test",
            "test/json/ForcedVO.json",
            10,
            20,
        )];
        let result = classifier.classify_domain("test", &make_domain_entry(), &schemas);
        assert_eq!(result.value_objects.len(), 1);
        assert_eq!(result.value_objects[0].title, "ForcedVO");
    }

    #[test]
    fn exclude_removes_from_results() {
        let classifier = AutoClassifier::new(HashSet::new(), HashMap::new());
        let schemas = vec![make_data(
            "ExcludedType",
            "test",
            "test/json/ExcludedType.json",
            5,
            15,
        )];
        let result = classifier.classify_domain("test", &make_domain_entry(), &schemas);
        assert!(result.entities.is_empty());
        assert!(result.value_objects.is_empty());
        assert_eq!(result.excluded, vec!["ExcludedType"]);
    }

    #[test]
    fn meta_path_excluded_automatically() {
        let classifier = AutoClassifier::new(HashSet::new(), HashMap::new());
        let entry = DomainEntry {
            force_entities: vec![],
            force_value_objects: vec![],
            exclude: vec![],
            ..make_domain_entry()
        };
        let schemas = vec![make_data(
            "hros",
            "test",
            "common/json/meta/hros.json",
            5,
            15,
        )];
        let result = classifier.classify_domain("test", &entry, &schemas);
        assert!(result.entities.is_empty());
        assert_eq!(result.excluded, vec!["hros"]);
    }

    #[test]
    fn classifier_toml_types_excluded() {
        let mut classifier_types = HashSet::new();
        classifier_types.insert("CodeType".to_string());
        let classifier = AutoClassifier::new(classifier_types, HashMap::new());
        let entry = DomainEntry {
            force_entities: vec![],
            force_value_objects: vec![],
            exclude: vec![],
            ..make_domain_entry()
        };
        let schemas = vec![make_data(
            "CodeType",
            "test",
            "test/json/CodeType.json",
            5,
            15,
        )];
        let result = classifier.classify_domain("test", &entry, &schemas);
        assert!(result.entities.is_empty());
        assert_eq!(result.excluded, vec!["CodeType"]);
    }

    // =========================================================================
    // Wave 0 regression tests: verify auto-classifier reproduces the expected
    // entity sets for wellness and timecard (the simplest domains).
    // =========================================================================

    fn empty_domain_entry(label: &str, schema_dir: &str) -> DomainEntry {
        DomainEntry {
            label: label.to_string(),
            schema_dir: schema_dir.to_string(),
            postgres_schema: schema_dir.to_string(),
            depends_on: vec![],
            dependencies: Vec::new(),
            entities: vec![],
            entity_config: HashMap::new(),
            auto_discover: Some(true),
            exclude_entities: vec![],
            force_entities: vec![],
            force_value_objects: vec![],
            exclude: vec![],
            auditable: None,
            tier: "extended".to_string(),
            worker_name: None,
            custom_domain: None,
            service_bindings: None,
            hyperdrive_binding: None,
            cron_triggers: None,
            remote_include_mode: None,
            webhooks: None,
            queue_name: None,
            queue_binding: None,
            queue_max_retries: None,
            queue_max_concurrency: None,
            observability: None,
            custom_routes: false,
        }
    }

    #[test]
    fn wave0_wellness_single_entity() {
        // WellnessType is the only schema in the wellness domain.
        // It should auto-classify as an entity: high field count, composes NounType.
        let classifier = AutoClassifier::new(HashSet::new(), HashMap::new());
        let entry = empty_domain_entry("Wellness", "wellness");
        let schemas = vec![SchemaClassificationData {
            namespace: None,
            title: "WellnessType".to_string(),
            domain: Some("wellness".to_string()),
            rel_path: "wellness/json/WellnessType.json".to_string(),
            schema_type: "object".to_string(),
            is_codelist: false,
            is_primitive_wrapper: false,
            has_all_of: true,
            composes_noun_type: true,
            field_count: 10,
            required_field_count: 2,
            ref_count: 3,
            in_degree: 0,
            is_enum: false,
            is_string_type: false,
            is_entity: false,
            source: None,
        }];
        let result = classifier.classify_domain("wellness", &entry, &schemas);
        let entity_names: Vec<&str> = result.entities.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            entity_names,
            vec!["WellnessType"],
            "Wave 0 regression: wellness should have exactly WellnessType as entity"
        );
        assert!(
            result.value_objects.is_empty(),
            "Wave 0 regression: wellness should have no value objects"
        );
    }

    #[test]
    fn wave0_timecard_single_entity() {
        // TimecardType is the only schema in the timecard domain.
        // It should auto-classify as an entity: high field count, composes NounType.
        let classifier = AutoClassifier::new(HashSet::new(), HashMap::new());
        let entry = empty_domain_entry("Timecard", "timecard");
        let schemas = vec![SchemaClassificationData {
            namespace: None,
            title: "TimecardType".to_string(),
            domain: Some("timecard".to_string()),
            rel_path: "timecard/json/TimecardType.json".to_string(),
            schema_type: "object".to_string(),
            is_codelist: false,
            is_primitive_wrapper: false,
            has_all_of: true,
            composes_noun_type: true,
            field_count: 12,
            required_field_count: 3,
            ref_count: 5,
            in_degree: 0,
            is_enum: false,
            is_string_type: false,
            is_entity: false,
            source: None,
        }];
        let result = classifier.classify_domain("timecard", &entry, &schemas);
        let entity_names: Vec<&str> = result.entities.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            entity_names,
            vec!["TimecardType"],
            "Wave 0 regression: timecard should have exactly TimecardType as entity"
        );
        assert!(
            result.value_objects.is_empty(),
            "Wave 0 regression: timecard should have no value objects"
        );
    }

    #[test]
    fn wave0_wellness_sample_excluded() {
        // Sample files in the wellness domain should be excluded by path rule.
        let classifier = AutoClassifier::new(HashSet::new(), HashMap::new());
        let entry = empty_domain_entry("Wellness", "wellness");
        let schemas = vec![
            SchemaClassificationData {
                namespace: None,
                title: "WellnessType".to_string(),
                domain: Some("wellness".to_string()),
                rel_path: "wellness/json/WellnessType.json".to_string(),
                schema_type: "object".to_string(),
                is_codelist: false,
                is_primitive_wrapper: false,
                has_all_of: true,
                composes_noun_type: true,
                field_count: 10,
                required_field_count: 2,
                ref_count: 3,
                in_degree: 0,
                is_enum: false,
                is_string_type: false,
                is_entity: false,
                source: None,
            },
            SchemaClassificationData {
                namespace: None,
                title: "activity_response_daily".to_string(),
                domain: Some("wellness".to_string()),
                rel_path: "wellness/json/samples/activity_response_daily.json".to_string(),
                schema_type: "object".to_string(),
                is_codelist: false,
                is_primitive_wrapper: false,
                has_all_of: false,
                composes_noun_type: false,
                field_count: 5,
                required_field_count: 1,
                ref_count: 0,
                in_degree: 0,
                is_enum: false,
                is_string_type: false,
                is_entity: false,
                source: None,
            },
        ];
        let result = classifier.classify_domain("wellness", &entry, &schemas);
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].title, "WellnessType");
        assert!(
            result
                .excluded
                .contains(&"activity_response_daily".to_string()),
            "Sample files should be excluded by path rule"
        );
    }

    #[test]
    fn naming_rule_soft_vo_can_be_overridden_by_force() {
        let classifier = AutoClassifier::new(HashSet::new(), HashMap::new());
        let entry = DomainEntry {
            force_entities: vec!["ScreeningReportType".to_string()],
            force_value_objects: vec![],
            exclude: vec![],
            ..make_domain_entry()
        };
        let schemas = vec![make_data(
            "ScreeningReportType",
            "test",
            "test/json/ScreeningReportType.json",
            3,
            10,
        )];
        let result = classifier.classify_domain("test", &entry, &schemas);
        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].title, "ScreeningReportType");
    }
}
