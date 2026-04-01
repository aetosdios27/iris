# Code Quality Audit

## Purpose

This document captures a code quality audit of Iris against standards inspired by *Designing Data-Intensive Applications*.

DDIA is not a GUI or graphics book, so the fit is not literal. The standards adapted here are:

- explicit contracts at system boundaries
- clear separation of concerns
- bounded resource usage
- intentional concurrency and backpressure
- observable failure modes
- evolvable architecture

The goal is to evaluate whether Iris is only a strong prototype or whether it is moving toward a release-grade system.

## Executive Verdict

Iris is a strong systems-heavy prototype with real architectural promise.

It is not yet a DDIA-grade production system.

The strongest parts of the codebase are:

- serious technical ambition
- credible Vulkan/DMA-BUF path
- working viewer shell with real features
- awareness of performance and caching concerns

The weakest parts are:

- implicit assumptions at platform boundaries
- incomplete resource accounting
- weak observability
- ad hoc concurrency
- entangled responsibilities between product logic and rendering control

## Audit Dimensions

### 1. Boundary Contracts

#### DDIA Standard

Systems should make assumptions explicit at boundaries:

- data formats
- negotiation rules
- compatibility guarantees
- failure behavior

#### Iris Assessment

This is the weakest major area in the current code.

The Linux presentation path assumes:

- modifier `0` is the relevant DMA-BUF path
- `set_modifier(0)` is sufficient
- exported sync fd can be created and then ignored
- fallback to readback is acceptable even when the fast path silently fails

That makes the render contract too implicit.

Key files:

- [src/viewport/mod.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/mod.rs)
- [src/viewport/vk/dmabuf.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/vk/dmabuf.rs)
- [src/viewport/vk/renderer.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/vk/renderer.rs)

Verdict:

- below release-grade

### 2. Separation Of Concerns

#### DDIA Standard

The system should separate:

- state coordination
- background work
- persistence
- presentation
- platform-specific execution

#### Iris Assessment

The current code mixes layers too aggressively.

Examples:

- `main.rs` owns UI, navigation state, thumbnail orchestration, persistence wiring, and product-level coordination
- `viewport/mod.rs` owns decode policy, animation logic, rendering coordination, fallback handling, and GTK presentation bridging
- there is duplicate thumbnail logic in `main.rs` and `thumbcache.rs`

This is still manageable for V1, but it reduces clarity and makes future refactors more expensive.

Key files:

- [src/main.rs](/home/aetos/DevNexus/Code/Projects/iris/src/main.rs)
- [src/viewport/mod.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/mod.rs)
- [src/thumbcache.rs](/home/aetos/DevNexus/Code/Projects/iris/src/thumbcache.rs)

Verdict:

- medium quality
- acceptable for prototype scale
- should be cleaned before V2

### 3. Bounded Resource Usage

#### DDIA Standard

Systems should define and enforce resource limits intentionally.

This includes:

- memory
- descriptors
- file descriptors
- queues
- cached state

#### Iris Assessment

The cache budget only partially models the real system.

What is bounded:

- texture cache bytes

What is not yet adequately bounded:

- descriptor set count
- animated GIF frame residency
- background decode admission

That means the app still has realistic exhaustion paths even though it has partial cache budgeting.

Key files:

- [src/viewport/vk/renderer.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/vk/renderer.rs)
- [src/viewport/mod.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/mod.rs)

Verdict:

- weak to medium

### 4. Concurrency And Backpressure

#### DDIA Standard

Background work should be intentional:

- queues should be bounded
- duplicate work should be minimized
- cancellation should be meaningful
- work admission should be policy-driven

#### Iris Assessment

The current model uses many opportunistic `rayon::spawn` and local futures.

That keeps the UI responsive, which is good.

But it does not yet provide:

- central admission control
- in-flight deduplication
- explicit prioritization
- structured cancellation beyond target checks

This is fine for early development, but DDIA-style discipline would treat this as a future operational risk.

Key files:

- [src/main.rs](/home/aetos/DevNexus/Code/Projects/iris/src/main.rs)
- [src/viewport/mod.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/mod.rs)

Verdict:

- functional but ad hoc

