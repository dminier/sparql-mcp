# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] — Unreleased

Initial public cut. Extracted from a larger private monorepo and scrubbed
of domain-specific content.

### Added
- `sparql-mcp` server crate (Oxigraph-backed SPARQL 1.1 MCP server).
- Core `smc:` ontology (`ontology/1-smc.ttl`).
- `kb-workbench` Claude Code plugin and skill.
- Shared SSE bridge script for multi-agent store sharing.
- Docker / docker-compose manifests.
- CI: fmt, clippy (-D warnings), tests.
