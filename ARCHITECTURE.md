# Iris — Architecture & Project History

> A high-performance, zero-copy Wayland image viewer built with Rust, GTK4, and raw Vulkan.

---

## Table of Contents

1. [Vision](#vision)
2. [Technology Choices](#technology-choices)
3. [Project Evolution](#project-evolution)
   - [Phase 1 — wgpu + GLArea](#phase-1--wgpu--glarea)
   - [Phase 2 — wgpu-hal HAL Hacks](#phase-2--wgpu-hal-hal-hacks)
   - [Phase 3 — Raw Vulkan + DMA-BUF](#phase-3--raw-vulkan--dma-buf)
4. [Current Architecture](#current-architecture)
   - [Directory Structure](#directory-structure)
   - [Component Map](#component-map)
   - [Data Flow](#data-flow)
5. [Component Deep-Dives](#component-deep-dives)
   - [main.rs — AppState & UI](#mainrs--appstate--ui)
   - [camera.rs — Camera & Transform](#cameraers--camera--transform)
   - [shader — image.wgsl](#shader--imagewgsl)
   - [vk/context.rs — VkContext](#vkcontextrs--vkcontext)
   - [vk/shader.rs — WGSL → SPIR-V](#vkshaderrs--wgsl--spir-v)
   - [vk/pipeline.rs — VkPipeline](#vkpipeliners--vkpipeline)
   - [vk/dmabuf.rs — DmabufImage](#vkdmabufrs--dmabufimage)
   - [vk/renderer.rs — VkRenderer](#vkrendererrs--vkrenderer)
   - [viewport/mod.rs — Viewport](#viewportmodrs--viewport)
6. [The DMA-BUF Zero-Copy Pipeline](#the-dma-buf-zero-copy-pipeline)
7. [Dependency Rationale](#dependency-rationale)
8. [Known Limitations & Open Issues](#known-limitations--open-issues)
9. [What Remains to Be Built](#what-remains-to-be-built)
10. [Future Vision](#future-vision)

---

## Vision

Iris aims to be the fastest image viewer on Linux. Not fast in a "loads quickly" sense — fast in a **zero-copy GPU pipeline** sense: an image decoded on the CPU is uploaded to the GPU exactly once, rendered by a custom Vulkan pipeline, and the result is handed to the Wayland compositor as a DMA-BUF so it can be scanned out directly to the monitor without a single extra copy or compositor blit.

The secondary goals are:

- A clean, native GTK4/libadwaita UI that feels at home on GNOME
- LRU-based GPU texture cache so adjacent images are instant to navigate
- Correct aspect-ratio-aware scaling, rotation, zoom, and pan entirely on the GPU
- Prefetching of adjacent directory images in the background

---

## Technology Choices

| Layer | Choice | Why |
|---|---|---|
| UI | GTK4 + libadwaita | Native GNOME look, `GraphicsOffload`, `DmabufTextureBuilder` |
| GPU API | `ash` (raw Vulkan) | Full control over memory, image layouts, DMA-BUF export |
| Shader language | WGSL | Ergonomic to write; compiled to SPIR-V at startup via `naga` |
| Shader compiler | `naga` | Pure-Rust WGSL→SPIR-V pipeline, no external tools needed |
| Image decoding | `image` crate | Broad format support (JPEG, PNG, WebP, AVIF, TIFF, BMP, GIF) |
| Async decode | `rayon` + `futures` | Decoding is CPU-bound; rayon threads + oneshot channels keep the GTK main loop free |
| Math | `glam` | SIMD-optimized Vec2/Mat2 for camera transforms |
| Zero-copy display | `GdkDmabufTextureBuilder` (GTK 4.14+) | Hands the Wayland compositor a Linux DMA-BUF fd for direct scanout |

---

## Project Evolution

### Phase 1 — wgpu + GLArea

The original approach used `wgpu` (a safe, cross-platform GPU abstraction) rendering into a GTK `GLArea` widget. The idea was to query GTK's current OpenGL framebuffer object at render time, wrap it in a `wgpu_hal` texture, and render directly into it.

**What was built:**
- Full wgpu render pipeline with a WGSL shader
- Uniform buffer for scale/rotation/zoom/pan
- LRU image texture cache with memory budgeting
- Camera struct with rotation-aware `fit_scale`
- All interactions: scroll zoom, drag pan, double-click reset, keyboard navigation

**Why it failed:**
- `wgpu_hal::api::Gles` does not expose a `Texture::from_raw` method; the correct entry point is `device.as_hal()` + `texture_from_raw_renderbuffer`, and GTK's FBO is not a renderbuffer
- Even when the API was fixed, wgpu manages its own separate OpenGL context — it cannot render into GTK's FBO from a different context
- The `wgpu-hal` version declared in `Cargo.toml` (28.x) conflicted with the version wgpu 22 uses internally (22.x), causing cascading type mismatches (`TextureUses` vs `TextureUsages`, `view_formats: &[]` vs `Vec`, etc.)
- After fixing all compilation errors, the fundamental approach was architecturally unsound

**Lesson:** GTK's GLArea and wgpu's GL backend cannot share a framebuffer. The compositor boundary cannot be crossed this way.

---

### Phase 2 — wgpu-hal HAL Hacks

After Phase 1, attempts were made to go deeper into `wgpu_hal` to directly manipulate the underlying GL objects. This was a dead end — the hal layer is an implementation detail of wgpu, not a stable rendering surface, and bypassing wgpu's resource tracking causes undefined behaviour.

**Lesson:** wgpu is the wrong tool when you need to own the GPU resources yourself. The abstraction works against you.

---

### Phase 3 — Raw Vulkan + DMA-BUF

The correct solution became clear: GTK 4.14 ships `GdkDmabufTextureBuilder` and `GraphicsOffload`. These two widgets together allow any application to:

1. Allocate a Vulkan image whose backing memory is exportable as a Linux DMA-BUF file descriptor
2. Hand that fd to GTK via `DmabufTextureBuilder`
3. Set the resulting `GdkTexture` on a `Picture` widget wrapped in `GraphicsOffload`
4. GTK tells the Wayland compositor about the DMA-BUF, which can then scan it out to the display **without copying it**

This is the path taken by hardware video players (like `gst-plugins-bad`'s `gtk4paintablesink`) and is the intended high-performance path for GTK 4.14+.

The wgpu layer was removed entirely. The project was rewritten around `ash` (raw Vulkan bindings) with full manual control over memory allocation, image layout transitions, command buffers, and fences.

---

## Current Architecture

### Directory Structure

```
iris/
├── Cargo.toml
├── ARCHITECTURE.md          ← this file
└── src/
    ├── main.rs              ← AppState, build_ui, keyboard/mouse handling
    └── viewport/
        ├── mod.rs           ← Viewport widget, GTK integration, trigger_render
        ├── camera.rs        ← Camera: position, zoom, rotation, fit_scale
        ├── shaders/
        │   └── image.wgsl   ← Vertex + fragment shader (WGSL, compiled to SPIR-V)
        └── vk/
            ├── mod.rs       ← Module re-exports
            ├── context.rs   ← VkContext: Vulkan instance, device, queue, command pool
            ├── shader.rs    ← WGSL → SPIR-V compiler (naga)
            ├── pipeline.rs  ← VkPipeline: render pass, descriptor layout, graphics pipeline
            ├── dmabuf.rs    ← DmabufImage: render image (OPTIMAL) + export image (LINEAR)
            └── renderer.rs  ← VkRenderer: full render orchestrator, texture cache
```

### Component Map

```
main.rs (AppState)
    │
    └── Viewport  (viewport/mod.rs)
            │
            ├── Camera         (camera.rs)        — pure math, no GPU
            │
            └── VkRenderer     (vk/renderer.rs)   — owns everything GPU
                    │
                    ├── VkContext    (vk/context.rs)   — Vulkan instance/device/queue
                    ├── VkPipeline   (vk/pipeline.rs)  — render pass, PSO
                    │       └── compile_wgsl  (vk/shader.rs) — naga WGSL→SPIR-V
                    ├── DmabufImage  (vk/dmabuf.rs)    — render + export targets
                    ├── CachedTexture × N              — per-image GPU textures
                    ├── VkBuffer (uniform)             — Uniforms struct (persistently mapped)
                    ├── VkDescriptorPool               — descriptor sets for all textures
                    ├── VkFramebuffer                  — rebuilt on resize
                    ├── VkCommandBuffer                — re-recorded every frame
                    └── VkFence                        — CPU/GPU sync per frame
```

### Data Flow

```
User opens file
    │
    ▼
AppState::load_directory()          scan parent directory, sort, build file list
    │
    ▼
Viewport::load_image(path)
    │
    ├─ Cache hit? ──────────────────► VkRenderer::activate_cached()
    │                                         │
    │                                         ▼
    │                               VkRenderer::render(camera)
    │                                         │
    │                               push_dmabuf_to_picture()  ◄── GTK Picture updated
    │
    └─ Cache miss:
           │
           ▼
    rayon::spawn → image::open()    decode RGBA on worker thread
           │
           ▼ (oneshot channel)
    glib::spawn_future_local        back on GTK main thread
           │
           ▼
    VkRenderer::upload_and_activate()
           │
           ├── staging buffer → vkCmdCopyBufferToImage → DEVICE_LOCAL VkImage
           ├── layout: UNDEFINED → TRANSFER_DST → SHADER_READ_ONLY_OPTIMAL
           ├── allocate descriptor set (uniform + texture + sampler)
           └── activate: set active_path, update image_dims
           │
           ▼
    VkRenderer::render(camera)
           │
           ├── sync_size: resize render target if widget changed size
           ├── write Uniforms (scale, rotation, zoom, pan) → persistently mapped buffer
           ├── begin command buffer
           ├── begin render pass → clear to #0D0D0D
           ├── bind pipeline, descriptor set
           ├── vkCmdDraw(6 vertices) → fullscreen quad → shader transforms image
           ├── end render pass
           ├── submit to queue, signal fence
           ├── wait fence
           └── DmabufImage::blit_render_to_export()
                   │
                   ├── transition render_image: COLOR_ATTACHMENT → TRANSFER_SRC
                   ├── vkCmdBlitImage: render_image → export_image
                   ├── transition render_image: TRANSFER_SRC → COLOR_ATTACHMENT
                   └── transition export_image: TRANSFER_DST → GENERAL
           │
           ▼
    export_fd_for_gtk()             dup() the DMA-BUF fd
           │
           ▼
    push_dmabuf_to_picture()
           │
           ├── GdkDmabufTextureBuilder::new()
           ├── set_width / set_height / set_fourcc / set_modifier
           ├── set_fd(0, fd) / set_stride / set_offset
           └── builder.build() → GdkTexture → Picture::set_paintable()
                                                        │
                                                        ▼
                                               Wayland compositor
                                               scans out DMA-BUF
                                               zero-copy to display
```

---

## Component Deep-Dives

### main.rs — AppState & UI

`AppState` holds the application's navigation state:

- `files: Vec<PathBuf>` — sorted list of all images in the current directory
- `current_index: usize` — which image is active
- `rotations: HashMap<PathBuf, i32>` — per-image rotation in degrees (0/90/180/270), persisted for the session
- `info_visible: bool` — whether the info panel is shown

Key methods:
- `load_directory(path)` — scans the parent directory, filters by image extension, sorts, and finds the index of the opened file
- `next() / prev()` — wrapping index navigation
- `adjacent_paths()` — returns up to 5 paths in each direction for prefetching

`build_ui` constructs the full GTK widget tree: `ToolbarView` → `HeaderBar`, `Stack` (welcome / image), `Viewport`, thumbnail strip, info panel. All keyboard shortcuts (`←/→`, `Space`, `R`, `+/-`, `0`, `F`, `Escape`, `I`) are handled via `EventControllerKey` on the window.

Thumbnail loading is async: `load_bytes_async` reads the file on a rayon thread, `pixbuf_from_bytes` decodes via GdkPixbuf, scaled to 90×90 and set on a `gtk4::Picture` inside a `Stack` (spinner → image).

---

### camera.rs — Camera & Transform

`Camera` holds the view state that gets serialised into the uniform buffer each frame:

```
position: Vec2   — pan offset in NDC space
zoom: f32        — multiplier (0.1 to 50.0)
rotation: f32    — radians
viewport_width / viewport_height: u32
```

`fit_scale(image_width, image_height) -> [f32; 2]` computes the aspect-ratio-correct scale that fits the image into the viewport. It is rotation-aware: at 90°/270° the effective image dimensions are swapped before computing the fit ratio, and the resulting scale components are assigned to the correct axes to account for the shader's rotation swapping `x` and `y`.

This is the most mathematically careful code in the project.

---

### shader — image.wgsl

The shader is a hardcoded fullscreen quad (no vertex buffer — positions are baked into the vertex shader as an array indexed by `vertex_index`).

The vertex shader applies transforms in this order:
1. `scale` — aspect-ratio fit (from `fit_scale`)
2. `rotate2d(rotation)` — rotation matrix
3. `zoom` — scalar zoom
4. `pan` — translation in NDC

The fragment shader is a single `textureSample` call. No tone mapping, no colour space conversion — the output is whatever the texture contains.

The uniform struct layout:
```wgsl
struct Uniforms {
    scale:    vec2<f32>,   // bytes 0–7
    rotation: f32,         // bytes 8–11
    zoom:     f32,         // bytes 12–15
    pan:      vec2<f32>,   // bytes 16–23
    _padding: vec2<f32>,   // bytes 24–31  (std140 alignment)
}
```

The WGSL is compiled to SPIR-V at application startup by `vk/shader.rs` using `naga`. No pre-compiled SPIR-V blobs are shipped.

---

### vk/context.rs — VkContext

`VkContext` is the root Vulkan object. It is wrapped in `Arc` so it can be shared between the pipeline, renderer, and DMA-BUF images.

**Instance extensions enabled:**
- `VK_KHR_get_physical_device_properties2`
- `VK_KHR_external_memory_capabilities`

**Device extensions enabled:**
- `VK_KHR_external_memory`
- `VK_KHR_external_memory_fd`
- `VK_EXT_external_memory_dma_buf`

**GPU selection priority:** discrete GPU → integrated GPU → first available.

**Command pool** is created with `RESET_COMMAND_BUFFER` so the per-frame command buffer can be re-recorded every frame without re-allocation.

**Helper methods:**
- `alloc_command_buffer()` — allocates one PRIMARY buffer from the pool
- `begin_one_shot_commands()` — allocates + begins with `ONE_TIME_SUBMIT`
- `end_one_shot_commands(cmd)` — ends + submits + waits + frees; used for uploads and layout transitions
- `submit_and_wait(cmd)` — submits to the graphics queue with a temporary fence, waits for completion

---

### vk/shader.rs — WGSL → SPIR-V

`compile_wgsl(device, source) -> vk::ShaderModule` is a pure function:

1. `naga::front::wgsl::parse_str` — parses WGSL text into a naga IR module
2. `naga::valid::Validator::validate` — validates the IR, produces `ModuleInfo`
3. `naga::back::spv::Writer::write` — emits SPIR-V words
4. `vkCreateShaderModule` — uploads the words to the driver

The shader module is created once per pipeline construction and destroyed immediately after `vkCreateGraphicsPipelines` (the driver retains its own compiled representation).

---

### vk/pipeline.rs — VkPipeline

`VkPipeline` bakes the entire graphics pipeline state object (PSO). It owns:

- `descriptor_set_layout` — 3 bindings: uniform buffer (vertex+fragment), sampled image (fragment), sampler (fragment)
- `pipeline_layout` — wraps the descriptor set layout
- `render_pass` — single colour attachment, `R8G8B8A8_UNORM`, load=CLEAR, store=STORE, final layout `TRANSFER_SRC_OPTIMAL`
- `pipeline` — the full PSO: no vertex input, triangle list, dynamic viewport+scissor, no culling, CCW winding, premultiplied alpha blending

The premultiplied alpha blend equation:
```
out.rgb = src.rgb * 1  +  dst.rgb * (1 - src.a)
out.a   = src.a   * 1  +  dst.a   * 0
```

---

### vk/dmabuf.rs — DmabufImage

This is the most architecturally important file. It manages two Vulkan images per render target:

#### render_image (OPTIMAL, DEVICE_LOCAL)
- `VK_IMAGE_TILING_OPTIMAL` — the GPU's native tiling; fast for rendering and sampling
- `VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | TRANSFER_SRC_BIT`
- Layout after creation: `COLOR_ATTACHMENT_OPTIMAL`
- This is what the render pass draws into

#### export_image (LINEAR, HOST_VISIBLE, DMA-BUF exportable)
- `VK_IMAGE_TILING_LINEAR` — row-major layout; required for DMA-BUF export
- `VK_IMAGE_USAGE_TRANSFER_DST_BIT` — only written to by blits
- Memory allocated with `VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT`
- The DMA-BUF fd is extracted once at creation and duplicated before each GTK handoff

#### blit_render_to_export()
After each render pass completes:
1. Transition `render_image`: `COLOR_ATTACHMENT_OPTIMAL` → `TRANSFER_SRC_OPTIMAL`
2. `vkCmdBlitImage` from render to export (LINEAR filter)
3. Transition `render_image` back: `TRANSFER_SRC_OPTIMAL` → `COLOR_ATTACHMENT_OPTIMAL`
4. Transition `export_image`: `TRANSFER_DST_OPTIMAL` → `GENERAL` (readable by compositor)
5. Reset `export_image` back to `TRANSFER_DST_OPTIMAL` for next frame

All layout transitions use `image_layout_transition()` — a local helper that emits a `vkCmdPipelineBarrier` with correct src/dst stage masks and access masks.

#### Why two images?
`OPTIMAL` tiling cannot be exported as a DMA-BUF on most drivers — the internal tile layout is undefined and not interpretable by external consumers. `LINEAR` tiling can be exported but is significantly slower for rendering. The blit is a necessary GPU-side copy: fast, happening entirely on the GPU with no CPU involvement.

---

### vk/renderer.rs — VkRenderer

`VkRenderer` is the central orchestrator. It owns:

| Resource | Count | Notes |
|---|---|---|
| `VkPipeline` | 1 | PSO, render pass, descriptor layout |
| `VkDescriptorPool` | 1 | capacity: 64 sets; FREE_DESCRIPTOR_SET flag |
| `VkSampler` | 1 | LINEAR filter, CLAMP_TO_EDGE, shared across all textures |
| Uniform `VkBuffer` + `VkDeviceMemory` | 1 | 32 bytes, persistently CPU-mapped |
| `CachedTexture` | 0–64 | one per cached image |
| `DmabufImage` | 1 | the current render target; rebuilt on resize |
| `VkFramebuffer` | 1 | wraps the render image view; rebuilt on resize |
| `VkCommandBuffer` | 1 | re-recorded every frame |
| `VkFence` | 1 | created SIGNALED; ensures previous frame is done before re-recording |

**`CachedTexture`** holds a DEVICE_LOCAL `VkImage` (OPTIMAL, SHADER_READ_ONLY_OPTIMAL), its `VkImageView`, `VkDeviceMemory`, and a fully-written `VkDescriptorSet` bound to the shared uniform buffer and sampler.

**Upload path** (`upload_rgba_texture`):
1. Allocate a HOST_VISIBLE staging buffer
2. `memcpy` RGBA bytes in
3. One-shot command: transition UNDEFINED→TRANSFER_DST, `vkCmdCopyBufferToImage`, transition TRANSFER_DST→SHADER_READ_ONLY_OPTIMAL
4. Free staging buffer
5. Create image view
6. Allocate and write descriptor set

**Render path** (`render(camera)`):
1. `wait_fence` + `reset_fence`
2. `write_uniforms` — `memcpy` of 32-byte `Uniforms` struct into persistently-mapped buffer
3. `record_and_submit` — begin cmd, begin render pass, bind pipeline+descriptors, draw 6 vertices, end render pass, end cmd, submit with fence
4. `blit_render_to_export` (via `DmabufImage`)

**Cache eviction:** LRU order is maintained in `cache_order: Vec<PathBuf>`. When `cache_memory_used + new_image_bytes > cache_memory_budget`, the oldest entry (excluding the blank placeholder) is evicted: its GPU resources are destroyed and its descriptor set is freed back to the pool.

---

### viewport/mod.rs — Viewport

`Viewport` is the GTK-facing layer. It holds:

- `widget: gtk4::Box` — the root widget to embed in the UI
- `picture: gtk4::Picture` — the paintable surface that receives the `GdkDmabufTexture`
- `_offload: gtk4::GraphicsOffload` — wraps the picture; signals to the compositor that this surface should be composited directly from the DMA-BUF
- `camera: Rc<RefCell<Camera>>`
- `renderer: Rc<RefCell<VkRenderer>>`
- `current_target: Rc<RefCell<Option<PathBuf>>>` — guards against late async decode callbacks
- Drag state cells for pan

**`trigger_render`** is the single internal render entry point:
1. `sync_size` — checks `picture.width/height()` against the renderer's current target size; if different, calls `renderer.resize()` and `camera.set_viewport_size()`
2. `renderer.render(camera)` — runs the Vulkan frame
3. `export_fd_for_gtk()` — dups the DMA-BUF fd
4. `push_dmabuf_to_picture()` — builds a `GdkDmabufTexture` and sets it on the picture

**`push_dmabuf_to_picture`** creates a new `GdkDmabufTextureBuilder` on every call. This is required because GTK takes ownership of the fd.

---

## The DMA-BUF Zero-Copy Pipeline

The full journey from Vulkan memory to display pixels:

```
Vulkan VRAM
  │  export_image: LINEAR, HOST_VISIBLE
  │  Memory allocated with VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT
  │
  ├── vkGetMemoryFdKHR() → raw Linux fd (e.g. fd=7)
  │
  ├── libc::dup(fd) → fd=8  (GTK gets 8, we keep 7)
  │
  ▼
GdkDmabufTextureBuilder
  set_fd(0, 8)
  set_stride(0, stride)
  set_fourcc(DRM_FORMAT_ABGR8888)
  set_modifier(DRM_FORMAT_MOD_LINEAR)
  build() → GdkDmabufTexture
  │
  ▼
gtk4::Picture::set_paintable(texture)
  │
  ▼
gtk4::GraphicsOffload  (GraphicsOffloadEnabled::Enabled)
  │  Tells GTK: "this widget's content is an opaque GPU surface"
  │  GTK does NOT composite this into its own GL framebuffer
  │
  ▼
Wayland protocol: linux-dmabuf-unstable-v1
  wl_buffer from dmabuf fd
  │
  ▼
Wayland compositor (Mutter/KWin/wlroots)
  Imports the dma_buf fd → GPU texture (zero-copy)
  Scans out directly to KMS/DRM plane
  │
  ▼
Monitor  ◄── pixel-perfect, zero intermediate copies
```

---

## Dependency Rationale

| Crate | Version | Role |
|---|---|---|
| `gtk4` | 0.9, v4_14 | UI framework; v4_14 required for `DmabufTextureBuilder` and `GraphicsOffload` |
| `libadwaita` | 0.7 | GNOME HIG widgets (`ApplicationWindow`, `ToolbarView`, `HeaderBar`) |
| `ash` | 0.38 | Raw Vulkan bindings; 0.38 dropped the `builder()` pattern in favour of method chaining on `Default` |
| `naga` | 28.0 | WGSL parser + SPIR-V emitter; `wgsl-in` and `spv-out` features required |
| `image` | 0.25 | Image decoding: JPEG, PNG, WebP, AVIF, TIFF, BMP, GIF, QOI |
| `rayon` | 1 | Thread pool for background image decoding |
| `futures` | 0.3 | `oneshot` channels to bridge rayon threads → GTK async tasks |
| `glib` | 0.22 | `spawn_future_local` for GTK-thread async |
| `bytemuck` | 1.25 | Safe `Pod`/`Zeroable` derive for the `Uniforms` struct |
| `glam` | 0.32 | `Vec2` for camera position |
| `libc` | 0.2 | `dup()` and `close()` for DMA-BUF fd management |
| `drm-fourcc` | 2.2 | DRM pixel format constants (currently unused; kept for future format negotiation) |
| `gdk-pixbuf` | 0.22 | Thumbnail generation only (separate from main render path) |
| `tokio` | 1 | Declared but currently unused; retained for potential future async I/O |

---

## Known Limitations & Open Issues

### Critical

1. **Wayland-only.** The DMA-BUF + `GraphicsOffload` path requires a Wayland compositor that supports `linux-dmabuf-unstable-v1`. On X11, `DmabufTextureBuilder::build()` will fail and the picture will remain blank. There is no X11 fallback.

2. **Sync is blocking.** `blit_render_to_export` calls `end_one_shot_commands` which submits and waits on a fence — this is a GPU pipeline stall. The correct approach is to use a semaphore to signal GTK's texture release callback rather than blocking the CPU.

3. **No display negotiation.** The export image is always `VK_FORMAT_R8G8B8A8_UNORM` / `DRM_FORMAT_ABGR8888`. Not all compositors or displays accept this format; proper implementations query supported formats via `DmabufTextureBuilder::get_display()` and pick from the compositor's advertised list.

4. **Resize is potentially jittery.** `sync_size` is called inside `trigger_render`, which is only called on user interaction. The render target does not automatically resize when the window is resized silently — only on the next interaction event.

5. **Single command buffer.** There is one command buffer that is re-recorded every frame. For maximum throughput, double-buffering (two command buffers + two fences + two DmabufImages) would eliminate the stall between frames.

### Moderate

6. **`expect()` everywhere.** All Vulkan calls panic on failure. A production viewer needs graceful error handling with fallback paths (e.g. Vulkan unavailable → software rendering via GdkPixbuf).

7. **No image max-size clamping.** The old wgpu renderer had `fit_to_gpu_limits` that downscaled images larger than `maxTextureDimension2D`. This is not yet ported to the Vulkan renderer — uploading a 20,000×20,000 TIFF will crash.

8. **Cache budget is hardcoded.** `VkRenderer` uses a fixed 512 MB budget. The old `GpuContext` had adaptive budget detection based on VRAM size; this needs to be ported.

9. **`tokio` declared but unused.** Vestigial from the wgpu era. Can be removed.

10. **`drm-fourcc` declared but unused.** Kept for future format negotiation but not yet wired up.

---

## What Remains to Be Built

### P0 — Must have for first working build

- [ ] **X11 fallback.** Detect whether the Wayland compositor accepted the DMA-BUF texture. If `build()` fails, fall back to rendering to a CPU-mapped buffer and loading it as a `gdk::MemoryTexture`.
- [ ] **Automatic resize handling.** Hook into GTK's layout cycle to detect size changes without requiring a user interaction. Candidate: `picture.connect_notify("width")` / `picture.connect_notify("height")` or embedding a `DrawingArea` purely as a size sensor.
- [ ] **Max texture size clamping.** Port `fit_to_gpu_limits` from the old renderer. Query `VkPhysicalDeviceLimits::maxImageDimension2D` from the device and downscale before upload.
- [ ] **Adaptive cache budget.** Query available VRAM from `/sys/class/drm/*/device/mem_info_vram_total` or Vulkan memory heap sizes. Use a percentage of available VRAM.

### P1 — Required for production quality

- [ ] **Async GPU sync.** Replace the blocking `submit_and_wait` in `blit_render_to_export` with a proper semaphore-based release callback using `GdkDmabufTextureBuilder::build_with_release_func`. The release func fires when the compositor is done with the buffer, allowing the next frame to begin without a CPU stall.
- [ ] **Double-buffering.** Two `DmabufImage` instances + two command buffers + two fences, ping-ponged each frame. While the compositor is scanning out frame N, the GPU is rendering frame N+1.
- [ ] **Error handling.** Replace all `.expect()` calls in the Vulkan path with `Result`-returning functions. Surface errors to the user as an `adw::Toast` rather than a panic.
- [ ] **Format negotiation.** Query the display's supported DMA-BUF formats via `GdkDisplay::dmabuf_formats()` and select the best matching `VkFormat`. Currently hardcoded to `R8G8B8A8_UNORM` / `DRM_FORMAT_ABGR8888`.
- [ ] **Mipmap generation.** For very large images displayed at small zoom levels, generate mipmaps at upload time (`vkCmdBlitImage` cascade) and use `LINEAR_MIPMAP_LINEAR` filtering. Eliminates aliasing on downscaled images.

### P2 — Quality of life

- [ ] **EXIF rotation.** Read EXIF orientation tags from JPEG/TIFF files and auto-rotate on load. Currently the user must rotate manually.
- [ ] **Animated GIF / WebP.** The `image` crate can decode animation frames. A timer-driven frame advance loop would feed new RGBA data to `VkRenderer::upload_and_activate` each tick.
- [ ] **Colour management.** HDR displays and wide-gamut monitors need proper ICC profile application. GTK 4.14 exposes `GdkColorState`; the fragment shader would need a colour space transform pass.
- [ ] **Per-image zoom memory.** Currently zoom and pan reset on every image navigation. Storing `(zoom, position)` per path in `AppState` would preserve the view when returning to a previously seen image.
- [ ] **Drag-to-open / file manager integration.** Accept `GtkDropTarget` drops and respond to `org.freedesktop.FileManager1` D-Bus activation.
- [ ] **Settings persistence.** Save window size, last opened directory, thumbnail strip visibility to a `~/.config/iris/config.toml`.
- [ ] **Remove unused dependencies.** `tokio` is declared but never used. Remove it. Wire up `drm-fourcc` properly for format negotiation or remove it too.

---

## Future Vision

The architecture as built is the correct foundation for several ambitious features that are not yet started:

### True zero-latency navigation
With double-buffering and prefetch, navigation to adjacent images could be instant: by the time the user presses `→`, the next image is already decoded, uploaded to a cached `CachedTexture`, and a rendered frame is sitting in a `DmabufImage` waiting to be handed to GTK. The only work on the hot path is updating the uniform buffer and submitting one command buffer.

### GPU-accelerated image processing
Because the render path is fully custom Vulkan, additional processing passes can be inserted before the final blit:
- A histogram-equalisation pass for exposure correction
- A sharpening pass (unsharp mask via a compute shader)
- A noise-reduction pass for high-ISO photos
- A chroma subsampling correction pass for JPEG artefact reduction

All of these would operate on the DEVICE_LOCAL render image and add zero CPU overhead.

### RAW camera file support
`libraw` (via FFI) can decode camera RAW files (CR2, NEF, ARW, DNG) to linear 16-bit RGBA. The Vulkan pipeline would need a `VK_FORMAT_R16G16B16A16_UNORM` texture path and a tone-mapping pass in the fragment shader (ACES or Reinhard). This would make Iris the fastest RAW previewer on Linux.

### Multi-monitor / HDR
The `DmabufTextureBuilder` path is per-surface. On a multi-monitor setup with one HDR and one SDR display, GTK routes the surface to the correct compositor plane. Iris would just need to set the correct `GdkColorState` on the texture builder and let the compositor handle the rest.

### Thumbnail cache on disk
A persistent thumbnail cache (following the [Freedesktop Thumbnail Managing Standard](https://specifications.freedesktop.org/thumbnail-spec/latest/)) would make cold-start directory loading instant. Thumbnails stored as PNG under `~/.cache/thumbnails/` keyed by URI + mtime.

---

*Last updated to reflect the state of the codebase after Phase 3 (Vulkan + DMA-BUF) implementation. All code compiles clean with zero errors under Rust 1.94 / edition 2024.*