### 5. Failure Semantics

#### DDIA Standard

Failures should be:

- explicit
- visible
- categorized
- recoverable where possible

#### Iris Assessment

Iris has an error type and some decent Vulkan error naming, which is a good start.

But the runtime behavior still often degrades to:

- log and continue
- fall back silently
- swallow persistence errors

That means operators and users have weak visibility into whether the app is actually running in its intended mode.

Key files:

- [src/error.rs](/home/aetos/DevNexus/Code/Projects/iris/src/error.rs)
- [src/config.rs](/home/aetos/DevNexus/Code/Projects/iris/src/config.rs)
- [src/viewport/mod.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/mod.rs)
- [src/viewport/vk/renderer.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/vk/renderer.rs)

Verdict:

- weak for release-grade expectations

### 6. Observability

#### DDIA Standard

A system should be able to answer:

- what mode am I in
- why did I choose this path
- what failed
- what resources am I consuming

#### Iris Assessment

Observability is currently mostly `println!` and `eprintln!`.

That is enough for local debugging, but not for release discipline.

The app should explicitly expose:

- selected render path
- zero-copy status
- fallback status
- cache pressure
- descriptor pressure
- important capability decisions

Verdict:

- weak

### 7. Evolvability

#### DDIA Standard

Good systems are easy to change because boundaries are intentional and local changes do not cascade unpredictably.

#### Iris Assessment

Iris is still evolvable, but the window is now.

Why it is still salvageable:

- the codebase is still compact
- module structure is understandable
- the core rendering path is conceptually coherent

Why it needs action soon:

- product logic and render orchestration are too coupled
- dead/duplicate paths are starting to appear
- future V2/V3 ambitions will strain the current shape badly

Verdict:

- medium today
- likely to degrade fast without V1.5 refactor work

## Existing Code: Edit Or Add Files?

## V1 Recommendation

For V1, the highest-value work is editing existing files, not creating many new ones.

Priority edit targets:

- [src/viewport/mod.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/mod.rs)
- [src/viewport/vk/renderer.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/vk/renderer.rs)
- [src/viewport/vk/dmabuf.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/vk/dmabuf.rs)
- [src/viewport/vk/context.rs](/home/aetos/DevNexus/Code/Projects/iris/src/viewport/vk/context.rs)
- [src/main.rs](/home/aetos/DevNexus/Code/Projects/iris/src/main.rs)
- [src/config.rs](/home/aetos/DevNexus/Code/Projects/iris/src/config.rs)
- [src/thumbcache.rs](/home/aetos/DevNexus/Code/Projects/iris/src/thumbcache.rs)

Recommended V1 actions:

- tighten DMA-BUF negotiation logic
- tighten presentation observability
- add descriptor-aware cache limits
- bound animated image residency
- unify or remove duplicate thumbnail code
- improve structured error surfacing

### V1.5 Recommendation

If the team enters architecture cleanup after V1, adding new files becomes worthwhile.

Likely additions:

- `src/render_backend.rs`
- `src/render_capabilities.rs`
- `src/cache_policy.rs`

Those files should exist only when the boundaries are ready to be real, not just aspirational.

## Ranked Technical Debt

### Tier 1: Must Fix For V1

- DMA-BUF modifier and presentation contract clarity
- descriptor-aware cache governance
- animated-frame resource limits
- render-path observability
- better failure semantics at the presentation boundary

### Tier 2: Should Fix For V1 Or V1.5

- background work dedupe and admission control
- duplicate thumbnail implementation cleanup
- clearer separation between UI shell and rendering orchestration

### Tier 3: V2 Preparation

- renderer abstraction
- capability model
- cache policy abstraction
- portable backend seams

## Final DDIA-Style Verdict

If judged against DDIA-inspired standards, Iris today is:

- strong on ambition
- strong on technical depth
- medium on structure
- weak on boundedness and observability
- weak on explicit contracts at the hardest system edge

That is a respectable place for an advanced prototype.

It is not yet where a sober production engineer would declare the system operationally mature.

The good news is that the problems are not signs of shallow thinking.
They are signs of a system that has reached the point where architecture discipline now matters more than adding new cleverness.
