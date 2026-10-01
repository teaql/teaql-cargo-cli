use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tree_sitter::{Language as TreeSitterLanguage, Node, Parser, Tree};
use walkdir::{DirEntry, WalkDir};

use crate::cli::{ExpressionCheckOutput, LanguageExpressionCheckArgs};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    Kotlin,
    Python,
    Csharp,
    Golang,
    Swift,
    Typescript,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PolicyConfig {
    version: u32,
    source_roots: Vec<PathBuf>,
    exclude_roots: Vec<PathBuf>,
    relation_members: Vec<String>,
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

#[derive(Clone, Debug)]
struct Relation {
    owner: String,
    member: String,
    target: String,
}

#[derive(Debug)]
struct Syntax {
    source: String,
    tree: Tree,
}

#[derive(Debug)]
struct MemberChain {
    start_byte: usize,
    end_byte: usize,
    segments: Vec<String>,
    called: Vec<bool>,
}

pub fn run(cwd: &Path, args: LanguageExpressionCheckArgs, language: Language) -> Result<()> {
    let cwd = fs::canonicalize(cwd)
        .with_context(|| format!("failed to resolve workspace directory {}", cwd.display()))?;
    let (policy, policy_path) = load_policy(&cwd, args.config.as_deref(), language)?;
    validate_policy(&policy, policy_path.as_deref(), language)?;

    let roots = resolve_roots(&cwd, &args.sources, &policy.source_roots)?;
    let excludes = resolve_excludes(&cwd, &args.excludes, &policy.exclude_roots);
    let files = collect_source_files(&cwd, &roots, &excludes, language)?;
    let nearby = nearby_catalog_roots(&cwd);
    let mut relations = discover_model_relations(&nearby)?;
    relations.extend(policy.relation_members.iter().map(|member| Relation {
        owner: "entity".to_string(),
        member: member.clone(),
        target: "relatedEntity".to_string(),
    }));

    let facade = discover_expression_facade(&nearby, language)?;
    let report = analyze_sources(&cwd, &files, &relations, facade.as_ref(), &policy, language)?;
    print_report(&report, args.format, language)?;
    if report.findings.is_empty() {
        Ok(())
    } else {
        bail!(
            "{} found {} violation(s)",
            language.command(),
            report.findings.len()
        )
    }
}

fn analyze_sources(
    cwd: &Path,
    files: &[PathBuf],
    relations: &[Relation],
    facade: Option<&PathBuf>,
    policy: &PolicyConfig,
    language: Language,
) -> Result<Report> {
    let relation_members = relation_members(relations, language);
    let mut findings = Vec::new();
    let mut suppressed = 0;

    if facade.is_none() {
        findings.push(Finding {
            rule: language.facade_rule(),
            path: "<workspace>".to_string(),
            line: 1,
            function: "<project>".to_string(),
            message: "generated TeaQL E facade is missing or has no expression entrypoints",
            help: format!(
                "generate a functional {} E facade before enforcing application expression access",
                language.label()
            ),
        });
    }

    if relation_members.is_empty() {
        findings.push(Finding {
            rule: language.facade_rule(),
            path: "<workspace>".to_string(),
            line: 1,
            function: "<project>".to_string(),
            message: "TeaQL relation metadata could not be discovered",
            help: format!(
                "keep the KSML/XML model under the workspace or configure relation_members in {}",
                language.default_config()
            ),
        });
    }

    for path in files {
        let syntax = parse_source(path, language)?;
        let masked = mask_non_code(&syntax);
        let chains = scan_member_chains(&masked);
        let expression_ranges = chains
            .iter()
            .filter(|chain| chain.segments.first().is_some_and(|root| root == "E"))
            .map(|chain| (chain.start_byte, chain.end_byte))
            .collect::<Vec<_>>();
        let relative_path = display_path(cwd, path);
        let mut seen = HashSet::new();

        for chain in chains {
            if expression_ranges
                .iter()
                .any(|(start, end)| chain.start_byte >= *start && chain.start_byte < *end)
                || line_is_import(&syntax.source, chain.start_byte, language)
            {
                continue;
            }
            let Some((relation_index, relation)) = matched_relation(&chain, &relation_members)
            else {
                continue;
            };
            if relation_index + 1 >= chain.segments.len()
                && !(language == Language::Golang && relation == "RelationEntity")
            {
                continue;
            }
            if !seen.insert((chain.start_byte, relation.to_string())) {
                continue;
            }

            let root = root_expression(&syntax.source, chain.start_byte);
            let leaf = chain
                .segments
                .get(relation_index + 1)
                .map(String::as_str)
                .unwrap_or("field");
            let relation_metadata = relation_for_member(relations, relation, language);
            let function = enclosing_function(
                syntax.tree.root_node(),
                chain.start_byte,
                &syntax.source,
                language,
            );
            let finding = Finding {
                rule: language.direct_rule(),
                path: relative_path.clone(),
                line: byte_line(&syntax.source, chain.start_byte),
                function,
                message: "TeaQL entity relation is accessed directly instead of through E",
                help: expression_help(language, &root, relation, leaf, relation_metadata),
            };
            if is_allowed(&policy.allow, &finding) {
                suppressed += 1;
            } else {
                findings.push(finding);
            }
        }
    }

    let mut retained = Vec::new();
    for finding in findings {
        if is_allowed(&policy.allow, &finding) {
            suppressed += 1;
        } else {
            retained.push(finding);
        }
    }
    retained.sort_by(|left, right| {
        (&left.path, left.line, left.rule).cmp(&(&right.path, right.line, right.rule))
    });
    Ok(Report {
        command: language.command(),
        result: if retained.is_empty() { "pass" } else { "fail" },
        files_scanned: files.len(),
        findings: retained,
        suppressed,
    })
}

fn matched_relation<'a>(
    chain: &'a MemberChain,
    relation_members: &'a BTreeSet<String>,
) -> Option<(usize, &'a str)> {
    chain
        .segments
        .iter()
        .enumerate()
        .skip(1)
        .find(|(index, segment)| {
            relation_members.contains(segment.as_str())
                && (*index + 1 < chain.segments.len()
                    || chain.called.get(*index).copied().unwrap_or(false))
        })
        .map(|(index, segment)| (index, segment.as_str()))
}

