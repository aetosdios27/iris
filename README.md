## About

Iris is a high-performance Wayland image viewer built with Rust, GTK4, and raw Vulkan. It's designed for maximum speed by employing a zero-copy GPU pipeline, aiming to be the fastest image viewer on Linux.

## Built With

*   **Rust:** The core programming language for performance and safety.
*   **GTK4 & libadwaita:** For a modern, native GNOME user interface and efficient graphics offloading.
*   **Vulkan (ash):** Low-level GPU access for fine-grained control over memory and DMA-BUF export.
*   **WGSL (naga):** Ergonomic shader language compiled to SPIR-V for cross-platform compatibility.
*   **DMA-BUF:** Enables zero-copy image display for maximum efficiency.

## Getting Started

This section guides you through setting up your development environment and building the `iris` project.

### Prerequisites

*   **Rust:** Ensure you have Rust and Cargo installed. [Install Rust](https://www.rust-lang.org/tools/install)
*   **Vulkan SDK:** Required for Vulkan development. [Download Vulkan SDK](https://vulkan.lunarg.com/sdk/home)
*   **GTK4 & libadwaita:** Development libraries for GTK4 and libadwaita. Installation varies by OS.

### Installation

1.  **Clone the repository:**
    ```bash
    git clone https://github.com/yourusername/iris.git
    cd iris
    ```
2.  **Build the project:**
    ```bash
    cargo build
    ```
3.  **Run the application:**
    ```bash
    cargo run
    ```

## Usage

Launch Iris by providing an image file path as a command-line argument.

```bash
iris /path/to/your/image.jpg
```

### Basic Navigation

*   **Pan:** Click and drag the image.
*   **Zoom:** Scroll your mouse wheel.
*   **Fit to View:** Press `F`.
*   **Reset View:** Press `R`.

### Features

Iris utilizes a zero-copy GPU pipeline for fast rendering. It supports common image formats and offers smooth, hardware-accelerated navigation.

# Contributing

We welcome contributions! Please follow these guidelines to help us maintain a consistent and high-quality project.

## Reporting Bugs

*   Clearly describe the bug and the steps to reproduce it.
*   Include your operating system and `iris` version.
*   If possible, provide relevant logs or screenshots.

## Suggesting Features

*   Open an issue to discuss your idea before submitting a pull request.
*   Explain the problem the feature solves and how it would be used.

## Pull Requests

*   Ensure your code follows the project's coding style.
*   Write clear commit messages.
*   Open a pull request with a descriptive title and summary of changes.
*   All contributions are subject to review.

## Architecture

### Directory Structure

*   `src/`: Contains the core application logic.
    *   `main.rs`: Manages application state and UI setup, handles user input.
    *   `viewport/`: Handles the image display and rendering.
        *   `mod.rs`: The main viewport widget, integrating with GTK and triggering renders.
        *   `camera.rs`: Manages camera properties like position, zoom, and rotation.
        *   `shaders/`: Contains shader code.
            *   `image.wgsl`: WGSL shader for image rendering, compiled to SPIR-V.
        *   `vk/`: Vulkan-specific implementation details.
            *   `context.rs`: Manages the Vulkan rendering context.
            *   `shader.rs`: Handles WGSL to SPIR-V compilation.

### Component Map

*   **GTK4 UI:** Provides the window and user interface elements.
*   **`AppState` (`main.rs`):** Holds the overall application state and orchestrates components.
*   **`Viewport` (`viewport/mod.rs`):** The core widget responsible for displaying the image. It receives input and signals the need for rendering.
*   **`Camera` (`viewport/camera.rs`):** Manages image transformations (pan, zoom, rotate).
*   **Vulkan Backend (`viewport/vk/`):** Handles low-level GPU rendering using Vulkan.
    *   `VkContext`: Initializes and manages Vulkan resources.
    *   Shader Compilation: Converts WGSL shaders to SPIR-V for the GPU.
*   **DMA-BUF:** Used for efficient, zero-copy image data transfer to the GPU.

### Data Flow

1.  **Image Loading:** An image file is loaded into memory.
2.  **Input Handling:** User input (keyboard, mouse) is captured by the GTK UI and passed to `AppState`.
3.  **State Update:** `AppState` updates the `Camera` or other relevant state based on input.
4.  **Render Trigger:** The `Viewport` widget is notified of state changes and triggers a render.
5.  **Vulkan Rendering:** The Vulkan backend prepares rendering commands.
    *   Image data is efficiently transferred to the GPU (e.g., via DMA-BUF).
    *   Shaders are compiled and applied.
    *   The image is rendered onto the screen.

## Future Vision

Iris is built on a foundation designed for ambitious, zero-latency features. Our long-term goals focus on pushing the boundaries of image viewing performance.

*   **True Zero-Latency Navigation:** Aiming for instant transitions between adjacent images. This involves prefetching and double-buffering to ensure the next image is decoded, uploaded to the GPU, and ready for display the moment you navigate.
*   **Enhanced GPU Pipeline:** Further optimizing the zero-copy GPU pipeline for maximum efficiency. This means images are uploaded to the GPU only once, processed through a custom Vulkan pipeline, and seamlessly handed off for rendering.

## Known Limitations & Open Issues

*   **Error Handling:** Vulkan calls currently panic on failure. Production builds require graceful error handling and fallback mechanisms (e.g., software rendering if Vulkan is unavailable).
*   **Image Size Limits:** The Vulkan renderer does not yet clamp image sizes to GPU limits. Uploading extremely large images (e.g., 20,000x20,000 pixels) may cause crashes.
*   **Cache Budget:** The cache budget for the Vulkan renderer is currently hardcoded and needs to be made configurable or dynamically managed.

---

*This README was generated by [DevDoq](https://devdoq.com)*