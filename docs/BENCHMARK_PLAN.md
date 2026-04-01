# Benchmark Plan

## Purpose

Benchmarking exists to validate product claims, not to generate marketing theater.

The benchmark plan should answer:

- is Iris actually faster in meaningful workflows
- where is it faster
- where is it merely competitive
- how much of the gain comes from zero-copy and caching

## Competitors

Primary comparison set:

- eog
- feh
- Gwenview
- Nomacs

Optional secondary comparison set:

- any other Linux-native image viewer that is commonly cited by users

## Benchmark Categories

### 1. Cold Open Latency

Measure:

- time from launching/opening an image to first visible frame

Purpose:

- validate startup/open responsiveness

### 2. Warm Adjacent Navigation

Measure:

- time to move between adjacent images in a directory

Purpose:

- validate cache and upload strategy

### 3. Rapid Scrubbing

Measure:

- responsiveness under repeated left/right navigation

Purpose:

- validate coalescing, prefetching, and cache policy

### 4. Thumbnail Population

Measure:

- time to first visible thumbnails
- time to fully populate thumbnail strip for a directory

Purpose:

- validate browsing usability, not just single-image rendering

### 5. Large Image Handling

Measure:

- open latency
- navigation latency
- memory growth

Image sizes:

- large JPEG/PNG
- large TIFF
- large RAW

Purpose:

- validate the app under stressful but realistic workloads

### 6. Long Session Resource Behavior

Measure:

- memory growth over many directory traversals
- stability under extended browsing

Purpose:

- catch slow degradation not visible in microbenchmarks

## Environment Recording

Every benchmark run must record:

- CPU
- GPU
- RAM
- Linux distro
- desktop environment
- session type
- compositor
- kernel version
- Mesa/Nvidia driver version when relevant

## Render Mode Recording

Every benchmark result must state:

- zero-copy DMA-BUF path active
- GPU fallback path active
- software fallback path active

Without that, the numbers are ambiguous.

## Image Corpus

The benchmark corpus should include:

- small web-sized JPEGs
- medium DSLR-sized JPEGs
- PNG with alpha
- TIFF
- RAW camera files
- animated GIFs if retained in V1 scope

The corpus should be versioned or at least fixed and documented.

## Methodology Rules

- run cold and warm cases separately
- do not mix one-shot timing with steady-state timing
- repeat enough times to smooth noise
- report median and spread, not just best case
- ensure the app is not timing against preloaded artifacts without disclosure

## Suggested Output Table

For each competitor and each scenario, capture:

- median latency
- p95 latency if possible
- peak memory during run
- render mode
- notes on anomalies

## Internal Questions To Answer

- how much does zero-copy help relative to GPU fallback
- how much does prefetch help
- how much do thumbnails cost
- where do competitors still beat Iris

## Publication Rule

Only publish benchmarks that can survive replication attempts by skeptical technical users.
