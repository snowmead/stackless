# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

## v0.1.0 — 2026-07-04

First public release of the stackless CLI for agents and automation.

### Added

- Lifecycle verbs: `up`, `down`, `verify`, `status`, `list`, `logs`, `check`
- Authoring and preflight: `init`, `adopt`, `doctor`
- Global `--json` with stable `error.code`, `remediation`, and optional `context`
- NDJSON progress on stderr during `up --json`
- Substrates: `local`, `render`, `vercel`, `fly`, `netlify`
- Prebuilt binaries for macOS (Apple Silicon + Intel), Linux (x64 + ARM64), and Windows (x64)
- Shell installer: `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/snowmead/stackless/releases/download/v0.1.0/stackless-installer.sh | sh`

### Install

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/snowmead/stackless/releases/download/v0.1.0/stackless-installer.sh | sh
stackless --version
```

### Release

To publish this version (if not already tagged):

```sh
git tag -a v0.1.0 -m "stackless v0.1.0"
git push origin v0.1.0
```
