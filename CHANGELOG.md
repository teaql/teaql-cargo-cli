# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Add `cargo teaql rust-expression-check` for enforcing E-expression reads and
  explicit projections in application-owned Rust query paths.
- Add `cargo teaql java-expression-check` for enforcing E-expression access
  across generated TeaQL entity relations without flagging ordinary Java getter chains.
- Add model-aware E-expression checks for Kotlin, Python, C#, Go, Swift, and TypeScript,
  including fail-closed diagnostics when a generated E facade is unavailable.

## [2.0.12] - 2026-08-21

### Changed
- Set default HTTP request timeout to 300 seconds for large KSML generation workflows
