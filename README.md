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

`gen-model` follows the current `generateModel.sh` behavior and sends `scope=frontend`.

## Config

Local config lives in `~/.teaql/config.yml`.

Example:

```yaml
service_url: http://springboot.teaql-gen-code.1496855407387739.cn-chengdu.fc.devsapp.net/generate
license_file: assets/public.LICENSE
build_dir: build
timeout_seconds: 300
```

If `license_file` is omitted, the bundled `assets/public.LICENSE` is used.
