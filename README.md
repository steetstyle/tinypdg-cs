# tiny-pdg-cs

C# program dependency graph builder. Parses C# source with tree-sitter, builds CFG and PDG, resolves calls through DI/reflection/CHA, and detects design patterns from structural and graph signals alone -- no naming heuristics.

Made for LLM-driven root cause analysis on C# microservices. The idea is to build a code dependency graph you can traverse systematically instead of dumping everything into a context window.

## What it does

Nine CLI commands:

```
parse      -- AST + type graph as JSON
cfg        -- control flow graph (DOT or JSON)
pdg        -- program dependence graph (control + data deps)
hammock    -- hammock block identification
resolve    -- call resolution (DI, reflection, CHA, virtual dispatch)
callgraph  -- focused call graph for a class, with dispatch tracing
detect     -- design pattern detection (GoF 22 + .NET idioms)
serve      -- HTTP server for tool integration
```

## Design pattern detection

All 22 GoF patterns plus .NET-specific ones (ControllerApi, MinimalApi, DotnetMediator, Handler, DiContainer). Detection is purely structural and graph-based:

- interface-implementor relationships
- method return types and parameter types
- class hierarchy and inheritance depth
- call graph signals (self-calls, object creations, dispatch patterns)
- field declarations (static cache detection for Flyweight)
- AST attributes and method signatures

No regex on class names, no "if name contains 'Factory'" tricks. Strategy and Command look the same on paper but differ in how they're called (Strategy: injected as parameter/constructor; Command: created via `new` + `Execute()` called externally; State: implementor self-calls the interface method).

Confidence scores accompany each detection. Integration tested against the Refactoring Guru C# examples and dotnet/eShop (494 files, 366 classes, 39 interfaces).

## Call resolution

Six categories with confidence estimates:

| Category | Example | Confidence |
|---|---|---|
| Direct/static | `Foo.Bar()` | 100% |
| Virtual/override | CHA, RTA | 50-100% |
| Abstract class | Type hierarchy scan | 60-90% |
| Interface dispatch | Single impl -> devirtual (95%), multi -> CHA set | 40-95% |
| DI container | `AddScoped<TIf, TImpl>()` registrations | 30-95% |
| Reflection/dynamic | `typeof(Foo).GetMethod("Bar")` | 0-80% |

## Multi-language core

The plan is to share CFG/PDG/Hammock infrastructure across languages. Currently C# only. TypeScript, Python, Go, Java parsers would plug into the same petgraph IR.

## Project structure

```
src/
  parse/          tree-sitter C# grammar wrapper + AST visitor
  cfg/            basic block construction, control flow, structured regions
  pdg/            post-dominator tree, control deps, reaching defs, data deps
  hammock/        hammock block identification
  resolve/        type graph, call targets, symbol table, DI/factory/reflection/CHA
  detect/         GoF 22 + .NET pattern detection
  graph/          petgraph wrapper, DOT export
  cli/            command handlers
  analysis/       focused call graph analysis
  main.rs         clap entry point
```

## Requirements

- Rust 2021 edition
- tree-sitter 0.25
- tree-sitter-c-sharp 0.23
- petgraph 0.6

## Examples on dotnet/eShop

