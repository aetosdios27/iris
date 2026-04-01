# V1 Release Criteria

## V1 Definition

V1 is a stable Linux application release.

That does not mean zero-copy must work on every machine.
It means the app must work correctly for Linux users in the presence or absence of the zero-copy fast path.

## Product Promise

The V1 promise should be:

- reliable Linux image viewing
- high performance on large images
- zero-copy GPU presentation where supported
- native GTK 4.14 integration
- thumbnail pipeline
- documented performance characteristics

## Hard Release Gates

### 1. Rendering Path Reliability

The application must support:

- preferred path: Vulkan + DMA-BUF + GTK offload
- fallback path: non-zero-copy GPU or memory-texture presentation
- software path: correctness-first escape hatch

The active path must be detectable and visible in logs at minimum.

### 2. DMA-BUF Negotiation

The compositor integration must be treated as a compatibility feature, not an assumption.

Release requires:

- correct format negotiation
- correct modifier handling
- explicit understanding of sync behavior
- tested behavior on real compositor/GPU combinations

### 3. Cache Stability

Release requires:

- byte-budgeted cache control
- descriptor-set-budgeted cache control
- bounded behavior for animated images
- no known exhaustion path from normal or adversarial usage

### 4. Resource Hygiene

Release requires:

- no known FD leaks in steady-state viewing
- no descriptor pool leaks
- no sync-fd accumulation
- no resize-triggered object leaks
- structured cleanup on upload and presentation failures

### 5. Resize and Presentation Correctness

Release requires:

- no known resize fence stall
- no blank-frame deadlock
- no use-after-free around render target replacement
- no silent corruption when the DMA-BUF handoff fails

### 6. Product Behavior

Release requires:

- open file
- open directory
- drag and drop
- thumbnail strip
- keyboard navigation
- rotation
- zoom and pan
- metadata panel
- saved window/config state

### 7. Packaging and Documentation

Release requires:

- installable release artifact
- supported environment documentation
- known limitations section
- performance methodology doc
- release notes

## Supported Environment Policy

If V1 is intended for "any Linux user", then supported-environment language must be honest.

Suggested support tiers:

- Tier 1: Wayland + GTK 4.14 + Vulkan + DMA-BUF path validated
- Tier 2: Linux systems where Vulkan works but zero-copy does not
- Tier 3: software fallback for correctness

This is much more defensible than pretending every Linux stack will get the same fast path.

## Benchmark Policy

Benchmarks must be defined before marketing claims.

Suggested benchmark categories:

- cold open latency
- warm navigation latency
- adjacent-image scrubbing latency
- memory footprint over directory traversal
- large image handling
- thumbnail population latency

Benchmarks should compare against:

- eog
- feh
- Gwenview
- Nomacs

Benchmarks must identify:

- machine spec
- compositor/session type
- GPU
- image corpus
- warm/cold cache state

## Test Matrix

Minimum release matrix:

- GNOME Wayland on Intel
- GNOME Wayland on AMD
- KDE Wayland on AMD or Intel
- at least one X11 or non-DMA-BUF path validation run

Stretch matrix:

- Nvidia on Wayland
- mixed iGPU/dGPU laptop
- very large RAW images
- animated GIF stress cases

## Known Risks To Eliminate Before Release

- modifier-0-only DMA-BUF assumptions
- explicit-sync machinery not actually connected to presentation
- descriptor exhaustion from many cached small textures
- unbounded animated-frame residency
- silent fallback from zero-copy to readback path without visibility

## V1 Exit Criteria

V1 is ready when:

1. the app is stable on a defined Linux matrix
2. the render path is observable and debuggable
3. the cache cannot trivially exhaust descriptor resources
4. zero-copy is proven where claimed
5. fallback behavior is correct where zero-copy is unavailable
6. docs and packaging are good enough for external users

If any of those are false, V1 is still a strong prototype, not a release.
