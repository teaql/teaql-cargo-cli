# teaql-cli

Rust CLI for TeaQL code generation workflows.

## Commands

```bash
cargo-teaql gen-code <model-path>
cargo-teaql gen-doc <model-path>
cargo-teaql gen-model <model-path>
cargo-teaql show-config
cargo-teaql config
cargo-teaql install-links
```

### CLI flags

```bash
cargo-teaql gen-code <model-path> \
  --service-url https://api.teaql.io/latest/ \
  --license-file /path/to/license \
  --output ./build \
  --timeout-seconds 300 \
  --cwd /workspace/project
```

### Symlink aliases

If you create symlink aliases to the same binary, these names also work:

```bash
cargo teaql-gen-code <model-path>
cargo teaql-gen-doc <model-path>
cargo teaql-gen-model <model-path>
cargo teaql-show-config
cargo teaql-config
```

Install the aliases automatically:

```bash
cargo-teaql install-links
```

This creates these symlinks next to the current executable:

```bash
teaql
cargo-teaql-gen-code
cargo-teaql-gen-doc
cargo-teaql-gen-model
cargo-teaql-show-config
cargo-teaql-config
```

You can also target a custom directory with `cargo-teaql install-links --dir /some/bin --force`.

`gen-model` sends `scope=frontend` to the service.

## Configuration

### Precedence (highest wins)

```
CLI flag  >  Environment variable  >  config.yml  >  Built-in default
```

### Environment variables

| Env var | Config key | Description |
|---|---|---|
| `TEAQL_SERVICE_URL` | `service_url` | Service endpoint URL |
| `TEAQL_LICENSE_FILE` | `license_file` | License file path |
| `TEAQL_BUILD_DIR` | `build_dir` | Output directory |
| `TEAQL_TIMEOUT_SECONDS` | `timeout_seconds` | HTTP timeout in seconds |

### Config file

Local config lives in `~/.teaql/config.yml`.

```yaml
service_url: https://api.teaql.io/latest/
license_file: /path/to/your.LICENSE   # optional — bundled public.LICENSE used if omitted
build_dir: build
timeout_seconds: 300
```

### Source tracking

At startup, the CLI prints where each effective config value came from:

```
  config (precedence: cli > env > config.yml > default):
    service_url     = https://api.teaql.io/latest/  (from: environment variable)
    license_file    = /home/user/.teaql/license       (from: ~/.teaql/config.yml)
    build_dir       = /workspace/build                (from: built-in default)
    timeout_seconds = 300                             (from: ~/.teaql/config.yml)
```