The eShop reference application (494 C# files, 366 classes, 39 interfaces, 4791 call sites) is the main validation target.

### Pattern detection

Scan a project and get all detected patterns with confidence scores and evidence:

```
$ tiny-pdg-cs detect src/Catalog.API
Analysis of 39 file(s):
  Classes: 33, Interfaces: 2, Call sites: 706

ControllerApi (3 hits):
  CatalogAI (c=0.70)
    evidence: GetEmbeddingAsync, GetEmbeddingsAsync, ...
  CatalogApi (c=0.70)
    evidence: MapCatalogApi, GetAllItemsV1, ...
  CatalogIntegrationEventService (c=0.70)
    evidence: PublishThroughEventBusAsync, ...
Facade (2 hits):
  CatalogAI (c=0.30)
  CatalogIntegrationEventService (c=0.30)
```

```
$ tiny-pdg-cs detect src/Ordering.API
Analysis of 68 file(s):
  Classes: 79, Interfaces: 3, Call sites: 321

ControllerApi (4 hits):
  OrdersApi (c=0.70)
  OrderQueries (c=0.70)
  OrderingApiTrace (c=0.70)
  LinqSelectExtensions (c=0.70)
Singleton (1 hits):
  OrderDraftDTO (c=0.90)
TemplateMethod (1 hits):
  IdentifiedCommandHandler (c=0.75)
```

### Call graph with dispatch tracing

Show what a class calls, what calls it, and trace interface dispatch to show all possible implementations and their internal call graphs:

```
$ tiny-pdg-cs callgraph src/Catalog.API --class CatalogAI --trace

CALL GRAPH FOR: CatalogAI

-- Outbound calls (what CatalogAI calls) --
  GetEmbeddingAsync() calls:
    |-- CatalogItemToString (1x)
    |-- GenerateVectorAsync via _embeddingGenerator! (1x)
    |-- GetElapsedTime via Stopwatch (1x)
    |-- LogTrace via _logger (1x)
  GetEmbeddingsAsync() calls:
    |-- GenerateAsync via _embeddingGenerator! (1x)
    |-- Select via items (2x)
    |-- ToList via embeddings (1x)

-- Inbound calls (what calls CatalogAI) --
  GetEmbeddingAsync() <-
    |-- CatalogApi (3x)
    |-- CatalogAI (1x via direct)
  GetEmbeddingsAsync() <-
    |-- CatalogContextSeed (1x via catalogAI)
```

The `!` suffix on `_embeddingGenerator!` marks an interface dispatch point. With `--trace`, those are expanded to show all implementations, their constructors, and what each implementation calls internally.

### Control Flow Graph

```
$ tiny-pdg-cs cfg tests/fixtures/control_flow/if_else.cs
digraph CFG {
  n0 [label="[0] Entry (L2-L7)"];
  n1 [label="[1] Exit (L2-L7)"];
  n2 [label="[2] Condition (L3-L6)"];
  n4 [label="[4] Statement (L4-L4)"];
  n5 [label="[5] Statement (L6-L6)"];
  n2 -> n4 [label="CondTrue"];
  n2 -> n5 [label="CondFalse"];
  n0 -> n2 [label="Seq"];
}
```

### Program Dependence Graph

PDG extends CFG with control and data dependence edges:

```
$ tiny-pdg-cs pdg tests/fixtures/control_flow/if_else.cs --format dot
digraph PDG {
  n2 -> n4 [label="cfg(CondTrue)"];
  n2 -> n5 [label="cfg(CondFalse)"];
  n2 -> n4 [label="control"];
  n2 -> n5 [label="control"];
}
```

### Call resolution

Shows top called methods and their call sites across a project:

```
$ tiny-pdg-cs resolve src/Catalog.API
Resolution analysis for 39 file(s):
  Classes: 33, Interfaces: 2, Call sites: 706

Top called methods:
  HasColumnType (79 calls)
  Property<int> (48 calls)
  Property<string> (24 calls)
  IsRequired (24 calls)
  Entity (18 calls)
  ToTable (17 calls)
  MapGet (12 calls)
```

### Parse

Dump the type graph as JSON for a single file:

```
$ tiny-pdg-cs parse src/Catalog.API/Infrastructure/CatalogContext.cs
{
  "classes": {
    "CatalogContext": {
      "name": "CatalogContext",
      "base_class": null,
      "interfaces": [],
      "methods": [
        { "method": "CatalogContext", "signature": "void CatalogContext(DbContextOptions<CatalogContext>, IConfiguration)" },
        { "method": "OnModelCreating", "signature": "void OnModelCreating(ModelBuilder)" }
      ]
    }
  }
}
```

## Tests

```
cargo test        # 107 tests, unit + integration + fixtures
cargo bench       # pdg benchmarks
```

Fixture categories: control flow, exceptions, async, data flow, DI, factory, reflection, virtual dispatch, abstract classes, interfaces, hammock blocks, cross-file resolution, dynamic calls, pattern examples.

## Why

LLMs struggle with large codebases because you hit context limits fast. A PDG gives you structured traversal -- start at a failure point, follow data and control dependencies to the root cause, pull in only the relevant slices. The pattern detection and call resolution layers make the graph richer for .NET idioms like DI registration and interface dispatch.
