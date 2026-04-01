use std::path::Path;

fn is_supported_image(path: &Path) -> bool {
    let is_standard = matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "tiff" | "bmp")
    );
    is_standard || iris::raw::is_raw(path)
}

#[test]
fn supported_standard_extensions_are_recognized() {
    let yes = [
        "a.jpg", "a.jpeg", "a.png", "a.gif", "a.webp", "a.avif", "a.tiff", "a.bmp",
    ];

    for p in yes {
        assert!(is_supported_image(Path::new(p)), "{p} should be supported");
    }
}

#[test]
fn supported_raw_extensions_are_recognized() {
    let yes = [
        "a.cr2", "a.cr3", "a.nef", "a.arw", "a.raf", "a.orf", "a.rw2", "a.pef", "a.dng",
    ];

    for p in yes {
        assert!(is_supported_image(Path::new(p)), "{p} should be supported");
    }
}

#[test]
fn unsupported_extensions_are_rejected() {
    let no = ["a.txt", "a.md", "a.rs", "a.mp4", "a.mov", "a.zip", "a"];

    for p in no {
        assert!(
            !is_supported_image(Path::new(p)),
            "{p} should not be supported"
        );
    }
}
