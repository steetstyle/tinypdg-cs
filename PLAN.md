# tiny-pdg-cs — Plan

C# (ve sonra TypeScript, Python, Go, Java) için Program Dependence Graph (PDG) builder,
GoF 22 tasarım kalıbı detektörü, DI/Factory/Reflection call resolution,
hammock-block analizi.

## Hedef

LLM-driven Root Cause Analysis için C# mikroservislerinin
kod bağımlılık grafiğini (PDG) inşa etmek. PDG üzerinde:
- Servis seviyesi: SDG (Service Dependency Graph)
- Kod seviyesi: PDG (Program Dependence Graph) + Hammock bloklar
- LLM traversal: Structured graph traversal ile kök neden analizi

## Mimari

```
tiny-pdg-cs/
├── Cargo.toml
├── PLAN.md
├── src/
│   ├── main.rs              # CLI (clap) — 6 komut
│   ├── lib.rs               # Modül ihracatı
│   ├── parse/               # tree-sitter → AST
│   │   ├── parser.rs        # tree-sitter C# grammar wrapper
│   │   └── visitor.rs       # AST traversal, StmtKind, method bulma
│   ├── cfg/                 # AST → CFG
│   │   ├── builder.rs       # Basic block + edge inşası
│   │   └── region.rs        # Structured region detection
│   ├── pdg/                 # CFG → PDG
│   │   ├── control_deps.rs  # Post-dominator → control dependence edges
│   │   ├── data_deps.rs     # Reaching definitions → data dependence edges
│   │   └── pdg_builder.rs   # CFG + deps → PDG
│   ├── hammock/             # PDG → Hammock bloklar
│   │   └── builder.rs       # Hammock identification (Johnson '94)
│   ├── resolve/             # Call resolution
│   │   ├── types.rs         # CallTarget, Resolution, Confidence
│   │   ├── direct.rs        # Kategori 1: Static/Non-virtual/Private
│   │   ├── virtual_table.rs # Kategori 2: CHA / RTA / VTA
│   │   ├── abstract_resolve.rs  # Kategori 3: Abstract class
│   │   ├── interface_resolve.rs # Kategori 4: Interface dispatch
│   │   ├── di.rs            # Kategori 5: DI container
│   │   ├── factory.rs       # Kategori 5b: Factory conditional
│   │   ├── reflection.rs    # Kategori 6: Reflection
│   │   ├── dynamic.rs       # Kategori 6b: C# dynamic / DLR
│   │   └── symbols.rs       # Sembol tablosu
│   ├── detect/              # GoF 22 pattern detection
│   │   ├── creational.rs    # Singleton, Factory, Builder, Prototype
│   │   ├── structural.rs    # Adapter, Decorator, Proxy, Composite, Facade, Bridge, Flyweight
│   │   ├── behavioral.rs    # Strategy, Observer, Command, Mediator, Chain, State, Template, Visitor, Iterator, Memento, Interpreter
│   │   └── types.rs         # PatternKind, PatternMatch
│   ├── graph/               # Graph utilities
│   │   ├── store.rs         # In-memory graph storage + query
│   │   └── dot.rs           # DOT export
│   └── cli/                 # CLI commands
│       └── commands.rs
├── tests/
│   ├── fixtures/            # C# test samples
│   │   ├── control_flow/
│   │   ├── exceptions/
│   │   ├── async/
│   │   ├── data_flow/
│   │   ├── di/
│   │   ├── factory/
│   │   ├── reflection/
│   │   ├── virtual/
│   │   ├── abstract_class/
│   │   ├── interface_resolve/
│   │   ├── hammock/
│   │   ├── cross_file/
│   │   ├── dynamic/
│   │   └── projects/
│   ├── integration/
│   └── expected/
├── benches/
│   └── bench_pdg.rs
└── .gitignore

## Call Resolution Sınıflandırması

### Kategori 1 — Doğrudan Çağrı (Direct/Static/Non-Virtual) [Confidence: %100]
- Static method: `Foo.Bar()`
- Non-virtual instance: `obj.NormalMethod()`
- Private/sealed: override kapalı

### Kategori 2 — Virtual & Override [Confidence: %50-%100]
- CHA: Class Hierarchy Analysis — tip ağacı taranır
- RTA: Rapid Type Analysis — `new` ifadeleri izlenir
- VTA: Variable Type Analysis — data flow ile tip izleme

### Kategori 3 — Abstract Class [Confidence: %60-%90]
- Abstract method → concrete implementations
- Type hierarchy'de `new`lenebilir sınıflar filtresi

### Kategori 4 — Interface Dispatch [Confidence: %40-%95]
- Single impl → devirtualization [%95]
- Multi impl → CHA/VTA kümesi [%40-%70]
- Explicit interface impl [%90]
- Default Interface Methods (DIM) [%85]

### Kategori 5 — Dependency Injection [Confidence: %30-%95]
- Static registration: `AddScoped<TIf, TImpl>()` [%95]
- Conditional/factory: lambda CFG branching [%50]
- Keyed/named: `GetKeyedService<IFoo>("key")` [%80-%90]
- Assembly scanning: `RegisterAssemblyTypes()` [%30-%60]

### Kategori 6 — Reflection & Dynamic [Confidence: %0-%80]
- Static string/nameof: `typeof(Foo).GetMethod("Bar")` [%70-%80]
- Dynamic string: `GetMethod(variableName)` [<%10]
- C# `dynamic` keyword: [%0 static, %100 runtime]

## GoF 22 Pattern Detection

C# (özellikle .NET 8/9+), OOP ve fonksiyonel programlama öğelerini harmanladığı için pattern altyapısına en ideal dildir. Modern C# özellikleri (Pattern Matching, Records, Default Interface Methods, Primary Constructors) pattern implementasyonlarındaki boilerplate kodunu dramatik şekilde azaltır.

Her pattern PDG call resolution'a girdi sağlar. Aşağıda her pattern'in PDG katkısı, confidence seviyesi ve modern C# idiom karşılığı verilmiştir:

| Pattern | PDG Katkısı | Confidence | Modern C# Idiom |
|---------|-------------|------------|-----------------|
| **Singleton** | `Instance` → tek nesne | %95 | `Lazy<T>`, `AddSingleton()` |
| **Factory Method** | `Create()` → concrete type | %80 | `Func<T>`, generic factory delegates |
| **Abstract Factory** | Factory interface → product family | %70 | `IServiceProvider`, DI modules |
| **Builder** | Method chaining → data flow | %75 | Fluent API, `init`-only properties |
| **Prototype** | `Clone()` → copy semantics | %85 | `record` + `with` expression |
| **Adapter** | Interface wrapping → call forwarding | %80 | Extension methods, wrapper classes |
| **Bridge** | Abstration/Impl ayrımı → 2D call | %70 | Interface segregation, event/delegate |
| **Composite** | `Add()` + tree → recursive call | %75 | `IEnumerable<T>` + LINQ |
| **Decorator** | DispatchProxy → wrapping chain | %90 | `DispatchProxy`, AOP interceptors |
| **Facade** | Delegasyon → call clustering | %80 | Aggregated service facades |
| **Flyweight** | Shared instance → data flow merge | %70 | `readonly struct`, `ConcurrentDictionary` cache |
| **Proxy** | Lazy/virtual → indirection | %85 | `Lazy<T>`, `DispatchProxy` |
| **Chain of Resp.** | Pipeline → sequential delegation | %65 | `Func<Context, Task>` middleware pipeline |
| **Command** | `ICommand.Execute()` → dispatch | %40 | `IRequest<TResponse>`, MediatR |
| **Iterator** | `yield` → stateful CFG | %80 | `yield return`, `IAsyncEnumerable<T>` |
| **Mediator** | `IMediator.Send()` → routing | %45 | MediatR, Event Aggregator |
| **Memento** | State snapshot → data flow | %70 | `record` snapshot, JSON serialization |
| **Observer** | `event`/`IObservable` → callback | %70 | `event`, `IObservable<T>`, `Action` delegates |
| **State** | `state.Handle()` → machine | %60 | `switch` pattern matching, state machine |
| **Strategy** | `IFoo.Do()` → CHA + DI | %95 | `Func<TIn, TOut>`, DI injection |
| **Template Method** | abstract class + virtual | %80 | `abstract class`, default interface methods |
| **Visitor** | `Accept(v)` → double dispatch | %85 | `switch` type pattern matching |

## Multi-Language Core

```
                    ┌─────────────────────┐
                    │   Core IR (petgraph) │
                    │  CFG / PDG / Hammock │
                    └──────────┬──────────┘
                               │
          ┌────────────────────┼────────────────────┐
          ▼                    ▼                    ▼
    ┌──────────┐        ┌──────────┐        ┌──────────┐
    │ C#       │        │TS/Python │        │ Go/Java  │
    │ parse/   │        │ parse/   │        │ parse/   │
    │ cfg/     │        │ cfg/     │        │ cfg/     │
    │ pdg/     │        │ pdg/     │        │ pdg/     │
    │ detect/  │        │ detect/  │        │ detect/  │
    │ resolve/ │        │ resolve/ │        │ resolve/ │
    └──────────┘        └──────────┘        └──────────┘
