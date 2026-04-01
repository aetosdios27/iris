# Platform Architecture Transition

## Purpose

This document describes the architecture Iris needs in order to move from:

- a Linux-first high-performance viewer prototype

to:

- a cross-platform programmable image application
- and eventually a GPU-native creative platform

## Current Reality

The current architecture is optimized around this loop:

- decode image
- upload texture
- render with Vulkan
- export DMA-BUF
- present via GTK

That is a valid V1 foundation for a Linux product.
It is not sufficient as the long-term architecture for V2 and V3.

## Architecture Target

Iris should evolve into four layers:

### 1. Product Core

Owns:

- asset model
- file loading
- metadata
- thumbnail policy
- cache policy
- navigation state
- persisted config
- document/session state

Must not depend directly on GTK or Vulkan.

### 2. View and Document Model

Owns:

- current image or document
- zoom, pan, rotation
- selection state
- effect stack
- undoable operations
- eventually non-destructive graph state

This layer becomes the bridge from viewer to editor.

### 3. Render Runtime

Owns:

- render trait / backend interface
- texture upload semantics
- render target lifecycle
- shader execution contract
- presentation capability model

Backends should include:

- Linux Vulkan DMA-BUF backend
- Linux fallback backend
- future macOS backend
- future Windows backend

### 4. Platform Shell

Owns:

- GTK windowing
- input binding
- drag and drop
- menus and panels
- platform integration

The shell should call into the product core and render runtime, not own business logic.

## Required Refactors Before V2

### Renderer Abstraction

Define a rendering interface that isolates:

- image upload
- render
- resize
- presentation
- capability reporting

The current Vulkan renderer should implement this interface first.

### Capability Model

The app must be able to answer:

- is zero-copy available
- which pixel formats are usable
- which modifiers are usable
- whether compute passes are available
- what fallback path is active

Capabilities should be first-class state, not ad hoc checks.

### Effects Contract

WGSL effects need a stable runtime contract:

- image inputs
- uniforms/params
- color assumptions
- output format
- error handling

Without this, a shader editor and marketplace will be chaos.

### Cache Contract

The cache must be modeled as a product system, not just a renderer detail.

It needs budgets for:

- bytes
- descriptors
- animated frames
- residency priority
- prefetch admission

### Document Model

Before V3, the app needs a canonical document structure.

Suggested future model:

- asset reference
- transform stack
- effect stack
- view state
- annotations or future layers

That gives non-destructive editing somewhere to live.

## Anti-Patterns To Avoid

### 1. Backend Leakage

Do not let GTK or product logic depend on Vulkan-specific assumptions beyond the renderer boundary.

### 2. Fast Path Worship

Do not design every system around the best-case Linux rendering path.

### 3. Feature-Led Architecture

Do not add shader editor, marketplace, or collaboration features directly onto the current viewer control flow.

### 4. Silent Degradation

Do not allow important runtime mode changes to happen invisibly.

## Phase Plan

### V1.5

Refactor without changing product category.

Deliverables:

- renderer trait
- capability reporting
- explicit render-path selection
- cache policy cleanup
- cleaner separation of UI and render backend

### V2

Extend the architecture, do not rewrite it.

Deliverables:

- cross-platform backend work
- shader editor
- live effect preview
- MCP exposure of image context

### V3

Build on document semantics.

Deliverables:

- non-destructive editing graph
- collaboration model
- team-facing product layers

## CTO Summary

The current system is a high-potential kernel.

To become a platform, Iris needs:

- one product core
- one document/view model
- one render abstraction
- many backends

If that separation happens soon, the roadmap is credible.

If it does not, future work will be dominated by rewrites, backend coupling, and broken promises.
