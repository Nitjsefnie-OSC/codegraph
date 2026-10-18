//! DDD design-plane ingest tests (issue #449 Phase 2): the rex-ir to
//! `DddModelGraph` conversion (module-major ordinals, every payload),
//! class title resolution (exact, then +type_suffix, then miss), the
//! end-to-end `ingest_ddd_files` run over a fixture dir (mox import,
//! `import schema`, and `import sigil` with the sibling candidate pool),
//! hard-error semantics, and the graph-cache inputs group.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use codegraph_core::mock::MockEngine;
use codegraph_core::traits::GraphQuerier;
use codegraph_core::types::{
    DddApplicationNode, DddDesignFlags, DddDocumentField, DddModuleNode, DddPagination,
    DddRepositoryNode, DddSearchField, SchemaNode,
};
use rex_ir::ddd::{
    Application, BuiltinRepositoryOp, DddModel, Delegation, Design, DesignFlags, Module,
    RankingStrategy, Repository, RepositoryOperation, SearchDef, SearchField, SearchFilter,
    SearchSort, Service, ServiceOperation, Stereotype,
};
use rex_ir::{Multiplicity, OperationParam, PrimitiveType, TypeRef, Upper};

// ── rex-ir fixture ─────────────────────────────────────────────────────

fn class_ref(package: &str, name: &str) -> TypeRef {
    TypeRef::Class {
        package: package.to_string(),
        name: name.to_string(),
    }
}

fn param(name: &str, type_: TypeRef, multiplicity: Option<Multiplicity>) -> OperationParam {
    OperationParam {
        name: name.to_string(),
        type_,
        multiplicity,
    }
}

/// Two modules: `lending` (entity/value/dto designs — the entity carries
/// all five flags and a repository with the four builtins plus a declared
/// finder — and a service with a declared op and a delegation), and
/// `discovery` (one more entity design plus a fully-loaded search).
fn sample_model() -> DddModel {
    DddModel::new()
        .application(Application {
            name: "Library".to_string(),
            base: Some("nz.example.library".to_string()),
        })
        .module(Module {
            name: "lending".to_string(),
            designs: vec![
                Design {
                    class: "nz.example.library::Book".to_string(),
                    stereotype: Stereotype::Entity,
                    is_abstract: false,
                    flags: DesignFlags {
                        scaffold: true,
                        auditable: true,
                        optimistic_locking: true,
                        non_persistent: false,
                        cache: true,
                    },
                    repository: Some(Repository {
                        name: "BookRepository".to_string(),
                        operations: vec![
                            RepositoryOperation::builtin("findById", BuiltinRepositoryOp::FindById),
                            RepositoryOperation::builtin("findAll", BuiltinRepositoryOp::FindAll),
                            RepositoryOperation::builtin("save", BuiltinRepositoryOp::Save),
                            RepositoryOperation::builtin("delete", BuiltinRepositoryOp::Delete),
                            RepositoryOperation::declared(
                                "byTitle",
                                Some(TypeRef::Primitive(PrimitiveType::String)),
                                vec![param(
                                    "title",
                                    TypeRef::Primitive(PrimitiveType::String),
                                    Some(Multiplicity::MANY),
                                )],
                            )
                            .with_return_multiplicity(Some(
                                Multiplicity {
                                    lower: 0,
                                    upper: Upper::Finite(5),
                                },
                            )),
                        ],
                    }),
                },
                Design {
                    class: "Money".to_string(),
                    stereotype: Stereotype::Value,
                    is_abstract: false,
                    flags: DesignFlags::default(),
                    repository: None,
                },
                Design {
                    class: "shop::reporting::LoanSummary".to_string(),
                    stereotype: Stereotype::Dto,
                    is_abstract: true,
                    flags: DesignFlags {
                        scaffold: false,
                        auditable: false,
                        optimistic_locking: false,
                        non_persistent: true,
                        cache: false,
                    },
                    repository: None,
                },
            ],
            services: vec![Service {
                name: "LoanService".to_string(),
                description: Some("Coordinates lending.".to_string()),
                operations: vec![
                    ServiceOperation {
                        name: "borrow".to_string(),
                        return_type: Some(TypeRef::Primitive(PrimitiveType::Boolean)),
                        return_multiplicity: None,
                        params: vec![param("book", class_ref("nz.example.library", "Book"), None)],
                        delegation: None,
                        capabilities: vec!["BorrowBooks".to_string()],
                        is_protected: false,
                    },
                    ServiceOperation {
                        name: "renew".to_string(),
                        return_type: None,
                        return_multiplicity: None,
                        params: Vec::new(),
                        delegation: Some(Delegation {
                            target: "LoanRepository".to_string(),
                            operation: "save".to_string(),
                        }),
                        capabilities: Vec::new(),
                        is_protected: false,
                    },
                ],
                dependencies: vec![
                    "LoanRepository".to_string(),
                    "NotificationService".to_string(),
                ],
            }],
            searches: Vec::new(),
        })
        .module(Module {
            name: "discovery".to_string(),
            designs: vec![Design {
                class: "nz.example.library.Chapter".to_string(),
                stereotype: Stereotype::Entity,
                is_abstract: false,
                flags: DesignFlags::default(),
                repository: None,
            }],
            services: Vec::new(),
            searches: vec![SearchDef {
                name: "BookSearch".to_string(),
                description: Some("Full-text catalogue search".to_string()),
                entity: "nz.example.library::Book".to_string(),
                text: vec![
                    SearchField {
                        property: "title".to_string(),
                        boost: Some(2.5.into()),
                        analyzer: Some("standard".to_string()),
                    },
                    SearchField {
                        property: "synopsis".to_string(),
                        boost: None,
                        analyzer: None,
                    },
                ],
                filters: vec![SearchFilter {
                    property: "category".to_string(),
                }],
                sort: vec![SearchSort {
                    property: "title".to_string(),
                }],
                document: vec![rex_ir::ddd::DocumentField {
                    name: "label".to_string(),
                    expr: r#"title + " - " + synopsis"#.to_string(),
                }],
                ranking: Some(RankingStrategy::Custom("recency".to_string())),
                analyzer: Some("english".to_string()),
                pagination: Some(rex_ir::ddd::Pagination {
                    limit: Some(20),
                    max_limit: Some(100),
                    cursor: true,
                }),
                capabilities: vec!["SearchBooks".to_string()],
            }],
        })
}

