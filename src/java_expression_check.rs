use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser, Tree};
use walkdir::{DirEntry, WalkDir};

use crate::cli::{ExpressionCheckOutput, JavaExpressionCheckArgs};

const DIRECT_RELATION_CHAIN: &str = "TQL-JAVA-EXPR-001";
const DEFAULT_CONFIG: &str = ".teaql/java-expression-check.yml";
const DEFAULT_EXCLUDED_COMPONENTS: &[&str] = &[
    ".git",
    "target",
    "build",
    "node_modules",
    "java-lib-core",
    "generated",
];
const CATALOG_EXCLUDED_COMPONENTS: &[&str] = &[".git", "target", "build", "node_modules"];

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PolicyConfig {
    version: u32,
    source_roots: Vec<PathBuf>,
    exclude_roots: Vec<PathBuf>,
    allow: Vec<Allowance>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Allowance {
    path: String,
    function: String,
    rules: Vec<String>,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
struct Finding {
    rule: &'static str,
    path: String,
    line: usize,
    function: String,
    message: &'static str,
    help: String,
}

#[derive(Debug, Serialize)]
struct Report {
    command: &'static str,
    result: &'static str,
    files_scanned: usize,
    findings: Vec<Finding>,
    suppressed: usize,
}

#[derive(Debug, Default)]
struct DomainCatalog {
    expression_entries: HashMap<String, String>,
    relations: HashMap<String, HashMap<String, String>>,
    parents: HashMap<String, String>,
}

struct JavaSyntax {
    source: String,
    tree: Tree,
}

pub fn run(cwd: &Path, args: JavaExpressionCheckArgs) -> Result<()> {
    let cwd = fs::canonicalize(cwd)
        .with_context(|| format!("failed to resolve workspace directory {}", cwd.display()))?;
    let (policy, policy_path) = load_policy(&cwd, args.config.as_deref())?;
    validate_policy(&policy, policy_path.as_deref())?;

    let roots = resolve_roots(&cwd, &args.sources, &policy.source_roots)?;
    let excludes = resolve_excludes(&cwd, &args.excludes, &policy.exclude_roots);
    let files = collect_java_files(&cwd, &roots, &excludes, DEFAULT_EXCLUDED_COMPONENTS)?;
    let catalog_files = collect_catalog_files(&cwd)?;
    let catalog = build_domain_catalog(&catalog_files)?;
    if catalog.expression_entries.is_empty() {
        bail!(
            "java-expression-check could not find a generated TeaQL E.java facade under {} or a sibling java-lib-core directory",
            cwd.display()
        );
    }

    let report = analyze_sources(&cwd, &files, &catalog, &policy)?;
    print_report(&report, args.format)?;
    if report.findings.is_empty() {
        Ok(())
    } else {
        bail!(
            "java-expression-check found {} violation(s)",
            report.findings.len()
        )
    }
}

fn analyze_sources(
    cwd: &Path,
    files: &[PathBuf],
    catalog: &DomainCatalog,
    policy: &PolicyConfig,
) -> Result<Report> {
    let mut findings = Vec::new();
    let mut suppressed = 0;

    for path in files {
        let syntax = parse_java(path)?;
        let relative_path = display_path(cwd, path);
        let mut file_findings = analyze_file(&relative_path, &syntax, catalog);
        for finding in file_findings.drain(..) {
            if is_allowed(&policy.allow, &finding) {
                suppressed += 1;
            } else {
                findings.push(finding);
            }
        }
    }

    findings.sort_by(|left, right| {
        (&left.path, left.line, left.rule).cmp(&(&right.path, right.line, right.rule))
    });
    Ok(Report {
        command: "java-expression-check",
        result: if findings.is_empty() { "pass" } else { "fail" },
        files_scanned: files.len(),
        findings,
        suppressed,
    })
}

fn analyze_file(path: &str, syntax: &JavaSyntax, catalog: &DomainCatalog) -> Vec<Finding> {
    let mut findings = Vec::new();
    walk(syntax.tree.root_node(), &mut |node| {
        if !matches!(
            node.kind(),
            "method_declaration" | "constructor_declaration"
        ) {
            return;
        }
        let function = node_text_field(node, "name", &syntax.source)
            .unwrap_or_else(|| "<anonymous>".to_string());
        let variables = collect_variable_types(node, &syntax.source);
        let mut seen = HashSet::new();

        walk(node, &mut |candidate| {
            if candidate.kind() != "method_invocation" {
                return;
            }
            let Some(object) = candidate.child_by_field_name("object") else {
                return;
            };
            if object.kind() != "method_invocation" {
                return;
            }
            let Some(relation_getter) = node_text_field(object, "name", &syntax.source) else {
                return;
            };
            let Some(root_object) = object.child_by_field_name("object") else {
                return;
            };
            let Some(root_name) = root_identifier(root_object, &syntax.source) else {
                return;
            };
            let Some(declared_type) = variables.get(&root_name) else {
                return;
            };
            let Some(entity_type) = resolve_entity_type(declared_type, catalog) else {
                return;
            };
            let Some(target_type) = catalog
                .relations
                .get(entity_type)
                .and_then(|relations| relations.get(&relation_getter))
            else {
                return;
            };
            let Some(property_getter) = node_text_field(candidate, "name", &syntax.source) else {
                return;
            };
            if !is_property_getter(&property_getter) {
                return;
            }

            let start = candidate.start_position();
            if !seen.insert((start.row, start.column, relation_getter.clone())) {
                return;
            }
            let entry = catalog
                .expression_entries
                .get(entity_type)
                .map(String::as_str)
                .unwrap_or("entity");
            findings.push(Finding {
                rule: DIRECT_RELATION_CHAIN,
                path: path.to_string(),
                line: start.row + 1,
                function: function.clone(),
                message: "TeaQL entity relation is accessed through a direct getter chain",
                help: format!(
                    "replace {root_name}.{relation_getter}().{property_getter}() with E.{entry}({root_name}).{relation_getter}().{property_getter}().eval() (target entity: {target_type})"
                ),
            });
        });
    });
    findings
}

fn collect_variable_types(method: Node<'_>, source: &str) -> HashMap<String, String> {
    let mut variables = HashMap::new();
    walk(method, &mut |node| match node.kind() {
        "formal_parameter" | "spread_parameter" | "catch_formal_parameter" => {
            insert_typed_name(node, source, &mut variables);
        }
        "enhanced_for_statement" => {
            insert_typed_name(node, source, &mut variables);
        }
        "local_variable_declaration" => {
            let Some(type_node) = node.child_by_field_name("type") else {
                return;
            };
            let declared_type = simple_type(node_text(type_node, source));
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() != "variable_declarator" {
                    continue;
                }
                if let Some(name) = node_text_field(child, "name", source) {
                    let inferred = if declared_type == "var" {
                        child
                            .child_by_field_name("value")
                            .and_then(|value| inferred_cast_type(value, source))
                    } else {
                        None
                    };
                    variables.insert(name, inferred.unwrap_or_else(|| declared_type.clone()));
                }
            }
        }
        _ => {}
    });
    variables
}

fn insert_typed_name(node: Node<'_>, source: &str, variables: &mut HashMap<String, String>) {
    let (Some(type_node), Some(name)) = (
        node.child_by_field_name("type"),
        node_text_field(node, "name", source),
    ) else {
        return;
    };
    variables.insert(name, simple_type(node_text(type_node, source)));
}

fn inferred_cast_type(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() == "cast_expression" {
        return node
            .child_by_field_name("type")
            .map(|value| simple_type(node_text(value, source)));
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|child| inferred_cast_type(child, source))
}

fn root_identifier(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(node_text(node, source).to_string()),
        "field_access" => node_text_field(node, "field", source),
        "parenthesized_expression" | "cast_expression" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find_map(|child| root_identifier(child, source))
        }
        _ => None,
    }
}

