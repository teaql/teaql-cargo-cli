use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use proc_macro2::Span;
use serde::{Deserialize, Serialize};
use syn::visit::{self, Visit};
use syn::{ExprMethodCall, ExprPath, ImplItem, ItemFn, ItemImpl, Type};
use walkdir::{DirEntry, WalkDir};

use crate::cli::{ExpressionCheckOutput, RustExpressionCheckArgs};

const READ_WITHOUT_EXPRESSION: &str = "TQL-RUST-EXPR-001";
const FULL_PROJECTION_IN_READ: &str = "TQL-RUST-EXPR-002";
const DEFAULT_CONFIG: &str = ".teaql/rust-expression-check.yml";
const DEFAULT_EXCLUDED_COMPONENTS: &[&str] = &[
    ".git",
    "target",
    "build",
    "node_modules",
    "rust-lib-core",
    "generate-lib",
    "generate-workspace",
    "bizcore",
];

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
pub struct Finding {
    rule: &'static str,
    path: String,
    line: usize,
    function: String,
    message: &'static str,
    help: &'static str,
}

#[derive(Debug, Serialize)]
struct Report {
    command: &'static str,
    result: &'static str,
    files_scanned: usize,
    findings: Vec<Finding>,
    suppressed: usize,
}

#[derive(Debug)]
struct FunctionSignals {
    name: String,
    gateway_key: String,
    uses_q: bool,
    uses_e: bool,
    calls: HashSet<String>,
    read_executes: Vec<Span>,
    full_projections: Vec<Span>,
    mutation_flow: bool,
}

#[derive(Debug)]
struct ParsedSource {
    relative_path: String,
    functions: Vec<FunctionSignals>,
}

pub fn run(cwd: &Path, args: RustExpressionCheckArgs) -> Result<()> {
    let cwd = fs::canonicalize(cwd)
        .with_context(|| format!("failed to resolve workspace directory {}", cwd.display()))?;
    let (policy, policy_path) = load_policy(&cwd, args.config.as_deref())?;
    validate_policy(&policy, policy_path.as_deref())?;

    let roots = resolve_roots(&cwd, &args.sources, &policy.source_roots)?;
    let excludes = resolve_excludes(&cwd, &args.excludes, &policy.exclude_roots);
    let files = collect_rust_files(&cwd, &roots, &excludes)?;
    let report = analyze_sources(&cwd, &files, &policy)?;
    print_report(&report, args.format)?;

    if report.findings.is_empty() {
        Ok(())
    } else {
        bail!(
            "rust-expression-check found {} violation(s)",
            report.findings.len()
        )
    }
}

fn analyze_sources(cwd: &Path, files: &[PathBuf], policy: &PolicyConfig) -> Result<Report> {
    let parsed = parse_sources(cwd, files)?;
    let expression_gateways = expression_gateways(&parsed);
    let mut findings = Vec::new();
    let mut suppressed = 0;

    for source in &parsed {
        for function in &source.functions {
            if !function.uses_q || function.read_executes.is_empty() || function.mutation_flow {
                continue;
            }

            let delegates_to_expression = function.uses_e
                || function
                    .calls
                    .iter()
                    .any(|call| expression_gateways.contains(call));

            if !delegates_to_expression {
                let finding = Finding {
                    rule: READ_WITHOUT_EXPRESSION,
                    path: source.relative_path.clone(),
                    line: line_of(function.read_executes[0]),
                    function: function.name.clone(),
                    message: "TeaQL read query bypasses E-expression evaluation",
                    help: "evaluate the selected entity through E in this function or a centralized read-model converter",
                };
                if is_allowed(&policy.allow, &finding) {
                    suppressed += 1;
                } else {
                    findings.push(finding);
                }
            }

            for span in &function.full_projections {
                let finding = Finding {
                    rule: FULL_PROJECTION_IN_READ,
                    path: source.relative_path.clone(),
                    line: line_of(*span),
                    function: function.name.clone(),
                    message: "read query uses select_self_fields()",
                    help: "start from *_minimal(), select only consumed fields and relations, then evaluate through E",
                };
                if is_allowed(&policy.allow, &finding) {
                    suppressed += 1;
                } else {
                    findings.push(finding);
                }
            }
        }
    }

    findings.sort_by(|left, right| {
        (&left.path, left.line, left.rule).cmp(&(&right.path, right.line, right.rule))
    });
    Ok(Report {
        command: "rust-expression-check",
        result: if findings.is_empty() { "pass" } else { "fail" },
        files_scanned: files.len(),
        findings,
        suppressed,
    })
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
        .with_context(|| format!("failed to read expression policy {}", path.display()))?;
    let policy = serde_yaml::from_str(&raw)
        .with_context(|| format!("failed to parse expression policy {}", path.display()))?;
    Ok((policy, Some(path)))
}

