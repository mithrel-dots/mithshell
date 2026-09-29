//! Allocation policy for the long-lived daemon, including its native libraries.

pub(super) fn configure_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // Preserve explicit administrator/benchmark allocator tuning. glibc
        // reads these at process startup, before we can query its policy.
        if std::env::vars_os().any(|(key, value)| {
            key.as_encoded_bytes().starts_with(b"MALLOC_")
                || (key == "GLIBC_TUNABLES"
                    && value
                        .to_string_lossy()
                        .split(':')
                        .any(|entry| entry.starts_with("glibc.malloc.")))
        }) {
            return;
        }

        // glibc starts with a 128 KiB mmap threshold, but dynamically raises
        // it (and the trim threshold) after large temporary allocations. GPU
        // initialization and image decoding can thus leave large allocations
        // mixed into the small-object heap for the rest of the session. Keep
        // large buffers independently releasable rather than periodically
        // trimming the heap on GTK's frame thread. Small allocations keep
        // their normal arenas and thread caches.
        // SAFETY: mallopt accepts these documented integer tuning parameters;
        // this runs once at daemon startup, before GTK and our workers start.
        let configured = unsafe {
            let mmap = libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024);
            let trim = libc::mallopt(libc::M_TRIM_THRESHOLD, 256 * 1024);
            mmap != 0 && trim != 0
        };
        log::debug!("daemon allocator thresholds configured: {configured}");
    }
}