fn resolve_entity_type<'a>(declared: &'a str, catalog: &'a DomainCatalog) -> Option<&'a str> {
    let mut current = declared;
    let mut visited = HashSet::new();
    loop {
        if catalog.expression_entries.contains_key(current) {
            return Some(current);
        }
        if !visited.insert(current.to_string()) {
            return None;
        }
        current = catalog.parents.get(current)?.as_str();
    }
}

fn is_property_getter(method: &str) -> bool {
    method.starts_with("get") || method.starts_with("is")
}

fn build_domain_catalog(files: &[PathBuf]) -> Result<DomainCatalog> {
    let mut catalog = DomainCatalog::default();

    for path in files {
        let syntax = parse_java(path)?;
        collect_parent_types(&syntax, &mut catalog.parents);
        if path.file_name().is_some_and(|name| name == "E.java") {
            collect_expression_entries(&syntax, &mut catalog.expression_entries);
        }
    }

    let entity_types = catalog
        .expression_entries
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    for path in files {
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if !entity_types.contains(stem) {
            continue;
        }
        let syntax = parse_java(path)?;
        collect_entity_relations(stem, &syntax, &entity_types, &mut catalog.relations);
    }
    Ok(catalog)
}

fn collect_expression_entries(syntax: &JavaSyntax, entries: &mut HashMap<String, String>) {
    walk(syntax.tree.root_node(), &mut |node| {
        if node.kind() != "method_declaration" {
            return;
        }
        let Some(method) = node_text_field(node, "name", &syntax.source) else {
            return;
        };
        let Some(parameters) = node.child_by_field_name("parameters") else {
            return;
        };
        let mut parameter_type = None;
        walk(parameters, &mut |parameter| {
            if parameter_type.is_some() || parameter.kind() != "formal_parameter" {
                return;
            }
            parameter_type = parameter
                .child_by_field_name("type")
                .map(|value| simple_type(node_text(value, &syntax.source)));
        });
        if let Some(entity_type) = parameter_type {
            entries.entry(entity_type).or_insert(method);
        }
    });
}

