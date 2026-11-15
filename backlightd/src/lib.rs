#![doc = include_str!("docs/index.md")]
#![warn(missing_docs)]

/// Shared command types, status records, and bounded JSON serialization.
pub use backlight_ipc as ipc;

/// Process ownership, serialization, hotplug, and shutdown invariants.
pub mod architecture {
    #![doc = include_str!("docs/architecture.md")]
}

/// Logical brightness, hardware quantization, calibration, and readback semantics.
pub mod brightness {
    #![doc = include_str!("docs/brightness.md")]
}

/// Validated configuration snapshots, reload ordering, and location selection.
pub mod configuration {
    #![doc = include_str!("docs/configuration.md")]
}

/// Source layout, tests, and documentation build instructions.
pub mod development {
    #![doc = include_str!("docs/development.md")]
}