fn relation_for_member<'a>(
    relations: &'a [Relation],
    member: &str,
    language: Language,
) -> Option<&'a Relation> {
    let mut candidates = relations
        .iter()
        .filter(|relation| language.direct_member(&relation.member) == member);
    let candidate = candidates.next()?;
    candidates.next().is_none().then_some(candidate)
}

fn expression_help(
    language: Language,
    root: &str,
    relation_member: &str,
    leaf: &str,
    relation: Option<&Relation>,
) -> String {
    let Some(relation) = relation else {
        return format!(
            "use the generated E facade for {root}.{relation_member}.{leaf}; multiple TeaQL relations share this member name"
        );
    };
    let owner = relation.owner.as_str();
    let target = relation.target.as_str();
    let relation_name = relation.member.as_str();
    let suggestion = match language {
        Language::Kotlin => format!(
            "E.{}({root}).get{}().get{}().eval()",
            lower_camel(owner),
            pascal_case(relation_name),
            pascal_case(leaf)
        ),
        Language::Python => format!(
            "E.{}({root}).{}().{}().eval()",
            snake_case(owner),
            snake_case(relation_name),
            snake_case(leaf)
        ),
        Language::Csharp => format!(
            "E.{}({root}).{}().{}().Eval()",
            pascal_case(owner),
            pascal_case(relation_name),
            pascal_case(leaf)
        ),
        Language::Golang => format!(
            "E.{}({root}).{}().{}().Eval()",
            pascal_case(owner),
            pascal_case(relation_name),
            pascal_case(leaf)
        ),
        Language::Swift | Language::Typescript => format!(
            "E.{}({root}).{}().{}().eval()",
            lower_camel(owner),
            lower_camel(relation_name),
            lower_camel(leaf)
        ),
    };
    format!("use {suggestion}; related entity: {target}")
}

fn relation_members(relations: &[Relation], language: Language) -> BTreeSet<String> {
    let mut members = relations
        .iter()
        .map(|relation| language.direct_member(&relation.member))
        .collect::<BTreeSet<_>>();
    if language == Language::Golang {
        members.insert("RelationEntity".to_string());
    }
    members
}