fn collect_parent_types(syntax: &JavaSyntax, parents: &mut HashMap<String, String>) {
    walk(syntax.tree.root_node(), &mut |node| {
        if node.kind() != "class_declaration" {
            return;
        }
        let (Some(name), Some(superclass)) = (
            node_text_field(node, "name", &syntax.source),
            node.child_by_field_name("superclass"),
        ) else {
            return;
        };
        let parent = simple_type(node_text(superclass, &syntax.source));
        if !parent.is_empty() {
            parents.insert(name, parent);
        }
    });
}

fn collect_entity_relations(
    owner: &str,
    syntax: &JavaSyntax,
    entity_types: &HashSet<String>,
    relations: &mut HashMap<String, HashMap<String, String>>,
) {
    walk(syntax.tree.root_node(), &mut |node| {
        if node.kind() != "method_declaration" {
            return;
        }
        let (Some(method), Some(return_type)) = (
            node_text_field(node, "name", &syntax.source),
            node.child_by_field_name("type"),
        ) else {
            return;
        };
        if !method.starts_with("get") {
            return;
        }
        let return_text = node_text(return_type, &syntax.source);
        let Some(target) = identifiers(return_text)
            .into_iter()
            .find(|identifier| entity_types.contains(identifier))
        else {
            return;
        };
        relations
            .entry(owner.to_string())
            .or_default()
            .entry(method)
            .or_insert(target);
    });
}

fn parse_java(path: &Path) -> Result<JavaSyntax> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read Java source {}", path.display()))?;
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .context("failed to initialize Java parser")?;
    let tree = parser
        .parse(&source, None)
        .with_context(|| format!("Java parser returned no tree for {}", path.display()))?;
    if tree.root_node().has_error() {
        bail!("failed to parse Java source {}", path.display());
    }
    Ok(JavaSyntax { source, tree })
}

fn walk<'tree>(node: Node<'tree>, visitor: &mut impl FnMut(Node<'tree>)) {
    visitor(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, visitor);
    }
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn node_text_field(node: Node<'_>, field: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field)
        .map(|value| node_text(value, source).to_string())
}

fn simple_type(value: &str) -> String {
    identifiers(value).into_iter().last().unwrap_or_default()
}

fn identifiers(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|part| !part.is_empty())
        .map(String::from)
        .collect()
}

fn load_policy(cwd: &Path, explicit: Option<&Path>) -> Result<(PolicyConfig, Option<PathBuf>)> {
    let path = explicit.map(|path| resolve(cwd, path)).or_else(|| {
        let default = cwd.join(DEFAULT_CONFIG);
        default.exists().then_some(default)
    });
    let Some(path) = path else {
        return Ok((PolicyConfig::default(), None));
    };
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read Java expression policy {}", path.display()))?;
    let policy = serde_yaml::from_str(&raw)
        .with_context(|| format!("failed to parse Java expression policy {}", path.display()))?;
    Ok((policy, Some(path)))
}

