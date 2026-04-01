# Support Matrix

## Support Policy

Iris V1 is a Linux application with tiered rendering support.

This is the only honest way to support a broad Linux audience while still pursuing the zero-copy fast path.

## Support Tiers

### Tier 1: Preferred Path

Environment:

- Linux
- Wayland
- GTK 4.14 capable environment
- Vulkan available
- compositor and driver accept the negotiated DMA-BUF path

Expected behavior:

- GPU rendering
- DMA-BUF presentation
- zero-copy path active where negotiated successfully

Product stance:

- this is the best supported performance configuration

### Tier 2: GPU Fallback

Environment:

- Linux
- Vulkan available
- zero-copy path unavailable or rejected

Expected behavior:

- GPU rendering still available
- presentation falls back to a slower path
- app remains functionally correct

Product stance:

- supported for correctness
- may not achieve flagship performance numbers

### Tier 3: Software Fallback

Environment:

- Linux
- Vulkan unavailable or renderer initialization fails

Expected behavior:

- app remains usable
- image viewing correctness is prioritized over performance

Product stance:

- compatibility fallback
- not representative of flagship performance

## Initial Validation Targets

Minimum matrix:

- GNOME Wayland on Intel
- GNOME Wayland on AMD
- KDE Wayland on Intel or AMD
- one environment where the fast path is not active, to validate fallback behavior

Stretch matrix:

- Nvidia on Wayland
- mixed iGPU/dGPU laptop
- X11-based session

## Unsupported Or Unverified Until Proven

Until validated, do not imply strong support for:

- all Wayland compositors
- all modifier combinations
- all GPU vendors
- all X11 environments
- every distro packaging path

## Runtime Reporting

The app should expose at minimum:

- selected render backend
- whether DMA-BUF presentation is active
- whether a fallback path is active

This can start as logging and later become a debug/about panel.

## User-Facing Language

Recommended claim:

"Iris uses zero-copy GPU presentation on supported Linux systems and falls back gracefully when that path is unavailable."

Avoid claiming:

"Iris always uses zero-copy on Linux."

## Support Matrix Maintenance

This document should be updated every time:

- a new environment is validated
- a known broken environment is discovered
- render path assumptions change
- packaging strategy changes