fn discover_model_relations(roots: &[PathBuf]) -> Result<Vec<Relation>> {
    let mut model_files = BTreeSet::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(root)
            .max_depth(8)
            .follow_links(false)
            .into_iter()
            .filter_entry(catalog_entry)
        {
            let entry = entry.with_context(|| format!("failed to walk {}", root.display()))?;
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "xml" || extension == "ksml")
            {
                model_files.insert(entry.into_path());
            }
        }
    }

    let mut relations = BTreeMap::new();
    for path in model_files {
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read TeaQL model {}", path.display()))?;
        let Ok(document) = roxmltree::Document::parse(&source) else {
            continue;
        };
        let root = document.root_element();
        let entities = root
            .children()
            .filter(|node| node.is_element() && !node.tag_name().name().starts_with('_'))
            .map(|node| node.tag_name().name().to_string())
            .collect::<HashSet<_>>();
        for entity in root.children().filter(|node| node.is_element()) {
            let owner = entity.tag_name().name();
            if !entities.contains(owner) {
                continue;
            }
            for attribute in entity.attributes() {
                if attribute.name().starts_with('_') {
                    continue;
                }
                let Some(target) = attribute.value().split('(').next() else {
                    continue;
                };
                if !attribute.value().contains('(') || !entities.contains(target) {
                    continue;
                }
                let relation = Relation {
                    owner: owner.to_string(),
                    member: attribute.name().to_string(),
                    target: target.to_string(),
                };
                relations.insert((relation.owner.clone(), relation.member.clone()), relation);
            }
        }
    }
    Ok(relations.into_values().collect())
}

fn discover_expression_facade(roots: &[PathBuf], language: Language) -> Result<Option<PathBuf>> {
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(root)
            .max_depth(14)
            .follow_links(false)
            .into_iter()
            .filter_entry(catalog_entry)
        {
            let entry = entry.with_context(|| format!("failed to walk {}", root.display()))?;
            if !entry.file_type().is_file() || !language.facade_candidate(entry.path()) {
                continue;
            }
            let source = fs::read_to_string(entry.path()).with_context(|| {
                format!(
                    "failed to read expression facade {}",
                    entry.path().display()
                )
            })?;
            if language.facade_is_functional(&source) {
                return Ok(Some(entry.into_path()));
            }
        }
    }
    Ok(None)
}

fn parse_source(path: &Path, language: Language) -> Result<Syntax> {
    let source = fs::read_to_string(path).with_context(|| {
        format!(
            "failed to read {} source {}",
            language.label(),
            path.display()
        )
    })?;
    let mut parser = Parser::new();
    parser
        .set_language(&language.parser_language(path))
        .with_context(|| format!("failed to initialize {} parser", language.label()))?;
    let tree = parser
        .parse(&source, None)
        .with_context(|| format!("parser returned no tree for {}", path.display()))?;
    if tree.root_node().has_error() {
        bail!(
            "failed to parse {} source {}",
            language.label(),
            path.display()
        );
    }
    Ok(Syntax { source, tree })
}

fn mask_non_code(syntax: &Syntax) -> Vec<u8> {
    let mut masked = syntax.source.as_bytes().to_vec();
    mask_node(syntax.tree.root_node(), &mut masked);
    masked
}

fn mask_node(node: Node<'_>, masked: &mut [u8]) {
    let kind = node.kind();
    if kind.contains("comment") {
        mask_range(masked, node.start_byte(), node.end_byte());
        return;
    }
    if (kind.contains("string") || kind.contains("character")) && node.named_child_count() == 0 {
        mask_range(masked, node.start_byte(), node.end_byte());
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        mask_node(child, masked);
    }
}

fn mask_range(bytes: &mut [u8], start: usize, end: usize) {
    for value in &mut bytes[start..end] {
        if *value != b'\n' && *value != b'\r' {
            *value = b' ';
        }
    }
}