fn validate_policy(policy: &PolicyConfig, path: Option<&Path>) -> Result<()> {
    if path.is_some() && policy.version != 1 {
        let location = path
            .map(|value| value.display().to_string())
            .unwrap_or_else(|| "<defaults>".to_string());
        bail!(
            "unsupported java-expression-check policy version {} in {}",
            policy.version,
            location
        );
    }
    for allowance in &policy.allow {
        if allowance.path.trim().is_empty()
            || allowance.function.trim().is_empty()
            || allowance.reason.trim().is_empty()
            || allowance.rules.is_empty()
        {
            bail!("every java-expression-check allowance needs path, function, rules and reason");
        }
        for rule in &allowance.rules {
            if rule != DIRECT_RELATION_CHAIN {
                bail!("unknown java-expression-check allowance rule {rule}");
            }
        }
    }
    Ok(())
}

fn resolve_roots(cwd: &Path, cli: &[PathBuf], configured: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let selected = if !cli.is_empty() {
        cli.to_vec()
    } else if !configured.is_empty() {
        configured.to_vec()
    } else if cwd.join("src").is_dir() {
        vec![PathBuf::from("src")]
    } else {
        vec![PathBuf::from(".")]
    };

    selected
        .into_iter()
        .map(|path| {
            let resolved = resolve(cwd, &path);
            if !resolved.exists() {
                bail!(
                    "Java expression source root does not exist: {}",
                    resolved.display()
                );
            }
            Ok(resolved)
        })
        .collect()
}

fn resolve_excludes(cwd: &Path, cli: &[PathBuf], configured: &[PathBuf]) -> Vec<PathBuf> {
    cli.iter()
        .chain(configured)
        .map(|path| resolve(cwd, path))
        .collect()
}

fn resolve(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn collect_catalog_files(cwd: &Path) -> Result<Vec<PathBuf>> {
    let mut roots = vec![cwd.to_path_buf()];
    if let Some(parent) = cwd.parent() {
        let sibling = parent.join("java-lib-core");
        if sibling.is_dir() && !cwd.starts_with(&sibling) {
            roots.push(sibling);
        }
    }
    collect_java_files(cwd, &roots, &[], CATALOG_EXCLUDED_COMPONENTS)
}

fn collect_java_files(
    cwd: &Path,
    roots: &[PathBuf],
    excludes: &[PathBuf],
    excluded_components: &[&str],
) -> Result<Vec<PathBuf>> {
    let mut files = BTreeSet::new();
    for root in roots {
        if root.is_file() {
            if root
                .extension()
                .is_some_and(|extension| extension == "java")
            {
                files.insert(root.clone());
            }
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| should_visit(entry, cwd, excludes, excluded_components))
        {
            let entry = entry.with_context(|| format!("failed to walk {}", root.display()))?;
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "java")
            {
                files.insert(entry.into_path());
            }
        }
    }
    Ok(files.into_iter().collect())
}

fn should_visit(
    entry: &DirEntry,
    cwd: &Path,
    excludes: &[PathBuf],
    excluded_components: &[&str],
) -> bool {
    let path = entry.path();
    let relative = path.strip_prefix(cwd).unwrap_or(path);
    if path != cwd
        && relative.components().any(|component| {
            let Component::Normal(value) = component else {
                return false;
            };
            excluded_components
                .iter()
                .any(|excluded| value == *excluded)
        })
    {
        return false;
    }
    !excludes.iter().any(|excluded| path.starts_with(excluded))
}

fn display_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\u{5c}', "/")
}

fn is_allowed(allowances: &[Allowance], finding: &Finding) -> bool {
    allowances.iter().any(|allowance| {
        allowance.path.replace('\u{5c}', "/") == finding.path
            && allowance.function == finding.function
            && allowance.rules.iter().any(|rule| rule == finding.rule)
    })
}

