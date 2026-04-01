# Iris Product Roadmap

## Thesis

Iris should be built as a GPU-native visual platform, but marketed and shipped as a reliable product first.

The current raw Vulkan + DMA-BUF path is a strong Linux acceleration backend. It should not be treated as the entire product architecture.

The right product shape is:

- a portable core for image/document state, decode, caching, transforms, and effects
- multiple rendering backends
- an elite Linux fast path where zero-copy is available
- clear fallbacks everywhere else

## Product Positioning By Phase

### V1

Fast, reliable Linux image viewer.

Primary promise:

- opens images correctly
- navigates quickly
- handles large files well
- feels native on Linux
- uses zero-copy acceleration where available

Primary audience:

- Linux desktop users
- photographers
- power users
- graphics and systems people

### V2

Programmable GPU-native image viewer.

Primary promise:

- live WGSL effects
- portable rendering foundation
- measurable performance leadership
- image context exposed as tooling surface

Primary audience:

- creators
- shader authors
- graphics developers
- technically sophisticated users

### V3

Collaborative GPU-native creative platform.

Primary promise:

- non-destructive editing
- multiplayer workflows
- team product tiers
- real differentiation from legacy creative tooling

Primary audience:

- creative teams
- technical design workflows
- media-heavy product teams

## Brutally Honest Strategic Read

The current repo is a strong nucleus for a product, but not yet a platform.

What exists today:

- strong Linux/Vulkan experimentation
- credible zero-copy rendering path
- working image viewer shell
- early compute/effects direction

What does not exist yet:

- portable rendering abstraction
- release-grade compatibility matrix
- document/effect graph model
- team/collaboration primitives
- operational packaging and release discipline

If the team keeps treating the Vulkan DMA-BUF path as the app itself, Iris risks becoming a technically impressive niche artifact.

If the team turns that path into one backend inside a larger system, Iris can become a durable product.

## Phase Goals

### V1 Goals

- ship a Linux app that is reliable across real user environments
- keep zero-copy as a premium acceleration path, not a hard requirement
- make render-path selection explicit and observable
- stabilize cache, synchronization, resize, and presentation behavior
- publish docs, packaging, and benchmark methodology

### V1 Non-Goals

- marketplace
- plugin economy
- collaboration
- cross-platform shell
- complex non-destructive editing

### V2 Goals

- establish a render backend abstraction
- support macOS and Windows
- introduce a shader/effect layer with live preview
- expose image context through an MCP server interface
- prove competitive benchmark leadership

### V2 Non-Goals

- full collaborative documents
- enterprise/team billing maturity
- broad third-party commercial marketplace

### V3 Goals

- introduce document semantics
- add non-destructive operation graph
- support collaboration and shared state
- define pricing tiers and product packaging
- position Iris as a GPU-native creative platform
- submit a paper centered on the zero-copy architecture and systems findings

## Strategic Sequencing

The correct sequence is:

1. Ship V1 as a real Linux product.
2. Insert an architecture stabilization phase.
3. Build V2 on that refactored base.
4. Only then begin V3 platform work.

Skipping the architecture phase will turn V2 and V3 into a rewrite disguised as feature work.

## Core Decisions

The following should be treated as strategic decisions, not implementation details:

### Zero-Copy Policy

Zero-copy must be an opportunistic acceleration path.

It must not be the only way the product can be considered correct.

### Architecture Policy

The app needs a separation between:

- product core
- rendering backend
- platform shell
- effects/shader runtime

### Positioning Policy

The product promise should be stated in user terms:

- speed
- reliability
- instant navigation
- live effects

It should not rely on users caring about Vulkan or DMA-BUF.

## Success Conditions

V1 is successful if users can install Iris on Linux and use it reliably, even when the elite path is unavailable.

V2 is successful if Iris becomes programmable without losing product quality.

V3 is successful if Iris stops being perceived as a viewer and starts being perceived as a serious creative system.