fn scan_member_chains(source: &[u8]) -> Vec<MemberChain> {
    let mut chains = Vec::new();
    let mut index = 0;
    while index < source.len() {
        if !identifier_start(source[index]) {
            index += 1;
            continue;
        }
        let start = index;
        let (root, next) = read_identifier(source, index);
        let mut segments = vec![root];
        let mut called = vec![false];
        let mut cursor = next;

        loop {
            cursor = skip_space(source, cursor);
            loop {
                cursor = skip_space(source, cursor);
                if cursor < source.len() && source[cursor] == b'(' {
                    if let Some(value) = called.last_mut() {
                        *value = true;
                    }
                    cursor = skip_balanced(source, cursor, b'(', b')');
                    continue;
                }
                if cursor < source.len() && source[cursor] == b'[' {
                    cursor = skip_balanced(source, cursor, b'[', b']');
                    continue;
                }
                break;
            }
            if cursor < source.len() && matches!(source[cursor], b'?' | b'!') {
                cursor += 1;
            }
            if cursor >= source.len() || source[cursor] != b'.' {
                break;
            }
            cursor = skip_space(source, cursor + 1);
            if cursor >= source.len() || !identifier_start(source[cursor]) {
                break;
            }
            let (segment, next_segment) = read_identifier(source, cursor);
            segments.push(segment);
            called.push(false);
            cursor = next_segment;
        }

        if segments.len() >= 2 {
            chains.push(MemberChain {
                start_byte: start,
                end_byte: cursor,
                segments,
                called,
            });
        }
        index = next.max(start + 1);
    }
    chains
}

fn identifier_start(value: u8) -> bool {
    value == b'_' || value.is_ascii_alphabetic()
}

fn identifier_continue(value: u8) -> bool {
    identifier_start(value) || value.is_ascii_digit()
}

fn read_identifier(source: &[u8], mut index: usize) -> (String, usize) {
    let start = index;
    while index < source.len() && identifier_continue(source[index]) {
        index += 1;
    }
    (
        String::from_utf8_lossy(&source[start..index]).to_string(),
        index,
    )
}

