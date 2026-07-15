# teaql-cli

[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/13620/badge)](https://www.bestpractices.dev/projects/13620)

Rust CLI for TeaQL code generation workflows.

## Commands

```bash
cargo-teaql <target-service> <model-path>

# Examples:
cargo-teaql java-app-console ./model
cargo-teaql rust-lib-core ./model
cargo-teaql services
cargo-teaql version
cargo-teaql show-config
cargo-teaql config
cargo-teaql install-links
```

The CLI supports dynamic command routing. If the target service is not a built-in command, the CLI will automatically forward the request to the backend. If an input path is provided, it acts as a code generation target (e.g. `java-app-console`). If no input path is provided, it acts as a general GET request (e.g. `services`).

If no arguments are provided at all, it automatically defaults to `cargo-teaql services`.

### CLI flags

```bash
cargo-teaql java-app-console <model-path> \
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
cargo teaql-java-app-console <model-path>
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

The command format is:
`cargo-teaql <lang>-assist-<action>/<entity> --input <model_file>`

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
cargo-teaql rust-assist-query/book --input modeling/bookstore.xml
cargo-teaql java-assist-create/customer --input modeling/bookstore.xml
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