fn validate_policy(policy: &PolicyConfig, path: Option<&Path>) -> Result<()> {
    if path.is_some() && policy.version != 1 {
        let location = path
            .map(|value| value.display().to_string())
            .unwrap_or_else(|| "<defaults>".to_string());
        bail!(
            "unsupported rust-expression-check policy version {} in {}",
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
            bail!("every rust-expression-check allowance needs path, function, rules and reason");
        }
        for rule in &allowance.rules {
            if rule != READ_WITHOUT_EXPRESSION && rule != FULL_PROJECTION_IN_READ {
                bail!("unknown rust-expression-check allowance rule {rule}");
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
                    "Rust expression source root does not exist: {}",
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

fn collect_rust_files(cwd: &Path, roots: &[PathBuf], excludes: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = BTreeSet::new();
    for root in roots {
        if root.is_file() {
            if root.extension().is_some_and(|extension| extension == "rs") {
                files.insert(root.clone());
            }
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| should_visit(entry, cwd, excludes))
        {
            let entry = entry.with_context(|| format!("failed to walk {}", root.display()))?;
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "rs")
            {
                files.insert(entry.into_path());
            }
        }
    }
    Ok(files.into_iter().collect())
}

fn should_visit(entry: &DirEntry, cwd: &Path, excludes: &[PathBuf]) -> bool {
    let path = entry.path();
    let relative = path.strip_prefix(cwd).unwrap_or(path);
    if path != cwd
        && relative.components().any(|component| {
            let Component::Normal(value) = component else {
                return false;
            };
            DEFAULT_EXCLUDED_COMPONENTS
                .iter()
                .any(|excluded| value == *excluded)
        })
    {
        return false;
    }
    !excludes.iter().any(|excluded| path.starts_with(excluded))
}

fn parse_sources(cwd: &Path, files: &[PathBuf]) -> Result<Vec<ParsedSource>> {
    files
        .iter()
        .map(|path| {
            let source = fs::read_to_string(path)
                .with_context(|| format!("failed to read Rust source {}", path.display()))?;
            let syntax = syn::parse_file(&source)
                .with_context(|| format!("failed to parse Rust source {}", path.display()))?;
            let mut collector = FunctionCollector::default();
            collector.visit_file(&syntax);
            Ok(ParsedSource {
                relative_path: display_path(cwd, path),
                functions: collector.functions,
            })
        })
        .collect()
}

fn display_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\u{5c}', "/")
}

fn expression_gateways(parsed: &[ParsedSource]) -> HashSet<String> {
    parsed
        .iter()
        .flat_map(|source| &source.functions)
        .filter(|function| function.uses_e)
        .map(|function| function.gateway_key.clone())
        .collect()
}

fn line_of(span: Span) -> usize {
    span.start().line.max(1)
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
                    "rust-expression-check passed: {} Rust file(s), {} audited exception(s)",
                    report.files_scanned, report.suppressed
                );
            } else {
                eprintln!(
                    "rust-expression-check failed: {} violation(s) in {} Rust file(s), {} audited exception(s)",
                    report.findings.len(),
                    report.files_scanned,
                    report.suppressed
                );
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct FunctionCollector {
    functions: Vec<FunctionSignals>,
}

impl<'ast> Visit<'ast> for FunctionCollector {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let name = node.sig.ident.to_string();
        self.functions
            .push(analyze_function(name.clone(), name, None, &node.block));
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        let owner = simple_type_name(&node.self_ty);
        for item in &node.items {
            let ImplItem::Fn(method) = item else {
                continue;
            };
            let name = method.sig.ident.to_string();
            let gateway_key = owner
                .as_ref()
                .map(|owner| format!("{owner}::{name}"))
                .unwrap_or_else(|| name.clone());
            self.functions.push(analyze_function(
                name,
                gateway_key,
                owner.as_deref(),
                &method.block,
            ));
        }
    }
}

fn simple_type_name(value: &Type) -> Option<String> {
    let Type::Path(path) = value else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn analyze_function(
    name: String,
    gateway_key: String,
    owner: Option<&str>,
    block: &syn::Block,
) -> FunctionSignals {
    let mut visitor = SignalVisitor::default();
    visitor.visit_block(block);
    if let Some(owner) = owner {
        visitor.calls = visitor
            .calls
            .into_iter()
            .map(|call| {
                call.strip_prefix("Self::")
                    .map(|method| format!("{owner}::{method}"))
                    .unwrap_or(call)
            })
            .collect();
    }
    FunctionSignals {
        name,
        gateway_key,
        uses_q: visitor.uses_q,
        uses_e: visitor.uses_e,
        calls: visitor.calls,
        read_executes: visitor.read_executes,
        full_projections: visitor.full_projections,
        mutation_flow: visitor.mutation_flow,
    }
}

#[derive(Default)]
struct SignalVisitor {
    uses_q: bool,
    uses_e: bool,
    calls: HashSet<String>,
    read_executes: Vec<Span>,
    full_projections: Vec<Span>,
    mutation_flow: bool,
}

impl<'ast> Visit<'ast> for SignalVisitor {
    fn visit_expr_path(&mut self, node: &'ast ExprPath) {
        for segment in &node.path.segments {
            let ident = segment.ident.to_string();
            self.uses_q |= ident == "Q";
            self.uses_e |= ident == "E";
        }
        let path = node
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");
        if !path.is_empty() {
            self.calls.insert(path);
        }
        visit::visit_expr_path(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let method = node.method.to_string();
        self.calls.insert(method.clone());
        if matches!(
            method.as_str(),
            "execute" | "execute_for_list" | "execute_for_page" | "execute_for_stream"
        ) {
            self.read_executes.push(node.method.span());
        }
        if method == "select_self_fields" {
            self.full_projections.push(node.method.span());
        }
        if method == "new_entity"
            || method == "audit_as"
            || method == "save"
            || method == "save_with"
            || method == "update"
            || method.starts_with("update_")
        {
            self.mutation_flow = true;
        }
        visit::visit_expr_method_call(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_fixture(source: &str, policy: &str) -> Result<Report> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("src"))?;
        fs::write(temp.path().join("src/lib.rs"), source)?;
        if !policy.is_empty() {
            fs::create_dir_all(temp.path().join(".teaql"))?;
            fs::write(temp.path().join(DEFAULT_CONFIG), policy)?;
        }

        let cwd = fs::canonicalize(temp.path())?;
        let (policy, _) = load_policy(&cwd, None)?;
        validate_policy(&policy, None)?;
        let roots = resolve_roots(&cwd, &[], &policy.source_roots)?;
        let files = collect_rust_files(&cwd, &roots, &[])?;
        analyze_sources(&cwd, &files, &policy)
    }

    #[test]
    fn rejects_read_query_without_expression_gateway() {
        let report = check_fixture(
            r#"
            async fn load(context: &Context) {
                let rows = Q::assets_minimal()
                    .select_path()
                    .execute_for_list(context).await.unwrap();
                consume(rows);
            }
            "#,
            "",
        )
        .unwrap();

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].rule, READ_WITHOUT_EXPRESSION);
    }

    #[test]
    fn accepts_centralized_expression_gateway() {
        let report = check_fixture(
            r#"
            fn from_selected(asset: Asset) -> LoadedAsset {
                LoadedAsset { path: E::asset(&asset).get_path().unwrap() }
            }
            async fn load(context: &Context) {
                let rows = Q::assets_minimal()
                    .select_path()
                    .execute_for_list(context).await.unwrap();
                rows.into_iter().map(from_selected).collect::<Vec<_>>();
            }
            "#,
            "",
        )
        .unwrap();

        assert!(report.findings.is_empty());
    }

    #[test]
    fn expression_gateway_is_qualified_by_owner_type() {
        let report = check_fixture(
            r#"
            struct Safe;
            impl Safe {
                fn from_selected(asset: Asset) -> String {
                    E::asset(&asset).get_path().unwrap()
                }
            }
            struct Unsafe;
            impl Unsafe {
                fn from_selected(asset: Asset) -> String {
                    asset.path().to_owned()
                }
            }
            async fn load(context: &Context) {
                let rows = Q::assets_minimal()
                    .select_path()
                    .execute_for_list(context).await.unwrap();
                rows.into_iter().map(Unsafe::from_selected).collect::<Vec<_>>();
            }
            "#,
            "",
        )
        .unwrap();

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].rule, READ_WITHOUT_EXPRESSION);
    }

    #[test]
    fn rejects_full_projection_in_read_flow() {
        let report = check_fixture(
            r#"
            async fn load(context: &Context) {
                let rows = Q::assets().select_self_fields()
                    .execute_for_list(context).await.unwrap();
                rows.into_iter().map(|asset| E::asset(&asset).get_path().unwrap());
            }
            "#,
            "",
        )
        .unwrap();

        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].rule, FULL_PROJECTION_IN_READ);
    }

    #[test]
    fn permits_full_entity_mutation_flow() {
        let report = check_fixture(
            r#"
            async fn update_asset(context: &Context) {
                let mut rows = Q::assets().select_self_fields()
                    .execute_for_list(context).await.unwrap();
                rows[0].update_path("next");
                rows[0].audit_as("update").save_with(context).await.unwrap();
            }
            "#,
            "",
        )
        .unwrap();

        assert!(report.findings.is_empty());
    }

    #[test]
    fn applies_exact_audited_allowance() {
        let report = check_fixture(
            r#"
            async fn hydrate_for_mutation(context: &Context) {
                Q::assets().select_self_fields()
                    .execute_for_list(context).await.unwrap();
            }
            "#,
            r#"
version: 1
allow:
  - path: src/lib.rs
    function: hydrate_for_mutation
    rules: [TQL-RUST-EXPR-001, TQL-RUST-EXPR-002]
    reason: entity is returned to a mutation boundary
            "#,
        )
        .unwrap();

        assert!(report.findings.is_empty());
        assert_eq!(report.suppressed, 2);
    }
}
