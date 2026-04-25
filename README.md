# teaql-cli

Rust CLI for TeaQL code generation workflows.

## Commands

```bash
teaql gen-code <model-path>
teaql gen-doc <model-path>
teaql gen-model <model-path>
teaql show-config
teaql config
```

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