fn print_report(report: &Report, format: ExpressionCheckOutput) -> Result<()> {
    match format {
        ExpressionCheckOutput::Json => println!("{}", serde_json::to_string_pretty(report)?),
        ExpressionCheckOutput::Text => {
            for finding in &report.findings {
                eprintln!("error[{}]: {}", finding.rule, finding.message);
                eprintln!("  --> {}:{}", finding.path, finding.line);
                eprintln!("   = function: {}", finding.function);
                eprintln!("   = help: {}", finding.help);
                eprintln!();
            }
            if report.findings.is_empty() {
                println!(
                    "java-expression-check passed: {} Java file(s), {} audited exception(s)",
                    report.files_scanned, report.suppressed
                );
            } else {
                eprintln!(
                    "java-expression-check failed: {} violation(s) in {} Java file(s), {} audited exception(s)",
                    report.findings.len(),
                    report.files_scanned,
                    report.suppressed
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_fixture(application: &str, policy: &str) -> Result<Report> {
        let temp = tempfile::tempdir()?;
        let generated = temp.path().join("java-lib-core/src/main/java/example");
        let application_root = temp.path().join("app/src/main/java/example");
        fs::create_dir_all(&generated)?;
        fs::create_dir_all(&application_root)?;
        fs::write(
            generated.join("E.java"),
            r#"
            class E {
                static TaskExpression task(Task value) { return null; }
                static TaskStatusExpression taskStatus(TaskStatus value) { return null; }
            }
            "#,
        )?;
        fs::write(
            generated.join("Task.java"),
            r#"
            class Task {
                TaskStatus getStatus() { return null; }
                String getName() { return ""; }
            }
            "#,
        )?;
        fs::write(
            generated.join("TaskStatus.java"),
            r#"
            class TaskStatus { String getCode() { return ""; } }
            "#,
        )?;
        fs::write(application_root.join("Example.java"), application)?;
        if !policy.is_empty() {
            fs::create_dir_all(temp.path().join(".teaql"))?;
            fs::write(temp.path().join(DEFAULT_CONFIG), policy)?;
        }

        let cwd = fs::canonicalize(temp.path())?;
        let (policy, _) = load_policy(&cwd, None)?;
        validate_policy(&policy, None)?;
        let files = collect_java_files(
            &cwd,
            std::slice::from_ref(&cwd),
            &[],
            DEFAULT_EXCLUDED_COMPONENTS,
        )?;
        let catalog_files = collect_catalog_files(&cwd)?;
        let catalog = build_domain_catalog(&catalog_files)?;
        analyze_sources(&cwd, &files, &catalog, &policy)
    }

    #[test]
    fn rejects_direct_teaql_relation_chain() {
        let report = check_fixture(
            r#"
            class Example {
                String status(Task task) {
                    return task.getStatus() != null ? task.getStatus().getCode() : "";
                }
            }
            "#,
            "",
        )
        .unwrap();

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].rule, DIRECT_RELATION_CHAIN);
    }

    #[test]
    fn accepts_generated_expression_chain() {
        let report = check_fixture(
            r#"
            class Example {
                String status(Task task) {
                    return E.task(task).getStatus().getCode().eval();
                }
            }
            "#,
            "",
        )
        .unwrap();

        assert!(report.findings.is_empty());
    }

    #[test]
    fn ignores_non_teaql_getter_chain_with_same_method_names() {
        let report = check_fixture(
            r#"
            class Example {
                String status(HttpResponse response) {
                    return response.getStatus().getCode();
                }
            }
            "#,
            "",
        )
        .unwrap();

        assert!(report.findings.is_empty());
    }

    #[test]
    fn recognizes_entity_subclasses() {
        let report = check_fixture(
            r#"
            class BusinessTask extends Task {}
            class Example {
                String status(BusinessTask task) {
                    return task.getStatus().getCode();
                }
            }
            "#,
            "",
        )
        .unwrap();

        assert_eq!(report.findings.len(), 1);
    }

    #[test]
    fn recognizes_entities_declared_by_enhanced_for_loop() {
        let report = check_fixture(
            r#"
            class Example {
                void render(java.util.List<Task> tasks) {
                    for (Task task : tasks) {
                        System.out.println(task.getStatus().getCode());
                    }
                }
            }
            "#,
            "",
        )
        .unwrap();

        assert_eq!(report.findings.len(), 1);
    }

    #[test]
    fn applies_exact_audited_allowance() {
        let report = check_fixture(
            r#"
            class Example {
                String legacyStatus(Task task) {
                    return task.getStatus().getCode();
                }
            }
            "#,
            r#"
version: 1
allow:
  - path: app/src/main/java/example/Example.java
    function: legacyStatus
    rules: [TQL-JAVA-EXPR-001]
    reason: legacy boundary awaiting migration
            "#,
        )
        .unwrap();

        assert!(report.findings.is_empty());
        assert_eq!(report.suppressed, 1);
    }
}