fn skip_space(source: &[u8], mut index: usize) -> usize {
    while index < source.len() && source[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn skip_balanced(source: &[u8], mut index: usize, open: u8, close: u8) -> usize {
    let mut depth = 0;
    while index < source.len() {
        if source[index] == open {
            depth += 1;
        } else if source[index] == close {
            depth -= 1;
            if depth == 0 {
                return index + 1;
            }
        }
        index += 1;
    }
    index
}

fn enclosing_function(root: Node<'_>, byte: usize, source: &str, language: Language) -> String {
    let Some(mut node) = root.descendant_for_byte_range(byte, byte.saturating_add(1)) else {
        return "<module>".to_string();
    };
    loop {
        if language.function_node(node.kind())
            && let Some(name) = node.child_by_field_name("name")
        {
            return name
                .utf8_text(source.as_bytes())
                .unwrap_or("<anonymous>")
                .to_string();
        }
        let Some(parent) = node.parent() else {
            break;
        };
        node = parent;
    }
    "<module>".to_string()
}

fn byte_line(source: &str, byte: usize) -> usize {
    source.as_bytes()[..byte.min(source.len())]
        .iter()
        .filter(|value| **value == b'\n')
        .count()
        + 1
}

fn root_expression(source: &str, byte: usize) -> String {
    let tail = &source[byte.min(source.len())..];
    let end = tail.find('.').unwrap_or(tail.len());
    tail[..end].trim().trim_end_matches(['?', '!']).to_string()
}

fn line_is_import(source: &str, byte: usize, language: Language) -> bool {
    let byte = byte.min(source.len());
    let start = source[..byte]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let end = source[byte..]
        .find('\n')
        .map(|index| byte + index)
        .unwrap_or(source.len());
    let line = source[start..end].trim_start();
    match language {
        Language::Kotlin => line.starts_with("import ") || line.starts_with("package "),
        Language::Python => line.starts_with("import ") || line.starts_with("from "),
        Language::Csharp => line.starts_with("using ") || line.starts_with("namespace "),
        Language::Golang => line.starts_with("package ") || line.starts_with("import "),
        Language::Swift => line.starts_with("import "),
        Language::Typescript => line.starts_with("import "),
    }
}

fn load_policy(
    cwd: &Path,
    explicit: Option<&Path>,
    language: Language,
) -> Result<(PolicyConfig, Option<PathBuf>)> {
    let path = explicit.map(|path| resolve(cwd, path)).or_else(|| {
        let default = cwd.join(language.default_config());
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

fn validate_policy(policy: &PolicyConfig, path: Option<&Path>, language: Language) -> Result<()> {
    if path.is_some() && policy.version != 1 {
        bail!(
            "unsupported {} policy version {} in {}",
            language.command(),
            policy.version,
            path.map(|value| value.display().to_string())
                .unwrap_or_else(|| "<defaults>".to_string())
        );
    }
    for allowance in &policy.allow {
        if allowance.path.trim().is_empty()
            || allowance.function.trim().is_empty()
            || allowance.reason.trim().is_empty()
            || allowance.rules.is_empty()
        {
            bail!(
                "every {} allowance needs path, function, rules and reason",
                language.command()
            );
        }
        for rule in &allowance.rules {
            if rule != language.direct_rule() && rule != language.facade_rule() {
                bail!("unknown {} allowance rule {rule}", language.command());
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
    } else if cwd.join("Sources").is_dir() {
        vec![PathBuf::from("Sources")]
    } else {
        vec![PathBuf::from(".")]
    };
    selected
        .into_iter()
        .map(|path| {
            let resolved = resolve(cwd, &path);
            if !resolved.exists() {
                bail!(
                    "expression source root does not exist: {}",
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

fn nearby_catalog_roots(cwd: &Path) -> Vec<PathBuf> {
    let mut roots = vec![cwd.to_path_buf()];
    let mut current = cwd;
    for _ in 0..3 {
        if current.join(".git").exists() {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == Path::new("/") {
            break;
        }
        roots.push(parent.to_path_buf());
        current = parent;
    }
    roots
}

fn collect_source_files(
    cwd: &Path,
    roots: &[PathBuf],
    excludes: &[PathBuf],
    language: Language,
) -> Result<Vec<PathBuf>> {
    let mut files = BTreeSet::new();
    for root in roots {
        if root.is_file() {
            if language.matches_extension(root) && !language.generated_file(root) {
                files.insert(root.clone());
            }
            continue;
        }
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| source_entry(entry, cwd, excludes, language))
        {
            let entry = entry.with_context(|| format!("failed to walk {}", root.display()))?;
            if entry.file_type().is_file()
                && language.matches_extension(entry.path())
                && !language.generated_file(entry.path())
            {
                files.insert(entry.into_path());
            }
        }
    }
    Ok(files.into_iter().collect())
}

fn source_entry(entry: &DirEntry, cwd: &Path, excludes: &[PathBuf], language: Language) -> bool {
    let path = entry.path();
    let relative = path.strip_prefix(cwd).unwrap_or(path);
    if path != cwd
        && relative.components().any(|component| {
            let Component::Normal(value) = component else {
                return false;
            };
            language
                .excluded_components()
                .contains(&value.to_string_lossy().as_ref())
        })
    {
        return false;
    }
    !excludes.iter().any(|excluded| path.starts_with(excluded))
}

fn catalog_entry(entry: &DirEntry) -> bool {
    !entry.path().components().any(|component| {
        let Component::Normal(value) = component else {
            return false;
        };
        matches!(
            value.to_string_lossy().as_ref(),
            ".git" | "target" | "build" | "node_modules" | "venv" | ".venv" | ".build"
        )
    })
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

fn print_report(report: &Report, format: ExpressionCheckOutput, language: Language) -> Result<()> {
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
                    "{} passed: {} {} file(s), {} audited exception(s)",
                    report.command,
                    report.files_scanned,
                    language.label(),
                    report.suppressed
                );
            } else {
                eprintln!(
                    "{} failed: {} violation(s) in {} {} file(s), {} audited exception(s)",
                    report.command,
                    report.findings.len(),
                    report.files_scanned,
                    language.label(),
                    report.suppressed
                );
            }
        }
    }
    Ok(())
}

fn words(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .flat_map(|part| {
            let mut pieces = Vec::new();
            let mut start = 0;
            let chars = part.char_indices().collect::<Vec<_>>();
            for index in 1..chars.len() {
                let (_, current) = chars[index];
                let (_, previous) = chars[index - 1];
                if current.is_ascii_uppercase() && previous.is_ascii_lowercase() {
                    pieces.push(part[start..chars[index].0].to_ascii_lowercase());
                    start = chars[index].0;
                }
            }
            pieces.push(part[start..].to_ascii_lowercase());
            pieces
        })
        .collect()
}

fn pascal_case(value: &str) -> String {
    words(value)
        .into_iter()
        .map(|word| {
            let mut characters = word.chars();
            characters
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + characters.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn lower_camel(value: &str) -> String {
    let pascal = pascal_case(value);
    let mut characters = pascal.chars();
    characters
        .next()
        .map(|first| first.to_ascii_lowercase().to_string() + characters.as_str())
        .unwrap_or_default()
}

fn snake_case(value: &str) -> String {
    words(value).join("_")
}

impl Language {
    fn label(self) -> &'static str {
        match self {
            Self::Kotlin => "Kotlin",
            Self::Python => "Python",
            Self::Csharp => "C#",
            Self::Golang => "Go",
            Self::Swift => "Swift",
            Self::Typescript => "TypeScript",
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Kotlin => "kotlin-expression-check",
            Self::Python => "python-expression-check",
            Self::Csharp => "csharp-expression-check",
            Self::Golang => "golang-expression-check",
            Self::Swift => "swift-expression-check",
            Self::Typescript => "typescript-expression-check",
        }
    }

    fn direct_rule(self) -> &'static str {
        match self {
            Self::Kotlin => "TQL-KOTLIN-EXPR-001",
            Self::Python => "TQL-PYTHON-EXPR-001",
            Self::Csharp => "TQL-CSHARP-EXPR-001",
            Self::Golang => "TQL-GOLANG-EXPR-001",
            Self::Swift => "TQL-SWIFT-EXPR-001",
            Self::Typescript => "TQL-TYPESCRIPT-EXPR-001",
        }
    }

    fn facade_rule(self) -> &'static str {
        match self {
            Self::Kotlin => "TQL-KOTLIN-EXPR-002",
            Self::Python => "TQL-PYTHON-EXPR-002",
            Self::Csharp => "TQL-CSHARP-EXPR-002",
            Self::Golang => "TQL-GOLANG-EXPR-002",
            Self::Swift => "TQL-SWIFT-EXPR-002",
            Self::Typescript => "TQL-TYPESCRIPT-EXPR-002",
        }
    }

    fn default_config(self) -> &'static str {
        match self {
            Self::Kotlin => ".teaql/kotlin-expression-check.yml",
            Self::Python => ".teaql/python-expression-check.yml",
            Self::Csharp => ".teaql/csharp-expression-check.yml",
            Self::Golang => ".teaql/golang-expression-check.yml",
            Self::Swift => ".teaql/swift-expression-check.yml",
            Self::Typescript => ".teaql/typescript-expression-check.yml",
        }
    }

    fn parser_language(self, path: &Path) -> TreeSitterLanguage {
        match self {
            Self::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Csharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Self::Golang => tree_sitter_go::LANGUAGE.into(),
            Self::Swift => tree_sitter_swift::LANGUAGE.into(),
            Self::Typescript if path.extension().is_some_and(|value| value == "tsx") => {
                tree_sitter_typescript::LANGUAGE_TSX.into()
            }
            Self::Typescript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Kotlin => "kt",
            Self::Python => "py",
            Self::Csharp => "cs",
            Self::Golang => "go",
            Self::Swift => "swift",
            Self::Typescript => "ts",
        }
    }

    fn matches_extension(self, path: &Path) -> bool {
        if self == Self::Typescript {
            return path
                .extension()
                .is_some_and(|extension| extension == "ts" || extension == "tsx");
        }
        path.extension()
            .is_some_and(|extension| extension == self.extension())
    }

    fn excluded_components(self) -> &'static [&'static str] {
        match self {
            Self::Kotlin => &[
                ".git",
                ".gradle",
                "target",
                "build",
                "java-lib-core",
                "generated",
            ],
            Self::Python => &[
                ".git",
                "venv",
                ".venv",
                "site-packages",
                "__pycache__",
                "build",
                "dist",
                "lib",
            ],
            Self::Csharp => &[
                ".git",
                "obj",
                "bin",
                "Models",
                "Generated",
                "dotnet-lib-core",
            ],
            Self::Golang => &[".git", "vendor", "lib", "golang-lib-core", "generated"],
            Self::Swift => &[".git", ".build", "Generated", "GeneratedTeaQL"],
            Self::Typescript => &[
                ".git",
                "node_modules",
                "dist",
                "build",
                "coverage",
                "generated",
                "generated-lib",
                "ts-lib-core",
                "typescript-node-lib-core",
            ],
        }
    }

    fn generated_file(self, path: &Path) -> bool {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        match self {
            Self::Kotlin => false,
            Self::Python => name == "expression.py",
            Self::Csharp => matches!(name, "E.cs" | "Q.cs" | "GeneratedRuntimeModule.cs"),
            Self::Golang => matches!(
                name,
                "e.go" | "q.go" | "runtime.go" | "entity.go" | "expression.go" | "request.go"
            ),
            Self::Swift => matches!(name, "E.swift" | "Q.swift" | "RuntimeModule.swift"),
            Self::Typescript => {
                path.to_string_lossy().ends_with(".d.ts")
                    || matches!(name, "E.ts" | "Q.ts" | "generated-bootstrap.ts")
            }
        }
    }

    fn facade_candidate(self, path: &Path) -> bool {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        match self {
            Self::Kotlin => name == "E.java",
            Self::Python => name == "expression.py" || name == "e.py",
            Self::Csharp => name == "E.cs",
            Self::Golang => name == "e.go",
            Self::Swift => name == "E.swift",
            Self::Typescript => name == "E.ts",
        }
    }

    fn facade_is_functional(self, source: &str) -> bool {
        match self {
            Self::Kotlin => source.contains("class E") && source.contains("Expression<"),
            Self::Python => {
                let Some(after_e) = source.split("class E").nth(1) else {
                    return false;
                };
                after_e.contains("def ")
            }
            Self::Csharp => {
                source.contains("static class E")
                    && source.contains("static ")
                    && source.contains("Expression")
            }
            Self::Golang => source.contains("var E") && source.contains("func (expressionFacade)"),
            Self::Swift => source.contains("enum E") && source.contains("static func"),
            Self::Typescript => source.contains("class E") && source.contains("static "),
        }
    }

    fn direct_member(self, model_member: &str) -> String {
        match self {
            Self::Kotlin => lower_camel(model_member),
            Self::Python => snake_case(model_member),
            Self::Csharp => format!("{}Entity", pascal_case(model_member)),
            Self::Golang => pascal_case(model_member),
            Self::Swift => format!("{}Entity", lower_camel(model_member)),
            Self::Typescript => lower_camel(model_member),
        }
    }

    fn function_node(self, kind: &str) -> bool {
        match self {
            Self::Kotlin => matches!(kind, "function_declaration" | "secondary_constructor"),
            Self::Python => matches!(kind, "function_definition" | "lambda"),
            Self::Csharp => matches!(
                kind,
                "method_declaration" | "local_function_statement" | "constructor_declaration"
            ),
            Self::Golang => matches!(kind, "function_declaration" | "method_declaration"),
            Self::Swift => matches!(kind, "function_declaration" | "init_declaration"),
            Self::Typescript => matches!(
                kind,
                "function_declaration"
                    | "method_definition"
                    | "generator_function_declaration"
                    | "arrow_function"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(source: &str, language: Language, relation: &str) -> usize {
        let mut parser = Parser::new();
        parser
            .set_language(&language.parser_language(Path::new("fixture.ts")))
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error());
        let syntax = Syntax {
            source: source.to_string(),
            tree,
        };
        let masked = mask_non_code(&syntax);
        let members = BTreeSet::from([relation.to_string()]);
        let chains = scan_member_chains(&masked);
        let expression_ranges = chains
            .iter()
            .filter(|chain| chain.segments.first().is_some_and(|root| root == "E"))
            .map(|chain| (chain.start_byte, chain.end_byte))
            .collect::<Vec<_>>();
        chains
            .into_iter()
            .filter(|chain| {
                !expression_ranges
                    .iter()
                    .any(|(start, end)| chain.start_byte >= *start && chain.start_byte < *end)
            })
            .filter(|chain| matched_relation(chain, &members).is_some())
            .count()
    }

    #[test]
    fn detects_kotlin_safe_call_relation_chain() {
        assert_eq!(
            findings(
                "fun render(order: Order) = order.status?.code ?: \"\"",
                Language::Kotlin,
                "status"
            ),
            1
        );
    }

    #[test]
    fn detects_python_relation_chain() {
        assert_eq!(
            findings(
                "def render(task):\n    return task.status.code\n",
                Language::Python,
                "status"
            ),
            1
        );
    }

    #[test]
    fn detects_csharp_relation_chain() {
        assert_eq!(
            findings(
                "class App { string Render(School school) => school.PlatformEntity?.Name; }",
                Language::Csharp,
                "PlatformEntity"
            ),
            1
        );
    }

    #[test]
    fn detects_go_low_level_relation_access() {
        assert_eq!(
            findings(
                "package app\nfunc render(s *School) { value, ok := s.RelationEntity(\"platformEntity\") }",
                Language::Golang,
                "RelationEntity"
            ),
            1
        );
    }

    #[test]
    fn detects_swift_relation_chain() {
        assert_eq!(
            findings(
                "func render(_ school: School) -> String? { school.platformEntity?.name }",
                Language::Swift,
                "platformEntity"
            ),
            1
        );
    }

    #[test]
    fn detects_typescript_optional_relation_chain() {
        assert_eq!(
            findings(
                "function render(item: WorkItem) { return item.platform?.name ?? ''; }",
                Language::Typescript,
                "platform"
            ),
            1
        );
    }

    #[test]
    fn parses_tsx_with_the_tsx_grammar() {
        let mut parser = Parser::new();
        parser
            .set_language(&Language::Typescript.parser_language(Path::new("view.tsx")))
            .unwrap();
        let tree = parser
            .parse(
                "const View = ({ item }: Props) => <span>{item.platform?.name}</span>;",
                None,
            )
            .unwrap();
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn typescript_command_rejects_direct_access_and_accepts_e_expression() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join(".git")).unwrap();
        fs::create_dir_all(workspace.path().join("src/generated")).unwrap();
        fs::write(
            workspace.path().join("model.xml"),
            r#"<root><platform name="string()"/><work_item platform="platform()"/></root>"#,
        )
        .unwrap();
        fs::write(
            workspace.path().join("src/generated/E.ts"),
            "export class E { static workItem(value: unknown) { return value; } }",
        )
        .unwrap();
        let application = workspace.path().join("src/app.ts");
        fs::write(
            &application,
            "export function render(item: WorkItem) { return item.platform?.name; }",
        )
        .unwrap();

        let args = || LanguageExpressionCheckArgs {
            sources: vec![PathBuf::from("src")],
            excludes: Vec::new(),
            config: None,
            format: ExpressionCheckOutput::Json,
        };
        let error = run(workspace.path(), args(), Language::Typescript).unwrap_err();
        assert!(
            error.to_string().contains("found 1 violation"),
            "unexpected error: {error:#}"
        );

        fs::write(
            application,
            "export function render(item: WorkItem) { return E.workItem(item).platform().name().eval(); }",
        )
        .unwrap();
        run(workspace.path(), args(), Language::Typescript).unwrap();
    }

    #[test]
    fn ignores_expression_facade_chain() {
        assert_eq!(
            findings(
                "fun render(value: Order) = E.vendingOrder(value).getStatus().getCode().eval()",
                Language::Kotlin,
                "getStatus"
            ),
            0
        );
    }

    #[test]
    fn model_name_conversions_are_stable() {
        assert_eq!(pascal_case("school_type"), "SchoolType");
        assert_eq!(lower_camel("school_type"), "schoolType");
        assert_eq!(snake_case("SchoolType"), "school_type");
    }
}
