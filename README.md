# Iris

Iris is a Linux image viewer built in Rust with GTK4/libadwaita and a GPU-native rendering stack.

The current direction is simple:

- ship a fast, reliable Linux image viewer first
- use zero-copy GPU presentation where the platform supports it
- keep the architecture strong enough to grow into a programmable image tool later

## Current Status

Iris is an active product prototype moving toward a V1 Linux release.

Today the codebase already has:

- GTK4/libadwaita desktop UI
- Vulkan-based renderer
- DMA-BUF presentation path for supported Linux environments
- software fallback when Vulkan is unavailable
- directory browsing and thumbnail strip
- RAW image support
- ICC-aware color conversion
- zoom, pan, rotation, metadata panel
- drag and drop for files and directories
- directory watching
- early compute-based image processing toggles

What it is not yet:

- a polished cross-platform app
- a guaranteed zero-copy path on every Linux setup
- a finished V1 release

## Product Direction

The product roadmap is:

- V1: stable Linux image viewer
- V2: cross-platform support and live shader layer
- V3: GPU-native creative platform

More detail lives in:

- [Product Roadmap](/home/aetos/DevNexus/Code/Projects/iris/docs/PRODUCT_ROADMAP.md)
- [V1 Release Criteria](/home/aetos/DevNexus/Code/Projects/iris/docs/V1_RELEASE_CRITERIA.md)
- [V1 Execution Plan](/home/aetos/DevNexus/Code/Projects/iris/docs/V1_EXECUTION_PLAN.md)
- [Platform Architecture Transition](/home/aetos/DevNexus/Code/Projects/iris/docs/PLATFORM_ARCHITECTURE_TRANSITION.md)

## Rendering Model

Iris is built around a GPU-native rendering approach on Linux.

Preferred path:

- decode image
- upload to GPU
- render with Vulkan
- export a DMA-BUF
- present through GTK/Wayland

Fallback path:

- if the preferred path is unavailable or rejected by the environment, Iris falls back to a slower but correct presentation path

That distinction matters. The goal is not "Vulkan at any cost." The goal is a reliable app with an elite fast path where available.

## Linux Support Model

Iris should be thought of as a Linux app with tiered rendering support:

- preferred: Wayland + Vulkan + DMA-BUF path active
- fallback: GPU rendering without zero-copy presentation
- fallback: software rendering for correctness

See the full support framing here:

- [Support Matrix](/home/aetos/DevNexus/Code/Projects/iris/docs/SUPPORT_MATRIX.md)

## Features In Tree

### Viewer

- open files and directories
- drag and drop files or folders
- keyboard navigation
- zoom in/out
- drag pan
- per-image rotation
- metadata/info panel
- persisted window state

### Image Handling

- common formats through the `image` crate
- RAW camera formats through `imagepipe`/`rawloader`
- ICC-aware conversion to sRGB
- animated GIF support

### Performance

- Vulkan renderer
- texture caching
- directional prefetching
- persistent thumbnail cache
- async image decode and metadata work

### Processing

- compute-pass toggles for enhance, sharpen, and denoise
- WGSL shader pipeline compiled through `naga`

## Tech Stack

- Rust
- GTK4
- libadwaita
- Vulkan via `ash`
- WGSL shaders via `naga`
- image decoding via `image`
- RAW decode via `imagepipe` and `rawloader`
- color transforms via `lcms2`

## Repo Layout

```text
iris/
├── src/
│   ├── main.rs                  # app shell, UI, navigation, thumbnails
│   ├── color.rs                 # ICC/profile handling
│   ├── config.rs                # persisted config
│   ├── raw.rs                   # RAW detection and decode helpers
│   ├── thumbcache.rs            # thumbnail cache helpers
│   └── viewport/
│       ├── mod.rs               # viewport, decode flow, presentation bridge
│       ├── camera.rs            # pan/zoom/rotation math
│       ├── shaders/             # WGSL shaders
│       └── vk/                  # Vulkan renderer internals
├── docs/                        # roadmap, release, architecture docs
├── tests/
└── ARCHITECTURE.md
```

## Build

### Prerequisites

You need:

- Rust and Cargo
- Vulkan loader and development libraries
- GTK4 development libraries
- libadwaita development libraries

Exact package names vary by distro.

### Build

```bash
cargo build
```

### Run

```bash
cargo run -- /path/to/image.jpg
```

You can also pass a directory path.

## Development Notes

The repo includes a broader strategic doc set than the current code alone would suggest. That is intentional. The code is still V1-oriented, but the architecture is being evaluated against later platform ambitions.

Useful docs:

- [Release Checklist](/home/aetos/DevNexus/Code/Projects/iris/docs/RELEASE_CHECKLIST.md)
- [Benchmark Plan](/home/aetos/DevNexus/Code/Projects/iris/docs/BENCHMARK_PLAN.md)
- [Support Matrix](/home/aetos/DevNexus/Code/Projects/iris/docs/SUPPORT_MATRIX.md)

## Benchmarks

Benchmark claims should be treated as pending until they are backed by the benchmark plan and measured against:

- eog
- feh
- Gwenview
- Nomacs

The benchmark methodology is tracked here:

- [Benchmark Plan](/home/aetos/DevNexus/Code/Projects/iris/docs/BENCHMARK_PLAN.md)

## Known Reality

Iris is promising, but still under active hardening.

Current areas that still matter for V1:

- DMA-BUF compatibility across real Linux stacks
- render-path observability
- descriptor/cache safety
- release packaging
- benchmark validation

## Contributing

If you want to contribute, start by reading:

- [V1 Release Criteria](/home/aetos/DevNexus/Code/Projects/iris/docs/V1_RELEASE_CRITERIA.md)
- [V1 Execution Plan](/home/aetos/DevNexus/Code/Projects/iris/docs/V1_EXECUTION_PLAN.md)
- [Platform Architecture Transition](/home/aetos/DevNexus/Code/Projects/iris/docs/PLATFORM_ARCHITECTURE_TRANSITION.md)

Contributions that improve Linux reliability, rendering correctness, cache safety, or release readiness are the highest leverage for V1.