```

## Language-Agnostic Pattern Architecture

Pattern tanımlarını dilden bağımsız modelleyip her dilin idiom'una uygun kod üreten altyapı:

```
[ 1. Pattern AST / Definition ] (JSON / YAML / Schema)
              │
              ├── Core Pattern Metadata (Adı, Tipi, UML, Problem, Çözüm)
              └── Code Intent Matrix (Create, Wrap, Dispatch, Notify)
              │
              ▼
[ 2. Multi-Language Code Generator / Engine ]
              │
              ├── C# Renderer       (Records, Interfaces, LINQ, DI)
              ├── TypeScript Renderer (Interfaces, Types, Generics)
              ├── Python Renderer    (Dataclasses, Protocols, ABC)
              ├── Go Renderer        (Structs, Interfaces, Composition)
              └── Java Renderer      (Classes, Interfaces, Generics)
```

### Diller Arası Dönüşüm (Idiomatic Translation)

Her dil kendi kültürüne uygun kod üretir:

| Dil | Yaklaşım | Polimorfizm | Bellek |
|-----|----------|-------------|--------|
| **C#** | Güçlü OOP | Interface, Generics | GC |
| **TypeScript** | Structural typing | Duck typing | JS runtime |
| **Go** | Composition | Interface satisfaction | Value/Ptr |
| **Python** | Duck typing | Protocol, ABC | GC ref count |
| **Java** | Güçlü OOP | Interface, Generics | GC |

## Aşamalar

### Faz 0: Proje iskeleti + PLAN.md (bugün)

### Faz 1a: Parse + CFG (C#) — ~1 hafta
- tree-sitter ile C# AST
- Basic CFG: if/else, for, foreach, while, switch
- Exception handling: try/catch/finally
- async/await (resume points)
- yield return/break

### Faz 1b: CFG test fixtures + golden tests (paralel)

### Faz 2a: PDG — ~1-2 hafta
- Control dependence (post-dominator tree)
- Data dependence (reaching definitions, def-use chains)
- PDG builder

### Faz 2b: Call resolution: direct + CHA + RTA (paralel)

### Faz 3a: Call resolution: DI + Factory + Reflection — ~1-2 hafta
- DI registration scanner
- Factory lambda CFG branching
- Reflection string propagation
- Conflict resolution (birden çok analiz sonucu birleştirme)

### Faz 3b: Hammock block identification (paralel)

### Faz 4: GoF 22 Pattern Detection — ~2-3 hafta

Strateji: Her pattern önce klasik (textbook) tanımıyla implement edilir, ardından modern C# idiom'ları ile güncellenir.

**Creational (4)**
- Singleton: `Lazy<T>` + IoC `AddSingleton()` entegrasyonu
- Factory Method: Reflection içermeyen, statik tipli `Func<T>` fabrika delegeleri
- Abstract Factory: `IServiceProvider` uyumlu fabrika arayüzleri
- Builder: Fluent API + `init`-only properties
- Prototype: `record` + `with` expression (derin kopya desteği)

**Structural (7)**
- Adapter: Interface bazlı wrapper'lar + extension method'lar
- Bridge: Event/delegate ile ayrıştırılmış Abstraction/Implementation
- Composite: `IEnumerable<T>` + LINQ uyumlu ağaç yapıları
- Decorator: `DispatchProxy` veya IoC decorator ile şeffaf sarmalama
- Facade: Kompleks alt sistemleri tek service altında toplama
- Flyweight: `readonly struct` + `ConcurrentDictionary` önbellek
- Proxy: `DispatchProxy`, AOP interceptor'ları, `Lazy<T>` sanal proxy

**Behavioral (11)**
- Chain of Resp: `Func<Context, Task>` middleware pipeline
- Command: `IRequest<TResponse>` / MediatR CQRS yapıları
- Iterator: `yield return` + `IAsyncEnumerable<T>` asenkron akış
- Mediator: MediatR / Event Aggregator in-process mesaj otobüsü
- Memento: `record` state snapshot + JSON serialization
- Observer: `IObservable<T>` / `event` / `Action` delegate ağları
- State: C# `switch` pattern matching ile sadeleştirilmiş state machine
- Strategy: `Func<TInput, TOutput>` veya interface injection
- Template Method: `abstract class` veya default interface methods
- Visitor: `switch` type pattern matching (double-dispatch alternatifi)
- Interpreter: Expression tree / `System.Linq.Expressions`

- Pattern → call resolution entegrasyonu (confidence boosting)

### Faz 5: Benchmark + optimizasyon — ~1 hafta

### Faz 6: Multi-language adapters — dil başına ~1 hafta
- C# ✓ (ilk): OOP, sınıflar, interface'ler, generics
- TypeScript: Type aliases, structural typing, functional modüller
- Python: Duck typing, `@dataclass`, `Protocol` (PEP 544), decorator'lar
- Go: Composition, interface embedding (sınıf/kalıtım yok)
- Java: Sınıflar, interface'ler, generics (C# ile yakın paralel)

## CLI Komutları

```bash
tiny-pdg-cs parse file.cs              # AST JSON
tiny-pdg-cs cfg file.cs                # CFG DOT+JSON
tiny-pdg-cs pdg file.cs                # PDG DOT+JSON
tiny-pdg-cs hammock file.cs            # Hammock DOT+JSON
tiny-pdg-cs resolve path               # Call resolution report
tiny-pdg-cs detect path                # Pattern detection report
```

## Test & Benchmark

### Test Fixture Kategorileri

| Kategori | Fixture | Test |
|----------|---------|------|
| control_flow | if_else.cs, switch.cs, loops.cs, ternary.cs | CFG branch edges |
| exceptions | try_catch.cs, try_finally.cs, using.cs | Exception CFG |
| async | basic_async.cs, await_chain.cs, yield.cs | Async resume points |
| data_flow | simple_def_use.cs, reaching_defs.cs, ref_out.cs | Data deps |
| di | add_scoped.cs, constructor_injection.cs, decorator.cs | DI resolution |
| factory | lambda_factory.cs, conditional_factory.cs | Factory CFG |
| reflection | typeof_getmethod.cs, activator.cs, nameof.cs | Reflection resolve |
| virtual | virtual_override.cs, rta_analysis.cs | CHA/RTA |
| abstract_class | abstract_class.cs | Abstract resolution |
| interface_resolve | multi_impl.cs, explicit_impl.cs, dim.cs | Interface dispatch |
| hammock | structured.cs, unstructured.cs, multiple_returns.cs | Hammock blocks |
| cross_file | IFoo.cs, FooImpl.cs, Bar.cs, Startup.cs | Cross-file calls |
| dynamic | dynamic_call.cs | C# dynamic |
| patterns | strategy.cs, observer.cs, factory_pattern.cs | GoF detection |

### Benchmarklar

```bash
cargo bench
```

| Benchmark | Veri | Beklenen |
|-----------|------|----------|
| parse_small | 10 satır | < 1ms |
| parse_large | 5000 satır | < 50ms |
| cfg_small | 10 stmt | < 0.5ms |
| cfg_large | 500 stmt | < 10ms |
| pdg_small | 10 stmt | < 2ms |
| pdg_large | 500 stmt + data flow | < 100ms |
| hammock_small | 5 nested | < 2ms |
| hammock_large | 50 nested + goto | < 50ms |
| di_resolve | 100 registration | < 50ms |
| reflection_resolve | 10 typeof+GetMethod | < 10ms |
| pattern_detect | 1000 lines | < 100ms |


## Bağımlılıklar

```toml
tree-sitter = "0.24"
tree-sitter-c-sharp = "0.23"
petgraph = "0.6"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
anyhow = "1"
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
insta = "1"
pretty_assertions = "1"
```