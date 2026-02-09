# Technical Specification: Wakaru-rs

**Project Goal:** To rebuild the core "unminify" logic of Wakaru in Rust, leveraging the `swc_core` ecosystem for maximum performance, type safety, and maintainability.

**Target Output:** A library (crate) and CLI tool that accepts bundled/minified JavaScript and outputs clean, readable, modern ESNext code.

---

## 1. Technology Stack

* **Language:** Rust
* **Core Framework:** `swc_core` (The primary interaction layer with ASTs)
* **Parallelism:** `rayon` (For processing multiple files/modules concurrently)
* **Pattern Matching:** `swc_ecma_utils` + Custom matcher helpers
* **CLI Interface:** `clap` (If building a standalone binary)

### Essential Crates

```toml
[dependencies]
swc_core = { version = "0.x", features = [
  "ecma_parser",
  "ecma_ast",
  "ecma_visit",
  "ecma_transforms_base",
  "ecma_codegen",
  "ecma_utils",
  "common"
]}
anyhow = "1.0" # Error handling
rayon = "1.7"  # Parallel iterator

```

---

## 2. System Architecture

The system follows a linear **Pipeline Architecture**. Data flows through a series of "Passes" where the AST is progressively mutated from "Minified" to "Readable."

### The Pipeline

1. **Ingestion & Parsing:**
* Input: Raw Source String / File Path.
* Action: Parse into `swc_ecma_ast::Module`. Handle syntax errors gracefully.
* Outcome: A raw, untyped AST.


2. **Semantic Analysis (The "Resolver"):**
* Action: Run `swc_ecma_transforms_base::resolver`.
* Outcome: The AST is annotated with `SyntaxContext`. Every identifier `a` is now uniquely distinct (e.g., `a#0`, `a#1`) based on its scope. **We do not need to write our own scope analyzer.**


3. **Transformation Pipeline (The "Rules"):**
* Action: Apply a sequence of `VisitMut` implementations.
* **Group A: Syntax Cleaning** (e.g., `void 0` → `undefined`).
* **Group B: Structural Restoration** (e.g., Sequence expressions → Statements).
* **Group C: De-Transpilation** (e.g., Babel helpers → Modern Syntax).


4. **Hygiene & Renaming:**
* Action: Run `swc_ecma_transforms_base::hygiene` and `swc_ecma_transforms_base::fixer`.
* Outcome: `SyntaxContext` marks are removed. Variables are renamed to avoid collisions (`a#0` → `a`, `a#1` → `a1`). Parentheses are inserted where operator precedence requires them.


5. **Codegen (Formatting):**
* Action: Convert AST back to string.
* Config: `minify: false`.
* Outcome: Readable JavaScript code.



---

## 3. Core Component Design

### 3.1 The Rule Interface

Every transformation rule implements the standard `VisitMut` trait. This ensures composability.

```rust
// core/rule.rs
use swc_core::ecma::visit::VisitMut;

pub trait Rule: VisitMut {
    fn name(&self) -> &'static str;
}

```

### 3.2 The Matcher Utility

Wakaru logic relies heavily on checking if a node matches a specific structure (e.g., "Is this a call to `_interopRequireDefault`?"). We will build a helper trait for this.

```rust
// utils/matcher.rs
impl Matcher {
    /// Checks if a generic CallExpr matches a specific helper name
    pub fn is_helper(expr: &CallExpr, name: &str) -> bool {
        // Logic to check callee name
        ...
    }

    /// Checks for "void 0" pattern
    pub fn is_void_zero(expr: &UnaryExpr) -> bool {
        matches!(expr.op, UnaryOp::Void) && is_literal_zero(&expr.arg)
    }
}

```

### 3.3 The Driver (Orchestrator)

This function stitches everything together.

```rust
pub fn decompile(source: String) -> Result<String> {
    let cm = Lrc::new(SourceMap::default());
    // 1. Parse
    let mut program = parse_js(&source, &cm)?;

    // 2. Resolve (Scope Analysis)
    program.visit_mut_with(&mut resolver(Mark::new(), Mark::new(), false));

    // 3. Apply Rules (Chained)
    program.visit_mut_with(&mut chain!(
        // Pass 1: Syntax Cleanup
        rules::RemoveVoid,
        rules::UnminifyBooleans,

        // Pass 2: Heavy Lifting
        rules::RestoreAsyncAwait, // Complex recursive rule

        // Pass 3: Final cleanup
        rules::DeadCodeElimination
    ));

    // 4. Hygiene (Renaming) & Fixer (Parens)
    program.visit_mut_with(&mut hygiene());
    program.visit_mut_with(&mut fixer(None));

    // 5. Codegen
    print_js(&program, &cm)
}

```

---

## 4. Implementation Roadmap

### Phase 1: The Foundation (Week 1)

* Set up the Rust project structure.
* Implement the `Driver` with a "No-Op" rule (parse -> print).
* Verify that parsing supports modern syntax (TS, JSX, etc.) by enabling correct parser options.

### Phase 2: The Basics (Week 2)

Implement "syntactic sugar" rules. These are stateless and easy to test.

* **`FlipComparisons`**: `null == x` → `x == null`.
* **`RemoveVoid`**: `void 0` → `undefined`.
* **`UnminifyBooleans`**: `!0` → `true`, `!1` → `false`.
* **`SimplifySequence`**: `(a(), b())` → `a(); b();`.

### Phase 3: The "Wakaru" Magic (Weeks 3-4)

Implement the complex logic that reverses Babel/Webpack artifacts.

* **`RestoreImports`**: Convert `require('react')` → `import React from 'react'`.
* **`RestoreAsyncAwait`**: Detect `_asyncToGenerator` wrappers and rewrite the inner generator function into an async function.
* *Strategy:* Match the wrapper → Extract the inner function → Transform `yield` to `await`.


* **`RemoveWebpackBoilerplate`**: Strip `__webpack_require__` calls.

### Phase 4: Control Flow (Advanced)

* **`Unswitch`**: Detect "switch-based control flow flattening" (common in obfuscated code) and restore generic `if/else` or loops.
* *Note:* This requires a Control Flow Graph (CFG) analysis, which is harder in SWC than simple AST traversal.



---

## 5. Key Advantages of This Spec

1. **Zero "Scope" Management:** By delegating scope analysis to `swc_core::resolver`, we eliminate an entire class of bugs related to variable shadowing and renaming.
2. **Performance:** Since SWC is written in Rust and optimized for compilation, this unminifier will likely be 10-50x faster than the JS version.
3. **Extensibility:** Adding a new rule is as simple as adding a new struct that implements `VisitMut`.
4. **Parallelism Ready:** The `Driver` can easily be wrapped in `rayon::par_iter` to decompile all files in a folder simultaneously.

---

Useful Reference:
https://github.com/swc-project/swc/blob/main/ARCHITECTURE.md

https://rustdoc.swc.rs/swc/

---

We will focus on test-driven to make sure the tool is stable and reduce regression. You should reuse wakaru's existing test case and modify them when the mismatch is simply caused by indent or codegen/printing (if it's reasonable).

rs rewrite will be put under /wakaru-rs
