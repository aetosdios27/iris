# V1 Execution Plan

## Objective

Ship Iris as a Linux application that external users can install and use reliably.

V1 is not complete when the fast path works on one machine.
V1 is complete when the app is stable across a defined Linux support matrix, with correct fallback behavior where the preferred path is unavailable.

## Release Definition

V1 release means:

- installable Linux app
- clear supported environments
- correct image viewing behavior
- stable navigation and thumbnailing
- zero-copy acceleration where supported
- no known critical resource leaks or trivial crash paths

## Foundational Principle

The product promise is:

- fast
- reliable
- Linux-native

The product promise is not:

- raw Vulkan purity
- DMA-BUF everywhere
- zero-copy at any cost

## Workstreams

### Workstream 1: Render Path Hardening

Goal:

- make the Linux rendering stack robust enough for external users

Deliverables:

- explicit render path reporting
- real DMA-BUF capability handling
- modifier-aware negotiation
- documented fallback behavior

Exit criteria:

- app logs selected path at startup
- app can identify whether it is in zero-copy, GPU fallback, or software fallback mode
- zero-copy does not silently fail without traceability

### Workstream 2: Resource and Cache Safety

Goal:

- eliminate release-blocking resource exhaustion and leak risks

Deliverables:

- descriptor-aware cache budgeting
- animated-frame cache limits
- upload-path cleanup discipline
- FD lifecycle audit

Exit criteria:

- no descriptor exhaustion from realistic or adversarial directory browsing
- no known sync-fd accumulation
- no known DMA-BUF fd leak
- no unbounded animated-image cache growth

### Workstream 3: Presentation Correctness

Goal:

- guarantee that what is rendered is safely and correctly presented

Deliverables:

- validated synchronization model
- presentation-path assertions and logging
- no known resize presentation regressions

Exit criteria:

- resize does not deadlock or visibly corrupt output
- presentation sync model is understood and documented
- readback fallback is intentional and observable

### Workstream 4: Product Completeness

Goal:

- ensure the app experience is good enough for external use

Deliverables:

- file open
- directory open
- drag and drop
- thumbnails
- metadata panel
- keyboard navigation
- rotation
- zoom and pan
- persisted config

Exit criteria:

- all primary viewer workflows are verified manually
- no critical UX dead ends for normal usage

### Workstream 5: Packaging and Release Operations

Goal:

- make the app distributable and supportable

Deliverables:

- release build instructions
- package/distribution strategy
- support matrix doc
- known limitations doc
- benchmark methodology doc

Exit criteria:

- external user can install the app without reading the codebase
- release notes can be written honestly

## Milestones

### Milestone 1: Release Contract Lock

Purpose:

- stop scope drift and define what V1 actually promises

Tasks:

- finalize support tiers
- finalize zero-copy positioning language
- decide whether animated GIF support is in V1 core or best-effort
- define benchmark corpus and machine matrix

Done when:

- the team can describe V1 in one paragraph with no contradictions

### Milestone 2: Rendering Compatibility Pass

Purpose:

- make the render stack trustworthy across Linux environments

Tasks:

- audit DMA-BUF format and modifier negotiation
- audit compositor assumptions
- make render-path selection visible
- validate fallback presentation path

Done when:

- the app behaves predictably across the initial Linux matrix

### Milestone 3: Resource Safety Pass

Purpose:

- eliminate the most likely release-killer bugs

Tasks:

- descriptor pool budgeting
- cache admission rules
- animated frame residency cap
- cleanup on failed Vulkan uploads
- FD lifecycle verification

Done when:

- long-running browsing sessions do not degrade into exhaustion or instability

### Milestone 4: Product Workflow Pass

Purpose:

- polish the viewer into a real app

Tasks:

- verify user workflows end-to-end
- tighten error messages
- ensure software fallback remains correct
- validate config persistence and directory reload behavior

Done when:

- a normal user can browse, inspect, and navigate images without hitting sharp edges

### Milestone 5: Benchmarks and Docs

Purpose:

- make the public release credible

Tasks:

- run comparative benchmark suite
- write benchmark methodology
- write support matrix
- write release notes
- write known limitations

Done when:

- performance claims are evidence-backed

### Milestone 6: Packaging and Release Candidate

Purpose:

- produce the first public-quality artifact

Tasks:

- build distributable package
- smoke test install path
- run release checklist
- freeze scope

Done when:

- there is a candidate binary/package you would actually hand to users

## Explicit V1 De-Scopes

Do not let these enter V1 unless they are nearly free:

- shader marketplace
- plugin economy
- collaboration
- non-destructive editing
- team billing
- advanced shader authoring UI
- cross-platform backend work

These are V2+ efforts.

## Critical Risks

### Risk 1: Overfitting To One Linux Stack

Failure mode:

- the app works on the development machine but fails on real compositor/GPU combinations

Mitigation:

- test matrix
- capability reporting
- fallback path validation

### Risk 2: Shipping Silent Degradation

Failure mode:

- the app markets zero-copy while quietly falling back to slower presentation

Mitigation:

- visible render mode
- logs
- benchmark both fast path and fallback path

### Risk 3: Resource Exhaustion Under Real Browsing

Failure mode:

- descriptor exhaustion
- runaway cache growth
- animated image blowups

Mitigation:

- descriptor budgeting
- frame caps
- residency policy

### Risk 4: Research Brain Hijacks Product Brain

Failure mode:

- elegant systems work delays a shippable app indefinitely

Mitigation:

- release gates
- de-scope discipline
- separate product work from platform ambition

## Suggested Acceptance Checklist

Before tagging V1, all of the following should be true:

- app opens files and directories correctly
- thumbnail strip works on large directories
- no critical crash or stall during resize
- render path is visible and understandable
- descriptor exhaustion is no longer trivial to trigger
- fallback path is correct when zero-copy fails
- support matrix is documented
- benchmark methodology is written
- package/install story exists
- known limitations are public and honest

## Recommended Internal Order Of Attack

If execution starts now, the highest-leverage sequence is:

1. lock support policy and V1 definition
2. fix render-path negotiation and observability
3. fix descriptor/cache/resource accounting
4. validate product workflows
5. benchmark and package

That order minimizes the risk of polishing a build whose runtime contract is still unstable.

## Final CTO Call

If a task does not increase:

- Linux reliability
- render-path correctness
- cache/resource stability
- release readiness

then it is probably not V1 work.