/// Book resolves via the +`Type` suffix (the mox bridge convention), Money
/// is exact, everything else is unresolved.
fn sample_resolver() -> impl Fn(&str) -> Option<String> {
    |name: &str| match name {
        "Book" => Some("BookType".to_string()),
        "Money" => Some("Money".to_string()),
        _ => None,
    }
}

fn type_json(value: &TypeRef) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

fn mult_json(value: &Multiplicity) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

// ── conversion ─────────────────────────────────────────────────────────

#[test]
fn conversion_maps_module_major_order_and_every_payload() {
    let model = sample_model();
    let graph = codegraph::ingest::ddd_ingest::ddd_model_graph_from_rex(
        &model,
        "design.ddd",
        &sample_resolver(),
    );

    assert_eq!(graph.source_path, "design.ddd");
    assert_eq!(
        graph.application,
        DddApplicationNode {
            name: "Library".to_string(),
            base: Some("nz.example.library".to_string()),
            source_path: "design.ddd".to_string(),
        }
    );

    // Modules in declaration order with their own ordinals.
    assert_eq!(
        graph.modules,
        vec![
            DddModuleNode {
                application: "Library".to_string(),
                name: "lending".to_string(),
                ordinal: 0,
            },
            DddModuleNode {
                application: "Library".to_string(),
                name: "discovery".to_string(),
                ordinal: 1,
            },
        ]
    );

    // Designs: module-major (lending's three, then discovery's one), with
    // per-module ordinals and resolved titles from the resolver.
    assert_eq!(graph.designs.len(), 4);
    let book = &graph.designs[0];
    assert_eq!(book.module, "lending");
    assert_eq!(book.class, "nz.example.library::Book");
    assert_eq!(book.resolved_title.as_deref(), Some("BookType"));
    assert_eq!(book.stereotype, "entity");
    assert!(!book.is_abstract);
    assert!(book.flags.scaffold && book.flags.auditable && book.flags.optimistic_locking);
    assert!(!book.flags.non_persistent && book.flags.cache);
    assert_eq!(book.ordinal, 0);

    let money = &graph.designs[1];
    assert_eq!(money.stereotype, "value");
    assert_eq!(money.resolved_title.as_deref(), Some("Money"));
    assert_eq!(money.flags, DddDesignFlags::default());
    assert_eq!(money.ordinal, 1);

    let summary = &graph.designs[2];
    assert_eq!(summary.stereotype, "dto");
    assert_eq!(summary.class, "shop::reporting::LoanSummary");
    assert_eq!(summary.resolved_title, None, "resolver miss → None");
    assert!(summary.is_abstract);
    assert!(summary.flags.non_persistent);
    assert_eq!(summary.ordinal, 2);

    let chapter = &graph.designs[3];
    assert_eq!(chapter.module, "discovery");
    assert_eq!(chapter.class, "nz.example.library.Chapter");
    assert_eq!(chapter.ordinal, 0, "ordinals restart per module");

    // Repositories: one, in module-major position, with builtin labels and
    // the declared finder's full signature JSON.
    assert_eq!(graph.repositories.len(), 1);
    let repo = &graph.repositories[0];
    assert_eq!(
        repo,
        &DddRepositoryNode {
            application: "Library".to_string(),
            name: "BookRepository".to_string(),
            design_class: "nz.example.library::Book".to_string(),
            operations: vec![
                rex_ir_builtin("findById", BuiltinRepositoryOp::FindById, 0),
                rex_ir_builtin("findAll", BuiltinRepositoryOp::FindAll, 1),
                rex_ir_builtin("save", BuiltinRepositoryOp::Save, 2),
                rex_ir_builtin("delete", BuiltinRepositoryOp::Delete, 3),
                codegraph_core::types::DddRepositoryOperation {
                    name: "byTitle".to_string(),
                    builtin: None,
                    return_type: Some(type_json(&TypeRef::Primitive(PrimitiveType::String))),
                    return_multiplicity: Some(mult_json(&Multiplicity {
                        lower: 0,
                        upper: Upper::Finite(5)
                    })),
                    params: vec![codegraph_core::types::DddParam {
                        name: "title".to_string(),
                        type_json: type_json(&TypeRef::Primitive(PrimitiveType::String)),
                        multiplicity: Some(mult_json(&Multiplicity::MANY)),
                    }],
                    ordinal: 4,
                    is_protected: false,
                },
            ],
        }
    );

    // Services: module-major, delegation split into target/operation,
    // capabilities verbatim.
    assert_eq!(graph.services.len(), 1);
    let service = &graph.services[0];
    assert_eq!(service.module, "lending");
    assert_eq!(service.ordinal, 0);
    assert_eq!(
        service.dependencies,
        ["LoanRepository", "NotificationService"]
    );
    let borrow = &service.operations[0];
    assert_eq!(
        borrow.return_type,
        Some(type_json(&TypeRef::Primitive(PrimitiveType::Boolean)))
    );
    assert_eq!(
        borrow.params,
        vec![codegraph_core::types::DddParam {
            name: "book".to_string(),
            type_json: type_json(&class_ref("nz.example.library", "Book")),
            multiplicity: None,
        }]
    );
    let renew = &service.operations[1];
    assert_eq!(renew.delegation_target.as_deref(), Some("LoanRepository"));
    assert_eq!(renew.delegation_operation.as_deref(), Some("save"));
    assert!(renew.return_type.is_none());
    assert_eq!(renew.ordinal, 1);

    // Searches: the discovery module's search carries every knob.
    assert_eq!(graph.searches.len(), 1);
    let search = &graph.searches[0];
    assert_eq!(search.module, "discovery");
    assert_eq!(search.ordinal, 0);
    assert_eq!(search.entity_class, "nz.example.library::Book");
    assert_eq!(search.entity_title.as_deref(), Some("BookType"));
    assert_eq!(
        search.text,
        vec![
            DddSearchField {
                property: "title".to_string(),
                boost: Some(2.5),
                analyzer: Some("standard".to_string()),
            },
            DddSearchField {
                property: "synopsis".to_string(),
                boost: None,
                analyzer: None,
            },
        ]
    );
    assert_eq!(search.filters, vec!["category".to_string()]);
    assert_eq!(search.sorts, vec!["title".to_string()]);
    assert_eq!(
        search.document,
        vec![DddDocumentField {
            name: "label".to_string(),
            expr: r#"title + " - " + synopsis"#.to_string(),
        }]
    );
    assert_eq!(search.ranking.as_deref(), Some("recency"));
    assert_eq!(search.analyzer.as_deref(), Some("english"));
    assert_eq!(
        search.pagination,
        Some(DddPagination {
            limit: Some(20),
            max_limit: Some(100),
            cursor: true,
        })
    );
    assert_eq!(search.capabilities, vec!["SearchBooks".to_string()]);
}

