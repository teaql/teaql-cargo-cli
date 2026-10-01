# teaql-cli

[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/13620/badge)](https://www.bestpractices.dev/projects/13620)

Rust CLI for evaluating TeaQL semantic models, generating language-native
libraries and workspaces, and discovering generated APIs through model-aware
Assist.

## Commands

```bash
# Discover the targets currently offered by the service.
cargo teaql services

# Evaluate before generation.
cargo teaql evaluate --input models/

# Generate a library or runnable workspace.
cargo teaql rust-lib-core --input models/ --output build/
cargo teaql java-app-console --input models/ --output build/

# Discover the generated API progressively.
cargo teaql rust-assist-query/school --input models/
cargo teaql rust-assist-query/school.established_date --input models/

# Local maintenance commands.
cargo teaql show-config
cargo teaql config
cargo teaql ping
cargo teaql check --tests
cargo teaql rust-expression-check --source src
cargo teaql java-expression-check --source src
cargo teaql kotlin-expression-check --source src
cargo teaql python-expression-check --source src
cargo teaql csharp-expression-check --source src
cargo teaql golang-expression-check --source src
cargo teaql swift-expression-check --source Sources
cargo teaql typescript-expression-check --source src
```

Generation and Assist targets are provided dynamically by the TeaQL service,
so the fixed `Commands` list in `--help` is not their complete inventory. Run
`cargo teaql services` to discover the current targets and `cargo teaql version`
to inspect their versions.

Use `--input` for the KSML model file or directory. A dynamic target with model
input performs evaluation, generation, or Assist; a remote information target
such as `services` or `version` performs a GET request.

If no command is provided, the CLI defaults to `cargo teaql services`.
`cargo-teaql` is an equivalent direct invocation.

### Rust expression policy check

`cargo teaql rust-expression-check` parses application-owned Rust source and
fails when a TeaQL read query bypasses the generated `E` expression facade or
uses `select_self_fields()` outside a mutation flow. It also recognizes
centralized expression gateways such as `LoadedAsset::from_selected`, so a
service can evaluate a partial entity once and return an application read
model.

TeaQL-owned diagnostics use the `TQL-` prefix so they are distinct from Rust
compiler and Clippy diagnostics:

- `TQL-RUST-EXPR-001`: a TeaQL query result is consumed without E-expression evaluation.
- `TQL-RUST-EXPR-002`: a read query uses `select_self_fields()` instead of an explicit projection.

```bash
cargo teaql rust-expression-check \
  --source rust-web-axum/src \
  --exclude rust-web-axum/src/generated
```

The default output is designed for humans and coding agents. Use
`--format json` for CI integrations. Projects can keep audited exceptions in
`.teaql/rust-expression-check.yml`:

```yaml
version: 1
source_roots:
  - rust-web-axum/src
exclude_roots:
  - rust-lib-core
allow:
  - path: rust-web-axum/src/services/asset_service.rs
    function: find_by_path
    rules: [TQL-RUST-EXPR-001, TQL-RUST-EXPR-002]
    reason: full entity is loaded only for mutation hydration
```

Exceptions are exact function-level records with mandatory reasons; they are
not wildcard suppressions. Keep the policy file under code review. Generated
directories such as `rust-lib-core`, `generate-lib`, and `bizcore` are excluded
by default.

### Java expression policy check

`cargo teaql java-expression-check` parses Java with Tree-sitter and reports
direct getter chains across TeaQL entity relations. It discovers entity and
relation types from generated `E.java` and entity sources, so ordinary Java,
Android, and framework getter chains are not treated as TeaQL expressions.

- `TQL-JAVA-EXPR-001`: a TeaQL entity relation is accessed through a direct getter chain instead of `E`.

```bash
cargo teaql java-expression-check \
  --source app/src/main/java \
  --format json
```

Generated `java-lib-core`, `target`, `build`, and `generated` directories are
excluded from application diagnostics. The generated library is still read as
the domain catalog. When the application module is the working directory, a
sibling `java-lib-core` directory is discovered automatically.

Audited exceptions can be kept in `.teaql/java-expression-check.yml`:

```yaml
version: 1
source_roots:
  - app/src/main/java
allow:
  - path: app/src/main/java/example/LegacyView.java
    function: renderStatus
    rules: [TQL-JAVA-EXPR-001]
    reason: legacy view awaiting E-expression migration
```

### Kotlin, Python, C#, Go, Swift, and TypeScript expression checks

The same local policy workflow is available for six additional TeaQL
ecosystems:

| Command | Direct-access rule | Capability rule |
| --- | --- | --- |
| `kotlin-expression-check` | `TQL-KOTLIN-EXPR-001` | `TQL-KOTLIN-EXPR-002` |
| `python-expression-check` | `TQL-PYTHON-EXPR-001` | `TQL-PYTHON-EXPR-002` |
| `csharp-expression-check` | `TQL-CSHARP-EXPR-001` | `TQL-CSHARP-EXPR-002` |
| `golang-expression-check` | `TQL-GOLANG-EXPR-001` | `TQL-GOLANG-EXPR-002` |
| `swift-expression-check` | `TQL-SWIFT-EXPR-001` | `TQL-SWIFT-EXPR-002` |
| `typescript-expression-check` | `TQL-TYPESCRIPT-EXPR-001` | `TQL-TYPESCRIPT-EXPR-002` |

The `001` rules report direct TeaQL relation traversal. The `002` rules fail
closed when the model relation metadata or a functional generated `E` facade
cannot be found. This prevents a project from appearing compliant when its
runtime cannot express the required safe access.

The TypeScript check scans both `.ts` and `.tsx`. For example,
`item.platform?.name` is reported, while
`E.workItem(item).platform().name().eval()` is accepted.

These checks parse each language with its Tree-sitter grammar and derive
relation names from workspace KSML/XML models. Generated code, dependency
caches, and build output are excluded from application diagnostics. If a model
is supplied only as a package, relation members can be declared explicitly:

```yaml
version: 1
source_roots:
  - src
relation_members:
  - status
  - platform
allow:
  - path: src/legacy_view.py
    function: render_status
    rules: [TQL-PYTHON-EXPR-001]
    reason: legacy view awaiting E-expression migration
```

Store the policy at `.teaql/<language>-expression-check.yml`, using `kotlin`,
`python`, `csharp`, `golang`, `swift`, or `typescript` as the language name.
All commands support `--source`, `--exclude`, `--config`, and
`--format text|json`.

### CLI flags

```bash
cargo teaql java-app-console --input models/ \
  --endpoint-prefix https://api.teaql.io/latest/ \
  --api-key ******** \
  --output ./build \
  --timeout-seconds 300 \
  --cwd /workspace/project
```

### Directory vs Single File Upload
When a **directory** is provided as input, `cargo-teaql` will normally compress it into a zip archive. The server expects exactly one file in the zip to be named `main.xml` to serve as the entry point.
However, if a directory is provided but it **does not contain a `main.xml`**, the CLI will search for a single `.xml` or `.ksml` model file. If it finds exactly one such file, it will **bypass compression** and upload that single file directly. Single-file uploads do not require the name `main.xml`. If multiple files are found and no `main.xml` is present, the CLI will abort with an error.

### Symlink aliases

If you create symlink aliases to the same binary, these names also work:

```bash
cargo teaql-java-app-console --input models/
cargo teaql-services
cargo teaql-version
cargo teaql-show-config
cargo teaql-config
```

Install the aliases automatically:

```bash
cargo-teaql install-links
```

You can also target a custom directory with `cargo-teaql install-links --dir /some/bin --force`.

## AI Agent Assist Commands

TeaQL provides specialized assist commands designed to help AI coding agents (and developers) quickly generate safe, compliant boilerplate templates for various business scenarios. 

The entity-level command format is:
`cargo teaql <lang>-assist-<action>/<entity> --input <model>`

For field-specific help, append the KSML field name:
`cargo teaql <lang>-assist-<action>/<entity>.<field> --input <model>`

Currently supported `rust-assist-*` targets:
- `rust-assist-query`: Generate a comprehensive query template with field selections and mandatory `.purpose()`/`.comment()` cascades.
- `rust-assist-list-page`: Generate a paginated query list template.
- `rust-assist-create`: Generate an entity creation template with proper `.audit_as()` constraints.
- `rust-assist-update`: Generate an entity update template.
- `rust-assist-delete`: Generate an entity deletion template safely.
- `rust-assist-expression`: Generate advanced business expression calculation templates using `E::`.

*(Note: The exact same variations are also available for `java-assist-*`)*

**Example Usage:**
```bash
cargo teaql rust-assist-query/book --input modeling/bookstore.xml
cargo teaql rust-assist-query/book.published_at --input modeling/bookstore.xml
cargo teaql java-assist-create/customer --input modeling/bookstore.xml
```

## Configuration

### Precedence (highest wins)

```
CLI flag  >  Environment variable  >  config.yml  >  Built-in default
```

### Environment variables

| Env var | Config key | Description |
|---|---|---|
| `TEAQL_ENDPOINT_PREFIX` | `endpoint_prefix` | TeaQL service endpoint prefix |
| `TEAQL_LICENSE_FILE` | `license_file` | License file path |
| `TEAQL_API_KEY` | `api_key` | API Key for service access |
| `TEAQL_BUILD_DIR` | `build_dir` | Output directory |
| `TEAQL_TIMEOUT_SECONDS` | `timeout_seconds` | HTTP timeout in seconds |

### API Key (OOTB Free Usage)

The CLI uses a built-in default Out-Of-The-Box (OOTB) API key. 
This default key is fully functional and can be used freely for development, testing, and experimentation. **Users and AI agents do not need to sign up or search for a real API key to get started.** The service will accept this default key. 

If you have a dedicated API key for higher rate limits or production usage, you can provide it via the `--api-key` CLI flag, the `TEAQL_API_KEY` environment variable, or in your `config.yml`.

`TEAQL_SERVICE_URL` is still accepted for compatibility, but new
configuration should use `TEAQL_ENDPOINT_PREFIX`.

### Config file

Local config lives in `~/.teaql/config.yml`.

```yaml
endpoint_prefix: https://api.teaql.io/latest/
api_key: "YOUR_API_KEY"           # optional — built-in free OOTB key used if omitted
build_dir: build
timeout_seconds: 300
```

The endpoint prefix is combined with service methods. For example, generation
uses `https://api.teaql.io/latest/generate`, and `cargo-teaql version` uses
`https://api.teaql.io/latest/version`.

### Source tracking

At startup, the CLI prints where each effective config value came from:

```
  config (precedence: cli > env > config.yml > default):
    endpoint_prefix = https://api.teaql.io/latest/          (from: environment variable)
    api_key         = ********                              (from: built-in default)
    build_dir       = /workspace/build                (from: built-in default)
    timeout_seconds = 300                             (from: ~/.teaql/config.yml)
```
