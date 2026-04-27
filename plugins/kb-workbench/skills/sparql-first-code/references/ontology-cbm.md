# `cbm:` ontology cheat-sheet

Mirrors the runtime emitter in `crates/sparql-mcp-core/src/cbm/turtle.rs`
(`emit_node`, `emit_edge`, `edge_type_to_property`). Use these names verbatim
in your SPARQL queries.

## Namespaces

```turtle
@prefix cbm:  <http://codebase-memory.dev/ontology#> .
@prefix inst: <http://codebase-memory.dev/instance/> .
```

## Node classes (used as rdf:type)

| Class | Source label in cbm graph |
|---|---|
| `cbm:Function` | Function |
| `cbm:Method` | Method |
| `cbm:Class` | Class |
| `cbm:Interface` | Interface |
| `cbm:Enum` | Enum |
| `cbm:Type` | Type |
| `cbm:Variable` | Variable |
| `cbm:Field` | Field |
| `cbm:Module` | Module |
| `cbm:File` | File |
| `cbm:Folder` | Folder |
| `cbm:Route` | Route |
| `cbm:Section` | Section |
| `cbm:Project` | Project |

## Node properties

`cbm:name`, `cbm:qualifiedName`, `cbm:filePath`, `cbm:extension`,
`cbm:startLine` (xsd:integer), `cbm:endLine`, `cbm:lines`, `cbm:complexity`,
`cbm:signature`, `cbm:docstring`,
`cbm:isExported` (xsd:boolean), `cbm:isEntryPoint`, `cbm:isTest`.

> **Important** — all node properties are **camelCase**, not snake_case. The
> emitter is `cbm/turtle.rs::emit_node`; consult its source if a new property
> shows up. Don't write `cbm:file_path` / `cbm:start_line` / `cbm:qualified_name`
> — those return zero rows.

## Edge properties (lowerCamel from edge_type)

`cbm:calls`, `cbm:defines`, `cbm:definesMethod`, `cbm:definesRoute`,
`cbm:containsFile`, `cbm:containsFolder`, `cbm:usage`, `cbm:implements`,
`cbm:configures`, `cbm:writes`, `cbm:similarTo`, `cbm:semanticallyRelated`,
`cbm:raises`, `cbm:httpCalls`.

> **Convention** — edge types in the cbm graph are SCREAMING_SNAKE
> (`CALLS`, `DEFINES_METHOD`, `HTTP_CALLS`). The Turtle emitter rewrites them
> to **lowerCamelCase** (`calls`, `definesMethod`, `httpCalls`) — see
> `crates/sparql-mcp-core/src/cbm/turtle.rs::edge_type_to_property`. Always
> query `cbm:calls`, never `cbm:CALLS` (returns 0). Classes stay PascalCase
> (`cbm:Function`, `cbm:Class`), node properties stay lowerCamel
> (`cbm:filePath`, `cbm:startLine`).

## Ingestion meta (in `urn:cbm:meta`)

| Property | Type | Required |
|---|---|---|
| `cbm:CodeIngestion` (rdf:type) | — | yes |
| `cbm:repo` | xsd:string | yes |
| `cbm:repoPath` | xsd:string | yes |
| `cbm:ingestedAt` | xsd:dateTime | yes |
| `cbm:provenance` | xsd:string | yes |
| `cbm:provenanceNote` | xsd:string | when not git |
| `cbm:gitBranch` | xsd:string | git only |
| `cbm:gitCommit` | xsd:string | git only |
| `cbm:gitCommitShort` | xsd:string | git only |
| `cbm:gitCommitDate` | xsd:dateTime | git only |
| `cbm:gitCommitMsg` | xsd:string | git only |
| `cbm:gitDirty` | xsd:boolean | git only |
| `cbm:gitRemote` | xsd:string | git only |
| `cbm:nodeCount` | xsd:integer | yes |
| `cbm:edgeCount` | xsd:integer | yes |
| `cbm:purgedAt` | xsd:dateTime | added on purge |