fn rex_ir_builtin(
    name: &str,
    builtin: BuiltinRepositoryOp,
    ordinal: usize,
) -> codegraph_core::types::DddRepositoryOperation {
    let label = match builtin {
        BuiltinRepositoryOp::FindById => "findById",
        BuiltinRepositoryOp::FindAll => "findAll",
        BuiltinRepositoryOp::FindByExample => "findByExample",
        BuiltinRepositoryOp::FindByKeys => "findByKeys",
        BuiltinRepositoryOp::Save => "save",
        BuiltinRepositoryOp::Delete => "delete",
    };
    codegraph_core::types::DddRepositoryOperation {
        name: name.to_string(),
        builtin: Some(label.to_string()),
        return_type: None,
        return_multiplicity: None,
        params: Vec::new(),
        ordinal,
        is_protected: false,
    }
}

#[test]
fn title_resolution_receives_the_short_name_and_hits_exact_suffix_and_miss() {
    let seen = std::cell::RefCell::new(Vec::new());
    let titles: HashSet<String> = ["BookType", "Money"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let resolver = |name: &str| -> Option<String> {
        seen.borrow_mut().push(name.to_string());
        codegraph::ingest::ddd_ingest::resolve_with_suffix(name, "Type", &titles)
    };

    let model = sample_model();
    let graph = codegraph::ingest::ddd_ingest::ddd_model_graph_from_rex(&model, "d.ddd", &resolver);

    // The resolver saw SHORT names: the `::`-qualified and dot-qualified
    // references were reduced to their last segment.
    assert_eq!(
        *seen.borrow(),
        vec![
            "Book".to_string(),
            "Money".to_string(),
            "LoanSummary".to_string(),
            "Chapter".to_string(),
            "Book".to_string(),
        ]
    );
    // Exact hit (Money), +type_suffix hit (Book → BookType), misses → None.
    assert_eq!(graph.designs[0].resolved_title.as_deref(), Some("BookType"));
    assert_eq!(graph.designs[1].resolved_title.as_deref(), Some("Money"));
    assert_eq!(graph.designs[2].resolved_title, None);
    assert_eq!(graph.searches[0].entity_title.as_deref(), Some("BookType"));
}

// ── end-to-end ingest ──────────────────────────────────────────────────

const LIB_MOX: &str = concat!(
    "package shop\n",
    "\n",
    "import schema \"x.json\" as Widget\n",
    "import sigil \"y.rosetta\"\n",
    "\n",
    "class Product {\n",
    "    String title\n",
    "    refers Widget widget\n",
    "}\n",
    "\n",
    "class Money {\n",
    "    readonly int cents\n",
    "}\n",
);

/// Imports a namespace that only exists in the SIBLING pool file
/// (`z.rosetta`, never named by any import): a clean compile proves the
/// candidate-pool discovery ran.
const Y_ROSETTA: &str = concat!(
    "namespace shop.notes\n",
    "version \"1.0.0\"\n",
    "\n",
    "import shop.other.*\n",
    "\n",
    "type Note:\n",
    "    label string (1..1)\n",
    "    tag shop.other.Tag (1..1)\n",
);

const Z_ROSETTA: &str = concat!(
    "namespace shop.other\n",
    "version \"1.0.0\"\n",
    "\n",
    "type Tag:\n",
    "    name string (1..1)\n",
);

const X_JSON: &str = r#"{"title": "Widget"}"#;

const DESIGN_DDD: &str = concat!(
    "import \"lib.mox\"\n",
    "\n",
    "application Shop {\n",
    "    base shop\n",
    "\n",
    "    module catalogue {\n",
    "        entity Product repository ProductRepository {\n",
    "            findById;\n",
    "            findAll;\n",
    "            save;\n",
    "            delete;\n",
    "            Money pricedAbove(int cents);\n",
    "        }\n",
    "        value Money nonPersistent\n",
    "        search ProductSearch {\n",
    "            entity Product\n",
    "            text {\n",
    "                title boost 3\n",
    "            }\n",
    "            filters {\n",
    "                title\n",
    "            }\n",
    "            sort {\n",
    "                title\n",
    "            }\n",
    "            document {\n",
    "                label = title;\n",
    "            }\n",
    "            ranking custom \"recency\"\n",
    "            pagination {\n",
    "                limit 10\n",
    "                max 50\n",
    "                cursor\n",
    "            }\n",
    "            capability SearchProducts\n",
    "        }\n",
    "        service ProductService {\n",
    "            inject ProductRepository;\n",
    "            find => ProductRepository.findById;\n",
    "        }\n",
    "    }\n",
    "}\n",
);

const DOMAINS_TOML: &str = r#"
[defaults]
operations = ["create", "read", "update", "delete", "list"]

[domains.shop]
label = "Shop"
schema_dir = "shop"
postgres_schema = "shop"
"#;

fn entity_schema(title: &str) -> SchemaNode {
    SchemaNode {
        namespace: None,
        schema_id: format!("shop/json/{title}.json"),
        title: title.to_string(),
        description: None,
        schema_type: "object".to_string(),
        classification: "entity_reference".to_string(),
        domain: Some("shop".to_string()),
        rel_path: format!("shop/json/{title}.json"),
        pg_type: "UUID".to_string(),
        rust_type: "Uuid".to_string(),
        sea_orm_type: "Uuid".to_string(),
        rust_type_name: title.strip_suffix("Type").unwrap_or(title).to_string(),
        pg_table_name: title.strip_suffix("Type").unwrap_or(title).to_lowercase(),
        api_path_segment: title.strip_suffix("Type").unwrap_or(title).to_lowercase(),
        parent_schema: None,
        is_entity: true,
        is_codelist: false,
        is_primitive_wrapper: false,
        has_all_of: false,
        has_one_of: false,
        has_any_of: false,
        has_definitions: false,
        custom_annotations: Default::default(),
        access: None,
        annotations: None,
    }
}

/// Writes the fixture dir and returns (design.ddd path, domains.toml path).
fn write_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    std::fs::write(dir.join("lib.mox"), LIB_MOX).unwrap();
    std::fs::write(dir.join("x.json"), X_JSON).unwrap();
    std::fs::write(dir.join("y.rosetta"), Y_ROSETTA).unwrap();
    std::fs::write(dir.join("z.rosetta"), Z_ROSETTA).unwrap();
    let design = dir.join("design.ddd");
    std::fs::write(&design, DESIGN_DDD).unwrap();
    let domains = dir.join("domains.toml");
    std::fs::write(&domains, DOMAINS_TOML).unwrap();
    (design, domains)
}

#[tokio::test]
async fn ingest_ddd_file_end_to_end_with_mox_schema_and_sigil_imports() {
    let dir = tempfile::tempdir().unwrap();
    let (design, domains_path) = write_fixture(dir.path());
    let domain_config = codegraph_config::config::parse_domain_config(&domains_path).unwrap();

    // The schema graph carries the mox-bridge titles: Product resolves via
    // the +Type suffix, Money exactly.
    let mock = MockEngine::builder()
        .with_schema(entity_schema("ProductType"))
        .with_schema(entity_schema("Money"))
        .build();

    let stats =
        codegraph::ingest::ddd_ingest::ingest_ddd_files(&mock, &mock, &[design], &domain_config)
            .await
            .unwrap();

    assert_eq!(stats.files, 1);
    assert_eq!(stats.designs, 2);
    assert_eq!(stats.repositories, 1);
    assert_eq!(stats.services, 1);
    assert_eq!(stats.searches, 1);
    assert_eq!(stats.unresolved_classes, 0, "both classes resolve");

    let models = mock.get_ddd_models().await.unwrap();
    assert_eq!(models.len(), 1);
    let graph = &models[0];
    assert_eq!(graph.application.name, "Shop");
    assert_eq!(graph.application.base.as_deref(), Some("shop"));
    assert_eq!(graph.designs.len(), 2);
    assert_eq!(graph.searches.len(), 1);

    // Title resolution against the ingested schema titles: exact first,
    // then +type_suffix.
    let product = graph
        .designs
        .iter()
        .find(|d| d.class == "Product")
        .expect("Product design");
    assert_eq!(product.resolved_title.as_deref(), Some("ProductType"));
    let money = graph
        .designs
        .iter()
        .find(|d| d.class == "Money")
        .expect("Money design");
    assert_eq!(money.resolved_title.as_deref(), Some("Money"));
    assert_eq!(
        graph.searches[0].entity_class, "Product",
        "as-authored reference"
    );
    assert_eq!(
        graph.searches[0].entity_title.as_deref(),
        Some("ProductType")
    );

    // The repository landed with the four builtins plus the declared op.
    assert_eq!(graph.repositories.len(), 1);
    let ops = &graph.repositories[0].operations;
    assert_eq!(ops.len(), 5);
    assert_eq!(ops[0].builtin.as_deref(), Some("findById"));
    assert_eq!(ops[4].name, "pricedAbove");
    assert!(ops[4].builtin.is_none());
    assert_eq!(
        ops[4].params[0].name, "cents",
        "the declared signature rides the graph"
    );
}

#[tokio::test]
async fn unknown_design_class_is_a_hard_error_with_the_diagnostic_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.mox"), LIB_MOX).unwrap();
    std::fs::write(dir.path().join("x.json"), X_JSON).unwrap();
    std::fs::write(dir.path().join("y.rosetta"), Y_ROSETTA).unwrap();
    std::fs::write(dir.path().join("z.rosetta"), Z_ROSETTA).unwrap();
    let design = dir.path().join("bad.ddd");
    std::fs::write(
        &design,
        concat!(
            "import \"lib.mox\"\n",
            "\n",
            "application Shop {\n",
            "    base shop\n",
            "    module m {\n",
            "        entity Ghost\n",
            "    }\n",
            "}\n",
        ),
    )
    .unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let domain_config = codegraph_config::config::parse_domain_config(&domains_path).unwrap();

    let mock = MockEngine::new();
    let err =
        codegraph::ingest::ddd_ingest::ingest_ddd_files(&mock, &mock, &[design], &domain_config)
            .await
            .unwrap_err();

    match &err {
        codegraph::error::Error::DddModel { file, reason } => {
            assert!(file.ends_with("bad.ddd"), "{file}");
            assert!(
                reason.contains("Ghost"),
                "the rendered diagnostic names the unknown class: {reason}"
            );
        }
        other => panic!("expected Error::DddModel, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_design_file_is_a_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let domain_config = codegraph_config::config::parse_domain_config(&domains_path).unwrap();

    let mock = MockEngine::new();
    let err = codegraph::ingest::ddd_ingest::ingest_ddd_files(
        &mock,
        &mock,
        &[dir.path().join("absent.ddd")],
        &domain_config,
    )
    .await
    .unwrap_err();

    match &err {
        codegraph::error::Error::DddModel { file, reason } => {
            assert!(file.ends_with("absent.ddd"), "{file}");
            assert!(reason.contains("could not be read"), "{reason}");
        }
        other => panic!("expected Error::DddModel, got {other:?}"),
    }
}

// ── graph-cache inputs ─────────────────────────────────────────────────

#[test]
fn ddd_files_are_part_of_the_graph_cache_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let domains_path = dir.path().join("domains.toml");
    std::fs::write(&domains_path, DOMAINS_TOML).unwrap();
    let design = dir.path().join("design.ddd");
    std::fs::write(&design, DESIGN_DDD).unwrap();

    let inputs = codegraph::artifact::collect_run_inputs(
        None,
        None,
        &domains_path,
        &[],
        &[],
        std::slice::from_ref(&design),
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(
        inputs
            .iter()
            .any(|f| f.path == design.display().to_string() && f.bytes == DESIGN_DDD.as_bytes()),
        "the .ddd file group rides the inputs list: {:?}",
        inputs.iter().map(|f| &f.path).collect::<Vec<_>>()
    );

    let before = codegraph::artifact::inputs_hash(&inputs);
    std::fs::write(
        &design,
        DESIGN_DDD.replace("ProductSearch", "RenamedSearch"),
    )
    .unwrap();
    let after = codegraph::artifact::inputs_hash(
        &codegraph::artifact::collect_run_inputs(
            None,
            None,
            &domains_path,
            &[],
            &[],
            std::slice::from_ref(&design),
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap(),
    );
    assert_ne!(
        before, after,
        "editing a .ddd file must bust the persisted-graph cache"
    );

    // A run without ddd files is unaffected (the group is empty).
    let without = codegraph::artifact::collect_run_inputs(
        None,
        None,
        &domains_path,
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .unwrap();
    assert!(
        !without.iter().any(|f| f.path.contains(".ddd")),
        "no ddd entries without ddd files"
    );
}
